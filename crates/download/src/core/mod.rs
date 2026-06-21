//! 通用下载核心。
pub mod error;
pub mod progress;
pub mod segment;
pub mod resume;
pub mod verify;
pub mod downloader;

pub use downloader::{Downloader, DownloadConfig, DownloadTask, ProbeResult};
