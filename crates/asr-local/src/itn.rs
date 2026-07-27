//! ASR 数字 ITN（Inverse Text Normalization）：中文数字→阿拉伯数字。
//!
//! 用 chinese2digits crate 的 `take_number_from_string`，从文本中找中文数字
//! 替换为阿拉伯数字，保留非数字文字。解决 Zipformer/Moonshine/Whisper 等
//! 无内置 ITN 引擎输出「二零二六年七月二十六日」而非「2026年7月26日」的问题。
//!
//! 详见 spec 2026-07-27-asr-itn-design.md。

use chinese2digits::take_number_from_string;

/// 中文数字→阿拉伯数字。始终应用，无开关。
///
/// - `force_simplified=true`：繁体数字字符（貳貳參...）先转简体再识别。
///   注意：只管数字字符的繁简，不替代 hans 模块的全文简繁归一。
/// - `pct=false`：不做百分比转换（「百分之五十」不转「50%」，避免误转）。
///
/// 自带 ITN 的引擎（Qwen3/SenseVoice/Paraformer）文本无中文数字 → no-op。
pub fn normalize(text: &str) -> String {
    take_number_from_string(text, false, true).replaced_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_and_date() {
        assert_eq!(normalize("二零二六年七月二十六日"), "2026年7月26日");
    }

    #[test]
    fn decimal() {
        assert_eq!(normalize("三点五"), "3.5");
    }

    #[test]
    fn count() {
        assert_eq!(normalize("十五个"), "15个");
    }

    #[test]
    fn no_chinese_number_noop() {
        assert_eq!(normalize("今天天气不错"), "今天天气不错");
    }

    #[test]
    fn already_arabic_noop() {
        // Qwen3/SenseVoice 输出已是阿拉伯数字
        assert_eq!(normalize("2026年7月26日"), "2026年7月26日");
    }

    #[test]
    fn english_noop() {
        assert_eq!(normalize("hello world"), "hello world");
    }
}
