//! 下载错误类型 + HTTP 状态分类。

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("fatal: HTTP {status} for {url}")]
    Fatal { status: u16, url: String },

    #[error("transient ({kind}): {message}")]
    Transient { kind: TransientKind, message: String },

    #[error("cancelled")]
    Cancelled,

    #[error("hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch { path: PathBuf, expected: String, actual: String },

    #[error("hf api error: HTTP {status} for {url}")]
    HfApi { status: u16, url: String },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientKind {
    ServerError,
    RateLimited,
    Timeout,
    Network,
}

impl TransientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransientKind::ServerError => "server_error",
            TransientKind::RateLimited => "rate_limited",
            TransientKind::Timeout => "timeout",
            TransientKind::Network => "network",
        }
    }
}

impl fmt::Display for TransientKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 把 HTTP status 分类：Fatal（不重试）/ Transient（可重试）。
/// 4xx 除 408/429 → Fatal；5xx/408/429 → Transient；3xx/2xx → None（成功）。
pub fn classify_status(status: u16) -> Option<ErrorClass> {
    match status {
        408 => Some(ErrorClass::Transient(TransientKind::Timeout)),
        429 => Some(ErrorClass::Transient(TransientKind::RateLimited)),
        400..=499 => Some(ErrorClass::Fatal),
        500..=599 => Some(ErrorClass::Transient(TransientKind::ServerError)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Fatal,
    Transient(TransientKind),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_4xx_is_fatal() {
        assert_eq!(classify_status(404), Some(ErrorClass::Fatal));
        assert_eq!(classify_status(403), Some(ErrorClass::Fatal));
    }

    #[test]
    fn classify_408_429_are_transient() {
        assert_eq!(
            classify_status(408),
            Some(ErrorClass::Transient(TransientKind::Timeout))
        );
        assert_eq!(
            classify_status(429),
            Some(ErrorClass::Transient(TransientKind::RateLimited))
        );
    }

    #[test]
    fn classify_5xx_is_transient_server() {
        assert_eq!(
            classify_status(500),
            Some(ErrorClass::Transient(TransientKind::ServerError))
        );
        assert_eq!(
            classify_status(503),
            Some(ErrorClass::Transient(TransientKind::ServerError))
        );
    }

    #[test]
    fn classify_2xx_3xx_is_none() {
        assert_eq!(classify_status(200), None);
        assert_eq!(classify_status(301), None);
    }
}
