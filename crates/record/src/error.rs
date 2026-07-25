//! RecordError：octopus-record crate 的错误类型。

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("platform not implemented: {0}")]
    PlatformNotImplemented(&'static str),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type RecordResult<T> = Result<T, RecordError>;
