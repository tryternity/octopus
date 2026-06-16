//! 统一应用配置 schema（config.yaml 的统一定义）。
//!
//! 本模块是 config.yaml 的唯一 schema 来源，所有 crate（asr/desktop/cli 等）共享。
//! 多余字段对不使用它们的 crate 无害——各 crate 只读自己关心的字段。

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::octopus_config_home;

/// LLM 润色模式（config.yaml 的 polish_mode 字段，整数 0/1/2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolishMode {
    /// 0 — 完全不润色（默认）
    #[default]
    Disabled,
    /// 1 — 仅最终润色（识别结束后润色一次）
    FinalOnly,
    /// 2 — 中间润色 + 最终润色
    Intermediate,
}

impl<'de> Deserialize<'de> for PolishMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let n = u8::deserialize(deserializer)?;
        Ok(match n {
            0 => PolishMode::Disabled,
            1 => PolishMode::FinalOnly,
            2 => PolishMode::Intermediate,
            other => {
                log::warn!("polish_mode={} 非法（应为 0/1/2），回退 0(Disabled)", other);
                PolishMode::Disabled
            }
        })
    }
}

impl Serialize for PolishMode {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            PolishMode::Disabled => 0,
            PolishMode::FinalOnly => 1,
            PolishMode::Intermediate => 2,
        })
    }
}

/// 应用完整配置，读取自 ~/.octopus/config.yaml，字段缺失时使用默认值。
///
/// 各端按需读取字段：cli 只用 microphone；desktop 用全部；asr 用 asr_engine 解析引擎。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    /// ASR 引擎模式: embedded | websocket | grpc
    #[serde(default = "default_engine_mode")]
    pub engine_mode: String,

    /// WebSocket 远程地址（engine_mode = websocket 时使用）
    #[serde(default = "default_remote_url")]
    pub remote_url: String,

    /// gRPC 端点（engine_mode = grpc 时使用）
    #[serde(default = "default_grpc_endpoint")]
    pub grpc_endpoint: String,

    /// ASR 引擎选择：DB models 表中的 name（精确匹配）；空/不匹配则回退兜底引擎。
    /// 显式参数（cli --model、server 请求 engine、AsrEngineManager.switch_model）优先级更高，不走此字段。
    #[serde(default)]
    pub asr_engine: String,

    /// 语言: auto | zh | en | ja | ko
    #[serde(default = "default_language")]
    pub language: String,

    /// 全局快捷键
    #[serde(default = "default_shortcut")]
    pub shortcut: String,

    /// 粘贴方式: clipboard | direct | none
    #[serde(default = "default_paste_method")]
    pub paste_method: String,

    /// 粘贴后是否把识别结果写入剪贴板（默认 true，方便他处再粘贴）。
    /// false 时保留用户原剪贴板内容（等同旧行为）。
    #[serde(default = "default_write_to_clipboard")]
    pub write_to_clipboard: bool,

    /// 麦克风名称（空 = 系统默认）
    #[serde(default)]
    pub microphone: String,

    /// VAD 伪流式：音频缓冲区累积时长阈值（秒）
    /// 缓冲区达到此时长时自动发送识别，默认 5.0 秒
    #[serde(default = "default_segment_duration")]
    pub segment_duration: f64,

    /// VAD 伪流式：静音触发识别的时长阈值（毫秒）
    /// 检测到语音后静音超过此时长即发送识别，默认 500 毫秒
    #[serde(default = "default_segment_silence")]
    pub segment_silence: f64,

    /// VAD 伪流式：相邻分段 overlap 时长（毫秒）
    /// 每段识别音频前会拼接前一段末尾此毫秒数的音频，确保识别文本连续性，默认 200 毫秒
    #[serde(default = "default_segment_overlap")]
    pub segment_overlap: f64,

    /// overlay 位置: top | bottom | none
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,

    /// 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
    #[serde(default)]
    pub polish_mode: PolishMode,

    /// 中间润色间隔（秒），0 = 仅最终润色
    #[serde(default = "default_polish_interval")]
    pub polish_interval: f64,

    /// 停顿驱动中间润色的静音阈值（毫秒）：静音达此值即触发全量润色（mode=2 only）
    #[serde(default = "default_pause_polish_threshold_ms")]
    pub pause_polish_threshold_ms: f64,

    /// 当前润色使用的 LLM 模型，格式为 "PREFIX:NAME"（见 `parse_model_spec`）：
    /// - "local:NAME" → is_local=true AND name（本地 LLM，如 Ollama）
    /// - "CATEGORY:NAME" → category AND name（如 "bigmodel:glm-4-flashx"）
    /// - "NAME"（无冒号）→ 仅按 name（向后兼容）
    #[serde(default = "default_polish_llm")]
    pub polish_llm: String,

    /// 是否使用 ASR 硬件加速
    #[serde(default = "default_asr_hardware_accelerated")]
    pub asr_hardware_accelerated: bool,

    /// 是否对 ASR 输出进行纠错与热词校正
    #[serde(default = "default_asr_correct")]
    pub asr_correct: bool,

    /// 是否启用 DeepFilterNet3 环境降噪（录音送 ASR 前降噪）
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
}

fn default_engine_mode() -> String {
    "embedded".into()
}
fn default_remote_url() -> String {
    "ws://127.0.0.1:3000/ws/stream".into()
}
fn default_grpc_endpoint() -> String {
    "http://127.0.0.1:50051".into()
}
fn default_language() -> String {
    "auto".into()
}
fn default_shortcut() -> String {
    "CmdOrCtrl+Shift+Space".into()
}
fn default_paste_method() -> String {
    "clipboard".into()
}
fn default_write_to_clipboard() -> bool {
    true
}
fn default_overlay_position() -> String {
    "top".into()
}
fn default_polish_interval() -> f64 {
    5.0
}
fn default_pause_polish_threshold_ms() -> f64 {
    600.0
}
fn default_polish_llm() -> String {
    "bigmodel:glm-4-flashx".into()
}
fn default_asr_hardware_accelerated() -> bool {
    false
}
fn default_asr_correct() -> bool {
    false
}
fn default_denoise_enabled() -> bool {
    true
}
fn default_segment_duration() -> f64 {
    5.0
}
fn default_segment_silence() -> f64 {
    500.0
}
fn default_segment_overlap() -> f64 {
    200.0
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            engine_mode: default_engine_mode(),
            remote_url: default_remote_url(),
            grpc_endpoint: default_grpc_endpoint(),
            // 未配置 asr_engine → 空，由 asr::resolve_active_engine 回退 to 兜底引擎
            asr_engine: String::new(),
            language: default_language(),
            shortcut: default_shortcut(),
            paste_method: default_paste_method(),
            write_to_clipboard: default_write_to_clipboard(),
            microphone: String::new(),
            segment_duration: default_segment_duration(),
            segment_silence: default_segment_silence(),
            segment_overlap: default_segment_overlap(),
            overlay_position: default_overlay_position(),
            polish_mode: PolishMode::default(),
            polish_interval: default_polish_interval(),
            pause_polish_threshold_ms: default_pause_polish_threshold_ms(),
            polish_llm: default_polish_llm(),
            asr_hardware_accelerated: default_asr_hardware_accelerated(),
            asr_correct: default_asr_correct(),
            denoise_enabled: default_denoise_enabled(),
        }
    }
}

/// 从 ~/.octopus/config.yaml 加载应用配置。
///
/// 文件不存在或字段缺失时使用默认值（不报错）。文件存在但解析失败才返回 Err。
/// 注意：不缓存——调用方各自决定是否缓存（asr 侧引擎配置另有 OnceLock 缓存）。
pub fn load_config() -> Result<AppConfig> {
    let config_path = octopus_config_home().join("config.yaml");

    if !config_path.exists() {
        return Ok(AppConfig::default());
    }

    let text = std::fs::read_to_string(&config_path)?;
    let config: AppConfig = serde_yaml::from_str(&text)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_mode_deserialize_values() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("0").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("1").unwrap(), PolishMode::FinalOnly);
        assert_eq!(serde_yaml::from_str::<PolishMode>("2").unwrap(), PolishMode::Intermediate);
    }

    #[test]
    fn polish_mode_invalid_falls_back_to_disabled() {
        assert_eq!(serde_yaml::from_str::<PolishMode>("3").unwrap(), PolishMode::Disabled);
        assert_eq!(serde_yaml::from_str::<PolishMode>("99").unwrap(), PolishMode::Disabled);
    }

    #[test]
    fn polish_mode_default_is_disabled() {
        assert_eq!(PolishMode::default(), PolishMode::Disabled);
    }

    #[test]
    fn write_to_clipboard_defaults_to_true() {
        // 空 yaml → 所有字段走 serde 默认；write_to_clipboard 应默认 true
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(cfg.write_to_clipboard, "write_to_clipboard 应默认 true");
    }

    #[test]
    fn pause_polish_threshold_ms_defaults_to_600() {
        // 空 yaml → pause_polish_threshold_ms 应默认 600（毫秒）
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(cfg.pause_polish_threshold_ms, 600.0);
    }

    #[test]
    fn asr_hardware_accelerated_defaults_to_false() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(!cfg.asr_hardware_accelerated);
    }

    #[test]
    fn asr_correct_defaults_to_false() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(!cfg.asr_correct);
    }

    #[test]
    fn app_config_serialize_round_trip_preserves_overrides() {
        // 构造一个带覆盖值的 AppConfig（从 yaml 解析）
        let yaml = "asr_engine: whisper-small\npolish_mode: 2\nmicrophone: \"My Mic\"\n";
        let cfg: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.asr_engine, "whisper-small");
        assert_eq!(cfg.polish_mode, PolishMode::Intermediate);

        // 序列化回 yaml，再解析，字段应保留
        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: AppConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg2.asr_engine, "whisper-small");
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");

        // polish_mode 序列化为整数（u8），非枚举名
        assert!(
            reserialized.contains("polish_mode: 2"),
            "polish_mode 应序列化为整数 2，实际: {}",
            reserialized
        );
    }

    #[test]
    fn denoise_enabled_defaults_to_true() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(cfg.denoise_enabled, "denoise_enabled 应默认 true");
    }

    #[test]
    fn denoise_enabled_override_from_yaml() {
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: false\n").unwrap();
        assert!(!cfg.denoise_enabled);
    }
}
