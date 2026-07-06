use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PaddleOcrError {
    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("model resolution failed: {0}")]
    ModelResolve(String),

    #[error("file not found: {0}")]
    FileNotFound(PathBuf),

    #[error("invalid image: {0}")]
    InvalidImage(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("decoding failed: {0}")]
    Decode(String),

    #[error("unsupported provider for v1: {0}")]
    UnsupportedProvider(String),

    #[error("unsupported runtime backend for v1: {0}")]
    UnsupportedBackend(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Ort(#[from] ort::Error),
}

pub type Result<T> = std::result::Result<T, PaddleOcrError>;
