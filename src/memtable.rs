use crate::{
    entry::{Entry, Key, Lookup, StoredValue, Value},
    WriteBatch, WriteOp,
};
use std::collections::BTreeMap;

impl Entry {
    pub(crate) fn lookup(&self) -> Lookup<&[u8]> {
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
    pub(crate) fn lookup(&self, key: &Key) -> Lookup<&[u8]> {
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

    pub(crate) fn apply(&mut self, key: Key, entry: Entry) {
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
