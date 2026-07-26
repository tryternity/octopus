//! 中文 passphrase：双字词组合，可加数字、符号。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;
use zeroize::Zeroizing;

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
    // words 是 &'static str（字面量引用，无 heap 明文）——不需 zeroize
    let words: Vec<&str> = (0..cfg.word_count)
        .map(|_| *ZH_WORDLIST_4096.choose(&mut rng).unwrap())
        .collect();

    // #8 修复：result 中间拼接材料用 Zeroizing——离开作用域自动清零
    let mut result: Zeroizing<String> = Zeroizing::new(words.join(&cfg.separator));

    if cfg.include_number {
        // OBS-PASSPHRASE-ZH-NUMBER-ASYMMETRY 修复（2026-07-27，第五十九轮）：
        // 之前 format!("{}{}", result, n) 无视 separator——separator="-" 时英文给
        // "Word1-Word2-3"（数字独立段），中文给 "词1-词2-词35"（数字粘最后词），
        // 行为不一致。现与 passphrase_en:62-71 对齐：separator 非空时数字前加分隔符。
        // 默认 separator="" 时行为不变（数字仍粘末尾）。
        let n: u32 = OsRng.gen_range(0..=9);
        let sep = if cfg.separator.is_empty() { "" } else { cfg.separator.as_str() };
        *result = format!("{}{}{}", result.as_str(), sep, n);
    }
    if cfg.include_symbol {
        // 同 include_number：与英文对称（虽然英文目前无 include_symbol，但保持
        // separator 语义一致，未来扩展英文 symbol 时可直接对齐）
        let symbols = ['!', '@', '#', '$', '%', '&', '*'];
        let s = symbols.choose(&mut rng).unwrap();
        let sep = if cfg.separator.is_empty() { "" } else { cfg.separator.as_str() };
        *result = format!("{}{}{}", result.as_str(), sep, s);
    }
    // 复制一份返回（Tauri IPC 需要 String；Zeroizing 在此清零 result 的 heap）
    Ok(result.as_str().to_string())
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

    /// OBS-PASSPHRASE-ZH-NUMBER-ASYMMETRY 回归守护（2026-07-27，第五十九轮）：
    /// 中文 separator 非空时，数字/符号前应加分隔符（与 passphrase_en:62-71 对称）。
    ///
    /// 之前 bug：format!("{}{}", result, n) 无视 separator——separator="-" + include_number
    /// 给 "词1-词2-词3-词45"（数字粘最后词），而英文给 "Word1-Word2-3"（数字独立段）。
    /// 修复后中文也应给 "词1-词2-词3-词4-5"（数字前有 separator）。
    #[test]
    fn test_separator_respected_for_number_and_symbol() {
        // include_number + separator="-" → 数字前应有 -
        let cfg = PassphraseZhConfig {
            separator: "-".into(),
            include_number: true,
            include_symbol: false,
            ..Default::default()
        };
        let s = generate(&cfg).unwrap();
        // 拆分后最后一段应是纯数字（数字独立段，非粘在词后）
        let parts: Vec<&str> = s.split('-').collect();
        let last = parts.last().unwrap();
        assert!(
            last.chars().all(|c| c.is_ascii_digit()),
            "separator='-' + include_number：最后一段应是纯数字（独立段），实际 {}（完整 {}）",
            last,
            s
        );

        // include_symbol + separator="-" → 符号前应有 -
        let cfg_sym = PassphraseZhConfig {
            separator: "-".into(),
            include_number: false,
            include_symbol: true,
            ..Default::default()
        };
        let s_sym = generate(&cfg_sym).unwrap();
        let parts_sym: Vec<&str> = s_sym.split('-').collect();
        let last_sym = parts_sym.last().unwrap();
        assert!(
            last_sym.chars().all(|c| ['!', '@', '#', '$', '%', '&', '*'].contains(&c)),
            "separator='-' + include_symbol：最后一段应是纯符号（独立段），实际 {}（完整 {}）",
            last_sym,
            s_sym
        );
    }

    /// 默认 separator（空字符串）时，数字/符号仍粘末尾——行为不变（向后兼容）。
    #[test]
    fn test_default_empty_separator_number_still_appends() {
        let cfg = PassphraseZhConfig::default();
        let s = generate(&cfg).unwrap();
        // 默认 separator 空 + include_number=true → 末尾是数字，但数字粘最后词后
        // （split('-') 无效，整个字符串是一段，末尾字符是数字）
        assert!(
            s.ends_with(|c: char| c.is_ascii_digit()),
            "默认空 separator：末尾应是数字（粘最后词后），实际 {}",
            s
        );
        // 不应含 '-'（默认 separator 空，数字前不加分隔符）
        assert!(!s.contains('-'), "默认空 separator 不应含 '-'：{}", s);
    }
}
