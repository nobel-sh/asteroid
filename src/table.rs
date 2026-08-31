use super::{
    error::{Error, Result},
    memtable::{Entry, Key, Lookup, Memtable, StoredValue},
};

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

const DELETE: u8 = 1;
const PUT: u8 = 2;
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

    fn encode_entry(key: &Key, entry: &Entry, buffer: &mut Vec<u8>) -> Result<()> {
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

    pub(crate) fn create(directory: &PathBuf, id: u32, memtable: &Memtable) -> Result<Self> {
        let mut buffer = Vec::new();

        buffer.extend_from_slice(&MAGIC_BYTES);
        buffer.extend_from_slice(&VERSION.to_be_bytes());
        let entry_count =
            u32::try_from(memtable.entry_count()).map_err(|_| Error::RecordTooLarge)?;
        buffer.extend_from_slice(&entry_count.to_be_bytes());

        for (key, entry) in memtable.iter() {
            Self::encode_entry(key, entry, &mut buffer)?;
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
    decoder.finish()?;
    Ok(entries)
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptSstable(message.into())
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| corrupt("entry length overflow"))?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| corrupt("truncated entry"))?;
        self.cursor = end;
        Ok(value)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn finish(&self) -> Result<()> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(corrupt("trailing bytes"))
        }
    }
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
