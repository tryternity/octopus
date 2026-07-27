//! 健康报告：弱密码 + 重复密码汇总。

pub mod duplicate;
pub mod strength;

use serde::Serialize;

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub weak_count: usize,
    pub weak_cipher_ids: Vec<String>,
    pub duplicate_groups: Vec<duplicate::DuplicateGroup>,
    pub total_logins: usize,
    pub average_score: f64,
    /// R-AVG-DENOM 修复（2026-07-25）：average_score 的真实分母。
    ///
    /// total_logins 含 password=None 的 Login（只存 username），但 average_score
    /// 只算 password=Some 的（无密码无强度）。两者并列展示时分母不一致误导——
    /// 前端可用本字段标注「基于 N 个有密码项」。
    pub scored_count: usize,
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
    // H1 修复（2026-07-24）：find_duplicates 签名改为 &[&Cipher]——不再深拷贝
    let duplicate_groups = duplicate::find_duplicates(&logins);
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
        scored_count: score_count, // R-AVG-DENOM：average_score 的真实分母（仅 password=Some 的 Login）
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

    /// R-AVG-DENOM 守护（2026-07-25）：password=None 的 Login 进 total_logins 但不进 scored_count。
    ///
    /// average_score 只算 password=Some 的（无密码无强度）。之前 total_logins 与
    /// average_score 分母不一致致 UI 误导。现 HealthReport 加 scored_count 透明化分母。
    #[test]
    fn test_scored_count_excludes_none_password() {
        // c1/c2 有密码，c3 无密码（只存 username）
        let mut ciphers = vec![
            make_cipher("c1", "password"),
            make_cipher("c2", "Tr0ub4dour&3-something-very-long"),
        ];
        let mut no_pwd = make_cipher("c3", "x"); // 临时占位
        // 改成无密码 Login
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(ref mut login) = no_pwd.data {
            login.password = None;
        }
        ciphers.push(no_pwd);

        let report = generate_report(&ciphers);
        assert_eq!(report.total_logins, 3, "total_logins 含无密码 Login");
        assert_eq!(
            report.scored_count, 2,
            "R-AVG-DENOM: scored_count 只算有密码的（c1+c2=2），不含 c3"
        );
        // average_score 是 c1+c2 的平均，不是除以 3
        assert!(
            report.average_score > 0.0,
            "average_score 应基于 2 个有密码项"
        );
    }
}
