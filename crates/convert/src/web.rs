//! URL → Markdown 纯函数层（spec 2026-08-18-url-to-markdown §3）。
//! fetch_page（网络）在 Task 2 加入；本层全部纯函数可单测。

/// 静态抓取整体超时（秒）。
pub const WEB_FETCH_TIMEOUT_SECS: u64 = 15;
/// HTML 大小帽（字节）。
pub const WEB_MAX_HTML_BYTES: usize = 20 * 1024 * 1024;
/// SPA 空壳判定：转出 markdown trim 后字符数低于此 → 尝试渲染 fallback。
pub const SPA_SHELL_THRESHOLD: usize = 200;

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
