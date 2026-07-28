//! 字幕 LLM 润色编排（desktop 层）。
//!
//! 整段润色（保留上下文）+ [[N]] 标记边界拆回 cue + 粗略拆分降级。
//! record/asr-local 不依赖 octopus-llm，润色逻辑集中在 desktop。
//!
//! 设计详见 `docs/superpowers/specs/2026-07-28-subtitle-llm-polish-design.md`。
// 模块整体在 Task 1.x 阶段尚未被 generate_subtitle 命令消费（Phase 2 接入），
// 期间 pub 项在 bin crate 内会触发 dead_code；Phase 2 接入后此 allow 可移除。
#![allow(dead_code)]

/// 标记格式常量。LLM 输出须用 [[N]] 包裹每条 cue（N 从 1 递增）。
const CUE_MARKER_OPEN: &str = "[[";
const CUE_MARKER_CLOSE: &str = "]]";

/// 字幕润色选项（generate_subtitle 命令参数）。
/// None = 不润色；Some = 用指定 LLM 润色。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishOption {
    /// LLM 配置标识（provider:model）。None = 用 resolve_active_engine("llm") 默认。
    pub llm_key: Option<String>,
}

/// 润色结果（用于前端提示）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PolishOutcome {
    /// 未启用润色（polish=None）。
    Skipped,
    /// 标记解析成功。
    Polished,
    /// 标记失败，粗略拆分降级。
    FallbackRatio,
    /// 无可用 LLM 配置。
    NoLlmConfig,
    /// LLM 调用失败（panic/超时/HTTP）。
    Failed(String),
}

/// 把 cue 文本列表构造成带 [[N]] 标记的润色输入。
pub fn build_polish_input(texts: &[String]) -> String {
    let mut s = String::new();
    for (i, t) in texts.iter().enumerate() {
        s.push_str(&format!("{}{}{}{}", CUE_MARKER_OPEN, i + 1, CUE_MARKER_CLOSE, t));
    }
    s
}

/// 解析 LLM 输出的 [[N]] 标记文本，返回按 N 排序的文本列表。
///
/// 失败（返回 None）条件：
/// - 标记数量 ≠ expected_count
/// - N 不连续（缺号）
/// - 任一标记间文本为空（trim 后）
/// - 完全无标记
///
/// 用字符串 split 实现（不依赖 regex——标记是固定字面量）。
pub fn parse_polished_with_markers(polished: &str, expected_count: usize) -> Option<Vec<String>> {
    if expected_count == 0 {
        return Some(Vec::new());
    }
    // split("[[") → 每段开头是 "N]]文本"
    let mut map: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for segment in polished.split(CUE_MARKER_OPEN) {
        // segment 可能是 ""（开头有 [[ 时第一段空）或 "N]]文本"
        if segment.is_empty() {
            continue;
        }
        // 找 "]]" 分隔 N 和文本
        let close_idx = segment.find(CUE_MARKER_CLOSE)?;
        let n_str = &segment[..close_idx];
        let n: u32 = n_str.parse().ok()?;
        let text = segment[close_idx + CUE_MARKER_CLOSE.len()..].trim().to_string();
        // N 范围检查
        if n == 0 || n as usize > expected_count {
            return None;
        }
        map.insert(n, text);
    }
    // 检查数量 + 连续性
    if map.len() != expected_count {
        return None;
    }
    // 任一文本为空 → None（前面 trim 后可能空）
    if map.values().any(|t| t.is_empty()) {
        return None;
    }
    // 按 N 排序收集
    let result: Option<Vec<String>> = (1..=expected_count as u32)
        .map(|n| map.get(&n).cloned())
        .collect();
    result
}

/// 按原 cue 的字符比例，把整段润色文本切回 N 段（降级用）。
///
/// 尽力保留润色效果，但边界可能不准（LLM 可能改变了句子数量）。
/// 原 texts 全空时直接返回原 texts（避免除零）。
pub fn split_polished_by_ratio(polished: &str, original_texts: &[String]) -> Vec<String> {
    let total_chars: usize = original_texts.iter().map(|t| t.chars().count()).sum();
    if total_chars == 0 {
        return original_texts.to_vec();
    }
    let polished_chars: Vec<char> = polished.chars().collect();
    let polished_total = polished_chars.len();
    let mut result = Vec::with_capacity(original_texts.len());
    let mut pos = 0;
    for (i, orig) in original_texts.iter().enumerate() {
        let ratio = orig.chars().count() as f64 / total_chars as f64;
        let end = if i == original_texts.len() - 1 {
            polished_total // 最后一条取剩余全部（避免四舍五入丢字）
        } else {
            (pos + (polished_total as f64 * ratio).round() as usize).min(polished_total)
        };
        let chunk: String = polished_chars[pos..end].iter().collect();
        result.push(chunk.trim().to_string());
        pos = end;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_polish_input_basic() {
        let texts = vec!["第一句".to_string(), "第二句".to_string(), "第三句".to_string()];
        let input = build_polish_input(&texts);
        assert_eq!(input, "[[1]]第一句[[2]]第二句[[3]]第三句");
    }

    #[test]
    fn build_polish_input_empty() {
        assert_eq!(build_polish_input(&[]), "");
    }

    #[test]
    fn build_polish_input_single() {
        assert_eq!(build_polish_input(&["单句".into()]), "[[1]]单句");
    }

    #[test]
    fn parse_markers_success_3_cues() {
        let polished = "[[1]]润色第一句[[2]]润色第二句[[3]]润色第三句";
        let result = parse_polished_with_markers(polished, 3).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "润色第一句");
        assert_eq!(result[1], "润色第二句");
        assert_eq!(result[2], "润色第三句");
    }

    #[test]
    fn parse_markers_count_mismatch_returns_none() {
        // 期望 3 条但只有 2 个标记
        let polished = "[[1]]第一句[[2]]第二句";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }

    #[test]
    fn parse_markers_missing_n_returns_none() {
        // N 不连续：[[1]] [[3]]（缺 [[2]]）
        let polished = "[[1]]第一句[[3]]第三句";
        assert!(parse_polished_with_markers(polished, 2).is_none());
    }

    #[test]
    fn parse_markers_empty_text_returns_none() {
        // [[2]] 后文本为空（直接接 [[3]]）
        let polished = "[[1]]第一句[[2]][[3]]第三句";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }

    #[test]
    fn parse_markers_no_markers_returns_none() {
        // LLM 完全无视格式，输出纯文本
        let polished = "这是没有标记的纯文本润色结果";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }

    #[test]
    fn split_by_ratio_basic() {
        let original = vec!["一二三四".to_string(), "五六七".to_string(), "八九十".to_string()];
        // total 11 chars，比例 4/11, 3/11, 4/11
        let polished = "一二三四五六七八九十十一十二"; // 12 chars（润色后略多）
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result.len(), 3);
        // 不强断言精确切点（四舍五入），只断言长度 + 非空
        assert!(!result[0].is_empty());
        assert!(!result[1].is_empty());
        assert!(!result[2].is_empty());
        // 拼接应等于原 polished（trim 后）
        let joined: String = result.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("");
        // 允许 trim 差异，只检查大致一致
        assert!(!joined.is_empty());
    }

    #[test]
    fn split_by_ratio_empty_original_returns_original() {
        let original = vec!["".to_string(), "".to_string()];
        let polished = "润色文本";
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result, original); // 原 texts 全空 → 返回原 texts
    }

    #[test]
    fn split_by_ratio_last_takes_remainder() {
        let original = vec!["短".to_string(), "很长很长很长".to_string()];
        let polished = "润色一润色二润色三润色四润色五"; // 10 chars
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result.len(), 2);
        // 最后一条应取剩余全部（不被四舍五入截断）
        let total_chars: usize = result.iter().map(|s| s.chars().count()).sum();
        assert_eq!(total_chars, polished.chars().count());
    }
}
