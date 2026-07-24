//! zxcvbn 密码强度评估。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PasswordStrength {
    /// 0-4（zxcvbn 评分）
    pub score: u8,
    pub entropy_bits: f64,
    pub warning: Option<String>,
    pub suggestions: Vec<String>,
}

/// 评估密码强度。
///
/// 注意：zxcvbn 3.1.1 的 `zxcvbn()` 直接返回 `Entropy`（非 `Result`），
/// 空密码等极端输入走内部早返，返回 `Score::Zero`、`guesses_log10 = NEG_INFINITY`。
/// 我们对非有限的 log10 兜底为 0，确保不出现 NaN/-inf。
///
/// 8.5 + M1 + N1 修复（2026-07-24）：对超长密码（> 1KB，如粘贴几 KB 文本）短路——
/// zxcvbn 对超长输入有 O(n²) 开销，用户粘贴大段文本时 UI 会卡。
///
/// **演进**：
/// - 8.5 初版：`char_count × 6.0`（按 64 字符集）估熵直接返 Score::4——对 `"a"×2048` 误报
/// - M1：改用 `unique_chars.log2() × char_count`——堵住 unique=1，但 `"ab"×1024`
///   （unique=2, log2(2)=1 → 2048 bit → Score::4）仍误报
/// - **N1（本修）**：取前 256 字符跑 zxcvbn 做模式识别（zxcvbn 能识别重复/循环/
///   键盘序列/字典词等多种低熵结构），用其 score；再用完整长度估熵做补充。
///   256 字符的 zxcvbn 开销可接受（不是 2KB），且抓住了 zxcvbn 的核心价值。
pub fn evaluate(password: &str) -> PasswordStrength {
    // 8.5：超长密码短路——避免 zxcvbn 对全长度的 O(n²) 开销
    const MAX_ZXCVBN_INPUT: usize = 1024;
    if password.len() > MAX_ZXCVBN_INPUT {
        // N1 修复：取前 256 字符跑 zxcvbn 做模式识别
        // （zxcvbn 能识别重复模式、键盘序列、字典词——纯熵公式抓不到这些）
        const ZXCVBN_SAMPLE_SIZE: usize = 256;
        let sample: String = password.chars().take(ZXCVBN_SAMPLE_SIZE).collect();
        let est = zxcvbn::zxcvbn(&sample, &[]);
        let pattern_score = u8::from(est.score());

        // 用完整长度估熵做补充（长度本身确实增加暴力破解成本）
        let chars: Vec<char> = password.chars().collect();
        let char_count = chars.len() as f64;
        let unique_count = std::collections::HashSet::<char>::from_iter(chars).len() as f64;
        let charset_bits = if unique_count > 1.0 {
            unique_count.log2()
        } else {
            0.0
        };
        let independent_entropy = char_count * charset_bits;

        // 综合：取 zxcvbn 模式识别 score 和熵估算的较低者
        // （两者都高才高——防止"长但重复"或"短采样恰好高熵"误报）
        let entropy_score: u8 = if independent_entropy < 28.0 {
            0
        } else if independent_entropy < 36.0 {
            1
        } else if independent_entropy < 60.0 {
            2
        } else if independent_entropy < 128.0 {
            3
        } else {
            4
        };
        let score = pattern_score.min(entropy_score);

        // H2 修复（2026-07-24）：entropy_bits 显示与 score 一致——
        // 当 zxcvbn 识别到模式（pattern_score < entropy_score）时，entropy_bits
        // 用 pattern_score 对应的熵上限（score→bit），避免「2048 bit 却 score=0」矛盾显示。
        let entropy_bits = if pattern_score < entropy_score {
            // zxcvbn 识别到低熵模式——entropy_bits 用 score 对应的上限
            match score {
                0 => independent_entropy.min(28.0),
                1 => independent_entropy.min(36.0),
                2 => independent_entropy.min(60.0),
                3 => independent_entropy.min(128.0),
                _ => independent_entropy,
            }
        } else {
            independent_entropy
        };

        let (warning, suggestions) = match est.feedback() {
            Some(fb) => {
                let warning = fb.warning().map(|w| w.to_string());
                let suggestions = fb.suggestions().iter().map(|s| s.to_string()).collect();
                (warning, suggestions)
            }
            None => (None, Vec::new()),
        };
        return PasswordStrength {
            score,
            entropy_bits,
            warning: if score < 3 && warning.is_none() {
                Some("密码虽长但模式重复，强度不足".to_string())
            } else {
                warning
            },
            suggestions: if suggestions.is_empty() {
                vec!["超长密码已取样前 256 字符做模式识别".to_string()]
            } else {
                suggestions
            },
        };
    }

    let est = zxcvbn::zxcvbn(password, &[]);

    let score = u8::from(est.score());
    // guesses_log10 → log2（× 3.32）。空密码返回 NEG_INFINITY，兜底为 0。
    let log10 = est.guesses_log10();
    let entropy_bits = if log10.is_finite() { log10 * 3.32 } else { 0.0 };

    let (warning, suggestions) = match est.feedback() {
        Some(fb) => {
            let warning = fb.warning().map(|w| w.to_string());
            let suggestions = fb.suggestions().iter().map(|s| s.to_string()).collect();
            (warning, suggestions)
        }
        None => (None, Vec::new()),
    };

    PasswordStrength {
        score,
        entropy_bits,
        warning,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weak_password_low_score() {
        let s = evaluate("password");
        assert!(s.score <= 1, "password 应是弱密码: score={}", s.score);
    }

    #[test]
    fn test_strong_password_high_score() {
        let s = evaluate("Tr0ub4dour&3-something-longer");
        assert!(s.score >= 3, "应是强密码: score={}", s.score);
    }

    /// M1 修复回归守护：超长但低熵的重复密码（如 "a".repeat(2048)）不应误报 Score::4。
    /// zxcvbn 本会识别为重复模式给低分——短路逻辑（8.5）之前绕过了这个检测，
    /// 用固定 6.0 bit/char 估熵导致误报。现在取前 256 字符跑 zxcvbn 做模式识别。
    #[test]
    fn test_very_long_repetitive_password_is_weak() {
        // 2KB 全相同字符——unique=1, 熵=0, zxcvbn 识别为重复 → Score::0（弱）
        let long_repetitive = "a".repeat(2048);
        let s = evaluate(&long_repetitive);
        assert!(
            s.score < 3,
            "全相同字符超长密码应是弱密码（M1 修复），实际 score={}",
            s.score
        );
    }

    /// N1 修复回归守护：低唯一字符数的循环重复（如 "ab"×1024）仍应给低分。
    /// M1 的 `unique.log2() × count` 公式对 unique=2 误报（log2(2)=1 → 2048 bit），
    /// N1 改用 zxcvbn 模式识别抓这类循环。
    #[test]
    fn test_very_long_low_unique_cycle_is_weak() {
        // "ab" 循环 1024 次——unique=2, 但 zxcvbn 识别为重复模式
        let cyclic = "ab".repeat(1024);
        let s = evaluate(&cyclic);
        assert!(
            s.score < 3,
            "低唯一字符循环重复应是弱密码（N1 修复），实际 score={}",
            s.score
        );
        // "abcabc..." 同理
        let cyclic3 = "abc".repeat(683); // ~2049 字符
        let s3 = evaluate(&cyclic3);
        assert!(
            s3.score < 3,
            "3 字符循环重复应是弱密码（N1），实际 score={}",
            s3.score
        );
    }

    /// M1/N1 补充：超长且高熵的密码（真随机字符）仍应返高分。
    #[test]
    fn test_very_long_diverse_password_is_strong() {
        // 构造 >1KB 的高多样性密码——用 LCG 伪随机（非纯循环），unique 多
        // 用 x^2+c 的低位映射到可打印 ASCII，避免 zxcvbn 识别为序列/重复
        let mut x: u64 = 12345;
        let diverse: String = (0..2048)
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*";
                let idx = ((x >> 33) as usize) % alphabet.len();
                alphabet.as_bytes()[idx] as char
            })
            .collect();
        let s = evaluate(&diverse);
        assert!(
            s.score >= 3,
            "高多样性超长密码应返高分（>=3），实际 score={}",
            s.score
        );
    }

    #[test]
    fn test_passphrase_strong() {
        let s = evaluate("correct horse battery staple");
        assert!(s.score >= 3, "应是强密码: score={}", s.score);
    }

    #[test]
    fn test_empty_password_no_panic() {
        let s = evaluate("");
        // 不应 panic
        assert_eq!(s.score, 0);
    }
}
