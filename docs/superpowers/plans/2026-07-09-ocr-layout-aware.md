# OCR 布局感知 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图/图片 OCR 输出从扁平文本升级为结构化 Markdown（段落 + 列表 + 标题），段内 reflow，提升「截图→AI」输入质量。

**Architecture:** 新增 `crates/ocr/src/layout.rs`——纯函数 `to_markdown(&[OcrBlock]) -> String`，基于 det 框几何信息（h=字号、y 间距=段落边界、text 前缀=列表标记）做布局分析。在 `engine.rs` 的 `run_ocr` 之后、`join("\n")` 之前插入此步骤。`recognize` / `recognize_with_blocks` 返回的 String 语义变为 Markdown。

**Tech Stack:** Rust, 纯算法（无外部依赖），`#[cfg(test)]` 内联测试。

**Spec:** [`docs/superpowers/specs/2026-07-09-ocr-layout-aware-design.md`](../specs/2026-07-09-ocr-layout-aware-design.md)

## Global Constraints

- 常量起始值（实现后可基于实测调参）：`MIN_BLOCKS_FOR_LAYOUT=3`、`TITLE_H2_RATIO=1.3`、`TITLE_H1_RATIO=1.6`、`PARAGRAPH_GAP_RATIO=0.8`
- 测试全部为合成 `OcrBlock` 输入，不依赖真实模型——纯算法可完全确定
- 测试写在 `layout.rs` 的 `#[cfg(test)] mod tests` 内（项目无独立 tests/ 目录）
- markdown 输出末尾无多余空行（trim 尾部）
- `OcrBlock` 定义在 `crates/ocr/src/engine.rs:25`，字段：`text: String, x: f64, y: f64, w: f64, h: f64, score: f64`
- `run_ocr` 返回的 blocks 已经过 `merge_same_line_blocks` + `segment_english_words`，按 y 中心升序排列
- 项目约定：中文交互、中文注释说明「为什么」而非「是什么」

---

## File Structure

| 文件 | 职责 | 操作 |
|------|------|------|
| `crates/ocr/src/layout.rs` | `to_markdown` 函数 + 分类/聚类/reflow 子函数 + 全部测试 | 新增 |
| `crates/ocr/src/engine.rs` | 挂接 `to_markdown`，移除冗余方法 | 修改 |
| `crates/ocr/src/lib.rs` | 声明 `pub mod layout` | 修改 |
| `docs/features/ocr.md` | §6 后处理新增 Markdown 布局感知说明 | 修改 |

---

### Task 1: 创建 layout.rs 骨架 + CJK/ASCII 空格工具函数

**Files:**
- Create: `crates/ocr/src/layout.rs`
- Modify: `crates/ocr/src/lib.rs`

**Interfaces:**
- Produces: `pub fn to_markdown(blocks: &[OcrBlock]) -> String`（本 task 只写签名 + 桩）
- Produces: `fn needs_space_between(prev_last_char: char, curr_first_char: char) -> bool`
- Consumes: `crate::engine::OcrBlock`（`text: String, x: f64, y: f64, w: f64, h: f64, score: f64`）

- [ ] **Step 1: 在 lib.rs 声明模块**

`crates/ocr/src/lib.rs` 当前内容（完整文件）：

```rust
pub mod engine;
pub mod model;
```

改为：

```rust
pub mod engine;
pub mod layout;
pub mod model;
```

- [ ] **Step 2: 创建 layout.rs——常量 + 桩函数 + 空格判定 + 测试**

写入 `crates/ocr/src/layout.rs`：

```rust
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
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p octopus-ocr --lib layout::tests`
Expected: 6 tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/ocr/src/layout.rs crates/ocr/src/lib.rs
git commit -m "feat(ocr): layout.rs 骨架 + CJK/ASCII 空格判定"
```

---

### Task 2: 全局基线 + 逐行分类（标题/列表/正文）

**Files:**
- Modify: `crates/ocr/src/layout.rs`

**Interfaces:**
- Produces: `fn median(values: &[f64]) -> f64`
- Produces: `enum LineKind { H1, H2, ListItem(Option<usize>), Body }` —— `ListItem(Some(n))` 有序（n=序号），`ListItem(None)` 无序
- Produces: `fn classify_line(text: &str, h: f64, median_h: f64) -> (LineKind, String)`
- Produces: `fn strip_unordered_marker(text: &str) -> Option<&str>`
- Produces: `fn strip_ordered_marker(text: &str) -> Option<(usize, &str)>`

- [ ] **Step 1: 在 needs_space_before 之前添加 median + LineKind + 列表标记 + classify_line**

在 `to_markdown` 函数之后、`needs_space_between` 之前插入：

```rust
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
        // ①-⑳：U+2460..U+2473
        if (0x2460..=0x2473).contains(&c) || (0x2474..=0x2487).contains(&c) || (0x2488..=0x249B).contains(&c) {
            let base = if (0x2460..=0x2473).contains(&c) { 0x2460u32 }
                else if (0x2474..=0x2487).contains(&c) { 0x2474u32 }
                else { 0x2488u32 };
            let n = (c - base + 1) as usize;
            let rest: String = chars[1..].iter().collect();
            let rest = rest.trim_start();
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
    if (trimmed.starts_with('（') || trimmed.starts_with('(')) && chars.len() >= 4 {
        let close = if trimmed.starts_with('（') { '）' } else { ')' };
        let inner: String = chars[1..].iter().collect();
        if let Some(close_pos) = inner.find(close) {
            let num_str = &inner[..close_pos];
            if let Ok(n) = num_str.parse::<usize>() {
                let rest = inner[close_pos + close.len_utf8()..].trim_start();
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
```

- [ ] **Step 2: 在 tests mod 添加分类测试**

在 `tests` mod 内（`empty_blocks` test 之后）追加：

```rust
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
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p octopus-ocr --lib layout::tests`
Expected: 全部 pass（原 6 + 新 10 = 16 tests）

- [ ] **Step 4: Commit**

```bash
git add crates/ocr/src/layout.rs
git commit -m "feat(ocr): 基线计算 + 逐行分类（标题/列表/正文）"
```

---

### Task 3: 段落聚类 + Reflow + Markdown 拼装

**Files:**
- Modify: `crates/ocr/src/layout.rs`

**Interfaces:**
- Produces: `fn reflow_paragraph(lines: &[&str]) -> String`
- Produces: `to_markdown` 的完整实现（替换 Task 1 的桩）

- [ ] **Step 1: 用完整实现替换 to_markdown 桩函数**

将 Task 1 的 `to_markdown` 函数替换为：

```rust
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
        ListItemOrdered(String),
        ListItemUnordered(String),
        BodyParagraph(Vec<(String, f64, f64)>),
    }

    let mut units: Vec<Unit> = Vec::new();
    for (kind, text, y, h) in &classified {
        match kind {
            LineKind::H1 => units.push(Unit::Heading(1, text.clone())),
            LineKind::H2 => units.push(Unit::Heading(2, text.clone())),
            LineKind::ListItem(Some(_)) => units.push(Unit::ListItemOrdered(text.clone())),
            LineKind::ListItem(None) => units.push(Unit::ListItemUnordered(text.clone())),
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

    for unit in &units {
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
            Unit::ListItemOrdered(text) => {
                if prev_was_list {
                    output.push('\n');
                } else if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&format!("{}. ", ordered_counter));
                ordered_counter += 1;
                output.push_str(text);
                prev_was_list = true;
            }
            Unit::ListItemUnordered(text) => {
                if prev_was_list {
                    output.push('\n');
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
                let texts: Vec<&str> = lines.iter().map(|(t, _, _)| t.as_str()).collect();
                output.push_str(&reflow_paragraph(&texts));
                prev_was_list = false;
            }
        }
    }

    output.trim_end().to_string()
}
```

- [ ] **Step 2: 在 needs_space_between 后添加 reflow_paragraph**

```rust
/// 将段落的多个文本行 reflow 为一行连续文本，行间按 CJK 感知规则补空格。
fn reflow_paragraph(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut result = String::from(lines[0].trim());
    for line in &lines[1..] {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let prev_last = result.chars().last();
        let curr_first = line.chars().next();
        if let (Some(pl), Some(cf)) = (prev_last, curr_first) {
            if needs_space_between(pl, cf) {
                result.push(' ');
            }
        }
        result.push_str(line);
    }
    result
}
```

- [ ] **Step 3: 在 tests mod 添加 reflow + 端到端测试**

在 `tests` mod 末尾（`classify_title_takes_priority_over_list` 之后）追加：

```rust
    #[test]
    fn reflow_single_line() {
        assert_eq!(reflow_paragraph(&["你好"]), "你好");
    }

    #[test]
    fn reflow_cjk_lines() {
        assert_eq!(
            reflow_paragraph(&["今天天气", "很好我们", "去公园"]),
            "今天天气很好我们去公园"
        );
    }

    #[test]
    fn reflow_mixed_cjk_ascii() {
        assert_eq!(
            reflow_paragraph(&["你好World", "Hello世界"]),
            "你好World Hello世界"
        );
    }

    #[test]
    fn reflow_english_lines() {
        assert_eq!(
            reflow_paragraph(&["The quick", "brown fox"]),
            "The quick brown fox"
        );
    }

    #[test]
    fn reflow_empty() {
        assert_eq!(reflow_paragraph(&[]), "");
    }

    #[test]
    fn end_to_end_single_paragraph() {
        let blocks = vec![
            mk_block("今天天气", 10.0, 0.0, 100.0, 20.0),
            mk_block("很好我们", 10.0, 24.0, 100.0, 20.0),
            mk_block("去公园", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "今天天气很好我们去公园");
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
        assert_eq!(to_markdown(&blocks), "第一段内容继续\n\n第二段内容继续");
    }

    #[test]
    fn end_to_end_h1_title_plus_body() {
        let blocks = vec![
            mk_block("会议纪要", 10.0, 0.0, 200.0, 36.0),  // h=36, median=20 → 1.8 → H1
            mk_block("今天讨论了", 10.0, 50.0, 200.0, 20.0),
            mk_block("三个议题", 10.0, 74.0, 200.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "# 会议纪要\n\n今天讨论了三个议题");
    }

    #[test]
    fn end_to_end_h2_title_plus_body() {
        let blocks = vec![
            mk_block("议题一", 10.0, 0.0, 200.0, 28.0),  // h=28, median=20 → 1.4 → H2
            mk_block("后端迁移", 10.0, 42.0, 200.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "## 议题一\n\n后端迁移");
    }

    #[test]
    fn end_to_end_no_title_equal_height() {
        let blocks = vec![
            mk_block("全部等高", 10.0, 0.0, 100.0, 20.0),
            mk_block("没有标题", 10.0, 24.0, 100.0, 20.0),
            mk_block("纯正文", 10.0, 48.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "全部等高没有标题纯正文");
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
            mk_block("正文段落", 10.0, 0.0, 100.0, 20.0),
            mk_block("• 列表项", 10.0, 50.0, 100.0, 20.0),
        ];
        assert_eq!(to_markdown(&blocks), "正文段落\n\n- 列表项");
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
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p octopus-ocr --lib layout::tests`
Expected: 全部 pass（原 16 + 新 14 = 30 tests）

- [ ] **Step 5: Commit**

```bash
git add crates/ocr/src/layout.rs
git commit -m "feat(ocr): 段落聚类 + reflow + Markdown 拼装"
```

---

### Task 4: 挂接 engine.rs——recognize 返回 Markdown

**Files:**
- Modify: `crates/ocr/src/engine.rs:152-161`（recognize）
- Modify: `crates/ocr/src/engine.rs:163-173`（recognize_with_blocks）
- Modify: `crates/ocr/src/engine.rs:251-260`（移除 recognize_image / recognize_long_image）

- [ ] **Step 1: 修改 recognize——走 with_blocks 路径 + to_markdown**

将 `engine.rs:152-161`：

```rust
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let lines = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image(&img)?
        } else {
            self.recognize_image(&img)?
        };
        Ok(lines.join("\n"))
    }
```

替换为：

```rust
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let blocks = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image_with_blocks(&img)?
        } else {
            self.recognize_image_with_blocks(&img)?
        };
        Ok(crate::layout::to_markdown(&blocks))
    }
```

- [ ] **Step 2: 修改 recognize_with_blocks——text 用 to_markdown**

将 `engine.rs:163-173`：

```rust
    pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let blocks = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image_with_blocks(&img)?
        } else {
            self.recognize_image_with_blocks(&img)?
        };
        let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        Ok((text, blocks))
    }
```

替换为：

```rust
    pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let blocks = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image_with_blocks(&img)?
        } else {
            self.recognize_image_with_blocks(&img)?
        };
        let text = crate::layout::to_markdown(&blocks);
        Ok((text, blocks))
    }
```

- [ ] **Step 3: 移除 recognize_image / recognize_long_image**

删除 `engine.rs:251-260`（recognize 改走 with_blocks 后这两个方法无调用方）：

```rust
    fn recognize_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        let blocks = self.run_ocr(img)?;
        Ok(blocks.into_iter().map(|b| b.text).collect())
    }

    fn recognize_long_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        // 复用 with_blocks 的坐标去重逻辑——纯文本版没有坐标，无法独立去重。
        let blocks = self.recognize_long_image_with_blocks(img)?;
        Ok(blocks.into_iter().map(|b| b.text).collect())
    }
```

- [ ] **Step 4: 编译检查**

Run: `cargo build -p octopus-ocr 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 5: 运行 OCR crate 全部测试**

Run: `cargo test -p octopus-ocr`
Expected: 全部 pass

- [ ] **Step 6: 全 workspace 编译检查**

Run: `cargo build --release -p octopus-server -p octopus-cli 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/ocr/src/engine.rs
git commit -m "feat(ocr): recognize 返回 Markdown，移除冗余纯文本方法"
```

---

### Task 5: 更新文档

**Files:**
- Modify: `docs/features/ocr.md:82`（§6 后处理表格，`segment_english_words` 行之后）

- [ ] **Step 1: 在 ocr.md §6 表格后追加布局感知说明**

找到 `docs/features/ocr.md` 中的 `segment_english_words` 行：

```markdown
| `segment_english_words` | 17.7K 英文词库 `words_common.txt` 贪心分词（仅 PP-OCRv5 需要——v5 CTC 不输出英文空格；v6 CTC space token 已激活，`use_word_segmentation` 按 model_name 前缀判断跳过） |
```

在其后追加（注意保留原表格的空行 `---`）：

```markdown

### 6.1 布局感知：Markdown 输出（2026-07-09）

`crates/ocr/src/layout.rs`——`to_markdown(blocks) -> String`，在 `run_ocr`（merge + segment）之后、`join("\n")` 之前执行。消费 det 框几何信息输出结构化 Markdown：

| 元素 | 检测依据 | 输出 |
|------|----------|------|
| **标题** | 框高 / median_h 比例：≥1.6 → H1（`#`），≥1.3 → H2（`##`） | det 框高 ≈ 字号，大字号独立行判为标题 |
| **列表** | 文本前缀匹配 `•`/`-`/`①`/`1.` 等标记 | 无序 `- text`，有序统一重编号 `1. 2. 3.` |
| **段落** | 连续 Body 行垂直间隙 > median_h × 0.8 → 新段落 | 段间 `\n\n` |
| **段内 reflow** | 同段多行合并为一行，CJK 感知空格（ASCII↔非 ASCII 边界补空格） | 一个段落 = 一段连续文本 |

常量（起始值，可调）：`MIN_BLOCKS_FOR_LAYOUT=3`、`TITLE_H1_RATIO=1.6`、`TITLE_H2_RATIO=1.3`、`PARAGRAPH_GAP_RATIO=0.8`。

块数 < 3 时不分析布局，直接 `\n\n` join。`recognize` / `recognize_with_blocks` 返回的 String 语义从扁平文本变为 Markdown，消费端（DB content / CompactEditor / AI 输入）零改动受益。前端 ImagePreview 叠加不受影响（blocks 仍是原始 det 框）。
```

- [ ] **Step 2: Commit**

```bash
git add docs/features/ocr.md
git commit -m "docs(ocr): 布局感知 Markdown 输出说明"
```

---

### Task 6: 集成验证

**Files:** 无代码改动——验证已有链路

- [ ] **Step 1: 全 workspace 编译（含 desktop）**

Run: `cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 2: 全 crate 测试**

Run: `cargo test -p octopus-ocr`
Expected: 全部 pass

- [ ] **Step 3: 搜索确认无残留调用**

Run: `rg "recognize_image\b|recognize_long_image\b" crates/ --type rust | grep -v "with_blocks"`
Expected: 无输出（确认移除的方法无残留调用）

- [ ] **Step 4: 确认 recognize 调用点不受影响**

Run: `rg "\.recognize" crates/ --type rust | grep -v test | grep -v "with_blocks"`
Expected: `recognize` 返回值直接进 DB content / CompactEditor / AI 输入，Markdown 语义对消费端透明

- [x] **Step 5: 回写 plan 偏差**

### 实际实现偏差（2026-07-09 回写）

1. **Task 2 — `strip_ordered_marker` 借用修复**：plan 原稿用 `chars[1..].iter().collect()` 创建局部 `String` 再返回引用，borrow checker 拒绝。实际改为直接对原始 `trimmed` 切片（circled-number 分支用 `chars[0].len_utf8()` 偏移，parenthesized 分支用 `open_len` 偏移 + `after_open.find(close)`）。功能不变，所有权正确。

2. **Task 3 — 两个测试用例块数不足**：`end_to_end_h2_title_plus_body` 和 `end_to_end_list_plus_paragraph` 原稿各只有 2 个 block，低于 `MIN_BLOCKS_FOR_LAYOUT=3` 阈值，走了 fallback `\n\n` join 路径而非布局分析。实际各自补了第 3 个 block（body 行），使其进入布局分析路径。

3. **Task 6 — desktop 构建需 dummy `dist/`**：worktree 无前端构建产物，Tauri build script 拒绝编译。用 `mkdir -p crates/desktop/dist` 创建空目录后 `cargo check` 通过。Rust 代码无问题，仅为前端 dist 缺失。

4. **常量值未调整**：`TITLE_H1_RATIO=1.6`、`TITLE_H2_RATIO=1.3`、`PARAGRAPH_GAP_RATIO=0.8`、`MIN_BLOCKS_FOR_LAYOUT=3` 均维持起始值，合成测试覆盖通过。真实截图调参留后续实测。

5. **SDD subagent dispatch 受限**：agent tool 仅有 read-only 工具（glob/grep/ls/view），无法执行 implementer subagent。实际由 controller 直接实现全部 6 个 task（TDD 流程不变：每个 task 先写代码+测试，跑通后提交）。

### 代码审查修复（2026-07-09）

6. **多行正文改用 code fence（用户反馈）**：原 plan 的 reflow 方案将多行正文合并为一行连续文本。用户反馈后改为用 ` ``` ` 包裹保留原始分行，避免破坏对齐排版/地址等场景。移除 `reflow_paragraph` + `needs_space_between`（无调用方）。单行正文直接输出，列表项不包裹。

7. **`strip_ordered_marker` 中文顿号 `、` 匹配失败（一轮审查）**：`bytes[i] as char` 只能匹配单字节 ASCII，`、`（U+3001, 3 字节）永远 false。改用 `starts_with` 检查分隔符 + 按字符类型计算字节长度。移除 `Vec<char>` 全量收集改用 `chars().next()`。

8. **`segment_english_words` 最小匹配长度 3→1（一轮审查）**：原 min_len=3 导致 "he"/"is"/"it"/"a" 等短词被拆成单字符。改 min_len=1 后可匹配短词。

9. **code fence 内容含 ``` 时嵌套冲突（一轮审查）**：内容含 ``` 时围栏加长为 ``````。

10. **`ocr_screenshot` 双重解码消除（一轮审查）**：PNG 解码一次后 save + OCR 共用 `DynamicImage`（`recognize_with_blocks_from_image` 新方法），避免 4K/5K 截图重复解码（省 ~100-300ms）。

11. **尾部 merge 回退（二轮审查）**：min_len 改 1 后 Step 4 merge 把合法短词也合并了（`comein` → `comein` 无空格）。删除 Step 4 merge。

12. **列表间距切分（二轮审查）**：`ListItemOrdered/Unordered` 携带 y/h 坐标，连续列表项间距 > median_h×0.8 时 `\n\n` + 重编号。

13. **全角分隔符 `．` `）`（二轮审查）**：补 `．`(U+FF0E) `）`(U+FF09) 支持，sep_len 按半角/全角计算。

14. **word box 分支标注（一轮审查）**：`rapid_ocr.rs` word box 分支标注未启用 + resize 冗余注释（octopus `return_word_box=false`，分支不执行）。
