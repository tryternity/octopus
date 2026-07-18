//! vault Tauri 命令层。
//!
//! 命令返回类型用 DTO（避免直接暴露 vault crate 内部类型）。
//! 错误统一映射为 `String`（前端用 `err` 分支即可）。

use std::sync::Arc;

use tauri::State;

use octopus_vault::crypto::DerivedKey;
use octopus_vault::generator::GeneratorConfig;
use octopus_vault::health::HealthReport;
use octopus_vault::importer::ImportReport;
use octopus_vault::types::{Cipher, CipherData, CipherInput, CipherType, RepromptType};

use crate::vault_state::SharedVaultSession;

// === DTO ===

#[derive(serde::Serialize)]
pub struct VaultStatusDto {
    pub initialized: bool,
    pub user_vault_unlocked: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct LoginUriDto {
    pub uri: String,
    pub match_type: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct LoginDataDto {
    pub uris: Vec<LoginUriDto>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub totp: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FieldDto {
    pub name: String,
    pub value: Option<String>,
    pub field_type: i64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CipherDto {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginDataDto>,
    pub fields: Vec<FieldDto>,
    pub reprompt: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(serde::Deserialize)]
pub struct CipherInputDto {
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginDataDto>,
    pub fields: Vec<FieldDto>,
    pub reprompt: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct TotpResultDto {
    pub code: String,
    pub seconds_remaining: u64,
}

// === AppState key 取用辅助 ===

/// 从 AppState 取 user_vault_key（必须解锁），否则返回 Err。
fn require_user_vault_key(state: &State<'_, SharedVaultSession>) -> Result<Arc<DerivedKey>, String> {
    let session = state.read();
    if !session.is_user_vault_unlocked() {
        return Err("vault 未解锁".into());
    }
    session
        .user_vault_key
        .clone()
        .ok_or_else(|| "vault 未解锁".to_string())
}

/// 从 AppState 取 app_key（启动时已 bootstrap，不应为空）。
#[allow(dead_code)] // Task 20+ 会接入 app_key 调用方
fn require_app_key(state: &State<'_, SharedVaultSession>) -> Result<Arc<DerivedKey>, String> {
    let session = state.read();
    session
        .app_key
        .clone()
        .ok_or_else(|| "vault app_key 不可用".to_string())
}

// === DTO ↔ Domain 转换 ===

fn cipher_to_dto(c: Cipher) -> CipherDto {
    // CipherData 当前仅 Login 单变体；保留 match 以便未来扩展 SecureNote/Card/Identity。
    #[allow(irrefutable_let_patterns)]
    let (login, atype) = match &c.data {
        CipherData::Login(l) => (
            Some(LoginDataDto {
                uris: l
                    .uris
                    .iter()
                    .map(|u| LoginUriDto {
                        uri: u.uri.clone(),
                        match_type: u.match_type.map(|m| m.into()),
                    })
                    .collect(),
                username: l.username.clone(),
                password: l.password.clone(),
                totp: l.totp.clone(),
            }),
            1,
        ),
    };
    CipherDto {
        id: c.id,
        folder_id: c.folder_id,
        favorite: c.favorite,
        atype,
        name: c.name,
        notes: c.notes,
        login,
        fields: c
            .fields
            .iter()
            .map(|f| FieldDto {
                name: f.name.clone(),
                value: f.value.clone(),
                field_type: f.field_type,
            })
            .collect(),
        reprompt: c.reprompt.into(),
        deleted_at: c.deleted_at,
        created_at: c.created_at,
        updated_at: c.updated_at,
    }
}

fn dto_to_input(dto: CipherInputDto) -> Result<CipherInput, String> {
    let login = dto.login.ok_or_else(|| "login 必填".to_string())?;
    Ok(CipherInput {
        folder_id: dto.folder_id,
        favorite: dto.favorite,
        atype: CipherType::Login,
        name: dto.name,
        notes: dto.notes,
        data: CipherData::Login(octopus_vault::types::LoginData {
            uris: login
                .uris
                .into_iter()
                .map(|u| octopus_vault::types::LoginUri {
                    uri: u.uri,
                    match_type: u
                        .match_type
                        .and_then(|m| octopus_vault::types::MatchType::try_from(m).ok()),
                })
                .collect(),
            username: login.username,
            password: login.password,
            totp: login.totp,
            password_revision_date: None,
        }),
        fields: dto
            .fields
            .into_iter()
            .map(|f| octopus_vault::types::Field {
                name: f.name,
                value: f.value,
                field_type: f.field_type,
            })
            .collect(),
        password_history: vec![],
        reprompt: dto
            .reprompt
            .map(RepromptType::from)
            .unwrap_or(RepromptType::None),
    })
}

// === Tauri 命令 ===

#[tauri::command]
pub fn vault_status(state: State<'_, SharedVaultSession>) -> Result<VaultStatusDto, String> {
    let initialized = octopus_vault::unlock::is_initialized().map_err(|e| e.to_string())?;
    let user_vault_unlocked = state.read().is_user_vault_unlocked();
    Ok(VaultStatusDto {
        initialized,
        user_vault_unlocked,
    })
}

#[tauri::command]
pub fn vault_setup(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys = octopus_vault::unlock::setup_vault(&password).map_err(|e| e.to_string())?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_unlock(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys =
        octopus_vault::unlock::unlock_with_master_password(&password).map_err(|e| e.to_string())?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_lock(state: State<'_, SharedVaultSession>) -> Result<(), String> {
    state.write().lock_user_vault();
    Ok(())
}

#[tauri::command]
pub fn vault_change_password(
    state: State<'_, SharedVaultSession>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    octopus_vault::unlock::change_master_password(&old_password, &new_password)
        .map_err(|e| e.to_string())?;
    // 改密码后不主动解锁 user_vault（让用户重新输）
    state.write().lock_user_vault();
    Ok(())
}

#[tauri::command]
pub fn vault_list_ciphers(state: State<'_, SharedVaultSession>) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    Ok(ciphers.into_iter().map(cipher_to_dto).collect())
}

#[tauri::command]
pub fn vault_get_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
) -> Result<CipherDto, String> {
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("cipher {} 不存在", id))?;
    Ok(cipher_to_dto(cipher))
}

#[tauri::command]
pub fn vault_create_cipher(
    state: State<'_, SharedVaultSession>,
    input: CipherInputDto,
) -> Result<i64, String> {
    let key = require_user_vault_key(&state)?;
    let domain = dto_to_input(input)?;
    octopus_vault::storage::create_cipher(&domain, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_update_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
    input: CipherInputDto,
) -> Result<(), String> {
    let key = require_user_vault_key(&state)?;
    let domain = dto_to_input(input)?;
    octopus_vault::storage::save_cipher(id, &domain, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_delete_cipher(
    _state: State<'_, SharedVaultSession>,
    id: i64,
    permanent: bool,
) -> Result<(), String> {
    // permanent=true 不需要 user_vault_key（只是删行）
    if permanent {
        octopus_vault::storage::permanent_delete(id).map_err(|e| e.to_string())
    } else {
        octopus_vault::storage::soft_delete(id).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn vault_restore_cipher(_state: State<'_, SharedVaultSession>, id: i64) -> Result<(), String> {
    octopus_vault::storage::restore(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_generate(cfg: GeneratorConfig) -> Result<String, String> {
    Ok(octopus_vault::generator::generate(&cfg))
}

#[tauri::command]
pub fn vault_generate_totp(
    state: State<'_, SharedVaultSession>,
    cipher_id: i64,
) -> Result<TotpResultDto, String> {
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;
    // CipherData 当前仅 Login 单变体；预留 unreachable arm 以便未来扩展。
    #[allow(unreachable_patterns)]
    let login = match cipher.data {
        CipherData::Login(l) => l,
        _ => return Err("非 Login 类型".into()),
    };
    let totp_secret = login.totp.ok_or_else(|| "无 TOTP secret".to_string())?;
    let gen = octopus_vault::totp::TotpGenerator::from_base32(&totp_secret)
        .map_err(|e| e.to_string())?;
    Ok(TotpResultDto {
        code: gen.current().map_err(|e| e.to_string())?,
        seconds_remaining: gen.seconds_remaining(),
    })
}

#[tauri::command]
pub fn vault_health_report(state: State<'_, SharedVaultSession>) -> Result<HealthReport, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    Ok(octopus_vault::health::generate_report(&ciphers))
}

#[tauri::command]
pub fn vault_import_bitwarden(
    state: State<'_, SharedVaultSession>,
    json: String,
) -> Result<ImportReport, String> {
    let key = require_user_vault_key(&state)?;
    octopus_vault::importer::import_bitwarden_json(&json, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_export(state: State<'_, SharedVaultSession>) -> Result<String, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    octopus_vault::importer::export_vault_json(&ciphers).map_err(|e| e.to_string())
}
