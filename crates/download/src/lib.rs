#![warn(clippy::all)]
//! octopus-download：通用文件下载 crate（分块并发 + 断点续传 + 校验 + 镜像）。
//!
//! 单模块 `core`——纯通用下载器，无 HF API 依赖。
//! 模型下载由调用方提供 manifest（DB `secret_key` 字段）逐文件驱动，
//! URL 模板替换（`{huggingface}` 等）由调用方完成。

pub mod core;

// 顶层便捷 re-export
pub use crate::core::downloader::{Downloader, DownloadConfig, DownloadTask, ProbeResult};
pub use crate::core::error::DownloadError;
pub use crate::core::progress::Progress;
pub use crate::core::verify::Hash;
