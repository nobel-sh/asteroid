use std::{
    fs::OpenOptions,
    io::{Seek, Write},
    path::{Path, PathBuf},
};

use super::data_block::DataBlockBuilder;
use super::sstable::{MAGIC_BYTES, VERSION};
use super::SSTable;
use crate::{
    entry::Entry,
    table::{
        data_block::{encoded_entry_size, EncodedDataBlock},
        index_block::{IndexBlock, IndexEntry},
    },
    Error, Key, Result,
};

pub(crate) struct SSTableBuilder {
    directory: PathBuf,
    table: SSTable,
    block_builder: DataBlockBuilder,
    last_key: Option<Key>,
}

impl SSTableBuilder {
    pub(crate) fn new(directory: &Path, id: u32) -> Result<Self> {
        let file_name = format!("{id:08}.sst.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(directory.join(file_name))?;
        file.write_all(&MAGIC_BYTES)?;
        file.write_all(&VERSION.to_be_bytes())?;
        let table = SSTable {
            id,
            file,
            index: IndexBlock {
                entries: Vec::new(),
            },
        };
        Ok(SSTableBuilder {
            directory: directory.to_path_buf(),
            table,
            block_builder: DataBlockBuilder::new(),
            last_key: None,
        })
    }

    fn flush_block(&mut self, block: &EncodedDataBlock) -> Result<()> {
        let offset = self.table.file.stream_position()?;
        let index_entry = IndexEntry::from_data_block(block, offset)?;
        self.table.file.write_all(&block.bytes)?;
        self.table.index.entries.push(index_entry);
        Ok(())
    }

    pub(crate) fn add(&mut self, key: &Key, entry: &Entry) -> Result<()> {
        if self
            .last_key
            .as_ref()
            .is_some_and(|previous| previous >= key)
        {
            return Err(Error::InvalidKeyOrder);
        }
        let entry_size = encoded_entry_size(key, entry)?;
        if self.block_builder.would_exceed(entry_size) && !self.block_builder.is_empty() {
            if let Some(block) = self.block_builder.finish() {
                self.flush_block(&block)?;
            }
            self.block_builder = DataBlockBuilder::new();
        }
        self.block_builder.add_entry(key, entry)?;
        self.last_key = Some(key.clone());
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<SSTable> {
        if let Some(block) = self.block_builder.finish() {
            self.flush_block(&block)?;
        }
        let index_offset = self.table.file.stream_position()?;
        let index_bytes = self.table.index.as_bytes()?;
        let index_size = u32::try_from(index_bytes.len()).map_err(|_| Error::RecordTooLarge)?;
        self.table.file.write_all(&index_bytes)?;
        self.table.file.write_all(&index_offset.to_be_bytes())?;
        self.table.file.write_all(&index_size.to_be_bytes())?;
        self.table.file.sync_all()?;
        let path = self.table.path(&self.directory);
        std::fs::rename(path.with_extension("sst.tmp"), &path)?;
        std::fs::File::open(&self.directory)?.sync_all()?;
        Ok(self.table)
    }
}
