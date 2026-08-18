//! 单文件转换核心（spec §4.1）——唯一的转换单元，多文件/文件夹共用。

use crate::dispatch::{code_language, format_kind, FormatKind};
use crate::error::ConvertError;
use std::path::Path;

/// 单文件转换结果。content 用 Result 服务两种错误语义（spec §4.1）：
/// 单文件场景上抛；文件夹场景降级为 skipped 标注不中断（folder.rs merge）。
#[derive(Debug)]
pub struct FileSection {
    pub rel_path: String,
    pub content: Result<String, ConvertError>,
}

/// 唯一转换单元：abs=磁盘路径（定扩展名+读内容），rel=展示用相对路径。
pub(crate) fn convert_one(abs: &Path, rel: &str) -> FileSection {
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    FileSection { rel_path: rel.to_string(), content: convert_content(abs, &ext) }
}

fn convert_content(abs: &Path, ext: &str) -> Result<String, ConvertError> {
    match format_kind(ext) {
        FormatKind::Anydoc => anydoc::to_markdown(abs).map_err(|e| {
            // 扫描版（纯图片）PDF anydoc 无法本地转换——文案附走 OCR 提示（spec §6）
            if ext.eq_ignore_ascii_case("pdf") {
                ConvertError::Anydoc(format!("{}（扫描版 PDF 暂不支持，可截图走 OCR）", e))
            } else {
                ConvertError::Anydoc(e.to_string())
            }
        }),
        FormatKind::Html => {
            let html = std::fs::read_to_string(abs)?;
            Ok(crate::html_to_markdown(&html))
        }
        FormatKind::Md => Ok(std::fs::read_to_string(abs)?),
        FormatKind::Code => {
            let body = std::fs::read_to_string(abs)?;
            Ok(format!("```{}\n{}\n```", code_language(ext), body))
        }
        FormatKind::Binary => Err(ConvertError::UnsupportedFormat(ext.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：临时目录下写文件（名字即测试内唯一键）。
    fn tmp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-convert-test-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn asset(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join(name)
    }

    #[test]
    fn test_convert_one_md_passthrough() {
        let p = tmp_file("a.md", b"hello md");
        let s = convert_one(&p, "a.md");
        assert_eq!(s.content.unwrap(), "hello md");
    }

    #[test]
    fn test_convert_one_code_block_with_language() {
        let p = tmp_file("a.py", b"print(1)");
        let s = convert_one(&p, "a.py");
        assert_eq!(s.content.unwrap(), "```python\nprint(1)\n```");
    }

    #[test]
    fn test_convert_one_html_file() {
        let p = tmp_file("a.html", b"<h1>T</h1>");
        let s = convert_one(&p, "a.html");
        assert!(s.content.unwrap().contains("# T"));
    }

    #[test]
    fn test_convert_one_binary_unsupported() {
        let p = tmp_file("a.png", b"\x89PNG fake");
        let s = convert_one(&p, "a.png");
        let err = s.content.unwrap_err();
        assert_eq!(err.to_string(), "暂不支持 .png 格式");
    }

    #[test]
    fn test_convert_one_csv_via_anydoc() {
        // 复制 asset 到临时文件（与 tmp_file 同目录、正名扩展）
        let placeholder = tmp_file("sample.csv", b"placeholder");
        let p = placeholder.parent().unwrap().join("sample.csv");
        std::fs::copy(asset("sample.csv"), &p).unwrap();
        let s = convert_one(&p, "sample.csv");
        let md = s.content.unwrap();
        assert!(md.contains("Alice"), "csv 应转出表格内容，md={}", md);
    }

    #[test]
    fn test_convert_one_docx_via_anydoc() {
        let placeholder = tmp_file("sample.docx", b"placeholder");
        let p = placeholder.parent().unwrap().join("sample.docx");
        std::fs::copy(asset("sample.docx"), &p).unwrap();
        let s = convert_one(&p, "sample.docx");
        let md = s.content.unwrap();
        assert!(
            md.to_lowercase().contains("octopus"),
            "docx 应转出源文本，md={}",
            md
        );
    }

    #[test]
    fn test_file_section_fields() {
        let p = tmp_file("b.rs", b"fn main() {}");
        let s = convert_one(&p, "sub/dir/b.rs");
        assert_eq!(s.rel_path, "sub/dir/b.rs");
        assert!(s.content.unwrap().contains("```rust"));
    }
}
