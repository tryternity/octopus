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

/// 模糊匹配：nucleo-matcher。Matcher 经 thread_local FUZZY_MATCHER 复用。
pub fn fuzzy_match(query: &str, target: &str) -> Option<Score> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let target_chars: Vec<char> = target.chars().collect();
    let target_str = Utf32Str::Unicode(&target_chars);
    FUZZY_MATCHER.with(|m| pattern.score(target_str, &mut m.borrow_mut()).map(|s| s as Score))
}

/// 拼音首字母匹配：query 全 ASCII 时，匹配 target 的拼音首字母。
/// 简单实现：硬编码常用中文菜单项。
pub fn pinyin_match(query: &str, target: &str) -> Option<Score> {
    if !query.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let initials = pinyin_initials(target);
    if initials.is_empty() {
        return None;
    }
    if initials.starts_with(&query.to_lowercase()) {
        let remaining = initials.chars().count().saturating_sub(query.chars().count());
        Some(3000 - remaining as Score)
    } else if initials.contains(&query.to_lowercase() as &str) {
        Some(1000)
    } else {
        None
    }
}

/// 取中文文本的拼音首字母。硬编码常用菜单项 + 简单 Unicode 范围判断。
fn pinyin_initials(text: &str) -> String {
    // 硬编码常用菜单名
    let known: &[(&str, &str)] = &[
        ("翻译", "fy"), ("搜索", "ss"), ("润色", "rs"), ("摘要", "zy"),
        ("解释", "js"), ("网页", "wy"), ("脚本", "jb"), ("复制路径", "fzlj"),
        ("系统", "xt"), ("设置", "sz"), ("退出", "tc"), ("问豆包", "wdb"),
    ];
    for (name, initials) in known {
        if text.contains(name) {
            return initials.to_string();
        }
    }
    String::new()
}

/// 综合匹配：按优先级尝试 exact > prefix > pinyin > fuzzy，取最高分。
pub fn match_score(query: &str, target: &str) -> Option<Score> {
    if query.is_empty() {
        return None;
    }
    exact_match(query, target)
        .or_else(|| prefix_match(query, target))
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
        assert_eq!(pinyin_match("fy", "翻译"), Some(3000));
        assert_eq!(pinyin_match("rs", "润色"), Some(3000));
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
        // exact > prefix > pinyin > fuzzy
        let exact = match_score("chrome", "Chrome").unwrap();
        let prefix = match_score("chr", "Chrome").unwrap();
        let fuzzy = match_score("cme", "Chrome").unwrap();
        assert!(exact > prefix);
        assert!(prefix > fuzzy);
    }
}
