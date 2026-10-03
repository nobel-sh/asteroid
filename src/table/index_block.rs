use super::data_block::EncodedDataBlock;
use crate::{
    codec::{Decoder, CHECKSUM_SIZE},
    entry::Key,
    Error, Result,
};

#[derive(Debug)]
pub(crate) struct IndexEntry {
    pub(crate) last_key: Key,
    pub(crate) block_offset: u64,
    pub(crate) block_size: u32,
}

impl IndexEntry {
    pub fn from_data_block(block: &EncodedDataBlock, block_offset: u64) -> Result<Self> {
        let block_size = u32::try_from(block.bytes.len()).map_err(|_| Error::RecordTooLarge)?;
        Ok(Self {
            last_key: block.last_key.clone(),
            block_offset,
            block_size,
        })
    }
}

#[derive(Debug)]
pub(crate) struct IndexBlock {
    pub(crate) entries: Vec<IndexEntry>,
}

impl IndexBlock {
    pub(crate) fn as_bytes(&self) -> Result<Vec<u8>> {
        let mut index_buffer = Vec::new();
        let entry_count = u32::try_from(self.entries.len()).map_err(|_| Error::RecordTooLarge)?;
        index_buffer.extend_from_slice(&entry_count.to_be_bytes());
        for entry in &self.entries {
            let last_key_size = u32::try_from(entry.last_key.as_bytes().len())
                .map_err(|_| Error::RecordTooLarge)?;
            index_buffer.extend_from_slice(&last_key_size.to_be_bytes());
            index_buffer.extend_from_slice(entry.last_key.as_bytes());
            index_buffer.extend_from_slice(&entry.block_offset.to_be_bytes());
            index_buffer.extend_from_slice(&entry.block_size.to_be_bytes());
        }
        let hash = crc32fast::hash(&index_buffer);
        index_buffer.extend_from_slice(&hash.to_be_bytes());
        Ok(index_buffer)
    }

    pub(crate) fn decode_index_block(bytes: &[u8]) -> Result<IndexBlock> {
        let checksum_offset = bytes
            .len()
            .checked_sub(CHECKSUM_SIZE)
            .ok_or_else(|| corrupt("truncated index block checksum"))?;
        let (contents, checksum_bytes) = bytes.split_at(checksum_offset);
        let stored_checksum = u32::from_be_bytes(checksum_bytes.try_into().unwrap());
        if crc32fast::hash(contents) != stored_checksum {
            return Err(corrupt("index block checksum mismatch"));
        }

        let mut entries = Vec::new();
        let mut decoder = Decoder::new(contents);
        let entry_count = decoder.read_u32()?;
        for _ in 0..entry_count {
            let last_key_size = decoder.read_u32()?;
            let key_size = usize::try_from(last_key_size).map_err(|_| Error::RecordTooLarge)?;
            let last_key = Key::from(decoder.take(key_size)?.to_vec());
            let block_offset = decoder.read_u64()?;
            let block_size = decoder.read_u32()?;

            if entries
                .last()
                .is_some_and(|previous: &IndexEntry| previous.last_key >= last_key)
            {
                return Err(corrupt("index keys are not strictly sorted"));
            }
            entries.push(IndexEntry {
                last_key,
                block_offset,
                block_size,
            });
        }
        decoder.finish()?;

        Ok(IndexBlock { entries })
    }

    pub(crate) fn find_block(&self, key: &Key) -> Option<usize> {
        let position = self.entries.partition_point(|entry| entry.last_key < *key);
        self.entries.get(position).map(|_| position)
    }

    pub(crate) fn validate_block_handles(&self, data_offset: u64, index_offset: u64) -> Result<()> {
        let mut expected_offset = data_offset;
        for entry in &self.entries {
            if entry.block_offset != expected_offset || entry.block_size == 0 {
                return Err(corrupt("invalid data block location"));
            }
            expected_offset = expected_offset
                .checked_add(u64::from(entry.block_size))
                .ok_or_else(|| corrupt("data block location overflow"))?;
        }
        if expected_offset != index_offset {
            return Err(corrupt("index does not cover the data block region"));
        }
        Ok(())
    }
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}
