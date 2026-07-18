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
    use crate::types::{CipherData, CipherType, LoginData, LoginUri, RepromptType};
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
}
