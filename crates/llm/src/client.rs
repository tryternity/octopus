// crates/llm/src/client.rs

use anyhow::{Context, Result};
use crate::CompatibleLlmConfig;
use crate::prompt;
use serde::{Deserialize, Serialize};

/// DeepSeek 专有：关闭思考模式的参数体。
#[derive(Serialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f32,
    max_tokens: u64,
    /// DeepSeek 关闭思考：`{"type": "disabled"}`
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking>,
    /// BigModel 等关闭思考：`false`
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
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

/// 对 ASR 识别文本进行润色。
/// - preserved=Some：增量润色，保留 preserved 原样、仅润色 to_polish（编辑后用）。
/// - preserved=None：全量润色 to_polish。
/// 返回润色后的完整文本。
///
/// max_tokens 基于 preserved + to_polish 的总字符数（×1.2 冗余系数），
/// 因为增量模式下 LLM 输出 = preserved（原样）+ 润色后的 to_polish，
/// 仅按 to_polish 算会导致长编辑时输出被截断。
pub fn polish(preserved: Option<&str>, to_polish: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if to_polish.trim().is_empty() {
        return Ok(to_polish.to_string());
    }

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let total_chars = to_polish.chars().count()
        + preserved.map(|p| p.chars().count()).unwrap_or(0);
    let max_tokens = ((total_chars as f64) * 1.2).ceil() as u64;

    // 按 provider 分派思考模式关闭方式：
    // - DeepSeek：`thinking: {type: "disabled"}`（专有字段）
    // - BigModel 等：`enable_thinking: false`（OpenAI 扩展字段）
    let (thinking, enable_thinking) = if config.needs_disable_thinking() {
        if config.provider.eq_ignore_ascii_case("deepseek") {
            (Some(Thinking { kind: "disabled".to_string() }), None)
        } else {
            (None, Some(false))
        }
    } else {
        (None, None)
    };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: prompt::system_prompt(),
            },
            Message {
                role: "user".to_string(),
                content: prompt::user_prompt(preserved, to_polish),
            },
        ],
        temperature: 0.3,
        max_tokens,
        thinking,
        enable_thinking,
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
        anyhow::bail!(
            "LLM 返回空 content（模型可能仍处于思考模式，或 max_tokens 不足）；润色建议确认 thinking 已关闭或改用非思考模型"
        );
    }

    Ok(polished)
}

/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// 成功返回 Ok(())，失败返回错误信息。用于设置页连接检测。
pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let (thinking, enable_thinking) = if config.needs_disable_thinking() {
        if config.provider.eq_ignore_ascii_case("deepseek") {
            (Some(Thinking { kind: "disabled".to_string() }), None)
        } else {
            (None, Some(false))
        }
    } else {
        (None, None)
    };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![Message {
            role: "user".to_string(),
            content: "Hi".to_string(),
        }],
        temperature: 0.0,
        max_tokens: 1,
        thinking,
        enable_thinking,
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.secret_key))
        .json(&request)
        .send()
        .context("LLM API 连接失败（检查网络 / API base URL）")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        anyhow::bail!("LLM API 返回错误 {}: {}", status, body);
    }

    Ok(())
}
