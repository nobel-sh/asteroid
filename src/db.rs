use std::path::Path;

use super::{
    error::{Error, Result},
    memtable::{Key, Lookup, Memtable, Value},
    table::SSTable,
    wal::Wal,
    write::{WriteBatch, WriteOp},
};

const MEMTABLE_LIMIT: usize = 4;

#[derive(Debug)]
pub struct Db {
    wal: Wal,
    memtable: Memtable,
    last_sequence: u64,
    sstable: SSTable,
}

impl Db {
    pub fn open(wal_path: impl AsRef<Path>, sst_path: impl AsRef<Path>) -> Result<Self> {
        let mut wal = Wal::open(wal_path)?;
        let records = wal.read_records()?;
        let sstable = SSTable::new(sst_path)?;
        let mut db = Self {
            wal,
            memtable: Memtable::default(),
            last_sequence: 0,
            sstable,
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
            db.last_sequence = last_sequence;
        }
        Ok(db)
    }

    pub fn get(&self, key: &Key) -> Option<&[u8]> {
        match self.memtable.lookup(key) {
            Lookup::Value(value) => Some(value),
            Lookup::Deleted => None,
            Lookup::NotFound => match self.sstable.lookup(key) {
                Lookup::Value(value) => Some(value),
                Lookup::Deleted | Lookup::NotFound => None,
            },
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

        if self.memtable_limit_exceeded() {
            self.sstable.write_memtable(&self.memtable)?;
            self.memtable = Memtable::default();
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
        Ok(())
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Db;
    use crate::{Key, WriteBatch};

    fn paths() -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("db-{}-{unique}", std::process::id()));
        (base.with_extension("wal"), base.with_extension("sst"))
    }

    #[test]
    fn batch_operations_are_applied_in_order() {
        let (wal_path, sst_path) = paths();
        let mut db = Db::open(&wal_path, &sst_path).unwrap();
        let mut batch = WriteBatch::new();
        batch.put("key", "first");
        batch.delete("key");
        batch.put("key", "last");

        db.write(batch).unwrap();

        assert_eq!(db.get(&Key::from("key")), Some(b"last".as_slice()));
        assert_eq!(db.last_sequence(), 3);

        drop(db);
        fs::remove_file(wal_path).unwrap();
        fs::remove_file(sst_path).unwrap();
    }

    #[test]
    fn recovers_from_wal() {
        let (wal_path, sst_path) = paths();

        let mut db = Db::open(&wal_path, &sst_path).unwrap();
        db.put("kept", "value").unwrap();
        db.put("deleted", "value").unwrap();
        db.delete("deleted").unwrap();
        drop(db);

        let db = Db::open(&wal_path, &sst_path).unwrap();

        assert_eq!(db.get(&Key::from("kept")), Some(b"value".as_slice()));
        assert_eq!(db.get(&Key::from("deleted")), None);
        assert_eq!(db.last_sequence(), 3);

        drop(db);
        fs::remove_file(wal_path).unwrap();
        fs::remove_file(sst_path).unwrap();
    }
}
