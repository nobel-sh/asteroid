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
    io::{Read, Seek, Write},
    path::Path,
};

use crate::{
    codec::Decoder,
    error::{Error, Result},
    write::WriteOp,
    Key, Value, WriteBatch,
};

const DELETE: u8 = 1;
const PUT: u8 = 2;
const MAGIC_BYTES: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const VERSION: u16 = 1;
const HEADER_SIZE: usize = MAGIC_BYTES.len() + size_of::<u16>() + size_of::<u32>();

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

fn decode_record(decoder: &mut Decoder<'_>) -> Result<Option<WalRecord>> {
    let record_start = decoder.position();
    if decoder.bytes().len() - record_start < HEADER_SIZE {
        return Ok(None);
    }

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
    let frame_end = decoder
        .position()
        .checked_add(payload_length)
        .and_then(|end| end.checked_add(size_of::<u32>()))
        .ok_or(crate::DecodeError::LengthOverflow)?;
    if frame_end > decoder.bytes().len() {
        return Ok(None);
    }
    let payload = decoder.take(payload_length)?;

    let checksum_end = decoder.position();
    let stored_checksum = decoder.read_u32()?;

    let actual_checksum = crc32fast::hash(&decoder.bytes()[record_start..checksum_end]);
    if stored_checksum != actual_checksum {
        return Err(Error::CorruptWal("checksum mismatch".to_owned()));
    }

    decode_payload(payload).map(Some)
}

fn validate_sequence_range(first_sequence: u64, operation_count: u32) -> Result<()> {
    if operation_count > 0 {
        first_sequence
            .checked_add(u64::from(operation_count) - 1)
            .ok_or(Error::SequenceExhausted)?;
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Wal {
    file: File,
    requires_recovery: bool,
}

impl Wal {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file,
            requires_recovery: false,
        })
    }

    pub(crate) fn append(&mut self, first_sequence: u64, batch: &WriteBatch) -> Result<()> {
        if self.requires_recovery {
            return Err(Error::WalRequiresRecovery);
        }
        let bytes = encode_record(first_sequence, batch)?;
        // An I/O failure may leave a partial frame; do not append behind it.
        self.requires_recovery = true;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.requires_recovery = false;
        Ok(())
    }

    pub(crate) fn read_records(&mut self) -> Result<Vec<WalRecord>> {
        self.requires_recovery = true;
        self.file.rewind()?;
        let mut buffer = Vec::new();
        self.file.read_to_end(&mut buffer)?;
        let mut decoder = Decoder::new(&buffer);
        let mut records = Vec::new();
        while !decoder.is_empty() {
            let record_start = decoder.position();
            match decode_record(&mut decoder)? {
                Some(record) => records.push(record),
                None => {
                    self.file.set_len(record_start as u64)?;
                    self.file.sync_all()?;
                    break;
                }
            }
        }
        self.requires_recovery = false;
        Ok(records)
    }

    pub(crate) fn size(&self) -> Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub(crate) fn reset(&mut self) -> Result<()> {
        self.file.set_len(0)?;
        self.file.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{decode_record, Wal, WalRecord};
    use crate::{codec::Decoder, WriteBatch};

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
        let decoded = decode_record(&mut decoder).unwrap().unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(decoded, expected);
    }
}
