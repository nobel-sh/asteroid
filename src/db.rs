use super::{
    error::{Error, Result},
    memtable::{Key, Memtable, Value},
    wal::Wal,
    write::{WriteBatch, WriteOp},
};

#[derive(Debug)]
pub struct Db {
    wal: Wal,
    memtable: Memtable,
    last_sequence: u64,
}

impl Db {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            wal: Wal::open(path)?,
            memtable: Memtable::default(),
            last_sequence: 0,
        })
    }

    pub fn get(&self, key: &Key) -> Option<&[u8]> {
        self.memtable.get(key)
    }

    pub fn put(&mut self, key: impl Into<Key>, value: impl Into<Value>) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        self.write(batch)
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
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Db;
    use crate::{Key, WriteBatch};

    #[test]
    fn batch_operations_are_applied_in_order() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("db-{}-{unique}.wal", std::process::id()));
        let mut db = Db::open(&path).unwrap();
        let mut batch = WriteBatch::new();
        batch.put("key", "first");
        batch.delete("key");
        batch.put("key", "last");

        db.write(batch).unwrap();

        assert_eq!(db.get(&Key::from("key")), Some(b"last".as_slice()));
        assert_eq!(db.last_sequence(), 3);

        drop(db);
        fs::remove_file(path).unwrap();
    }
}
