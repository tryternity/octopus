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

    // 过滤空文本 block
    let blocks: Vec<&OcrBlock> = blocks.iter()
        .filter(|b| !b.text.trim().is_empty())
        .collect();
    if blocks.is_empty() {
        return String::new();
    }

    // 块数不足：直接 \n\n join，不做布局分析
    if blocks.len() < MIN_BLOCKS_FOR_LAYOUT {
        return blocks.iter()
            .map(|b| b.text.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
    }

    // 全局基线
    let heights: Vec<f64> = blocks.iter().map(|b| b.h).collect();
    let median_h = median(&heights);

    // 逐行分类
    let classified: Vec<(LineKind, String, f64, f64)> = blocks.iter()
        .map(|b| {
            let (kind, text) = classify_line(&b.text, b.h, median_h);
            (kind, text, b.y, b.h)
        })
        .collect();

    // 段落聚类——输出单元
    enum Unit {
        Heading(u8, String),
        ListItemOrdered(String, f64, f64),
        ListItemUnordered(String, f64, f64),
        BodyParagraph(Vec<(String, f64, f64)>),
    }

    let mut units: Vec<Unit> = Vec::new();
    for (kind, text, y, h) in &classified {
        match kind {
            LineKind::H1 => units.push(Unit::Heading(1, text.clone())),
            LineKind::H2 => units.push(Unit::Heading(2, text.clone())),
            LineKind::ListItem(Some(_)) => units.push(Unit::ListItemOrdered(text.clone(), *y, *h)),
            LineKind::ListItem(None) => units.push(Unit::ListItemUnordered(text.clone(), *y, *h)),
            LineKind::Body => {
                let should_new_para = match units.last() {
                    Some(Unit::BodyParagraph(lines)) => {
                        let (_, prev_y, prev_h) = *lines.last().unwrap();
                        let gap = y - (prev_y + prev_h);
                        gap > median_h * PARAGRAPH_GAP_RATIO
                    }
                    _ => true,
                };
                if should_new_para {
                    units.push(Unit::BodyParagraph(vec![(text.clone(), *y, *h)]));
                } else if let Some(Unit::BodyParagraph(lines)) = units.last_mut() {
                    lines.push((text.clone(), *y, *h));
                }
            }
        }
    }

    // Markdown 拼装
    let mut output = String::new();
    let mut ordered_counter = 1usize;
    let mut prev_was_list = false;

    for (unit_idx, unit) in units.iter().enumerate() {
        match unit {
            Unit::Heading(level, text) => {
                ordered_counter = 1;
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&"#".repeat(*level as usize));
                output.push(' ');
                output.push_str(text);
                prev_was_list = false;
            }
            Unit::ListItemOrdered(text, y, _h) => {
                // 检查与前一个列表项的间距——大间距说明是不同列表
                if prev_was_list {
                    let need_split = match units.get(unit_idx - 1) {
                        Some(Unit::ListItemOrdered(_, py, ph))
                        | Some(Unit::ListItemUnordered(_, py, ph)) => {
                            y - (py + ph) > median_h * PARAGRAPH_GAP_RATIO
                        }
                        _ => false,
                    };
                    if need_split {
                        ordered_counter = 1;
                        output.push_str("\n\n");
                    } else {
                        output.push('\n');
                    }
                } else if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&format!("{}. ", ordered_counter));
                ordered_counter += 1;
                output.push_str(text);
                prev_was_list = true;
            }
            Unit::ListItemUnordered(text, y, _h) => {
                if prev_was_list {
                    let need_split = match units.get(unit_idx - 1) {
                        Some(Unit::ListItemOrdered(_, py, ph))
                        | Some(Unit::ListItemUnordered(_, py, ph)) => {
                            y - (py + ph) > median_h * PARAGRAPH_GAP_RATIO
                        }
                        _ => false,
                    };
                    if need_split {
                        ordered_counter = 1;
                        output.push_str("\n\n");
                    } else {
                        output.push('\n');
                    }
                } else if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str("- ");
                output.push_str(text);
                prev_was_list = true;
            }
            Unit::BodyParagraph(lines) => {
                ordered_counter = 1;
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                if lines.len() >= 2 {
                    // 多行正文用 code fence 包裹，保留原始分行（不 reflow）
                    // 内容含 ``` 时加长围栏避免嵌套冲突
                    let fence = if lines.iter().any(|(t, _, _)| t.contains("```")) {
                        "````"
                    } else {
                        "```"
                    };
                    output.push_str(fence);
                    output.push('\n');
                    for (t, _, _) in lines {
                        output.push_str(t);
                        output.push('\n');
                    }
                    output.push_str(fence);
                } else {
                    output.push_str(&lines[0].0);
                }
                prev_was_list = false;
            }
        }
    }

    output.trim_end().to_string()
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

    if let Some(first_char) = trimmed.chars().next() {
        let c = first_char as u32;
        // ①-⑳：U+2460..U+2473 / ⑴-⑲⒇：U+2474..U+2487 / ⒈-⒛：U+2488..U+249B
        if (0x2460..=0x2473).contains(&c) || (0x2474..=0x2487).contains(&c) || (0x2488..=0x249B).contains(&c) {
            let base = if (0x2460..=0x2473).contains(&c) { 0x2460u32 }
                else if (0x2474..=0x2487).contains(&c) { 0x2474u32 }
                else { 0x2488u32 };
            let n = (c - base + 1) as usize;
            let first_len = first_char.len_utf8();
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
    if i > 0 && i < trimmed.len() {
        let after_digits = &trimmed[i..];
        if after_digits.starts_with('.') || after_digits.starts_with('、')
            || after_digits.starts_with(')') || after_digits.starts_with('．')
            || after_digits.starts_with('）')
        {
            // 全角字符（、 ． ）占 3 字节，半角（. )）占 1 字节
            let sep_len = if after_digits.starts_with('.') || after_digits.starts_with(')') { 1 } else { 3 };
            let num_str = &trimmed[..i];
            if let Ok(n) = num_str.parse::<usize>() {
                let rest = trimmed[i + sep_len..].trim_start();
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn classify_ordered_cn_comma() {
        let (kind, text) = classify_line("1、第一项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(1)));
        assert_eq!(text, "第一项");
    }

    #[test]
    fn classify_ordered_fullwidth_dot() {
        let (kind, text) = classify_line("1．第一项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(1)));
        assert_eq!(text, "第一项");
    }

    #[test]
    fn classify_ordered_fullwidth_paren() {
        let (kind, text) = classify_line("2）第二项", 20.0, 20.0);
        assert_eq!(kind, LineKind::ListItem(Some(2)));
        assert_eq!(text, "第二项");
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

    #[test]
    fn end_to_end_single_paragraph_fenced() {
        let blocks = vec![
            mk_block("今天天气", 10.0, 0.0, 100.0, 20.0),
            mk_block("很好我们", 10.0, 24.0, 100.0, 20.0),
            mk_block("去公园", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "```\n今天天气\n很好我们\n去公园\n```");
    }

    #[test]
    fn end_to_end_two_paragraphs() {
        let blocks = vec![
            mk_block("第一段内容", 10.0, 0.0, 100.0, 20.0),
            mk_block("继续", 10.0, 24.0, 100.0, 20.0),
            // gap_y = 62 - (24+20) = 18 > 20*0.8=16 → 新段落
            mk_block("第二段内容", 10.0, 62.0, 100.0, 20.0),
            mk_block("继续", 10.0, 86.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "```\n第一段内容\n继续\n```\n\n```\n第二段内容\n继续\n```");
    }

    #[test]
    fn end_to_end_h1_title_plus_body() {
        let blocks = vec![
            mk_block("会议纪要", 10.0, 0.0, 200.0, 36.0),  // h=36, median=20 → 1.8 → H1
            mk_block("今天讨论了", 10.0, 50.0, 200.0, 20.0),
            mk_block("三个议题", 10.0, 74.0, 200.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "# 会议纪要\n\n```\n今天讨论了\n三个议题\n```");
    }

    #[test]
    fn end_to_end_h2_title_plus_body() {
        let blocks = vec![
            mk_block("议题一", 10.0, 0.0, 200.0, 28.0),  // h=28, median=20 → 1.4 → H2
            mk_block("后端迁移", 10.0, 42.0, 200.0, 20.0),
            mk_block("预计完成", 10.0, 66.0, 200.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "## 议题一\n\n```\n后端迁移\n预计完成\n```");
    }

    #[test]
    fn end_to_end_no_title_equal_height() {
        let blocks = vec![
            mk_block("全部等高", 10.0, 0.0, 100.0, 20.0),
            mk_block("没有标题", 10.0, 24.0, 100.0, 20.0),
            mk_block("纯正文", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "```\n全部等高\n没有标题\n纯正文\n```");
    }

    #[test]
    fn end_to_end_unordered_list() {
        let blocks = vec![
            mk_block("• 第一项", 10.0, 0.0, 100.0, 20.0),
            mk_block("• 第二项", 10.0, 24.0, 100.0, 20.0),
            mk_block("• 第三项", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "- 第一项\n- 第二项\n- 第三项");
    }

    #[test]
    fn end_to_end_ordered_list() {
        let blocks = vec![
            mk_block("① 第一项", 10.0, 0.0, 100.0, 20.0),
            mk_block("② 第二项", 10.0, 24.0, 100.0, 20.0),
            mk_block("③ 第三项", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "1. 第一项\n2. 第二项\n3. 第三项");
    }

    #[test]
    fn end_to_end_list_plus_paragraph() {
        let blocks = vec![
            mk_block("正文段落一", 10.0, 0.0, 100.0, 20.0),
            mk_block("正文段落二", 10.0, 24.0, 100.0, 20.0),
            mk_block("• 列表项", 10.0, 70.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "```\n正文段落一\n正文段落二\n```\n\n- 列表项");
    }

    #[test]
    fn end_to_end_empty_block_filtered() {
        let blocks = vec![
            mk_block("正文", 10.0, 0.0, 100.0, 20.0),
            mk_block("", 10.0, 24.0, 100.0, 20.0),     // 空文本，过滤后块数 < 3 → \n\n join
            mk_block("继续", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "正文\n\n继续");
    }

    #[test]
    fn end_to_end_few_blocks_no_layout() {
        // < MIN_BLOCKS_FOR_LAYOUT(3)：不做布局，\n\n join
        let blocks = vec![
            mk_block("只有两行", 10.0, 0.0, 100.0, 20.0),
            mk_block("不分析", 10.0, 24.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "只有两行\n\n不分析");
    }

    #[test]
    fn end_to_end_list_gap_split() {
        // 两个有序列表，间距很大 → 应回车重编号
        // list A: y=0,24; gap: 70-(24+20)=26 > 20*0.8=16; list B: y=70,94
        let blocks = vec![
            mk_block("① 第一项", 10.0, 0.0, 100.0, 20.0),
            mk_block("② 第二项", 10.0, 24.0, 100.0, 20.0),
            mk_block("① 第三项", 10.0, 70.0, 100.0, 20.0),
            mk_block("② 第四项", 10.0, 94.0, 100.0, 20.0),
        ];
        assert_eq!(
            to_markdown(&blocks),
            "1. 第一项\n2. 第二项\n\n1. 第三项\n2. 第四项"
        );
    }

    #[test]
    fn end_to_end_unordered_list_gap_split() {
        let blocks = vec![
            mk_block("• A1", 10.0, 0.0, 100.0, 20.0),
            mk_block("• A2", 10.0, 24.0, 100.0, 20.0),
            // gap: 70 - (24+20) = 26 > 16
            mk_block("• B1", 10.0, 70.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "- A1\n- A2\n\n- B1");
    }
}
