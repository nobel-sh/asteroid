use crate::{Error, Result};

// Sequence, operation kind, key length, and value length.
const ENTRY_HEADER_SIZE: usize = size_of::<u64>() + size_of::<u8>() + 2 * size_of::<u32>();

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

impl Entry {
    pub(crate) fn encoded_size(&self, key: &Key) -> Result<usize> {
        let value_size = match &self.value {
            StoredValue::Data(value) => value.as_bytes().len(),
            StoredValue::Tombstone => 0,
        };
        ENTRY_HEADER_SIZE
            .checked_add(key.as_bytes().len())
            .and_then(|size| size.checked_add(value_size))
            .ok_or(Error::RecordTooLarge)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Lookup<T> {
    Value(T),
    Deleted,
    NotFound,
}
