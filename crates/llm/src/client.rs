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

/// 一段文档区域。preserve=true → 用户已校对（[] 标记为语境参考）；false → 待润色。
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
/// `max_tokens` 由调用方按「LLM 预期输出整篇字符数 × 2.0」算好传入：
/// - 单段 polish：输出 = preserved(原样) + 润色后 to_polish，按两者总长。
/// - 多段 polish_regions：输出 = 所有 regions 拼接（edited verbatim + 润色后非 edited），按 regions 总长。
fn chat_text(
    system: &str,
    user: &str,
    max_tokens: u64,
    config: &CompatibleLlmConfig,
    timeout: Option<std::time::Duration>,
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
    let mut builder = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.secret_key))
        .json(&request);
    // 调用方可覆盖 client 级超时（Run And Paste silent 用 30s，默认 120s）
    if let Some(dur) = timeout {
        builder = builder.timeout(dur);
    }
    let response = builder
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
/// max_tokens 基于 preserved + to_polish 的总字符数（×2.0 冗余系数）。
/// 中文每字符在多数 tokenizer 中约 1-2 token，×1.2 曾导致润色截断；×2.0 更安全，
/// max_tokens 是上限非目标值，英文场景多分配无副作用。
pub fn polish(preserved: Option<&str>, to_polish: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if to_polish.trim().is_empty() {
        return Ok(to_polish.to_string());
    }

    let total_chars = to_polish.chars().count()
        + preserved.map(|p| p.chars().count()).unwrap_or(0);
    let max_tokens = ((total_chars as f64) * 2.0).ceil() as u64;

    chat_text(
        &prompt::system_prompt(),
        &prompt::user_prompt(preserved, to_polish),
        max_tokens,
        config,
        None,
    )
}

/// 通用 LLM 文本补全（action bar 翻译/摘要/解释等非润色场景）。
/// 自定义 system + user prompt，不读全局 SYSTEM_PROMPT，不污染 ASR 润色。
/// max_tokens 按输入文本字符数 × 2.0 计算（与 polish 一致）。
/// timeout_secs: 可选超时（秒），None 用全局默认 120s。Run And Paste silent 传 30s。
pub fn chat_text_with_prompt(
    system: &str,
    user: &str,
    config: &CompatibleLlmConfig,
    timeout_secs: Option<u64>,
) -> Result<String> {
    let total_chars = user.chars().count();
    let max_tokens = ((total_chars as f64) * 2.0).ceil() as u64;
    let timeout = timeout_secs.map(std::time::Duration::from_secs);
    chat_text(system, user, max_tokens, config, timeout)
}

/// 多段润色：按 regions 顺序，edited 区（preserve=true）verbatim 保留、其余润色，返回整篇。
///
/// max_tokens 按所有 regions 文本总字符数 × 2.0 算（中文 1-2 token/char，
/// ×1.2 曾致润色截断；max_tokens 是上限非目标值，英文多分配无副作用）。
/// 无 regions 或全部空 → 返回空串（不调 LLM）。
pub fn polish_regions(
    regions: &[PolishRegion],
    config: &CompatibleLlmConfig,
) -> Result<String> {
    let total_chars: usize = regions.iter().map(|r| r.text.chars().count()).sum();
    if total_chars == 0 {
        return Ok(String::new());
    }
    let max_tokens = ((total_chars as f64) * 2.0).ceil() as u64;

    let result = chat_text(
        &prompt::system_prompt(),
        &prompt::regions_prompt(regions),
        max_tokens,
        config,
        None,
    )?;
    // 防御性 strip：LLM 可能未遵守「去掉花括号标记」，残留 {} 会泄漏到最终文本。
    Ok(strip_edited_markers(&result))
}

/// 去除 LLM 输出中残留的 edited 标记花括号（{word} → word）。
/// 仅去除包裹单个词的 `{}`，不影响 JSON/代码中的合法花括号（那些不会出现在润色输出中）。
fn strip_edited_markers(text: &str) -> String {
    // 简单策略：去掉所有 { 和 } 字符。
    // 润色输出是纯文本（无 JSON/代码），花括号在此语境的唯一来源就是 edited 标记。
    text.replace(['{', '}'], "")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_edited_markers_removes_braces() {
        assert_eq!(strip_edited_markers("hello {world}"), "hello world");
        assert_eq!(strip_edited_markers("{nginx}配置"), "nginx配置");
        assert_eq!(strip_edited_markers("no markers here"), "no markers here");
        assert_eq!(strip_edited_markers("{a}{b}"), "ab");
    }
}
