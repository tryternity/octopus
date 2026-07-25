//! RecordError：octopus-record crate 的错误类型。

use crate::session::SessionState;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("helper binary not found at {0}")]
    HelperNotFound(PathBuf),

    #[error("helper spawn failed: {0}")]
    SpawnFailed(#[from] std::io::Error),

    #[error("helper error: code={code}, message={message}")]
    HelperError { code: String, message: String },

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("invalid session state: expected {expected:?}, actual {actual:?}")]
    InvalidState { expected: SessionState, actual: SessionState },

    #[error("session already running")]
    AlreadyRunning,

    #[error("session not running")]
    NotRunning,

    #[error("timeout waiting for {event}")]
    Timeout { event: &'static str },

    #[error("platform not implemented: {0}")]
    PlatformNotImplemented(&'static str),

    #[error("recording not found: id={0}")]
    NotFound(i64),

    #[error("DB error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type RecordResult<T> = Result<T, RecordError>;
