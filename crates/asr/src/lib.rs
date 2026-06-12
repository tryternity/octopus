//! octopus-asr: ASR inference library (Whisper, SenseVoice, Paraformer, Qwen3-ASR, Silero VAD)
//!
//! All models are discovered via `~/.octopus/model.json` and `~/.octopus/config.yaml`.

pub mod audio;
pub mod config;
pub mod engine;
pub mod paraformer;
pub mod qwen3_asr;
pub mod sensevoice;
pub mod streaming_paraformer;
pub mod streaming_zipformer;
pub mod vad;
pub mod whisper;
pub mod whisper_mel_matrix;
pub mod zipformer;

