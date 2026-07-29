//! vault 条目（cipher）+ 文件夹（folder）CRUD + AutoTypeMode 枚举。
//!
//! 共享 helper（cipher_to_dto / dto_to_input / merge_password_history /
//! require_user_vault_key）+ DTO struct（CipherDto / CipherInputDto）留 mod.rs，
//! 子模块用 `use super::{...}` 引用。

use tauri::State;

use octopus_vault::storage::FolderDto;

use crate::runtime_config::SharedRuntimeConfig;
use crate::vault::vault_error::{self, VaultError};
use crate::vault::vault_state::SharedVaultSession;

use super::{
    cipher_to_dto, dto_to_input, merge_password_history, require_user_vault_key, CipherDto,
    CipherInputDto,
};

/// Auto-Type 模式（2026-07-20 三模式）。
///
/// **背景**：webmail SPA（mail.163.com 等）的 Tab 键切焦点不可靠——SPA 自己拦截 Tab
/// 或密码框是 iframe，导致 `username + Tab + password` 的密码进不了密码框。
///
/// 解决方案：让用户据当前光标位置选合适模式——
/// - `UsernamePassword`：旧行为，username + Tab + password。仅当焦点在 username 框
///   且网站 Tab 行为正常时用。
/// - `PasswordOnly`：只输 password 到当前焦点。最常用——用户手动点密码框后触发。
/// - `UsernameOnly`：只输 username 到当前焦点。用于"换用户名"场景。
///
/// Tauri 命令签名 camelCase 映射：`mode: "PasswordOnly"` 等。
/// 默认（前端不传 / null）：`PasswordOnly`——最稳健，webmail SPA 首选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoTypeMode {
    UsernamePassword,
    PasswordOnly,
    UsernameOnly,
}

impl Default for AutoTypeMode {
    fn default() -> Self {
        // 默认 PasswordOnly：webmail SPA 最稳健，且与现代密码管理器
        // （Bitwarden/1Password 桌面助手）默认行为对齐。
        AutoTypeMode::PasswordOnly
    }
}


// === Tauri 命令 ===

#[tauri::command]
pub fn vault_list_ciphers(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let (ciphers, failures) =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!("vault_list_ciphers: {} 条记录解密失败已跳过", failures.len());
    }
    Ok(ciphers.into_iter().map(cipher_to_dto).collect())
}


// === Folder 命令（follow-up #6） ===
//
// folder.name 与 cipher.name 一致——以 user_vault_key 加密存盘，命令边界只接收 / 返回明文。
// vault_delete_folder 不需要 key（仅删行；FK ON DELETE SET NULL 让 cipher 回到根目录），
// 但仍要求 vault 已解锁——避免未解锁会话误触。

#[tauri::command]
pub fn vault_list_folders(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<Vec<FolderDto>, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    // 修复 #9：list_folders 现返回 (folders, failures)——单行解密失败不让整表 Err。
    // 失败行记 log，前端只看到完好的 folders（坏行由用户重新创建/修复）。
    let (folders, failures) =
        octopus_vault::storage::folder::list_folders(&key).map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!(
            "vault_list_folders: {} 个文件夹解密失败已跳过（ids={:?}）",
            failures.len(),
            failures
        );
    }
    Ok(folders)
}

#[tauri::command]
pub fn vault_create_folder(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    name: String,
) -> Result<String, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    // 2026-07-21 v44：调用方生成 UUID（不再 AUTOINCREMENT）
    let new_id = uuid::Uuid::new_v4().to_string();
    octopus_vault::storage::folder::create_folder(&new_id, &name, &key)
        .map_err(vault_error::to_tauri_error)?;
    Ok(new_id)
}

#[tauri::command]
pub fn vault_rename_folder(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
    name: String,
) -> Result<(), String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::rename_folder(&id, &name, &key)
        .map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_delete_folder(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
) -> Result<(), String> {
    // 不需要 user_vault_key（只删行），但仍要求 vault 已解锁——避免未解锁会话误触。
    let _ = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::delete_folder(&id).map_err(vault_error::to_tauri_error)
}


#[tauri::command]
pub fn vault_get_cipher(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
) -> Result<CipherDto, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(&id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(id.clone())))?;
    Ok(cipher_to_dto(cipher))
}


#[tauri::command]
pub fn vault_create_cipher(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    input: CipherInputDto,
) -> Result<String, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let domain = dto_to_input(input).map_err(|e| vault_error::serialize(&e))?;
    // 2026-07-21 v44：调用方生成 UUID 字符串 id（不再 AUTOINCREMENT）
    let new_id = uuid::Uuid::new_v4().to_string();
    octopus_vault::storage::create_cipher(&new_id, &domain, &key)
        .map_err(vault_error::to_tauri_error)?;
    Ok(new_id)
}


#[tauri::command]
pub fn vault_update_cipher(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
    input: CipherInputDto,
) -> Result<(), String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let domain = dto_to_input(input).map_err(|e| vault_error::serialize(&e))?;

    // MVP：前端 CipherInputDto 不管理 password_history，编辑 cipher 时
    // 直接保留数据库中已有的历史，避免每次保存都把 history 清成 []。
    // （final-review I3）+ password 变化时自动追加条目（follow-up #2）
    let domain =
        match octopus_vault::storage::load_cipher(&id, &key).map_err(vault_error::to_tauri_error)? {
            Some(existing) => merge_password_history(domain, &existing),
            None => domain,
        };

    octopus_vault::storage::save_cipher(&id, &domain, &key).map_err(vault_error::to_tauri_error)
}


#[tauri::command]
pub fn vault_delete_cipher(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
    permanent: bool,
) -> Result<(), String> {
    // C-DELETE-NO-UNLOCK-CHECK 修复（2026-07-25）：与 vault_delete_folder :396 同构——
    // 不需要 user_vault_key（仅删行），但仍要求 vault 已解锁——避免未解锁会话
    // （锁定后他人/恶意前端/DevTools）invoke 删除造成不可恢复丢失。
    let _ = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    // permanent=true 不需要 user_vault_key（只是删行）
    if permanent {
        octopus_vault::storage::permanent_delete(&id).map_err(vault_error::to_tauri_error)
    } else {
        octopus_vault::storage::soft_delete(&id).map_err(vault_error::to_tauri_error)
    }
}


#[tauri::command]
pub fn vault_restore_cipher(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    id: String,
) -> Result<(), String> {
    // C-DELETE-NO-UNLOCK-CHECK 修复：要求 vault 已解锁（与 delete_folder / delete_cipher 同构）
    let _ = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::restore(&id).map_err(vault_error::to_tauri_error)
}


/// 清空回收站：批量永久删除所有软删 cipher。
///
/// 单条失败不中断——返回 `(deleted_count, failed_count)`，前端 toast 提示。
///
/// 清空回收站（SYNC_LOCK 已下沉到 empty_trash 内部，T2 修复）。
#[tauri::command]
pub fn vault_empty_trash(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<(usize, usize), String> {
    // C-DELETE-NO-UNLOCK-CHECK 修复：要求 vault 已解锁（清空回收站是永久删除，不可恢复）
    let _ = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    // T2：锁在 storage::empty_trash 内部（与 meta_lock 下沉 save_vault_meta 一致），
    // 此处不再重复取锁。sync 进行中时 empty_trash 返 Err → to_tauri_error 映射。
    let (deleted, errors) =
        octopus_vault::storage::empty_trash().map_err(vault_error::to_tauri_error)?;
    if !errors.is_empty() {
        log::warn!("vault_empty_trash: {} 条删除失败已跳过", errors.len());
    }
    Ok((deleted, errors.len()))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    // mod.rs 共享 helper / DTO struct / 常量——`super` 在 cipher.rs::tests 中指向
    // cipher.rs 本身，故通过 crate 路径引用 mod.rs 的符号。
    use crate::vault::vault_commands::{
        cipher_to_dto, dto_to_input, merge_password_history, require_app_key_from_session,
        CipherInputDto, PASSWORD_HISTORY_MAX, VAULT_DETECT_MATCH_LIMIT,
    };
    use octopus_vault::crypto::DerivedKey;
    use octopus_vault::types::{
        Cipher, CipherData, CipherInput, CipherType, Field, LoginData, LoginUri, MatchType,
        PasswordHistoryEntry, RepromptType,
    };
    use zeroize::Zeroizing;

    /// 构造一份字段齐全的 Cipher（Login 类型 + favorite + notes + fields）。
    fn sample_cipher() -> Cipher {
        Cipher {
            id: "test-cipher-42".to_string(),
            folder_id: Some("test-folder-7".to_string()),
            favorite: true,
            atype: CipherType::Login,
            name: "Example Login".into(),
            notes: Some("some notes".into()),
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: "https://example.com".into(),
                    match_type: Some(MatchType::Domain),
                }],
                username: Some("user1".into()),
                password: Some("s3cret".into()),
                totp: Some("JBSWY3DPEHPK3PXP".into()),
                password_revision_date: None,
            }),
            fields: vec![Field {
                name: "custom".into(),
                value: Some("v".into()),
                field_type: 0,
            }],
            password_history: vec![PasswordHistoryEntry {
                password: "old".into(),
                last_used_at: "2026-01-01".into(),
            }],
            reprompt: RepromptType::Password,
            is_deleted: false,
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-02T00:00:00".into(),
        }
    }

    /// 构造一份字段齐全的 CipherInputDto（前端 → 后端输入）。
    fn sample_input_dto() -> CipherInputDto {
        CipherInputDto {
            folder_id: Some("test-folder-3".to_string()),
            favorite: false,
            name: "New Entry".into(),
            notes: None,
            login: Some(LoginData {
                uris: vec![LoginUri {
                    uri: "https://test.com".into(),
                    match_type: MatchType::try_from(0).ok(),
                }],
                username: Some("u".into()),
                password: Some("p".into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![Field {
                name: "f".into(),
                value: None,
                field_type: 1,
            }],
            reprompt: Some(1),
        }
    }

    /// cipher_to_dto：所有字段应原样保留（id/folder_id/favorite/name/notes/login/fields/...）。
    #[test]
    fn test_cipher_to_dto_preserves_all_fields() {
        let cipher = sample_cipher();
        let dto = cipher_to_dto(cipher.clone());

        assert_eq!(dto.id, "test-cipher-42");
        assert_eq!(dto.folder_id.as_deref(), Some("test-folder-7"));
        assert!(dto.favorite);
        assert_eq!(dto.atype, 1, "Login 应映射为 atype=1");
        assert_eq!(dto.name, "Example Login");
        assert_eq!(dto.notes.as_deref(), Some("some notes"));
        assert_eq!(dto.reprompt, 1, "RepromptType::Password 应映射为 1");
        assert!(!dto.is_deleted);
        assert_eq!(dto.created_at, "2026-01-01T00:00:00");
        assert_eq!(dto.updated_at, "2026-01-02T00:00:00");

        // login sub-fields
        let login = dto.login.expect("login should be Some for Login cipher");
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri, "https://example.com");
        assert_eq!(login.uris[0].match_type, Some(MatchType::Domain), "MatchType::Domain 应映射为 0");
        assert_eq!(login.username.as_deref(), Some("user1"));
        assert_eq!(login.password.as_deref(), Some("s3cret"));
        assert_eq!(login.totp.as_deref(), Some("JBSWY3DPEHPK3PXP"));

        // fields
        assert_eq!(dto.fields.len(), 1);
        assert_eq!(dto.fields[0].name, "custom");
        assert_eq!(dto.fields[0].value.as_deref(), Some("v"));
        assert_eq!(dto.fields[0].field_type, 0);
    }

    /// cipher_to_dto：空 uris / 无 notes 边界情况。
    #[test]
    fn test_cipher_to_dto_handles_empty_optionals() {
        let mut cipher = sample_cipher();
        cipher.notes = None;
        // CipherData 当前仅 Login 单变体；保留不可达 arm 以便未来扩展。
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(ref mut l) = cipher.data {
            l.uris.clear();
            l.totp = None;
        }
        let dto = cipher_to_dto(cipher);
        assert!(dto.notes.is_none());
        let login = dto.login.expect("login present");
        assert!(login.uris.is_empty());
        assert!(login.totp.is_none());
    }

    /// dto_to_input：完整 DTO → CipherInput，字段应保留；login 必须存在。
    #[test]
    fn test_dto_to_input_preserves_fields() {
        let dto = sample_input_dto();
        let input = dto_to_input(dto).expect("conversion should succeed");

        assert_eq!(input.folder_id.as_deref(), Some("test-folder-3"));
        assert!(!input.favorite);
        assert_eq!(input.name, "New Entry");
        assert!(input.notes.is_none());
        assert!(matches!(input.atype, CipherType::Login));
        assert!(matches!(input.reprompt, RepromptType::Password));

        // login data
        #[allow(irrefutable_let_patterns)]
        let CipherData::Login(login) = input.data else {
            panic!("应为 Login");
        };
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri, "https://test.com");
        assert_eq!(
            login.uris[0].match_type,
            Some(MatchType::Domain),
            "match_type=0 应转回 MatchType::Domain"
        );
        assert_eq!(login.username.as_deref(), Some("u"));
        assert_eq!(login.password.as_deref(), Some("p"));
        assert!(login.totp.is_none());

        // fields
        assert_eq!(input.fields.len(), 1);
        assert_eq!(input.fields[0].name, "f");
        assert!(input.fields[0].value.is_none());
        assert_eq!(input.fields[0].field_type, 1);

        // password_history 应初始化为空（dto 不携带，由 update 命令补）
        assert!(input.password_history.is_empty());
    }

    /// dto_to_input：reprompt=None 时默认 RepromptType::None。
    #[test]
    fn test_dto_to_input_defaults_reprompt_to_none() {
        let mut dto = sample_input_dto();
        dto.reprompt = None;
        let input = dto_to_input(dto).expect("convert");
        assert!(matches!(input.reprompt, RepromptType::None));
    }

    /// dto_to_input：login=None 时应失败（MVP 仅支持 Login）。
    #[test]
    fn test_dto_to_input_requires_login() {
        let mut dto = sample_input_dto();
        dto.login = None;
        let result = dto_to_input(dto);
        assert!(result.is_err(), "dto_to_input 应在 login 缺失时返回 Err");
    }

    /// cipher_to_dto + dto_to_input 双向：dto_to_input 的产物再回 dto，核心字段一致。
    /// 这覆盖了「同一份数据在两套 DTO 间不丢字段」的核心不变量。
    #[test]
    fn test_round_trip_dto_to_input_then_back() {
        let dto_in = sample_input_dto();
        // 记下关键期望值（dto_to_input 会消费 dto_in，无法后续比对）
        let exp_folder = dto_in.folder_id.clone();
        let exp_favorite = dto_in.favorite;
        let exp_name = dto_in.name.clone();
        let exp_notes = dto_in.notes.clone();
        let exp_reprompt = dto_in.reprompt.unwrap_or(0);
        let exp_username = dto_in.login.as_ref().and_then(|l| l.username.clone());
        let exp_fields_len = dto_in.fields.len();
        let input = dto_to_input(dto_in).expect("convert");

        // 构造一个完整 Cipher（补 id/时间戳/password_history——dto_to_input 不产出这些）
        let cipher = Cipher {
            id: "round-trip-99".to_string(),
            folder_id: input.folder_id.clone(),
            favorite: input.favorite,
            atype: input.atype,
            name: input.name.clone(),
            notes: input.notes.clone(),
            data: input.data.clone(),
            fields: input.fields.clone(),
            password_history: vec![],
            reprompt: input.reprompt,
            is_deleted: false,
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-01T00:00:00".into(),
        };
        let dto_out = cipher_to_dto(cipher);

        assert_eq!(dto_out.folder_id, exp_folder);
        assert_eq!(dto_out.favorite, exp_favorite);
        assert_eq!(dto_out.name, exp_name);
        assert_eq!(dto_out.notes, exp_notes);
        assert_eq!(dto_out.reprompt, exp_reprompt);
        assert_eq!(
            dto_out.login.as_ref().and_then(|l| l.username.clone()),
            exp_username
        );
        assert_eq!(dto_out.fields.len(), exp_fields_len);
    }

    // === 边界 case：match_type 全覆盖 + 失败回退 ===

    /// cipher_to_dto：多个 URI 各带不同的 match_type（Some(Domain)/Some(Host)/None）。
    /// 关键不变量：Some 应映射为 Some(0)/Some(1)，None 必须保持 None（不能用 0 替代）。
    #[test]
    fn test_cipher_to_dto_preserves_uri_match_types() {
        let mut cipher = sample_cipher();
        cipher.data = CipherData::Login(LoginData {
            uris: vec![
                LoginUri {
                    uri: "https://a.com".into(),
                    match_type: Some(MatchType::Domain), // → 0
                },
                LoginUri {
                    uri: "https://b.com".into(),
                    match_type: Some(MatchType::Host), // → 1
                },
                LoginUri {
                    uri: "https://c.com".into(),
                    match_type: None, // 必须保持 None
                },
            ],
            username: None,
            password: None,
            totp: None,
            password_revision_date: None,
        });
        let dto = cipher_to_dto(cipher);
        let login = dto.login.expect("login present");
        assert_eq!(login.uris.len(), 3);
        assert_eq!(login.uris[0].match_type, Some(MatchType::Domain), "Domain → 0");
        assert_eq!(login.uris[1].match_type, Some(MatchType::Host), "Host → 1");
        assert_eq!(login.uris[2].match_type, None, "None 必须保持 None");
    }

    /// dto_to_input：覆盖所有 6 种合法 match_type（0..=5）均能正确映射。
    /// 这是「同一份数据在两套 DTO 间不丢字段」的扩展验证（覆盖完整枚举而非仅 Domain）。
    #[test]
    fn test_dto_to_input_preserves_all_match_types() {
        let mut dto = sample_input_dto();
        dto.login = Some(LoginData {
            uris: vec![
                LoginUri { uri: "u0".into(), match_type: MatchType::try_from(0).ok() }, // Domain
                LoginUri { uri: "u1".into(), match_type: MatchType::try_from(1).ok() }, // Host
                LoginUri { uri: "u2".into(), match_type: MatchType::try_from(2).ok() }, // StartsWith（Bitwarden 官方 2）
                LoginUri { uri: "u3".into(), match_type: MatchType::try_from(3).ok() }, // Exact（Bitwarden 官方 3）
                LoginUri { uri: "u4".into(), match_type: MatchType::try_from(4).ok() }, // RegularExpression
                LoginUri { uri: "u5".into(), match_type: MatchType::try_from(5).ok() }, // Never
                LoginUri { uri: "u_none".into(), match_type: None },
            ],
            username: None,
            password: None,
            totp: None,
            password_revision_date: None,
        });
        let input = dto_to_input(dto).expect("convert");
        #[allow(irrefutable_let_patterns)]
        let CipherData::Login(login) = input.data else {
            panic!("应为 Login");
        };
        assert_eq!(login.uris.len(), 7);
        assert_eq!(login.uris[0].match_type, Some(MatchType::Domain));
        assert_eq!(login.uris[1].match_type, Some(MatchType::Host));
        // 2026-07-24 #1 修复后对齐 Bitwarden 官方值：2=StartsWith, 3=Exact
        assert_eq!(login.uris[2].match_type, Some(MatchType::StartsWith));
        assert_eq!(login.uris[3].match_type, Some(MatchType::Exact));
        assert_eq!(login.uris[4].match_type, Some(MatchType::RegularExpression));
        assert_eq!(login.uris[5].match_type, Some(MatchType::Never));
        assert_eq!(login.uris[6].match_type, None, "None 必须原样保留");
    }

    /// dto_to_input：非法 match_type（99）应回退为 None（MatchType::try_from 失败 → .ok() = None）。
    /// 关键：不能 panic，不能整条转换失败——仅该 URI 的 match_type 降级为 None。
    #[test]
    fn test_dto_to_input_invalid_match_type_falls_back_to_none() {
        let mut dto = sample_input_dto();
        dto.login = Some(LoginData {
            uris: vec![
                LoginUri { uri: "valid".into(), match_type: MatchType::try_from(0).ok() },
                LoginUri { uri: "invalid".into(), match_type: MatchType::try_from(99).ok() },
            ],
            username: None,
            password: None,
            totp: None,
            password_revision_date: None,
        });
        let input = dto_to_input(dto).expect("整条转换应成功（单 URI match_type 非法不影响整体）");
        #[allow(irrefutable_let_patterns)]
        let CipherData::Login(login) = input.data else {
            panic!("应为 Login");
        };
        assert_eq!(login.uris.len(), 2);
        assert_eq!(login.uris[0].match_type, Some(MatchType::Domain));
        assert_eq!(
            login.uris[1].match_type,
            None,
            "非法 match_type=99 应回退为 None"
        );
    }

    /// dto_to_input：folder_id=None 时 CipherInput.folder_id 应为 None（不要回退到 0）。
    #[test]
    fn test_dto_to_input_preserves_none_folder_id() {
        let mut dto = sample_input_dto();
        dto.folder_id = None;
        let input = dto_to_input(dto).expect("convert");
        assert_eq!(input.folder_id, None);
    }

    /// dto_to_input：空 fields vec 应原样保留（不应被替换为默认值或 panic）。
    #[test]
    fn test_dto_to_input_preserves_empty_fields() {
        let mut dto = sample_input_dto();
        dto.fields = vec![];
        let input = dto_to_input(dto).expect("convert");
        assert!(input.fields.is_empty());
    }

    /// cipher_to_dto：empty collections（fields、uris）应转为空数组而非保持为空。
    /// （DTO 结构本身用 Vec<FieldDto>，天然为 []；此处显式断言该不变量。）
    #[test]
    fn test_cipher_to_dto_empty_collections_to_empty_arrays() {
        let mut cipher = sample_cipher();
        cipher.fields = vec![];
        cipher.data = CipherData::Login(LoginData {
            uris: vec![],
            username: None,
            password: None,
            totp: None,
            password_revision_date: None,
        });
        let dto = cipher_to_dto(cipher);
        assert!(dto.fields.is_empty(), "empty fields 应映射为 []");
        let login = dto.login.expect("login present");
        assert!(login.uris.is_empty(), "empty uris 应映射为 []");
    }

    // === Follow-up #2: password_history 自动追加 ===

    /// 用真实 in-memory DB + set_test_keychain 跑 setup_vault，得到 user_vault_key，
    /// 然后用它构造 CipherInput（走真实 storage::create_cipher / save_cipher / load_cipher
    /// 全链路），方便后续 test_vault_update_cipher_* 系列测试。
    fn setup_test_vault_with_key() -> Arc<DerivedKey> {
        use rusqlite::Connection;
        // 干净 DB + 干净 Keychain（thread_local 隔离，互不污染）
        octopus_infra::db::set_test_db(Connection::open_in_memory().expect("in-memory DB"));
        octopus_vault::keychain::set_test_keychain();
        let _ = octopus_vault::keychain::delete_machine_key();

        let keys = octopus_vault::unlock::setup_vault(Zeroizing::new("Test-master-pw1!".into()))
            .expect("setup_vault");
        let _ = octopus_vault::keychain::delete_machine_key();
        Arc::new(keys.user_vault_key)
    }

    /// 构造一条带 password 的 Login CipherInput（其他字段最小化）。
    fn login_input(name: &str, password: &str) -> CipherInput {
        CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: name.into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![],
                username: Some("u".into()),
                password: Some(password.into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
        }
    }

    /// 端到端测试：password 变化时自动追加 history 条目。
    ///
    /// 流程：
    ///   1. create cipher with "A"
    ///   2. update → "B" → history 应有 1 条 (A)
    ///   3. update → "C" → history 应有 2 条 [A, B]
    ///   4. update → "C" (不变) → history 仍 2 条（无新条目）
    #[test]
    fn test_vault_update_cipher_appends_history_on_password_change() {
        let key = setup_test_vault_with_key();

        // 1. create with "A"
        let id = "history-test-1";
        octopus_vault::storage::create_cipher(id, &login_input("site", "A"), &key)
            .expect("create");

        // 2. update → "B" → history 1 条
        let update_b = login_input("site", "B");
        let existing = octopus_vault::storage::load_cipher(id, &key)
            .expect("load")
            .expect("exists");
        let merged = merge_password_history(update_b, &existing);
        octopus_vault::storage::save_cipher(id, &merged, &key).expect("save");
        let loaded = octopus_vault::storage::load_cipher(id, &key)
            .expect("load")
            .expect("exists");
        assert_eq!(loaded.password_history.len(), 1, "改 A→B 应追加 1 条");
        assert_eq!(loaded.password_history[0].password, "A");

        // 3. update → "C" → history 2 条 [A, B]
        let update_c = login_input("site", "C");
        let existing = loaded;
        let merged = merge_password_history(update_c, &existing);
        octopus_vault::storage::save_cipher(id, &merged, &key).expect("save");
        let loaded = octopus_vault::storage::load_cipher(id, &key)
            .expect("load")
            .expect("exists");
        assert_eq!(loaded.password_history.len(), 2, "改 B→C 应再追加 1 条");
        assert_eq!(loaded.password_history[0].password, "A");
        assert_eq!(loaded.password_history[1].password, "B");

        // 4. update → "C" (不变) → history 仍 2 条
        let update_c_again = login_input("site", "C");
        let existing = loaded;
        let merged = merge_password_history(update_c_again, &existing);
        octopus_vault::storage::save_cipher(id, &merged, &key).expect("save");
        let loaded = octopus_vault::storage::load_cipher(id, &key)
            .expect("load")
            .expect("exists");
        assert_eq!(
            loaded.password_history.len(),
            2,
            "password 未变（C→C）不应追加条目"
        );
    }

    /// password_history 上限：连续改 25 次密码后条目应被截断到 PASSWORD_HISTORY_MAX。
    #[test]
    fn test_vault_update_cipher_history_cap() {
        let key = setup_test_vault_with_key();
        let id = "history-cap-test";
        octopus_vault::storage::create_cipher(id, &login_input("site", "p0"), &key)
            .expect("create");

        // 连续改 25 次密码（p0 → p1 → ... → p25）
        for i in 1..=25 {
            let existing = octopus_vault::storage::load_cipher(id, &key)
                .expect("load")
                .expect("exists");
            let input = login_input("site", &format!("p{}", i));
            let merged = merge_password_history(input, &existing);
            octopus_vault::storage::save_cipher(id, &merged, &key).expect("save");
        }

        let loaded = octopus_vault::storage::load_cipher(id, &key)
            .expect("load")
            .expect("exists");
        assert_eq!(
            loaded.password_history.len(),
            PASSWORD_HISTORY_MAX,
            "history 应被截断到 PASSWORD_HISTORY_MAX"
        );
        // 应丢最老的（FIFO）—— p0..p4 应被丢掉，剩下 p5..p24
        // 注意：每次 save 时 existing.updated_at 是上一轮 save 写入的时间戳，
        // 这里只验证数量 + 最新一条是上一轮的密码（p24）。
        assert_eq!(loaded.password_history.last().unwrap().password, "p24");
    }

    // === Follow-up #7: secret_key 透明解密集成测试 ===

    /// 用真实 setup_vault 得到 user_vault_key + app_key（含真实 keychain 注入路径）。
    /// 与 setup_test_vault_with_key 对称，但额外返回 app_key——follow-up #7 的
    /// secret_key 解密要用 app_key。
    fn setup_test_vault_with_keys() -> (Arc<DerivedKey>, Arc<DerivedKey>) {
        use rusqlite::Connection;
        octopus_infra::db::set_test_db(Connection::open_in_memory().expect("in-memory DB"));
        octopus_vault::keychain::set_test_keychain();
        let _ = octopus_vault::keychain::delete_machine_key();

        let keys = octopus_vault::unlock::setup_vault(Zeroizing::new("Test-master-pw1!".into()))
            .expect("setup_vault");
        let _ = octopus_vault::keychain::delete_machine_key();
        (Arc::new(keys.user_vault_key), Arc::new(keys.app_key))
    }

    /// 构造一个 app_key 已注入的 SharedVaultSession。
    fn session_with_app_key(app_key: Arc<DerivedKey>) -> SharedVaultSession {
        use parking_lot::RwLock;
        let mut s = crate::vault::vault_state::VaultSession::default();
        s.app_key = Some(app_key);
        Arc::new(RwLock::new(s))
    }

    /// 向 models 表插一行云端模型（is_local=0）+ 指定 secret_key，返回新行 id。
    /// 仅提供 NOT NULL 无默认值的字段；UNIQUE 通过 model_name 随机后缀避免冲突。
    fn insert_cloud_test_model(secret_key: &str) -> i64 {
        use rusqlite::params;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let suffix = SEQ.fetch_add(1, Ordering::SeqCst);
        let model_name = format!("test-cloud-model-{}", suffix);

        octopus_infra::db::with_db(|conn| {
            conn.execute(
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, source_type)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    "llm",
                    "test_provider",
                    "test_category",
                    model_name,
                    "https://test-source.example.com",
                    secret_key,
                    2, // source_type=2 → 云端模型
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .expect("insert test cloud model should succeed")
    }

    /// 直接按 model_name 读 models.secret_key 原值（验证迁移效果用）。
    fn read_cloud_model_secret_by_name(model_name: &str) -> String {
        use rusqlite::params;
        octopus_infra::db::with_db(|conn| {
            let v: String = conn.query_row(
                "SELECT secret_key FROM models WHERE model_name = ?",
                params![model_name],
                |r| r.get(0),
            )?;
            Ok(v)
        })
        .expect("read secret_key should succeed")
    }

    /// 端到端集成测试：Task 20 迁移后，read_model_secret_key 能还原明文 API Key。
    ///
    /// 流程（模拟 vault setup → cloud key 加密迁移 → 推理热路径消费）：
    ///   1. setup_vault → 得到 app_key
    ///   2. 插入一行 cloud model（is_local=0）含明文 secret_key
    ///   3. 调 migrate_secret_keys_to_encrypted(app_key) → DB 中 secret_key 变 v1: 密文
    ///   4. 调 read_model_secret_key(model_name, &session) → 应返回原明文 API Key
    ///
    /// 若 chokepoint 缺失（推理热路径直接读 DB 原值），返回的会是 "v1:..." 密文 →
    /// 上层 HTTP Bearer 会把加密 blob 当 API Key 发出去 → 401。
    #[test]
    fn test_read_model_secret_key_round_trip_after_migration() {
        let (_user_key, app_key) = setup_test_vault_with_keys();
        let session = session_with_app_key(app_key.clone());

        // 1. 插入云端模型（明文 secret_key）
        let plaintext = "sk-test-cloud-api-key-12345";
        // 取 model_name（insert_cloud_test_model 内部生成，需要回读）
        let _id = insert_cloud_test_model(plaintext);
        // 拿到 model_name（按 secret_key 反查，确保后续 read 找得到）
        let model_name =
            octopus_infra::db::with_db(|conn| {
                let n: String = conn.query_row(
                    "SELECT model_name FROM models WHERE secret_key = ?",
                    rusqlite::params![plaintext],
                    |r| r.get(0),
                )?;
                Ok(n)
            })
            .expect("find model_name");

        // 2. 迁移前：secret_key 仍是明文
        assert_eq!(read_cloud_model_secret_by_name(&model_name), plaintext);

        // 3. 迁移：migrate_secret_keys_to_encrypted 把所有 is_local=0 行加密
        let count = octopus_vault::migrate::migrate_secret_keys_to_encrypted(&app_key)
            .expect("migrate");
        assert!(count >= 1, "至少应迁移 1 行");

        // 迁移后：DB 里是 v1: 密文（不再是明文）
        let migrated = read_cloud_model_secret_by_name(&model_name);
        assert!(
            migrated.starts_with("v1:"),
            "迁移后 secret_key 应以 v1: 开头，got: {}",
            migrated
        );
        assert_ne!(migrated, plaintext, "迁移后 DB 不应再存明文");

        // 4. chokepoint 应还原明文（这是 #7 修复的核心断言）
        let decrypted = require_app_key_from_session(&session).ok();
        assert!(decrypted.is_some(), "app_key 应可取");

        // 直接调 read_model_secret_key（ chokepoint 入口）
        let result = crate::vault::vault_secret_access::read_model_secret_key(&model_name, &session)
            .expect("read_model_secret_key 应成功");
        assert_eq!(
            result, plaintext,
            "read_model_secret_key 应还原为原明文 API Key"
        );
    }
    /// 未迁移的明文 secret_key：read_model_secret_key 应原样返回（向后兼容）。
    #[test]
    fn test_read_model_secret_key_passthrough_plaintext() {
        let (_user_key, app_key) = setup_test_vault_with_keys();
        let session = session_with_app_key(app_key);

        let plaintext = "sk-plain-unmigrated-key";
        let _id = insert_cloud_test_model(plaintext);
        let model_name =
            octopus_infra::db::with_db(|conn| {
                let n: String = conn.query_row(
                    "SELECT model_name FROM models WHERE secret_key = ?",
                    rusqlite::params![plaintext],
                    |r| r.get(0),
                )?;
                Ok(n)
            })
            .expect("find model_name");

        // 未调 migrate → DB 仍是明文 → read_model_secret_key 原样返回
        let result = crate::vault::vault_secret_access::read_model_secret_key(&model_name, &session)
            .expect("read should succeed");
        assert_eq!(result, plaintext);
    }

    /// 本地模型 manifest JSON（is_local=1）：read_model_secret_key 应原样返回。
    /// 迁移跳过 is_local=1 行（migrate.rs 的 SQL 含 is_local=0 守卫），且 helper
    /// 按 v1: 前缀判定——manifest JSON 不以 v1: 开头，直接 passthrough。
    #[test]
    fn test_read_model_secret_key_local_manifest_passthrough() {
        let (_user_key, app_key) = setup_test_vault_with_keys();
        let session = session_with_app_key(app_key.clone());

        // 插入一行本地模型（is_local=1，secret_key 是 manifest JSON）
        use rusqlite::params;
        let manifest = r#"{"version":"1.0","files":[{"path":"a.onnx","sha256":"abc"}]}"#;
        let model_name = "test-local-manifest-model";
        octopus_infra::db::with_db(|conn| {
            conn.execute(
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, source_type)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    "asr",
                    "local",
                    "whisper",
                    model_name,
                    "test/repo",
                    manifest,
                    1,
                ],
            )?;
            Ok(())
        })
        .expect("insert local model");

        // 跑 migrate（不应动 is_local=1 行）
        let _ = octopus_vault::migrate::migrate_secret_keys_to_encrypted(&app_key)
            .expect("migrate");

        // helper 应原样返回 manifest JSON（不解密）
        let result = crate::vault::vault_secret_access::read_model_secret_key(model_name, &session)
            .expect("read should succeed");
        assert_eq!(result, manifest);
    }

    // === Follow-up #8: detect_and_match fallback 限制 ===

    /// 常量应为 spec 规定的值（20 / 50）。
    #[test]
    fn test_detect_match_limits_are_spec_values() {
        assert_eq!(VAULT_DETECT_MATCH_LIMIT, 50);
    }
}
