//! 一次性迁移：把 models.secret_key 的明文 API Key 用 app_key 加密回写。
//!
//! 触发时机：首次 setup_vault 之后。
//! 规则：
//!   - 仅处理 is_local=0（云端 API Key）的行
//!   - 跳过已 v1: 开头的行（避免重复加密）
//!   - 迁移后字段以 v1: 前缀存密文

use anyhow::Result;
use octopus_infra::db;

use crate::crypto::DerivedKey;

/// 迁移所有未加密的 secret_key。返回迁移的行数。
pub fn migrate_secret_keys_to_encrypted(app_key: &DerivedKey) -> Result<usize> {
    let models = db::list_models_for_secret_migration()?;
    let mut count = 0usize;
    for (model_id, plaintext) in models {
        let encrypted = app_key.encrypt(plaintext.as_bytes())?;
        db::update_model_secret_key(model_id, &encrypted)?;
        count += 1;
        log::info!("迁移 model {} 的 secret_key 为加密格式", model_id);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Zeroizing;
    use octopus_infra::db;
    use rusqlite::params;

    /// 构造一份确定性的 32B DerivedKey（每个 byte 都为 `byte`），用于加解密往返。
    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(Zeroizing::new([byte; 32]))
    }

    /// 为当前测试线程注入一份干净的 in-memory DB（schema 已建，含 seed models 但无云端模型）。
    /// 与 storage/meta.rs / folder.rs 的测试用例一致——thread-local 注入，互不污染。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
    }

    /// 直接向 models 表插一行云端或本地模型，返回新行 id。
    ///
    /// 仅提供 NOT NULL 无默认值的字段（domain / category / model_name / source）；
    /// secret_key / is_local 由参数显式传入；其余列走 schema DEFAULT。
    /// UNIQUE(domain, provider, category, model_name) 通过 model_name 附加随机后缀避免冲突。
    fn insert_test_model(secret_key: &str, is_local: i64) -> i64 {
        // 用 AtomicU64 生成的简单递增 id 作为后缀——避免引入 std::sync::Mutex 全局计数器。
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let suffix = SEQ.fetch_add(1, Ordering::SeqCst);

        db::with_db(|conn| {
            conn.execute(
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, is_local)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    "asr",
                    "test_provider",
                    "test_category",
                    format!("test-model-{}-{}", suffix, is_local),
                    "test-source",
                    secret_key,
                    is_local,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .expect("insert test model should succeed")
    }

    /// 直接按 id 读回 models.secret_key。
    fn read_secret_key(id: i64) -> String {
        db::with_db(|conn| {
            let value: String = conn.query_row(
                "SELECT secret_key FROM models WHERE id = ?",
                params![id],
                |r| r.get(0),
            )?;
            Ok(value)
        })
        .expect("read secret_key should succeed")
    }

    /// 迁移明文密钥：is_local=0 + 非 v1: 前缀的行应被加密；本地模型（is_local=1）跳过。
    #[test]
    fn migrate_encrypts_plaintext_keys() {
        setup_clean_db();
        let key = make_key(1);

        let id1 = insert_test_model("plaintext-api-key-1", 0);
        let id2 = insert_test_model("plaintext-api-key-2", 0);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 2, "both cloud models should be migrated");

        // 迁移后字段均应以 v1: 开头
        let sk1 = read_secret_key(id1);
        let sk2 = read_secret_key(id2);
        assert!(
            sk1.starts_with("v1:"),
            "secret_key should now be encrypted (v1: prefix), got: {}",
            sk1
        );
        assert!(
            sk2.starts_with("v1:"),
            "secret_key should now be encrypted (v1: prefix), got: {}",
            sk2
        );

        // 解密后应还原为原明文
        let pt1 = String::from_utf8(key.decrypt(&sk1).unwrap().to_vec()).unwrap();
        let pt2 = String::from_utf8(key.decrypt(&sk2).unwrap().to_vec()).unwrap();
        assert_eq!(pt1, "plaintext-api-key-1");
        assert_eq!(pt2, "plaintext-api-key-2");
    }

    /// is_local=1 的模型（本地 manifest JSON）不应被迁移——只有云端 API Key 才加密。
    #[test]
    fn migrate_skips_local_models() {
        setup_clean_db();
        let key = make_key(1);

        let cloud_id = insert_test_model("cloud-api-key", 0);
        let local_id = insert_test_model("{\"manifest\":\"json-payload\"}", 1);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 1, "only the cloud model (is_local=0) should be migrated");

        // 云端行已加密
        assert!(read_secret_key(cloud_id).starts_with("v1:"));
        // 本地行保持原样（仍是明文 manifest JSON）
        assert_eq!(read_secret_key(local_id), "{\"manifest\":\"json-payload\"}");
    }

    /// 已加密（v1: 前缀）的行应被跳过——避免重复加密导致密文不可解。
    #[test]
    fn migrate_skips_already_encrypted() {
        setup_clean_db();
        let key = make_key(1);

        // 预先加密一行
        let encrypted = key.encrypt(b"already-encrypted-key").unwrap();
        let enc_id = insert_test_model(&encrypted, 0);
        let plain_id = insert_test_model("plaintext-key", 0);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 1, "only the plaintext row should be migrated");

        // 已加密行不变
        assert_eq!(read_secret_key(enc_id), encrypted);
        // 明文行已加密
        assert!(read_secret_key(plain_id).starts_with("v1:"));
    }

    /// 幂等性：连续迁移两次，第二次应返回 0（所有行已是 v1:）。
    #[test]
    fn migrate_is_idempotent() {
        setup_clean_db();
        let key = make_key(1);

        insert_test_model("plaintext-key-1", 0);
        insert_test_model("plaintext-key-2", 0);

        let count1 = migrate_secret_keys_to_encrypted(&key).expect("first migration");
        assert_eq!(count1, 2);

        let count2 = migrate_secret_keys_to_encrypted(&key).expect("second migration");
        assert_eq!(count2, 0, "second run should find nothing to migrate");
    }

    /// 空 secret_key（''）的行也跳过——避免对没有配置 API Key 的模型做无意义加密。
    /// list_models_for_secret_migration 的 SQL 含 `secret_key != ''` 守卫。
    #[test]
    fn migrate_skips_empty_secret_key() {
        setup_clean_db();
        let key = make_key(1);

        insert_test_model("", 0);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 0, "empty secret_key should be skipped");
    }

    /// 保留签名编译测试——证明 `migrate_secret_keys_to_encrypted` 的类型签名在编译期可解析。
    #[test]
    fn test_signature_compiles() {
        let _ = std::any::TypeId::of::<fn(&DerivedKey) -> Result<usize>>();
    }
}
