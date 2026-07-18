//! 英文 passphrase：EFF 7776 词，可加数字、大写、分隔符。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

use super::eff_wordlist::EFF_WORDLIST;
use super::PassphraseEnConfig;

pub fn generate(cfg: &PassphraseEnConfig) -> Result<String> {
    ensure!(
        cfg.word_count >= 3,
        "英文短语词数必须 ≥ 3（当前 {}）",
        cfg.word_count
    );
    ensure!(
        cfg.word_count <= 10,
        "英文短语词数必须 ≤ 10（当前 {}）",
        cfg.word_count
    );

    let mut rng = OsRng;
    let words: Vec<String> = (0..cfg.word_count)
        .map(|_| EFF_WORDLIST.choose(&mut rng).unwrap().to_string())
        // EFF 列表中有 4 个带连字符的词（yo-yo / drop-down / felt-tip / t-shirt）。
        // 由于 separator 默认 '-'，且 capitalize 会把首个字母大写，连字符词会
        // 让生成的 passphrase 在 split('-') 后无法逐词校验。这里把连字符去掉
        // （yo-yo → yoyo），既保持源词熵不变，又让默认 '-' 分隔符语义清晰。
        .map(|w| w.replace('-', ""))
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
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count() {
        let cfg = PassphraseEnConfig::default();
        for _ in 0..50 {
            let s = generate(&cfg).unwrap();
            // 默认 3 词 + 1 数字（带 -）→ "word1-word2-word3-5" 共 4 段
            let parts: Vec<&str> = s.split('-').collect();
            assert_eq!(parts.len(), 4, "实际: {}", s);
        }
    }

    #[test]
    fn test_capitalize() {
        let cfg = PassphraseEnConfig::default();
        let s = generate(&cfg).unwrap();
        // 至少一个词首字母大写
        assert!(s.chars().any(|c| c.is_uppercase()), "应有大写: {}", s);
    }

    #[test]
    fn test_all_words_from_eff_list() {
        let cfg = PassphraseEnConfig {
            include_number: false,
            ..Default::default()
        };
        // generate() 会把所选 EFF 词的连字符去掉（yo-yo → yoyo），
        // 因此校验时也要用去连字符的 EFF 集合比对。
        let eff_deshyd: Vec<String> = EFF_WORDLIST
            .iter()
            .map(|w| w.replace('-', ""))
            .collect();
        for _ in 0..50 {
            let s = generate(&cfg).unwrap();
            for word in s.split('-') {
                let lower = word.to_lowercase();
                assert!(
                    eff_deshyd.iter().any(|w| w == &lower),
                    "词 {} 不在 EFF 列表（generated: {}）",
                    word,
                    s
                );
            }
        }
    }

    #[test]
    fn test_too_few_words_errors() {
        let cfg = PassphraseEnConfig {
            word_count: 2,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_too_many_words_errors() {
        let cfg = PassphraseEnConfig {
            word_count: 11,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_word_count_bounds_ok() {
        let cfg_min = PassphraseEnConfig {
            word_count: 3,
            ..Default::default()
        };
        assert!(generate(&cfg_min).is_ok());
        let cfg_max = PassphraseEnConfig {
            word_count: 10,
            ..Default::default()
        };
        assert!(generate(&cfg_max).is_ok());
    }
}
