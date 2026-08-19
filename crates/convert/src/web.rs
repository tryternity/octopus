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

/// Safari macOS UA——与 WKWebView 渲染 fallback 同一浏览器族（spec §9⑮：原注释误标 Chrome）。
const DESKTOP_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

/// 静态抓取（spec §3）：GET → 状态/类型/大小守卫 → charset 解码 → title。
/// 网络函数不进单测（编译级 + 手动 e2e）；15s 超时、Safari macOS UA（与渲染
/// fallback 一致）、gzip。错误统一 `ConvertError::Web` 裸消息（§9⑭——前缀由编排层叠）。
/// blocking client——调用方（Tauri 命令层）在 spawn_blocking 上下文里执行。
pub fn fetch_page(url: &str) -> Result<FetchedPage, ConvertError> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent(DESKTOP_UA)
        .build()
        .map_err(|e| ConvertError::Web(e.to_string()))?
        .get(url)
        // brief 将 header 挂在 ClientBuilder 上——reqwest 0.12 blocking 无此方法，
        // 移到 RequestBuilder（语义等价：本请求的 Accept-Language）。
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .send()
        .map_err(|e| ConvertError::Web(format!("网络请求失败: {}", e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ConvertError::Web(format!("HTTP {}", status.as_u16())));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.contains("text/html") && !content_type.contains("xhtml") && !content_type.contains("xml") {
        return Err(ConvertError::Web("该 URL 不是 HTML 页面".into()));
    }
    let final_url = resp.url().to_string();
    let header_charset = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("charset=").map(str::to_string));
    let bytes = resp
        .bytes()
        .map_err(|e| ConvertError::Web(format!("读取响应失败: {}", e)))?;
    if bytes.len() > WEB_MAX_HTML_BYTES {
        return Err(ConvertError::Web("页面过大（上限 20MB）".into()));
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

// ── 图片 base64 内嵌（spec 2026-08-19-markdown-embed-images §3）──

/// 内嵌守卫（spec §3，变更需回写 spec）。
pub const EMBED_MAX_IMAGES: usize = 20;
pub const EMBED_MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const EMBED_MAX_TOTAL_BYTES: usize = 30 * 1024 * 1024;
pub const EMBED_TIMEOUT_SECS: u64 = 10;

/// `![alt](url "title")` 图片链接正则（title 可选）。extract 与 embed 共用同一
/// 实例（plan 自审风险②：两处 OnceLock 同 pattern 抽一处，DRY）。
/// URL 捕获 `(?:\\\)|[^)\s])+`：字面 `\)`（htmd 会把 URL 中的括号转义为
/// `\(`/`\)`——Wikimedia 文件名 `Foo_\(bar\).png` 常见），**或**非 `)`/非空白
/// 字符。`\)` 分支必须在前——leftmost-first 语义下单字符分支先吃掉 `\` 会让
/// 捕获在 `\)` 处仍截断（旧形态 `[^)\s]+` 即此 bug，终审实证发现）。捕获组内
/// 是 md 中的**转义形态**，消费前经 [`unescape_md_url`] 还原。
fn img_re() -> &'static regex::Regex {
    static IMG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    IMG_RE.get_or_init(|| {
        regex::Regex::new(r#"!\[([^\]]*)\]\(((?:\\\)|[^)\s])+)(?:\s+"[^"]*")?\)"#).unwrap()
    })
}

/// md URL 反转义（与 img_re 允许的 `\(`/`\)` 对称）：`\(`→`(`、`\)`→`)`。
/// extract 返回、下载器入参、replacements 查键统一用 unescaped 真实 URL；
/// 未内嵌链接的原 match 文本不动（保留转义形态，byte-identical）。
fn unescape_md_url(url: &str) -> String {
    url.replace("\\(", "(").replace("\\)", ")")
}

/// 提取 md 中可内嵌的远程图片链接：仅 `![alt](http/https://...)`；
/// 文本链接 / data: / file: / 相对路径跳过。URL 统一返回 **unescaped** 形态
/// （htmd 转义 `\(`/`\)` 已还原，spec §8 注⑩）。
pub fn extract_image_links(md: &str) -> Vec<(String, String)> {
    img_re()
        .captures_iter(md)
        .filter_map(|c| {
            let alt = c.get(1)?.as_str().to_string();
            let url = unescape_md_url(c.get(2)?.as_str());
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Some((alt, url))
            } else {
                None
            }
        })
        .collect()
}

/// 内嵌 pass（spec §2）：逐张经 download 下载 → 守卫 → data URI 替换。
/// 返回 (md', embedded, total)。失败/超帽保留原链接继续其余（spec §5）。
/// 同 URL 出现多次只下载一次（replacements 映射覆盖全部出现）；total 按 md
/// 中出现次数计（extract_image_links 计所有匹配），embedded 按成功下载的
/// 不同 URL 计（同一 URL 替换两处也只计 1）。
pub fn embed_images_with(
    md: &str,
    download: impl Fn(&str) -> Result<(String, Vec<u8>), String>,
) -> (String, usize, usize) {
    let targets = extract_image_links(md);
    let total = targets.len();
    let mut embedded = 0usize;
    let mut accumulated = 0usize;
    // 预解析成功的 URL → data URI 映射（守卫决定谁进映射）
    let mut replacements: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for url in targets.iter().map(|(_, u)| u) {
        if replacements.len() >= EMBED_MAX_IMAGES || accumulated >= EMBED_MAX_TOTAL_BYTES {
            break; // 数量/总量帽：停止后续（spec §5）
        }
        if replacements.contains_key(url) {
            continue; // 同 URL 只下载一次
        }
        let Ok((mime, bytes)) = download(url) else { continue };
        if bytes.len() > EMBED_MAX_IMAGE_BYTES {
            continue;
        }
        if accumulated + bytes.len() > EMBED_MAX_TOTAL_BYTES {
            continue;
        }
        accumulated += bytes.len();
        let b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        replacements.insert(url.clone(), format!("data:{};base64,{}", mime, b64));
        embedded += 1;
    }
    if embedded == 0 {
        return (md.to_string(), 0, total); // 全部失败=原样（spec §5）
    }
    let out = img_re()
        .replace_all(md, |caps: &regex::Captures| {
            // 捕获组是 md 中的转义形态（`\(`/`\)`）——按 unescaped 查 replacements
            // （下载键来自 extract_image_links，同为 unescaped）；未命中保留原
            // match 文本（转义形态 byte-identical，spec §8 注⑩）。
            let url = unescape_md_url(caps.get(2).map(|m| m.as_str()).unwrap_or(""));
            match replacements.get(&url) {
                Some(data) => {
                    format!("![{}]({})", caps.get(1).map(|m| m.as_str()).unwrap_or(""), data)
                }
                None => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .into_owned();
    (out, embedded, total)
}

/// MIME fallback：扩展名映射（spec §3）。未知扩展名 → None（保留原链接，不瞎猜）。
/// 先去 query string 再取最后一段扩展名——`x.png?w=100` 映射 png。
fn mime_from_ext(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// 生产下载绑定：GET（EMBED_TIMEOUT_SECS、DESKTOP_UA）→ (mime, bytes)。
/// mime：Content-Type（strip ;charset）优先，fallback 扩展名映射；都不明 → Err。
/// 网络函数不进单测（编译级验证 + 手动 e2e）——与 fetch_page 同策略。
fn download_image(url: &str) -> Result<(String, Vec<u8>), String> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(EMBED_TIMEOUT_SECS))
        .user_agent(DESKTOP_UA)
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| format!("下载失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let bytes = resp.bytes().map_err(|e| format!("读取失败: {}", e))?.to_vec();
    let mime = ct
        .filter(|m| m.starts_with("image/"))
        .or_else(|| mime_from_ext(url).map(str::to_string))
        .ok_or_else(|| "未知图片类型".to_string())?;
    Ok((mime, bytes))
}

/// 生产入口（spec §2）：embed_images_with + 真下载。编译级验证 + 手动 e2e。
/// blocking client——调用方（Tauri 命令层）在 spawn_blocking 上下文里执行。
pub fn embed_images(md: &str) -> (String, usize, usize) {
    embed_images_with(md, download_image)
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

    // ── 图片内嵌（spec 2026-08-19-markdown-embed-images）──

    #[test]
    fn test_extract_image_links() {
        let md = "![图一](https://a.com/x.png) [链接](https://a.com/page) \
![图二](http://b.com/y.jpg \"title\") ![data](data:image/png;base64,xx) \
![file](file:///tmp/z.png) ![rel](img/w.png) ![无alt](https://c.com/w.svg)";
        let links = extract_image_links(md);
        let urls: Vec<&str> = links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, vec!["https://a.com/x.png", "http://b.com/y.jpg", "https://c.com/w.svg"]);
        assert_eq!(links[0].0, "图一");
        assert_eq!(links[2].0, "无alt");
    }

    #[test]
    fn test_embed_images_with_success() {
        use base64::Engine;
        let md = "前文 ![alt](https://a.com/x.png) 后文";
        let (out, n, total) = embed_images_with(md, |_u| Ok(("image/png".into(), vec![1u8, 2, 3])));
        assert_eq!((n, total), (1, 1));
        let b64 = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        assert!(out.contains(&format!("![alt](data:image/png;base64,{})", b64)), "out={}", out);
        assert!(!out.contains("https://a.com/x.png"));
    }

    #[test]
    fn test_embed_images_with_failure_keeps_link() {
        let md = "![a](https://a.com/x.png) ![b](https://b.com/y.png)";
        let (out, n, total) = embed_images_with(md, |u| {
            if u.contains("a.com") { Err("timeout".into()) } else { Ok(("image/png".into(), vec![9u8])) }
        });
        assert_eq!((n, total), (1, 2));
        assert!(out.contains("![a](https://a.com/x.png)"), "失败保留原链接");
        assert!(out.contains("data:image/png;base64,"));
    }

    #[test]
    fn test_embed_images_with_per_image_cap() {
        let md = "![big](https://a.com/x.png)";
        let (out, n, _) = embed_images_with(md, |_u| Ok(("image/png".into(), vec![0u8; EMBED_MAX_IMAGE_BYTES + 1])));
        assert_eq!(n, 0);
        assert!(out.contains("https://a.com/x.png"), "超单图帽保留链接");
    }

    #[test]
    fn test_embed_images_with_total_cap_stops_later() {
        // brief 原测试 size=TOTAL/2+1=15MB 超 5MB 单图帽，永远到不了总帽判定——修正：
        // 7 张各恰 5MB（单图帽 `>` 判定，等号放行）：前 6 张累计 30MB=总帽，
        // 第 7 张触发总帽停止。验证语义不变：先到者内嵌、后续保留原链接。
        let md: String = (0..7).map(|i| format!("![i{}]({}/{}.png)\n", i, "https://a.com", i)).collect();
        let (out, n, total) = embed_images_with(&md, |_u| Ok(("image/png".into(), vec![0u8; EMBED_MAX_IMAGE_BYTES])));
        assert_eq!(total, 7);
        assert_eq!(n, 6, "第 7 张超总帽停止");
        assert!(out.contains("![i6](https://a.com/6.png)"), "超总帽保留原链接");
        assert!(out.contains("data:image/png;base64,"));
    }

    #[test]
    fn test_embed_images_with_count_cap() {
        let md: String = (0..25).map(|i| format!("![i{}]({}/{}.png)\n", i, "https://a.com", i)).collect();
        let (out, n, total) = embed_images_with(&md, |_u| Ok(("image/png".into(), vec![1u8])));
        assert_eq!(total, 25);
        assert_eq!(n, EMBED_MAX_IMAGES);
        assert!(out.contains("![i20](https://a.com/20.png)"), "第 21 张起保留链接");
        assert!(out.contains("data:image/png;base64,"));
    }

    #[test]
    fn test_embed_images_with_no_images_noop() {
        let (out, n, total) = embed_images_with("纯文本 [链接](https://a.com)", |_u| panic!("无图不应下载"));
        assert_eq!((n, total), (0, 0));
        assert_eq!(out, "纯文本 [链接](https://a.com)");
    }

    #[test]
    fn test_embed_images_with_escaped_parens_url() {
        // 终审实证发现（spec §8 注⑩）：htmd 把 URL 中的括号转义为 `\(`/`\)`——
        // 旧正则 `[^)\s]+` 在 `\)` 处截断捕获（`...Foo_\(bar\`）→ 下载必失败，
        // Wikimedia 旗舰场景（文件名含括号）永不内嵌。
        let md = r"![wiki](https://upload.wikimedia.org/w/commons/Foo_\(bar\).png)";
        // (a) extract 返回 UNESCAPED url
        let links = extract_image_links(md);
        assert_eq!(
            links,
            vec![("wiki".to_string(), "https://upload.wikimedia.org/w/commons/Foo_(bar).png".to_string())]
        );
        // (b) 成功下载 → data URI 整体替换，无 `\(` 残留、无截断尾巴
        let (out, n, total) = embed_images_with(md, |u| {
            assert_eq!(u, "https://upload.wikimedia.org/w/commons/Foo_(bar).png", "下载器收到 unescaped URL");
            Ok(("image/png".into(), vec![1u8, 2]))
        });
        assert_eq!((n, total), (1, 1));
        assert!(out.starts_with("![wiki](data:image/png;base64,"), "out={}", out);
        assert!(!out.contains('\\'), "无残留转义反斜杠");
        assert!(!out.contains(".png"), "无原 URL 残留");
        // (c) 失败下载 → 原 md byte-identical（转义形态原样保留）
        let (out2, n2, total2) = embed_images_with(md, |_u| Err("timeout".into()));
        assert_eq!((n2, total2), (0, 1));
        assert_eq!(out2, md, "失败保留原 escaped md");
    }
}
