//! octopus-record：屏幕录制纯逻辑库。
//!
//! 职责：
//! - spawn 平台 helper 子进程（macOS: octopus-sck-helper）
//! - 通过 JSON-over-stdio 协议控制录制（start/stop/pause/resume）
//! - 录屏元数据入库（recordings 表）
//!
//! 不含 UI，不含 Tauri 命令（命令在 crates/desktop/src/record_commands.rs）。

pub mod error;
pub mod protocol;
pub mod session;
pub mod store;
mod platform;

pub use error::{RecordError, RecordResult};
