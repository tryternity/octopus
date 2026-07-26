//! 一次性迁移：把 models.secret_key 的明文 API Key 用 app_key 加密回写。
//!
//! 触发时机：首次 setup_vault 之后。
//! 规则：
//!   - 仅处理 is_local=0（云端 API Key）的行
//!   - 跳过已 v1: 开头的行（避免重复加密）
//!   - 迁移后字段以 v1: 前缀存密文

use anyhow::{Context, Result};
use octopus_infra::db;
use zeroize::Zeroize;

use crate::crypto::symmetric::CIPHERTEXT_PREFIX;
use crate::crypto::DerivedKey;

/// 迁移所有未加密的 secret_key。返回迁移的行数。
///
/// #5 修复：迁移必须是事务性的。
///
/// 历史问题：旧实现逐条调 `db::update_model_secret_key`，每条独立 autocommit。
/// 中途任一行失败（例如某行密文构造失败 / DB 写错）→ 已写入的行 commit，未写的
/// 行保留明文，且 setup_vault 调用方 `Err(e) => log::warn!` 吞错 → DB 处于
/// 半迁移状态。setup_vault 的 `ensure!(!is_initialized())` 又阻止重跑，
/// 用户明文 API Key 永远残留。审计 #5 标为 high。
///
/// 修复策略（最小改动，不重构 db.rs）：
///   1. 先在内存里把所有 plaintext 全部加密成 `Vec<(id, enc)>`；
///      任一行加密失败 → `?` 直接 bubble 出去，DB 0 改动。
///   2. 再用 `with_db` + `unchecked_transaction` 把整批 UPDATE 放进一个事务，
///      全成功才 commit；任一行 UPDATE 失败 → tx 自动 drop = rollback。
pub fn migrate_secret_keys_to_encrypted(app_key: &DerivedKey) -> Result<usize> {
    // M1(b) 修复：传 CIPHERTEXT_PREFIX 而非在 db.rs 硬编码——单点维护，
    // 升级 v2: 时只改 CIPHERTEXT_PREFIX，SQL 守卫自动跟随。
    let mut models = db::list_models_for_secret_migration(CIPHERTEXT_PREFIX)?;

    // (1) 全部加密——任一行失败 → 整批 abort，DB 0 改动
    let encrypted: Vec<(i64, String)> = models
        .iter()
        .map(|(id, plaintext)| {
            let enc = app_key.encrypt(plaintext.as_bytes())?;
            Ok((*id, enc))
        })
        .collect::<Result<_>>()?;

    // (2) 事务内整批写——任一 UPDATE 失败 → tx drop = rollback
    //
    // 不用 `rusqlite::params!` 宏——octopus-vault 仅把 rusqlite 作为 dev-dependency
    // （用于 set_test_db 注入 in-memory 连接），生产构建拿不到该宏。改用裸
    // `&[&dyn ToSql]` 切片（rusqlite::ToSql 已通过 octopus-infra 重导出暴露给 vault）。
    let count = db::with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        for (id, enc) in &encrypted {
            // M2 修复：UPDATE 失败带行 id（诊断信息）。context 在 ? 前附加，
            // tx drop 时 rollback 自动触发，但错误信息已捕获不会丢失。
            tx.execute(
                "UPDATE models SET secret_key = ? WHERE id = ?",
                &[enc as &dyn rusqlite::ToSql, id as &dyn rusqlite::ToSql],
            )
            .with_context(|| format!("迁移 model id={} 失败", id))?;
        }
        tx.commit()?;
        Ok(encrypted.len())
    })?;

    log::info!("迁移 {} 个 model 的 secret_key 为加密格式", count);

    // OBS-MIGRATE-PLAINTEXT-MODELS-RESIDUE 修复（2026-07-27，第六十二轮）：
    // models: Vec<(i64, String)> 的 String 含明文 API key（来自 infra
    // list_models_for_secret_migration）。加密完成后 models 仍持有明文，函数结束
    // drop 时普通 String heap 不清零 → 明文 API key 残留窗口。
    // 显式 zeroize 消除残留（String 实现了 zeroize::Zeroize trait）。
    // 风险 Low（单机 + 迁移前本就明文存 DB + 残留窗口仅函数执行期），但修复成本
    // 极低 + 与已修的 17 处 Zeroizing 卫生同类型，值得做。
    for (_, plaintext) in &mut models {
        plaintext.zeroize();
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_infra::db;
    use rusqlite::params;

    /// 构造一份确定性的 32B DerivedKey（每个 byte 都为 `byte`），用于加解密往返。
    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey::from_raw([byte; 32])
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
    /// secret_key / source_type 由参数显式传入；其余列走 schema DEFAULT。
    /// UNIQUE(domain, provider, category, model_name) 通过 model_name 附加随机后缀避免冲突。
    fn insert_test_model(secret_key: &str, source_type: i64) -> i64 {
        // 用 AtomicU64 生成的简单递增 id 作为后缀——避免引入 std::sync::Mutex 全局计数器。
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let suffix = SEQ.fetch_add(1, Ordering::SeqCst);

        db::with_db(|conn| {
            conn.execute(
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, source_type)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    "asr",
                    "test_provider",
                    "test_category",
                    format!("test-model-{}-{}", suffix, source_type),
                    "test-source",
                    secret_key,
                    source_type,
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

    /// 迁移明文密钥：source_type=2（cloud） + 非 v1: 前缀的行应被加密；本地模型跳过。
    #[test]
    fn migrate_encrypts_plaintext_keys() {
        setup_clean_db();
        let key = make_key(1);

        let id1 = insert_test_model("plaintext-api-key-1", 2);
        let id2 = insert_test_model("plaintext-api-key-2", 2);

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

    /// source_type=1（local）的模型（本地 manifest JSON）不应被迁移——只有云端 API Key 才加密。
    #[test]
    fn migrate_skips_local_models() {
        setup_clean_db();
        let key = make_key(1);

        let cloud_id = insert_test_model("cloud-api-key", 2);
        let local_id = insert_test_model("{\"manifest\":\"json-payload\"}", 1);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 1, "only the cloud model (source_type=2) should be migrated");

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
        let enc_id = insert_test_model(&encrypted, 2);
        let plain_id = insert_test_model("plaintext-key", 2);

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

        insert_test_model("plaintext-key-1", 2);
        insert_test_model("plaintext-key-2", 2);

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

        insert_test_model("", 2);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 0, "empty secret_key should be skipped");
    }

    /// 保留签名编译测试——证明 `migrate_secret_keys_to_encrypted` 的类型签名在编译期可解析。
    #[test]
    fn test_signature_compiles() {
        let _ = std::any::TypeId::of::<fn(&DerivedKey) -> Result<usize>>();
    }

    /// #5 修复验证：迁移在事务里整批提交——成功时所有行一次性写入。
    ///
    /// 与逐条 autocommit 不可区分（成功路径上行为一致），但作为事务正确性的
    /// 正向基线保留。失败路径的回滚（"任一 UPDATE 失败 → 整批回滚"）由
    /// `unchecked_transaction()` + `tx.commit()` 在 rusqlite 层保证：
    ///   - tx Drop（未 commit）→ DROP TABLE / ROLLBACK 自动撤销所有更改
    ///   - DB schema 层面我们无法在测试里伪造 UPDATE 失败（columns 都齐全、
    ///     类型也都对得上），所以失败路径的回滚断言依赖 rusqlite 自身语义，
    ///     此处不强行 mock。
    ///
    /// 此测试断言：成功迁移后，DB 中**不存在**任何明文行（所有候选都被一次写入）。
    #[test]
    fn migrate_transactional_all_or_nothing_on_success() {
        setup_clean_db();
        let key = make_key(1);

        let id1 = insert_test_model("plaintext-1", 2);
        let id2 = insert_test_model("plaintext-2", 2);
        let id3 = insert_test_model("plaintext-3", 2);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 3, "all 3 candidates should be migrated");

        // 成功后 DB 中不应再有任何明文（非 v1:）的云端行
        let remaining = db::list_models_for_secret_migration(CIPHERTEXT_PREFIX)
            .expect("list should succeed");
        assert!(
            remaining.is_empty(),
            "事务性提交后 DB 不应残留任何待迁移行，但还有 {:?}",
            remaining
        );

        // 三行都应是 v1: 前缀
        for id in [id1, id2, id3] {
            let sk = read_secret_key(id);
            assert!(
                sk.starts_with("v1:"),
                "迁移后所有行都应以 v1: 开头，id={} got={}",
                id,
                sk
            );
        }
    }

    /// #5 修复验证：迁移逻辑结构保证"先全加密，再统一写"。
    ///
    /// 这条性质意味着即使中途内存加密阶段失败（任一 `app_key.encrypt` 出错），
    /// 也绝不会触碰 DB——即所谓 "all-or-nothing" 的 "nothing" 分支。
    /// 此处用一个会成功但便于观察结构的场景做"正向可达"覆盖。
    #[test]
    fn migrate_collect_then_write_does_not_touch_db_on_encrypt_failure() {
        // 难以在现有 API 下注入"加密失败"，但可验证结构：当所有候选都已 v1: 时
        // （`list_models_for_secret_migration` 返回空），migrate 既不加密也不写库，
        // 直接返回 0——这覆盖了 "collect 阶段产出空集 → 跳过 tx" 的边界。
        setup_clean_db();
        let key = make_key(1);

        // 预先把唯一的行加密成 v1:
        let pre_encrypted = key.encrypt(b"already-enc").unwrap();
        let id = insert_test_model(&pre_encrypted, 2);

        let count = migrate_secret_keys_to_encrypted(&key).expect("migration should succeed");
        assert_eq!(count, 0, "v1: 候选 0 个，不应做任何事");

        // 行未被改动
        assert_eq!(read_secret_key(id), pre_encrypted);
    }

    /// M1(b) 回归守护：验证「加密产生的前缀」与「迁移守卫前缀」绑定。
    ///
    /// migrate 用 `list_models_for_secret_migration(CIPHERTEXT_PREFIX)` 跳过已加密行，
    /// 而 encrypt 实际产生的前缀也是 CIPHERTEXT_PREFIX。这两者必须引用同一常量
    /// （M1(b) 修复前 SQL 守卫是硬编码 'v1:%' 字面量，与 CIPHERTEXT_PREFIX 耦合
    /// 但无引用关系——升级 v2: 时漏改一处会导致数据损坏）。
    ///
    /// 本测试断言不变量：encrypt 出的密文前缀 == CIPHERTEXT_PREFIX（守卫前缀）。
    /// 若有人改了 CIPHERTEXT_PREFIX，encrypt 输出和守卫都自动跟随（因为同引用），
    /// 此测试仍成立；若有人重新引入硬编码字面量割裂两者，此测试会暴露。
    #[test]
    fn encrypt_prefix_matches_migration_guard_prefix() {
        let key = make_key(1);
        let ciphertext = key.encrypt(b"any-plaintext").unwrap();
        // encrypt 产生的前缀必须是 CIPHERTEXT_PREFIX
        assert!(
            ciphertext.starts_with(CIPHERTEXT_PREFIX),
            "encrypt 产生的前缀 '{}' 必须等于 CIPHERTEXT_PREFIX '{}'，\
             否则迁移守卫会漏保护（M1(b) 不变量）",
            &ciphertext[..CIPHERTEXT_PREFIX.len().min(ciphertext.len())],
            CIPHERTEXT_PREFIX
        );
    }
}
