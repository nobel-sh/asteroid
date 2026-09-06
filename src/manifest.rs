use crate::{codec::Decoder, DecodeError, Error, Result};
use std::{
    fs::OpenOptions,
    io::{ErrorKind, Read, Write},
    path::Path,
};

const MAGIC_BYTES: [u8; 4] = *b"MNFT";
const VERSION: u16 = 1;

#[derive(Clone, Debug)]
pub(crate) struct Manifest {
    pub(crate) last_sequence: u64,
    pub(crate) next_table_id: u32,
    pub(crate) table_ids: Vec<u32>,
}

impl Manifest {
    pub(crate) fn store(&self, directory: &Path) -> Result<()> {
        self.validate()?;
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&MAGIC_BYTES);
        buffer.extend_from_slice(&VERSION.to_be_bytes());
        buffer.extend_from_slice(&self.last_sequence.to_be_bytes());
        buffer.extend_from_slice(&self.next_table_id.to_be_bytes());
        let table_count = u32::try_from(self.table_ids.len()).map_err(|_| Error::RecordTooLarge)?;
        buffer.extend_from_slice(&table_count.to_be_bytes());
        for id in &self.table_ids {
            buffer.extend_from_slice(&id.to_be_bytes());
        }
        let hash = crc32fast::hash(&buffer);
        buffer.extend_from_slice(&hash.to_be_bytes());

        let tmp_file_name = directory.join("MANIFEST.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_file_name)?;
        file.write_all(&buffer)?;
        file.sync_all()?;

        let file_name = directory.join("MANIFEST");
        std::fs::rename(tmp_file_name, file_name)?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    }

    pub(crate) fn load(directory: &Path) -> Result<Option<Self>> {
        let mut buffer = Vec::new();
        let path = directory.join("MANIFEST");
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        file.read_to_end(&mut buffer)?;
        let mut decoder = Decoder::new(&buffer);
        if decoder.take(MAGIC_BYTES.len()).map_err(decode_error)? != MAGIC_BYTES {
            return Err(corrupt("invalid magic bytes"));
        }
        let version = decoder.read_u16().map_err(decode_error)?;
        if version != VERSION {
            return Err(corrupt(format!("unsupported version {version}")));
        }
        let last_sequence = decoder.read_u64().map_err(decode_error)?;
        let next_table_id = decoder.read_u32().map_err(decode_error)?;
        let table_id_count = decoder.read_u32().map_err(decode_error)?;
        let mut table_ids = Vec::new();
        for _ in 0..table_id_count {
            let id = decoder.read_u32().map_err(decode_error)?;
            table_ids.push(id);
        }
        let calculated_hash = crc32fast::hash(&buffer[0..decoder.position()]);
        let read_hash = decoder.read_u32().map_err(decode_error)?;
        if calculated_hash != read_hash {
            return Err(corrupt("checksum mismatch"));
        }
        decoder.finish().map_err(decode_error)?;
        let manifest = Manifest {
            last_sequence,
            next_table_id,
            table_ids,
        };
        manifest.validate()?;
        Ok(Some(manifest))
    }

    fn validate(&self) -> Result<()> {
        if self.table_ids.windows(2).any(|ids| ids[0] >= ids[1]) {
            return Err(corrupt("table IDs are not strictly increasing"));
        }
        if self
            .table_ids
            .last()
            .is_some_and(|id| self.next_table_id <= *id)
        {
            return Err(corrupt("next table ID is not greater than live table IDs"));
        }
        Ok(())
    }
}

fn decode_error(error: DecodeError) -> Error {
    corrupt(error.to_string())
}

fn corrupt(message: impl Into<String>) -> Error {
    Error::CorruptManifest(message.into())
}
