//! content_html → 纯文本抽取：scraper 解析，块级元素间加换行，<img> 转「[图片]」。
//! 后端为 content_text 的 source of truth（前端 update_note 只传 content_html）。

use scraper::{Html, Selector};

/// 把富文本 HTML 抽取为纯文本（FTS 索引 + 列表预览用）。
///
/// 规则：按 DOM 顺序遍历，块级元素（p/h1-6/li/blockquote/div）之间插入换行；
/// `<br>` 转换行；`<img>` 转「[图片]」；其余取 `.text()` 拼接；折叠多余空白。
pub fn extract_text(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    let fragment = Html::parse_fragment(html);
    // TipTap 块级输出为扁平兄弟（p/h1-6/li/blockquote），不含 div 嵌套，
    // 故按这些标签逐块取文本、块间换行即可。<img> 转「[图片]」，<br> 忽略（块间已换行）。
    let block_sel = Selector::parse("p, h1, h2, h3, h4, h5, h6, li, blockquote, br, img").unwrap();

    let mut blocks: Vec<String> = Vec::new();
    for el in fragment.select(&block_sel) {
        let tag = el.value().name();
        if tag == "img" {
            blocks.push("[图片]".to_string());
        } else if tag == "br" {
            // 块间已是 \n 分隔，br 不额外产生空块
        } else {
            let text: String = el.text().collect::<Vec<_>>().join("");
            if !text.trim().is_empty() {
                blocks.push(text);
            }
        }
    }

    // 无任何块级命中（裸文本 HTML）→ 取整段文本
    let joined = if blocks.is_empty() {
        fragment.root_element().text().collect::<Vec<_>>().join("")
    } else {
        blocks.join("\n")
    };

    // 折叠连续空白（保留换行），trim 首尾
    collapse_whitespace(&joined)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_nl = false;
    for line in s.lines() {
        let trimmed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if !prev_nl && !out.is_empty() {
                out.push('\n');
                prev_nl = true;
            }
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&trimmed);
            prev_nl = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_html_returns_empty() {
        assert_eq!(extract_text(""), "");
        assert_eq!(extract_text("   "), "");
    }

    #[test]
    fn single_paragraph() {
        assert_eq!(extract_text("<p>你好世界</p>"), "你好世界");
    }

    #[test]
    fn multiple_blocks_get_newlines() {
        let html = "<h1>标题</h1><p>第一段</p><p>第二段</p>";
        assert_eq!(extract_text(html), "标题\n第一段\n第二段");
    }

    #[test]
    fn img_becomes_placeholder() {
        assert_eq!(extract_text(r#"<p>前</p><img src="note-img:abc" alt="x"><p>后</p>"#), "前\n[图片]\n后");
    }

    #[test]
    fn bare_text_html() {
        assert_eq!(extract_text("裸文本内容"), "裸文本内容");
    }

    #[test]
    fn nested_list_items() {
        let html = "<ul><li>一项</li><li>二项</li></ul>";
        assert_eq!(extract_text(html), "一项\n二项");
    }

    #[test]
    fn list_and_paragraph_mix() {
        let html = "<p>引言</p><ul><li>A</li><li>B</li></ul><p>结语</p>";
        assert_eq!(extract_text(html), "引言\nA\nB\n结语");
    }
}
