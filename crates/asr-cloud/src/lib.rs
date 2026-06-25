//! 云端 ASR（cli/server 批处理用）。
//!
//! 4 provider（Aliyun/ByteDance/Tencent/Baidu）WSS 协议层 + 批引擎（impl
//! `octopus_asr::engine::OfflineAsrEngine`）。协议层从 `octopus-desktop` 复刻
//!（见各 `*_stream.rs`），改造为不依赖 tauri runtime：`open()` 内部用 `tokio::spawn`，
//! 调用方（`CloudBatchEngine`）在自有 tokio runtime 上 `block_on` 驱动。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md`。

pub mod cloud_types;
pub mod aliyun_stream;
pub mod bytedance_stream;
pub mod tencent_stream;
pub mod baidu_stream;
