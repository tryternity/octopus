//! 随机字符密码：保证每种启用字符类型至少出现 1 次。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;

use super::RandomConfig;

const UPPER: &[&str] = &[
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
const LOWER: &[&str] = &[
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];
const DIGITS: &[&str] = &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const SYMBOLS: &[&str] = &[
    "!", "@", "#", "$", "%", "^", "&", "*", "(", ")", "-", "_", "=", "+", "[", "]", "{", "}", "<",
    ">", "?",
];
const AMBIGUOUS: &[char] = &['l', '1', 'I', 'O', '0', '|', '`', '\'', '"'];

fn build_charset(cfg: &RandomConfig) -> Vec<char> {
    let mut s: String = String::new();
    if cfg.uppercase {
        s.push_str(UPPER.concat().as_str());
    }
    if cfg.lowercase {
        s.push_str(LOWER.concat().as_str());
    }
    if cfg.numbers {
        s.push_str(DIGITS.concat().as_str());
    }
    if cfg.symbols {
        s.push_str(SYMBOLS.concat().as_str());
    }
    if cfg.avoid_ambiguous {
        s = s.chars().filter(|c| !AMBIGUOUS.contains(c)).collect();
    }
    s.chars().collect()
}

pub fn generate(cfg: &RandomConfig) -> Result<String> {
    ensure!(
        cfg.length >= 5,
        "密码长度必须 ≥ 5（当前 {}）",
        cfg.length
    );
    ensure!(
        cfg.length <= 128,
        "密码长度必须 ≤ 128（当前 {}）",
        cfg.length
    );

    let mut rng = OsRng;
    let mut result: Vec<char> = Vec::with_capacity(cfg.length as usize);

    // 强制每种启用类型至少 1 个
    if cfg.uppercase && !cfg.avoid_ambiguous {
        result.extend(UPPER.choose(&mut rng).unwrap().chars());
    } else if cfg.uppercase {
        let filtered: Vec<&str> = UPPER
            .iter()
            .filter(|s| !AMBIGUOUS.contains(&s.chars().next().unwrap()))
            .copied()
            .collect();
        if let Some(c) = filtered.choose(&mut rng) {
            result.extend(c.chars());
        }
    }
    if cfg.lowercase {
        let pool: Vec<&str> = LOWER
            .iter()
            .filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap()))
            .copied()
            .collect();
        if let Some(c) = pool.choose(&mut rng) {
            result.extend(c.chars());
        }
    }
    if cfg.numbers {
        let pool: Vec<&str> = DIGITS
            .iter()
            .filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap()))
            .copied()
            .collect();
        if let Some(c) = pool.choose(&mut rng) {
            result.extend(c.chars());
        }
    }
    if cfg.symbols {
        let pool: Vec<&str> = SYMBOLS
            .iter()
            .filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap()))
            .copied()
            .collect();
        if let Some(c) = pool.choose(&mut rng) {
            result.extend(c.chars());
        }
    }

    let charset = build_charset(cfg);
    ensure!(
        !charset.is_empty(),
        "至少需要启用一种字符类型（大写/小写/数字/符号）"
    );
    while (result.len() as u32) < cfg.length {
        if let Some(c) = charset.choose(&mut rng) {
            result.push(*c);
        }
    }

    result.shuffle(&mut rng);
    Ok(result.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_length_within_bounds() {
        let cfg = RandomConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg).unwrap();
            assert_eq!(s.len(), 16);
        }
    }

    #[test]
    fn test_avoid_ambiguous_default() {
        let cfg = RandomConfig::default();
        for _ in 0..200 {
            let s = generate(&cfg).unwrap();
            assert!(!s.contains('l'), "不应含 l: {}", s);
            assert!(!s.contains('1'), "不应含 1: {}", s);
            assert!(!s.contains('O'), "不应含 O: {}", s);
            assert!(!s.contains('0'), "不应含 0: {}", s);
        }
    }

    #[test]
    fn test_each_type_present() {
        let cfg = RandomConfig {
            length: 30,
            uppercase: true,
            lowercase: true,
            numbers: true,
            symbols: true,
            avoid_ambiguous: false,
        };
        for _ in 0..100 {
            let s = generate(&cfg).unwrap();
            assert!(s.chars().any(|c| c.is_uppercase()), "缺大写: {}", s);
            assert!(s.chars().any(|c| c.is_lowercase()), "缺小写: {}", s);
            assert!(s.chars().any(|c| c.is_ascii_digit()), "缺数字: {}", s);
            assert!(
                s.chars()
                    .any(|c| SYMBOLS.iter().any(|sym| sym.chars().any(|sc| sc == c))),
                "缺符号: {}",
                s
            );
        }
    }

    #[test]
    fn test_only_numbers() {
        let cfg = RandomConfig {
            length: 10,
            uppercase: false,
            lowercase: false,
            numbers: true,
            symbols: false,
            avoid_ambiguous: false,
        };
        let s = generate(&cfg).unwrap();
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_too_short_errors() {
        let cfg = RandomConfig {
            length: 4,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_too_long_errors() {
        let cfg = RandomConfig {
            length: 129,
            ..Default::default()
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_no_charset_selected_errors() {
        let cfg = RandomConfig {
            length: 16,
            uppercase: false,
            lowercase: false,
            numbers: false,
            symbols: false,
            avoid_ambiguous: false,
        };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_length_bounds_ok() {
        // 边界值：5 与 128 都应成功
        let cfg_min = RandomConfig {
            length: 5,
            ..Default::default()
        };
        assert!(generate(&cfg_min).is_ok());
        let cfg_max = RandomConfig {
            length: 128,
            ..Default::default()
        };
        assert!(generate(&cfg_max).is_ok());
    }
}
