//! Cipher 数据模型（解密后的明文结构 + 序列化辅助）。
//!
//! 这些类型仅在 vault 解锁状态下出现。落盘时通过 `to_encrypted_strings`
//! 转为密文字符串写入 SQLite。

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::crypto::DerivedKey;

/// cipher 类型（MVP 仅 Login）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", from = "i64")]
pub enum CipherType {
    Login = 1,
    SecureNote = 2,
    Card = 3,
    Identity = 4,
}

impl From<CipherType> for i64 {
    fn from(t: CipherType) -> i64 {
        t as i64
    }
}

impl From<i64> for CipherType {
    fn from(v: i64) -> Self {
        match v {
            2 => CipherType::SecureNote,
            3 => CipherType::Card,
            4 => CipherType::Identity,
            _ => CipherType::Login, // 兜底为 Login（兼容未知类型）
        }
    }
}

/// 敏感操作前是否需要再次确认主密码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", from = "i64")]
pub enum RepromptType {
    None = 0,
    Password = 1,
}

impl From<RepromptType> for i64 {
    fn from(t: RepromptType) -> i64 {
        t as i64
    }
}

impl From<i64> for RepromptType {
    fn from(v: i64) -> Self {
        if v == 1 {
            RepromptType::Password
        } else {
            RepromptType::None
        }
    }
}

/// URI 匹配策略（直接抄 Bitwarden 5 种 + Never）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", try_from = "i64")]
pub enum MatchType {
    Domain = 0,
    Host = 1,
    Exact = 2,
    StartsWith = 3,
    RegularExpression = 4,
    Never = 5,
}

impl From<MatchType> for i64 {
    fn from(t: MatchType) -> i64 {
        t as i64
    }
}

impl TryFrom<i64> for MatchType {
    type Error = anyhow::Error;
    fn try_from(v: i64) -> Result<Self> {
        Ok(match v {
            0 => MatchType::Domain,
            1 => MatchType::Host,
            2 => MatchType::Exact,
            3 => MatchType::StartsWith,
            4 => MatchType::RegularExpression,
            5 => MatchType::Never,
            _ => anyhow::bail!("无效的 MatchType: {}", v),
        })
    }
}

/// 单条 URI + 其匹配策略（None 表示用客户端默认，octopus 强制 Domain）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginUri {
    pub uri: String,
    /// null = 用客户端默认（Domain）
    pub match_type: Option<MatchType>,
}

/// Login 类型 cipher 的明文 payload（落盘时加密为 data 字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginData {
    pub uris: Vec<LoginUri>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Base32 secret（如 "JBSWY3DPEHPK3PXP"），不带 otpauth:// 前缀。
    pub totp: Option<String>,
    pub password_revision_date: Option<String>,
}

/// 自定义字段（密码、文本、隐藏等）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: Option<String>,
    /// 0=Text 1=Hidden 2=Boolean（Bitwarden 协议）
    pub field_type: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordHistoryEntry {
    pub password: String,
    pub last_used_at: String,
}

/// cipher data 枚举（MVP 仅 Login，未来扩展 SecureNote/Card/Identity）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
pub enum CipherData {
    Login(LoginData),
}

/// 解密后的 cipher 完整对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cipher {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: CipherType,
    pub name: String,
    pub notes: Option<String>,
    pub data: CipherData,
    pub fields: Vec<Field>,
    pub password_history: Vec<PasswordHistoryEntry>,
    pub reprompt: RepromptType,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新建/更新 cipher 的输入（不带 id/时间戳）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CipherInput {
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: CipherType,
    pub name: String,
    pub notes: Option<String>,
    pub data: CipherData,
    pub fields: Vec<Field>,
    pub password_history: Vec<PasswordHistoryEntry>,
    pub reprompt: RepromptType,
}

/// 加密后的 cipher 字段（与 db.rs 的 VaultCipher 明文字段一一对应）。
/// 由 vault crate 调用 `Cipher::encrypt_strings(&key)` 生成，再调
/// `VaultCipherInput { name, notes, data, fields, password_history, ... }` 落库。
pub struct CipherEncStrings {
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
}

impl Cipher {
    /// 用 user_vault_key 加密所有敏感字段。
    pub fn encrypt_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings> {
        let name = key.encrypt(self.name.as_bytes())?;
        let notes = match &self.notes {
            Some(n) => Some(key.encrypt(n.as_bytes())?),
            None => None,
        };
        let data_json = serde_json::to_vec(&self.data)?;
        let data = key.encrypt(&data_json)?;
        let fields = if self.fields.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.fields)?;
            Some(key.encrypt(&json)?)
        };
        let password_history = if self.password_history.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.password_history)?;
            Some(key.encrypt(&json)?)
        };
        Ok(CipherEncStrings {
            name,
            notes,
            data,
            fields,
            password_history,
        })
    }
}

impl CipherInput {
    /// 用 user_vault_key 加密。
    pub fn encrypt_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings> {
        let name = key.encrypt(self.name.as_bytes())?;
        let notes = match &self.notes {
            Some(n) => Some(key.encrypt(n.as_bytes())?),
            None => None,
        };
        let data_json = serde_json::to_vec(&self.data)?;
        let data = key.encrypt(&data_json)?;
        let fields = if self.fields.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.fields)?;
            Some(key.encrypt(&json)?)
        };
        let password_history = if self.password_history.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.password_history)?;
            Some(key.encrypt(&json)?)
        };
        Ok(CipherEncStrings {
            name,
            notes,
            data,
            fields,
            password_history,
        })
    }
}

/// 从 infra 的 VaultCipher（密文行）+ 解密 key → 解密 Cipher。
pub fn decrypt_cipher_row(
    row: &octopus_infra::db::VaultCipher,
    key: &DerivedKey,
) -> Result<Cipher> {
    let name_bytes = key.decrypt(&row.name)?;
    let name = String::from_utf8(name_bytes.to_vec())?;

    let notes = match &row.notes {
        Some(n) => {
            let bytes = key.decrypt(n)?;
            Some(String::from_utf8(bytes.to_vec())?)
        }
        None => None,
    };

    let data_bytes = key.decrypt(&row.data)?;
    let data: CipherData = serde_json::from_slice(&data_bytes)?;

    let fields = match &row.fields {
        Some(f) => {
            let bytes = key.decrypt(f)?;
            serde_json::from_slice(&bytes)?
        }
        None => vec![],
    };

    let password_history = match &row.password_history {
        Some(p) => {
            let bytes = key.decrypt(p)?;
            serde_json::from_slice(&bytes)?
        }
        None => vec![],
    };

    Ok(Cipher {
        id: row.id,
        folder_id: row.folder_id,
        favorite: row.favorite,
        atype: row.atype.into(),
        name,
        notes,
        data,
        fields,
        password_history,
        reprompt: row.reprompt.into(),
        deleted_at: row.deleted_at.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    fn sample_input() -> CipherInput {
        CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: "GitHub".into(),
            notes: Some("personal".into()),
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: "https://github.com".into(),
                    match_type: Some(MatchType::Domain),
                }],
                username: Some("user@example.com".into()),
                password: Some("p@ssw0rd".into()),
                totp: Some("JBSWY3DPEHPK3PXP".into()),
                password_revision_date: None,
            }),
            fields: vec![Field {
                name: "backup_code".into(),
                value: Some("12345".into()),
                field_type: 1,
            }],
            password_history: vec![],
            reprompt: RepromptType::None,
        }
    }

    #[test]
    fn test_cipher_encrypt_decrypt_round_trip() {
        let key = make_key(1);
        let input = sample_input();
        let enc = input.encrypt_strings(&key).unwrap();
        assert!(enc.name.starts_with("v1:"));
        assert!(enc.data.starts_with("v1:"));
        assert!(enc.fields.as_ref().unwrap().starts_with("v1:"));

        // 构造一个 VaultCipher 行模拟解密路径
        let row = octopus_infra::db::VaultCipher {
            id: 1,
            folder_id: None,
            favorite: false,
            atype: 1,
            name: enc.name,
            notes: enc.notes,
            data: enc.data,
            fields: enc.fields,
            password_history: enc.password_history,
            reprompt: 0,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        };
        let decrypted = decrypt_cipher_row(&row, &key).unwrap();
        assert_eq!(decrypted.name, "GitHub");
        assert_eq!(decrypted.notes, Some("personal".into()));
        // CipherData 当前只有 Login 变体；未来扩展 SecureNote/Card/Identity 后此 if let 即可正常分支。
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(login) = decrypted.data {
            assert_eq!(login.username, Some("user@example.com".into()));
            assert_eq!(login.password, Some("p@ssw0rd".into()));
            assert_eq!(login.uris[0].uri, "https://github.com");
        } else {
            panic!("应为 Login");
        }
        assert_eq!(decrypted.fields[0].name, "backup_code");
        assert_eq!(decrypted.fields[0].value, Some("12345".into()));
    }

    #[test]
    fn test_cipher_encrypt_empty_fields_omitted() {
        let key = make_key(1);
        let mut input = sample_input();
        input.fields = vec![];
        input.password_history = vec![];
        input.notes = None;
        let enc = input.encrypt_strings(&key).unwrap();
        assert!(enc.fields.is_none(), "空 fields 应省略");
        assert!(enc.password_history.is_none(), "空 history 应省略");
        assert!(enc.notes.is_none(), "None notes 应省略");
    }

    #[test]
    fn test_cipher_type_round_trip() {
        assert_eq!(i64::from(CipherType::Login), 1);
        assert_eq!(i64::from(CipherType::SecureNote), 2);
        assert_eq!(CipherType::from(1), CipherType::Login);
        assert_eq!(CipherType::from(99), CipherType::Login); // 兜底
    }

    #[test]
    fn test_match_type_round_trip() {
        assert_eq!(i64::from(MatchType::Domain), 0);
        assert_eq!(i64::from(MatchType::RegularExpression), 4);
        assert_eq!(MatchType::try_from(0).unwrap(), MatchType::Domain);
        assert!(MatchType::try_from(99).is_err());
    }
}
