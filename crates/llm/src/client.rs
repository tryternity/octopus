// crates/llm/src/client.rs

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use crate::CompatibleLlmConfig;
use crate::prompt;
use serde::{Deserialize, Serialize};

/// 共享 HTTP Client（带超时），避免每次调用新建（无连接池）+ 统一超时。
static HTTP_CLIENT: Lazy<reqwest::blocking::Client> = Lazy::new(|| {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(octopus_infra::net::HTTP_TIMEOUT_SECS))
        .build()
        .expect("failed to build LLM HTTP client")
});

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

/// 一段文档区域。preserve=true → 原样保留（edited）；false → 待润色。
/// 字段顺序/类型严格按 plan，Task 4 coordinator 按此构造。
#[derive(Debug, Clone)]
pub struct PolishRegion {
    pub preserve: bool,
    pub text: String,
}

/// 按 provider 分派思考模式关闭方式：
/// - DeepSeek：`thinking: {type: "disabled"}`（专有字段）
/// - BigModel 等：`enable_thinking: false`（OpenAI 扩展字段）
/// - 无需关闭思考：两字段均 None
fn thinking_flags(config: &CompatibleLlmConfig) -> (Option<Thinking>, Option<bool>) {
    if config.needs_disable_thinking() {
        if config.provider.eq_ignore_ascii_case("deepseek") {
            (Some(Thinking { kind: "disabled".to_string() }), None)
        } else {
            (None, Some(false))
        }
    } else {
        (None, None)
    }
}

/// 通用 chat completion：system+user → messages → HTTP → 取 content 文本。
/// `polish`（单段，user_prompt）与 `polish_regions`（多段，regions_prompt）共用此 helper，
/// 避免 LLM 调用逻辑（HTTP / provider 分派 / 错误处理 / 空 content bail）复制粘贴。
///
/// `max_tokens` 由调用方按「LLM 预期输出整篇字符数 × 1.2」算好传入：
/// - 单段 polish：输出 = preserved(原样) + 润色后 to_polish，按两者总长。
/// - 多段 polish_regions：输出 = 所有 regions 拼接（edited verbatim + 润色后非 edited），按 regions 总长。
fn chat_text(
    system: &str,
    user: &str,
    max_tokens: u64,
    config: &CompatibleLlmConfig,
) -> Result<String> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let (thinking, enable_thinking) = thinking_flags(config);

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            Message { role: "system".to_string(), content: system.to_string() },
            Message { role: "user".to_string(), content: user.to_string() },
        ],
        temperature: 0.3,
        max_tokens,
        thinking,
        enable_thinking,
    };

    let client = &*HTTP_CLIENT;
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

/// 对 ASR 识别文本进行润色。
/// - preserved=Some：增量润色，保留 preserved 原样、仅润色 to_polish（编辑后用）。
/// - preserved=None：全量润色 to_polish。
///   返回润色后的完整文本。
///
/// max_tokens 基于 preserved + to_polish 的总字符数（×1.2 冗余系数），
/// 因为增量模式下 LLM 输出 = preserved（原样）+ 润色后的 to_polish，
/// 仅按 to_polish 算会导致长编辑时输出被截断。
pub fn polish(preserved: Option<&str>, to_polish: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if to_polish.trim().is_empty() {
        return Ok(to_polish.to_string());
    }

    let total_chars = to_polish.chars().count()
        + preserved.map(|p| p.chars().count()).unwrap_or(0);
    let max_tokens = ((total_chars as f64) * 1.2).ceil() as u64;

    chat_text(
        &prompt::system_prompt(),
        &prompt::user_prompt(preserved, to_polish),
        max_tokens,
        config,
    )
}

/// 多段润色：按 regions 顺序，edited 区（preserve=true）verbatim 保留、其余润色，返回整篇。
///
/// max_tokens 按所有 regions 文本总字符数 × 1.2 算（输出整篇 = edited 原样 + 润色后非 edited 拼接）。
/// 无 regions 或全部空 → 返回空串（不调 LLM）。
pub fn polish_regions(
    regions: &[PolishRegion],
    config: &CompatibleLlmConfig,
) -> Result<String> {
    let total_chars: usize = regions.iter().map(|r| r.text.chars().count()).sum();
    if total_chars == 0 {
        return Ok(String::new());
    }
    let max_tokens = ((total_chars as f64) * 1.2).ceil() as u64;

    chat_text(
        &prompt::system_prompt(),
        &prompt::regions_prompt(regions),
        max_tokens,
        config,
    )
}

/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// 成功返回 Ok(())，失败返回错误信息。用于设置页连接检测。
pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let (thinking, enable_thinking) = thinking_flags(config);

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

    let client = &*HTTP_CLIENT;

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
