#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sequence number exhausted")]
    SequenceExhausted,

    #[error("WAL record is too large")]
    RecordTooLarge,

    #[error("corrupt WAL: {0}")]
    CorruptWal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
