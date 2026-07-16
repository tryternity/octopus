//! 模糊匹配 + 拼音首字母。

use nucleo_matcher::{Matcher, Config};
use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};
use nucleo_matcher::Utf32Str;
use std::cell::RefCell;

/// 匹配得分类型（越高越好，<0 表示不匹配）。
pub type Score = i32;

// fuzzy Matcher 复用（thread_local）。nucleo 的 Matcher 设计为 reset 复用，
// 对大书签列表（数百~上千条逐条调 fuzzy_match）避免每次重新分配 score table。
thread_local! {
    static FUZZY_MATCHER: RefCell<Matcher> = RefCell::new(Matcher::new(Config::DEFAULT));
}

/// 精确匹配：query == target（忽略大小写）。
pub fn exact_match(query: &str, target: &str) -> Option<Score> {
    if query.eq_ignore_ascii_case(target) {
        Some(10000)
    } else {
        None
    }
}

/// 前缀匹配：target 以 query 开头（忽略大小写）。
/// 打分：base 5000，剩余字符越少（越接近精确匹配）分越高。
/// 用 char count 而非 byte len——CJK 文字 3 bytes/char，byte 算法系统性压低中文。
pub fn prefix_match(query: &str, target: &str) -> Option<Score> {
    if target.to_lowercase().starts_with(&query.to_lowercase()) {
        let remaining = target.chars().count().saturating_sub(query.chars().count());
        Some(5000 - remaining as Score)
    } else {
        None
    }
}

/// 词级前缀匹配：target 按空格/标点分词后，query 匹配某个**词**的开头。
/// 例："chrome" 匹配 "Google Chrome" 的 "Chrome" 词 → 高分（远高于 fuzzy）。
/// 打分：base 4500（比全局 prefix 低 500，因为不是首词），剩余字符越少分越高。
/// 解决 "Google Chrome" 搜 "chrome" 时 prefix 失败、只能走低分 fuzzy 的问题。
pub fn word_prefix_match(query: &str, target: &str) -> Option<Score> {
    let query_lower = query.to_lowercase();
    // 按非字母数字字符分词（空格 / 连字符 / 斜杠 / 点等）
    let best = target
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter_map(|word| {
            let word_lower = word.to_lowercase();
            if word_lower.starts_with(&query_lower) {
                let remaining = word.chars().count().saturating_sub(query.chars().count());
                Some(4500 - remaining as Score)
            } else {
                None
            }
        })
        .max()?;
    Some(best)
}

/// 模糊匹配：nucleo-matcher。Matcher 经 thread_local FUZZY_MATCHER 复用。
pub fn fuzzy_match(query: &str, target: &str) -> Option<Score> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let target_chars: Vec<char> = target.chars().collect();
    let target_str = Utf32Str::Unicode(&target_chars);
    FUZZY_MATCHER.with(|m| pattern.score(target_str, &mut m.borrow_mut()).map(|s| s as Score))
}

/// 拼音首字母匹配：query 全 ASCII 时，匹配 target 的拼音首字母。
/// 优先级接近前缀匹配——拼音首字母匹配是强意图信号。
pub fn pinyin_match(query: &str, target: &str) -> Option<Score> {
    if !query.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let initials = pinyin_initials(target);
    if initials.is_empty() {
        return None;
    }
    // 复用单次小写转换（L4——原两次 query.to_lowercase() 各分配一次 String）
    let query_lower = query.to_lowercase();
    if initials.starts_with(&query_lower) {
        let remaining = initials.chars().count().saturating_sub(query.chars().count());
        // 完全匹配（remaining=0）= 4000，每多一个剩余字符 -1
        // 远高于 fuzzy match，确保拼音匹配的应用排在最前
        Some(4000 - remaining as Score)
    } else if initials.contains(&query_lower) {
        Some(1000)
    } else {
        None
    }
}

/// 取中文文本的拼音首字母。用 pinyin crate（覆盖全部 CJK 汉字）。
///
/// **限制（L5）**：`first_letter()` 只返回多音字的常用读音首字母。
/// 多音字（行=xing/hang、重=zhong/chong、长=chang/zhang）的另一读音不会被生成，
/// 按非主流读音首字母搜索时召回率低。pinyin crate 的 `to_pinyin()` 返回多读音迭代，
/// 但展开所有组合会导致首字母序列组合爆炸（N 个多音字 → 2^N 组合），
/// 不值得为低频场景引入。保持单读音 + 文档标注此限制。
fn pinyin_initials(text: &str) -> String {
    use pinyin::ToPinyin;
    let mut result = String::new();
    for ch in text.chars() {
        if let Some(py) = ch.to_pinyin() {
            result.push_str(py.first_letter());
        } else if ch.is_ascii_alphabetic() {
            result.push(ch.to_ascii_lowercase());
        }
    }
    result
}

/// 综合匹配：按优先级尝试 exact > prefix > word-prefix > pinyin > fuzzy，取最高分。
/// word-prefix 介于 prefix 和 pinyin 之间——"Google Chrome" 搜 "chrome" 走 word-prefix
/// 拿 ~4495 分（远高于书签的 fuzzy 几百分），应用稳排书签前。
pub fn match_score(query: &str, target: &str) -> Option<Score> {
    if query.is_empty() {
        return None;
    }
    exact_match(query, target)
        .or_else(|| prefix_match(query, target))
        .or_else(|| word_prefix_match(query, target))
        .or_else(|| pinyin_match(query, target))
        .or_else(|| fuzzy_match(query, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_case_insensitive() {
        assert_eq!(exact_match("chrome", "Chrome"), Some(10000));
        assert_eq!(exact_match("CHROME", "Chrome"), Some(10000));
        assert_eq!(exact_match("chrom", "Chrome"), None);
    }

    #[test]
    fn prefix_match_basic() {
        assert!(prefix_match("chr", "Chrome").is_some());
        assert!(prefix_match("xyz", "Chrome").is_none());
    }

    #[test]
    fn prefix_match_shorter_target_scores_higher() {
        // 短目标（接近精确）得分高于长目标
        let short = prefix_match("chr", "Chrome").unwrap();
        let long = prefix_match("chr", "Chrome Apps Helper").unwrap();
        assert!(short > long, "Chrome ({}) should outrank Chrome Apps Helper ({})", short, long);
    }

    #[test]
    fn fuzzy_match_basic() {
        assert!(fuzzy_match("chr", "Chrome").is_some());
        assert!(fuzzy_match("cme", "Chrome").is_some());
        assert!(fuzzy_match("xyz", "Chrome").is_none());
    }

    #[test]
    fn pinyin_match_chinese_menu() {
        assert_eq!(pinyin_match("fy", "翻译"), Some(4000));
        assert_eq!(pinyin_match("rs", "润色"), Some(4000));
        assert_eq!(pinyin_match("xyz", "翻译"), None);
    }

    #[test]
    fn pinyin_match_shorter_initials_scores_higher() {
        // 短首字母项（翻译=fy）得分高于长首字母项（复制路径=fzlj）
        let short = pinyin_match("f", "翻译").unwrap();
        let long = pinyin_match("f", "复制路径").unwrap();
        assert!(short > long, "翻译 ({}) should outrank 复制路径 ({})", short, long);
    }

    #[test]
    fn match_score_priority() {
        // exact > prefix > word-prefix > pinyin > fuzzy
        let exact = match_score("chrome", "Chrome").unwrap();
        let prefix = match_score("chr", "Chrome").unwrap();
        let fuzzy = match_score("cme", "Chrome").unwrap();
        assert!(exact > prefix);
        assert!(prefix > fuzzy);
    }

    #[test]
    fn word_prefix_match_non_first_word() {
        // "chrome" 匹配 "Google Chrome" 的第二个词
        let score = word_prefix_match("chrome", "Google Chrome");
        assert!(score.is_some(), "应匹配 Google Chrome 的 Chrome 词");
        // 分数应远高于 fuzzy（~几百）
        assert!(score.unwrap() > 4000, "word-prefix 应 > 4000");
    }

    #[test]
    fn word_prefix_match_partial_word() {
        // "chr" 匹配 "Google Chrome" 的 "Chrome" 词前缀
        assert!(word_prefix_match("chr", "Google Chrome").is_some());
        assert!(word_prefix_match("chr", "Google Chrome Helper").is_some());
    }

    #[test]
    fn word_prefix_match_rejects_non_prefix() {
        // "hrome"（缺首字母 c）不是任何词的前缀
        assert!(word_prefix_match("hrome", "Google Chrome").is_none());
        // "xyz" 不匹配
        assert!(word_prefix_match("xyz", "Google Chrome").is_none());
    }

    #[test]
    fn word_prefix_splits_on_punctuation() {
        // 按 - / . 分词
        assert!(word_prefix_match("app", "my-app").is_some());
        assert!(word_prefix_match("bar", "foo/bar").is_some());
        assert!(word_prefix_match("com", "example.com").is_some());
    }

    #[test]
    fn match_score_google_chrome_chrome_outranks_bookmark_fuzzy() {
        // 核心场景：搜 "chrome"，"Google Chrome" 应用应远高于书签的 fuzzy 匹配
        let app_score = match_score("chrome", "Google Chrome").unwrap() + 2000; // app +2000 权重
        let bookmark_fuzzy = fuzzy_match("chrome", "Chrome Extension Dev").unwrap_or(0);
        assert!(app_score > bookmark_fuzzy,
            "Google Chrome 应用 ({}) 应高于书签 fuzzy ({})，否则应用被书签挤出 top-10", app_score, bookmark_fuzzy);
    }
}
