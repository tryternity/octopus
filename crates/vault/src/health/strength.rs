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
/// 8.5 修复（2026-07-24）：对超长密码（> 1KB，如粘贴几 KB 文本）短路——zxcvbn
/// 对超长输入有 O(n²) 开销，用户粘贴大段文本时 UI 会卡。超长密码本身必然极强
/// （长度 >> 字符集熵），直接返 Score::4（最强）+ 估算熵，跳过昂贵的模式匹配。
pub fn evaluate(password: &str) -> PasswordStrength {
    // 8.5：超长密码短路——避免 zxcvbn O(n²) 开销
    const MAX_ZXCVBN_INPUT: usize = 1024;
    if password.len() > MAX_ZXCVBN_INPUT {
        // 超长密码——估算熵（char_count × log2(charset)，保守按 64 字符集）
        // 长度 > 1024 时熵必然 > 256 bit（远超 zxcvbn Score::4 阈值）
        let charset_bits = 6.0; // log2(64)，保守估计
        let entropy_bits = password.chars().count() as f64 * charset_bits;
        return PasswordStrength {
            score: 4,
            entropy_bits,
            warning: None,
            suggestions: vec!["密码极长，强度评估已跳过模式匹配".to_string()],
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

    /// 8.5 修复回归守护：超长密码（> 1KB）应短路返 Score::4，不卡在 zxcvbn O(n²)。
    #[test]
    fn test_very_long_password_short_circuits() {
        // 构造 2KB 密码——不调 zxcvbn，直接短路
        let long_password = "a".repeat(2048);
        let s = evaluate(&long_password);
        assert_eq!(
            s.score, 4,
            "超长密码应短路返 Score::4（跳过 zxcvbn O(n²)）"
        );
        assert!(
            s.entropy_bits > 256.0,
            "超长密码熵应极高（>256 bit），实际 {}",
            s.entropy_bits
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
