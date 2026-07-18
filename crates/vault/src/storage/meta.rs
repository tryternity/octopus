//! vault_meta 表的薄包装（直接转发 infra）。

use anyhow::Result;
use octopus_infra::db::{self, VaultMeta, VaultMetaInput};

pub fn read_vault_meta() -> Result<Option<VaultMeta>> {
    Ok(db::load_vault_meta()?)
}

pub fn save_vault_meta(input: &VaultMetaInput) -> Result<()> {
    Ok(db::upsert_vault_meta(input)?)
}

pub fn update_security_stamp(stamp: &str) -> Result<()> {
    Ok(db::update_vault_security_stamp(stamp)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 为当前测试线程注入一份干净的 in-memory DB（schema 已建、无 vault_meta 行）。
    /// 测试结束 clear_test_db 恢复全局路径。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
    }

    /// 构造一份合法的 VaultMetaInput（字段值随意但合法）。
    fn sample_input() -> VaultMetaInput {
        VaultMetaInput {
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:protected-uvk".into(),
            app_key_local_enc: "v1:local-enc".into(),
            app_key_sync_enc: "v1:sync-enc".into(),
            security_stamp: "stamp-1".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
        }
    }

    /// 全新库（无 vault_meta 行）→ read_vault_meta 应返回 None。
    #[test]
    fn read_vault_meta_returns_none_when_empty() {
        setup_clean_db();
        let result = read_vault_meta().expect("read should succeed");
        assert!(result.is_none(), "fresh DB should have no vault_meta row");
    }

    /// save_vault_meta + read_vault_meta 往返：写入后应能读回完整字段。
    #[test]
    fn save_vault_meta_then_read_round_trips() {
        setup_clean_db();
        let input = sample_input();
        save_vault_meta(&input).expect("save should succeed");

        let loaded = read_vault_meta()
            .expect("read should succeed")
            .expect("vault_meta row should exist after save");
        assert_eq!(loaded.id, 1, "single-row table, id always 1");
        assert_eq!(loaded.kdf_type, 0);
        assert_eq!(loaded.kdf_salt, vec![1u8; 32]);
        assert_eq!(loaded.kdf_iterations, 3);
        assert_eq!(loaded.kdf_memory_kib, 65_536);
        assert_eq!(loaded.kdf_parallelism, 4);
        assert_eq!(loaded.protected_user_vault_key, "v1:protected-uvk");
        assert_eq!(loaded.app_key_local_enc, "v1:local-enc");
        assert_eq!(loaded.app_key_sync_enc, "v1:sync-enc");
        assert_eq!(loaded.security_stamp, "stamp-1");
        assert_eq!(loaded.equivalent_domains, "[]");
        assert!(loaded.public_key.is_none());
        assert!(loaded.protected_private_key.is_none());
    }

    /// 二次 save（upsert）：security_stamp 被覆盖，仍是单行。
    #[test]
    fn save_vault_meta_upserts_single_row() {
        setup_clean_db();
        let mut input = sample_input();
        save_vault_meta(&input).expect("first save");

        input.security_stamp = "stamp-2".into();
        save_vault_meta(&input).expect("second save (upsert)");

        let loaded = read_vault_meta().expect("read").expect("row exists");
        assert_eq!(loaded.security_stamp, "stamp-2");
    }

    /// update_security_stamp：仅改 stamp 字段，其他字段保持。
    #[test]
    fn update_security_stamp_changes_only_stamp() {
        setup_clean_db();
        let input = sample_input();
        save_vault_meta(&input).expect("save");

        update_security_stamp("new-stamp-xyz").expect("update stamp");

        let loaded = read_vault_meta().expect("read").expect("row exists");
        assert_eq!(loaded.security_stamp, "new-stamp-xyz");
        // 其他字段不应被改动
        assert_eq!(loaded.kdf_iterations, 3);
        assert_eq!(loaded.app_key_local_enc, "v1:local-enc");
    }
}
