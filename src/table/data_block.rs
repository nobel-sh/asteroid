use crate::{
    codec::{Decoder, CHECKSUM_SIZE},
    entry::{Entry, Key, StoredValue},
    error::{Error, Result},
};

const DATA_BLOCK_SIZE: usize = 4 * 1024;
const BLOCK_OVERHEAD: usize = size_of::<u32>() + CHECKSUM_SIZE;
pub(crate) const DELETE: u8 = 1;
pub(crate) const PUT: u8 = 2;

pub(crate) struct DataBlockBuilder {
    buffer: Vec<u8>,
    entry_count: u32,
    last_key: Option<Key>,
}

pub(crate) struct EncodedDataBlock {
    pub(crate) bytes: Vec<u8>,
    pub(crate) last_key: Key,
}

#[derive(Debug)]
pub(crate) struct DataBlock {
    pub(crate) entries: Vec<(Key, Entry)>,
}

impl DataBlock {
    pub(crate) fn memory_usage(&self) -> usize {
        size_of::<Self>()
            + 2 * size_of::<usize>()
            + self.entries.capacity() * size_of::<(Key, Entry)>()
            + self
                .entries
                .iter()
                .map(|(key, entry)| {
                    key.heap_size()
                        + match &entry.value {
                            StoredValue::Data(value) => value.heap_size(),
                            StoredValue::Tombstone => 0,
                        }
                })
                .sum::<usize>()
    }
}

impl DataBlockBuilder {
    pub(crate) fn new() -> Self {
        Self {
            buffer: Vec::new(),
            entry_count: 0,
            last_key: None,
        }
    }

    pub(crate) fn add_entry(&mut self, key: &Key, entry: &Entry) -> Result<()> {
        encode_entry(key, entry, &mut self.buffer)?;
        self.entry_count = self
            .entry_count
            .checked_add(1)
            .ok_or(Error::RecordTooLarge)?;
        self.last_key = Some(key.clone());
        Ok(())
    }

    pub(crate) fn finish(&self) -> Option<EncodedDataBlock> {
        let last_key = self.last_key.clone()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.entry_count.to_be_bytes());
        bytes.extend_from_slice(&self.buffer);

        let checksum = crc32fast::hash(&bytes);
        bytes.extend_from_slice(&checksum.to_be_bytes());
        Some(EncodedDataBlock { bytes, last_key })
    }

    pub(crate) fn would_exceed(&self, entry_size: usize) -> bool {
        self.buffer
            .len()
            .saturating_add(entry_size)
            .saturating_add(BLOCK_OVERHEAD)
            > DATA_BLOCK_SIZE
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entry_count == 0
    }
}

pub(crate) fn encode_entry(key: &Key, entry: &Entry, buffer: &mut Vec<u8>) -> Result<()> {
    let key_bytes = key.as_bytes();
    let (kind, value_bytes) = match &entry.value {
        StoredValue::Data(data) => (PUT, data.as_bytes()),
        StoredValue::Tombstone => (DELETE, &[][..]),
    };
    let key_size = u32::try_from(key_bytes.len()).map_err(|_| Error::RecordTooLarge)?;
    let value_size = u32::try_from(value_bytes.len()).map_err(|_| Error::RecordTooLarge)?;

    buffer.extend_from_slice(&entry.sequence.to_be_bytes());
    buffer.push(kind);
    buffer.extend_from_slice(&key_size.to_be_bytes());
    buffer.extend_from_slice(&value_size.to_be_bytes());
    buffer.extend_from_slice(key_bytes);
    buffer.extend_from_slice(value_bytes);
    Ok(())
}

pub(crate) fn decode_data_block(bytes: &[u8]) -> Result<DataBlock> {
    let mut decoder = Decoder::new(bytes);
    let block = decode_block(&mut decoder)?;
    decoder.finish()?;
    Ok(block)
}

fn decode_block(decoder: &mut Decoder<'_>) -> Result<DataBlock> {
    let block_start = decoder.position();
    let entry_count = decoder.read_u32()?;
    let entries = decode_entries(decoder, entry_count)?;
    let checksum_start = decoder.position();
    let stored_checksum = decoder.read_u32()?;
    if crc32fast::hash(&decoder.bytes()[block_start..checksum_start]) != stored_checksum {
        return Err(corrupt("data block checksum mismatch"));
    }
    Ok(DataBlock { entries })
}

pub(crate) fn decode_entries(
    decoder: &mut Decoder<'_>,
    entry_count: u32,
) -> Result<Vec<(Key, Entry)>> {
    let mut entries = Vec::new();

    for _ in 0..entry_count {
        let sequence = decoder.read_u64()?;
        let kind = decoder.read_u8()?;
        let key_size = decoder.read_u32()? as usize;
        let value_size = decoder.read_u32()? as usize;
        let key = Key::from(decoder.take(key_size)?.to_vec());
        let value = match kind {
            DELETE if value_size == 0 => StoredValue::Tombstone,
            DELETE => return Err(corrupt("delete entry has a value")),
            PUT => StoredValue::Data(decoder.take(value_size)?.to_vec().into()),
            _ => return Err(corrupt(format!("unknown entry kind {kind}"))),
        };

        if entries
            .last()
            .is_some_and(|(previous, _): &(Key, Entry)| previous >= &key)
        {
            return Err(corrupt("entries are not strictly sorted"));
        }
        entries.push((key, Entry { sequence, value }));
    }
    Ok(entries)
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}

#[cfg(test)]
mod tests {
    use super::{decode_data_block, DataBlockBuilder, CHECKSUM_SIZE};
    use crate::{memtable::Memtable, Key, Value};

    #[test]
    fn frames_sorted_entries() {
        let mut memtable = Memtable::default();
        memtable.put(1, Key::from("a"), Value::from(vec![0; 4096]));
        memtable.put(2, Key::from("b"), Value::from("value"));

        for (key, entry) in memtable.iter() {
            let mut builder = DataBlockBuilder::new();
            builder.add_entry(key, entry).unwrap();
            let block = builder.finish().unwrap();
            assert_eq!(&block.last_key, key);
            let checksum_at = block.bytes.len() - CHECKSUM_SIZE;
            assert_eq!(
                u32::from_be_bytes(block.bytes[checksum_at..].try_into().unwrap()),
                crc32fast::hash(&block.bytes[..checksum_at])
            );
            let decoded = decode_data_block(&block.bytes).unwrap();
            assert_eq!(decoded.entries.last().unwrap().0, block.last_key);
        }
    }
}
