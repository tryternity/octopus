// crates/infra/src/consts.rs
// 跨 crate 共享的固定常量：集中管理随应用打包的小模型 / 配置文件路径 / VAD 行为参数，
// 方便开发期统一调整，消除散落在各 crate 中的硬编码。
//
// 路径常量均为相对 ~/.octopus/ 根目录的片段，由调用方 join 到 home。

/// Silero VAD 模型相对路径（~/.octopus/models/silero_vad_v4.onnx）。
/// 固定加载、随应用打包，不读配置 / HF 缓存——唯一 VAD 方案。
pub const SILERO_VAD_PATH: &str = "models/silero_vad_v4.onnx";

/// 兜底（默认）ASR 模型 source（路径标识，domain/name 格式，与其他 local 模型一致）。
/// zipformer-small 的 source，27M，builtin（source_type=0），首次启动下载。
pub const DEFAULT_ASR_MODEL_DIR: &str = "asr/zipformer-small";

/// VAD 伪流式连续语音强制截断阈值（秒）。缓冲区达到此时长仍未静音 → 强制切断送识别。
/// 兜底逻辑——正常人不会连续说这么久。原为 config 字段，因属实现细节（用户不可感知）改为常量。
pub const SEGMENT_DURATION_S: f64 = 20.0;

/// VAD 强制切断时保留下一段的 overlap 时长（毫秒）。仅在连续语音 ≥ `SEGMENT_DURATION_S`
/// 强制切断时生效（语句被硬切，需重叠保连贯）；静音切分是自然语句边界，不带 overlap。
/// 200ms ≈ 一个音节，给 ASR 引擎足够声学线索补全段首残字。原为 config 字段，因属实现细节改为常量。
pub const SEGMENT_OVERLAP_MS: f64 = 200.0;

/// 主图编码格式链：`<格式>:<质量>` 列表（`;` 分割），`clipboard::image::encode_to_webp` 按序尝试首个成功。
///
/// 2026-07-20 perf（基于 img-bench 实测，3176×1866 截图，release build）：
///
/// | 编码          | 耗时    | 体积    | 备注                  |
/// |---------------|---------|---------|-----------------------|
/// | WebP lossless | 1510ms  | 316KB   | 原 default，慢        |
/// | WebP q80      | 483ms   | 997KB   | 体积小但慢            |
/// | JPEG q85      | 56ms    | 1888KB  | **8.6x 加速** ✅ 推荐 |
///
/// 改 JPEG q85 单条：JPEG 编码不会失败（除非图像 >65500px，那种情况 app 整体已异常），
/// 多格式降级链是历史遗留（防 webp panic）。截图/剪贴板历史场景对画质要求够用，
/// 体积翻倍可接受（每图 2MB vs 1MB），换 8-10x 加速。
/// 想换回有损 WebP 或加 fallback，改本常量即可（如 `"webp:80;jpeg:85"`）。
pub const IMAGE_SAVE_QUALITY: &str = "jpeg:85";

/// 缩略图编码格式链：240×240 nearest resize 后的输出格式。
/// q10 极轻质量（thumb 仅作列表预览，不要求细节）；240×240 这么小，q10 vs q85 肉眼几乎无差。
pub const THUMB_SAVE_QUALITY: &str = "jpeg:10";
