#![warn(clippy::all)]
//! octopus-asr-local: ASR inference library (Whisper, SenseVoice, Paraformer, Qwen3-ASR, FireRedASR2, Moonshine, Zipformer, Silero VAD)
//!
//! 模型配置存于 `~/.octopus/octopus.db`（models 表，唯一来源）；应用配置读 `~/.octopus/config.yaml`。

pub mod audio;
pub mod config;
pub mod manifest;
pub use octopus_infra::db;

#[cfg(test)]
pub(crate) mod test_helpers;

// ── 功能域子目录（2026-07-30 重组）──
// 文件搬入子目录后，lib.rs 用 pub use 逐项 re-export 保持 octopus_asr_local::<module>::xxx 路径不变。

// audio/ 域：preprocess（原 audio.rs）+ vad + denoise。
// preprocess 的公开项经 audio/mod.rs 的 `pub use preprocess::*` 提升到 audio:: 顶层，
// 故 octopus_asr_local::audio::read_wav_16k 等路径不变；vad / denoise 再 re-export 到 crate 根。
pub use audio::{vad, denoise};

// engines/ 域：离线引擎 trait / 实现 + 共享特征 + pipeline。
// feature 保持 crate 私有（pub(crate) mod feature 在 engines/mod.rs 内），
// 经 pub(crate) use 回到 crate 根，保持 crate::feature::... 内部路径有效，但不对外暴露。
pub mod engines;
pub use engines::{
    engine, whisper, paraformer, qwen3_asr, fbank, sensevoice_orig, firered, moonshine, zipformer,
    pipeline, whisper_mel_matrix,
};
pub(crate) use engines::feature;

// streaming/ 域：会话管理 + runner + Paraformer/Zipformer 流式实现。
pub mod streaming;
pub use streaming::{streaming_engine, streaming_runner, streaming_paraformer, streaming_zipformer};

pub mod text;
pub use text::{corrector, hotword, hans, itn, miner};

/// 句间分隔符（按 language 选择），全 workspace ASR 文本拼接复用。
pub use paraformer::sentence_separator;


