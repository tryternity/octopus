//! Bitwarden unencrypted JSON 导入。
//!
//! 仅支持 type=1 (Login)。
//! 加密导出（encrypted=true）不支持（MVP）。

use std::collections::HashSet;

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
    /// Bitwarden reprompt（0=None, 1=Password）。修复 #4：之前 serde 静默丢失，
    /// 落库硬编码 None。`#[serde(default)]` 保证旧导出（无此字段）仍兼容。
    #[serde(default)]
    reprompt: i64,
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

/// 去重 key：spec §6.1 / INV-I3 要求「按 name + 第一条 uri 去重」。
///
/// `(name, first_uri)`：first_uri 取 `login.uris[0].uri`；无 login / 无 uri 时为 None。
/// 这样能精确匹配 `import_bitwarden_json` 的输入与已落库 cipher——后者按
/// `Cipher` 结构在 [`cipher_dedup_key`] 中算同样的 key。
fn dedup_key(item: &BitwardenItem) -> (String, Option<String>) {
    let first_uri = item
        .login
        .as_ref()
        .and_then(|l| l.uris.first())
        .map(|u| u.uri.clone());
    (item.name.clone(), first_uri)
}

/// 已落库 Cipher → dedup key（与 [`dedup_key`] 对称，spec §6.1 / INV-I3）。
///
/// 与 `dedup_key(BitwardenItem)` 保持完全一致的 key 构造规则——
/// name 取明文，first_uri 取 `login.uris[0].uri`（无则 None）。
/// 这是 #2 重复导入判定的不变量。
fn cipher_dedup_key(c: &crate::types::Cipher) -> (String, Option<String>) {
    let first_uri = match &c.data {
        CipherData::Login(l) => l.uris.first().map(|u| u.uri.clone()),
    };
    (c.name.clone(), first_uri)
}

pub fn import_bitwarden_json(json: &str, key: &DerivedKey) -> Result<ImportReport> {
    let export: BitwardenExport = serde_json::from_str(json).context("JSON 解析失败")?;
    ensure!(!export.encrypted, "不支持加密导出（仅 unencrypted JSON）");

    // 修复 #2：先读出库内已有 cipher，按 (name, first_uri) 建索引避免重复落库。
    // spec §6.1 / INV-I3 要求「按 name + 第一条 uri 去重」——重复导入同一份 JSON
    // 不应让条目数翻倍。
    //
    // O2 修复（第五轮审查）：必须显式跳过 `deleted_at.is_some()` 的行（软删/回收站）。
    // `storage::list_ciphers` 不过滤软删行（设计如此——回收站视图需要列出它们），
    // 但 dedup 不应把软删项算进 seen，否则用户软删后再导入同一份备份会被静默 skip，
    // 永远无法通过导入恢复。
    let existing = storage::list_ciphers(key)
        .map(|(ciphers, _failures)| {
            // 注意：list_ciphers 返回的是已解密 Cipher（含明文 name + uris），
            // 我们直接基于明文重算 dedup key 即可，不需要重新解密。
            ciphers
                .into_iter()
                .filter(|c| c.deleted_at.is_none())
                .map(|c| cipher_dedup_key(&c))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut seen: HashSet<(String, Option<String>)> = existing;
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

        // #2 去重：相同 (name, first_uri) 已存在（库内或本轮）→ 跳过。
        let key_tuple = dedup_key(item);
        if !seen.insert(key_tuple.clone()) {
            skipped += 1;
            continue;
        }

        // #4：从导入字段读 reprompt，不再硬编码 None。
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
            reprompt: RepromptType::from(item.reprompt),
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
    use crate::types::RepromptType;
    use octopus_infra::db;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    /// 注入干净 in-memory DB（含 vault_ciphers 表，无数据）——与 cipher.rs 测试一致。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
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

    /// #2：同一份 JSON 导入两次，第二次应全部 skipped（不翻倍）。
    ///
    /// spec §6.1 / INV-I3：按 (name, first_uri) 去重。
    #[test]
    fn test_import_dedup_on_second_import() {
        setup_clean_db();
        let key = make_key(7);
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "GitHub",
                    "type": 1,
                    "login": {"username": "u", "password": "p",
                              "uris": [{"uri": "https://github.com"}]}
                },
                {
                    "name": "GitLab",
                    "type": 1,
                    "login": {"username": "u", "password": "p",
                              "uris": [{"uri": "https://gitlab.com"}]}
                }
            ]
        }"#;

        // 第一次：全部导入
        let r1 = import_bitwarden_json(json, &key).expect("first import");
        assert_eq!(r1.imported, 2, "首次导入 2 条全部新增");
        assert_eq!(r1.skipped, 0);

        // 第二次：同样的 JSON —— 应全部去重跳过，库内不翻倍
        let r2 = import_bitwarden_json(json, &key).expect("second import");
        assert_eq!(r2.imported, 0, "重复导入不应新增");
        assert_eq!(r2.skipped, 2, "应跳过 2 条已存在");

        // 校验库内确实只有 2 行
        let (ciphers, _) = storage::list_ciphers(&key).expect("list");
        assert_eq!(ciphers.len(), 2, "去重后库内应有 2 条，不应翻倍");
    }

    /// #2 补充：不同 name 或不同 first_uri 视为不同条目，应分别入库。
    #[test]
    fn test_import_distinct_keys_both_added() {
        setup_clean_db();
        let key = make_key(8);
        let json = r#"{
            "encrypted": false,
            "items": [
                {"name": "A", "type": 1,
                 "login": {"uris": [{"uri": "https://a.com"}]}},
                {"name": "B", "type": 1,
                 "login": {"uris": [{"uri": "https://b.com"}]}}
            ]
        }"#;
        let r = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(r.imported, 2);
        assert_eq!(r.skipped, 0);
    }

    /// #4：导入含 `reprompt: 1` 的 JSON，落库 cipher.reprompt 应为 Password。
    #[test]
    fn test_import_reprompt_password_persists() {
        setup_clean_db();
        let key = make_key(9);
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "Sensitive",
                    "type": 1,
                    "reprompt": 1,
                    "login": {"username": "u", "password": "p",
                              "uris": [{"uri": "https://example.com"}]}
                }
            ]
        }"#;

        let r = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(r.imported, 1);

        let (ciphers, _) = storage::list_ciphers(&key).expect("list");
        assert_eq!(ciphers.len(), 1);
        assert_eq!(
            ciphers[0].reprompt,
            RepromptType::Password,
            "reprompt=1 应落库为 Password（修复 #4：不再硬编码 None）"
        );
    }

    /// #4 补充：缺省 / reprompt=0 落库为 None（向后兼容）。
    #[test]
    fn test_import_reprompt_default_is_none() {
        setup_clean_db();
        let key = make_key(10);
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "Normal",
                    "type": 1,
                    "login": {"uris": [{"uri": "https://example.com"}]}
                }
            ]
        }"#;
        let r = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(r.imported, 1);
        let (ciphers, _) = storage::list_ciphers(&key).expect("list");
        assert_eq!(ciphers[0].reprompt, RepromptType::None);
    }

    /// O2（第五轮审查）：软删除某条 cipher 后，再次导入同一份 JSON 应**重新导入**，
    /// 不应被静默 skip。
    ///
    /// 旧实现：`storage::list_ciphers` 不过滤 deleted_at（设计如此，回收站视图需要），
    /// dedup 把软删项也算进 seen → 用户软删后想通过重新导入恢复，会被去重逻辑挡住。
    /// 修复：importer 在算 dedup seen 时显式 filter `deleted_at.is_none()`。
    #[test]
    fn test_import_after_soft_delete_re_imports() {
        setup_clean_db();
        let key = make_key(11);
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "GitHub",
                    "type": 1,
                    "login": {"username": "u", "password": "p",
                              "uris": [{"uri": "https://github.com"}]}
                }
            ]
        }"#;

        // 第一次导入：1 条新增
        let r1 = import_bitwarden_json(json, &key).expect("first import");
        assert_eq!(r1.imported, 1);

        // 软删除该条（→ 回收站，deleted_at=Some）
        let (ciphers, _) = storage::list_ciphers(&key).expect("list after import");
        assert_eq!(ciphers.len(), 1);
        let id = ciphers[0].id;
        storage::soft_delete(id).expect("soft delete");

        // 第二次导入同一份 JSON —— 应重新导入（不被软删行去重）
        let r2 = import_bitwarden_json(json, &key).expect("second import after soft delete");
        assert_eq!(
            r2.imported, 1,
            "O2 修复：软删后再次导入应重新入库，不应被静默 skip"
        );

        // 校验：库内应有 2 行（1 软删 + 1 新），未软删的有 1 行
        let (all, _) = storage::list_ciphers(&key).expect("list final");
        assert_eq!(all.len(), 2, "应有 2 行（软删 1 + 新 1）");
        let live: Vec<_> = all.iter().filter(|c| c.deleted_at.is_none()).collect();
        assert_eq!(live.len(), 1, "应有 1 行未软删");
    }
}
