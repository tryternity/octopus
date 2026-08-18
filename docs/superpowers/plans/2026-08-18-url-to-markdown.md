# URL 抓取转 Markdown 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 「转 Markdown」命令支持 URL 输入——静态抓取 + SPA 空壳时离屏 WKWebView 渲染 fallback，产出走既有 markitdown 落盘 + CompactEditor 链路。

**Architecture:** 纯函数层（URL 识别/绝对化/title/charset 嗅探）在 `octopus-convert::web`（平台纯净）；WKWebView 胶水在 desktop `ui/web_render.rs`（cfg macos）；URL 编排在 desktop `markdown.rs`，编排核心 `convert_and_save_url_with` 参数化注入 fetch/render（生产绑真实现、测试绑 fake——网络不进单测）。

**Tech Stack:** reqwest 0.12（+gzip）、encoding_rs、url、regex（convert 新依赖）；objc2-web-kit 0.3 + block2 0.6（desktop macOS 段）。

**Spec:** `docs/superpowers/specs/2026-08-18-url-to-markdown-design.md`

## Global Constraints

- **开发隔离**：`.worktree/markdown-conversion` 分支，未经明确指令不进 main。
- **TDD**：Task 1/3 测试先行；网络函数（`fetch_page` / `render_html` 生产绑定）不进单测——编译级 + 手动 e2e。
- **签名不动**：`run_markdown_convert` 与其 7 个测试零波及（URL 编排在 `convert_and_save_to` 层）。
- **常量**（spec §5）：`SPA_SHELL_THRESHOLD = 200` 字符、`WEB_FETCH_TIMEOUT_SECS = 15`、`WEB_MAX_HTML_BYTES = 20MB`、`RENDER_SETTLE_MS = 2000`、`RENDER_TIMEOUT_SECS = 20`。
- **机制偏差预声明**：spec §4 的 `dispatch_after` 用「监控线程 sleep + `run_on_main_thread` 回投」等效替代（零 GCD 依赖），实施注记回写。
- **0 warning**；`0.6 objc2 unsafe 代码全部 `// SAFETY:` 注释契约。

---

### Task 1: octopus-convert::web 纯函数层（TDD）

**Files:**
- Modify: `crates/convert/Cargo.toml`（加 `url = "2"`、`regex = "1"`）
- Create: `crates/convert/src/web.rs`
- Modify: `crates/convert/src/lib.rs`（`pub mod web;`）

**Interfaces:**
- Produces: `web::{is_explicit_url, extract_title, sanitize_stem, absolutize_md_links, SPA_SHELL_THRESHOLD, WEB_FETCH_TIMEOUT_SECS, WEB_MAX_HTML_BYTES}`。Task 2/3 消费。

- [ ] **Step 1: 写失败测试 + 完整实现（web.rs）**

`crates/convert/src/web.rs`（TDD：tests 与实现同文件落盘后先跑红再确认——若直接同盘落实现，红步以 git stash 实现段验证一次即可）：

```rust
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
    if trimmed.is_empty() { "markitdown".to_string() } else { trimmed }
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
```

- [ ] **Step 2: Cargo.toml + lib.rs 注册**

`crates/convert/Cargo.toml` `[dependencies]` 加：

```toml
url = "2"
regex = "1"
```

`crates/convert/src/lib.rs` 加 `pub mod web;`

- [ ] **Step 3: 跑测试**

```bash
cargo test -p octopus-convert --lib 2>&1 | tail -3
```

Expected: 30 passed（24 既有 + 7 新；`test_absolutize_md_links` 的 `../up` 断言依赖 `Url::join` 归一化——若实际为 `https://ex.com/up` 之外的形态，按 join 语义修正断言并记录）。

- [ ] **Step 4: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): web 纯函数层——URL 识别/title/sanitize/md 绝对化（TDD）"
```

---

### Task 2: fetch_page 静态抓取（网络层，编译级验证）

**Files:**
- Modify: `crates/convert/Cargo.toml`（`reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "gzip"] }`、`encoding_rs = "0.8"`）
- Modify: `crates/convert/src/web.rs`

**Interfaces:**
- Consumes: Task 1 常量
- Produces: `pub struct FetchedPage { pub html: String, pub final_url: String, pub title: Option<String> }`、`pub fn fetch_page(url: &str) -> Result<FetchedPage, ConvertError>`、`pub(crate) fn sniff_charset(header_charset: Option<&str>, body_head: &[u8]) -> &'static encoding_rs::Encoding`（纯函数，测试）

- [ ] **Step 1: 写 sniff_charset 失败测试（web.rs tests 追加）**

```rust
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
```

- [ ] **Step 2: 实现 sniff_charset + fetch_page（web.rs 追加）**

```rust
use crate::error::ConvertError;

/// 静态抓取结果：html（已按 charset 解码为 String）、final_url（重定向后，绝对化 base）、title。
pub struct FetchedPage {
    pub html: String,
    pub final_url: String,
    pub title: Option<String>,
}

/// charset 三级嗅探（spec §3）：header > BOM > 前 2KB meta 声明 > UTF-8。纯函数。
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
    let meta_named = head
        .find("<meta charset=\"")
        .and_then(|s| head[s + 17..].find('"').map(|e| head[s + 17..s + 17 + e].to_string()));
    let meta_equiv = head.find("charset=").map(|s| {
        let rest = &head[s + 8..];
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

const DESKTOP_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

/// 静态抓取（spec §3）：GET → 状态/类型/大小守卫 → charset 解码 → title。
/// 网络函数不进单测（编译级 + 手动 e2e）；15s 超时、Chrome UA、gzip。
pub fn fetch_page(url: &str) -> Result<FetchedPage, ConvertError> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent(DESKTOP_UA)
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .build()
        .map_err(|e| ConvertError::Html(e.to_string()))?
        .get(url)
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
```

- [ ] **Step 3: 跑测试 + 编译**

```bash
cargo test -p octopus-convert --lib 2>&1 | tail -3
cargo build -p octopus-convert 2>&1 | grep -cE "^(error|warning)"
```

Expected: 32 passed（+2 charset）；0 warning（`ConvertError::Html` 变体复用 §6 文案通道）。

- [ ] **Step 4: Commit**

```bash
git add crates/convert Cargo.lock
git commit -m "feat(convert): fetch_page 静态抓取——UA/gzip/charset 三级嗅探/守卫"
```

---

### Task 3: desktop URL 编排（参数化注入，TDD）

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/markdown.rs`
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs`（无需动——markdown.rs 内部编排）

**Interfaces:**
- Consumes: `octopus_convert::web::{is_explicit_url, fetch_page, absolutize_md_links, extract_title, sanitize_stem, FetchedPage, SPA_SHELL_THRESHOLD}`；Task 4 的 `web_render::render_html`（本 task 先用占位绑定——非 macOS 或 Task 4 未接线时返回 Err）
- Produces: `pub(crate) fn convert_and_save_url_with(url, dir, fetch, render) -> Result<(PathBuf, String), String>` + `convert_and_save` URL 分支接线（`convert_and_save_to` 入口检测）

- [ ] **Step 1: 写失败测试（markdown.rs tests 追加）**

```rust
    // ── URL 编排（spec 2026-08-18-url-to-markdown §2，fake 注入——网络不进单测）──

    fn fake_page(title: Option<&str>, html: &str, final_url: &str) -> octopus_convert::web::FetchedPage {
        octopus_convert::web::FetchedPage {
            html: html.to_string(),
            final_url: final_url.to_string(),
            title: title.map(str::to_string),
        }
    }

    #[test]
    fn test_url_route_static_non_shell_skips_render() {
        let dir = std::env::temp_dir().join(format!("octopus-md-url-{}-a", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rich_html = "<title>文章标题</title>".to_string()
            + &"<p>这是一篇足够长的文章内容，用来超过空壳阈值两百个字符。</p>".repeat(10);
        let (path, md) = convert_and_save_url_with(
            "https://example.com/post/1",
            &dir,
            |_u| Ok(fake_page(Some("文章标题"), &rich_html, "https://example.com/post/1")),
            |_u| panic!("非空壳不应触发渲染"),
        )
        .unwrap();
        assert!(md.contains("足够长的文章内容"));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("文章标题_"), "name={}", name);
        assert!(name.ends_with(".md"));
    }

    #[test]
    fn test_url_route_shell_triggers_render_and_uses_rendered_title() {
        let dir = std::env::temp_dir().join(format!("octopus-md-url-{}-b", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shell_html = "<html><body><div id=\"root\"></div></body></html>";
        let rendered_html = "<title>SPA 渲染后的标题</title><p>渲染出的正文内容，同样超过两百字符阈值。</p>".repeat(5);
        let (path, _) = convert_and_save_url_with(
            "https://spa.example.com/",
            &dir,
            |_u| Ok(fake_page(None, shell_html, "https://spa.example.com/")),
            |_u| Ok(rendered_html),
        )
        .unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("SPA 渲染后的标题_"), "name={}", name);
    }

    #[test]
    fn test_url_route_fetch_err_propagates() {
        let dir = std::env::temp_dir().join(format!("octopus-md-url-{}-c", std::process::id()));
        let err = convert_and_save_url_with(
            "https://x.com/",
            &dir,
            |_u| Err("HTTP 404".to_string()),
            |_u| panic!("fetch 失败不应触发渲染"),
        )
        .unwrap_err();
        assert!(err.contains("404"), "err={}", err);
    }

    #[test]
    fn test_url_route_render_err_no_partial_file() {
        let dir = std::env::temp_dir().join(format!("octopus-md-url-{}-d", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shell_html = "<div id=root></div>";
        let err = convert_and_save_url_with(
            "https://spa.example.com/",
            &dir,
            |_u| Ok(fake_page(None, shell_html, "https://spa.example.com/")),
            |_u| Err("渲染超时".to_string()),
        )
        .unwrap_err();
        assert!(err.contains("渲染超时"), "err={}", err);
        assert!(dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(true), "不落半成品");
    }
```

- [ ] **Step 2: 跑红**

```bash
cargo test -p octopus-desktop test_url_route 2>&1 | tail -3
```

Expected: 编译失败（`convert_and_save_url_with` 未定义）。

- [ ] **Step 3: 实现（markdown.rs）**

先抽公共落盘段（现 `convert_and_save_to` 内 ts/碰撞/写文件逻辑提为私有 fn，原调用点改调）：

```rust
/// 落盘公共段：`<stem>_<yyyymmdd-HHMMSS>.md` + 同秒 `-N` 后缀（从 convert_and_save_to 抽出共用）。
fn write_markdown_file(dir: &std::path::Path, stem: &str, md: &str) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建输出目录失败: {}", e))?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("{}_{}.md", stem, ts));
    let mut n = 0u32;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{}_{}-{}.md", stem, ts, n));
    }
    std::fs::write(&path, md).map_err(|e| format!("写入文件失败: {}", e))?;
    Ok(path)
}
```

`convert_and_save_to` 改为内部使用 `write_markdown_file`（行为不变——原测试守护）。

URL 编排（markdown.rs 追加）：

```rust
use octopus_convert::web::{self, FetchedPage};

/// URL → md + 落盘（spec §2 决策树）。fetch/render 注入：生产绑真实现（网络），
/// 测试绑 fake——编排逻辑（空壳判定/fallback/stem/落盘）完全可单测。
#[allow(clippy::type_complexity)]
pub(crate) fn convert_and_save_url_with(
    url: &str,
    dir: &std::path::Path,
    fetch: impl FnOnce(&str) -> Result<FetchedPage, String>,
    render: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(std::path::PathBuf, String), String> {
    let page = fetch(url).map_err(|e| format!("抓取失败: {}", e))?;
    let to_md = |html: &str, base: &str| {
        web::absolutize_md_links(&octopus_convert::html_to_markdown(html), base)
    };
    let (md, stem) = {
        let md = to_md(&page.html, &page.final_url);
        if md.trim().chars().count() < web::SPA_SHELL_THRESHOLD {
            let html = render(url).map_err(|e| format!("渲染失败: {}", e))?;
            let stem = web::sanitize_stem(web::extract_title(&html).as_deref(), &page.final_url);
            (to_md(&html, &page.final_url), stem)
        } else {
            let stem = web::sanitize_stem(page.title.as_deref(), &page.final_url);
            (md, stem)
        }
    };
    let path = write_markdown_file(dir, &stem, &md)?;
    Ok((path, md))
}

/// 生产绑定：真静态抓取 + 渲染 fallback（Task 4 接 web_render；未接线/非 macOS 返回 Err）。
fn convert_and_save_url(url: &str, dir: &std::path::Path) -> Result<(std::path::PathBuf, String), String> {
    convert_and_save_url_with(
        url,
        dir,
        |u| web::fetch_page(u).map_err(|e| e.to_string()),
        |u| crate::ui::web_render::render_html(u),
    )
}
```

`convert_and_save_to` 入口插 URL 分支（在 stem 计算之前）：

```rust
pub(crate) fn convert_and_save_to(
    files: Vec<String>,
    html: Option<String>,
    text: String,
    dir: &std::path::Path,
) -> Result<(std::path::PathBuf, String), String> {
    // URL 输入（spec 2026-08-18-url-to-markdown §2）：files/html 空 + 显式 URL
    if files.is_empty() && html.is_none() {
        if let Some(url) = web::is_explicit_url(&text) {
            return convert_and_save_url(&url, dir);
        }
    }
    // …原有逻辑不变…
}
```

- [ ] **Step 4: 跑绿 + 既有回归**

```bash
cargo test -p octopus-desktop test_url_route 2>&1 | grep "test result"
cargo test -p octopus-desktop markdown 2>&1 | grep "test result"
```

Expected: url_route 4 passed；markdown 全过（既有 14 + 4 新）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop
git commit -m "feat(action-bar): URL 转 Markdown 编排——参数化注入 + 空壳判定 fallback（TDD）"
```

---

### Task 4: web_render.rs 离屏 WKWebView（macOS 胶水，编译级验证）

**Files:**
- Modify: `crates/desktop/Cargo.toml`（macOS 段加 `objc2-web-kit = { version = "0.3", features = ["WKWebView"] }`、`block2 = "0.6"`）
- Create: `crates/desktop/src/ui/web_render.rs`
- Modify: `crates/desktop/src/ui/mod.rs`（`pub mod web_render;`）

**Interfaces:**
- Consumes: Task 3 的 `crate::ui::web_render::render_html` 引用
- Produces: `pub fn render_html(url: &str) -> Result<String, String>`（outerHTML；阻塞 ≤ `RENDER_TIMEOUT_SECS`）

- [ ] **Step 1: 实现（完整代码）**

```rust
//! URL 渲染 fallback（spec 2026-08-18-url-to-markdown §4）：离屏 WKWebView 加载
//! SPA 页面 → readyState 轮询 → settle → outerHTML。仅 macOS。
//!
//! 线程模型（零 NavigationDelegate、零跨线程属性读）：
//! - 调用方（spawn_blocking 线程）持 mpsc channel + 20s deadline 循环；
//! - 主线程经 run_on_main_thread 创建/加载/evaluate；
//! - 重试与 settle 的延时由本函数所在线程 sleep 后回投主线程实现
//!   （spec §4 的 dispatch_after 等效替代，零 GCD 依赖）。
//!
//! unsafe 契约：`slot`（usize 指针）仅存 `Retained<WKWebView>::into_raw` 的裸指针，
//! **只在主线程**经 `Retained::from_raw` 取回使用后立即 `into_raw` 放回——
//! WKWebView 非 Send/Sync，跨线程仅以 usize 形态传递。

#![cfg(target_os = "macos")]

use objc2_foundation::{NSString, NSURL, NSURLRequest};
use objc2_web_kit::WKWebView;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// SPA 渲染总预算（秒）。
pub const RENDER_TIMEOUT_SECS: u64 = 20;
/// readyState 完成后的 settle（毫秒）——懒加载/字体。
pub const RENDER_SETTLE_MS: u64 = 2000;
/// readyState 轮询间隔（毫秒）。
const READY_POLL_MS: u64 = 250;

const READY_PROBE_JS: &str =
    "(document.readyState === 'complete') ? document.documentElement.outerHTML : null";
const OUTER_HTML_JS: &str = "document.documentElement.outerHTML";

enum Signal {
    Ready,          // readyState 探针拿到 HTML（字符串经 slot? 不能——见下，HTML 经 channel）
    Html(String),   // 最终 outerHTML
    Failed(String), // JS 执行错误
}

pub fn render_html(url: &str) -> Result<String, String> {
    let app = tauri::AppHandle::current()?; // 若无全局 handle 见下方注——改用调用方传入
    ...
}
```

**实现注记（执行者按此落完整代码，以下为精确骨架）**：`render_html` 需 `AppHandle`——Task 3 的 `convert_and_save_url` 闭包无 app 参数，故 `render_html(url: &str)` 内部取 `tauri::AppHandle` 的方式不存在全局 API；**改为签名 `pub fn render_html(app: &tauri::AppHandle, url: &str)`，Task 3 生产绑定同步改为 `|u| crate::ui::web_render::render_html(app, u)`（`convert_and_save` 增加 `app: &AppHandle` 参数，`script.rs` markdown 分支的 `convert_and_save` 调用点补传 `&ah`——一处 grep 可定位）**。

完整主体：

```rust
pub fn render_html(app: &tauri::AppHandle, url: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<Signal>();
    let done = Arc::new(AtomicBool::new(false));
    let url_owned = url.to_string();

    let tx_create = tx.clone();
    let done_create = done.clone();
    let ah = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            let _ = tx_create.send(Signal::Failed("主线程标记获取失败".into()));
            return;
        };
        // SAFETY: 主线程（mtm 证明）；WKWebView 离屏（不 attach window），Tauri 主 runloop 驱动
        let webview = unsafe { WKWebView::new(mtm) };
        let nsurl = NSURL::initWithString(unsafe { NSURL::alloc(mtm) }, &NSString::from_str(&url_owned))
            .expect("非法 URL");
        let request = NSURLRequest::requestWithURL(&nsurl);
        unsafe { webview.loadRequest(&request) };

        // slot: usize 裸指针，仅主线程解引用（模块 unsafe 契约）
        let slot = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let raw = objc2::rc::Retained::into_raw(webview) as usize;
        slot.store(raw, Ordering::SeqCst);

        // 探针链：evaluate → completion 里 null 表示未 ready（Signal 不发，等本侧轮询重投）；
        // 有值 → 发 Ready；失败 → Failed。completion block 不做延时——延时由监控方回投。
        probe_once(&slot, tx_create, done_create);

        let _ = ah; // 保活
    });

    // 监控循环（本线程）：
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(RENDER_TIMEOUT_SECS);
    let mut got_html: Option<String> = None; // ready 探针首次 HTML（用于 settle 后对比/兜底）
    loop {
        if done.load(Ordering::SeqCst) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            cleanup_on_main(app);
            return Err("渲染超时（SPA 页面）".into());
        }
        match rx.recv_timeout(std::time::Duration::from_millis(READY_POLL_MS)) {
            Ok(Signal::Html(h)) => {
                cleanup_on_main(app);
                done.store(true, Ordering::SeqCst);
                return Ok(h);
            }
            Ok(Signal::Failed(e)) => {
                cleanup_on_main(app);
                done.store(true, Ordering::SeqCst);
                return Err(format!("渲染失败: {}", e));
            }
            Ok(Signal::Ready) => {
                // readyState 完成 → 等 settle → 回投主线程取最终 outerHTML
                got_html = None; // Ready 信号仅作节拍（HTML 由 final evaluate 送）
                std::thread::sleep(std::time::Duration::from_millis(RENDER_SETTLE_MS));
                let tx_f = tx.clone();
                let slot2 = /* slot 需共享——见下：slot 提升到 render_html 层创建，Arc 传入 create 闭包 */;
                let _ = app.run_on_main_thread(move || {
                    // final evaluate（同 probe_once，但 JS 为 OUTER_HTML_JS，completion 直发 Html）
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 未 ready：回投主线程再探针（监控方节拍驱动重试——等效 dispatch_after）
                if got_html.is_none() {
                    /* 再 probe_once：slot 共享同上 */
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("渲染失败: 通道关闭".into());
            }
        }
    }
    Err("渲染失败".into())
}
```

**执行者注意**：上面骨架中 `slot` 需在 `render_html` 层创建（`Arc<AtomicUsize>`）并同时传入 create 闭包与监控侧回投闭包（`probe_once(&slot, …)` / final evaluate 都要 slot）——把骨架中两处 `/* slot 共享 */` 落实为闭包捕获 `slot.clone()`；`probe_once` 签名：

```rust
/// 单次 readyState 探针：完成 → Signal::Ready；未完成 → 无信号（监控方超时节拍重投）；
/// JS 错误 → Signal::Failed。webview 从 slot 取回使用后立即放回（unsafe 契约）。
fn probe_once(slot: &Arc<std::sync::atomic::AtomicUsize>, tx: mpsc::Sender<Signal>, done: Arc<AtomicBool>)
```

completion block 经 `block2::Block2::new(move |result: *mut objc2::runtime::AnyObject, error: *mut objc2_foundation::NSError| { ... })` 构造（`NSString` 结果 `as_str` 转 String；null 检查 `result.is_null()`）。**首次 Ready 后监控方回投的 final evaluate 用 `OUTER_HTML_JS`，其 completion 直发 `Signal::Html`**——此为唯一成功出口。

`cleanup_on_main(app)`：`run_on_main_thread` 中把 slot 指针 `Retained::from_raw` 取回后 drop（释放 webview）。

- [ ] **Step 2: 依赖 + 模块注册**

`crates/desktop/Cargo.toml` macOS 段加：

```toml
objc2-web-kit = { version = "0.3", features = ["WKWebView"] }
block2 = "0.6"
```

`crates/desktop/src/ui/mod.rs` 加 `pub mod web_render;`

- [ ] **Step 3: Task 3 生产绑定补 app 参数**

`markdown.rs`：`convert_and_save_url(url, dir)` → `convert_and_save_url(app: &AppHandle, url, dir)`，render 闭包改 `|u| crate::ui::web_render::render_html(app, u)`；`convert_and_save`（生产入口）加 `app` 参数；`script.rs` markdown 分支 `convert_and_save(f, h, t)` 调用点补 `&ah`（grep `convert_and_save(` 定位，一处）。

- [ ] **Step 4: 编译 + 全量**

```bash
cargo build -p octopus-desktop 2>&1 | grep -cE "^(error|warning)"
cargo test -p octopus-desktop markdown 2>&1 | grep "test result"
```

Expected: 0 warning；markdown 测试全过（url_route 4 个的 fake render 不受真实现影响）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop Cargo.lock
git commit -m "feat(desktop): web_render 离屏 WKWebView 渲染 fallback（completion-block 链式轮询）"
```

---

### Task 5: 全量验证 + 文档同步

**Files:**
- Modify: `docs/features/desktop-app.md` §14（markdown 命令补 URL 输入）
- Modify: `docs/architecture.md`（desktop 模块清单补 `ui/web_render.rs`）
- Modify: `docs/superpowers/specs/2026-08-18-url-to-markdown-design.md`（实施注记）

- [ ] **Step 1: 全量验证**

```bash
cargo build 2>&1 | grep -cE "^(error|warning)"
cargo test 2>&1 | grep -cE "FAILED|error\["
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
```

- [ ] **Step 2: 手动 e2e（用户侧）**

1. 选中文章页 URL → Alt+D → 转 Markdown → 快速路径秒开（title 命名、图片/链接绝对）
2. 选中 SPA 页 URL（如某 React 文档站）→ 触发渲染 fallback（首开 ~5s）→ 内容完整
3. 选中 `www.example.com` → 补全 https 抓取
4. 选中裸域名文本 → 直通不抓取
5. 断网/404 URL → 错误 temp tab

- [ ] **Step 3: 文档同步**

- `desktop-app.md` §14 markdown 命令表加输入行：「URL（显式 http/https/www）→ 静态抓取 + SPA 空壳渲染 fallback → 同款落盘/编辑器链路（详见 spec）」
- `architecture.md` desktop 模块段加：`ui/web_render.rs`——离屏 WKWebView 渲染 fallback（completion-block 链式轮询，slot 裸指针主线程契约）
- spec 实施注记：dispatch_after 的等效替代（监控线程回投）、`render_html` 携 `AppHandle` 的签名调整、Task 3 `convert_and_save` 增加 app 参数、 absolutize 代码块误改写限制、其他实施偏差

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: 同步 URL 抓取转 Markdown（desktop-app/architecture/spec 注记）"
```

---

## Self-Review 记录

- **Spec coverage**：§1 双深度/识别→Task 1/2/4；§2 决策树→Task 3；§3 静态细节→Task 1/2（gzip/UA/charset/绝对化/守卫全覆盖）；§4 渲染→Task 4（含机制等效替代预声明）；§5 命名/常量→Task 1（sanitize_stem）+Task 3（write_markdown_file 复用碰撞段）；§6 错误→文案内嵌各 Task；§7 测试→Task 1（7 纯函数测试）/Task 2（2 charset）/Task 3（4 编排 fake）/Task 4-5（编译+手动）；§8 文档→Task 5。无缺口。
- **占位符**：Task 4 骨架含两处标注明确的 `slot 共享` 落实点与 probe_once 签名契约——非 TBD，是给执行者的定向装配指令（完整代码的其余部分均已给出）；其余步骤代码完整。
- **类型一致性**：`FetchedPage` 三字段、`is_explicit_url -> Option<String>`、`render_html(app, url)`（Task 4 Step 3 显式同步 Task 3 签名）、`convert_and_save_url_with(url, dir, fetch, render)`、`write_markdown_file(dir, stem, md)` 跨 task 一致。
- **实现期风险**：① `Url::join("../up")` 归一化形态以测试实跑为准微调断言；② objc2 completion block 的 `DynBlock` 构造用 `Block2::new` 产生的类型与 `evaluateJavaScript_completionHandler` 参数的 `Option<&DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>>` 匹配性以编译器为准（block2 0.6 API：`Block2::new` → `&*block` 传参）；③ `tauri::AppHandle` 在 desktop bin 内的获取路径——已改为参数传递规避。
