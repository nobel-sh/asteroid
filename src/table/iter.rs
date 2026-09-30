use std::{ops::Bound, os::unix::fs::FileExt};

use crate::{entry::Entry, table::data_block::decode_data_block, Error, Key, Result};

use super::SSTable;

pub(crate) struct TableIterator<'t> {
    table: &'t SSTable,
    next_block: usize,
    entries: std::vec::IntoIter<(Key, Entry)>,
    start: Bound<Key>,
    end: Bound<Key>,
}

impl<'t> TableIterator<'t> {
    pub(super) fn new(table: &'t SSTable) -> Self {
        Self {
            table,
            next_block: 0,
            entries: Vec::new().into_iter(),
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }

    pub(crate) fn from_bounds(table: &'t SSTable, start: &Bound<Key>, end: Bound<Key>) -> Self {
        let mut iter = Self::new(table);
        iter.start = start.clone();
        iter.end = end;
        iter.next_block = table.index.entries.partition_point(|entry| match start {
            Bound::Included(key) => entry.last_key < *key,
            Bound::Excluded(key) => entry.last_key <= *key,
            Bound::Unbounded => false,
        });
        iter
    }

    fn load_block(&mut self) -> Result<()> {
        let block_index = &self.table.index.entries[self.next_block];
        let block_size =
            usize::try_from(block_index.block_size).map_err(|_| Error::RecordTooLarge)?;
        let mut buffer = vec![0_u8; block_size];
        self.table
            .file
            .read_exact_at(&mut buffer, block_index.block_offset)?;
        let block = decode_data_block(&buffer)?;
        let Some((first_key, _)) = block.entries.first() else {
            return Err(Error::CorruptSstable("empty indexed data block".into()));
        };
        if block
            .entries
            .last()
            .is_none_or(|(key, _)| key != &block_index.last_key)
        {
            return Err(Error::CorruptSstable(
                "index key does not match data block".into(),
            ));
        }
        if self.next_block > 0
            && first_key <= &self.table.index.entries[self.next_block - 1].last_key
        {
            return Err(Error::CorruptSstable(
                "data blocks are not strictly sorted".into(),
            ));
        }
        let skip = block.entries.partition_point(|(key, _)| match &self.start {
            Bound::Included(start) => key < start,
            Bound::Excluded(start) => key <= start,
            Bound::Unbounded => false,
        });
        self.entries = block.entries.into_iter();
        if skip > 0 {
            self.entries.nth(skip - 1);
        }
        self.start = Bound::Unbounded;
        self.next_block += 1;
        Ok(())
    }
}

impl Iterator for TableIterator<'_> {
    type Item = Result<(Key, Entry)>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(value) = self.entries.next() {
                let past_end = match &self.end {
                    Bound::Included(end) => value.0 > *end,
                    Bound::Excluded(end) => value.0 >= *end,
                    Bound::Unbounded => false,
                };
                if past_end {
                    self.entries = Vec::new().into_iter();
                    self.next_block = self.table.index.entries.len();
                    return None;
                }
                return Some(Ok(value));
            }
            if self.next_block >= self.table.index.entries.len() {
                return None;
            }
            if self.next_block > 0 {
                let previous_last_key = &self.table.index.entries[self.next_block - 1].last_key;
                if matches!(&self.end, Bound::Included(end) | Bound::Excluded(end) if previous_last_key >= end)
                {
                    self.next_block = self.table.index.entries.len();
                    return None;
                }
            }
            if let Err(error) = self.load_block() {
                self.next_block = self.table.index.entries.len();
                return Some(Err(error));
            }
        }
    }
}
