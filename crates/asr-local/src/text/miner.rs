//! 候选挖掘：扫历史 ASR 文本，jieba 分词 + 词频过滤，低频高频专名 → 返回候选词列表（命令层追加到版本）。
//! jieba 复用 corrector 单例（见 collect_candidate_words），避免每次新建的词典加载开销。

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
    // 复用 corrector 单例的 jieba（cut 是 &self 只读，线程安全）；避免每次挖掘重建词典。
    let jieba = crate::corrector::get_corrector().jieba();
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

/// bigram 上下文打分用的历史条数（与挖掘共用，但只取 voice）。
const BIGRAM_HISTORY_LIMIT: i64 = 500;

/// 扫历史 voice 文本 → 字级 bigram 频次表。
/// 对每条文本取相邻字符对计数（不分词，极轻量）。
/// 用于 corrector 多命中排序的上下文打分（bigram_score）。
pub fn build_char_bigram_index() -> anyhow::Result<std::collections::HashMap<(char, char), usize>> {
    let texts = octopus_infra::db::list_recent_voice_text(BIGRAM_HISTORY_LIMIT)?;
    Ok(build_char_bigram_index_from(&texts))
}

/// 从给定文本列表构建字级 bigram 频次表（测试用，不碰 DB）。
pub fn build_char_bigram_index_from(texts: &[String]) -> std::collections::HashMap<(char, char), usize> {
    let mut index: std::collections::HashMap<(char, char), usize> = std::collections::HashMap::new();
    for t in texts {
        let chars: Vec<char> = t.chars().collect();
        for pair in chars.windows(2) {
            *index.entry((pair[0], pair[1])).or_insert(0) += 1;
        }
    }
    index
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

    // ── 字级 bigram 构建（2026-08-01）──

    #[test]
    fn bigram_counts_adjacent_char_pairs() {
        let idx = build_char_bigram_index_from(&["打开八爪鱼".into()]);
        // (打,开)(开,八)(八,爪)(爪,鱼) 各 1
        assert_eq!(idx.get(&('打', '开')), Some(&1));
        assert_eq!(idx.get(&('开', '八')), Some(&1));
        assert_eq!(idx.get(&('八', '爪')), Some(&1));
        assert_eq!(idx.get(&('爪', '鱼')), Some(&1));
        // 不相邻的字符对不在
        assert!(idx.get(&('打', '八')).is_none());
    }

    #[test]
    fn bigram_accumulates_across_texts() {
        let idx = build_char_bigram_index_from(&[
            "打开八爪鱼".into(),
            "打开浮窗".into(),
        ]);
        // 「打开」在两条都出现 → (打,开) = 2
        assert_eq!(idx.get(&('打', '开')), Some(&2));
        // 跨条不连续（条1末「鱼」+ 条2首「打」不是 bigram）
        assert!(idx.get(&('鱼', '打')).is_none());
    }
}
