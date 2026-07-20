//! 健康报告：弱密码 + 重复密码汇总。

pub mod duplicate;
pub mod strength;

use serde::Serialize;

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub weak_count: usize,
    pub weak_cipher_ids: Vec<String>,
    pub duplicate_groups: Vec<duplicate::DuplicateGroup>,
    pub total_logins: usize,
    pub average_score: f64,
}

pub fn generate_report(ciphers: &[Cipher]) -> HealthReport {
    let logins: Vec<&Cipher> = ciphers
        .iter()
        .filter(|c| matches!(&c.data, CipherData::Login(_)) && c.deleted_at.is_none())
        .collect();

    // 弱密码：score < 3
    let mut weak_cipher_ids = Vec::new();
    let mut total_score: f64 = 0.0;
    let mut score_count: usize = 0;
    for c in &logins {
        // CipherData 当前仅 Login 单变体；保留 if let 以便未来扩展 SecureNote/Card/Identity。
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(login) = &c.data {
            if let Some(pwd) = &login.password {
                let s = strength::evaluate(pwd);
                total_score += s.score as f64;
                score_count += 1;
                if s.score < 3 {
                    weak_cipher_ids.push(c.id.clone());
                }
            }
        }
    }

    let weak_count = weak_cipher_ids.len();
    let duplicate_groups =
        duplicate::find_duplicates(&logins.iter().map(|&r| r.clone()).collect::<Vec<_>>());
    let average_score = if score_count > 0 {
        total_score / score_count as f64
    } else {
        0.0
    };

    HealthReport {
        weak_count,
        weak_cipher_ids,
        duplicate_groups,
        total_logins: logins.len(),
        average_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CipherType, LoginData, LoginUri, RepromptType};

    fn make_cipher(id: &str, password: &str) -> Cipher {
        Cipher {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: format!("c-{}", id),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: format!("https://{}.com", id),
                    match_type: None,
                }],
                username: None,
                password: Some(password.into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_report_aggregates() {
        let ciphers = vec![
            make_cipher("c1", "password"),                        // 弱 + 重复
            make_cipher("c2", "password"),                        // 弱 + 重复
            make_cipher("c3", "Tr0ub4dour&3-something-very-long"), // 强
        ];
        let report = generate_report(&ciphers);
        assert_eq!(report.total_logins, 3);
        assert!(report.weak_count >= 2, "至少 2 个弱: {}", report.weak_count);
        assert_eq!(report.duplicate_groups.len(), 1);
        assert_eq!(report.duplicate_groups[0].cipher_ids.len(), 2);
    }

    #[test]
    fn test_report_excludes_deleted() {
        let mut ciphers = vec![make_cipher("c1", "weak")];
        ciphers[0].deleted_at = Some("2026-07-18".into());
        let report = generate_report(&ciphers);
        assert_eq!(report.total_logins, 0);
    }
}
