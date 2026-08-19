#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("sequence number exhausted")]
    SequenceExhausted,
}

pub type Result<T> = std::result::Result<T, Error>;
