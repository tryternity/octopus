//! vault_ciphers 表的高层 API：Cipher 加解密 + CRUD。

use anyhow::Result;

use octopus_infra::db::{self, VaultCipherInput};

use crate::crypto::DerivedKey;
use crate::types::{decrypt_cipher_row, Cipher, CipherInput};

pub fn list_ciphers(key: &DerivedKey) -> Result<Vec<Cipher>> {
    let rows = db::list_vault_ciphers()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(decrypt_cipher_row(&row, key)?);
    }
    Ok(out)
}

pub fn load_cipher(id: i64, key: &DerivedKey) -> Result<Option<Cipher>> {
    let row = match db::load_vault_cipher(id)? {
        Some(r) => r,
        None => return Ok(None),
    };
    Ok(Some(decrypt_cipher_row(&row, key)?))
}

pub fn create_cipher(input: &CipherInput, key: &DerivedKey) -> Result<i64> {
    let enc = input.encrypt_strings(key)?;
    let db_input = VaultCipherInput {
        folder_id: input.folder_id,
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
    };
    Ok(db::insert_vault_cipher(&db_input)?)
}

pub fn save_cipher(id: i64, input: &CipherInput, key: &DerivedKey) -> Result<()> {
    let enc = input.encrypt_strings(key)?;
    let db_input = VaultCipherInput {
        folder_id: input.folder_id,
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
    };
    Ok(db::update_vault_cipher(id, &db_input)?)
}

pub fn soft_delete(id: i64) -> Result<()> {
    Ok(db::soft_delete_vault_cipher(id)?)
}

pub fn restore(id: i64) -> Result<()> {
    Ok(db::restore_vault_cipher(id)?)
}

pub fn permanent_delete(id: i64) -> Result<()> {
    Ok(db::permanent_delete_vault_cipher(id)?)
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
        let id = create_cipher(&input, &key).expect("create");
        assert!(id > 0);

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
        let empty = list_ciphers(&key).expect("list empty");
        assert!(empty.is_empty());

        // 插两条
        let id_a = create_cipher(&sample_input("SiteA"), &key).expect("create a");
        let id_b = create_cipher(&sample_input("SiteB"), &key).expect("create b");

        let all = list_ciphers(&key).expect("list");
        assert_eq!(all.len(), 2);
        let names: Vec<String> = all.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"SiteA".to_string()));
        assert!(names.contains(&"SiteB".to_string()));
        let ids: Vec<i64> = all.iter().map(|c| c.id).collect();
        assert!(ids.contains(&id_a));
        assert!(ids.contains(&id_b));
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

        let id = create_cipher(&input, &key).expect("create");
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

        let id = create_cipher(&input, &key).expect("create");
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
        let id = create_cipher(&sample_input("Initial"), &key).expect("create");

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
        let result = load_cipher(99999, &key).expect("应返回 Ok(None) 而非 Err");
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
        let id_a = create_cipher(&sample_input("A"), &key).expect("create a");
        let id_b = create_cipher(&sample_input("B"), &key).expect("create b");
        let id_c = create_cipher(&sample_input("C"), &key).expect("create c");

        // 让 A 最新（最后 save）—— sleep 1s 确保 updated_at 字符串不同
        // （DB 用 datetime('now')，精度到秒）
        std::thread::sleep(std::time::Duration::from_secs(1));
        save_cipher(id_a, &sample_input("A-bumped"), &key).expect("save a");

        let all = list_ciphers(&key).expect("list");
        assert_eq!(all.len(), 3, "应有 3 行");
        // A 是最近 updated → 应排在第 0 位
        assert_eq!(all[0].id, id_a, "最近 save 的 A 应排第 0");
        // 剩下 B / C 按 created 顺序（同秒）—— 只验证 A 在最前即可（其余顺序与实现细节耦合）
        let rest_ids: Vec<i64> = all[1..].iter().map(|c| c.id).collect();
        assert!(rest_ids.contains(&id_b));
        assert!(rest_ids.contains(&id_c));
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
        let id = create_cipher(&sample_input("ToDelete"), &key).expect("create");

        soft_delete(id).expect("soft delete");
        // load 应仍能拿到（deleted_at=Some）
        let loaded = load_cipher(id, &key).expect("load").expect("should still exist");
        assert!(loaded.deleted_at.is_some(), "软删后 deleted_at 应有值");
        // list 也仍能拿到（不过滤）
        let all = list_ciphers(&key).expect("list");
        assert!(all.iter().any(|c| c.id == id), "软删后 list 应仍包含该行");
    }
}
