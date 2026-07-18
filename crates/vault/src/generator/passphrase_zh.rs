//! 中文 passphrase：双字词组合，可加数字、符号。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

use super::zh_wordlist_4096::ZH_WORDLIST_4096;
use super::PassphraseZhConfig;

pub fn generate(cfg: &PassphraseZhConfig) -> Result<String> {
    ensure!(
        cfg.word_count >= 3,
        "中文短语词数必须 ≥ 3（当前 {}）",
        cfg.word_count
    );
    ensure!(
        cfg.word_count <= 8,
        "中文短语词数必须 ≤ 8（当前 {}）",
        cfg.word_count
    );

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
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count_default() {
        let cfg = PassphraseZhConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg).unwrap();
            // 4 词 = 8 字符 + 1 数字 = 9 字符
            let chars: Vec<char> = s.chars().filter(|c| !c.is_ascii_digit()).collect();
            assert_eq!(chars.len(), 8, "应为 4 个双字词 (8 字符)，实际: {}", s);
        }
    }

    #[test]
    fn test_no_separator() {
        let cfg = PassphraseZhConfig::default();
        let s = generate(&cfg).unwrap();
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
        let s = generate(&cfg).unwrap();
        assert!(
            s.ends_with(['!', '@', '#', '$', '%', '&', '*']),
            "应以符号结尾: {}",
            s
        );
    }

    #[test]
    fn test_too_few_words_errors() {
        let cfg = PassphraseZhConfig {
            word_count: 2,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_too_many_words_errors() {
        let cfg = PassphraseZhConfig {
            word_count: 9,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_word_count_bounds_ok() {
        // 边界值：3 与 8 都应成功
        let cfg_min = PassphraseZhConfig {
            word_count: 3,
            ..Default::default()
        };
        assert!(generate(&cfg_min).is_ok());
        let cfg_max = PassphraseZhConfig {
            word_count: 8,
            ..Default::default()
        };
        assert!(generate(&cfg_max).is_ok());
    }

    #[test]
    fn test_wordlist_size_4096_after_completion() {
        assert_eq!(
            ZH_WORDLIST_4096.len(),
            4096,
            "当前词表大小: {}",
            ZH_WORDLIST_4096.len()
        );
    }

    /// 词表必须无重复——早期 100 词版本曾有 '现在' 重复（progress.md 记录），
    /// 扩到 4096 时已清理；此测试锁死该不变量防回归。
    /// 重复会降低实际熵（用户以为 48 bit，实际可能更低）+ 让生成结果偶尔显得「怪」。
    #[test]
    fn test_wordlist_no_duplicates() {
        let mut sorted: Vec<&str> = ZH_WORDLIST_4096.to_vec();
        sorted.sort_unstable();
        let total = sorted.len();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            total,
            "词表存在重复：原 {} 词去重后 {} 词",
            total,
            sorted.len()
        );
    }

    /// 每个词必须恰好 2 个 CJK 字符（U+4E00..U+9FA5）——
    /// 词表面板提示「双字词」，违反会让用户困惑 + 影响视觉一致性。
    #[test]
    fn test_wordlist_all_two_cjk_chars() {
        for (i, word) in ZH_WORDLIST_4096.iter().enumerate() {
            assert_eq!(
                word.chars().count(),
                2,
                "索引 {} 的词 {:?} 不是 2 字符",
                i,
                word
            );
            for c in word.chars() {
                let cp = c as u32;
                assert!(
                    (0x4E00..=0x9FA5).contains(&cp),
                    "词 {:?} 含非 CJK 字符 U+{:04X}",
                    word,
                    cp
                );
            }
        }
    }
}
