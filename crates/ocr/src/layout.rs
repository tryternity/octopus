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

/// 计算中位数。空切片返回 0.0。
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

/// 行类型分类结果。
#[derive(Debug, Clone, PartialEq)]
enum LineKind {
    H1,
    H2,
    /// `Some(n)` = 有序列表（n 从 1 开始的序号）；`None` = 无序列表
    ListItem(Option<usize>),
    Body,
}

/// 判断文本是否以无序列表标记开头，返回去掉标记后的正文。
/// 支持：• · ○ ● ■ □ ◆ ◇ ► ▶ － - — ＊ * + 空格
fn strip_unordered_marker(text: &str) -> Option<&str> {
    let markers: &[&str] = &[
        "•", "·", "○", "●", "■", "□", "◆", "◇", "►", "▶",
        "－", "-", "—", "＊", "*", "+",
    ];
    let trimmed = text.trim_start();
    for m in markers {
        if let Some(rest) = trimmed.strip_prefix(m) {
            let rest = rest.trim_start();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

/// 判断文本是否以有序列表标记开头，返回 (序号, 去掉标记后的正文)。
/// 支持：①-⑳ ⑴-⑵⓪ ⒈-⒛ \d+[.、)] [（(]\d+[)）]
fn strip_ordered_marker(text: &str) -> Option<(usize, &str)> {
    let trimmed = text.trim_start();
    let chars: Vec<char> = trimmed.chars().collect();

    if !chars.is_empty() {
        let c = chars[0] as u32;
        // ①-⑳：U+2460..U+2473 / ⑴-⑲⒇：U+2474..U+2487 / ⒈-⒛：U+2488..U+249B
        if (0x2460..=0x2473).contains(&c) || (0x2474..=0x2487).contains(&c) || (0x2488..=0x249B).contains(&c) {
            let base = if (0x2460..=0x2473).contains(&c) { 0x2460u32 }
                else if (0x2474..=0x2487).contains(&c) { 0x2474u32 }
                else { 0x2488u32 };
            let n = (c - base + 1) as usize;
            let first_len = chars[0].len_utf8();
            let rest = trimmed[first_len..].trim_start();
            if !rest.is_empty() {
                return Some((n, rest));
            }
        }
    }

    // \d+[.、)] 空格?
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() {
        let sep = bytes[i] as char;
        if sep == '.' || sep == '、' || sep == ')' {
            let num_str = &trimmed[..i];
            if let Ok(n) = num_str.parse::<usize>() {
                let rest = trimmed[i + 1..].trim_start();
                if !rest.is_empty() {
                    return Some((n, rest));
                }
            }
        }
    }

    // [（(]\d+[)）]
    if trimmed.starts_with('（') || trimmed.starts_with('(') {
        let open_len = if trimmed.starts_with('（') { 3 } else { 1 };
        let close = if trimmed.starts_with('（') { '）' } else { ')' };
        let after_open = &trimmed[open_len..];
        if let Some(close_byte_pos) = after_open.find(close) {
            let num_str = &after_open[..close_byte_pos];
            if let Ok(n) = num_str.parse::<usize>() {
                let rest = after_open[close_byte_pos + close.len_utf8()..].trim_start();
                if !rest.is_empty() {
                    return Some((n, rest));
                }
            }
        }
    }

    None
}

/// 对单行 block 做分类。h=块高，median_h=正文中位行高。
/// 返回 (LineKind, 去掉列表标记后的正文)。
fn classify_line(text: &str, h: f64, median_h: f64) -> (LineKind, String) {
    // 先判标题（字号特征比文本前缀更可靠）
    if median_h > 0.0 {
        let ratio = h / median_h;
        if ratio >= TITLE_H1_RATIO {
            return (LineKind::H1, text.trim().to_string());
        }
        if ratio >= TITLE_H2_RATIO {
            return (LineKind::H2, text.trim().to_string());
        }
    }

    // 再判列表标记
    if let Some((n, rest)) = strip_ordered_marker(text) {
        return (LineKind::ListItem(Some(n)), rest.to_string());
    }
    if let Some(rest) = strip_unordered_marker(text) {
        return (LineKind::ListItem(None), rest.to_string());
    }

    (LineKind::Body, text.trim().to_string())
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

    fn mk_block(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrBlock {
        OcrBlock { text: text.to_string(), x, y, w, h, score: 0.9 }
    }

    #[test]
    fn median_basic() {
        assert_eq!(median(&[1.0, 3.0, 2.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn classify_h1() {
        let (kind, _) = classify_line("标题", 32.0, 20.0);
        assert_eq!(kind, LineKind::H1); // 32/20 = 1.6
    }

    #[test]
    fn classify_h2() {
        let (kind, _) = classify_line("子标题", 26.0, 20.0);
        assert_eq!(kind, LineKind::H2); // 26/20 = 1.3
    }

    #[test]
    fn classify_body_equal_height() {
        let (kind, _) = classify_line("正文", 20.0, 20.0);
        assert_eq!(kind, LineKind::Body);
    }

    #[test]
    fn classify_unordered_bullet() {
        let (kind, text) = classify_line("• 第一项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(None));
        assert_eq!(text, "第一项");
    }

    #[test]
    fn classify_unordered_dash() {
        let (kind, text) = classify_line("- 第二项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(None));
        assert_eq!(text, "第二项");
    }

    #[test]
    fn classify_ordered_circled() {
        let (kind, text) = classify_line("①第一项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(1)));
        assert_eq!(text, "第一项");
    }

    #[test]
    fn classify_ordered_digit_dot() {
        let (kind, text) = classify_line("1. 第一项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(1)));
        assert_eq!(text, "第一项");
    }

    #[test]
    fn classify_ordered_paren_cn() {
        let (kind, text) = classify_line("（2）第二项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(2)));
        assert_eq!(text, "第二项");
    }

    #[test]
    fn classify_title_takes_priority_over_list() {
        // 大字号 + 列表标记 → 标题优先
        let (kind, _) = classify_line("• 大标题", 32.0, 20.0);
        assert_eq!(kind, LineKind::H1);
    }
}
