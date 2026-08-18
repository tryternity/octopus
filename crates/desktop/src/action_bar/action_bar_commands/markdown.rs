//! 「转 Markdown」命令（action_type = "markdown"）的纯分派逻辑（spec §3/§5.2）。
//! execute_action_bar_inner 的 markdown 分支是薄包装，本模块可单测。

use octopus_convert::web::{self, FetchedPage};

/// 输出文件名（不含扩展名）：源文件/文件夹 stem + 时间戳（yyyymmdd-HHMMSS）——
/// 零碰撞可排序；text/html 源用 "markitdown"（spec §5.2 修订）。
/// 同秒碰撞由 convert_and_save_to 的 `-1/-2` 后缀兜底。
pub(crate) fn output_file_stem(files: &[String], html: Option<&str>, text: &str) -> String {
    let sanitize = |s: String| -> String {
        // 源名理论上是合法文件名片段，防御性替换路径分隔符即可
        s.replace('/', "_").replace('\\', "_")
    };
    if !files.is_empty() {
        let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
        if paths.len() == 1 {
            let p = &paths[0];
            let stem = if p.is_dir() {
                p.file_name().unwrap_or_default().to_string_lossy().to_string()
            } else {
                p.file_stem().unwrap_or_default().to_string_lossy().to_string()
            };
            return sanitize(stem);
        }
        // 多选：公共父目录名（与合并文档标题一致）；无父目录兜底 "markitdown"
        let parent_name = paths
            .first()
            .and_then(|p| p.parent())
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "markitdown".to_string());
        return sanitize(parent_name);
    }
    if html.filter(|h| !h.trim().is_empty()).is_some() || !text.trim().is_empty() {
        return "markitdown".to_string();
    }
    "markitdown".to_string()
}

/// 转换并保存到指定目录（dir 注入便于测试），返回 (文件路径, 内容)。
/// 文件名 `<stem>_<yyyymmdd-HHMMSS>.md`；同秒碰撞追加 `-1/-2...` 后缀。
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
    let stem = output_file_stem(&files, html.as_deref(), &text);
    let md = run_markdown_convert(files, html, text)?;
    let path = write_markdown_file(dir, &stem, &md)?;
    Ok((path, md))
}

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
        // Task 4 接线 web_render 后替换（spec §2 渲染 fallback）
        |_u| Err("渲染 fallback 尚未接线".to_string()),
    )
}

/// 生产入口：转换并保存到 markitdown_dir()（~/Documents/octopus/markitdown，可配置覆盖）。
pub(crate) fn convert_and_save(
    files: Vec<String>,
    html: Option<String>,
    text: String,
) -> Result<(std::path::PathBuf, String), String> {
    convert_and_save_to(files, html, text, &octopus_infra::paths::markitdown_dir())
}

/// 输入分派（优先级 spec §3）：files > html > text；全空 Err。
/// 混合文件夹+文件：文件夹逐个 convert_folder，与文件结果以 `---` 分隔拼接。
pub(crate) fn run_markdown_convert(
    files: Vec<String>,
    html: Option<String>,
    text: String,
) -> Result<String, String> {
    if !files.is_empty() {
        let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
        let mut parts: Vec<String> = Vec::new();
        for d in paths.iter().filter(|p| p.is_dir()) {
            parts.push(octopus_convert::convert_folder(d).map_err(|e| e.to_string())?);
        }
        let plain: Vec<std::path::PathBuf> =
            paths.iter().filter(|p| p.is_file()).cloned().collect();
        if !plain.is_empty() {
            parts.push(octopus_convert::convert_files(&plain).map_err(|e| e.to_string())?);
        }
        if parts.is_empty() {
            return Err("没有可转换的内容".to_string()); // 选中路径在执行前被删光
        }
        return Ok(parts.join("\n\n---\n\n"));
    }
    if let Some(h) = html.filter(|h| !h.trim().is_empty()) {
        return Ok(octopus_convert::html_to_markdown(&h));
    }
    if !text.trim().is_empty() {
        return Ok(text); // 纯文本直通（spec §6）
    }
    Err("没有可转换的内容".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str, bytes: &[u8]) -> String {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-cmd-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn test_text_passthrough() {
        assert_eq!(
            run_markdown_convert(vec![], None, "纯文本".into()).unwrap(),
            "纯文本"
        );
    }

    #[test]
    fn test_html_priority_over_text() {
        let md = run_markdown_convert(vec![], Some("<h1>H</h1>".into()), "plain".into()).unwrap();
        assert!(md.contains("# H"), "html 优先于纯文本，md={}", md);
    }

    #[test]
    fn test_empty_all_inputs_err() {
        let err = run_markdown_convert(vec![], None, "  ".into()).unwrap_err();
        assert_eq!(err, "没有可转换的内容");
    }

    #[test]
    fn test_single_file_no_tree() {
        let p = tmp_file("solo.md", b"solo content");
        let md = run_markdown_convert(vec![p], None, String::new()).unwrap();
        assert_eq!(md, "solo content");
    }

    #[test]
    fn test_binary_file_err_message() {
        let p = tmp_file("img.png", b"\x89PNG");
        let err = run_markdown_convert(vec![p], None, String::new()).unwrap_err();
        assert!(err.contains("暂不支持 .png"));
    }

    #[test]
    fn test_folder_merge_contains_tree() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-cmd-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), b"aaa").unwrap();
        std::fs::write(dir.join("b.py"), b"x=1").unwrap();
        let md = run_markdown_convert(vec![dir.to_string_lossy().to_string()], None, String::new())
            .unwrap();
        assert!(md.contains("## 文件树"));
        assert!(md.contains("## a.md"));
        assert!(md.contains("```python"));
    }

    #[test]
    fn test_files_priority_over_html_and_text() {
        let p = tmp_file("prio.md", b"from file");
        let md = run_markdown_convert(
            vec![p],
            Some("<h1>from html</h1>".into()),
            "from text".into(),
        )
        .unwrap();
        assert_eq!(md, "from file");
    }

    // ── output_file_stem（spec §5.2 修订）──

    #[test]
    fn test_output_file_stem_single_file() {
        let p = tmp_file("report.docx", b"x");
        assert_eq!(output_file_stem(&[p], None, ""), "report");
    }

    #[test]
    fn test_output_file_stem_folder() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-stem-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            output_file_stem(&[dir.to_string_lossy().to_string()], None, ""),
            dir.file_name().unwrap().to_string_lossy().to_string()
        );
    }

    #[test]
    fn test_output_file_stem_multi_files_uses_parent() {
        let a = tmp_file("multi_a.md", b"a");
        let b = tmp_file("multi_b.md", b"b");
        let parent = std::path::PathBuf::from(&a)
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(output_file_stem(&[a, b], None, ""), parent);
    }

    #[test]
    fn test_output_file_stem_text_html_fallback() {
        assert_eq!(output_file_stem(&[], None, "纯文本"), "markitdown");
        assert_eq!(output_file_stem(&[], Some("<h1>x</h1>"), ""), "markitdown");
        assert_eq!(output_file_stem(&[], None, ""), "markitdown");
    }

    // ── convert_and_save_to（spec §5.2 修订）──

    #[test]
    fn test_convert_and_save_to_writes_file() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-save-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = tmp_file("save_me.md", b"saved content");
        let (path, md) =
            convert_and_save_to(vec![p], None, String::new(), &dir).unwrap();
        assert_eq!(md, "saved content");
        assert!(path.extension().unwrap() == "md");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("save_me_"), "文件名应含 stem，实际 {}", name);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved content");
    }

    #[test]
    fn test_convert_and_save_to_collision_suffix() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-save-{}-coll", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = tmp_file("coll.md", b"c");
        let (p1, _) = convert_and_save_to(vec![p.clone()], None, String::new(), &dir).unwrap();
        let (p2, _) = convert_and_save_to(vec![p], None, String::new(), &dir).unwrap();
        assert_ne!(p1, p2, "同秒两次保存不应同路径");
        assert!(p1.exists() && p2.exists());
    }

    #[test]
    fn test_convert_and_save_to_err_propagates() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-save-{}-err", std::process::id()));
        let p = tmp_file("bad.bin", b"\x00");
        let err = convert_and_save_to(vec![p], None, String::new(), &dir).unwrap_err();
        assert!(err.contains("暂不支持 .bin"), "err={}", err);
    }

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
}
