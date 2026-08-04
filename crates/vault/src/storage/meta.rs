//! vault_meta 表的薄包装（直接转发 infra）。
//!
//! **meta 写锁**（复审 #2 修复，2026-07-19）：两个写函数（save_vault_meta /
//! update_security_stamp）内部都自动 `acquire_meta_write_lock()`——覆盖所有 meta
//! 写路径（含 change_master_password / refresh_app_key_local_enc /
//! regenerate_security_stamp / setup_vault），调用方无需显式持锁。
//! ReentrantMutex 让外层 RMW 已持锁时内层 save 再次 lock 不死锁。

use anyhow::Result;
use octopus_infra::db::{self, VaultMeta, VaultMetaInput};

pub fn read_vault_meta() -> Result<Option<VaultMeta>> {
    db::load_vault_meta()
}

pub fn save_vault_meta(input: &VaultMetaInput) -> Result<()> {
    // 锁下沉到写函数内部——覆盖所有调用路径（复审 #2）。
    // ReentrantMutex 让 change_master_password 外层已持锁时内层再次 lock 不死锁。
    let _guard = crate::meta_lock::acquire_meta_write_lock();
    db::upsert_vault_meta(input)
}

pub fn update_security_stamp(stamp: &str) -> Result<()> {
    // 同 save_vault_meta——单字段 UPDATE 也加锁，防与整行覆盖写交错丢字段。
    let _guard = crate::meta_lock::acquire_meta_write_lock();
    db::update_vault_security_stamp(stamp)
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

    /// update_security_stamp 多次连续更新：每次都覆盖前值。
    /// 固化「单行表 + UPDATE WHERE id=1」的不变量（不会插新行）。
    #[test]
    fn update_security_stamp_overwrites_previous_value() {
        setup_clean_db();
        save_vault_meta(&sample_input()).expect("save");

        update_security_stamp("stamp-A").expect("update A");
        assert_eq!(
            read_vault_meta().unwrap().unwrap().security_stamp,
            "stamp-A"
        );

        update_security_stamp("stamp-B").expect("update B");
        assert_eq!(
            read_vault_meta().unwrap().unwrap().security_stamp,
            "stamp-B"
        );

        // 仍只有一行
        let loaded = read_vault_meta().unwrap().unwrap();
        assert_eq!(loaded.id, 1, "单行表，多次 update 不应新增行");
    }

    /// update_security_stamp 在无 vault_meta 行时的行为：
    /// 底层 SQL `UPDATE vault_meta SET ... WHERE id = 1` 不匹配任何行 → 静默 no-op，
    /// 返回 Ok(())（不报错）。这是当前实现细节，本测试固化之——避免未来误改为
    /// "无行时报错"，破坏解锁流程（解锁流程会先 read_vault_meta 判定是否存在，
    /// 此处只是兜底）。
    #[test]
    fn update_security_stamp_on_missing_meta_is_silent_noop() {
        setup_clean_db();
        // 无 vault_meta 行（read_vault_meta 应返回 None）
        assert!(read_vault_meta().unwrap().is_none(), "前置：无 vault_meta 行");

        // update 应返回 Ok（不报错）
        let result = update_security_stamp("anything");
        assert!(result.is_ok(), "无 vault_meta 行时 update 应 Ok（静默 no-op）");

        // 验证确实没插入新行
        assert!(
            read_vault_meta().unwrap().is_none(),
            "update 不应在无行时插入新行"
        );
    }

    /// update_security_stamp 空 stamp 字面量：合法（不校验非空），覆盖原有值。
    #[test]
    fn update_security_stamp_accepts_empty_string() {
        setup_clean_db();
        save_vault_meta(&sample_input()).expect("save");
        assert_eq!(
            read_vault_meta().unwrap().unwrap().security_stamp,
            "stamp-1"
        );

        update_security_stamp("").expect("update with empty");
        assert_eq!(
            read_vault_meta().unwrap().unwrap().security_stamp,
            "",
            "空 stamp 应被原样写入"
        );
    }
}
