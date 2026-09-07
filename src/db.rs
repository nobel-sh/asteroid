use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use crate::{
    entry::{Entry, StoredValue},
    manifest::Manifest,
};

use super::{
    entry::{Key, Lookup, Value},
    error::{Error, Result},
    memtable::Memtable,
    table::SSTable,
    wal::Wal,
    write::{WriteBatch, WriteOp},
};

const MEMTABLE_LIMIT: usize = 4;
const COMPACTION_TRIGGER: usize = 2;

#[derive(Debug)]
pub struct Db {
    wal: Wal,
    memtable: Memtable,
    manifest: Manifest,
    sstables: Vec<SSTable>,
    db_dir: PathBuf,
    _lockfile: File,
}

impl Db {
    pub fn open(db_dir: PathBuf) -> Result<Self> {
        std::fs::DirBuilder::new().recursive(true).create(&db_dir)?;
        let wal_path = Path::new(&db_dir).join("wal.db");
        let lockfile_path = Path::new(&db_dir).join("LOCKFILE");
        let _lockfile = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lockfile_path)?;

        match _lockfile.try_lock() {
            Err(TryLockError::WouldBlock) => return Err(Error::DbLocked),
            Err(TryLockError::Error(e)) => return Err(e.into()),
            Ok(()) => {}
        };

        let mut wal = Wal::open(wal_path)?;
        let records = wal.read_records()?;

        let mut discovered = discover_sstables(&db_dir)?;

        let (manifest, table_list) = match Manifest::load(&db_dir)? {
            Some(manifest) => {
                let mut tables = Vec::new();
                for id in &manifest.table_ids {
                    let path = discovered.remove(id).ok_or_else(|| {
                        Error::CorruptManifest(format!("live SSTable {id} is missing"))
                    })?;
                    tables.push(SSTable::open(&path, *id)?);
                }

                let had_orphans = !discovered.is_empty();
                for path in discovered.into_values() {
                    std::fs::remove_file(path)?;
                }
                if had_orphans {
                    std::fs::File::open(&db_dir)?.sync_all()?;
                }
                (manifest, tables)
            }
            None => {
                if !discovered.is_empty() {
                    return Err(Error::CorruptManifest(
                        "SSTables exist but MANIFEST is missing".into(),
                    ));
                }

                let manifest = Manifest {
                    last_sequence: 0,
                    next_table_id: 1,
                    table_ids: Vec::new(),
                };
                manifest.store(&db_dir)?;
                (manifest, Vec::new())
            }
        };

        let mut db = Self {
            wal,
            memtable: Memtable::default(),
            manifest,
            sstables: table_list,
            db_dir,
            _lockfile,
        };

        for record in records {
            if record.batch.is_empty() {
                continue;
            }
            let operation_count =
                u64::try_from(record.batch.len()).map_err(|_| Error::SequenceExhausted)?;
            let last_sequence = record
                .first_sequence
                .checked_add(operation_count - 1)
                .ok_or(Error::SequenceExhausted)?;
            db.memtable
                .apply_batch(&record.batch, record.first_sequence);
            db.manifest.last_sequence = db.manifest.last_sequence.max(last_sequence);
        }
        Ok(db)
    }

    pub fn get(&self, key: &Key) -> Result<Option<Value>> {
        match self.memtable.lookup(key) {
            Lookup::Value(value) => Ok(Some(Value::from(value))),
            Lookup::Deleted => Ok(None),
            Lookup::NotFound => {
                for table in self.sstables.iter().rev() {
                    match table.lookup(key)? {
                        Lookup::Value(value) => return Ok(Some(value)),
                        Lookup::Deleted => return Ok(None),
                        Lookup::NotFound => {}
                    }
                }
                Ok(None)
            }
        }
    }

    pub fn put(&mut self, key: impl Into<Key>, value: impl Into<Value>) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        self.write(batch)
    }

    fn memtable_limit_exceeded(&self) -> bool {
        self.memtable.entry_count() >= MEMTABLE_LIMIT
    }

    pub fn delete(&mut self, key: impl Into<Key>) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.delete(key);
        self.write(batch)
    }

    pub fn write(&mut self, batch: WriteBatch) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        let operation_count =
            u64::try_from(batch.operations.len()).map_err(|_| Error::SequenceExhausted)?;
        let first_sequence = self
            .manifest
            .last_sequence
            .checked_add(1)
            .ok_or(Error::SequenceExhausted)?;
        let final_sequence = self
            .manifest
            .last_sequence
            .checked_add(operation_count)
            .ok_or(Error::SequenceExhausted)?;

        self.wal.append(first_sequence, &batch)?;
        let mut sequence = self.manifest.last_sequence;
        for operation in batch.operations {
            sequence += 1;
            match operation {
                WriteOp::Put { key, value } => self.memtable.put(sequence, key, value),
                WriteOp::Delete { key } => self.memtable.delete(sequence, key),
            }
        }
        self.manifest.last_sequence = final_sequence;

        if self.memtable_limit_exceeded() {
            self.flush_memtable()?;
        }

        Ok(())
    }

    pub fn last_sequence(&self) -> u64 {
        self.manifest.last_sequence
    }

    fn flush_memtable(&mut self) -> Result<()> {
        if self.memtable.is_empty() {
            return Ok(());
        }

        let table_id = self.manifest.next_table_id;
        let next_table_id = table_id.checked_add(1).ok_or(Error::TableIdExhausted)?;
        let table = SSTable::create(&self.db_dir, table_id, &self.memtable)?;
        let mut next_manifest = self.manifest.clone();
        next_manifest.table_ids.push(table_id);
        next_manifest.next_table_id = next_table_id;
        next_manifest.store(&self.db_dir)?;

        self.sstables.push(table);
        self.manifest = next_manifest;
        self.wal.reset()?;
        self.memtable.clear();
        if self.sstables.len() >= COMPACTION_TRIGGER {
            self.compact()?;
        }
        Ok(())
    }

    fn compact(&mut self) -> Result<()> {
        if self.sstables.len() < 2 {
            return Ok(());
        }

        let mut merged: BTreeMap<Key, Entry> = BTreeMap::new();
        for table in &self.sstables {
            for (key, entry) in table.read_all_entries()? {
                if merged
                    .get(&key)
                    .is_none_or(|current| entry.sequence > current.sequence)
                {
                    merged.insert(key, entry);
                }
            }
        }

        merged.retain(|_, entry| !matches!(entry.value, StoredValue::Tombstone));
        let mut compacted = Memtable::default();
        for (key, entry) in merged {
            compacted.apply(key, entry);
        }

        let replacement_id = self.manifest.next_table_id;
        let next_table_id = replacement_id
            .checked_add(1)
            .ok_or(Error::TableIdExhausted)?;
        let replacement = SSTable::create(&self.db_dir, replacement_id, &compacted)?;
        let mut next_manifest = self.manifest.clone();
        next_manifest.table_ids = vec![replacement_id];
        next_manifest.next_table_id = next_table_id;
        next_manifest.store(&self.db_dir)?;

        let old_paths: Vec<_> = self
            .sstables
            .iter()
            .map(|table| table.path(&self.db_dir))
            .collect();
        let old_tables = std::mem::replace(&mut self.sstables, vec![replacement]);
        self.manifest = next_manifest;
        drop(old_tables);

        for path in old_paths {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        std::fs::File::open(&self.db_dir)?.sync_all()?;
        Ok(())
    }
}

fn discover_sstables(directory: &Path) -> Result<BTreeMap<u32, PathBuf>> {
    let mut tables = BTreeMap::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension() != Some(OsStr::new("sst")) {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse().ok())
            .ok_or_else(|| Error::CorruptSstable("invalid SSTable filename".into()))?;
        tables.insert(id, path);
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Db;
    use crate::{Error, Key, WriteBatch};

    fn path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("db-{}-{unique}", std::process::id()))
    }

    #[test]
    fn recovers_multiple_sstables_and_wal() {
        let path = path();
        let mut db = Db::open(path.clone()).unwrap();

        let mut first = WriteBatch::new();
        first.put("a", "old");
        first.put("b", "value");
        first.put("c", "value");
        first.put("d", "value");
        db.write(first).unwrap();

        let mut second = WriteBatch::new();
        second.put("a", "new");
        second.delete("b");
        second.put("e", "value");
        second.put("f", "value");
        db.write(second).unwrap();
        drop(db);

        let mut db = Db::open(path.clone()).unwrap();
        assert_eq!(db.get(&Key::from("a")).unwrap(), Some("new".into()));
        assert_eq!(db.get(&Key::from("b")).unwrap(), None);
        assert_eq!(db.get(&Key::from("c")).unwrap(), Some("value".into()));
        assert_eq!(db.get(&Key::from("e")).unwrap(), Some("value".into()));
        assert_eq!(db.last_sequence(), 8);

        db.put("wal-only", "value").unwrap();
        drop(db);

        let db = Db::open(path.clone()).unwrap();
        assert_eq!(
            db.get(&Key::from("wal-only")).unwrap(),
            Some("value".into())
        );
        assert_eq!(db.last_sequence(), 9);

        drop(db);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rejects_sstables_without_manifest() {
        let path = path();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("1.sst"), []).unwrap();

        assert!(matches!(
            Db::open(path.clone()),
            Err(Error::CorruptManifest(_))
        ));

        fs::remove_dir_all(path).unwrap();
    }
}
