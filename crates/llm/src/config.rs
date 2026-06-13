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

impl CompatibleLlmConfig {
    /// 是否需要显式关闭思考模式（DeepSeek 等默认开启思考的模型）。
    ///
    /// 这些模型若不关闭思考，润色这类明确任务的 `content` 可能为空
    /// （输出耗在 `reasoning_content` 上，实测 deepseek-v4-flash 润色时 content 直接为空）。
    /// 关闭后 `content` 直接返回润色结果。
    ///
    /// `thinking` 是 DeepSeek 独有参数，其他 OpenAI 兼容服务不支持；
    /// 故仅对需要的 provider 发送，避免向不兼容的 API 传入未知字段。
    /// 未来新增支持思考的 provider，在此扩展判断即可。
    pub fn needs_disable_thinking(&self) -> bool {
        self.provider.eq_ignore_ascii_case("deepseek")
    }
}
