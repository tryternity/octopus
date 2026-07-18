//! vault_folders 表的薄包装（MVP UI 不暴露，但提供 API）。

use anyhow::Result;
use octopus_infra::db::{self, VaultFolder};

pub fn list_folders() -> Result<Vec<VaultFolder>> {
    Ok(db::list_vault_folders()?)
}

/// 注意：name 应由调用者先用 user_vault_key.encrypt() 加密后再传入。
/// MVP UI 不使用，故不在 storage 层做加密。
pub fn create_folder(name_encrypted: &str) -> Result<i64> {
    Ok(db::insert_vault_folder(name_encrypted)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入干净 in-memory DB（含 vault_folders 表，无数据）。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
    }

    /// 全新库无 folder 行 → list_folders 返回空 Vec。
    #[test]
    fn list_folders_empty_initially() {
        setup_clean_db();
        let folders = list_folders().expect("list should succeed");
        assert!(folders.is_empty(), "fresh DB should have no folders");
    }

    /// create_folder + list_folders 往返：插入后应能列出，且 name 与 sort_order 正确。
    #[test]
    fn create_folder_then_list_round_trips() {
        setup_clean_db();
        let id = create_folder("v1:enc-folder-1").expect("create should succeed");
        assert!(id > 0, "new folder id should be positive");

        let folders = list_folders().expect("list should succeed");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, id);
        assert_eq!(folders[0].name, "v1:enc-folder-1");
        assert_eq!(folders[0].sort_order, 0, "default sort_order is 0");
    }

    /// 插入多个 folder：list 顺序按 sort_order（默认全 0，再按 id）。
    #[test]
    fn create_multiple_folders_lists_all() {
        setup_clean_db();
        let id_a = create_folder("v1:enc-a").expect("create a");
        let id_b = create_folder("v1:enc-b").expect("create b");
        let id_c = create_folder("v1:enc-c").expect("create c");

        let folders = list_folders().expect("list");
        assert_eq!(folders.len(), 3);
        let ids: Vec<i64> = folders.iter().map(|f| f.id).collect();
        assert!(ids.contains(&id_a));
        assert!(ids.contains(&id_b));
        assert!(ids.contains(&id_c));
    }
}
