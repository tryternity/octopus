//! 候选挖掘：扫历史 ASR 文本，jieba 分词 + 词频过滤，低频高频专名 → DB pending。

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

/// 扫历史 → jieba 分词 → 词频过滤 → top-N 写 pending。返回写入条数。
pub fn mine_pending_candidates() -> anyhow::Result<usize> {
    let texts = octopus_infra::db::list_recent_text(HISTORY_LIMIT)?;
    if texts.is_empty() {
        return Ok(0);
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
    // 用户高频（≥ MIN_USER_COUNT）的候选，按频次降序取 top-N
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_USER_COUNT)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(MAX_CANDIDATES);

    let mut written = 0;
    for (word, _) in &ranked {
        // INSERT（word 唯一约束）：已存在（任意状态）则 Err 被吞 → 等价 OR IGNORE，不覆盖 active
        match octopus_infra::db::insert_hotword(word, "mined", "pending") {
            Ok(_) => written += 1,
            Err(_) => {} // 已存在，跳过
        }
    }
    log::info!("[hotword-miner] 挖掘写入 {} 条 pending 候选", written);
    Ok(written)
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
}
