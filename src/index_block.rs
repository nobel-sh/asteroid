use crate::{
    codec::Decoder,
    data_block::{DataBlock, EncodedDataBlock},
    Error, Key, Result,
};

pub(crate) struct IndexEntry {
    last_key_size: u32,
    last_key: Key,
    block_offset: u64,
    block_size: u32,
}

impl IndexEntry {
    fn from_data_block(block: &EncodedDataBlock, block_offset: u64) -> Result<Self> {
        let block_size = u32::try_from(block.bytes.len()).map_err(|_| Error::RecordTooLarge)?;
        let last_key_size =
            u32::try_from(block.last_key.as_bytes().len()).map_err(|_| Error::RecordTooLarge)?;
        Ok(Self {
            last_key_size,
            last_key: block.last_key.clone(),
            block_offset,
            block_size,
        })
    }
}

pub(crate) struct IndexBlock {
    entries: Vec<IndexEntry>,
}

impl IndexBlock {
    pub(crate) fn as_bytes(&self) -> Result<Vec<u8>> {
        let mut index_buffer = Vec::new();
        let entry_count = u32::try_from(self.entries.len()).map_err(|_| Error::RecordTooLarge)?;
        index_buffer.extend_from_slice(&entry_count.to_be_bytes());
        for entry in &self.entries {
            index_buffer.extend_from_slice(&entry.last_key_size.to_be_bytes());
            index_buffer.extend_from_slice(entry.last_key.as_bytes());
            index_buffer.extend_from_slice(&entry.block_offset.to_be_bytes());
            index_buffer.extend_from_slice(&entry.block_size.to_be_bytes());
        }
        let hash = crc32fast::hash(&index_buffer);
        index_buffer.extend_from_slice(&hash.to_be_bytes());
        Ok(index_buffer)
    }

    pub(crate) fn from_data_blocks(
        data_blocks: &[EncodedDataBlock],
        starting_offset: u64,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        let mut offset = starting_offset;
        for block in data_blocks {
            let index_entry = IndexEntry::from_data_block(block, offset)?;
            offset = offset
                .checked_add(u64::from(index_entry.block_size))
                .ok_or(Error::RecordTooLarge)?;
            entries.push(index_entry);
        }
        Ok(Self { entries })
    }

    pub(crate) fn decode_index_block(bytes: &[u8]) -> Result<IndexBlock> {
        let checksum_offset = bytes
            .len()
            .checked_sub(4)
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
                last_key_size,
                last_key,
                block_offset,
                block_size,
            });
        }
        decoder.finish()?;

        Ok(IndexBlock { entries })
    }

    pub(crate) fn validate_last_keys(&self, data_blocks: &[DataBlock]) -> Result<()> {
        if self.entries.len() != data_blocks.len() {
            return Err(corrupt("index entry count does not match data blocks"));
        }

        for (index_entry, data_block) in self.entries.iter().zip(data_blocks) {
            if data_block
                .entries
                .last()
                .is_none_or(|(last_key, _)| last_key != &index_entry.last_key)
            {
                return Err(corrupt("index key does not match data block"));
            }
        }
        Ok(())
    }
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}
