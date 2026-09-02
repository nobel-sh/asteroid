use super::{
    codec::Decoder,
    data_block::{build_data_blocks, decode_data_blocks},
    error::{Error, Result},
    memtable::{Entry, Key, Lookup, Memtable},
};

use std::{fs::OpenOptions, io::Write, path::Path};

const MAGIC_BYTES: [u8; 4] = *b"SST1";
const VERSION: u16 = 1;

#[derive(Debug)]
pub(crate) struct SSTable {
    id: u32,
    entries: Vec<(Key, Entry)>,
}

impl SSTable {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn last_sequence(&self) -> Option<u64> {
        self.entries.iter().map(|(_, entry)| entry.sequence).max()
    }

    pub(crate) fn create(directory: &Path, id: u32, memtable: &Memtable) -> Result<Self> {
        let mut buffer = Vec::new();

        buffer.extend_from_slice(&MAGIC_BYTES);
        buffer.extend_from_slice(&VERSION.to_be_bytes());

        for block in build_data_blocks(memtable)? {
            buffer.extend_from_slice(&block.bytes);
        }

        let entries = decode_sstable(&buffer)?;

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

        Ok(Self { id, entries })
    }

    pub(crate) fn lookup(&self, key: &Key) -> Lookup<'_> {
        match self
            .entries
            .binary_search_by(|(stored_key, _)| stored_key.cmp(key))
        {
            Ok(index) => self.entries[index].1.lookup(),
            Err(_) => Lookup::NotFound,
        }
    }

    pub(crate) fn open(path: &Path, id: u32) -> Result<Self> {
        Ok(Self {
            id,
            entries: decode_sstable(&std::fs::read(path)?)?,
        })
    }
}

fn decode_sstable(bytes: &[u8]) -> Result<Vec<(Key, Entry)>> {
    let mut decoder = Decoder::new(bytes);
    if decoder.take(MAGIC_BYTES.len())? != MAGIC_BYTES {
        return Err(corrupt("invalid magic bytes"));
    }
    let version = decoder.read_u16()?;
    if version != VERSION {
        return Err(corrupt(format!("unsupported version {version}")));
    }

    let mut entries = Vec::new();
    for block in decode_data_blocks(&decoder.bytes()[decoder.position()..])? {
        if entries
            .last()
            .zip(block.entries.first())
            .is_some_and(|((previous_key, _), (next_key, _))| previous_key >= next_key)
        {
            return Err(corrupt("entries are not strictly sorted"));
        }
        entries.extend(block.entries);
    }
    Ok(entries)
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}

#[cfg(test)]
mod tests {
    use super::decode_sstable;
    use crate::Error;

    #[test]
    fn rejects_corrupt_checksum() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SST1");
        bytes.extend_from_slice(&1_u16.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());

        assert!(matches!(
            decode_sstable(&bytes),
            Err(Error::CorruptSstable(message)) if message == "data block checksum mismatch"
        ));
    }
}
