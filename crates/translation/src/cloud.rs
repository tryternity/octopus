//! 云端 LLM 翻译引擎——OpenAI 兼容协议。
//! 覆盖 OpenAI/DeepSeek/Moonshot/智谱/百炼/MiniMax，差异仅在 DB models 行的
//! provider/source(base_url)/secret_key(api_key)/model_name。
//! 复用 octopus-llm::client 的 reqwest::blocking HTTP 客户端。

use crate::engine::TranslationEngine;
use anyhow::Result;
use async_trait::async_trait;

/// 云端 LLM 翻译引擎。
pub struct CloudLlmEngine {
    config: octopus_llm::CompatibleLlmConfig,
    name: String,
}

impl CloudLlmEngine {
    /// 从 DB models 行字段构造。
    /// is_thinking 模型翻译时会被 octopus-llm 自动关闭思考（needs_disable_thinking）。
    pub fn new(
        provider: &str,
        model: &str,
        base_url: &str,
        secret_key: &str,
        is_thinking: bool,
    ) -> Self {
        Self {
            config: octopus_llm::CompatibleLlmConfig {
                provider: provider.to_string(),
                model: model.to_string(),
                base_url: base_url.to_string(),
                secret_key: secret_key.to_string(),
                is_thinking,
                source_type: 2,
                is_enabled: true,
            },
            name: format!("{}:{}", provider, model),
        }
    }
}

#[async_trait]
impl TranslationEngine for CloudLlmEngine {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        let prompt = build_translate_prompt(source_lang, target_lang);
        // 复用 octopus-llm::client（reqwest::blocking）。
        // translation crate 是纯推理库（无 tokio 运行时依赖，tokio 仅 dev-dep），
        // 不能在此 crate 内 spawn_blocking。调用方（desktop translate.rs）负责隔离
        //（第十五轮 P2-A：CloudModel 分支已加 spawn_blocking，对齐 FallbackLlm）。
        octopus_llm::chat_text_with_prompt(&prompt, text, &self.config, None)
            .map_err(|e| anyhow::anyhow!("云端翻译失败: {}", e))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 构造翻译 prompt。语言代码（"zh"/"en"）映射成英文全称增强 LLM 理解。
/// 参考 CopyTranslator openai.ts 的 prompt 设计。
pub fn build_translate_prompt(source_lang: &str, target_lang: &str) -> String {
    let from = lang_to_english(source_lang);
    let to = lang_to_english(target_lang);
    format!(
        "Translate the following text from {} to {}. Only output the translation, without any explanation or extra text.",
        from, to
    )
}

fn lang_to_english(lang: &str) -> &'static str {
    match lang {
        "zh" => "Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        _ => "the original language",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_translate_prompt_zh_to_en() {
        let p = build_translate_prompt("zh", "en");
        assert!(p.contains("from Chinese to English"));
        assert!(p.contains("Only output the translation"));
    }

    #[test]
    fn test_build_translate_prompt_en_to_zh() {
        let p = build_translate_prompt("en", "zh");
        assert!(p.contains("from English to Chinese"));
    }

    #[test]
    fn test_cloud_engine_name() {
        let e = CloudLlmEngine::new("deepseek", "deepseek-chat", "https://api.deepseek.com", "sk-test", false);
        assert_eq!(e.name(), "deepseek:deepseek-chat");
    }
}
