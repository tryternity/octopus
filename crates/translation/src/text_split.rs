//! 文本切分工具——m2m100 / opus_mt 共用。
//!
//! 抽取自 m2m100.rs / opus_mt.rs 各自重复的 `split_sentences` + `is_sentence_end`
//!（两份逐字相同的实现）。长文本翻译按句子边界分段，避免硬切破坏语义。

/// 按句子边界切分文本。支持 CJK 标点（。！？）和 Latin 标点（.!?）+ 换行。
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if is_sentence_end(ch) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }

    if sentences.is_empty() {
        vec![text.to_string()]
    } else {
        sentences
    }
}

/// 判断字符是否为句子结束标点（CJK + Latin + 换行）。
pub(crate) fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '．' | '\n' | '.' | '!' | '?' | ';' | '；')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_by_cjk_punctuation() {
        assert_eq!(split_sentences("你好。世界！"), vec!["你好。", "世界！"]);
    }

    #[test]
    fn split_by_latin_punctuation() {
        assert_eq!(split_sentences("Hello. World!"), vec!["Hello.", " World!"]);
    }

    #[test]
    fn split_by_newline() {
        assert_eq!(split_sentences("line1\nline2\n"), vec!["line1\n", "line2\n"]);
    }

    #[test]
    fn no_punctuation_returns_whole_text() {
        // 无标点 → 整段作为一个 sentence
        assert_eq!(split_sentences("无标点文本"), vec!["无标点文本"]);
    }

    #[test]
    fn empty_text_returns_empty_vec() {
        // 空串 → sentences 为空 → fallback 返回 [""]
        //（与原 m2m100/opus_mt 实现行为一致：sentences.is_empty() 时 vec![text.to_string()]）
        let result = split_sentences("");
        assert_eq!(result, vec![""]);
    }
}
