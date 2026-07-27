//! vault_folders 表的高层 API：folder 名加解密 + CRUD。
//!
//! folder.name 与 cipher.name 一致——存密文（`v1:<base64(...)>`，user_vault_key 加密），
//! 由本模块在边界做加解密；DB / 上层只见到明文（[`FolderDto::name`]）。
//!
//! follow-up #6 修复：之前 `create_folder(name)` 接收明文直接写库（MVP UI 未使用），
//! 现在所有写路径都强制走 `key.encrypt()`。

use anyhow::Result;
use octopus_infra::db::{self, VaultFolder};

use crate::crypto::DerivedKey;

/// 文件夹 DTO：name 已解密为明文。
///
/// `id` / `sort_order` / 时间戳直接透传自 DB row。
///
/// 2026-07-21 v44：id 从 i64 改 String（UUID 字符串）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: String,
    /// 已解密的明文名称（DB 中存的是 `v1:` 前缀密文）。
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 列出所有 folder（按 sort_order ASC）。name 自动解密。
///
/// **单行容错**（修复 #9）：照搬 `cipher::list_ciphers` 修复 #6 的模式——
/// 单行解密失败不让整表 Err，坏行记 log + 收集到 `failures` 返回，
/// 调用方可 toast 提示用户「X 个文件夹解密失败已跳过」。
///
/// 失败常见原因：DB 部分写入、bit-flip、跨版本迁移残留、name 字段损坏。
/// 任一都不应让用户看不到其他完好的 N-1 个文件夹。
///
/// 返回 `(成功的 folder 列表, 失败的 folder_id 列表)`。
pub fn list_folders(key: &DerivedKey) -> Result<(Vec<FolderDto>, Vec<String>)> {
    let rows = db::list_vault_folders()?;
    let mut out = Vec::with_capacity(rows.len());
    let mut failures: Vec<String> = Vec::new();
    for row in rows {
        match row_to_dto(&row, key) {
            Ok(dto) => out.push(dto),
            Err(e) => {
                log::warn!(
                    "folder id={} 解密失败，已跳过（其他文件夹继续可见）：{}",
                    row.id,
                    e
                );
                failures.push(row.id);
            }
        }
    }
    Ok((out, failures))
}

/// 创建 folder。`name` 是明文；内部用 `key.encrypt()` 加密后写库。
///
/// 调用方必须先生成 UUID 传入（2026-07-21 v44：不再 AUTOINCREMENT）。
pub fn create_folder(id: &str, name: &str, key: &DerivedKey) -> Result<()> {
    let encrypted = key.encrypt(name.as_bytes())?;
    // 新建 folder sort_order=0（db.sql DEFAULT）——md5 用 0 算
    let md5 = crate::sync::fingerprint::folder_md5_from_fields(id, &encrypted, 0);
    Ok(db::insert_vault_folder(id, &encrypted, &md5)?)
}

/// 重命名 folder。`new_name` 是明文；内部加密后写库。
pub fn rename_folder(id: &str, new_name: &str, key: &DerivedKey) -> Result<()> {
    let encrypted = key.encrypt(new_name.as_bytes())?;
    // rename 不改 sort_order——读当前值算 md5（之前 unwrap_or_default 吞 DB 错误，
    // 失败时 sort_order 错算为 0 导致 sync_md5 不准——#6 修复）
    let sort_order = db::list_vault_folders()?
        .into_iter()
        .find(|f| f.id == id)
        .map(|f| f.sort_order)
        .unwrap_or(0);
    let md5 = crate::sync::fingerprint::folder_md5_from_fields(id, &encrypted, sort_order);
    Ok(db::update_vault_folder_name(id, &encrypted, &md5)?)
}

/// 删除 folder。
///
/// vault_ciphers.folder_id 的 FK 是 `ON DELETE SET NULL`——本文件夹下的 cipher
/// 不会被删，只是 folder_id 被置为 NULL（回到根目录）。
pub fn delete_folder(id: &str) -> Result<()> {
    Ok(db::delete_vault_folder(id)?)
}

/// 把 DB row 解密成 DTO（共享给 list_folders / 后续可能的单点查询）。
fn row_to_dto(row: &VaultFolder, key: &DerivedKey) -> Result<FolderDto> {
    let name_bytes = key.decrypt(&row.name)?;
    let name = String::from_utf8(name_bytes.to_vec())?;
    Ok(FolderDto {
        id: row.id.clone(),
        name,
        sort_order: row.sort_order,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_infra::db;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey::from_raw([byte; 32])
    }

    /// 注入干净 in-memory DB（含 vault_folders 表，无数据）。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
    }

    /// 全新库无 folder 行 → list_folders 返回空 Vec。
    #[test]
    fn list_folders_empty_initially() {
        setup_clean_db();
        let key = make_key(1);
        let (folders, failures) = list_folders(&key).expect("list should succeed");
        assert!(folders.is_empty(), "fresh DB should have no folders");
        assert!(failures.is_empty(), "fresh DB 应无失败行");
    }

    /// create_folder + list_folders 往返：明文名 → 加密入库 → 解密回明文。
    ///
    /// 这是 follow-up #6 的核心断言：name 在 DB 中是密文，但 API 层只见明文。
    #[test]
    fn create_folder_then_list_round_trips_with_encryption() {
        setup_clean_db();
        let key = make_key(7);

        let id = "11111111-1111-4111-8111-111111111111";
        create_folder(id, "工作", &key).expect("create should succeed");

        let (folders, failures) = list_folders(&key).expect("list should succeed");
        assert!(failures.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, id);
        assert_eq!(folders[0].name, "工作", "name 应被还原为明文");
        assert_eq!(folders[0].sort_order, 0, "default sort_order is 0");

        // 同时确认 DB 里存的不是明文（防 regression）
        let raw: Vec<octopus_infra::db::VaultFolder> = db::list_vault_folders().expect("raw list");
        assert_eq!(raw.len(), 1);
        assert!(
            raw[0].name.starts_with("v1:"),
            "DB 中应存密文，got: {}",
            raw[0].name
        );
        assert_ne!(raw[0].name, "工作", "DB 不应存明文");
    }

    /// 插入多个 folder：list 顺序按 sort_order（默认全 0，再按 id ASC）。
    #[test]
    fn create_multiple_folders_lists_all() {
        setup_clean_db();
        let key = make_key(2);

        let id_a = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let id_b = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let id_c = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
        create_folder(id_a, "alpha", &key).expect("create a");
        create_folder(id_b, "beta", &key).expect("create b");
        create_folder(id_c, "gamma", &key).expect("create c");

        let (folders, failures) = list_folders(&key).expect("list");
        assert!(failures.is_empty());
        assert_eq!(folders.len(), 3);
        let names: Vec<String> = folders.iter().map(|f| f.name.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert!(names.contains(&"gamma".to_string()));
        let ids: Vec<String> = folders.iter().map(|f| f.id.clone()).collect();
        assert!(ids.contains(&id_a.to_string()));
        assert!(ids.contains(&id_b.to_string()));
        assert!(ids.contains(&id_c.to_string()));
    }

    /// rename_folder：明文新名 → 加密入库 → list 看到新明文。
    #[test]
    fn rename_folder_round_trips() {
        setup_clean_db();
        let key = make_key(3);

        let id = "33333333-3333-4333-8333-333333333333";
        create_folder(id, "old", &key).expect("create");
        rename_folder(id, "new name", &key).expect("rename");

        let (folders, failures) = list_folders(&key).expect("list");
        assert!(failures.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "new name");
    }

    /// delete_folder：删除后 list 应不再包含；DB 行也确实被删。
    #[test]
    fn delete_folder_removes_row() {
        setup_clean_db();
        let key = make_key(4);

        let id_a = "44444444-4444-4444-8444-444444444444";
        let id_b = "55555555-5555-4555-8555-555555555555";
        create_folder(id_a, "keep", &key).expect("create a");
        create_folder(id_b, "drop", &key).expect("create b");
        delete_folder(id_b).expect("delete");

        let (folders, failures) = list_folders(&key).expect("list");
        assert!(failures.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, id_a);
        assert_eq!(folders[0].name, "keep");
    }

    /// 用错误的 key 解密 folder.name：修复 #9 后整表不再 Err，
    /// 而是返回 `(空 Vec, 含该 id 的 failures)`——坏行跳过，其他行仍可见。
    #[test]
    fn list_folders_with_wrong_key_skips_bad_row() {
        setup_clean_db();
        let key1 = make_key(10);
        let key2 = make_key(11);

        let id = "10101010-1010-4110-8110-101010101010";
        create_folder(id, "secret-folder", &key1).expect("create");
        let (folders, failures) = list_folders(&key2).expect(
            "修复 #9 后错误 key 不应让整表 Err，而是返回部分结果",
        );
        assert!(
            folders.is_empty(),
            "错误 key 下无行可解密，folders 应为空"
        );
        assert_eq!(failures, vec![id.to_string()], "失败行 id 应进 failures");
    }

    /// #9 容错核心场景：库内 2 行，1 行用 key1 加密、1 行用 key2 加密，
    /// 用 key1 list 时：key2 加密的行应进 failures，key1 行仍正常返回。
    #[test]
    fn list_folders_partial_failure_keeps_other_rows() {
        setup_clean_db();
        let key1 = make_key(20);
        let key2 = make_key(21);

        let id_ok = "20202020-2020-4220-8220-202020202020";
        let id_bad = "21212121-2121-4321-8321-212121212121";
        create_folder(id_ok, "good", &key1).expect("create good");
        create_folder(id_bad, "corrupted", &key2).expect("create bad");

        let (folders, failures) = list_folders(&key1).expect("partial result");
        assert_eq!(folders.len(), 1, "key1 应能解密自己加密的行");
        assert_eq!(folders[0].id, id_ok);
        assert_eq!(folders[0].name, "good");
        assert_eq!(failures, vec![id_bad.to_string()], "key2 加密的行应进 failures");
    }
}
