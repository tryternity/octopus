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

/// 第二十二轮 P2-l1 / 第二十六轮 P2-l3 复查：读取错误响应 body 并截断——
/// - **P2-l1（信息泄漏，已修）**：某些 provider 4xx body 会 echo 请求头（含
///   `Authorization: Bearer ...`）或 stack trace，整 body 进 bail message → toast/日志
///   泄漏。截断到 500 字符（足够诊断 HTTP 错误，不会完整暴露 echo 的敏感头）。
/// - **P2-l3（OOM，留后续 P3）**：`response.text()`（:25）仍全量读内存——截断只省
///   message 不防 OOM。原注释自称"避免全量入内存"与实现矛盾（第二十六轮纠正）。实际
///   威胁低：LLM provider 是用户自配 API（OpenAI/DeepSeek/Ollama），非不可信外部服务，
///   GB 级 body 需 provider 主动作恶。完整修复需 reqwest streaming + 上限分块读，复杂度
///   超出收益，留 P3。
const ERROR_BODY_MAX_CHARS: usize = 500;
fn read_error_body(response: reqwest::blocking::Response) -> String {
    let text = response.text().unwrap_or_default();
    truncate_error_body(&text)
}

/// 截断逻辑分离（便于单元测试，无需 HTTP）。
fn truncate_error_body(text: &str) -> String {
    if text.chars().count() <= ERROR_BODY_MAX_CHARS {
        return text.to_string();
    }
    let truncated: String = text.chars().take(ERROR_BODY_MAX_CHARS).collect();
    format!("{}...(truncated)", truncated)
}

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

/// 一段文档区域。preserve=true → 用户已校对（{} 标记为语境参考）；false → 待润色。
/// candidates=Some → 热词多命中候选（<> 标记，LLM 从列表选一个）。
#[derive(Debug, Clone)]
pub struct PolishRegion {
    pub preserve: bool,
    pub text: String,
    /// 热词多命中候选列表（Hotwords 段）。Some 时 regions_prompt 用 `<a|b|c>` 标记。
    pub candidates: Option<Vec<String>>,
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

    let mut builder = build_chat_post(config).json(&request);
    // 调用方可覆盖 client 级超时（Run And Paste silent 用 30s，默认 120s）
    if let Some(dur) = timeout {
        builder = builder.timeout(dur);
    }
    let response = builder
        .send()
        .context("LLM API 请求失败")?;

    let status = response.status();
    if !status.is_success() {
        let body = read_error_body(response);
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

/// 用 LLM 从用户编辑段文本中提取热词候选（专有名词）。
///
/// `system_prompt` = 挖掘提示词（调用方从 resource 文件读，允许用户自定义覆盖）。
/// 复用润色 LLM 客户端（同 API key / endpoint）。
/// 失败返回 Err（调用方回退 jieba 分词挖掘）。
pub fn mine_hotwords(system_prompt: &str, edited_texts: &str, config: &CompatibleLlmConfig) -> Result<Vec<String>> {
    if edited_texts.trim().is_empty() {
        return Ok(Vec::new());
    }
    let user = format!("以下是语音识别后用户手动编辑纠正的文本片段：\n\n{}", edited_texts);

    let response = chat_text_with_prompt(system_prompt, &user, config, Some(30))?;
    // 解析 LLM 返回——每行一个词，只保留纯汉字行（2-6 字）
    let words: Vec<String> = response
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| {
            let chars: Vec<char> = l.chars().collect();
            chars.len() >= 2 && chars.len() <= 6
                && chars.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        })
        .collect();
    Ok(words)
}

/// 多段润色：按 regions 顺序，edited 区（preserve=true）verbatim 保留、其余润色，返回整篇。
///
/// max_tokens 按所有 regions 文本总字符数 × 2.0 算（中文 1-2 token/char，
/// ×1.2 曾致润色截断；max_tokens 是上限非目标值，英文多分配无副作用）。
/// 无 regions 或全部空 → 返回空串（不调 LLM）。
pub fn polish_regions(
    regions: &[PolishRegion],
    config: &CompatibleLlmConfig,
    prompt_content: &str,
    app_context: Option<&prompt::AppContext>,
) -> Result<String> {
    let total_chars: usize = regions.iter().map(|r| r.text.chars().count()).sum();
    if total_chars == 0 {
        return Ok(String::new());
    }
    let max_tokens = ((total_chars as f64) * 2.0).ceil() as u64;

    let result = chat_text(
        &prompt::build_system_prompt(prompt_content),
        &prompt::regions_prompt(regions, app_context),
        max_tokens,
        config,
        None,
    )?;
    // 防御性 strip：LLM 可能未遵守「去掉花括号标记」，残留 {} 会泄漏到最终文本。
    Ok(strip_edited_markers(&result))
}

/// 去除 LLM 输出中残留的 edited/hotwords 标记：
/// - `{word}`（edited 语境标记）→ `word`
/// - `<cand1|cand2>`（hotwords 多候选标记）→ 整体移除
///
/// 仅去包裹标记的括号，不影响用户文本里的字面 `{`/`}`/`<`/`>`（代码/数学/HTML）——
/// 那些场景下括号是无修饰的散字符，不构成 `{...}`/`<...>` 的配对标记。
/// 正则要求内部无嵌套 `{}`/`<>`（标记格式契约），字面文本里的孤立括号不受影响。
///
/// 第六轮 L1-b：两个 regex 预编译为 static Lazy（原每次 polish_regions 调用都
/// Regex::new×2，中间润色 mode=2 停顿驱动频繁触发，热路径不必要开销）。
static RE_EDITED_MARKER: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\{([^{}]*)\}").unwrap());
static RE_HOTWORDS_MARKER: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"<[^<>]*>").unwrap());

fn strip_edited_markers(text: &str) -> String {
    // {word} → word（edited 语境标记，内部不含嵌套 {}）
    let text = RE_EDITED_MARKER.replace_all(text, "$1").to_string();
    // <cand1|cand2|cand3> → 移除（hotwords 候选标记，LLM 应已选定一个，残留才清理）
    RE_HOTWORDS_MARKER.replace_all(&text, "").to_string()
}

/// 构造 LLM chat/completions 的 POST RequestBuilder（url + Content-Type + Authorization）。
/// 调用方再 .json(&request) + .send()。
/// 2026-08-05 抽取：消除 chat_text / test_connection 的 URL + headers 构造重复。
fn build_chat_post(config: &CompatibleLlmConfig) -> reqwest::blocking::RequestBuilder {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    HTTP_CLIENT
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.secret_key))
}

/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// 成功返回 Ok(())，失败返回错误信息。用于设置页连接检测。
pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()> {
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

    let response = build_chat_post(config)
        .json(&request)
        .send()
        .context("LLM API 连接失败（检查网络 / API base URL）")?;

    let status = response.status();
    if !status.is_success() {
        let body = read_error_body(response);
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

    /// 第六轮 L1-a 残余风险文档化：字面花括号（代码/JSON 语法）与 edited 标记同形，
    /// regex 无法区分。ASR 转写文本几乎不含代码语法，此为可接受的残余风险。
    /// 嵌套 `{config={key:value}}`（prompt.rs:121 把 edited 文本包成 `{...}`）：
    /// 内层先匹配 → `config=key:value`，外层不再重扫 → 外层括号泄漏（已知限制）。
    #[test]
    fn strip_edited_markers_literal_braces_residual_risk() {
        // 平铺字面花括号（代码语法）会被误处理——残余风险
        assert_eq!(strip_edited_markers("config={key:value}"), "config=key:value");
        // 嵌套（edited 区含字面括号）——外层泄漏，已知限制
        assert_eq!(strip_edited_markers("{config={key:value}}"), "{config=key:value}");
    }

    /// 第二十二轮 P2-l1/P2-l3：truncate_error_body 必须截断超长 body（防泄漏 + 防 OOM）。
    #[test]
    fn truncate_error_body_truncates_long_text() {
        // 短 body 原样返回
        assert_eq!(truncate_error_body("短错误"), "短错误");
        assert_eq!(truncate_error_body(""), "");
        // 恰好 500 字符——不截断
        let exactly_max: String = "x".repeat(ERROR_BODY_MAX_CHARS);
        assert_eq!(truncate_error_body(&exactly_max), exactly_max);

        // 501 字符——截断 + 标记
        let over: String = "y".repeat(600);
        let truncated = truncate_error_body(&over);
        assert!(truncated.ends_with("...(truncated)"), "超长 body 必须截断标记");
        // 截断后 = 500 chars + "...(truncated)"(14 chars) = 514
        assert_eq!(truncated.chars().count(), ERROR_BODY_MAX_CHARS + "...(truncated)".len());

        // 多字节 UTF-8（中文）按字符截断，不切断字符边界
        let chinese_over: String = "中".repeat(600);
        let cn_truncated = truncate_error_body(&chinese_over);
        assert!(cn_truncated.ends_with("...(truncated)"));
        // 500 个"中" + 标记，每个"中"是 1 char
        assert_eq!(cn_truncated.chars().filter(|&c| c == '中').count(), ERROR_BODY_MAX_CHARS);
    }
}
