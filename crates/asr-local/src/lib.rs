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
pub mod vad;
pub mod denoise;
pub mod whisper;
pub mod whisper_mel_matrix;
pub mod firered;
pub mod moonshine;
pub mod zipformer;
pub mod corrector;
pub mod hans;
pub mod pipeline;

/// 句间分隔符（按 language 选择），全 workspace ASR 文本拼接复用。
pub use paraformer::sentence_separator;


