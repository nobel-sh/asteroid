use crate::{
    entry::{Entry, Key, Lookup, StoredValue, Value},
    write::WriteOp,
    Error, Result, WriteBatch,
};
use std::{
    collections::{btree_map, BTreeMap},
    ops::Bound,
};

impl Entry {
    fn memory_usage(&self) -> usize {
        size_of::<Self>()
            + match &self.value {
                StoredValue::Data(value) => value.heap_size(),
                StoredValue::Tombstone => 0,
            }
    }

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
    estimated_bytes: usize,
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
        )
    }

    pub(crate) fn delete(&mut self, sequence: u64, key: Key) {
        self.apply(
            key,
            Entry {
                sequence,
                value: StoredValue::Tombstone,
            },
        )
    }

    pub(crate) fn apply(&mut self, key: Key, entry: Entry) {
        match self.entries.entry(key) {
            btree_map::Entry::Occupied(mut slot) => {
                if entry.sequence > slot.get().sequence {
                    self.estimated_bytes -= slot.get().memory_usage();
                    self.estimated_bytes += entry.memory_usage();
                    slot.insert(entry);
                }
            }
            btree_map::Entry::Vacant(slot) => {
                self.estimated_bytes +=
                    size_of::<Key>() + slot.key().heap_size() + entry.memory_usage();
                slot.insert(entry);
            }
        }
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub(crate) fn apply_batch(&mut self, batch: &WriteBatch, starting_sequence: u64) -> Result<()> {
        for (offset, operation) in (0_u64..).zip(&batch.operations) {
            let sequence = starting_sequence
                .checked_add(offset)
                .ok_or(Error::SequenceExhausted)?;
            match operation {
                WriteOp::Delete { key } => self.delete(sequence, key.clone()),
                WriteOp::Put { key, value } => self.put(sequence, key.clone(), value.clone()),
            }
        }
        Ok(())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Key, &Entry)> {
        self.entries.iter()
    }

    pub(crate) fn range(
        &self,
        start: Bound<Key>,
        end: Bound<Key>,
    ) -> btree_map::Range<'_, Key, Entry> {
        self.entries.range((start, end))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.estimated_bytes = 0;
    }
}
