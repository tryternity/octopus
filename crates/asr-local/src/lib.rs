#![warn(clippy::all)]
//! octopus-asr-local: ASR inference library (Whisper, SenseVoice, Paraformer, Qwen3-ASR, FireRedASR2, Moonshine, Zipformer, Silero VAD)
//!
//! 模型配置存于 `~/.octopus/octopus.db`（models 表，唯一来源）；应用配置读 `~/.octopus/config.yaml`。

pub mod audio;
pub mod config;
pub(crate) mod feature;
pub mod manifest;
pub use octopus_infra::db;
pub mod engine;
pub mod paraformer;
pub mod qwen3_asr;
pub mod fbank;
pub mod sensevoice_orig;
pub mod streaming_paraformer;
pub mod streaming_zipformer;
pub mod streaming_engine;
pub mod streaming_runner;
pub mod whisper;
pub mod whisper_mel_matrix;
pub mod firered;
pub mod moonshine;
pub mod zipformer;
pub mod pipeline;

#[cfg(test)]
pub(crate) mod test_helpers;

/// 句间分隔符（按 language 选择），全 workspace ASR 文本拼接复用。
pub use paraformer::sentence_separator;

// ── 功能域子目录（2026-07-30 重组）──
// 文件搬入子目录后，lib.rs 用 pub use 逐项 re-export 保持 octopus_asr_local::<module>::xxx 路径不变。

// audio/ 域：preprocess（原 audio.rs）+ vad + denoise。
// preprocess 的公开项经 audio/mod.rs 的 `pub use preprocess::*` 提升到 audio:: 顶层，
// 故 octopus_asr_local::audio::read_wav_16k 等路径不变；vad / denoise 再 re-export 到 crate 根。
pub use audio::{vad, denoise};

pub mod text;
pub use text::{corrector, hotword, hans, itn, miner};


