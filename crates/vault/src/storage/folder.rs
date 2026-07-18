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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FolderDto {
    pub id: i64,
    /// 已解密的明文名称（DB 中存的是 `v1:` 前缀密文）。
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 列出所有 folder（按 sort_order ASC）。name 自动解密。
pub fn list_folders(key: &DerivedKey) -> Result<Vec<FolderDto>> {
    let rows = db::list_vault_folders()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_dto(&row, key)?);
    }
    Ok(out)
}

/// 创建 folder。`name` 是明文；内部用 `key.encrypt()` 加密后写库。
///
/// 返回新插入行的 id。
pub fn create_folder(name: &str, key: &DerivedKey) -> Result<i64> {
    let encrypted = key.encrypt(name.as_bytes())?;
    Ok(db::insert_vault_folder(&encrypted)?)
}

/// 重命名 folder。`new_name` 是明文；内部加密后写库。
pub fn rename_folder(id: i64, new_name: &str, key: &DerivedKey) -> Result<()> {
    let encrypted = key.encrypt(new_name.as_bytes())?;
    Ok(db::update_vault_folder_name(id, &encrypted)?)
}

/// 删除 folder。
///
/// vault_ciphers.folder_id 的 FK 是 `ON DELETE SET NULL`——本文件夹下的 cipher
/// 不会被删，只是 folder_id 被置为 NULL（回到根目录）。
pub fn delete_folder(id: i64) -> Result<()> {
    Ok(db::delete_vault_folder(id)?)
}

/// 把 DB row 解密成 DTO（共享给 list_folders / 后续可能的单点查询）。
fn row_to_dto(row: &VaultFolder, key: &DerivedKey) -> Result<FolderDto> {
    let name_bytes = key.decrypt(&row.name)?;
    let name = String::from_utf8(name_bytes.to_vec())?;
    Ok(FolderDto {
        id: row.id,
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
        DerivedKey(crate::Zeroizing::new([byte; 32]))
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
        let folders = list_folders(&key).expect("list should succeed");
        assert!(folders.is_empty(), "fresh DB should have no folders");
    }

    /// create_folder + list_folders 往返：明文名 → 加密入库 → 解密回明文。
    ///
    /// 这是 follow-up #6 的核心断言：name 在 DB 中是密文，但 API 层只见明文。
    #[test]
    fn create_folder_then_list_round_trips_with_encryption() {
        setup_clean_db();
        let key = make_key(7);

        let id = create_folder("工作", &key).expect("create should succeed");
        assert!(id > 0, "new folder id should be positive");

        let folders = list_folders(&key).expect("list should succeed");
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

        let id_a = create_folder("alpha", &key).expect("create a");
        let id_b = create_folder("beta", &key).expect("create b");
        let id_c = create_folder("gamma", &key).expect("create c");

        let folders = list_folders(&key).expect("list");
        assert_eq!(folders.len(), 3);
        let names: Vec<String> = folders.iter().map(|f| f.name.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert!(names.contains(&"gamma".to_string()));
        let ids: Vec<i64> = folders.iter().map(|f| f.id).collect();
        assert!(ids.contains(&id_a));
        assert!(ids.contains(&id_b));
        assert!(ids.contains(&id_c));
    }

    /// rename_folder：明文新名 → 加密入库 → list 看到新明文。
    #[test]
    fn rename_folder_round_trips() {
        setup_clean_db();
        let key = make_key(3);

        let id = create_folder("old", &key).expect("create");
        rename_folder(id, "new name", &key).expect("rename");

        let folders = list_folders(&key).expect("list");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "new name");
    }

    /// delete_folder：删除后 list 应不再包含；DB 行也确实被删。
    #[test]
    fn delete_folder_removes_row() {
        setup_clean_db();
        let key = make_key(4);

        let id_a = create_folder("keep", &key).expect("create a");
        let id_b = create_folder("drop", &key).expect("create b");
        delete_folder(id_b).expect("delete");

        let folders = list_folders(&key).expect("list");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, id_a);
        assert_eq!(folders[0].name, "keep");
    }

    /// 用错误的 key 解密 folder.name 应失败（验证 name 确实是被加密过的）。
    #[test]
    fn list_folders_with_wrong_key_fails() {
        setup_clean_db();
        let key1 = make_key(10);
        let key2 = make_key(11);

        create_folder("secret-folder", &key1).expect("create");
        let result = list_folders(&key2);
        assert!(
            result.is_err(),
            "用错误 key 解密应失败——证明 name 确实被加密"
        );
    }
}
