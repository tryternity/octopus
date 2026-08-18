//! URL → Markdown 纯函数层（spec 2026-08-18-url-to-markdown §3）。
//! fetch_page（网络）不进单测——编译级验证 + 手动 e2e；其余纯函数可单测。

use crate::error::ConvertError;

/// 静态抓取整体超时（秒）。
pub const WEB_FETCH_TIMEOUT_SECS: u64 = 15;
/// HTML 大小帽（字节）。
pub const WEB_MAX_HTML_BYTES: usize = 20 * 1024 * 1024;
/// SPA 空壳判定：转出 markdown trim 后字符数低于此 → 尝试渲染 fallback。
pub const SPA_SHELL_THRESHOLD: usize = 200;

/// charset 三级嗅探（spec §3）：header > BOM > 前 2KB meta 声明 > UTF-8。纯函数。
/// to_ascii_lowercase 不改字节位置——lower 后的索引可安全切片 head。
pub(crate) fn sniff_charset(
    header_charset: Option<&str>,
    body_head: &[u8],
) -> &'static encoding_rs::Encoding {
    if let Some(name) = header_charset {
        if let Some(enc) = encoding_rs::Encoding::for_label(name.as_bytes()) {
            return enc;
        }
    }
    // BOM（UTF-8/UTF-16 由 encoding_rs 的 decode 自动处理——这里显式识别 UTF-16 BOM 交 decode_bom）
    if body_head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return encoding_rs::UTF_8;
    }
    // meta 声明（前 2KB，大小写不敏感）
    let head = String::from_utf8_lossy(&body_head[..body_head.len().min(2048)]).to_ascii_lowercase();
    // 注意：`<meta charset="` 恰 15 字节——brief 原文写 s+17 会截掉前 2 字符（gbk→k），已修正。
    let meta_named = head
        .find("<meta charset=\"")
        .and_then(|s| head[s + 15..].find('"').map(|e| head[s + 15..s + 15 + e].to_string()));
    let meta_equiv = head.find("charset=").map(|s| {
        let rest = &head[s + 8..];
        // brief 补丁：值可能被引号包裹（charset='x' 单引号变体）——跳过开头引号、按同引号截断，
        // 否则 meta_equiv 直接命中开头引号取到空值。
        let rest = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let inner = &rest[1..];
                &inner[..inner.find(q).unwrap_or(inner.len())]
            }
            _ => rest,
        };
        let end = rest.find(|c: char| c == '"' || c == '\'' || c == ';').unwrap_or(rest.len());
        rest[..end].to_string()
    });
    for candidate in [meta_named, meta_equiv].into_iter().flatten() {
        if let Some(enc) = encoding_rs::Encoding::for_label(candidate.as_bytes()) {
            return enc;
        }
    }
    encoding_rs::UTF_8
}

/// 静态抓取结果：html（已按 charset 解码为 String）、final_url（重定向后，绝对化 base）、title。
pub struct FetchedPage {
    pub html: String,
    pub final_url: String,
    pub title: Option<String>,
}

const DESKTOP_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

/// 静态抓取（spec §3）：GET → 状态/类型/大小守卫 → charset 解码 → title。
/// 网络函数不进单测（编译级 + 手动 e2e）；15s 超时、Chrome UA、gzip。
/// blocking client——调用方（Tauri 命令层）在 spawn_blocking 上下文里执行。
pub fn fetch_page(url: &str) -> Result<FetchedPage, ConvertError> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent(DESKTOP_UA)
        .build()
        .map_err(|e| ConvertError::Html(e.to_string()))?
        .get(url)
        // brief 将 header 挂在 ClientBuilder 上——reqwest 0.12 blocking 无此方法，
        // 移到 RequestBuilder（语义等价：本请求的 Accept-Language）。
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .send()
        .map_err(|e| ConvertError::Html(format!("网络请求失败: {}", e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ConvertError::Html(format!("HTTP {}", status.as_u16())));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.contains("text/html") && !content_type.contains("xhtml") && !content_type.contains("xml") {
        return Err(ConvertError::Html("该 URL 不是 HTML 页面".into()));
    }
    let final_url = resp.url().to_string();
    let header_charset = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("charset=").map(str::to_string));
    let bytes = resp
        .bytes()
        .map_err(|e| ConvertError::Html(format!("读取响应失败: {}", e)))?;
    if bytes.len() > WEB_MAX_HTML_BYTES {
        return Err(ConvertError::Html("页面过大（上限 20MB）".into()));
    }
    let enc = sniff_charset(header_charset.as_deref(), &bytes);
    let (html, _, _) = enc.decode(&bytes);
    let html = html.into_owned();
    let title = extract_title(&html);
    Ok(FetchedPage { html, final_url, title })
}

/// 仅显式 URL（spec §1）：单行且 http:// | https:// | www. 开头（www. 补全 https://）。
/// 裸域名/IP 不识别——防普通文本误抓。
pub fn is_explicit_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(t.to_string())
    } else if lower.starts_with("www.") {
        Some(format!("https://{}", t))
    } else {
        None
    }
}

/// 宽容提取 <title>（大小写不敏感、容忍标签属性；最小实体解码）。
/// to_ascii_lowercase 不改字节位置——lower 的索引可安全切片 html。
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let tail = &lower[start..];
    let open_end = start + tail.find('>')?;
    let close = start + tail.find("</title>")?;
    if close <= open_end {
        return None;
    }
    let content = html[open_end + 1..close].trim();
    if content.is_empty() {
        return None;
    }
    Some(content
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'"))
}

/// 文件名 stem：title（sanitize）优先 → URL host → "markitdown"。
/// sanitize：白名单外字符→`_`、≤60 chars、去首尾空白与点。
pub fn sanitize_stem(title: Option<&str>, fallback_base_url: &str) -> String {
    let raw = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            url::Url::parse(fallback_base_url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "markitdown".to_string())
        });
    let mapped: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.()[]".contains(c) { c } else { '_' })
        .collect();
    let stem: String = mapped.chars().take(60).collect();
    let trimmed = stem.trim().trim_matches('.').to_string();
    // 全非字母数字（如 "///" → "___"）不可作文件名 → 兜底（brief 测试期望）
    if trimmed.is_empty() || !trimmed.chars().any(char::is_alphanumeric) {
        "markitdown".to_string()
    } else {
        trimmed
    }
}

/// md 后处理绝对化（spec §3）：`[text](rel)` / `![alt](rel)` 相对 URL 经
/// `url::Url::join(base)` 绝对化；跳过 `#` / `mailto:` / `data:` / 已含 scheme。
/// 已知限制（接受，spec 注记）：代码块内的示例相对路径同样被改写。
pub fn absolutize_md_links(md: &str, base: &str) -> String {
    let base_url = match url::Url::parse(base) {
        Ok(u) => u,
        Err(_) => return md.to_string(),
    };
    static LINK_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = LINK_RE.get_or_init(|| {
        // ! 前缀不在捕获组内原样保留；容忍 "title" 尾参
        regex::Regex::new(r#"\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#).unwrap()
    });
    re.replace_all(md, |caps: &regex::Captures| {
        let whole = caps.get(0).unwrap().as_str();
        let target = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let needs_resolve = !target.is_empty()
            && !target.starts_with('#')
            && !target.contains("://")
            && !target.starts_with("mailto:")
            && !target.starts_with("data:");
        if !needs_resolve {
            return whole.to_string();
        }
        match base_url.join(target) {
            Ok(abs) => format!("[{}]({})", caps.get(1).map(|m| m.as_str()).unwrap_or(""), abs),
            Err(_) => whole.to_string(),
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sniff_charset_priority() {
        let utf8 = encoding_rs::UTF_8;
        let gbk = encoding_rs::GBK;
        // header 声明优先
        assert_eq!(sniff_charset(Some("gbk"), b"<html>"), gbk);
        // 无 header → BOM
        assert_eq!(sniff_charset(None, [0xEFu8, 0xBB, 0xBF, b'<'].as_slice()), utf8);
        // 无 header 无 BOM → meta charset
        assert_eq!(sniff_charset(None, b"<meta charset=\"gbk\">"), gbk);
        // 全无 → UTF-8
        assert_eq!(sniff_charset(None, b"<html>"), utf8);
    }

    #[test]
    fn test_sniff_charset_meta_variants() {
        assert_eq!(sniff_charset(None, b"<META HTTP-EQUIV=\"Content-Type\" content=\"text/html; charset=Big5\">"), encoding_rs::BIG5);
        assert_eq!(sniff_charset(None, b"<meta charset='Shift_JIS'>"), encoding_rs::SHIFT_JIS);
    }

    #[test]
    fn test_is_explicit_url_positive() {
        assert_eq!(is_explicit_url("https://example.com/a?b=1"), Some("https://example.com/a?b=1".into()));
        assert_eq!(is_explicit_url("http://a.b"), Some("http://a.b".into()));
        assert_eq!(is_explicit_url("WWW.Example.com/x"), Some("https://WWW.Example.com/x".into()));
        assert_eq!(is_explicit_url("  https://a.b  "), Some("https://a.b".into()));
    }

    #[test]
    fn test_is_explicit_url_negative() {
        assert_eq!(is_explicit_url("example.com/page"), None, "裸域名不识别");
        assert_eq!(is_explicit_url("192.168.1.1"), None);
        assert_eq!(is_explicit_url("看这段文字 https://a.b"), None, "非单行");
        assert_eq!(is_explicit_url(""), None);
        assert_eq!(is_explicit_url("https://a.b 还有多余文字"), None);
    }

    #[test]
    fn test_extract_title() {
        assert_eq!(extract_title("<html><head><TITLE>我的 页面</TITLE></head></html>"), Some("我的 页面".into()));
        assert_eq!(extract_title("<title lang=\"zh\">A &amp; B</title>"), Some("A & B".into()));
        assert_eq!(extract_title("<title>  \n  多行\n标题  </title>"), Some("多行\n标题".into()));
        assert_eq!(extract_title("no title here"), None);
        assert_eq!(extract_title("<title></title>"), None);
    }

    #[test]
    fn test_sanitize_stem() {
        assert_eq!(sanitize_stem(Some("Hello World"), "https://x.com/"), "Hello World");
        assert_eq!(sanitize_stem(Some("a/b\\c:d*e"), "https://x.com/"), "a_b_c_d_e");
        assert_eq!(sanitize_stem(Some("  .trim me..  "), "https://x.com/"), "trim me");
        let long: String = std::iter::repeat('标').take(80).collect();
        assert_eq!(sanitize_stem(Some(&long), "https://x.com/").chars().count(), 60);
        assert_eq!(sanitize_stem(None, "https://blog.example.com/post/1"), "blog.example.com");
        assert_eq!(sanitize_stem(None, "::bad url::"), "markitdown");
        assert_eq!(sanitize_stem(Some("///"), "https://x.com/"), "markitdown");
    }

    #[test]
    fn test_absolutize_md_links() {
        let md = "[相对](/a/b) ![图](img/x.png) [绝对](https://o.com/c) [锚](#sec) [邮件](mailto:x@y.z) [数据](data:image/png;base64,xx) [回溯](../up)";
        let out = absolutize_md_links(md, "https://ex.com/dir/page.html");
        assert!(out.contains("](https://ex.com/a/b)"), "out={}", out);
        assert!(out.contains("](https://ex.com/dir/img/x.png)"), "out={}", out);
        assert!(out.contains("](https://ex.com/up)"), "join 回溯归一，out={}", out);
        assert!(out.contains("(https://o.com/c)"), "已是绝对不动");
        assert!(out.contains("(#sec)"), "锚点跳过");
        assert!(out.contains("(mailto:x@y.z)"), "mailto 跳过");
        assert!(out.contains("(data:image/png"), "data 跳过");
    }

    #[test]
    fn test_absolutize_md_links_bad_base_noop() {
        let md = "[x](/a)";
        assert_eq!(absolutize_md_links(md, "not a url"), md);
    }
}
