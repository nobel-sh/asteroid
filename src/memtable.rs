use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Key(Vec<u8>);

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

#[derive(Debug, Default)]
pub struct Memtable {
    entries: BTreeMap<Key, StoredValue>,
}

impl Memtable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, key: Key, value: impl Into<Value>) {
        self.entries.insert(key, StoredValue::Data(value.into()));
    }

    pub fn get(&self, key: &Key) -> Option<&[u8]> {
        match self.entries.get(key) {
            Some(StoredValue::Data(value)) => Some(&value.0),
            Some(StoredValue::Tombstone) | None => None,
        }
    }

    pub fn delete(&mut self, key: Key) {
        self.entries.insert(key, StoredValue::Tombstone);
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, Memtable, StoredValue};

    #[test]
    fn put_get_replace_and_delete() {
        let mut memtable = Memtable::new();
        let key = Key::from("foo");

        assert_eq!(memtable.get(&key), None);

        memtable.put(key.clone(), "first");
        assert_eq!(memtable.get(&key), Some(b"first".as_slice()));

        memtable.put(key.clone(), "second");
        assert_eq!(memtable.get(&key), Some(b"second".as_slice()));

        memtable.delete(key.clone());
        assert_eq!(memtable.get(&key), None);
        assert_eq!(memtable.entries.get(&key), Some(&StoredValue::Tombstone));
    }

    #[test]
    fn deleting_leaves_a_tombstone() {
        let mut memtable = Memtable::new();
        let key = Key::from("missing");

        memtable.delete(key.clone());

        assert_eq!(memtable.get(&key), None);
        assert_eq!(memtable.entries.get(&key), Some(&StoredValue::Tombstone));
    }

    #[test]
    fn keys_byte_order() {
        let mut memtable = Memtable::new();
        memtable.put(Key::from("c"), "");
        memtable.put(Key::from("a"), "");
        memtable.put(Key::from("b"), "");

        let keys: Vec<&[u8]> = memtable
            .entries
            .keys()
            .map(|key| key.0.as_slice())
            .collect();

        assert_eq!(keys, [b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]);
    }
}
