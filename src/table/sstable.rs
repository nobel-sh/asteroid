use super::{
    data_block::{build_data_blocks, decode_data_block},
    index_block::IndexBlock,
};
use crate::{
    codec::Decoder,
    entry::{Entry, Key, Lookup, StoredValue, Value},
    error::{Error, Result},
    memtable::Memtable,
};

use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

const MAGIC_BYTES: [u8; 4] = *b"SST1";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = MAGIC_BYTES.len() + size_of::<u16>();
const FOOTER_SIZE: usize = size_of::<u64>() + size_of::<u32>() + size_of::<u64>();

#[derive(Debug)]
pub(crate) struct SSTable {
    id: u32,
    file: File,
    index: IndexBlock,
    last_sequence: u64,
}

impl SSTable {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn path(&self, directory: &Path) -> PathBuf {
        let table_file = format!("{:08}.sst", self.id);
        directory.join(table_file)
    }

    pub(crate) fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub(crate) fn read_all_entries(&self) -> Result<Vec<(Key, Entry)>> {
        let mut entries = Vec::new();
        for index in &self.index.entries {
            let block_size =
                usize::try_from(index.block_size).map_err(|_| Error::RecordTooLarge)?;
            let mut buffer = vec![0; block_size];
            self.file.read_exact_at(&mut buffer, index.block_offset)?;
            let data_block = decode_data_block(&buffer)?;
            entries.extend(data_block.entries);
        }
        Ok(entries)
    }

    pub(crate) fn create(
        directory: &Path,
        id: u32,
        memtable: &Memtable,
        last_sequence: u64,
    ) -> Result<Self> {
        let mut buffer = Vec::new();

        buffer.extend_from_slice(&MAGIC_BYTES);
        buffer.extend_from_slice(&VERSION.to_be_bytes());

        let blocks = build_data_blocks(memtable)?;
        let index = IndexBlock::from_data_blocks(&blocks, HEADER_SIZE as u64)?;
        for block in &blocks {
            buffer.extend_from_slice(&block.bytes);
        }

        let index_offset = u64::try_from(buffer.len()).map_err(|_| Error::RecordTooLarge)?;
        let index_bytes = index.as_bytes()?;
        let index_size = u32::try_from(index_bytes.len()).map_err(|_| Error::RecordTooLarge)?;
        buffer.extend_from_slice(&index_bytes);

        buffer.extend_from_slice(&index_offset.to_be_bytes());
        buffer.extend_from_slice(&index_size.to_be_bytes());
        buffer.extend_from_slice(&last_sequence.to_be_bytes());

        let file_name = format!("{:08}.sst", id);
        let tmp_file_name = format!("{}.tmp", file_name);
        let path = directory.join(tmp_file_name);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        file.write_all(&buffer)?;
        file.sync_all()?;

        let new_file = directory.join(file_name);
        std::fs::rename(path, &new_file)?;
        std::fs::File::open(directory)?.sync_all()?;

        let file = OpenOptions::new().read(true).open(new_file)?;

        Ok(Self {
            id,
            file,
            index,
            last_sequence,
        })
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
        let (index, last_sequence) = read_and_validate_metadata(&file)?;

        Ok(Self {
            id,
            file,
            last_sequence,
            index,
        })
    }
}

fn read_and_validate_metadata(file: &File) -> Result<(IndexBlock, u64)> {
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
    let last_sequence = footer.read_u64()?;
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
    Ok((index, last_sequence))
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}
