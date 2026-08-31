use crate::{WriteBatch, WriteOp};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Key(Vec<u8>);

impl Key {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<&str> for Key {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<&[u8]> for Key {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl From<Vec<u8>> for Key {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Value(Vec<u8>);

impl Value {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum StoredValue {
    Data(Value),
    Tombstone,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) sequence: u64,
    pub(crate) value: StoredValue,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Lookup<'a> {
    Value(&'a [u8]),
    Deleted,
    NotFound,
}

impl Entry {
    pub(crate) fn lookup(&self) -> Lookup<'_> {
        match &self.value {
            StoredValue::Data(value) => Lookup::Value(value.as_bytes()),
            StoredValue::Tombstone => Lookup::Deleted,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Memtable {
    entries: BTreeMap<Key, Entry>,
}

impl Memtable {
    pub(crate) fn lookup(&self, key: &Key) -> Lookup<'_> {
        self.entries
            .get(key)
            .map_or(Lookup::NotFound, Entry::lookup)
    }

    pub(crate) fn put(&mut self, sequence: u64, key: Key, value: Value) {
        self.apply(
            key,
            Entry {
                sequence,
                value: StoredValue::Data(value),
            },
        );
    }

    pub(crate) fn delete(&mut self, sequence: u64, key: Key) {
        self.apply(
            key,
            Entry {
                sequence,
                value: StoredValue::Tombstone,
            },
        );
    }

    fn apply(&mut self, key: Key, entry: Entry) {
        if self
            .entries
            .get(&key)
            .is_none_or(|current| entry.sequence > current.sequence)
        {
            self.entries.insert(key, entry);
        }
    }

    pub(crate) fn apply_batch(&mut self, batch: &WriteBatch, starting_sequence: u64) {
        for (offset, operation) in (0_u64..).zip(&batch.operations) {
            let sequence = offset + starting_sequence;
            match operation {
                WriteOp::Delete { key } => self.delete(sequence, key.clone()),
                WriteOp::Put { key, value } => self.put(sequence, key.clone(), value.clone()),
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Key, &Entry)> {
        self.entries.iter()
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, Lookup, Memtable, Value};

    #[test]
    fn newer_entries_win() {
        let mut memtable = Memtable::default();
        let key = Key::from("key");

        memtable.put(2, key.clone(), Value::from("new"));
        memtable.put(1, key.clone(), Value::from("old"));

        assert_eq!(memtable.lookup(&key), Lookup::Value(b"new"));
    }
}
