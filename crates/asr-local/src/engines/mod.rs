//! ASR 引擎域：离线引擎 trait / 实现 + 共享特征设施 + 批处理 pipeline。
//!
//! 子模块：
//! - [`engine`]：`OfflineAsrEngine` trait + `AsrEngineManager`（引擎注册 / 路由）。
//! - [`whisper`] / [`paraformer`] / [`qwen3_asr`] / [`zipformer`] / [`firered`] /
//!   [`moonshine`] / [`sensevoice_orig`]：各离线引擎实现。
//! - [`feature`]：共享特征提取（mel filterbank / LFR / 窗口），crate 私有。
//! - [`fbank`]：80-bin log-fbank + LFR 堆叠（re-export feature 部分接口）。
//! - [`whisper_mel_matrix`]：Whisper mel 滤波器组静态权重表。
//! - [`pipeline`]：`transcribe_batch` / `PipelineConfig`（VAD 分段编排 + 后处理）。
//!
//! 公开子模块经 lib.rs 的 `pub use engines::{...}` re-export 到 crate 根，
//! 保持 `octopus_asr_local::engine::AsrEngineManager` 等历史路径不变；
//! `feature` 保持 crate 私有（`pub(crate)`），不进 re-export。

pub mod engine;
pub mod whisper;
pub mod paraformer;
pub mod qwen3_asr;
pub mod fbank;
pub mod sensevoice_orig;
pub mod firered;
pub mod moonshine;
pub mod zipformer;
pub mod pipeline;
pub mod whisper_mel_matrix;

pub(crate) mod feature;
