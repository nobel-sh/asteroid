use std::os::unix::fs::FileExt;

use crate::{entry::Entry, table::data_block::decode_data_block, Error, Key, Result};

use super::SSTable;

pub(crate) struct TableIterator<'t> {
    table: &'t SSTable,
    next_block: usize,
    entries: std::vec::IntoIter<(Key, Entry)>,
}

impl<'t> TableIterator<'t> {
    pub(super) fn new(table: &'t SSTable) -> Self {
        Self {
            table,
            next_block: 0,
            entries: Vec::new().into_iter(),
        }
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
        self.entries = block.entries.into_iter();
        self.next_block += 1;
        Ok(())
    }
}

impl Iterator for TableIterator<'_> {
    type Item = Result<(Key, Entry)>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(value) = self.entries.next() {
            return Some(Ok(value));
        }
        if self.next_block >= self.table.index.entries.len() {
            return None;
        }
        if let Err(error) = self.load_block() {
            self.next_block = self.table.index.entries.len();
            return Some(Err(error));
        }
        self.entries.next().map(Ok)
    }
}
