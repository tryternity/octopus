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
            1 => CipherType::Login,
            2 => CipherType::SecureNote,
            3 => CipherType::Card,
            4 => CipherType::Identity,
            other => {
                // M4（2026-07-24）：非法值不再静默兜底——记 log 让问题可观测。
                // 保留兜底为 Login（数据兼容性——旧库可能有未知类型，不能让读取失败），
                // 但至少诊断时有迹可循。威胁模型假设 DB 不被直接改（单机）。
                log::warn!("CipherType 非法值 {}，兜底为 Login", other);
                CipherType::Login
            }
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
        match v {
            0 => RepromptType::None,
            1 => RepromptType::Password,
            other => {
                // M4（2026-07-24）：非法值不再静默降级 None——记 log 让问题可观测。
                // 仍降级为 None（数据兼容性），但诊断时可发现 DB 被篡改的迹象。
                // 注意：降级 None 意味着绕过二次验证——威胁模型假设 DB 不被直接改。
                log::warn!(
                    "RepromptType 非法值 {}，降级为 None（二次验证被绕过——检查 DB 是否被篡改）",
                    other
                );
                RepromptType::None
            }
        }
    }
}

/// URI 匹配策略（与 Bitwarden 官方 `UriMatchType` 枚举值严格对齐）。
///
/// ⚠️ **2026-07-24 协议对齐修复**：之前 `Exact=2, StartsWith=3` 与官方相反，
/// 导致 Bitwarden 导入/导出 JSON 的 `match` 字段语义静默互换（导入把官方 Exact=3
/// 解析成 StartsWith；导出把 StartsWith=3 写成 Exact）。经核对 Bitwarden server
/// 源码 `src/Core/Enums/UriMatchType.cs` 确认官方值，已修正。
///
/// 官方值（[UriMatchType.cs](https://github.com/bitwarden/server/blob/main/src/Core/Enums/UriMatchType.cs)）：
/// ```text
/// Domain = 0, Host = 1, StartsWith = 2, Exact = 3,
/// RegularExpression = 4, Never = 5
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", try_from = "i64")]
pub enum MatchType {
    Domain = 0,
    Host = 1,
    StartsWith = 2,
    Exact = 3,
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
            2 => MatchType::StartsWith,
            3 => MatchType::Exact,
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
    pub id: String, // UUID v4 字符串（2026-07-21 v44：支持 git 同步）
    pub folder_id: Option<String>,
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

/// 新建/更新 cipher 的输入（不带 id/时间戳——id 由调用方在 create_cipher 时生成 UUID）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CipherInput {
    pub folder_id: Option<String>,
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
        // 8.1 清理：提取共用函数，消除与 CipherInput::encrypt_strings 的 60 行重复
        encrypt_cipher_fields(
            &self.name,
            &self.notes,
            &self.data,
            &self.fields,
            &self.password_history,
            key,
        )
    }
}

impl CipherInput {
    /// 用 user_vault_key 加密。
    pub fn encrypt_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings> {
        encrypt_cipher_fields(
            &self.name,
            &self.notes,
            &self.data,
            &self.fields,
            &self.password_history,
            key,
        )
    }
}

/// 加密 cipher 的敏感字段（Cipher / CipherInput 共用，8.1 清理重复代码）。
///
/// 之前 Cipher::encrypt_strings 和 CipherInput::encrypt_strings 是 60 行逐字相同的
/// 重复代码——两者加密逻辑完全一致（字段同名同类型），只是 struct 不同。
fn encrypt_cipher_fields(
    name: &str,
    notes: &Option<String>,
    data: &CipherData,
    fields: &[Field],
    password_history: &[PasswordHistoryEntry],
    key: &DerivedKey,
) -> Result<CipherEncStrings> {
    let name = key.encrypt(name.as_bytes())?;
    let notes = match notes {
        Some(n) => Some(key.encrypt(n.as_bytes())?),
        None => None,
    };
    let data_json = serde_json::to_vec(data)?;
    let data = key.encrypt(&data_json)?;
    let fields = if fields.is_empty() {
        None
    } else {
        let json = serde_json::to_vec(fields)?;
        Some(key.encrypt(&json)?)
    };
    let password_history = if password_history.is_empty() {
        None
    } else {
        let json = serde_json::to_vec(password_history)?;
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
        id: row.id.clone(),
        folder_id: row.folder_id.clone(),
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
        DerivedKey::from_raw([byte; 32])
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
            id: "test-uuid-1".to_string(),
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
            sync_md5: None,
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

        // 对齐 Bitwarden 官方 UriMatchType（2026-07-24 协议修正后的回归守护）：
        // 之前 octopus 把 Exact=2/StartsWith=3 与官方（StartsWith=2/Exact=3）弄反，
        // 导致 Bitwarden 导入/导出的 match 字段语义静默互换。此断言对齐官方值，
        // 防止未来再次反转。
        assert_eq!(
            MatchType::try_from(2).unwrap(),
            MatchType::StartsWith,
            "Bitwarden 官方协议 2 = StartsWith"
        );
        assert_eq!(
            MatchType::try_from(3).unwrap(),
            MatchType::Exact,
            "Bitwarden 官方协议 3 = Exact"
        );
        assert_eq!(i64::from(MatchType::StartsWith), 2);
        assert_eq!(i64::from(MatchType::Exact), 3);
    }
}
