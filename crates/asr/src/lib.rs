//! octopus-asr: ASR inference library (Whisper, SenseVoice, Paraformer, Qwen3-ASR, Silero VAD)
//!
//! 模型配置存于 `~/.octopus/octopus.db`（models 表，唯一来源）；应用配置读 `~/.octopus/config.yaml`。

pub mod audio;
pub mod config;
pub use octopus_infra::db;
pub mod engine;
pub mod paraformer;
pub mod qwen3_asr;
pub mod sensevoice;
pub mod streaming_paraformer;
pub mod streaming_zipformer;
pub mod streaming_engine;
pub mod vad;
pub mod denoise;
pub mod whisper;
pub mod whisper_mel_matrix;
pub mod moonshine;
pub mod zipformer;
pub mod corrector;
pub mod hans;


