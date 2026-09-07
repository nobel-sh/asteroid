#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("decoded length overflow")]
    LengthOverflow,

    #[error("truncated encoded data")]
    Truncated,

    #[error("trailing encoded data")]
    TrailingBytes,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("decode error: {0}")]
    Decode(#[from] DecodeError),

    #[error("sequence number exhausted")]
    SequenceExhausted,

    #[error("table ID exhausted")]
    TableIdExhausted,

    #[error("encoded record is too large")]
    RecordTooLarge,

    #[error("corrupt WAL: {0}")]
    CorruptWal(String),

    #[error("corrupt SSTable: {0}")]
    CorruptSstable(String),

    #[error("corrupt Manifest: {0}")]
    CorruptManifest(String),

    #[error("database is already in use by another instance")]
    DbLocked,
}

pub type Result<T> = std::result::Result<T, Error>;
