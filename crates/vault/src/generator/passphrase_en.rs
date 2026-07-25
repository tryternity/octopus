//! 英文 passphrase：EFF 7776 词，可加数字、大写、分隔符。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;
use zeroize::Zeroizing;

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
    // #8 修复：words 中间材料用 Zeroizing——选词过程产生的明文 passphrase 组成部分
    // 离开作用域时自动清零（防止生成失败/异常时 heap dump 恢复中间状态）
    let words: Zeroizing<Vec<String>> = Zeroizing::new(
        (0..cfg.word_count)
            .map(|_| EFF_WORDLIST.choose(&mut rng).unwrap().to_string())
            // EFF 列表中有 4 个带连字符的词（yo-yo / drop-down / felt-tip / t-shirt）。
            // 由于 separator 默认 '-'，且 capitalize 会把首个字母大写，连字符词会
            // 让生成的 passphrase 在 split('-') 后无法逐词校验。这里把连字符去掉
            // （yo-yo → yoyo），让默认 '-' 分隔符语义清晰。
            //
            // G-YOYO-COLLIDE（2026-07-25）：去连字符后 yo-yo→yoyo 与已有 yoyo 碰撞，
            // 实际唯一输出版 7775（少 1）。熵损 = log2(7776/7775) ≈ 0.000186 bit/词，
            // 3-10 词 passphrase 总熵损 < 0.002 bit，可忽略。非 octopus 引入——EFF
            // 官方大词表本身同时含两者。守护测试 test_eff_wordlist_no_dedash_collision
            // 锁住此已知碰撞，防止未来词表改动引入新的去连字符碰撞。
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
            .collect(),
    );

    // G-EN-RESULT-NO-ZEROIZE 修复（2026-07-25）：result 用 Zeroizing<String> 包裹，
    // 与 random（Zeroizing<Vec<char>>）/ passphrase_zh / pin 一致——之前 en 独漏，
    // words（中间词数组）包了 Zeroizing 但 join 产出的最终明文副本没包。
    // 模式照 passphrase_zh :31/:35/:43。
    let mut result: Zeroizing<String> = Zeroizing::new(words.join(&cfg.separator));
    if cfg.include_number {
        let n: u32 = rng.gen_range(0..=9);
        *result = format!(
            "{}{}{}",
            result.as_str(),
            if cfg.separator.is_empty() {
                ""
            } else {
                cfg.separator.as_str()
            },
            n
        );
    }
    // 复制一份返回（Tauri IPC 需要 String；Zeroizing 在此清零 result 的 heap）
    Ok(result.as_str().to_string())
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

    /// G-EFF-NOGUARD 守护（2026-07-25）：EFF 词表大小必须恰好 7776。
    ///
    /// 7776 行静态 const，误删几行 / 编辑引入异常，CI 之前无任何守护，静默降熵。
    /// 与 zh_wordlist_4096 的 test_wordlist_size_4096_after_completion 对称。
    #[test]
    fn test_eff_wordlist_size_7776() {
        assert_eq!(
            EFF_WORDLIST.len(),
            7776,
            "EFF 词表必须恰好 7776 词（当前 {}），误删/误增会静默降熵",
            EFF_WORDLIST.len()
        );
    }

    /// G-EFF-NOGUARD 守护：EFF 词表无重复词。
    #[test]
    fn test_eff_wordlist_no_duplicates() {
        let mut sorted = EFF_WORDLIST.to_vec();
        sorted.sort();
        let dups: Vec<&str> = sorted
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(
            dups.is_empty(),
            "EFF 词表不应有重复词（原始词，未去连字符）：{:?}",
            dups
        );
    }

    /// G-EFF-NOGUARD + G-YOYO-COLLIDE 守护：去连字符后无新增碰撞（除已知 yo-yo/yoyo）。
    ///
    /// passphrase 生成会 `w.replace('-', "")`——若去连字符后两词变成相同，实际唯一
    /// 输出版减少，静默降熵。EFF 官方词表已知 yo-yo→yoyo 与 yoyo 碰撞（熵损可忽略，
    /// 见生成函数注释）。此测试锁住「仅此一对已知碰撞」，防止未来词表改动引入更多。
    #[test]
    fn test_eff_wordlist_no_dedash_collision() {
        use std::collections::HashMap;
        // key = 去连字符后的词（owned），value = 原始词（记录首个出现的）
        let mut seen: HashMap<String, &str> = HashMap::new();
        let mut collisions: Vec<(&str, &str)> = Vec::new();
        for &word in EFF_WORDLIST.iter() {
            let dedashed = word.replace('-', "");
            if let Some(&prev) = seen.get(&dedashed) {
                // 已知碰撞：yo-yo / yoyo（EFF 官方词表固有，熵损可忽略）
                let is_known =
                    (prev == "yo-yo" && word == "yoyo") || (prev == "yoyo" && word == "yo-yo");
                if !is_known {
                    collisions.push((prev, word));
                }
            } else {
                seen.insert(dedashed, word);
            }
        }
        assert!(
            collisions.is_empty(),
            "去连字符后出现非已知碰撞（会静默降熵）：{:?}",
            collisions
        );
    }
}
