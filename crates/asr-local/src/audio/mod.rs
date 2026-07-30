//! 音频处理域：预处理（resample / wav 读写 / speech 过滤）+ VAD + 降噪。
//!
//! 子模块：
//! - [`preprocess`]：WAV 解码、16kHz 重采样、VAD 段切分等（原平铺 `audio.rs`）。
//! - [`vad`]：Silero VAD 推理（编译期内嵌 `silero_vad_v4.onnx`）。
//! - [`denoise`]：DeepFilterNet3 降噪。
//!
//! `preprocess` 的公开项通过 `pub use preprocess::*` 提升到 `audio::` 顶层，
//! 保持 `octopus_asr_local::audio::read_wav_16k` 等历史路径不变；
//! `vad` / `denoise` 子模块经 lib.rs 的 `pub use audio::{vad, denoise}` 再 re-export 到 crate 根，
//! 保持 `octopus_asr_local::vad::SileroVad` / `::denoise::DenoiseProcessor` 路径不变。

pub mod preprocess;
pub mod vad;
pub mod denoise;

pub use preprocess::*;
