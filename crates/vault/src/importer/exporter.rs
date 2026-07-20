//! 导出 vault 为 Bitwarden unencrypted JSON。

use anyhow::Result;
use serde::Serialize;

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
struct BitwardenExport {
    encrypted: bool,
    version: i64,
    items: Vec<BitwardenItem>,
    folders: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BitwardenItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    favorite: bool,
    #[serde(rename = "type")]
    item_type: i64,
    fields: Vec<BitwardenField>,
    login: Option<BitwardenLogin>,
    /// 修复 #4：之前导出端未写 reprompt，导致 round-trip 丢失。Bitwarden 用 i64 表示。
    reprompt: i64,
}

#[derive(Debug, Serialize)]
struct BitwardenField {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(rename = "type")]
    field_type: i64,
}

#[derive(Debug, Serialize)]
struct BitwardenLogin {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    totp: Option<String>,
    uris: Vec<BitwardenUri>,
}

#[derive(Debug, Serialize)]
struct BitwardenUri {
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#match: Option<i64>,
}

pub fn export_vault_json(ciphers: &[Cipher]) -> Result<String> {
    let items: Vec<BitwardenItem> = ciphers
        .iter()
        .filter(|c| c.deleted_at.is_none())
        .map(|c| {
            let (item_type, login) = match &c.data {
                CipherData::Login(l) => (
                    1i64,
                    Some(BitwardenLogin {
                        username: l.username.clone(),
                        password: l.password.clone(),
                        totp: l.totp.clone(),
                        uris: l
                            .uris
                            .iter()
                            .map(|u| BitwardenUri {
                                uri: u.uri.clone(),
                                r#match: u.match_type.map(|m| m.into()),
                            })
                            .collect(),
                    }),
                ),
            };
            BitwardenItem {
                name: c.name.clone(),
                notes: c.notes.clone(),
                favorite: c.favorite,
                item_type,
                fields: c
                    .fields
                    .iter()
                    .map(|f| BitwardenField {
                        name: f.name.clone(),
                        value: f.value.clone(),
                        field_type: f.field_type,
                    })
                    .collect(),
                login,
                // #4：导出 reprompt，避免 round-trip 丢失（与 bitwarden.rs 导入端对称）。
                reprompt: i64::from(c.reprompt),
            }
        })
        .collect();

    let export = BitwardenExport {
        encrypted: false,
        version: 2,
        items,
        folders: vec![],
    };

    Ok(serde_json::to_string_pretty(&export)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CipherType, Field, LoginData, LoginUri, PasswordHistoryEntry, RepromptType,
    };

    fn make_login_cipher(name: &str) -> Cipher {
        Cipher {
            id: format!("exporter-{}", name),
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: name.into(),
            notes: Some("personal".into()),
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: "https://example.com".into(),
                    match_type: None,
                }],
                username: Some("user".into()),
                password: Some("pass".into()),
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

    // 抑制未使用警告：保留为后续 test 用例预留的导入，便于扩展
    #[allow(dead_code)]
    fn _retain_imports() {
        let _ = Field {
            name: String::new(),
            value: None,
            field_type: 0,
        };
        let _ = PasswordHistoryEntry {
            password: String::new(),
            last_used_at: String::new(),
        };
    }

    #[test]
    fn test_export_round_trip_parse() {
        let ciphers = vec![make_login_cipher("GitHub")];
        let json = export_vault_json(&ciphers).unwrap();
        assert!(json.contains("\"GitHub\""));
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"pass\""));

        // 重新解析回来
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["encrypted"], false);
        assert_eq!(parsed["items"][0]["name"], "GitHub");
    }

    #[test]
    fn test_export_skips_deleted() {
        let mut c = make_login_cipher("GitHub");
        c.deleted_at = Some("2026-07-18".into());
        let json = export_vault_json(&[c]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["items"].as_array().unwrap().len(), 0);
    }

    /// #4：导出 reprompt=Password（i64=1）应出现在 JSON 中——之前完全缺失。
    #[test]
    fn test_export_includes_reprompt() {
        let mut c = make_login_cipher("Sensitive");
        c.reprompt = RepromptType::Password;
        let json = export_vault_json(&[c]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["items"][0]["reprompt"], 1,
            "导出应含 reprompt=1（修复 #4：导出端不再丢失该字段）"
        );
    }

    /// #4 round-trip：导出 → 重新导入应保留 reprompt 状态。
    #[test]
    fn test_export_reprompt_round_trip_parse() {
        let mut c = make_login_cipher("Sensitive");
        c.reprompt = RepromptType::Password;
        let json = export_vault_json(&[c]).unwrap();
        // 重新解析回 Bitwarden JSON 格式（导入端 struct 在 bitwarden.rs，此处只校验 JSON 字段存在）
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["items"][0]["reprompt"], 1);
        assert_eq!(parsed["items"][0]["name"], "Sensitive");
    }
}
