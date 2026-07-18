//! Bitwarden unencrypted JSON 导入。
//!
//! 仅支持 type=1 (Login)。
//! 加密导出（encrypted=true）不支持（MVP）。

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::crypto::DerivedKey;
use crate::storage;
use crate::types::{
    CipherData, CipherInput, CipherType, Field, LoginData, LoginUri, MatchType, RepromptType,
};

#[derive(Debug, Deserialize)]
struct BitwardenExport {
    encrypted: bool,
    #[serde(default)]
    items: Vec<BitwardenItem>,
}

#[derive(Debug, Deserialize)]
struct BitwardenItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    favorite: bool,
    #[serde(default = "default_type")]
    #[serde(rename = "type")]
    item_type: i64,
    #[serde(default)]
    fields: Vec<BitwardenField>,
    #[serde(default)]
    login: Option<BitwardenLogin>,
}

fn default_type() -> i64 {
    1
}

#[derive(Debug, Deserialize)]
struct BitwardenField {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    field_type: i64,
}

#[derive(Debug, Deserialize)]
struct BitwardenLogin {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    uris: Vec<BitwardenUri>,
}

#[derive(Debug, Deserialize)]
struct BitwardenUri {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    r#match: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct ImportReport {
    pub total: usize,
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub fn import_bitwarden_json(json: &str, key: &DerivedKey) -> Result<ImportReport> {
    let export: BitwardenExport = serde_json::from_str(json).context("JSON 解析失败")?;
    ensure!(!export.encrypted, "不支持加密导出（仅 unencrypted JSON）");

    let mut imported = 0;
    let mut skipped = 0;
    let mut errors: Vec<String> = Vec::new();

    for (idx, item) in export.items.iter().enumerate() {
        if item.item_type != 1 {
            skipped += 1;
            continue;
        }
        let login = match &item.login {
            Some(l) => l,
            None => {
                skipped += 1;
                continue;
            }
        };

        let input = CipherInput {
            folder_id: None,
            favorite: item.favorite,
            atype: CipherType::Login,
            name: item.name.clone(),
            notes: item.notes.clone(),
            data: CipherData::Login(LoginData {
                uris: login
                    .uris
                    .iter()
                    .map(|u| LoginUri {
                        uri: u.uri.clone(),
                        match_type: u.r#match.and_then(|m| MatchType::try_from(m).ok()),
                    })
                    .collect(),
                username: login.username.clone(),
                password: login.password.clone(),
                totp: login.totp.clone(),
                password_revision_date: None,
            }),
            fields: item
                .fields
                .iter()
                .map(|f| Field {
                    name: f.name.clone(),
                    value: f.value.clone(),
                    field_type: f.field_type,
                })
                .collect(),
            password_history: vec![],
            reprompt: RepromptType::None,
        };

        match storage::create_cipher(&input, key) {
            Ok(_) => imported += 1,
            Err(e) => {
                errors.push(format!("Item {} ({}): {}", idx, item.name, e));
                skipped += 1;
            }
        }
    }

    Ok(ImportReport {
        total: export.items.len(),
        imported,
        skipped,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    #[test]
    fn test_reject_encrypted_export() {
        let key = make_key(1);
        let json = r#"{"encrypted": true, "items": []}"#;
        let result = import_bitwarden_json(json, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_minimal_export() {
        // 仅测 JSON 解析（不实际写入 DB）
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "GitHub",
                    "favorite": false,
                    "type": 1,
                    "login": {
                        "username": "user@example.com",
                        "password": "secret",
                        "uris": [{"uri": "https://github.com", "match": null}]
                    }
                }
            ]
        }"#;
        let export: BitwardenExport = serde_json::from_str(json).unwrap();
        assert!(!export.encrypted);
        assert_eq!(export.items.len(), 1);
        assert_eq!(export.items[0].name, "GitHub");
    }

    #[test]
    fn test_skip_non_login_type() {
        let json = r#"{
            "encrypted": false,
            "items": [
                {"name": "Note", "type": 2, "notes": "secret"},
                {"name": "Login", "type": 1, "login": {"username": "u"}}
            ]
        }"#;
        let export: BitwardenExport = serde_json::from_str(json).unwrap();
        let login_count = export.items.iter().filter(|i| i.item_type == 1).count();
        assert_eq!(login_count, 1);
    }

    #[test]
    fn test_invalid_json_errors() {
        let key = make_key(1);
        let result = import_bitwarden_json("not json", &key);
        assert!(result.is_err());
    }
}
