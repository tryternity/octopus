//! Bitwarden unencrypted JSON 导入。
//!
//! 仅支持 type=1 (Login)。
//! 加密导出（encrypted=true）不支持（MVP）。

use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::crypto::DerivedKey;
use crate::storage;
use octopus_infra::db::{self, VaultCipherInput};
use crate::types::{
    CipherData, CipherInput, CipherType, Field, LoginData, LoginUri, MatchType, RepromptType,
};

#[derive(Debug, Deserialize)]
struct BitwardenExport {
    encrypted: bool,
    #[serde(default)]
    items: Vec<BitwardenItem>,
    /// M6 修复：folders 数组（之前导入端不解析）。`#[serde(default)]` 保证旧导出兼容。
    #[serde(default)]
    folders: Vec<BitwardenFolder>,
}

/// Bitwarden folder（M6 修复）。
#[derive(Debug, Deserialize)]
struct BitwardenFolder {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

/// Bitwarden 密码历史条目（M6 修复）。
#[derive(Debug, Deserialize)]
struct BitwardenPasswordHistory {
    #[serde(default)]
    password: String,
    #[serde(default)]
    #[serde(rename = "lastUsedDate")]
    last_used_date: String,
}

#[derive(Debug, Deserialize)]
struct BitwardenItem {
    #[serde(default)]
    name: String,
    /// M6 修复：folderId 引用 folders.id（之前导入端不解析 → 丢失文件夹归属）。
    #[serde(default)]
    #[serde(rename = "folderId")]
    folder_id: Option<String>,
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
    /// M6 修复：密码历史（之前导入端不解析 → 清空）。
    #[serde(default)]
    #[serde(rename = "passwordHistory")]
    password_history: Vec<BitwardenPasswordHistory>,
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
#[serde(rename_all = "camelCase")]
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
    // O2 修复（第五轮审查）：必须显式跳过 `is_deleted` 的行（软删/回收站）。
    // `storage::list_ciphers` 不过滤软删行（设计如此——回收站视图需要列出它们），
    // 但 dedup 不应把软删项算进 seen，否则用户软删后再导入同一份备份会被静默 skip，
    // 永远无法通过导入恢复。
    let existing = storage::list_ciphers(key)
        .map(|(ciphers, _failures)| {
            // 注意：list_ciphers 返回的是已解密 Cipher（含明文 name + uris），
            // 我们直接基于明文重算 dedup key 即可，不需要重新解密。
            ciphers
                .into_iter()
                .filter(|c| c.is_deleted == 0)
                .map(|c| cipher_dedup_key(&c))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut seen: HashSet<(String, Option<String>)> = existing;
    let mut skipped = 0;
    let mut errors: Vec<String> = Vec::new();

    // M6 修复（2026-07-24）：导入 folders——建「导出 folderId → 本机 folder_id」映射。
    // M7 修复（2026-07-24）：folder 必须先于 cipher 创建（cipher.folder_id 是 FK），
    // 但记录新建的 folder_id——若 cipher batch 失败则补偿删除（避免空 folder 残留）。
    // L20 修复（2026-07-24）：循环外 list_folders 一次建 name→id HashMap（不再 N+1）。
    let mut existing_folder_names: std::collections::HashMap<String, String> = storage::list_folders(key)
        .map(|(folders, _)| {
            folders
                .into_iter()
                .map(|f| (f.name, f.id))
                .collect()
        })
        .unwrap_or_default();

    let mut folder_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut created_folder_ids: Vec<String> = Vec::new(); // M7：batch 失败时补偿删除
    for f in &export.folders {
        if f.name.is_empty() {
            continue;
        }
        // 同名 folder 复用现有 UUID（避免重复建）
        if let Some(existing_id) = existing_folder_names.get(&f.name) {
            folder_map.insert(f.id.clone(), existing_id.clone());
        } else {
            let new_id = uuid::Uuid::new_v4().to_string();
            match storage::create_folder(&new_id, &f.name, key) {
                Ok(()) => {
                    folder_map.insert(f.id.clone(), new_id.clone());
                    // N1 修复（2026-07-24）：回填到 existing_folder_names——
                    // 同次导入内若有两个同名 export folder（不同 export id），
                    // 第二个复用首次创建的本机 id（避免重复同名 folder）。
                    existing_folder_names.insert(f.name.clone(), new_id.clone());
                    created_folder_ids.push(new_id); // 记录以便补偿回滚
                }
                Err(e) => {
                    // I-FOLDER-WARN 修复（2026-07-25）：folder 创建失败不仅 log，还记入 errors
                    // ——引用该 folderId 的 cipher 的 folder_id 会静默降级为 None（folder_map
                    // 无此 id → get 返 None），用户丢失文件夹归属却不被告知。记入 errors
                    // 让用户至少能在导入报告里看到。
                    log::warn!("[import] 创建 folder {} 失败：{}", f.name, e);
                    errors.push(format!("创建 folder {} 失败：{}", f.name, e));
                }
            }
        }
    }

    // L8 修复（2026-07-24）：两阶段——先加密收集 Vec（加密失败记 errors 跳过），
    // 再一次性 batch 事务化 insert。既保证 DB 原子性（不会部分入库），又保留
    // 「跳过坏条目」的容错。之前逐条 create_cipher 各自 autocommit，中途失败
    // 留部分数据 + 用户看到「失败」却有数据已入库 → 重导可能重复。
    let mut batch: Vec<VaultCipherInput> = Vec::new();

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

        // #4：从导入字段读 reprompt。M6：folder_id 从映射取 + password_history 从 item 读。
        let input = CipherInput {
            // M6 修复：folderId 映射到本机 folder_id（旧导出无 folderId → None）
            folder_id: item
                .folder_id
                .as_ref()
                .and_then(|fid| folder_map.get(fid).cloned()),
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
            // M6 修复：从导入字段读 passwordHistory（之前硬编码 vec![]）
            password_history: item
                .password_history
                .iter()
                .map(|h| crate::types::PasswordHistoryEntry {
                    password: h.password.clone(),
                    last_used_at: h.last_used_date.clone(),
                })
                .collect(),
            reprompt: RepromptType::from(item.reprompt),
        };

        // 阶段 1：加密 + 算 sync_md5（纯内存，不落库）——失败记 errors 跳过
        let new_id = uuid::Uuid::new_v4().to_string();
        match storage::prepare_cipher_input(&new_id, &input, key) {
            Ok(db_input) => batch.push(db_input),
            Err(e) => {
                errors.push(format!("Item {} ({}): {}", idx, item.name, e));
                skipped += 1;
            }
        }
    }

    // 阶段 2：事务化批量 insert（全成功或全回滚）
    let batch_len = batch.len();
    if let Err(e) = db::insert_vault_ciphers_batch(&batch) {
        // M7 修复：batch 失败时补偿删除已建 folder——避免空 folder 残留。
        // folder 必须先于 cipher 创建（FK 约束），但 batch 失败后它们成孤儿。
        for folder_id in &created_folder_ids {
            if let Err(fe) = storage::delete_folder(folder_id) {
                log::warn!("[import] 补偿删除 folder {} 失败：{}", folder_id, fe);
            }
        }
        return Err(e);
    }

    // I8 修复（2026-07-25）：batch 成功后清理未被任何已入库 cipher 引用的 created folder。
    //
    // M7 只覆盖 batch 返 Err 的场景；但 batch 为空（&[]）时返 Ok(()) 不进 Err 分支。
    // 触发链：用户从 Bitwarden 全量导出（含 SecureNote/Card/Identity），octopus 只
    // 支持 Login（type=1）→ items 全 skip → batch 空 → folder 全残留为孤儿。
    // 即使 batch 非空，也可能有 folder 被创建但无任何 cipher 引用（item 全是其他 folder 的）。
    //
    // 本清理覆盖所有孤儿场景：扫描 batch 里实际被引用的 folder_id，删掉 created 但未引用的。
    if !created_folder_ids.is_empty() {
        let referenced: HashSet<&str> = batch
            .iter()
            .filter_map(|input| input.folder_id.as_deref())
            .collect();
        for folder_id in &created_folder_ids {
            if !referenced.contains(folder_id.as_str()) {
                if let Err(fe) = storage::delete_folder(folder_id) {
                    log::warn!("[import] I8 清理未引用 folder {} 失败：{}", folder_id, fe);
                }
            }
        }
    }

    Ok(ImportReport {
        total: export.items.len(),
        imported: batch_len,
        skipped,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RepromptType;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey::from_raw([byte; 32])
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
    /// 旧实现：`storage::list_ciphers` 不过滤 is_deleted（设计如此，回收站视图需要），
    /// dedup 把软删项也算进 seen → 用户软删后想通过重新导入恢复，会被去重逻辑挡住。
    /// 修复：importer 在算 dedup seen 时显式 filter `!is_deleted`。
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

        // 软删除该条（→ 回收站，is_deleted=true）
        let (ciphers, _) = storage::list_ciphers(&key).expect("list after import");
        assert_eq!(ciphers.len(), 1);
        let id = ciphers[0].id.clone();
        storage::soft_delete(&id).expect("soft delete");

        // 第二次导入同一份 JSON —— 应重新导入（不被软删行去重）
        let r2 = import_bitwarden_json(json, &key).expect("second import after soft delete");
        assert_eq!(
            r2.imported, 1,
            "O2 修复：软删后再次导入应重新入库，不应被静默 skip"
        );

        // 校验：库内应有 2 行（1 软删 + 1 新），未软删的有 1 行
        let (all, _) = storage::list_ciphers(&key).expect("list final");
        assert_eq!(all.len(), 2, "应有 2 行（软删 1 + 新 1）");
        let live: Vec<_> = all.iter().filter(|c| c.is_deleted == 0).collect();
        assert_eq!(live.len(), 1, "应有 1 行未软删");
    }

    /// M6 回归守护：导入含 folders + folderId + passwordHistory 的 JSON——
    /// folder 应被创建 + item 的 folder_id 正确映射 + password_history 存活。
    #[test]
    fn test_import_folders_and_password_history() {
        setup_clean_db();
        let key = make_key(1);
        let json = r#"{
            "encrypted": false,
            "folders": [
                {"id": "export-folder-1", "name": "Social"}
            ],
            "items": [
                {
                    "name": "GitHub",
                    "folderId": "export-folder-1",
                    "favorite": false,
                    "type": 1,
                    "login": {
                        "username": "user",
                        "password": "secret",
                        "uris": [{"uri": "https://github.com", "match": null}]
                    },
                    "passwordHistory": [
                        {"password": "old-pass", "lastUsedDate": "2026-01-01T00:00:00"}
                    ]
                }
            ]
        }"#;
        let report = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(report.imported, 1, "应导入 1 个 item");

        // 验证 folder 被创建
        let (folders, _) = storage::list_folders(&key).expect("list folders");
        assert_eq!(folders.len(), 1, "M6: folder 应被创建");
        assert_eq!(folders[0].name, "Social");

        // 验证 item 的 folder_id 映射到创建的 folder
        let (ciphers, _) = storage::list_ciphers(&key).expect("list ciphers");
        assert_eq!(ciphers.len(), 1);
        let cipher = &ciphers[0];
        assert_eq!(
            cipher.folder_id.as_deref(),
            Some(folders[0].id.as_str()),
            "M6: item 的 folder_id 应映射到创建的 folder"
        );

        // 验证 password_history 存活
        assert_eq!(
            cipher.password_history.len(),
            1,
            "M6: password_history 应存活"
        );
        assert_eq!(cipher.password_history[0].password, "old-pass");
    }

    /// M6 向后兼容：旧导出（无 folders / folderId / passwordHistory）仍能正常导入。
    #[test]
    fn test_import_old_export_without_folders_still_works() {
        setup_clean_db();
        let key = make_key(1);
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "Legacy",
                    "favorite": false,
                    "type": 1,
                    "login": {
                        "username": "user",
                        "password": "pass",
                        "uris": [{"uri": "https://example.com", "match": null}]
                    }
                }
            ]
        }"#;
        let report = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(report.imported, 1, "旧导出应正常导入");

        let (ciphers, _) = storage::list_ciphers(&key).expect("list");
        assert_eq!(ciphers.len(), 1);
        assert!(
            ciphers[0].folder_id.is_none(),
            "旧导出无 folderId → folder_id 应为 None"
        );
        assert!(
            ciphers[0].password_history.is_empty(),
            "旧导出无 passwordHistory → 应为空"
        );
    }

    /// N1 回归守护：同次导入内两个同名 export folder（不同 export id）应只创建一个本机 folder。
    ///
    /// 之前 bug：create_folder 成功后未回填 existing_folder_names → 第二个同名 folder
    /// 仍查不到 → 再创建 → 库内出现重复同名 folder。
    #[test]
    fn test_import_duplicate_named_folders_dedup() {
        setup_clean_db();
        let key = make_key(1);
        // 两个同名 folder（不同 export id f1/f2）
        let json = r#"{
            "encrypted": false,
            "folders": [
                {"id": "f1", "name": "Social"},
                {"id": "f2", "name": "Social"}
            ],
            "items": [
                {
                    "name": "Item1",
                    "folderId": "f1",
                    "favorite": false,
                    "type": 1,
                    "login": {"username": "u1", "password": "p1", "uris": [{"uri": "https://a.com", "match": null}]}
                },
                {
                    "name": "Item2",
                    "folderId": "f2",
                    "favorite": false,
                    "type": 1,
                    "login": {"username": "u2", "password": "p2", "uris": [{"uri": "https://b.com", "match": null}]}
                }
            ]
        }"#;
        let report = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(report.imported, 2, "应导入 2 个 item");

        // N1 核心：只应有 1 个 "Social" folder（不是 2 个）
        let (folders, _) = storage::list_folders(&key).expect("list folders");
        let social_count = folders.iter().filter(|f| f.name == "Social").count();
        assert_eq!(
            social_count, 1,
            "N1: 同名 export folder 应只创建 1 个本机 folder，实际 {}",
            social_count
        );

        // 两个 item 的 folder_id 应映射到同一个本机 folder
        let (ciphers, _) = storage::list_ciphers(&key).expect("list ciphers");
        assert_eq!(ciphers.len(), 2);
        assert_eq!(
            ciphers[0].folder_id, ciphers[1].folder_id,
            "两个 item 的 folder_id 应映射到同一个本机 folder"
        );
    }

    /// I8 回归守护：folders 非空但 items 全部无效（type != 1 / 无 login）→
    /// batch 为空 → folder 不应残留为孤儿。
    ///
    /// 触发场景：用户从 Bitwarden 全量导出（含 SecureNote/Card/Identity），
    /// octopus 只支持 Login → items 全 skip，但 folders 已先行创建。
    /// M7 补偿只覆盖 batch 返 Err；batch 空（Ok）不进 Err 分支 → 之前 folder 残留。
    /// I8 修复：batch 成功后清理未被任何 cipher 引用的 created folder。
    #[test]
    fn test_import_all_items_skipped_no_orphan_folders() {
        setup_clean_db();
        let key = make_key(1);
        // folders 非空，但 items 全是 type=2（SecureNote，octopus 不支持）
        let json = r#"{
            "encrypted": false,
            "folders": [
                {"id": "f1", "name": "Notes"},
                {"id": "f2", "name": "Cards"}
            ],
            "items": [
                {"name": "MyNote", "type": 2, "folderId": "f1", "notes": "secret"},
                {"name": "MyCard", "type": 3, "folderId": "f2"}
            ]
        }"#;
        let report = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(report.imported, 0, "无 Login item 可导入");
        assert_eq!(report.skipped, 2, "2 个非 Login item 被 skip");

        // I8 核心：folder 不应残留（之前会留下 2 个空 folder）
        let (folders, _) = storage::list_folders(&key).expect("list folders");
        assert_eq!(
            folders.len(),
            0,
            "I8: items 全 skip 时 created folder 应被清理，不应残留孤儿（实际 {} 个）",
            folders.len()
        );
    }

    /// I8 补充：部分 folder 被引用、部分未引用 → 只清理未引用的，被引用的保留。
    #[test]
    fn test_import_partial_orphan_folders_cleaned() {
        setup_clean_db();
        let key = make_key(1);
        // f1 有 Login item 引用，f2 只有 SecureNote 引用（item 被 skip）
        let json = r#"{
            "encrypted": false,
            "folders": [
                {"id": "f1", "name": "Logins"},
                {"id": "f2", "name": "Notes"}
            ],
            "items": [
                {"name": "GitHub", "type": 1, "folderId": "f1",
                 "login": {"username": "u", "password": "p", "uris": [{"uri": "https://github.com"}]}},
                {"name": "Note1", "type": 2, "folderId": "f2", "notes": "secret"}
            ]
        }"#;
        let report = import_bitwarden_json(json, &key).expect("import");
        assert_eq!(report.imported, 1, "1 个 Login 导入");
        assert_eq!(report.skipped, 1, "1 个 SecureNote skip");

        let (folders, _) = storage::list_folders(&key).expect("list folders");
        assert_eq!(folders.len(), 1, "应只保留被引用的 Logins folder");
        assert_eq!(folders[0].name, "Logins");

        // cipher 的 folder_id 应指向保留的 folder
        let (ciphers, _) = storage::list_ciphers(&key).expect("list ciphers");
        assert_eq!(ciphers.len(), 1);
        assert_eq!(ciphers[0].folder_id.as_deref(), Some(folders[0].id.as_str()));
    }
}
