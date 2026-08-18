//! 格式分派矩阵（spec §3.1）——扩展名 → 转换策略。纯函数，封闭清单。

/// 单文件转换策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    /// anydoc：14 类办公/文档格式
    Anydoc,
    /// htmd：HTML 文件
    Html,
    /// .md 原样嵌入
    Md,
    /// fenced code block + 语言标注
    Code,
    /// 一切不在清单内/无扩展——单文件报不支持，文件夹场景 skipped
    Binary,
}

/// anydoc 覆盖格式（封闭清单，spec §3.1）。
pub const ANYDOC_EXTS: &[&str] = &[
    "doc", "docx", "docm", "ppt", "pptx", "pptm", "pps", "ppsx", "ppsm", "pot",
    "xls", "xlsx", "xlsm", "xlsb", "odt", "ods", "odp", "rtf", "epub", "pdf", "csv",
];
pub const HTML_EXTS: &[&str] = &["html", "htm"];
pub const MD_EXTS: &[&str] = &["md", "markdown"];
/// Code 封闭清单（spec §3.1）——清单外一律 Binary。
pub const CODE_EXTS: &[&str] = &[
    "py", "rs", "ts", "tsx", "js", "jsx", "json", "yml", "yaml", "toml",
    "xml", "sh", "bash", "zsh", "txt", "log",
];

/// 扩展名 → FormatKind。输入带不带点、大小写均可。
pub fn format_kind(ext: &str) -> FormatKind {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    if ANYDOC_EXTS.contains(&e.as_str()) {
        FormatKind::Anydoc
    } else if HTML_EXTS.contains(&e.as_str()) {
        FormatKind::Html
    } else if MD_EXTS.contains(&e.as_str()) {
        FormatKind::Md
    } else if CODE_EXTS.contains(&e.as_str()) {
        FormatKind::Code
    } else {
        FormatKind::Binary
    }
}

/// Code 类扩展名 → fenced code block 语言标注。
pub fn code_language(ext: &str) -> &'static str {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "py" => "python",
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "yml" | "yaml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anydoc_full_matrix() {
        for ext in ANYDOC_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Anydoc, "ext={}", ext);
        }
    }

    #[test]
    fn test_html_md_code_matrix() {
        for ext in HTML_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Html);
        }
        for ext in MD_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Md);
        }
        for ext in CODE_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Code);
        }
    }

    #[test]
    fn test_binary_and_case_insensitive() {
        assert_eq!(format_kind("png"), FormatKind::Binary);
        assert_eq!(format_kind(""), FormatKind::Binary);
        assert_eq!(format_kind("exe"), FormatKind::Binary);
        // 大小写 + 前导点
        assert_eq!(format_kind(".DOCX"), FormatKind::Anydoc);
        assert_eq!(format_kind("Py"), FormatKind::Code);
    }

    #[test]
    fn test_code_language_mapping() {
        assert_eq!(code_language("py"), "python");
        assert_eq!(code_language("rs"), "rust");
        assert_eq!(code_language("tsx"), "typescript");
        assert_eq!(code_language("jsx"), "javascript");
        assert_eq!(code_language("yml"), "yaml");
        assert_eq!(code_language("zsh"), "bash");
        assert_eq!(code_language("txt"), "text");
        assert_eq!(code_language("unknown"), "text");
    }
}
