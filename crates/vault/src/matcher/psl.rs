//! eTLD+1 提取（简化版：按 `.` 分段取最后两段）。
//!
//! 完整实现需要公共后缀列表（PSL）才能正确处理 `.co.uk` / `.com.cn` 等多段
//! TLD。MVP 阶段采用简化策略——直接按 `.` 分段，对常见单 TLD 场景（.com/.io/
//! .cn/.jp 等）足够准确；多段 TLD 场景（example.co.uk → co.uk）会出错，
//! brief 已明确接受此局限。
//!
//! 因此本模块**不**依赖 `publicsuffix` crate（即使 `Cargo.toml` 中保留了
//! `publicsuffix = "2"`，本文件移除了相关 import，避免 unused import 警告）。

/// 提取 eTLD+1（简化版）。
///
/// - 空串 → `None`
/// - `localhost` / 单段 → 原样返回
/// - `example.com` → 原样返回（已是最小可注册域名）
/// - `mail.google.com` → `google.com`（取最后两段）
/// - `foo.bar.example.co.uk` → `co.uk`（**已知错误**，PSL 才能正确处理）
pub fn etld_plus_one(host: &str) -> Option<String> {
    // 简化版：按 . 分段处理
    // 完整版需要 PSL（公共后缀列表）才能正确处理 .co.uk 等
    // MVP 接受这个局限（多数登录网站是 .com / .cn / .io 等单 TLD）
    if host.is_empty() {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    match parts.len() {
        0 => None,
        1 => Some(host.to_string()), // localhost
        2 => Some(host.to_string()), // example.com
        _ => {
            // 取最后两段：mail.google.com → google.com
            // 局限：example.co.uk → co.uk（错，但 MVP 接受）
            let n = parts.len();
            Some(format!("{}.{}", parts[n - 2], parts[n - 1]))
        }
    }
}

/// MVP 内置默认等价域名（借鉴 Bitwarden global_domains.json）。
///
/// 同一组的域名视为"同一家公司"——任一域名的 cipher 也可被组内其他域名的
/// URL 匹配到。例如 google.com 的账号在 youtube.com 上也会被列出。
pub fn default_equivalent_domains() -> Vec<Vec<String>> {
    vec![
        vec![
            "google.com".into(),
            "youtube.com".into(),
            "gmail.com".into(),
            "g.co".into(),
        ],
        vec!["live.com".into(), "hotmail.com".into(), "outlook.com".into()],
        vec!["apple.com".into(), "icloud.com".into()],
        vec![
            "amazon.com".into(),
            "amazon.co.jp".into(),
            "amazon.co.uk".into(),
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_etld_plus_one_simple() {
        // 简化版按 `.` 分段取最后两段
        let r = etld_plus_one("example.com");
        assert_eq!(r.as_deref(), Some("example.com"));

        let sub = etld_plus_one("mail.google.com");
        assert_eq!(sub.as_deref(), Some("google.com"));

        let deep = etld_plus_one("a.b.c.example.io");
        assert_eq!(deep.as_deref(), Some("example.io"));

        assert!(etld_plus_one("").is_none());
    }

    #[test]
    fn test_localhost_returns_as_is() {
        let r = etld_plus_one("localhost");
        assert_eq!(r.as_deref(), Some("localhost"));
    }

    #[test]
    fn test_default_equivalent_domains_nonempty() {
        let d = default_equivalent_domains();
        assert!(!d.is_empty());
        assert!(d.iter().any(|g| g.contains(&"google.com".to_string())));
    }
}
