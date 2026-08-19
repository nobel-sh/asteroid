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
enum StoredValue {
    Data(Value),
    Tombstone,
}

#[derive(Debug, Eq, PartialEq)]
struct Entry {
    sequence: u64,
    value: StoredValue,
}

#[derive(Debug, Default)]
pub(crate) struct Memtable {
    entries: BTreeMap<Key, Entry>,
}

impl Memtable {
    pub(crate) fn get(&self, key: &Key) -> Option<&[u8]> {
        match self.entries.get(key).map(|entry| &entry.value) {
            Some(StoredValue::Data(value)) => Some(&value.0),
            Some(StoredValue::Tombstone) | None => None,
        }
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
}

#[cfg(test)]
mod tests {
    use super::{Key, Memtable, Value};

    #[test]
    fn newer_entries_win() {
        let mut memtable = Memtable::default();
        let key = Key::from("key");

        memtable.put(2, key.clone(), Value::from("new"));
        memtable.put(1, key.clone(), Value::from("old"));

        assert_eq!(memtable.get(&key), Some(b"new".as_slice()));
    }
}
