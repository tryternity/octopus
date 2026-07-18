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
pub fn evaluate(password: &str) -> PasswordStrength {
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
