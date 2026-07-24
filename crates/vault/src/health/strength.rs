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
/// 8.5 + M1 修复（2026-07-24）：对超长密码（> 1KB，如粘贴几 KB 文本）短路——
/// zxcvbn 对超长输入有 O(n²) 开销，用户粘贴大段文本时 UI 会卡。
///
/// **M1 修正**：初版用 `char_count × 6.0`（按 64 字符集）估熵直接返 Score::4——
/// 但这对低熵重复序列（如 `"a".repeat(2048)`）误报极强。zxcvbn 本会识别重复模式
/// 给低分，短路反而绕过了这个检测。现在改用**唯一字符数**估熵：
/// `unique_chars.log2() × char_count`——重复序列 unique=1, log2(1)=0 → 熵=0。
pub fn evaluate(password: &str) -> PasswordStrength {
    // 8.5：超长密码短路——避免 zxcvbn O(n²) 开销
    const MAX_ZXCVBN_INPUT: usize = 1024;
    if password.len() > MAX_ZXCVBN_INPUT {
        // M1 修正：用唯一字符数估熵，而非固定 6.0 bit/char
        let chars: Vec<char> = password.chars().collect();
        let char_count = chars.len() as f64;
        let unique: std::collections::HashSet<char> = chars.into_iter().collect();
        let unique_count = unique.len() as f64;
        // 熵 = log2(字符集大小) × 长度。unique=1 → log2(1)=0 → 熵=0（弱）
        // unique=70（正常长密码）→ log2(70)≈6.13 × 1024 ≈ 6275 bit（极强）
        let charset_bits = if unique_count > 1.0 {
            unique_count.log2()
        } else {
            0.0 // 全相同字符——熵为 0
        };
        let entropy_bits = char_count * charset_bits;
        // score 阈值：zxcvbn 用 0-4，对应熵 <28/28-36/36-60/60-128/>128 bit
        let score: u8 = if entropy_bits < 28.0 {
            0
        } else if entropy_bits < 36.0 {
            1
        } else if entropy_bits < 60.0 {
            2
        } else if entropy_bits < 128.0 {
            3
        } else {
            4
        };
        return PasswordStrength {
            score,
            entropy_bits,
            warning: if score < 3 {
                Some("密码虽长但字符重复度高，强度不足".to_string())
            } else {
                None
            },
            suggestions: vec!["超长密码已跳过模式匹配，按字符多样性估算强度".to_string()],
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
    /// 用固定 6.0 bit/char 估熵导致误报。现在用唯一字符数估熵，重复序列给低分。
    #[test]
    fn test_very_long_repetitive_password_is_weak() {
        // 2KB 全相同字符——unique=1, log2(1)=0 → 熵=0 → Score::0（弱）
        let long_repetitive = "a".repeat(2048);
        let s = evaluate(&long_repetitive);
        assert!(
            s.score < 3,
            "重复序列超长密码应是弱密码（M1 修复），实际 score={}",
            s.score
        );
    }

    /// M1 补充：超长且高熵的密码（多字符混合）仍应短路返高分。
    #[test]
    fn test_very_long_diverse_password_is_strong() {
        // 构造 >1KB 的高多样性密码——unique 多，熵高
        let diverse: String = (0..2048)
            .map(|i| {
                let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$";
                alphabet.as_bytes()[i % alphabet.len()] as char
            })
            .collect();
        let s = evaluate(&diverse);
        assert_eq!(
            s.score, 4,
            "高多样性超长密码应短路返 Score::4，实际 {}", s.score
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
