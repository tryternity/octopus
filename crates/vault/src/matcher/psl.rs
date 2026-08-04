//! eTLD+1 提取（基于 Mozilla 公共后缀列表 PSL）。
//!
//! 历史包袱：首发版用「按 `.` 分段取最后两段」的简化算法，注释自承"已知错误"
//! （见 git blame）。审查发现这是钓鱼漏洞——`barclays.co.uk` 与 `evil-attacker.co.uk`
//! 都退化到 `co.uk` 互相匹配，autotype 会把银行密码注入钓鱼站。
//!
//! 现在用 `publicsuffix` crate 的 `DefaultProvider`（编译期内嵌 PSL 数据）。
//! 参考 [Mozilla PSL](https://publicsuffix.org/)。
//!
//! 安全不变量：
//! - **IP 字面量精确匹配**——`192.168.1.1` 与 `10.20.1.1` 不应互相匹配
//! - **PSL 查不到时返回 host 本身**（如 localhost / 内网单段名）——宁可不匹配也不要错匹配
//! - **多段 TLD 必须正确**——`.co.uk / .com.cn / .co.jp` 等不再退化

use publicsuffix::Psl;
use std::net::IpAddr;

/// 编译期内嵌 Mozilla PSL 数据（从 publicsuffix.org 拉取，~330KB）。
///
/// 升级 `publicsuffix` crate 版本不会自动更新这份列表——需手动重新下载：
/// ```bash
/// curl -o crates/infra/resources/dicts/public_suffix_list.dat \
///   https://publicsuffix.org/list/public_suffix_list.dat
/// ```
/// Mozilla 大约每月更新一次 PSL，建议季度级同步。
static PSL_BYTES: &[u8] = octopus_infra::resources::public_suffix_list();

/// 内嵌 PSL 单例——`OnceLock` 保证首次访问时解析一次。
static PSL: std::sync::OnceLock<publicsuffix::List> = std::sync::OnceLock::new();

fn psl() -> &'static publicsuffix::List {
    PSL.get_or_init(|| {
        publicsuffix::List::from_bytes(PSL_BYTES)
            .expect("内嵌的 public_suffix_list.dat 解析失败——请重新下载")
    })
}

/// 提取 eTLD+1（registrable domain）。
///
/// - 空串 → `None`
/// - IP 字面量（IPv4/IPv6）→ 原样返回（精确匹配，不做 eTLD+1）
/// - `localhost` / 单段名 → 原样返回
/// - `example.com` → `example.com`
/// - `mail.google.com` → `google.com`
/// - `foo.bar.example.co.uk` → `example.co.uk`（PSL 正确处理多段 TLD）
/// - `barclays.co.uk` → `barclays.co.uk`；`evil-attacker.co.uk` → `evil-attacker.co.uk`
///   （两者不再退化为 `co.uk` 互相匹配）
pub fn etld_plus_one(host: &str) -> Option<String> {
    if host.is_empty() {
        return None;
    }

    // IP 字面量：精确匹配，不做 eTLD+1（否则 `192.168.1.1` 与 `10.20.1.1` 会都被
    // 简化算法处理成 `1.1` 互相匹配 → 路由器密码钓鱼）。
    if host.parse::<IpAddr>().is_ok() {
        return Some(host.to_string());
    }

    // PSL 查询：返回 registrable domain（eTLD+1）。
    // PSL 查不到（localhost / 内网单段名 / 未知 TLD）→ 返回 host 本身。
    // 这比"取最后两段"安全——宁可匹配失败也不要错匹配（fail-closed）。
    psl().domain(host.as_bytes())
        .map(|d| String::from_utf8_lossy(d.as_bytes()).into_owned())
        .or_else(|| Some(host.to_string()))
}

/// MVP 内置默认等价域名（借鉴 Bitwarden global_domains.json）。
///
/// 同一组的域名视为"同一家公司"——任一域名的 cipher 也可被组内其他域名的
/// URL 匹配到。例如 google.com 的账号在 youtube.com 上也会被列出。
///
/// 注意：跨 ccTLD 的等价（如 `amazon.com / amazon.co.jp`）现在 PSL 能正确
/// 分辨，但仍保留等价组——因为它们确实是同一家公司、共享账号体系。
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
        assert_eq!(etld_plus_one("example.com").as_deref(), Some("example.com"));
        assert_eq!(etld_plus_one("mail.google.com").as_deref(), Some("google.com"));
        assert_eq!(etld_plus_one("a.b.c.example.io").as_deref(), Some("example.io"));
        assert!(etld_plus_one("").is_none());
    }

    #[test]
    fn test_localhost_returns_as_is() {
        assert_eq!(etld_plus_one("localhost").as_deref(), Some("localhost"));
    }

    /// 关键安全测试：多段 TLD 必须正确处理，钓鱼站不应与受害者共享 eTLD+1。
    #[test]
    fn test_phishing_protection_multilevel_tld() {
        // 银行域名 eTLD+1 = barclays.co.uk（PSL 知道 .co.uk 是两段 public suffix）
        assert_eq!(
            etld_plus_one("barclays.co.uk").as_deref(),
            Some("barclays.co.uk")
        );
        assert_eq!(
            etld_plus_one("www.barclays.co.uk").as_deref(),
            Some("barclays.co.uk")
        );
        // 钓鱼站 eTLD+1 = evil-attacker.co.uk（≠ barclays.co.uk）
        assert_eq!(
            etld_plus_one("evil-attacker.co.uk").as_deref(),
            Some("evil-attacker.co.uk")
        );
        // 两者不等 → matches_domain 不会命中，autotype 不会误填密码
        assert_ne!(
            etld_plus_one("barclays.co.uk"),
            etld_plus_one("evil-attacker.co.uk")
        );

        // 类似 .com.cn / .co.jp / .com.au 案例
        assert_eq!(
            etld_plus_one("example.com.cn").as_deref(),
            Some("example.com.cn")
        );
        assert_eq!(
            etld_plus_one("attacker.com.cn").as_deref(),
            Some("attacker.com.cn")
        );
        assert_ne!(
            etld_plus_one("example.com.cn"),
            etld_plus_one("attacker.com.cn")
        );
    }

    /// IP 字面量精确匹配——路由器管理界面之间不应互相匹配。
    #[test]
    fn test_ip_literal_exact_match() {
        assert_eq!(
            etld_plus_one("192.168.1.1").as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(
            etld_plus_one("10.20.1.1").as_deref(),
            Some("10.20.1.1")
        );
        // 两个不同 IP 不应匹配（简化算法会都退化到 "1.1"）
        assert_ne!(
            etld_plus_one("192.168.1.1"),
            etld_plus_one("10.20.1.1")
        );

        // IPv6 同理
        assert_eq!(
            etld_plus_one("::1").as_deref(),
            Some("::1")
        );
        assert_eq!(
            etld_plus_one("fe80::1").as_deref(),
            Some("fe80::1")
        );
    }

    #[test]
    fn test_default_equivalent_domains_nonempty() {
        let d = default_equivalent_domains();
        assert!(!d.is_empty());
        assert!(d.iter().any(|g| g.contains(&"google.com".to_string())));
    }
}
