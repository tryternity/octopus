//! 导出 vault 为 Bitwarden unencrypted JSON。
//!
//! M6 修复（2026-07-24）：补全 passwordHistory + folder 结构的 round-trip——
//! 之前导出端丢弃这两项，导出→重新导入后密码历史清空 + 文件夹归属丢失。

use anyhow::Result;
use serde::Serialize;

use crate::storage::FolderDto;
use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
struct BitwardenExport {
    encrypted: bool,
    version: i64,
    folders: Vec<BitwardenFolder>,
    items: Vec<BitwardenItem>,
}

#[derive(Debug, Serialize)]
struct BitwardenFolder {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct BitwardenItem {
    name: String,
    /// M6 修复：folderId 引用 folders.id（之前完全缺失 → 导入后丢失文件夹归属）
    #[serde(rename = "folderId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    favorite: bool,
    #[serde(rename = "type")]
    item_type: i64,
    fields: Vec<BitwardenField>,
    login: Option<BitwardenLogin>,
    /// 修复 #4：之前导出端未写 reprompt，导致 round-trip 丢失。
    reprompt: i64,
    /// M6 修复：密码历史（之前完全缺失 → 导入后清空）
    #[serde(rename = "passwordHistory")]
    password_history: Vec<BitwardenPasswordHistory>,
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

/// Bitwarden 密码历史条目（M6 修复）。
///
/// Bitwarden JSON 用 `passwordHistory`（camelCase）+ `lastUsedDate`（ISO 8601）。
#[derive(Debug, Serialize)]
struct BitwardenPasswordHistory {
    password: String,
    #[serde(rename = "lastUsedDate")]
    last_used_date: String,
}

/// L18 修复（2026-07-24）：把 octopus 内部时间戳归一化为 Bitwarden ISO 8601。
///
/// octopus 的 `last_used_at` 来自 SQLite `datetime('now')`（格式 `"2026-07-24 12:00:00"`，
/// 空格分隔、无时区）。Bitwarden 标准 `lastUsedDate` 是 `"2026-07-24T12:00:00.000Z"`。
/// octopus 自身 round-trip 安全（字符串透传），但导出到真 Bitwarden 需归一化。
///
/// 策略：把第一个空格替换为 `T`，追加 `.000Z` 后缀。已是 ISO 格式的透传。
fn normalize_to_iso8601(ts: &str) -> String {
    let trimmed = ts.trim();
    if trimmed.is_empty() {
        return "1970-01-01T00:00:00.000Z".to_string();
    }
    // 已含 T（ISO 格式）→ 检查是否需补 .000Z
    if trimmed.contains('T') {
        if trimmed.ends_with('Z') || trimmed.contains('.') {
            return trimmed.to_string();
        }
        return format!("{}.000Z", trimmed);
    }
    // SQLite 格式（空格分隔）→ 替换为 T + 补 .000Z
    let with_t = trimmed.replacen(' ', "T", 1);
    format!("{}.000Z", with_t)
}

/// 导出 vault 为 Bitwarden unencrypted JSON。
///
/// M6 修复（2026-07-24）：签名改为收 `(&[Cipher], &[FolderDto])`——
/// folders 数据（已解密明文）单独传入，导出为 Bitwarden folders 数组。
/// 每个 item 的 folderId 引用 folder.id，实现文件夹结构 round-trip。
pub fn export_vault_json(ciphers: &[Cipher], folders: &[FolderDto]) -> Result<String> {
    let bw_folders: Vec<BitwardenFolder> = folders
        .iter()
        .map(|f| BitwardenFolder {
            id: f.id.clone(),
            name: f.name.clone(),
        })
        .collect();

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
                folder_id: c.folder_id.clone(), // M6：folderId 引用
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
                reprompt: i64::from(c.reprompt),
                password_history: c
                    .password_history
                    .iter()
                    .map(|h| BitwardenPasswordHistory {
                        password: h.password.clone(),
                        // L18 修复：归一化为 Bitwarden ISO 8601
                        last_used_date: normalize_to_iso8601(&h.last_used_at),
                    })
                    .collect(),
            }
        })
        .collect();

    let export = BitwardenExport {
        encrypted: false,
        version: 2,
        folders: bw_folders,
        items,
    };

    Ok(serde_json::to_string_pretty(&export)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CipherType, LoginData, LoginUri, PasswordHistoryEntry, RepromptType,
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

    fn make_folder(id: &str, name: &str) -> FolderDto {
        FolderDto {
            id: id.into(),
            name: name.into(),
            sort_order: 0,
            created_at: "2026-07-24".into(),
            updated_at: "2026-07-24".into(),
        }
    }

    #[test]
    fn test_export_round_trip_parse() {
        let ciphers = vec![make_login_cipher("GitHub")];
        let json = export_vault_json(&ciphers, &[]).unwrap();
        assert!(json.contains("\"GitHub\""));
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"pass\""));

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["encrypted"], false);
        assert_eq!(parsed["items"][0]["name"], "GitHub");
    }

    #[test]
    fn test_export_skips_deleted() {
        let mut c = make_login_cipher("GitHub");
        c.deleted_at = Some("2026-07-18".into());
        let json = export_vault_json(&[c], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["items"].as_array().unwrap().len(), 0);
    }

    /// #4：导出 reprompt=Password（i64=1）应出现在 JSON 中。
    #[test]
    fn test_export_includes_reprompt() {
        let mut c = make_login_cipher("Sensitive");
        c.reprompt = RepromptType::Password;
        let json = export_vault_json(&[c], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["items"][0]["reprompt"], 1,
            "导出应含 reprompt=1（修复 #4）"
        );
    }

    /// M6：导出应含 passwordHistory（之前完全缺失）。
    #[test]
    fn test_export_includes_password_history() {
        let mut c = make_login_cipher("WithHistory");
        c.password_history = vec![
            PasswordHistoryEntry {
                password: "old-pass-1".into(),
                last_used_at: "2026-01-01T00:00:00".into(),
            },
            PasswordHistoryEntry {
                password: "old-pass-2".into(),
                last_used_at: "2026-02-01T00:00:00".into(),
            },
        ];
        let json = export_vault_json(&[c], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let hist = &parsed["items"][0]["passwordHistory"];
        assert_eq!(
            hist.as_array().unwrap().len(),
            2,
            "M6: 导出应含 2 条密码历史"
        );
        assert_eq!(hist[0]["password"], "old-pass-1");
        assert_eq!(hist[0]["lastUsedDate"], "2026-01-01T00:00:00.000Z");
    }

    /// M6：导出应含 folders + item 的 folderId（之前 folders 硬编码空）。
    #[test]
    fn test_export_includes_folders() {
        let mut c = make_login_cipher("InFolder");
        c.folder_id = Some("folder-uuid-1".into());
        let folders = vec![make_folder("folder-uuid-1", "Social")];
        let json = export_vault_json(&[c], &folders).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        // folders 数组含 folder
        assert_eq!(parsed["folders"][0]["id"], "folder-uuid-1");
        assert_eq!(parsed["folders"][0]["name"], "Social");
        // item 的 folderId 引用
        assert_eq!(parsed["items"][0]["folderId"], "folder-uuid-1");
    }

    /// M6：无 folder 的 cipher 不输出 folderId（skip_serializing_if）。
    #[test]
    fn test_export_omits_folder_id_when_none() {
        let c = make_login_cipher("RootLevel");
        let json = export_vault_json(&[c], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parsed["items"][0].get("folderId").is_none(),
            "无 folder 的 cipher 不应输出 folderId"
        );
    }

    /// M6：空 password_history 的 cipher 不输出 passwordHistory 字段（空数组仍输出，
    /// Bitwarden 格式如此——空数组是合法的，表示无历史）。
    #[test]
    fn test_export_empty_password_history_is_empty_array() {
        let c = make_login_cipher("NoHistory");
        let json = export_vault_json(&[c], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["items"][0]["passwordHistory"].as_array().unwrap().len(),
            0
        );
    }

    /// L18：SQLite datetime 格式（空格分隔）应归一化为 Bitwarden ISO 8601。
    #[test]
    fn test_normalize_to_iso8601() {
        // SQLite 格式（空格分隔）→ T + .000Z
        assert_eq!(
            normalize_to_iso8601("2026-07-24 12:00:00"),
            "2026-07-24T12:00:00.000Z"
        );
        // 已含 T 无 .000Z → 补 .000Z
        assert_eq!(
            normalize_to_iso8601("2026-01-01T00:00:00"),
            "2026-01-01T00:00:00.000Z"
        );
        // 已是完整 ISO → 透传
        assert_eq!(
            normalize_to_iso8601("2026-01-01T00:00:00.000Z"),
            "2026-01-01T00:00:00.000Z"
        );
        // 空字符串 → epoch
        assert_eq!(normalize_to_iso8601(""), "1970-01-01T00:00:00.000Z");
    }
}
