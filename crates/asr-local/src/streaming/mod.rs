//! 流式 ASR 编排域：会话管理 + runner + 两种流式引擎实现。
//!
//! 子模块：
//! - [`streaming_engine`]：`StreamingSession` / `StreamingSessionManager`（Paraformer / Zipformer 路由）。
//! - [`streaming_runner`]：`StreamingEngine` trait + `StreamingRunner` + `TranscriptEvent`。
//! - [`streaming_paraformer`]：chunk-by-chunk Paraformer（CIF + decoder 状态）。
//! - [`streaming_zipformer`]：chunk-by-chunk Zipformer（CTC + stateful caches）。
//!
//! 子模块经 lib.rs 的 `pub use streaming::{...}` re-export 到 crate 根，
//! 保持 `octopus_asr_local::streaming_engine::` 等历史路径不变。

pub mod streaming_engine;
pub mod streaming_runner;
pub mod streaming_paraformer;
pub mod streaming_zipformer;
