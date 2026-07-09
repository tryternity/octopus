use std::collections::{HashMap, HashSet};

use pinyin::ToPinyin;

/// 模糊拼音归一化——与 corrector.rs 旧逻辑 1:1 一致：
/// zh→z, ch→c, sh→s（平翘舌）；n→l；ing→in, eng→en, ang→an（前后鼻音）。
pub fn normalize_fuzzy_pinyin(py: &str) -> String {
    let mut n = py.to_lowercase();
    if n.starts_with("zh") {
        n = n.replacen("zh", "z", 1);
    } else if n.starts_with("ch") {
        n = n.replacen("ch", "c", 1);
    } else if n.starts_with("sh") {
        n = n.replacen("sh", "s", 1);
    }
    if n.starts_with('n') {
        n = format!("l{}", &n[1..]);
    }
    if n.ends_with("ing") {
        n = n[..n.len() - 3].to_string() + "in";
    } else if n.ends_with("eng") {
        n = n[..n.len() - 3].to_string() + "en";
    } else if n.ends_with("ang") {
        n = n[..n.len() - 3].to_string() + "an";
    }
    n
}

/// 单字 → 归一化模糊拼音；非汉字（无拼音）返回 None。
pub fn char_fuzzy_pinyin(c: char) -> Option<String> {
    c.to_pinyin().map(|p| normalize_fuzzy_pinyin(p.plain()))
}

/// 热词的内存索引：按「字数 → 归一化拼音 → 候选词列表」分组。
/// 纠错热路径按窗口字数与拼音 O(1) 查表。
pub struct HotwordIndex {
    by_len_py: HashMap<usize, HashMap<String, Vec<String>>>,
    active_words: HashSet<String>,
}

impl HotwordIndex {
    pub fn empty() -> Self {
        Self { by_len_py: HashMap::new(), active_words: HashSet::new() }
    }

    /// words 为 active 热词文本列表（来自 DB list_active_hotword_words）。
    /// 单字热词忽略（歧义太大）；含非汉字的热词忽略（拼音数 ≠ 字数）。
    pub fn from_words(words: &[String]) -> Self {
        let mut by_len_py: HashMap<usize, HashMap<String, Vec<String>>> = HashMap::new();
        let mut active_words = HashSet::new();
        for w in words {
            let chars: Vec<char> = w.chars().collect();
            let len = chars.len();
            if len < 2 { continue; }
            let py: Vec<String> = chars.iter().filter_map(|&c| char_fuzzy_pinyin(c)).collect();
            if py.len() != len { continue; } // 含非汉字 → 跳过
            let key = py.join("-");
            by_len_py.entry(len).or_default().entry(key).or_default().push(w.clone());
            active_words.insert(w.clone());
        }
        Self { by_len_py, active_words }
    }

    pub fn is_empty(&self) -> bool { self.active_words.is_empty() }

    pub fn max_len(&self) -> usize { *self.by_len_py.keys().max().unwrap_or(&0) }

    pub fn lookup(&self, len: usize, py: &str) -> Option<&Vec<String>> {
        self.by_len_py.get(&len)?.get(py)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_is_empty() {
        let idx = HotwordIndex::from_words(&[]);
        assert!(idx.is_empty());
        assert_eq!(idx.max_len(), 0);
        assert!(idx.lookup(3, "ba-zhua-yu").is_none());
    }

    #[test]
    fn groups_by_length_and_pinyin() {
        let idx = HotwordIndex::from_words(&[
            "八爪鱼".to_string(),   // 八(ba) 爪(zhao→zao) 鱼(yu) → "ba-zao-yu", len 3
            "巴掌鱼".to_string(),   // 巴(ba) 掌(zhang→zan) 鱼(yu) → "ba-zan-yu", len 3
            "吴大锐".to_string(),   // 吴(wu) 大(da) 锐(rui) → "wu-da-rui", len 3
        ]);
        assert!(!idx.is_empty());
        assert_eq!(idx.max_len(), 3);
        // 归一化拼音 lookup：爪 zhao→zao（平翘舌归一）
        assert!(idx.lookup(3, "ba-zao-yu").is_some());
        // 模糊：掌 zhang→zan 归一后能查到「巴掌鱼」
        assert!(idx.lookup(3, "ba-zan-yu").is_some());
        // 不存在的拼音
        assert!(idx.lookup(3, "xxx-yyy-zzz").is_none());
    }

    #[test]
    fn fuzzy_pinyin_normalizes_accents() {
        // 卫 wei（无归一），生 sheng→sen（sh→s, eng→en）
        assert_eq!(char_fuzzy_pinyin('卫'), Some("wei".to_string()));
        assert_eq!(char_fuzzy_pinyin('生'), Some("sen".to_string()));
        assert_eq!(char_fuzzy_pinyin('A'), None); // 非汉字
    }
}
