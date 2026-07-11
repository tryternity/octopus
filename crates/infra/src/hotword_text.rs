//! 热词文本工具：拼音首字母 + 写入规范化（切词→去重→排序→拼接）。
//! 纯函数、无 DB、无全局状态——供 db.rs（迁移/写 words_text）与 asr-local/desktop 复用。

use pinyin::ToPinyin;

/// 词 → 拼音首字母串（大写，非汉字跳过）。如「八爪鱼」→`BZY`、「浮窗」→`FC`。
pub fn pinyin_initials(word: &str) -> String {
    word.chars()
        .filter_map(|c| c.to_pinyin().and_then(|p| p.plain().chars().next()))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 写入规范化：任意空白切词 → 去重 → 按 `(pinyin_initials, 词文本)` 升序 → 空格拼接。
/// `hotword_sets.words_text` 始终经此函数，保持有序、去重的规范形态。
pub fn normalize_words_text(words: &str) -> String {
    let mut v: Vec<String> = words.split_whitespace().map(|s| s.to_string()).collect();
    v.sort_by(|a, b| {
        pinyin_initials(a)
            .cmp(&pinyin_initials(b))
            .then_with(|| a.cmp(b))
    });
    v.dedup(); // 排序后去相邻重复
    v.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_initials_basic() {
        assert_eq!(pinyin_initials("八爪鱼"), "BZY");
        assert_eq!(pinyin_initials("浮窗"), "FC");
        assert_eq!(pinyin_initials("热词"), "RC");
        assert_eq!(pinyin_initials("AI助手"), "ZS"); // 非汉字跳过
        assert_eq!(pinyin_initials(""), "");
    }

    #[test]
    fn normalize_splits_any_whitespace() {
        // 空格 / 换行 / 制表符 都切
        assert_eq!(normalize_words_text("八爪鱼 吴大锐\n浮窗"), "八爪鱼 浮窗 吴大锐");
    }

    #[test]
    fn normalize_dedupes() {
        assert_eq!(normalize_words_text("八爪鱼 八爪鱼 吴大锐"), "八爪鱼 吴大锐");
    }

    #[test]
    fn normalize_sorts_by_initials_then_text() {
        // 浮窗(FC) 热词(RC) 八爪鱼(BZY 按 B 排前)
        // B < F < R → 八爪鱼 浮窗 热词
        assert_eq!(normalize_words_text("热词 浮窗 八爪鱼"), "八爪鱼 浮窗 热词");
    }

    #[test]
    fn normalize_empty() {
        assert_eq!(normalize_words_text(""), "");
        assert_eq!(normalize_words_text("   \n\t  "), "");
    }

    #[test]
    fn normalize_keeps_non_hanzi() {
        // 含非汉字的词保留（HotwordIndex 会自行跳过；normalize 不删）。
        // AI助手 首字母 = ZS（助=Z、手=S），八爪鱼 = BZY；B < Z → 八爪鱼 在前。
        assert_eq!(normalize_words_text("AI助手 八爪鱼"), "八爪鱼 AI助手");
    }
}
