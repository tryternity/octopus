//! 中文 passphrase：双字词组合，可加数字、符号。

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

use super::zh_wordlist_4096::ZH_WORDLIST_4096;
use super::PassphraseZhConfig;

pub fn generate(cfg: &PassphraseZhConfig) -> String {
    assert!(cfg.word_count >= 3, "word_count 必须 >= 3");
    assert!(cfg.word_count <= 8, "word_count 必须 <= 8");

    let mut rng = OsRng;
    let words: Vec<&str> = (0..cfg.word_count)
        .map(|_| *ZH_WORDLIST_4096.choose(&mut rng).unwrap())
        .collect();

    let mut result = words.join(&cfg.separator);

    if cfg.include_number {
        let n: u32 = OsRng.gen_range(0..=9);
        result = format!("{}{}", result, n);
    }
    if cfg.include_symbol {
        let symbols = ['!', '@', '#', '$', '%', '&', '*'];
        let s = symbols.choose(&mut rng).unwrap();
        result = format!("{}{}", result, s);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count_default() {
        let cfg = PassphraseZhConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg);
            // 4 词 = 8 字符 + 1 数字 = 9 字符
            let chars: Vec<char> = s.chars().filter(|c| !c.is_ascii_digit()).collect();
            assert_eq!(chars.len(), 8, "应为 4 个双字词 (8 字符)，实际: {}", s);
        }
    }

    #[test]
    fn test_no_separator() {
        let cfg = PassphraseZhConfig::default();
        let s = generate(&cfg);
        // 默认 separator 是空字符串，不应有 - 或空格
        assert!(!s.contains('-'));
        assert!(!s.contains(' '));
    }

    #[test]
    fn test_with_symbol() {
        let cfg = PassphraseZhConfig {
            include_symbol: true,
            include_number: false,
            ..Default::default()
        };
        let s = generate(&cfg);
        assert!(
            s.ends_with(['!', '@', '#', '$', '%', '&', '*']),
            "应以符号结尾: {}",
            s
        );
    }

    #[test]
    #[ignore]
    fn test_wordlist_size_4096_after_completion() {
        // TODO: 词表补全到 4096 后启用此测试
        assert_eq!(
            ZH_WORDLIST_4096.len(),
            4096,
            "当前词表大小: {}",
            ZH_WORDLIST_4096.len()
        );
    }
}
