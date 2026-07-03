// crates/infra/src/consts.rs
// 跨 crate 共享的固定常量：集中管理随应用打包的小模型 / 配置文件路径 / VAD 行为参数，
// 方便开发期统一调整，消除散落在各 crate 中的硬编码。
//
// 路径常量均为相对 ~/.octopus/ 根目录的片段，由调用方 join 到 home。

/// Silero VAD 模型相对路径（~/.octopus/models/silero_vad_v4.onnx）。
/// 固定加载、随应用打包，不读配置 / HF 缓存——唯一 VAD 方案。
pub const SILERO_VAD_PATH: &str = "models/silero_vad_v4.onnx";

/// 兜底（默认）ASR 模型目录相对路径（~/.octopus/models/zipformer）。
/// zipformer-small-ctc 的 source，27M，随应用打包，开箱即用。
pub const DEFAULT_ASR_MODEL_DIR: &str = "models/zipformer";

/// VAD 伪流式连续语音强制截断阈值（秒）。缓冲区达到此时长仍未静音 → 强制切断送识别。
/// 兜底逻辑——正常人不会连续说这么久。原为 config 字段，因属实现细节（用户不可感知）改为常量。
pub const SEGMENT_DURATION_S: f64 = 20.0;

/// VAD 强制切断时保留下一段的 overlap 时长（毫秒）。仅在连续语音 ≥ `SEGMENT_DURATION_S`
/// 强制切断时生效（语句被硬切，需重叠保连贯）；静音切分是自然语句边界，不带 overlap。
/// 200ms ≈ 一个音节，给 ASR 引擎足够声学线索补全段首残字。原为 config 字段，因属实现细节改为常量。
pub const SEGMENT_OVERLAP_MS: f64 = 200.0;

/// 超长截图（>16383px）WebP 编码全失败时的 JPEG 兜底质量。
pub const BOTTOM_JPEG_QUALITY: u8 = 50;
