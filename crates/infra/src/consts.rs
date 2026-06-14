// crates/infra/src/consts.rs
// 跨 crate 共享的固定常量：集中管理随应用打包的小模型 / 配置文件路径，
// 方便开发期统一调整，消除散落在各 crate 中的硬编码。
//
// 所有路径均为相对 ~/.octopus/ 根目录的片段，由调用方 join 到 home。

/// Silero VAD 模型相对路径（~/.octopus/models/silero_vad_v4.onnx）。
/// 固定加载、随应用打包，不读配置 / HF 缓存——唯一 VAD 方案。
pub const SILERO_VAD_PATH: &str = "models/silero_vad_v4.onnx";

/// 兜底（默认）ASR 模型目录相对路径（~/.octopus/models/zipformer）。
/// zipformer-small-ctc 的 source，27M，随应用打包，开箱即用。
pub const DEFAULT_ASR_MODEL_DIR: &str = "models/zipformer";

/// 自定义润色 system prompt 文件名（~/.octopus/VOICE_POLISH.md）。
/// 文件存在且非空时覆盖 llm 内置默认 prompt。
pub const VOICE_POLISH_FILE: &str = "VOICE_POLISH.md";
