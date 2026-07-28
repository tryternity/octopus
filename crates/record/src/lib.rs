//! octopus-record：屏幕录制纯逻辑库。
//!
//! 职责：
//! - spawn 平台 helper 子进程（macOS: octopus-sck-helper）
//! - 通过 JSON-over-stdio 协议控制录制（start/stop/pause/resume）
//! - 录屏元数据入库（recordings 表）
//!
//! 不含 UI，不含 Tauri 命令（命令在 crates/desktop/src/record_commands.rs）。

pub mod audio_tracks;
pub use audio_tracks::{AudioTrack, AudioTrackSource, RawAudioTrack, infer_audio_tracks};
pub mod error;
pub mod protocol;
pub use protocol::*;
pub mod session;
pub use session::{RecordSession, SessionState, StartedInfo, StoppedInfo};
pub mod store;
pub mod subtitle;
pub mod platform;

pub use error::{RecordError, RecordResult};
pub use store::{RecordingMeta, RecordStore, ListFilter};
