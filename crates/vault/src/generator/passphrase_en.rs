//! 英文 passphrase：EFF 7776 词，可加数字、大写、分隔符。

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

use super::eff_wordlist::EFF_WORDLIST;
use super::PassphraseEnConfig;

pub fn generate(cfg: &PassphraseEnConfig) -> String {
    assert!(cfg.word_count >= 3, "word_count 必须 >= 3");
    assert!(cfg.word_count <= 10, "word_count 必须 <= 10");

    let mut rng = OsRng;
    let words: Vec<String> = (0..cfg.word_count)
        .map(|_| EFF_WORDLIST.choose(&mut rng).unwrap().to_string())
        .map(|w| {
            if cfg.capitalize {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            } else {
                w
            }
        })
        .collect();

    let mut result = words.join(&cfg.separator);
    if cfg.include_number {
        let n: u32 = rng.gen_range(0..=9);
        result = format!(
            "{}{}{}",
            result,
            if cfg.separator.is_empty() {
                ""
            } else {
                cfg.separator.as_str()
            },
            n
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count() {
        let cfg = PassphraseEnConfig::default();
        for _ in 0..50 {
            let s = generate(&cfg);
            // 默认 3 词 + 1 数字（带 -）→ "word1-word2-word3-5" 共 4 段
            let parts: Vec<&str> = s.split('-').collect();
            assert_eq!(parts.len(), 4, "实际: {}", s);
        }
    }

    #[test]
    fn test_capitalize() {
        let cfg = PassphraseEnConfig::default();
        let s = generate(&cfg);
        // 至少一个词首字母大写
        assert!(s.chars().any(|c| c.is_uppercase()), "应有大写: {}", s);
    }

    #[test]
    fn test_all_words_from_eff_list() {
        let cfg = PassphraseEnConfig {
            include_number: false,
            ..Default::default()
        };
        for _ in 0..50 {
            let s = generate(&cfg);
            for word in s.split('-') {
                let lower = word.to_lowercase();
                assert!(
                    EFF_WORDLIST.iter().any(|w| *w == lower),
                    "词 {} 不在 EFF 列表",
                    word
                );
            }
        }
    }
}
