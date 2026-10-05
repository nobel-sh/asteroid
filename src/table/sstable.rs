use super::{
    bloom::BloomFilter,
    builder::SSTableBuilder,
    data_block::{decode_data_block, DataBlock},
    index_block::IndexBlock,
    iter::TableIterator,
};
use crate::{
    cache::BlockCache,
    codec::Decoder,
    entry::{Key, Lookup, StoredValue, Value},
    error::{Error, Result},
    memtable::Memtable,
};

use std::{
    fs::{File, OpenOptions},
    ops::Bound,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) const MAGIC_BYTES: [u8; 4] = *b"SST1";
pub(super) const VERSION: u16 = 1;
const HEADER_SIZE: usize = MAGIC_BYTES.len() + size_of::<u16>();
const FOOTER_SIZE: usize = 2 * (size_of::<u64>() + size_of::<u32>());

#[derive(Debug)]
pub(crate) struct SSTable {
    pub(super) id: u32,
    pub(super) file: File,
    pub(super) index: IndexBlock,
    pub(super) filter: Option<BloomFilter>,
    pub(super) cache: Arc<BlockCache>,
}

impl SSTable {
    pub(crate) fn path(&self, directory: &Path) -> PathBuf {
        let table_file = format!("{:08}.sst", self.id);
        directory.join(table_file)
    }

    pub(crate) fn iter(&self) -> TableIterator<'_> {
        TableIterator::new(self)
    }

    pub(crate) fn create(
        directory: &Path,
        id: u32,
        memtable: &Memtable,
        cache: Arc<BlockCache>,
        bits_per_key: Option<u8>,
    ) -> Result<Self> {
        let mut builder = SSTableBuilder::new(directory, id, cache, bits_per_key)?;
        for (key, entry) in memtable.iter() {
            builder.add(key, entry)?;
        }
        builder.finish()
    }

    pub(crate) fn overlaps(&self, start: Bound<&Key>, end: Bound<&Key>) -> bool {
        self.index.overlaps(start, end)
    }

    pub(crate) fn lookup(&self, key: &Key) -> Result<Lookup<Value>> {
        if !self
            .index
            .overlaps(Bound::Included(key), Bound::Included(key))
        {
            return Ok(Lookup::NotFound);
        }
        if self
            .filter
            .as_ref()
            .is_some_and(|filter| !filter.may_contain(key.as_bytes()))
        {
            return Ok(Lookup::NotFound);
        }
        let Some(position) = self.index.find_block(key) else {
            return Ok(Lookup::NotFound);
        };
        let offset = self.index.entries[position].block_offset;
        let block = match self.cache.get(self.id, offset) {
            Some(block) => block,
            None => {
                let block = Arc::new(self.read_block(position)?);
                self.cache.insert(self.id, offset, Arc::clone(&block));
                block
            }
        };

        let Ok(position) = block
            .entries
            .binary_search_by(|(stored_key, _)| stored_key.cmp(key))
        else {
            return Ok(Lookup::NotFound);
        };
        match &block.entries[position].1.value {
            StoredValue::Data(value) => Ok(Lookup::Value(value.clone())),
            StoredValue::Tombstone => Ok(Lookup::Deleted),
        }
    }

    pub(crate) fn read_block(&self, position: usize) -> Result<DataBlock> {
        let entry = &self.index.entries[position];
        let block_size = usize::try_from(entry.block_size).map_err(|_| Error::RecordTooLarge)?;
        let mut bytes = vec![0; block_size];
        self.file.read_exact_at(&mut bytes, entry.block_offset)?;
        let block = decode_data_block(&bytes)?;
        if block
            .entries
            .last()
            .is_none_or(|(key, _)| key != &entry.last_key)
        {
            return Err(corrupt("index key does not match data block"));
        }
        if position == 0
            && self
                .index
                .smallest_key
                .as_ref()
                .is_some_and(|first| block.entries.first().is_none_or(|(key, _)| key != first))
        {
            return Err(corrupt("smallest key does not match data block"));
        }
        if position > 0
            && block
                .entries
                .first()
                .is_some_and(|(key, _)| key <= &self.index.entries[position - 1].last_key)
        {
            return Err(corrupt("data blocks are not strictly sorted"));
        }
        Ok(block)
    }

    pub(crate) fn open(
        path: &Path,
        id: u32,
        cache: Arc<BlockCache>,
        use_filter: bool,
    ) -> Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let (index, filter) = read_and_validate_metadata(&file)?;

        Ok(Self {
            id,
            file,
            index,
            filter: if use_filter { filter } else { None },
            cache,
        })
    }
}

fn read_and_validate_metadata(file: &File) -> Result<(IndexBlock, Option<BloomFilter>)> {
    let file_size = file.metadata()?.len();

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

    let footer_offset = file_size
        .checked_sub(FOOTER_SIZE as u64)
        .filter(|offset| *offset >= HEADER_SIZE as u64)
        .ok_or_else(|| corrupt("truncated footer"))?;
    let mut footer_bytes = [0; FOOTER_SIZE];
    file.read_exact_at(&mut footer_bytes, footer_offset)?;
    let mut footer = Decoder::new(&footer_bytes);
    let index_offset = footer.read_u64()?;
    let index_size = footer.read_u32()?;
    let filter_offset = footer.read_u64()?;
    let filter_size = footer.read_u32()?;
    footer.finish()?;
    if index_offset < HEADER_SIZE as u64
        || index_offset.checked_add(u64::from(index_size)) != Some(filter_offset)
        || filter_offset.checked_add(u64::from(filter_size)) != Some(footer_offset)
    {
        return Err(corrupt("invalid index location"));
    }

    let mut index_bytes = vec![0; index_size as usize];
    file.read_exact_at(&mut index_bytes, index_offset)?;
    let index = IndexBlock::decode_index_block(&index_bytes)?;
    index.validate_block_handles(HEADER_SIZE as u64, index_offset)?;
    let filter = if filter_size == 0 {
        None
    } else {
        let mut bytes = vec![0; filter_size as usize];
        file.read_exact_at(&mut bytes, filter_offset)?;
        Some(BloomFilter::decode(&bytes)?)
    };
    Ok((index, filter))
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}
