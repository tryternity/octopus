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
    /// 是否为思考（reasoning）模型。
    ///
    /// 思考模型默认将大量 token 花在 reasoning_content / thinking 上，
    /// 导致润色等明确任务的 content 为空。需显式关闭思考模式。
    /// 该字段来自 DB models.is_thinking，由用户按模型实际情况配置。
    pub is_thinking: bool,
}

impl CompatibleLlmConfig {
    /// 润色时是否需要显式关闭思考模式。
    ///
    /// 思考模型（`is_thinking=true`）在润色等明确任务中若不关闭思考，
    /// content 可能为空（token 全花在 reasoning 上）。
    /// 关闭方式依 provider 而定：DeepSeek 用 `thinking: {type: "disabled"}`，
    /// 其他 provider 的开关字段在 client.rs 中按 provider 分派。
    pub fn needs_disable_thinking(&self) -> bool {
        self.is_thinking
    }
}
