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
use octopus_vault::types::{Cipher, CipherData, CipherInput, CipherType, RepromptType};

use crate::autotype;
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
    let keys = octopus_vault::unlock::change_master_password(&old_password, &new_password)
        .map_err(|e| e.to_string())?;
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

/// password_history 上限（避免无界增长）。FIFO 截断：丢最老的。
pub const PASSWORD_HISTORY_MAX: usize = 20;

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
    let key = require_user_vault_key(&state)?;
    let domain = dto_to_input(input)?;

    // MVP：前端 CipherInputDto 不管理 password_history，编辑 cipher 时
    // 直接保留数据库中已有的历史，避免每次保存都把 history 清成 []。
    // （final-review I3）+ password 变化时自动追加条目（follow-up #2）
    let domain = match octopus_vault::storage::load_cipher(id, &key).map_err(|e| e.to_string())? {
        Some(existing) => merge_password_history(domain, &existing),
        None => domain,
    };

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
    let key = require_user_vault_key(&state)?;

    // 1. 取 cipher
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;

    // 2. reprompt 确认（如有）—— 由前端在调本命令前弹密码框；本命令不直接处理

    // 3. 提取 username / password
    // CipherData 当前仅 Login 单变体；预留 unreachable arm 以便未来扩展 SecureNote/Card/Identity。
    #[allow(unreachable_patterns)]
    let (username, password) = match &cipher.data {
        CipherData::Login(l) => (
            l.username.clone().unwrap_or_default(),
            l.password.clone().unwrap_or_default(),
        ),
        _ => return Err("非 Login 类型".into()),
    };

    // 4. Auto-Type
    match autotype::autotype_login(&username, &password, false) {
        Ok(()) => Ok(AutoTypeResultDto {
            filled: true,
            message: "已填充".into(),
            fallback_to_clipboard: false,
        }),
        Err(e) => {
            // fallback：复制密码到剪贴板（必须先 suppress_next 防 watcher 入库）
            clipboard.suppress_next();
            let _ = autotype::copy_concealed(&password);
            Ok(AutoTypeResultDto {
                filled: false,
                message: format!("Auto-Type 失败，已复制密码到剪贴板（30s 清空）: {}", e),
                fallback_to_clipboard: true,
            })
        }
    }
}

/// 检测当前浏览器 URL + 返回匹配 cipher 列表。
/// URL 检测失败时返回全部 cipher（让用户手动选）。
#[tauri::command]
pub fn vault_detect_and_match(
    state: State<'_, SharedVaultSession>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state)?;

    let url_str = autotype::current_browser_url()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if url_str.is_empty() {
        // URL 检测失败 → 返回全部 cipher 让用户手动选
        return octopus_vault::storage::list_ciphers(&key)
            .map_err(|e| e.to_string())
            .map(|cs| cs.into_iter().map(cipher_to_dto).collect());
    }

    let url = url::Url::parse(&url_str).map_err(|e| format!("URL 解析失败: {}", e))?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;

    // 默认等价域名（MVP）
    let equivalent = octopus_vault::matcher::psl::default_equivalent_domains();

    let matched = octopus_vault::matcher::find_matching_ciphers(&url, &ciphers, &equivalent);
    Ok(matched.into_iter().cloned().map(cipher_to_dto).collect())
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
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;

    // CipherData 当前仅 Login 单变体；保留 unreachable arm 以便未来扩展。
    #[allow(irrefutable_let_patterns)]
    if let CipherData::Login(l) = cipher.data {
        if let Some(pwd) = l.password {
            clipboard.suppress_next(); // BEFORE copy_concealed
            autotype::copy_concealed(&pwd).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("无密码".into())
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
}
