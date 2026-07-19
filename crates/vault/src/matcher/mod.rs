//! URL 匹配：5 种策略 + 等价域名。
//!
//! 直接借鉴 Bitwarden：
//!   - `Domain` (eTLD+1, 默认)：注册域名相等或同属一个等价域名组
//!   - `Host`：完整 host 字符串相等
//!   - `Exact`：完整 URL 字符串相等
//!   - `StartsWith`：URL 以 cipher_uri 为前缀
//!   - `RegularExpression`：URL 匹配正则
//!   - `Never`：永不匹配
//!
//! 非 Login cipher 永不匹配；标记删除（deleted_at）的 cipher 在入口被过滤。

pub mod psl;

use std::collections::HashSet;

use regex::Regex;
use url::Url;

use crate::types::{Cipher, CipherData, MatchType};

/// 从一组 cipher 中找出 URL 匹配上的，按出现顺序返回引用列表。
///
/// - `equivalent_domains`：等价域名分组（参见 `psl::default_equivalent_domains`）。
///   仅在 `Domain` 策略下生效——target_domain 落在 cipher_domain 所属分组内即匹配。
/// - 标记删除（`deleted_at.is_some()`）的 cipher 被跳过。
pub fn find_matching_ciphers<'a>(
    url: &Url,
    ciphers: &'a [Cipher],
    equivalent_domains: &[Vec<String>],
) -> Vec<&'a Cipher> {
    ciphers
        .iter()
        .filter(|c| c.deleted_at.is_none())
        .filter(|c| matches_any_uri(url, c, equivalent_domains))
        .collect()
}

/// 一个 cipher 的任一 URI 匹配即为命中。
fn matches_any_uri(url: &Url, cipher: &Cipher, equivalent: &[Vec<String>]) -> bool {
    // `CipherData` 当前只有 Login 变体，未来扩展 SecureNote/Card/Identity 后
    // 此 `_` 分支即生效——非 Login cipher 不参与 URL 匹配。
    #[allow(unreachable_patterns)]
    let login = match &cipher.data {
        CipherData::Login(l) => l,
        _ => return false,
    };
    login
        .uris
        .iter()
        .any(|lu| match_uri_one(url, lu, equivalent))
}

/// 单条 URI 的匹配判定（按其 match_type 走 5 种策略之一）。
fn match_uri_one(
    url: &Url,
    lu: &crate::types::LoginUri,
    equivalent: &[Vec<String>],
) -> bool {
    let strategy = lu.match_type.unwrap_or(MatchType::Domain);
    let target = url.as_str();
    let cipher_uri = &lu.uri;

    // 修复 #11：空 cipher_uri 视为 Never——避免 starts_with("") / Regex::new("")
    // 恒真匹配任意站点。Domain / Host 策略下空串也会走 fallback 路径误匹配。
    if cipher_uri.trim().is_empty() {
        return false;
    }

    match strategy {
        MatchType::Domain => psl::etld_plus_one(url.host_str().unwrap_or(""))
            .map(|target_domain| matches_domain(&target_domain, cipher_uri, equivalent))
            .unwrap_or(false),
        MatchType::Host => {
            let target_host = url.host_str().unwrap_or("");
            // cipher_uri 可能是 https://github.com 或 github.com
            let cipher_host = Url::parse(cipher_uri)
                .ok()
                .and_then(|u| u.host_str().map(String::from))
                .unwrap_or_else(|| cipher_uri.to_string());
            target_host == cipher_host
        }
        MatchType::Exact => target == cipher_uri,
        MatchType::StartsWith => target.starts_with(cipher_uri.as_str()),
        MatchType::RegularExpression => Regex::new(cipher_uri)
            .map(|r| r.is_match(target))
            .unwrap_or(false),
        MatchType::Never => false,
    }
}

/// Domain 匹配：target_domain 是否在 cipher_domain + 其等价域名组内。
///
/// candidate 集合 = { cipher_domain } ∪ { 与 cipher_domain 同组的所有域名 }。
/// target_domain ∈ candidate 即匹配。
fn matches_domain(
    target_domain: &str,
    cipher_uri: &str,
    equivalent: &[Vec<String>],
) -> bool {
    let cipher_host = Url::parse(cipher_uri)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| cipher_uri.to_string());
    let cipher_domain = psl::etld_plus_one(&cipher_host).unwrap_or_else(|| cipher_host.clone());

    let mut candidates: HashSet<String> = HashSet::new();
    candidates.insert(cipher_domain.clone());
    // 加入等价域名
    for group in equivalent {
        if group.contains(&cipher_domain) {
            for d in group {
                candidates.insert(d.clone());
            }
        }
    }
    candidates.contains(target_domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LoginData, LoginUri};

    fn make_cipher(uris: &[(&str, Option<MatchType>)]) -> Cipher {
        Cipher {
            id: 1,
            folder_id: None,
            favorite: false,
            atype: crate::types::CipherType::Login,
            name: "test".into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: uris
                    .iter()
                    .map(|(u, m)| LoginUri {
                        uri: u.to_string(),
                        match_type: *m,
                    })
                    .collect(),
                username: None,
                password: None,
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: crate::types::RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_domain_match_subdomain() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://gist.github.com/foo").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_domain_match_exact() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://github.com/login").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_domain_match_different_etld_no() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://github.io/foo").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 0); // 不同 eTLD+1
    }

    #[test]
    fn test_host_match() {
        let cipher = make_cipher(&[("https://mail.google.com", Some(MatchType::Host))]);
        let url_match = Url::parse("https://mail.google.com/inbox").unwrap();
        let url_nomatch = Url::parse("https://drive.google.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url_match, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
        assert_eq!(
            find_matching_ciphers(&url_nomatch, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_exact_match() {
        let cipher = make_cipher(&[("https://example.com/login", Some(MatchType::Exact))]);
        let url_match = Url::parse("https://example.com/login").unwrap();
        let url_nomatch = Url::parse("https://example.com/login?foo=1").unwrap();
        assert_eq!(
            find_matching_ciphers(&url_match, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
        assert_eq!(
            find_matching_ciphers(&url_nomatch, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_starts_with_match() {
        let cipher = make_cipher(&[("https://example.com/admin", Some(MatchType::StartsWith))]);
        let url = Url::parse("https://example.com/admin/users").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }

    #[test]
    fn test_regex_match() {
        let cipher =
            make_cipher(&[(r"^https://.*\.example\.com", Some(MatchType::RegularExpression))]);
        let url = Url::parse("https://foo.example.com/bar").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }

    #[test]
    fn test_never_match() {
        let cipher = make_cipher(&[("https://example.com", Some(MatchType::Never))]);
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_equivalent_domains() {
        let cipher = make_cipher(&[("https://google.com", None)]);
        let equivalent = vec![vec!["google.com".to_string(), "youtube.com".to_string()]];
        let url = Url::parse("https://youtube.com/watch?v=123").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &equivalent).len(),
            1
        );
    }

    #[test]
    fn test_skip_deleted_cipher() {
        let mut cipher = make_cipher(&[("https://example.com", None)]);
        cipher.deleted_at = Some("2026-07-18".into());
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_multiple_uris_any_match() {
        let cipher = make_cipher(&[
            ("https://github.com", None),
            ("https://gitlab.com", None),
        ]);
        let url = Url::parse("https://gitlab.com/foo").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }

    /// #11：空 cipher_uri + StartsWith 不应匹配任意 URL（之前 starts_with("") 恒真）。
    #[test]
    fn test_empty_cipher_uri_startswith_does_not_match() {
        let cipher = make_cipher(&[("", Some(MatchType::StartsWith))]);
        let url = Url::parse("https://example.com/anything").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0,
            "空 cipher_uri 不应匹配任意站点（修复 #11）"
        );
    }

    /// #11 补充：空 cipher_uri + RegularExpression 也不应匹配。
    #[test]
    fn test_empty_cipher_uri_regex_does_not_match() {
        let cipher = make_cipher(&[("", Some(MatchType::RegularExpression))]);
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0,
            "空 cipher_uri + Regex 不应匹配（Regex::new(\"\") 恒真——已修复）"
        );
    }

    /// #11 补充：空白字符（仅空格）的 cipher_uri 也应视为 Never。
    #[test]
    fn test_whitespace_only_cipher_uri_does_not_match() {
        let cipher = make_cipher(&[("   ", Some(MatchType::StartsWith))]);
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }
}
