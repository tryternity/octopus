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

    /// 润色总开关
    #[serde(default)]
    pub polish_enabled: bool,

    /// 中间润色间隔（秒），0 = 仅最终润色
    #[serde(default = "default_polish_interval")]
    pub polish_interval: f64,

    /// 提供商标识（openai/deepseek/自定义）
    #[serde(default)]
    pub llm_provider: String,

    /// 模型名
    #[serde(default = "default_polish_model")]
    pub llm_model: String,

    /// API base URL
    #[serde(default = "default_polish_base_url")]
    pub llm_base_url: String,

    /// API Key
    #[serde(default)]
    pub llm_secret_key: String,
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
fn default_polish_interval() -> f64 {
    5.0
}
fn default_polish_model() -> String {
    "gpt-4o-mini".into()
}
fn default_polish_base_url() -> String {
    "https://api.openai.com/v1".into()
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
            segment_duration: default_segment_duration(),
            segment_silence: default_segment_silence(),
            segment_overlap: default_segment_overlap(),
            overlay_position: default_overlay_position(),
            polish_enabled: false,
            polish_interval: default_polish_interval(),
            llm_provider: String::new(),
            llm_model: default_polish_model(),
            llm_base_url: default_polish_base_url(),
            llm_secret_key: String::new(),
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

    /// 构建 LLM 配置，用于传给 octopus_llm::polish()
    /// 如果 polish_enabled 为 false 或 secret_key 为空，返回 None
    pub fn llm_config(&self) -> Option<octopus_llm::CompatibleLlmConfig> {
        if !self.polish_enabled || self.llm_secret_key.is_empty() {
            return None;
        }
        Some(octopus_llm::CompatibleLlmConfig {
            provider: self.llm_provider.clone(),
            model: self.llm_model.clone(),
            base_url: self.llm_base_url.clone(),
            secret_key: self.llm_secret_key.clone(),
        })
    }
}

/// 从 ~/.octopus/config.yaml 加载桌面配置
pub fn load_desktop_config() -> Result<DesktopConfig> {
    let config_home = octopus_infra::octopus_config_home();
    let config_path = config_home.join("config.yaml");

    if !config_path.exists() {
        return Ok(DesktopConfig::default());
    }

    let text = std::fs::read_to_string(&config_path)?;
    let config: DesktopConfig = serde_yaml::from_str(&text)?;
    Ok(config)
}
