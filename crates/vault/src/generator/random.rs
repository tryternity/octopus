//! 随机字符密码：保证每种启用字符类型至少出现 1 次。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use zeroize::Zeroizing;

use super::RandomConfig;

// R8 修复（2026-07-25）：字符集改 &[char]——之前 &[&str] 每次 build_charset 都
// concat() 4 次堆分配拼 String 再 .chars()。字符集静态已知， &[char] 零分配。
const UPPER: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];
const LOWER: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z',
];
const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
const SYMBOLS: &[char] = &[
    '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '-', '_', '=', '+', '[', ']', '{', '}', '<',
    '>', '?',
];
// R-AMBIGUOUS-DEAD 修复（2026-07-25）：删 4 个死字符 |/`/'/"——它们不在
// UPPER/LOWER/DIGITS/SYMBOLS 任一字符集，build_charset 的 retain 和强制阶段
// filter 对它们永远是 no-op。保留 5 个真正有效的（l/1/I/O/0 在字符集内会被过滤）。
// 之前作者意图排除易混淆字符但忘了把它们纳入 SYMBOLS——当前无害（no-op），
// 删除让 AMBIGUOUS 语义准确（只列实际参与生成且需过滤的字符）。
const AMBIGUOUS: &[char] = &['l', '1', 'I', 'O', '0'];

fn build_charset(cfg: &RandomConfig) -> Vec<char> {
    let mut s: Vec<char> = Vec::new();
    if cfg.uppercase {
        s.extend_from_slice(UPPER);
    }
    if cfg.lowercase {
        s.extend_from_slice(LOWER);
    }
    if cfg.numbers {
        s.extend_from_slice(DIGITS);
    }
    if cfg.symbols {
        s.extend_from_slice(SYMBOLS);
    }
    if cfg.avoid_ambiguous {
        s.retain(|c| !AMBIGUOUS.contains(c));
    }
    s
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
    // #8 修复：中间材料用 Zeroizing——函数返回后自动清零 heap 中的明文密码字符
    // （返回值 String 会进 Tauri IPC → JS heap，Zeroizing 在边界保护意义有限；
    // 但生成过程中的中间材料在生成失败/异常时不应残留 heap，可被 dump 恢复）
    let mut result: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(cfg.length as usize));

    // 强制每种启用类型至少 1 个
    // R8 修复后字符集是 &[char]，直接 choose 拿 char，无需 .chars() 转换。
    // R5 修复：统一用 if let Some，消除 unwrap。
    // R-UPPER-BRANCH-ASYMMETRY 修复（2026-07-25）：uppercase 改统一 filter 写法，
    // 与 lowercase/numbers/symbols 对齐（之前 uppercase 用双分支，其余用统一 filter）。
    if cfg.uppercase {
        let pool: Vec<&char> = UPPER
            .iter()
            .filter(|c| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(c))
            .collect();
        if let Some(&&c) = pool.choose(&mut rng) {
            result.push(c);
        }
    }
    if cfg.lowercase {
        let pool: Vec<&char> = LOWER
            .iter()
            .filter(|c| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(c))
            .collect();
        if let Some(&&c) = pool.choose(&mut rng) {
            result.push(c);
        }
    }
    if cfg.numbers {
        let pool: Vec<&char> = DIGITS
            .iter()
            .filter(|c| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(c))
            .collect();
        if let Some(&&c) = pool.choose(&mut rng) {
            result.push(c);
        }
    }
    if cfg.symbols {
        let pool: Vec<&char> = SYMBOLS
            .iter()
            .filter(|c| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(c))
            .collect();
        if let Some(&&c) = pool.choose(&mut rng) {
            result.push(c);
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
    // 取出内部 Vec<char> 转 String——result 离开作用域时 Zeroizing 已清零 Vec 内存，
    // 但返回的 String 是新的 heap 分配（明文密码进 Tauri IPC → JS heap，由调用方管理）
    Ok(result.iter().collect())
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
                s.chars().any(|c| SYMBOLS.contains(&c)),
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
