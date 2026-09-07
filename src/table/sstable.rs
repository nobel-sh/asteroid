use super::{
    builder::SSTableBuilder, data_block::decode_data_block, index_block::IndexBlock,
    iter::TableIterator,
};
use crate::{
    codec::Decoder,
    entry::{Key, Lookup, StoredValue, Value},
    error::{Error, Result},
    memtable::Memtable,
};

use std::{
    fs::{File, OpenOptions},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

pub(super) const MAGIC_BYTES: [u8; 4] = *b"SST1";
pub(super) const VERSION: u16 = 1;
const HEADER_SIZE: usize = MAGIC_BYTES.len() + size_of::<u16>();
const FOOTER_SIZE: usize = size_of::<u64>() + size_of::<u32>();

#[derive(Debug)]
pub(crate) struct SSTable {
    pub(super) id: u32,
    pub(super) file: File,
    pub(super) index: IndexBlock,
}

impl SSTable {
    pub(crate) fn path(&self, directory: &Path) -> PathBuf {
        let table_file = format!("{:08}.sst", self.id);
        directory.join(table_file)
    }

    pub(crate) fn iter(&self) -> TableIterator<'_> {
        TableIterator::new(self)
    }

    pub(crate) fn create(directory: &Path, id: u32, memtable: &Memtable) -> Result<Self> {
        let mut builder = SSTableBuilder::new(directory, id)?;
        for (key, entry) in memtable.iter() {
            builder.add(key, entry)?;
        }
        builder.finish()
    }

    pub(crate) fn lookup(&self, key: &Key) -> Result<Lookup<Value>> {
        let Some((expected_last_key, block_offset, block_size)) = self.index.find_block(key) else {
            return Ok(Lookup::NotFound);
        };
        let block_size = usize::try_from(block_size).map_err(|_| Error::RecordTooLarge)?;
        let mut bytes = vec![0; block_size];
        self.file.read_exact_at(&mut bytes, block_offset)?;

        let mut block = decode_data_block(&bytes)?;
        if block
            .entries
            .last()
            .is_none_or(|(last_key, _)| last_key != expected_last_key)
        {
            return Err(corrupt("index key does not match data block"));
        }

        let Ok(position) = block
            .entries
            .binary_search_by(|(stored_key, _)| stored_key.cmp(key))
        else {
            return Ok(Lookup::NotFound);
        };
        match block.entries.swap_remove(position).1.value {
            StoredValue::Data(value) => Ok(Lookup::Value(value)),
            StoredValue::Tombstone => Ok(Lookup::Deleted),
        }
    }

    pub(crate) fn open(path: &Path, id: u32) -> Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let index = read_and_validate_metadata(&file)?;

        Ok(Self { id, file, index })
    }
}

fn read_and_validate_metadata(file: &File) -> Result<IndexBlock> {
    let file_size = file.metadata()?.len();
    let footer_offset = file_size
        .checked_sub(FOOTER_SIZE as u64)
        .ok_or_else(|| corrupt("truncated footer"))?;

    let mut header_bytes = [0; HEADER_SIZE];
    file.read_exact_at(&mut header_bytes, 0)?;
    let mut header = Decoder::new(&header_bytes);
    if header.take(MAGIC_BYTES.len())? != MAGIC_BYTES {
        return Err(corrupt("invalid magic bytes"));
    }
    let version = header.read_u16()?;
    if version != VERSION {
        return Err(corrupt(format!("unsupported version {version}")));
    }

    let mut footer_bytes = [0; FOOTER_SIZE];
    file.read_exact_at(&mut footer_bytes, footer_offset)?;
    let mut footer = Decoder::new(&footer_bytes);
    let index_offset = footer.read_u64()?;
    let index_size = footer.read_u32()?;
    footer.finish()?;
    if index_offset < HEADER_SIZE as u64
        || index_offset.checked_add(u64::from(index_size)) != Some(footer_offset)
    {
        return Err(corrupt("invalid index location"));
    }

    let mut index_bytes = vec![0; index_size as usize];
    file.read_exact_at(&mut index_bytes, index_offset)?;
    let index = IndexBlock::decode_index_block(&index_bytes)?;
    index.validate_block_handles(HEADER_SIZE as u64, index_offset)?;
    Ok(index)
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}
