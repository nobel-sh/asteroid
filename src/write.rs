use super::memtable::{Key, Value};

#[derive(Debug, Eq, PartialEq)]
pub enum WriteOp {
    Put { key: Key, value: Value },
    Delete { key: Key },
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct WriteBatch {
    pub(crate) operations: Vec<WriteOp>,
}

impl WriteBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, key: impl Into<Key>, value: impl Into<Value>) {
        self.operations.push(WriteOp::Put {
            key: key.into(),
            value: value.into(),
        });
    }

    pub fn delete(&mut self, key: impl Into<Key>) {
        self.operations.push(WriteOp::Delete { key: key.into() });
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}
