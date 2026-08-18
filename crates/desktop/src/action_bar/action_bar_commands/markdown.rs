//! 「转 Markdown」命令（action_type = "markdown"）的纯分派逻辑（spec §3/§5.2）。
//! execute_action_bar_inner 的 markdown 分支是薄包装，本模块可单测。

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
}
