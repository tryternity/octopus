// crates/llm/src/config.rs

use serde::{Deserialize, Serialize};

/// 兼容 OpenAI 接口的 LLM 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibleLlmConfig {
    /// 提供商标识（如 "openai", "deepseek"），仅用于日志
    pub provider: String,
    /// 模型名（如 "gpt-4o-mini", "deepseek-chat"）
    pub model: String,
    /// API base URL（如 "https://api.openai.com/v1"）
    pub base_url: String,
    /// API Key
    pub secret_key: String,
}
