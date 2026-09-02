use super::{
    codec::Decoder,
    data_block::{decode_entries, encode_entry},
    error::{Error, Result},
    memtable::{Entry, Key, Lookup, Memtable},
};

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

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

    pub(crate) fn create(directory: &PathBuf, id: u32, memtable: &Memtable) -> Result<Self> {
        let mut buffer = Vec::new();

        buffer.extend_from_slice(&MAGIC_BYTES);
        buffer.extend_from_slice(&VERSION.to_be_bytes());
        let entry_count =
            u32::try_from(memtable.entry_count()).map_err(|_| Error::RecordTooLarge)?;
        buffer.extend_from_slice(&entry_count.to_be_bytes());

        for (key, entry) in memtable.iter() {
            encode_entry(key, entry, &mut buffer)?;
        }

        let checksum = crc32fast::hash(&buffer);
        buffer.extend_from_slice(&checksum.to_be_bytes());
        let entries = decode_sstable(&buffer)?;

        let file_name = format!("{:08}.sst", id);
        let tmp_file_name = format!("{}.tmp", file_name);
        let file = Path::new(&tmp_file_name);
        let path = Path::new(directory).join(file);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        file.write_all(&buffer)?;
        file.sync_all()?;

        let new_file_name = Path::new(&file_name);
        let new_file = Path::new(directory).join(new_file_name);
        std::fs::rename(path, &new_file)?;
        std::fs::File::open(directory)?.sync_all()?;

        let table = Self { id, entries };
        Ok(table)
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
        let mut table = Self {
            id,
            entries: Vec::new(),
        };
        let bytes = std::fs::read(path)?;
        let entries = decode_sstable(&bytes)?;
        table.entries = entries;
        Ok(table)
    }
}

fn decode_sstable(bytes: &[u8]) -> Result<Vec<(Key, Entry)>> {
    let checksum_start = bytes
        .len()
        .checked_sub(4)
        .ok_or_else(|| corrupt("truncated checksum"))?;
    let (contents, checksum_bytes) = bytes.split_at(checksum_start);
    let stored_checksum = u32::from_be_bytes(checksum_bytes.try_into().unwrap());
    if crc32fast::hash(contents) != stored_checksum {
        return Err(corrupt("checksum mismatch"));
    }

    let mut decoder = Decoder::new(contents);
    if decoder.take(MAGIC_BYTES.len())? != MAGIC_BYTES {
        return Err(corrupt("invalid magic bytes"));
    }
    let version = decoder.read_u16()?;
    if version != VERSION {
        return Err(corrupt(format!("unsupported version {version}")));
    }

    let entry_count = decoder.read_u32()?;
    let entries = decode_entries(&mut decoder, entry_count)?;
    decoder.finish()?;
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
            Err(Error::CorruptSstable(message)) if message == "checksum mismatch"
        ));
    }
}
