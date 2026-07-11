//! 候选挖掘：扫历史 ASR 文本，jieba 分词 + 词频过滤，低频高频专名 → 返回候选词列表（命令层追加到版本）。

use jieba_rs::Jieba;

/// 用户历史中至少出现此次数才作候选。
const MIN_USER_COUNT: usize = 2;
/// 单次挖掘回看的历史条数。
const HISTORY_LIMIT: i64 = 500;
/// 单次最多写入的候选数。
const MAX_CANDIDATES: usize = 30;
/// 候选词长度范围（专名通常是 2-4 字）。
const MIN_LEN: usize = 2;
const MAX_LEN: usize = 4;

/// 是否值得作为候选：长度 2-4、纯汉字、且不在语言模型常用词表内（低频专名）。
pub fn is_candidate(word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < MIN_LEN || chars.len() > MAX_LEN {
        return false;
    }
    // 纯汉字（非汉字 char_fuzzy_pinyin 返回 None → 含则排除）
    if chars
        .iter()
        .any(|c| crate::hotword::char_fuzzy_pinyin(*c).is_none())
    {
        return false;
    }
    // 在 corrector 语言模型常用词表 → 过滤；不在 → 低频专名，保留
    !crate::corrector::is_common_word(word)
}

/// 扫历史 → jieba 分词 → 词频过滤 → top-N 候选词。返回词列表（不写 DB）。
/// 命令层拿去追加到用户选定版本（废弃旧 pending 流）。
pub fn collect_candidate_words() -> anyhow::Result<Vec<String>> {
    let texts = octopus_infra::db::list_recent_text(HISTORY_LIMIT)?;
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let jieba = Jieba::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &texts {
        for w in jieba.cut(t, true) {
            if !is_candidate(w) {
                continue;
            }
            *counts.entry(w.to_string()).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_USER_COUNT)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(MAX_CANDIDATES);
    let words: Vec<String> = ranked.into_iter().map(|(w, _)| w).collect();
    log::info!("[hotword-miner] 挖掘 {} 条候选词", words.len());
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_rare_drops_common() {
        // 「我们」在语言模型常用词表（过滤）；「八爪鱼」不在（保留）
        let keep = is_candidate("八爪鱼");
        let drop_common = is_candidate("我们");
        assert!(keep, "低频专名应保留");
        assert!(!drop_common, "高频常用词应过滤");
    }

    #[test]
    fn length_bounds_enforced() {
        // 单字非候选（长度 < MIN_LEN）
        assert!(!is_candidate("的"));
    }

    #[test]
    fn collect_returns_ranked_candidates() {
        // collect_candidate_words 不写 DB，仅返回候选词列表（依赖 list_recent_text）。
        // 此处只验返回类型与非 panic；真实历史由 e2e 覆盖。
        let _ = collect_candidate_words();
    }
}
