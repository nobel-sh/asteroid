use super::{
    error::{Error, Result},
    memtable::{Key, Memtable, Value},
    write::{WriteBatch, WriteOp},
};

#[derive(Debug, Default)]
pub struct Db {
    memtable: Memtable,
    last_sequence: u64,
}

impl Db {
    pub fn new() -> Self {
        Self::default()
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
        let operation_count =
            u64::try_from(batch.operations.len()).map_err(|_| Error::SequenceExhausted)?;
        let final_sequence = self
            .last_sequence
            .checked_add(operation_count)
            .ok_or(Error::SequenceExhausted)?;

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
    use super::Db;
    use crate::{Key, WriteBatch};

    #[test]
    fn batch_operations_are_applied_in_order() {
        let mut db = Db::new();
        let mut batch = WriteBatch::new();
        batch.put("key", "first");
        batch.delete("key");
        batch.put("key", "last");

        db.write(batch).unwrap();

        assert_eq!(db.get(&Key::from("key")), Some(b"last".as_slice()));
        assert_eq!(db.last_sequence(), 3);
    }
}
