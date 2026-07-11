//! OCR 布局感知后处理：将 det 文本框按几何关系聚合成 Markdown 结构。
//!
//! 消费 `OcrBlock` 的 h（字号代理）、y 间距（段落边界）、text 前缀（列表标记），
//! 输出段落（`\n\n`）+ 列表（`- ` / `1. `）+ 标题（`#` / `##`）的 Markdown。

use crate::engine::OcrBlock;

/// 块数不足时不分析布局——太少行无法可靠统计基线。
const MIN_BLOCKS_FOR_LAYOUT: usize = 3;
/// h ≥ median_h × 1.3 → H2 标题
const TITLE_H2_RATIO: f64 = 1.3;
/// h ≥ median_h × 1.6 → H1 标题
const TITLE_H1_RATIO: f64 = 1.6;
/// 两行垂直间隙 > median_h × 0.8 → 新段落
const PARAGRAPH_GAP_RATIO: f64 = 0.8;

/// 将 OCR 文本框转为结构化 Markdown。
///
/// `blocks` 应已过 `merge_same_line_blocks` + `segment_english_words`，
/// 按 y 中心升序排列。
pub fn to_markdown(blocks: &[OcrBlock]) -> String {
    if blocks.is_empty() {
        return String::new();
    }
    // Task 2-3 逐步填充
    blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// 判断 reflow 时两行之间是否需要插入空格（CJK 感知）。
/// ASCII↔非 ASCII 边界补空格；CJK↔CJK 不补。
fn needs_space_between(prev_last: char, curr_first: char) -> bool {
    prev_last.is_ascii() || curr_first.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_space_cjk_cjk() {
        assert!(!needs_space_between('界', '你'));
    }

    #[test]
    fn needs_space_cjk_ascii() {
        assert!(needs_space_between('界', 'H'));
    }

    #[test]
    fn needs_space_ascii_cjk() {
        assert!(needs_space_between('d', '你'));
    }

    #[test]
    fn needs_space_ascii_ascii() {
        assert!(needs_space_between('d', 'W'));
    }

    #[test]
    fn needs_space_punctuation() {
        // 中文标点（。，！）非 ASCII → 不补空格
        assert!(!needs_space_between('。', '你'));
        // 英文标点（.,!）是 ASCII → 补空格
        assert!(needs_space_between('.', 'W'));
    }

    #[test]
    fn empty_blocks() {
        assert_eq!(to_markdown(&[]), "");
    }
}
