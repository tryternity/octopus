//! 统一应用配置 schema（config.yaml 的统一定义）。
//!
//! 本模块是 config.yaml 的唯一 schema 来源，所有 crate（asr/desktop/cli 等）共享。
//! 多余字段对不使用它们的 crate 无害——各 crate 只读自己关心的字段。

use anyhow::Result;
use serde::{Deserialize, Serialize};

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

    /// 全局 ASR 激活/关闭快捷键
    #[serde(default = "default_asr_shortcut")]
    pub asr_shortcut: String,

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

    /// VAD 伪流式：静音触发识别的时长阈值（毫秒）
    /// 检测到语音后静音超过此时长即发送识别，默认 500 毫秒
    #[serde(default = "default_segment_silence")]
    pub segment_silence: f64,

    /// overlay 位置: top | bottom | none
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,

    /// 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
    #[serde(default)]
    pub polish_mode: PolishMode,

    /// 中间润色间隔（秒），0 = 仅最终润色
    #[serde(default = "default_polish_min_interval")]
    pub polish_min_interval: f64,

    /// 停顿驱动中间润色的静音阈值（毫秒）：静音达此值即触发全量润色（mode=2 only）
    #[serde(default = "default_pause_polish_threshold_ms")]
    pub pause_polish_threshold_ms: f64,

    /// 当前润色使用的 LLM 模型，格式为 "PREFIX:NAME" 或 3-part（见 `parse_model_spec`）：
    /// - "local:NAME" → is_local=true AND name（本地 LLM，如 Ollama）
    /// - "PROVIDER:CATEGORY:MODEL_NAME" → 精确匹配（如 "bigmodel:glm:glm-4-flashx"）
    /// - "NAME"（无冒号）→ 仅按 name（向后兼容）
    #[serde(default = "default_polish_llm")]
    pub polish_llm: String,

    /// 是否使用 ASR 硬件加速
    #[serde(default = "default_asr_hardware_accelerated")]
    pub asr_hardware_accelerated: bool,

    /// 是否对 ASR 输出进行纠错与热词校正
    #[serde(default = "default_asr_correct")]
    pub asr_correct: bool,

    /// ASR 输出字形：true→简体（繁→简），false→繁体（简→繁）。默认简体。
    #[serde(default = "default_output_simplified")]
    pub output_simplified: bool,

    /// 结果展示区工具栏是否自动隐藏。true→鼠标移入显示、移出隐藏（默认）；
    /// false→工具栏始终显示（窗口高度保持展开态）。
    #[serde(default = "default_hide_toolbar")]
    pub hide_toolbar: bool,

    /// 降噪模式：0=无降噪，1=轻度降噪，2=深度降噪。默认 1。
    #[serde(default = "default_denoise_mode")]
    pub denoise_mode: u8,

    /// 结果展示区编辑 toggle 快捷键——进入与保存（退出）编辑都用此键，与 ✏️ 按钮同语义
    /// （窗口内、仅结果窗聚焦时生效）。Tauri Accelerator 格式（如 "Cmd+Enter"），默认 "Cmd+Enter"。
    #[serde(default = "default_edit_shortcut")]
    pub edit_shortcut: String,

    /// HF 模型下载镜像 host（如 `https://hf-mirror.com`）。空 = 官方源 huggingface.co。
    /// cli `download --mirror` 临时覆盖此值；优先级 `--mirror` > 此字段 > 官方源。
    #[serde(default = "default_download_mirror")]
    pub download_mirror: String,
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
fn default_asr_shortcut() -> String {
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
fn default_polish_min_interval() -> f64 {
    5.0
}
fn default_pause_polish_threshold_ms() -> f64 {
    600.0
}
fn default_polish_llm() -> String {
    // 3-part（provider:category:model_name），与 db.sql 的 bigmodel glm seed 对齐，
    // 避免每次启动 parse_model_spec 把默认值当 2-part 旧格式走 warn + NameOnly 兜底。
    "bigmodel:glm:glm-4-flashx".into()
}
fn default_asr_hardware_accelerated() -> bool {
    false
}
fn default_asr_correct() -> bool {
    false
}
fn default_output_simplified() -> bool {
    true
}
fn default_hide_toolbar() -> bool {
    true
}
fn default_denoise_mode() -> u8 {
    1
}
fn default_edit_shortcut() -> String {
    "Cmd+Enter".into()
}
fn default_download_mirror() -> String {
    String::new()
}
fn default_segment_silence() -> f64 {
    400.0
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
            asr_shortcut: default_asr_shortcut(),
            paste_method: default_paste_method(),
            write_to_clipboard: default_write_to_clipboard(),
            microphone: String::new(),
            segment_silence: default_segment_silence(),
            overlay_position: default_overlay_position(),
            polish_mode: PolishMode::default(),
            polish_min_interval: default_polish_min_interval(),
            pause_polish_threshold_ms: default_pause_polish_threshold_ms(),
            polish_llm: default_polish_llm(),
            asr_hardware_accelerated: default_asr_hardware_accelerated(),
            asr_correct: default_asr_correct(),
            output_simplified: default_output_simplified(),
            hide_toolbar: default_hide_toolbar(),
            denoise_mode: default_denoise_mode(),
            edit_shortcut: default_edit_shortcut(),
            download_mirror: default_download_mirror(),
        }
    }
}

/// 从 DB app_config 表加载应用配置。
///
/// 内部委托 `db::load_app_config()`（首次调用时触发 ensure_db + init_schema，
/// 包含 yaml → DB 一次性迁移）。不缓存——调用方各自决定是否缓存
/// （asr 侧引擎配置另有 OnceLock 缓存）。
pub fn load_config() -> Result<AppConfig> {
    crate::db::load_app_config()
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
    fn app_config_default_values() {
        let cfg = AppConfig::default();
        assert!(cfg.write_to_clipboard, "write_to_clipboard 应默认 true");
        assert_eq!(cfg.pause_polish_threshold_ms, 600.0);
        assert!(!cfg.asr_hardware_accelerated);
        assert!(!cfg.asr_correct);
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "Cmd+Enter");
        assert_eq!(cfg.segment_silence, 400.0);
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
    fn denoise_mode_explicit_from_yaml() {
        // 显式 0/1/2 直接落到字段。
        let cfg: AppConfig = serde_yaml::from_str("denoise_mode: 2\n").unwrap();
        assert_eq!(cfg.denoise_mode, 2);
        let cfg: AppConfig = serde_yaml::from_str("denoise_mode: 0\n").unwrap();
        assert_eq!(cfg.denoise_mode, 0);
    }

    #[test]
    fn denoise_mode_legacy_denoise_enabled_ignored() {
        // 旧 denoise_enabled 已删除：serde 默认忽略未知字段，
        // 旧 config.yaml 里残留的 denoise_enabled 不影响 denoise_mode 解析。
        let cfg: AppConfig =
            serde_yaml::from_str("denoise_mode: 2\ndenoise_enabled: false\n").unwrap();
        assert_eq!(cfg.denoise_mode, 2);
    }

    #[test]
    fn edit_shortcut_explicit_from_yaml() {
        // 显式值原样落到字段（Tauri Accelerator 字符串）
        let cfg: AppConfig = serde_yaml::from_str("edit_shortcut: CmdOrCtrl+Shift+E\n").unwrap();
        assert_eq!(cfg.edit_shortcut, "CmdOrCtrl+Shift+E");
    }

    #[test]
    fn download_mirror_defaults_empty() {
        assert_eq!(AppConfig::default().download_mirror, "");
    }

    #[test]
    fn download_mirror_parsed_from_yaml() {
        let cfg: AppConfig =
            serde_yaml::from_str("download_mirror: https://hf-mirror.com\n").unwrap();
        assert_eq!(cfg.download_mirror, "https://hf-mirror.com");
    }

    #[test]
    fn download_mirror_absent_keeps_default() {
        // 缺该字段的旧 config → default 空（serde default）
        let cfg: AppConfig = serde_yaml::from_str("language: zh\n").unwrap();
        assert_eq!(cfg.download_mirror, "");
    }
}
