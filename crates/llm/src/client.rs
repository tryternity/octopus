// crates/llm/src/client.rs

use anyhow::{Context, Result};
use crate::config::CompatibleLlmConfig;
use crate::prompt;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f32,
    max_tokens: u64,
}

#[derive(Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageContent,
}

#[derive(Deserialize)]
struct MessageContent {
    content: String,
}

/// 对 ASR 识别文本进行润色
/// - 修正识别错误
/// - 去除无意义语气词
/// - 不改变内容原意，不过度润色
/// 返回润色后的完整文本
pub fn polish(text: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let max_tokens = ((text.chars().count() as f64) * 1.2).ceil() as u64;

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: prompt::system_prompt().to_string(),
            },
            Message {
                role: "user".to_string(),
                content: prompt::user_prompt(text),
            },
        ],
        temperature: 0.3,
        max_tokens,
    };

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.secret_key))
        .json(&request)
        .send()
        .context("LLM API 请求失败")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        anyhow::bail!("LLM API 返回错误 {}: {}", status, body);
    }

    let chat_response: ChatResponse = response
        .json()
        .context("LLM API 响应解析失败")?;

    let polished = chat_response
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();

    if polished.is_empty() {
        anyhow::bail!("LLM 返回空结果");
    }

    Ok(polished)
}
