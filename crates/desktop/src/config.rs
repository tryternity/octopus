use anyhow::Result;
use serde::Deserialize;

/// Desktop 应用完整配置
/// 读取自 ~/.octopus/config.yaml，字段缺失时使用默认值
#[derive(Debug, Deserialize, Clone)]
pub struct DesktopConfig {
    /// ASR 引擎模式: embedded | websocket | grpc
    #[serde(default = "default_engine_mode")]
    pub engine_mode: String,

    /// WebSocket 远程地址（engine_mode = websocket 时使用）
    #[serde(default = "default_remote_url")]
    pub remote_url: String,

    /// gRPC 端点（engine_mode = grpc 时使用）
    #[serde(default = "default_grpc_endpoint")]
    pub grpc_endpoint: String,

    /// ASR 引擎选择: sensevoice | whisper | paraformer-streaming | paraformer-large
    #[serde(default = "default_asr_engine")]
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

    /// 麦克风名称（空 = 系统默认）
    #[serde(default)]
    pub microphone: String,

    /// overlay 位置: top | bottom | none
    #[serde(default = "default_overlay_position")]
    pub overlay_position: String,
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
fn default_asr_engine() -> String {
    "sensevoice".into()
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
fn default_overlay_position() -> String {
    "top".into()
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            engine_mode: default_engine_mode(),
            remote_url: default_remote_url(),
            grpc_endpoint: default_grpc_endpoint(),
            asr_engine: default_asr_engine(),
            language: default_language(),
            shortcut: default_shortcut(),
            paste_method: default_paste_method(),
            microphone: String::new(),
            overlay_position: default_overlay_position(),
        }
    }
}

impl DesktopConfig {
    /// 检查当前配置的 ASR 引擎是否支持流式识别。
    /// 仅 Paraformer 和 Zipformer 支持流式。
    pub fn is_streaming_engine(&self) -> bool {
        match octopus_asr::config::resolve_engine_category(&self.asr_engine) {
            Some(
                octopus_asr::config::EngineCategory::Paraformer
                | octopus_asr::config::EngineCategory::Zipformer,
            ) => true,
            _ => false,
        }
    }
}

/// 从 ~/.octopus/config.yaml 加载桌面配置
pub fn load_desktop_config() -> Result<DesktopConfig> {
    let handy_home = octopus_asr::config::handy_home();
    let config_path = handy_home.join("config.yaml");

    if !config_path.exists() {
        return Ok(DesktopConfig::default());
    }

    let text = std::fs::read_to_string(&config_path)?;
    let config: DesktopConfig = serde_yaml::from_str(&text)?;
    Ok(config)
}
