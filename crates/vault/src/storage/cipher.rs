//! vault_ciphers 表的高层 API：Cipher 加解密 + CRUD。

use anyhow::Result;
use rusqlite::params;

use octopus_infra::db::{self, load_vault_cipher_at, VaultCipherInput};

use crate::crypto::DerivedKey;
use crate::types::{decrypt_cipher_row, Cipher, CipherInput};

/// 列表查询 + 批量解密。
///
/// **单行容错**（修复 #6）：单行解密失败不会让整表 Err——失败的 row 记 log + 收集
/// 到 `failures` 返回，调用方可 toast 提示用户「X 条记录解密失败已跳过」。
///
/// 失败常见原因：DB 部分写入、bit-flip、跨版本迁移残留、cipher 字段损坏。
/// 任一都不应让用户看不到其他完好的 N-1 条。
///
/// 返回 `(成功的 cipher 列表, 失败的 cipher_id 列表)`。
///
/// 2026-07-21 v44：failures 类型从 `Vec<i64>` 改 `Vec<String>`（UUID 字符串）。
pub fn list_ciphers(key: &DerivedKey) -> Result<(Vec<Cipher>, Vec<String>)> {
    let rows = db::list_vault_ciphers()?;
    let mut out = Vec::with_capacity(rows.len());
    let mut failures: Vec<String> = Vec::new();
    for row in rows {
        match decrypt_cipher_row(&row, key) {
            Ok(c) => out.push(c),
            Err(e) => {
                log::warn!(
                    "cipher id={} 解密失败，已跳过（其他条目继续可见）：{}",
                    row.id,
                    e
                );
                failures.push(row.id);
            }
        }
    }
    Ok((out, failures))
}

pub fn load_cipher(id: &str, key: &DerivedKey) -> Result<Option<Cipher>> {
    let row = match db::load_vault_cipher(id)? {
        Some(r) => r,
        None => return Ok(None),
    };
    Ok(Some(decrypt_cipher_row(&row, key)?))
}

/// 创建 cipher——调用方必须先 `Uuid::new_v4().to_string()` 生成 id 传入。
/// 2026-07-21 v44：id 从 AUTOINCREMENT 改 UUID 字符串（git 同步跨设备无冲突）。
pub fn create_cipher(id: &str, input: &CipherInput, key: &DerivedKey) -> Result<()> {
    let db_input = prepare_cipher_input(id, input, key)?;
    Ok(db::insert_vault_cipher(&db_input)?)
}

/// 仅加密 + 算 sync_md5，不落库（L8 修复，2026-07-24）。
///
/// 供 importer 批量事务化用：先循环调此函数收集 `Vec<VaultCipherInput>`，
/// 再一次性 `db::insert_vault_ciphers_batch` 事务化 insert。加密是纯内存操作，
/// 失败的条目在循环阶段跳过（记 errors），只把成功的进 batch——既原子又容错。
pub fn prepare_cipher_input(
    id: &str,
    input: &CipherInput,
    key: &DerivedKey,
) -> Result<VaultCipherInput> {
    let enc = input.encrypt_strings(key)?;
    let db_input = VaultCipherInput {
        id: id.to_string(),
        folder_id: input.folder_id.clone(),
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
        deleted_at: None, // 新建默认未软删（H2 修复）
        sync_md5: None,   // 下面算好填入
    };
    let sync_md5 = crate::sync::fingerprint::cipher_md5_from_input(id, &db_input);
    Ok(VaultCipherInput { sync_md5: Some(sync_md5), ..db_input })
}

pub fn save_cipher(id: &str, input: &CipherInput, key: &DerivedKey) -> Result<()> {
    let enc = input.encrypt_strings(key)?;
    // H2 修复：编辑时保留现有 deleted_at（不碰删除状态）——读现有 row 取值。
    // update SQL 现在会 SET deleted_at，若传 None 会把已软删的 cipher 恢复成 live。
    let existing_deleted_at = db::load_vault_cipher(id)?
        .map(|row| row.deleted_at)
        .unwrap_or(None);
    let db_input = VaultCipherInput {
        // update 不需要 id（id 是 WHERE 条件），但 VaultCipherInput struct 现在含 id 字段
        // ——填占位（不会被 update_vault_cipher 使用，只 WHERE id = ? 用外部传入的 id）
        id: id.to_string(),
        folder_id: input.folder_id.clone(),
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
        deleted_at: existing_deleted_at, // 保留现有删除状态（H2 修复）
        sync_md5: None,
    };
    let sync_md5 = crate::sync::fingerprint::cipher_md5_from_input(id, &db_input);
    let db_input = VaultCipherInput { sync_md5: Some(sync_md5), ..db_input };
    Ok(db::update_vault_cipher(id, &db_input)?)
}

/// S1 修复（2026-07-24）：soft_delete + sync_md5 重算合并为单事务——
/// 之前两步独立 autocommit，第 2 步失败时 deleted_at 已改但 sync_md5 仍旧 →
/// incremental_export 用旧 md5 对比旧 outline → 一致 → 文件不重写 → 删除不传播。
pub fn soft_delete(id: &str) -> Result<()> {
    db::with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        // 1. UPDATE deleted_at
        tx.execute(
            "UPDATE vault_ciphers SET deleted_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        // 2. 读完整 row（含新 deleted_at）算 md5
        let row = load_vault_cipher_at(&tx, id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
        let md5 = crate::sync::fingerprint::cipher_md5(&row);
        // 3. UPDATE sync_md5
        tx.execute(
            "UPDATE vault_ciphers SET sync_md5 = ?1 WHERE id = ?2",
            params![md5, id],
        )?;
        tx.commit()?;
        Ok(())
    })
}

/// S1 修复（2026-07-24）：restore + sync_md5 重算合并为单事务（与 soft_delete 对称）。
pub fn restore(id: &str) -> Result<()> {
    db::with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE vault_ciphers SET deleted_at = NULL WHERE id = ?1",
            params![id],
        )?;
        let row = load_vault_cipher_at(&tx, id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
        let md5 = crate::sync::fingerprint::cipher_md5(&row);
        tx.execute(
            "UPDATE vault_ciphers SET sync_md5 = ?1 WHERE id = ?2",
            params![md5, id],
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn permanent_delete(id: &str) -> Result<()> {
    Ok(db::permanent_delete_vault_cipher(id)?)
}

/// 清空回收站：批量永久删除所有 deleted_at IS NOT NULL 的 cipher。
///
/// 逐条 `permanent_delete`（而非单条 DELETE FROM deleted_at）——保持与 sync_md5
/// 一致性逻辑的对称（虽然 permanent delete 后行已不存在，md5 无需更新，但走同一
/// 函数路径便于未来加审计/级联清理）。单条失败不中断——收集 errors 返回，让调用
/// 方 toast 提示「清空了 N 条，M 条失败」。
///
/// T2 修复（2026-07-24）：SYNC_LOCK 下沉到函数内部——与 sync_now 并发时挡住，
/// 避免「刚永久删的行被并发 pull 重新插入」（M5 的本地并发表现）。锁在函数内
/// 而非调用方，避免未来新增调用方（CLI/批量清理）忘记加锁。与 meta_lock 下沉
/// 到 save_vault_meta 的设计一致。
pub fn empty_trash() -> Result<(usize, Vec<String>)> {
    // T2：取 sync 锁——sync 进行中返 Err（guard 持续整个函数生命周期）
    let _sync_guard = crate::sync::engine::try_sync_lock()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let ids = db::list_trash_cipher_ids()?;
    let mut errors: Vec<String> = Vec::new();
    let mut deleted = 0;
    for id in &ids {
        match db::permanent_delete_vault_cipher(id) {
            Ok(_) => deleted += 1,
            Err(e) => {
                log::warn!("empty_trash: 删除 cipher {} 失败: {}", id, e);
                errors.push(id.clone());
            }
        }
    }
    Ok((deleted, errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CipherData, CipherType, LoginData, LoginUri, MatchType, RepromptType};
    use octopus_infra::db;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    fn sample_input(name: &str) -> CipherInput {
        CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: name.into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: format!("https://{}.com", name.to_lowercase()),
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
        }
    }

    /// 注入干净 in-memory DB（含 vault_ciphers 表，无数据）。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        db::set_test_db(conn);
    }

    // 使用 set_test_db 注入 in-memory DB，无需写入 ~/.octopus/octopus.db，CI 友好。
    #[test]
    fn test_cipher_crud_round_trip_with_real_db() {
        setup_clean_db();
        let key = make_key(7);
        let input = sample_input("TestSite");

        // create + load + verify
        let id = "77777777-7777-4777-8777-777777777777";
        create_cipher(id, &input, &key).expect("create");

        let loaded = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded.name, "TestSite");
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(login) = loaded.data {
            assert_eq!(login.username, Some("user".into()));
        } else {
            panic!("应为 Login");
        }

        // 更新
        let mut input2 = input.clone();
        input2.name = "TestSite2".into();
        save_cipher(id, &input2, &key).expect("save");
        let loaded2 = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded2.name, "TestSite2");

        // 软删除 + 恢复
        soft_delete(id).expect("soft delete");
        let loaded3 = load_cipher(id, &key).expect("load").expect("should still exist (soft del)");
        assert!(loaded3.deleted_at.is_some());

        restore(id).expect("restore");
        let loaded4 = load_cipher(id, &key).expect("load").expect("should exist");
        assert!(loaded4.deleted_at.is_none());

        // 物理删除
        permanent_delete(id).expect("perm delete");
        assert!(load_cipher(id, &key).expect("load").is_none());
    }

    /// list_ciphers：空库返回空；插入两条后应能解密回两条。
    #[test]
    fn list_ciphers_returns_all_and_decrypts() {
        setup_clean_db();
        let key = make_key(9);

        // 空库
        let (empty, _) = list_ciphers(&key).expect("list empty");
        assert!(empty.is_empty());

        // 插两条
        let id_a = "99999999-9999-4999-8999-999999999999";
        let id_b = "88888888-8888-4888-8888-888888888888";
        create_cipher(id_a, &sample_input("SiteA"), &key).expect("create a");
        create_cipher(id_b, &sample_input("SiteB"), &key).expect("create b");

        let (all, _) = list_ciphers(&key).expect("list");
        assert_eq!(all.len(), 2);
        let names: Vec<String> = all.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"SiteA".to_string()));
        assert!(names.contains(&"SiteB".to_string()));
        let ids: Vec<String> = all.iter().map(|c| c.id.clone()).collect();
        assert!(ids.contains(&id_a.to_string()));
        assert!(ids.contains(&id_b.to_string()));
    }

    /// 最小化 CipherInput（空 uris，None username/password/totp）仍能 create + load。
    /// 关键不变量：空 collection / None 可选字段不应破坏加解密链路。
    #[test]
    fn create_cipher_with_minimal_data_round_trips() {
        setup_clean_db();
        let key = make_key(1);
        let input = CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: "Minimal".into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![],
                username: None,
                password: None,
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
        };

        let id = "11111111-1111-4111-8111-111111111111";
        create_cipher(id, &input, &key).expect("create");
        let loaded = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded.name, "Minimal");
        #[allow(irrefutable_let_patterns)]
        let CipherData::Login(login) = loaded.data else {
            panic!("应为 Login");
        };
        assert!(login.uris.is_empty(), "uris 应保持空");
        assert!(login.username.is_none());
        assert!(login.password.is_none());
        assert!(login.totp.is_none());
    }

    /// 多 URI（3+）+ 不同 match_type：全部应原样 round-trip。
    #[test]
    fn create_cipher_with_multiple_uris_and_match_types_round_trips() {
        setup_clean_db();
        let key = make_key(2);
        let input = CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: "MultiUri".into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![
                    LoginUri { uri: "https://a.com".into(), match_type: Some(MatchType::Domain) },
                    LoginUri { uri: "https://b.com".into(), match_type: Some(MatchType::Host) },
                    LoginUri { uri: "https://c.com".into(), match_type: Some(MatchType::Exact) },
                    LoginUri { uri: "https://d.com".into(), match_type: None },
                ],
                username: Some("u".into()),
                password: Some("p".into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
        };

        let id = "22222222-2222-4222-8222-222222222222";
        create_cipher(id, &input, &key).expect("create");
        let loaded = load_cipher(id, &key).expect("load").expect("should exist");
        #[allow(irrefutable_let_patterns)]
        let CipherData::Login(login) = loaded.data else {
            panic!("应为 Login");
        };
        assert_eq!(login.uris.len(), 4);
        assert_eq!(login.uris[0].match_type, Some(MatchType::Domain));
        assert_eq!(login.uris[1].match_type, Some(MatchType::Host));
        assert_eq!(login.uris[2].match_type, Some(MatchType::Exact));
        assert_eq!(login.uris[3].match_type, None, "None 必须保持 None");
        assert_eq!(login.uris[0].uri, "https://a.com");
        assert_eq!(login.uris[3].uri, "https://d.com");
    }

    /// save_cipher 用相同 id 覆盖后：id 不变（FK 一致性），新字段写入。
    #[test]
    fn save_cipher_preserves_id_and_overwrites_fields() {
        setup_clean_db();
        let key = make_key(3);
        let id = "33333333-3333-4333-8333-333333333333";
        create_cipher(id, &sample_input("Initial"), &key).expect("create");

        // save with same id, different name
        let mut updated = sample_input("Updated");
        updated.name = "Updated".into();
        save_cipher(id, &updated, &key).expect("save");

        let loaded = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded.id, id, "id 必须保持不变（同一行）");
        assert_eq!(loaded.name, "Updated");
    }

    /// load_cipher 不存在的 id → Ok(None)（不是 Err）。这是「未找到」语义的不变量。
    #[test]
    fn load_cipher_nonexistent_returns_ok_none() {
        setup_clean_db();
        let key = make_key(4);
        let result = load_cipher("nonexistent-uuid", &key).expect("应返回 Ok(None) 而非 Err");
        assert!(result.is_none());
    }

    /// list_ciphers 应按 updated_at DESC 排序（infra 层 SQL ORDER BY updated_at DESC）。
    ///
    /// 通过 save_cipher 触发 updated_at 更新（DB 自动写 datetime('now')），
    /// 验证最后 save 的行排在最前。
    #[test]
    fn list_ciphers_orders_by_updated_at_desc() {
        setup_clean_db();
        let key = make_key(5);
        let id_a = "55555555-5555-4555-8555-555555555555";
        let id_b = "66666666-6666-4666-8666-666666666666";
        let id_c = "77777777-7777-4777-8777-777777777777";
        create_cipher(id_a, &sample_input("A"), &key).expect("create a");
        create_cipher(id_b, &sample_input("B"), &key).expect("create b");
        create_cipher(id_c, &sample_input("C"), &key).expect("create c");

        // 让 A 最新（最后 save）—— sleep 1s 确保 updated_at 字符串不同
        // （DB 用 datetime('now')，精度到秒）
        std::thread::sleep(std::time::Duration::from_secs(1));
        save_cipher(id_a, &sample_input("A-bumped"), &key).expect("save a");

        let (all, _) = list_ciphers(&key).expect("list");
        assert_eq!(all.len(), 3, "应有 3 行");
        // A 是最近 updated → 应排在第 0 位
        assert_eq!(all[0].id, id_a, "最近 save 的 A 应排第 0");
        // 剩下 B / C 按 created 顺序（同秒）—— 只验证 A 在最前即可（其余顺序与实现细节耦合）
        let rest_ids: Vec<String> = all[1..].iter().map(|c| c.id.clone()).collect();
        assert!(rest_ids.contains(&id_b.to_string()));
        assert!(rest_ids.contains(&id_c.to_string()));
        // 不变量：列表中前一行的 updated_at >= 后一行
        for w in all.windows(2) {
            assert!(
                w[0].updated_at >= w[1].updated_at,
                "updated_at DESC 顺序违反：{} < {}",
                w[0].updated_at,
                w[1].updated_at
            );
        }
    }

    /// 软删除后再 list：行仍在表（deleted_at 已设），load 仍能解密回带 deleted_at 的 cipher。
    /// （infra 的 list_vault_ciphers 不过滤 deleted_at；命令层另做过滤。）
    #[test]
    fn soft_delete_marks_but_keeps_row_in_list() {
        setup_clean_db();
        let key = make_key(6);
        let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        create_cipher(id, &sample_input("ToDelete"), &key).expect("create");

        soft_delete(id).expect("soft delete");
        // load 应仍能拿到（deleted_at=Some）
        let loaded = load_cipher(id, &key).expect("load").expect("should still exist");
        assert!(loaded.deleted_at.is_some(), "软删后 deleted_at 应有值");
        // list 也仍能拿到（不过滤）
        let (all, _) = list_ciphers(&key).expect("list");
        assert!(all.iter().any(|c| c.id == id), "软删后 list 应仍包含该行");
    }

    /// S1 回归守护：soft_delete / restore 后 sync_md5 必须反映 deleted_at 变化——
    /// 之前两步非原子，第 2 步失败时 deleted_at 已改但 sync_md5 仍旧 → 删除不传播。
    /// 现在合并为单事务，两者原子一致。
    #[test]
    fn soft_delete_and_restore_update_sync_md5_atomically() {
        setup_clean_db();
        let key = make_key(7);
        let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        create_cipher(id, &sample_input("AtomTest"), &key).expect("create");

        // 取初始 sync_md5（deleted_at=None）
        let initial = octopus_infra::db::load_vault_cipher(id)
            .unwrap()
            .unwrap();
        let initial_md5 = initial.sync_md5.clone().unwrap();

        // soft_delete 后 sync_md5 应变（含 deleted_at）
        soft_delete(id).expect("soft delete");
        let after_delete = octopus_infra::db::load_vault_cipher(id)
            .unwrap()
            .unwrap();
        assert!(
            after_delete.deleted_at.is_some(),
            "deleted_at 应有值"
        );
        assert_ne!(
            after_delete.sync_md5.as_deref().unwrap_or(""),
            initial_md5,
            "S1: soft_delete 后 sync_md5 应变（反映 deleted_at），否则删除不传播"
        );

        // 手动验证：用 DB row 算的 md5 应与存储的 sync_md5 一致（原子性）
        let recomputed = crate::sync::fingerprint::cipher_md5(&after_delete);
        assert_eq!(
            after_delete.sync_md5.as_deref().unwrap_or(""),
            recomputed,
            "S1: sync_md5 应与 DB row 的实际 md5 一致（单事务原子性）"
        );

        // restore 后 sync_md5 应回到接近初始值（deleted_at=None）
        restore(id).expect("restore");
        let after_restore = octopus_infra::db::load_vault_cipher(id)
            .unwrap()
            .unwrap();
        assert!(
            after_restore.deleted_at.is_none(),
            "deleted_at 应回 None"
        );
        // restore 后 md5 应与用 restored row 算的一致
        let restored_md5 = crate::sync::fingerprint::cipher_md5(&after_restore);
        assert_eq!(
            after_restore.sync_md5.as_deref().unwrap_or(""),
            restored_md5,
            "S1: restore 后 sync_md5 应与 DB row 一致"
        );
    }
}
