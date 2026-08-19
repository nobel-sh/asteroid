//! Data Arrangement for encoded WAL records.
//! Numeric fields use big-endian byte order and fields have no padding.
//!
//! ```text
//! record    = magic<4 bytes> + version<2 bytes> + payload_size<4 bytes> +
//!             payload<payload_size bytes> + checksum<4 bytes>
//!
//! payload   = first_sequence<8 bytes> + operation_count<4 bytes> +
//!             operation<operation_count entries>
//!
//! operation = kind<1 byte> + key_size<4 bytes> + value_size<4 bytes> +
//!             key<key_size bytes> + value<value_size bytes>
//!
//! kind = 1 (Delete) | 2 (Put)
//! ```
//!
//! Checksum is CRC32 and covers everything before the checksum.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use crate::{
    error::{Error, Result},
    Key, Value, WriteBatch, WriteOp,
};

const DELETE: u8 = 1;
const PUT: u8 = 2;
const MAGIC_BYTES: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const VERSION: u16 = 1;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct WalRecord {
    pub(crate) first_sequence: u64,
    pub(crate) batch: WriteBatch,
}

pub(crate) fn encode_record(first_sequence: u64, batch: &WriteBatch) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();

    bytes.extend_from_slice(&MAGIC_BYTES);
    bytes.extend_from_slice(&VERSION.to_be_bytes());
    let payload = encode_payload(first_sequence, batch)?;
    let payload_length = u32::try_from(payload.len()).map_err(|_| Error::RecordTooLarge)?;
    bytes.extend_from_slice(&payload_length.to_be_bytes());
    bytes.extend_from_slice(&payload);

    let checksum = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&checksum.to_be_bytes());

    Ok(bytes)
}

fn encode_payload(first_sequence: u64, batch: &WriteBatch) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    let operation_count = u32::try_from(batch.len()).map_err(|_| Error::RecordTooLarge)?;
    validate_sequence_range(first_sequence, operation_count)?;

    payload.extend_from_slice(&first_sequence.to_be_bytes());
    payload.extend_from_slice(&operation_count.to_be_bytes());

    for operation in &batch.operations {
        encode_operation(operation, &mut payload)?;
    }

    Ok(payload)
}

fn encode_operation(operation: &WriteOp, bytes: &mut Vec<u8>) -> Result<()> {
    match operation {
        WriteOp::Delete { key } => {
            bytes.push(DELETE);
            push_length(bytes, key.as_bytes().len())?;
            bytes.extend_from_slice(&0_u32.to_be_bytes());
            bytes.extend_from_slice(key.as_bytes());
        }
        WriteOp::Put { key, value } => {
            bytes.push(PUT);
            push_length(bytes, key.as_bytes().len())?;
            push_length(bytes, value.as_bytes().len())?;
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
    }
    Ok(())
}

fn push_length(bytes: &mut Vec<u8>, length: usize) -> Result<()> {
    let length = u32::try_from(length).map_err(|_| Error::RecordTooLarge)?;
    bytes.extend_from_slice(&length.to_be_bytes());
    Ok(())
}

fn decode_payload(bytes: &[u8]) -> Result<WalRecord> {
    let mut decoder = Decoder::new(bytes);
    let first_sequence = decoder.read_u64()?;
    let operation_count = decoder.read_u32()?;
    validate_sequence_range(first_sequence, operation_count)
        .map_err(|_| Error::CorruptWal("sequence range overflow".to_owned()))?;

    let mut operations = Vec::new();
    for _ in 0..operation_count {
        let kind = decoder.read_u8()?;
        let key_length = decoder.read_u32()? as usize;
        let value_length = decoder.read_u32()? as usize;
        let key = Key::from(decoder.take(key_length)?.to_vec());

        let operation = match kind {
            DELETE if value_length == 0 => WriteOp::Delete { key },
            DELETE => {
                return Err(Error::CorruptWal("delete operation has a value".to_owned()));
            }
            PUT => {
                let value = Value::from(decoder.take(value_length)?.to_vec());
                WriteOp::Put { key, value }
            }
            _ => {
                return Err(Error::CorruptWal(format!("unknown operation kind {kind}")));
            }
        };
        operations.push(operation);
    }

    decoder.finish()?;

    Ok(WalRecord {
        first_sequence,
        batch: WriteBatch { operations },
    })
}

fn decode_record(decoder: &mut Decoder) -> Result<WalRecord> {
    let record_start = decoder.position();

    if decoder.take(MAGIC_BYTES.len())? != MAGIC_BYTES {
        return Err(Error::CorruptWal("invalid magic bytes".to_owned()));
    }

    let version = decoder.read_u16()?;
    if version != VERSION {
        return Err(Error::CorruptWal(format!(
            "unsupported WAL version {version}"
        )));
    }

    let payload_length = decoder.read_u32()? as usize;
    let payload = decoder.take(payload_length)?;

    let checksum_end = decoder.position();
    let stored_checksum = decoder.read_u32()?;

    let actual_checksum = crc32fast::hash(&decoder.bytes[record_start..checksum_end]);
    if stored_checksum != actual_checksum {
        return Err(Error::CorruptWal("checksum mismatch".to_owned()));
    }

    decode_payload(payload)
}

fn validate_sequence_range(first_sequence: u64, operation_count: u32) -> Result<()> {
    if operation_count > 0 {
        first_sequence
            .checked_add(u64::from(operation_count) - 1)
            .ok_or(Error::SequenceExhausted)?;
    }
    Ok(())
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
            .ok_or_else(|| Error::CorruptWal("record length overflow".to_owned()))?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| Error::CorruptWal("truncated record".to_owned()))?;
        self.cursor = end;
        Ok(value)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn finish(&self) -> Result<()> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::CorruptWal("trailing bytes".to_owned()))
        }
    }

    fn position(&self) -> usize {
        self.cursor
    }

    fn is_empty(&self) -> bool {
        self.cursor == self.bytes.len()
    }
}

#[derive(Debug)]
pub(crate) struct Wal {
    file: File,
}

impl Wal {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        Ok(Self { file })
    }

    pub(crate) fn append(&mut self, first_sequence: u64, batch: &WriteBatch) -> Result<()> {
        let bytes = encode_record(first_sequence, batch)?;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        Ok(())
    }

    pub(crate) fn read_records(&mut self) -> Result<Vec<WalRecord>> {
        let mut buffer = Vec::new();
        self.file.read_to_end(&mut buffer)?;
        let mut decoder = Decoder::new(&buffer);
        let mut records = Vec::new();
        while !decoder.is_empty() {
            records.push(decode_record(&mut decoder)?);
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{decode_record, Decoder, Wal, WalRecord};
    use crate::WriteBatch;

    #[test]
    fn batch_round_trips_through_a_file() {
        let mut batch = WriteBatch::new();
        batch.put("foo", "bar");
        batch.delete("old-key");

        let expected = WalRecord {
            first_sequence: 10,
            batch,
        };
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("wal-{}-{unique}.log", std::process::id()));

        let mut wal = Wal::open(&path).unwrap();
        wal.append(expected.first_sequence, &expected.batch)
            .unwrap();
        drop(wal);
        let bytes = fs::read(&path).unwrap();
        let mut decoder = Decoder::new(&bytes);
        let decoded = decode_record(&mut decoder).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(decoded, expected);
    }
}
