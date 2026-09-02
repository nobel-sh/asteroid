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

    #[error("encoded record is too large")]
    RecordTooLarge,

    #[error("corrupt WAL: {0}")]
    CorruptWal(String),

    #[error("corrupt SSTable: {0}")]
    CorruptSstable(String),
}

pub type Result<T> = std::result::Result<T, Error>;
