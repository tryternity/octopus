//! vault Tauri 命令层。
//!
//! 命令返回类型用 DTO（避免直接暴露 vault crate 内部类型）。
//! 错误统一映射为 `String`（前端用 `err` 分支即可）。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use octopus_clipboard::ClipboardHandle;
use octopus_vault::crypto::DerivedKey;
use octopus_vault::generator::GeneratorConfig;
use octopus_vault::health::HealthReport;
use octopus_vault::importer::ImportReport;
use octopus_vault::storage::FolderDto;
use octopus_vault::types::{Cipher, CipherData, CipherInput, CipherType, RepromptType};

use crate::autotype;
use crate::vault_error::{self, VaultError};
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

/// 从 AppState 取 user_vault_key（必须解锁），否则返回 [`VaultError::Locked`]。
///
/// 直接返回精确的 `VaultError`（而非走字符串启发式 `classify`）——这是命令层
/// 已知语义的最精确分类点。调用方用 `.map_err(|e| vault_error::serialize(&e))?`
/// 转 Tauri 的 `Result<_, String>`。
fn require_user_vault_key(
    state: &State<'_, SharedVaultSession>,
) -> Result<Arc<DerivedKey>, VaultError> {
    let session = state.read();
    if !session.is_user_vault_unlocked() {
        return Err(VaultError::Locked);
    }
    session.user_vault_key.clone().ok_or(VaultError::Locked)
}

/// 从 AppState 取 app_key（启动时已 bootstrap，不应为空）。
///
/// follow-up #7 起被 `vault_secret_access::try_decrypt_secret` 复用——cloud 推理热路径
/// 需要用 app_key 解 `v1:` 前缀的 secret_key。
///
/// 提供裸 `SharedVaultSession` 版本（非命令层调用点：`vault_secret_access`、
/// 未来其它内部消费方）。Tauri 命令层若需用，从 `state.inner()` 取 `&SharedVaultSession`
/// 传入即可。
///
/// 返回 [`VaultError::KeychainUnavailable`] 表达「app_key 不可用」——通常意味着
/// bootstrap 失败（keychain 拒访）或 vault 未初始化。
pub(crate) fn require_app_key_from_session(
    session: &SharedVaultSession,
) -> Result<Arc<DerivedKey>, VaultError> {
    let s = session.read();
    s.app_key.clone().ok_or(VaultError::KeychainUnavailable)
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

fn dto_to_input(dto: CipherInputDto) -> Result<CipherInput, VaultError> {
    let login = dto.login.ok_or_else(|| VaultError::InvalidInput("login 必填".into()))?;
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
    let initialized = octopus_vault::unlock::is_initialized().map_err(vault_error::to_tauri_error)?;
    let user_vault_unlocked = state.read().is_user_vault_unlocked();
    Ok(VaultStatusDto {
        initialized,
        user_vault_unlocked,
    })
}

#[tauri::command]
pub fn vault_setup(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys =
        octopus_vault::unlock::setup_vault(&password).map_err(vault_error::to_tauri_error)?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_unlock(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys = octopus_vault::unlock::unlock_with_master_password(&password)
        .map_err(vault_error::to_tauri_error)?;
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
    let keys = octopus_vault::unlock::change_master_password(&old_password, &new_password)
        .map_err(vault_error::to_tauri_error)?;
    // 改密码成功后用返回的 keys 刷新 session（即使之前是 locked 也 re-unlock）。
    // user_vault_key / app_key 在改密码流程中不变（INV-7），但显式刷一下让
    // 「先 lock 再改密码」也能用——跟 setup_vault / vault_unlock 一致。
    // （follow-up #3）
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_list_ciphers(state: State<'_, SharedVaultSession>) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let ciphers =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    Ok(ciphers.into_iter().map(cipher_to_dto).collect())
}

// === Folder 命令（follow-up #6） ===
//
// folder.name 与 cipher.name 一致——以 user_vault_key 加密存盘，命令边界只接收 / 返回明文。
// vault_delete_folder 不需要 key（仅删行；FK ON DELETE SET NULL 让 cipher 回到根目录），
// 但仍要求 vault 已解锁——避免未解锁会话误触。

#[tauri::command]
pub fn vault_list_folders(state: State<'_, SharedVaultSession>) -> Result<Vec<FolderDto>, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::list_folders(&key).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_create_folder(
    state: State<'_, SharedVaultSession>,
    name: String,
) -> Result<i64, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::create_folder(&name, &key).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_rename_folder(
    state: State<'_, SharedVaultSession>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::rename_folder(id, &name, &key)
        .map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_delete_folder(
    state: State<'_, SharedVaultSession>,
    id: i64,
) -> Result<(), String> {
    // 不需要 user_vault_key（只删行），但仍要求 vault 已解锁——避免未解锁会话误触。
    let _ = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::folder::delete_folder(id).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_get_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
) -> Result<CipherDto, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(id)))?;
    Ok(cipher_to_dto(cipher))
}

#[tauri::command]
pub fn vault_create_cipher(
    state: State<'_, SharedVaultSession>,
    input: CipherInputDto,
) -> Result<i64, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let domain = dto_to_input(input).map_err(|e| vault_error::serialize(&e))?;
    octopus_vault::storage::create_cipher(&domain, &key).map_err(vault_error::to_tauri_error)
}

/// password_history 上限（避免无界增长）。FIFO 截断：丢最老的。
pub const PASSWORD_HISTORY_MAX: usize = 20;

/// `vault_detect_and_match` URL 检测失败时的 fallback 上限（follow-up #8）。
///
/// URL 检测失败时按 `updated_at DESC` 取最近使用过的 N 条，让用户手动选——
/// 避免大 vault（500+）全量返回的噪音和延迟。
pub const VAULT_DETECT_FALLBACK_LIMIT: usize = 20;

/// `vault_detect_and_match` URL 匹配命中时的上限（follow-up #8）。
///
/// 同域可能挂很多 cipher（如多个测试账号），仍限制数量避免列表过长。
pub const VAULT_DETECT_MATCH_LIMIT: usize = 50;

/// 在保存前合并现有 history 并按需追加新条目。
///
/// 行为：
///   - 始终以 `existing.password_history` 为起点（前端 CipherInputDto 不管理 history，
///     避免每次保存都清成 `[]` —— final-review I3）
///   - 当 password 字段变化（且旧值非空）时追加一条 `PasswordHistoryEntry`，
///     `last_used_at` 用 cipher 的 `updated_at`（代表它最后一次作为活动密码的时间）
///     —— follow-up #2
///   - FIFO 截断到 `PASSWORD_HISTORY_MAX` 条（丢最老的）
///
/// 抽出为自由函数便于单元测试（不需要 Tauri `State`）。
fn merge_password_history(
    mut domain: CipherInput,
    existing: &Cipher,
) -> CipherInput {
    // CipherData 当前仅 Login 单变体；保留 irrefutable_let_patterns 以便未来扩展。
    #[allow(irrefutable_let_patterns)]
    let old_password: Option<&str> = match &existing.data {
        CipherData::Login(l) => l.password.as_deref(),
    };
    #[allow(irrefutable_let_patterns)]
    let new_password: Option<&str> = match &domain.data {
        CipherData::Login(l) => l.password.as_deref(),
    };

    // 1. 保留现有 history（不动）
    domain.password_history = existing.password_history.clone();

    // 2. password 变化（且旧值非空）→ 追加
    if let (Some(old_pwd), Some(new_pwd)) = (old_password, new_password) {
        if old_pwd != new_pwd && !old_pwd.is_empty() {
            use octopus_vault::types::PasswordHistoryEntry;
            domain.password_history.push(PasswordHistoryEntry {
                password: old_pwd.to_string(),
                last_used_at: existing.updated_at.clone(),
            });
            // 3. FIFO 截断
            if domain.password_history.len() > PASSWORD_HISTORY_MAX {
                let excess = domain.password_history.len() - PASSWORD_HISTORY_MAX;
                domain.password_history.drain(0..excess);
            }
        }
    }

    domain
}

#[tauri::command]
pub fn vault_update_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
    input: CipherInputDto,
) -> Result<(), String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let domain = dto_to_input(input).map_err(|e| vault_error::serialize(&e))?;

    // MVP：前端 CipherInputDto 不管理 password_history，编辑 cipher 时
    // 直接保留数据库中已有的历史，避免每次保存都把 history 清成 []。
    // （final-review I3）+ password 变化时自动追加条目（follow-up #2）
    let domain =
        match octopus_vault::storage::load_cipher(id, &key).map_err(vault_error::to_tauri_error)? {
            Some(existing) => merge_password_history(domain, &existing),
            None => domain,
        };

    octopus_vault::storage::save_cipher(id, &domain, &key).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_delete_cipher(
    _state: State<'_, SharedVaultSession>,
    id: i64,
    permanent: bool,
) -> Result<(), String> {
    // permanent=true 不需要 user_vault_key（只是删行）
    if permanent {
        octopus_vault::storage::permanent_delete(id).map_err(vault_error::to_tauri_error)
    } else {
        octopus_vault::storage::soft_delete(id).map_err(vault_error::to_tauri_error)
    }
}

#[tauri::command]
pub fn vault_restore_cipher(_state: State<'_, SharedVaultSession>, id: i64) -> Result<(), String> {
    octopus_vault::storage::restore(id).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_generate(cfg: GeneratorConfig) -> Result<String, String> {
    octopus_vault::generator::generate(&cfg).map_err(vault_error::to_tauri_error)
}

#[tauri::command]
pub fn vault_generate_totp(
    state: State<'_, SharedVaultSession>,
    cipher_id: i64,
) -> Result<TotpResultDto, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id)))?;
    // CipherData 当前仅 Login 单变体；预留 unreachable arm 以便未来扩展。
    #[allow(unreachable_patterns)]
    let login = match cipher.data {
        CipherData::Login(l) => l,
        _ => return Err(vault_error::serialize(&VaultError::InvalidInput("非 Login 类型".into()))),
    };
    let totp_secret = login
        .totp
        .ok_or_else(|| vault_error::serialize(&VaultError::InvalidInput("无 TOTP secret".into())))?;
    let gen = octopus_vault::totp::TotpGenerator::from_base32(&totp_secret)
        .map_err(vault_error::to_tauri_error)?;
    Ok(TotpResultDto {
        code: gen.current().map_err(vault_error::to_tauri_error)?,
        seconds_remaining: gen.seconds_remaining(),
    })
}

#[tauri::command]
pub fn vault_health_report(state: State<'_, SharedVaultSession>) -> Result<HealthReport, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let ciphers =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    Ok(octopus_vault::health::generate_report(&ciphers))
}

#[tauri::command]
pub fn vault_import_bitwarden(
    state: State<'_, SharedVaultSession>,
    json: String,
) -> Result<ImportReport, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    // 内部 anyhow 错误可能含 JSON parse 详情 / SQL 片段——统一映射到 ImportFailed
    // 的稳定 message，不透传内部细节。
    octopus_vault::importer::import_bitwarden_json(&json, &key)
        .map_err(|_| vault_error::serialize(&VaultError::ImportFailed(String::new())))
}

#[tauri::command]
pub fn vault_export(state: State<'_, SharedVaultSession>) -> Result<String, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let ciphers =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    octopus_vault::importer::export_vault_json(&ciphers).map_err(vault_error::to_tauri_error)
}

// === Auto-Type 命令（Task 19） ===

#[derive(serde::Serialize)]
pub struct AutoTypeResultDto {
    pub filled: bool,
    pub message: String,
    pub fallback_to_clipboard: bool,
}

/// 触发 Auto-Type 完整流程：取 cipher → 提取 username/password → 模拟键盘。
///
/// 失败时降级到 concealed 剪贴板（30s 自动清空）。`ClipboardHandle.suppress_next()`
/// 必须在 `copy_concealed` 之前调用——后者直接走 NSPasteboard，绕过 ClipboardHandle::write_text
/// 自动 suppress，不手动抑制会导致自身 clipboard_history watcher 把密码写进 FTS 库。
#[tauri::command]
pub fn vault_autotype(
    state: State<'_, SharedVaultSession>,
    clipboard: State<'_, Arc<ClipboardHandle>>,
    cipher_id: i64,
) -> Result<AutoTypeResultDto, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;

    // 1. 取 cipher
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id)))?;

    // 2. reprompt 确认（如有）—— 由前端在调本命令前弹密码框；本命令不直接处理

    // 3. 提取 username / password
    // CipherData 当前仅 Login 单变体；预留 unreachable arm 以便未来扩展 SecureNote/Card/Identity。
    #[allow(unreachable_patterns)]
    let (username, password) = match &cipher.data {
        CipherData::Login(l) => (
            l.username.clone().unwrap_or_default(),
            l.password.clone().unwrap_or_default(),
        ),
        _ => {
            return Err(vault_error::serialize(&VaultError::InvalidInput(
                "非 Login 类型".into(),
            )))
        }
    };

    // 4. Auto-Type
    match autotype::autotype_login(&username, &password, false) {
        Ok(()) => Ok(AutoTypeResultDto {
            filled: true,
            message: "已填充".into(),
            fallback_to_clipboard: false,
        }),
        Err(_) => {
            // fallback：复制密码到剪贴板（必须先 suppress_next 防 watcher 入库）
            // 失败信息走 VaultError::AutoTypeFailed 的稳定 message，不透传内部细节。
            clipboard.suppress_next();
            let _ = autotype::copy_concealed(&password);
            Ok(AutoTypeResultDto {
                filled: false,
                message: VaultError::AutoTypeFailed.user_message().to_string(),
                fallback_to_clipboard: true,
            })
        }
    }
}

/// 检测当前浏览器 URL + 返回匹配 cipher 列表。
/// URL 检测失败时返回最近使用的若干 cipher（follow-up #8：take 20），
/// URL 匹配命中时也限制数量（take 50，避免大域共享导致列表过长）。
#[tauri::command]
pub fn vault_detect_and_match(
    state: State<'_, SharedVaultSession>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;

    let url_str = autotype::current_browser_url()
        .map_err(vault_error::to_tauri_error)?
        .unwrap_or_default();
    if url_str.is_empty() {
        // URL 检测失败 → 返回 last-N-used（按 updated_at DESC）让用户手动选
        // （follow-up #8：限制为 20 条，避免大 vault 全量返回 500+ 条的噪音/延迟）
        return octopus_vault::storage::list_ciphers(&key)
            .map_err(vault_error::to_tauri_error)
            .map(|cs| {
                let mut filtered: Vec<Cipher> = cs
                    .into_iter()
                    .filter(|c| c.deleted_at.is_none())
                    .collect();
                // updated_at DESC：最近用过的（vault_autotype 每次访问会 bump）排在前面
                filtered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                filtered
                    .into_iter()
                    .take(VAULT_DETECT_FALLBACK_LIMIT)
                    .map(cipher_to_dto)
                    .collect()
            });
    }

    let url = url::Url::parse(&url_str).map_err(|e| vault_error::to_tauri_error(anyhow::anyhow!(e)))?;
    let ciphers =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;

    // 默认等价域名（MVP）
    let equivalent = octopus_vault::matcher::psl::default_equivalent_domains();

    let matched = octopus_vault::matcher::find_matching_ciphers(&url, &ciphers, &equivalent);
    // follow-up #8：URL 匹配也限制数量（同域可能挂很多 cipher）
    Ok(matched
        .into_iter()
        .take(VAULT_DETECT_MATCH_LIMIT)
        .cloned()
        .map(cipher_to_dto)
        .collect())
}

/// 复制指定 cipher 的密码到 concealed 剪贴板。
///
/// `suppress_next()` 必须在 `copy_concealed` 之前调用——`copy_concealed` 直接写 NSPasteboard，
/// 绕过 `ClipboardHandle::write_text` 的自动 suppress，不手动抑制会让自身 clipboard_history
/// watcher 把密码写入 FTS 索引库。
#[tauri::command]
pub fn vault_copy_password(
    state: State<'_, SharedVaultSession>,
    clipboard: State<'_, Arc<ClipboardHandle>>,
    cipher_id: i64,
) -> Result<(), String> {
    let key = require_user_vault_key(&state).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id)))?;

    // CipherData 当前仅 Login 单变体；保留 unreachable arm 以便未来扩展。
    #[allow(irrefutable_let_patterns)]
    if let CipherData::Login(l) = cipher.data {
        if let Some(pwd) = l.password {
            clipboard.suppress_next(); // BEFORE copy_concealed
            autotype::copy_concealed(&pwd).map_err(vault_error::to_tauri_error)?;
            return Ok(());
        }
    }
    Err(vault_error::serialize(&VaultError::InvalidInput(
        "无密码".into(),
    )))
}

// === 全局热键注册（Task 19） ===

/// 注册 vault Auto-Type 全局热键（默认 CmdOrCtrl+Shift+L）。
///
/// 触发时新建/聚焦 `vault_picker_window`：窗口 mount 后 useEffect 调
/// `vault_detect_and_match` 取匹配 cipher，用户选择后调 `vault_autotype` /
/// `vault_copy_password`。窗口已存在时 show + set_focus + emit
/// `vault://picker-refresh`（前端监听后重新拉取，保证每次按热键都拿到最新数据）。
///
/// 注：原实现只 emit `vault://autotype-triggered` 而前端无监听，导致热键「死键」。
/// （follow-up #4 修复）
pub fn register_vault_autotype_shortcut(
    app: &AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("解析热键 '{}' 失败: {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                log::info!("vault autotype 触发");
                // toggle 语义：已存在 → show + set_focus + 通知前端刷新；
                // 不存在 → 新建（前端 mount 后自动调 vault_detect_and_match）。
                use tauri::Manager;
                if let Some(win) = app_handle.get_webview_window("vault_picker_window") {
                    let _ = win.show();
                    let _ = win.set_focus();
                    let _ = app_handle.emit("vault://picker-refresh", ());
                } else {
                    let _ = tauri::WebviewWindowBuilder::new(
                        &app_handle,
                        "vault_picker_window",
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .title("Vault Auto-Type")
                    .inner_size(400.0, 360.0)
                    .resizable(false)
                    .decorations(false)
                    .always_on_top(true)
                    .build();
                }
            }
        })
        .map_err(|e| format!("注册热键 '{}' 失败: {}", shortcut_str, e))?;
    Ok(())
}

/// 注册 vault 密码生成器浮窗全局热键（默认 CmdOrCtrl+Shift+G）。
/// 触发时新建 webview window "password_generator_window"。
pub fn register_vault_generator_shortcut(
    app: &AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("解析热键 '{}' 失败: {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                log::info!("vault generator 触发");
                // 已存在 → show + set_focus；不存在 → 新建。toggle 语义避免重复 build 报错。
                use tauri::Manager;
                if let Some(win) = app_handle.get_webview_window("password_generator_window") {
                    let _ = win.show();
                    let _ = win.set_focus();
                } else {
                    let _ = tauri::WebviewWindowBuilder::new(
                        &app_handle,
                        "password_generator_window",
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .title("密码生成器")
                    .inner_size(480.0, 360.0)
                    .resizable(false)
                    .build();
                }
            }
        })
        .map_err(|e| format!("注册热键 '{}' 失败: {}", shortcut_str, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_vault::types::{
        CipherData, CipherType, Field, LoginData, LoginUri, MatchType, PasswordHistoryEntry,
        RepromptType,
    };

    /// 构造一份字段齐全的 Cipher（Login 类型 + favorite + notes + fields）。
    fn sample_cipher() -> Cipher {
        Cipher {
            id: 42,
            folder_id: Some(7),
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
            deleted_at: None,
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-02T00:00:00".into(),
        }
    }

    /// 构造一份字段齐全的 CipherInputDto（前端 → 后端输入）。
    fn sample_input_dto() -> CipherInputDto {
        CipherInputDto {
            folder_id: Some(3),
            favorite: false,
            name: "New Entry".into(),
            notes: None,
            login: Some(LoginDataDto {
                uris: vec![LoginUriDto {
                    uri: "https://test.com".into(),
                    match_type: Some(0),
                }],
                username: Some("u".into()),
                password: Some("p".into()),
                totp: None,
            }),
            fields: vec![FieldDto {
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

        assert_eq!(dto.id, 42);
        assert_eq!(dto.folder_id, Some(7));
        assert!(dto.favorite);
        assert_eq!(dto.atype, 1, "Login 应映射为 atype=1");
        assert_eq!(dto.name, "Example Login");
        assert_eq!(dto.notes.as_deref(), Some("some notes"));
        assert_eq!(dto.reprompt, 1, "RepromptType::Password 应映射为 1");
        assert_eq!(dto.deleted_at, None);
        assert_eq!(dto.created_at, "2026-01-01T00:00:00");
        assert_eq!(dto.updated_at, "2026-01-02T00:00:00");

        // login sub-fields
        let login = dto.login.expect("login should be Some for Login cipher");
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri, "https://example.com");
        assert_eq!(login.uris[0].match_type, Some(0), "MatchType::Domain 应映射为 0");
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

        assert_eq!(input.folder_id, Some(3));
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
        let exp_folder = dto_in.folder_id;
        let exp_favorite = dto_in.favorite;
        let exp_name = dto_in.name.clone();
        let exp_notes = dto_in.notes.clone();
        let exp_reprompt = dto_in.reprompt.unwrap_or(0);
        let exp_username = dto_in.login.as_ref().and_then(|l| l.username.clone());
        let exp_fields_len = dto_in.fields.len();
        let input = dto_to_input(dto_in).expect("convert");

        // 构造一个完整 Cipher（补 id/时间戳/password_history——dto_to_input 不产出这些）
        let cipher = Cipher {
            id: 99,
            folder_id: input.folder_id,
            favorite: input.favorite,
            atype: input.atype,
            name: input.name.clone(),
            notes: input.notes.clone(),
            data: input.data.clone(),
            fields: input.fields.clone(),
            password_history: vec![],
            reprompt: input.reprompt,
            deleted_at: None,
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

        let keys = octopus_vault::unlock::setup_vault("test-master-pw").expect("setup_vault");
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
        let id = octopus_vault::storage::create_cipher(&login_input("site", "A"), &key)
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
        let id = octopus_vault::storage::create_cipher(&login_input("site", "p0"), &key)
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

        let keys = octopus_vault::unlock::setup_vault("test-master-pw").expect("setup_vault");
        let _ = octopus_vault::keychain::delete_machine_key();
        (Arc::new(keys.user_vault_key), Arc::new(keys.app_key))
    }

    /// 构造一个 app_key 已注入的 SharedVaultSession。
    fn session_with_app_key(app_key: Arc<DerivedKey>) -> SharedVaultSession {
        use parking_lot::RwLock;
        let mut s = crate::vault_state::VaultSession::default();
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
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, is_local)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    "llm",
                    "test_provider",
                    "test_category",
                    model_name,
                    "https://test-source.example.com",
                    secret_key,
                    0, // is_local=0 → 云端模型
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
        let decrypted = super::require_app_key_from_session(&session).ok();
        assert!(decrypted.is_some(), "app_key 应可取");

        // 直接调 read_model_secret_key（ chokepoint 入口）
        let result = crate::vault_secret_access::read_model_secret_key(&model_name, &session)
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
        let result = crate::vault_secret_access::read_model_secret_key(&model_name, &session)
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
                "INSERT INTO models (domain, provider, category, model_name, source, secret_key, is_local)
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
        let result = crate::vault_secret_access::read_model_secret_key(model_name, &session)
            .expect("read should succeed");
        assert_eq!(result, manifest);
    }

    // === Follow-up #8: detect_and_match fallback 限制 ===

    /// 常量应为 spec 规定的值（20 / 50）。
    #[test]
    fn test_detect_match_limits_are_spec_values() {
        assert_eq!(VAULT_DETECT_FALLBACK_LIMIT, 20);
        assert_eq!(VAULT_DETECT_MATCH_LIMIT, 50);
    }
}
