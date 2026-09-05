use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::entry::{Entry, StoredValue};

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
    last_sequence: u64,
    sstables: Vec<SSTable>,
    db_dir: PathBuf,
    next_table_id: u32,
}

impl Db {
    pub fn open(db_dir: PathBuf) -> Result<Self> {
        std::fs::DirBuilder::new().recursive(true).create(&db_dir)?;
        let wal_path = Path::new(&db_dir).join("wal.db");

        let mut wal = Wal::open(wal_path)?;
        let records = wal.read_records()?;

        let mut table_list = Vec::new();
        for entry in std::fs::read_dir(&db_dir)? {
            let table_path = entry?.path();
            if table_path.extension() != Some(OsStr::new("sst")) {
                continue;
            }

            let id: u32 = table_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse().ok())
                .ok_or_else(|| Error::CorruptSstable("Invalid SSTable filename".into()))?;
            let file_path = Path::new(table_path.as_path());
            let table = SSTable::open(file_path, id)?;
            table_list.push(table);
        }
        table_list.sort_by_key(SSTable::id);
        let next_table_id = table_list.last().map_or(1, |table| table.id() + 1);
        let last_sequence = table_list
            .iter()
            .map(SSTable::last_sequence)
            .max()
            .unwrap_or(0);

        let mut db = Self {
            wal,
            memtable: Memtable::default(),
            last_sequence,
            sstables: table_list,
            db_dir,
            next_table_id,
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
            db.last_sequence = db.last_sequence.max(last_sequence);
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
            .last_sequence
            .checked_add(1)
            .ok_or(Error::SequenceExhausted)?;
        let final_sequence = self
            .last_sequence
            .checked_add(operation_count)
            .ok_or(Error::SequenceExhausted)?;

        self.wal.append(first_sequence, &batch)?;
        let mut sequence = self.last_sequence;
        for operation in batch.operations {
            sequence += 1;
            match operation {
                WriteOp::Put { key, value } => self.memtable.put(sequence, key, value),
                WriteOp::Delete { key } => self.memtable.delete(sequence, key),
            }
        }
        self.last_sequence = final_sequence;

        if self.memtable_limit_exceeded() {
            self.flush_memtable()?;
        }

        Ok(())
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    fn flush_memtable(&mut self) -> Result<()> {
        if self.memtable.is_empty() {
            return Ok(());
        }

        let table = SSTable::create(
            &self.db_dir,
            self.next_table_id,
            &self.memtable,
            self.last_sequence,
        )?;
        self.sstables.push(table);
        self.next_table_id = self
            .next_table_id
            .checked_add(1)
            .ok_or(Error::TableIdExhausted)?;
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

        let last_sequence = self
            .sstables
            .iter()
            .map(SSTable::last_sequence)
            .max()
            .unwrap_or(0);

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

        let replacement =
            SSTable::create(&self.db_dir, self.next_table_id, &compacted, last_sequence)?;
        self.next_table_id = self
            .next_table_id
            .checked_add(1)
            .ok_or(Error::TableIdExhausted)?;

        let old_paths: Vec<_> = self
            .sstables
            .iter()
            .map(|table| table.path(&self.db_dir))
            .collect();
        let old_tables = std::mem::replace(&mut self.sstables, vec![replacement]);
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Db;
    use crate::{Key, WriteBatch};

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
}
