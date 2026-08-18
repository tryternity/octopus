//! octopus-convert——文档转 Markdown 领域库（spec 2026-08-18-actionbar-markdown-conversion-design）。
//! 零项目内依赖（对齐 infra 惯例）。格式分派 / 单文件转换 / 多文件与文件夹合并。

pub mod convert;
pub mod dispatch;
pub mod error;
pub mod folder;
pub mod web;

pub use convert::FileSection;
pub use error::ConvertError;
pub use folder::{convert_files, convert_folder, MAX_FILES, MAX_TOTAL_BYTES};

static HTML_CONVERTER: std::sync::OnceLock<htmd::HtmlToMarkdown> = std::sync::OnceLock::new();

/// HTML → Markdown（剪贴板 HTML flavor / .html 文件共用，spec §4.1）。
/// skip script/style（浏览器复制的脚本噪声）；失败回退原文（比空串更有用）。
pub fn html_to_markdown(html: &str) -> String {
    let converter = HTML_CONVERTER.get_or_init(|| {
        htmd::HtmlToMarkdown::builder()
            .skip_tags(vec!["script", "style"])
            .build()
    });
    converter.convert(html).unwrap_or_else(|_| html.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_markdown_basic() {
        let md = html_to_markdown("<h1>标题</h1><p>段落<b>加粗</b></p>");
        assert!(md.contains("# 标题"), "md={}", md);
        assert!(md.contains("**加粗**"), "md={}", md);
    }

    #[test]
    fn test_html_to_markdown_skips_script() {
        let md = html_to_markdown("<p>ok</p><script>var x = 1;</script>");
        assert!(md.contains("ok"));
        assert!(!md.contains("var x"), "script 应被剔除，md={}", md);
    }
}
