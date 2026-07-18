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
    _state: State<'_, SharedVaultSession>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    octopus_vault::unlock::change_master_password(&old_password, &new_password)
        .map_err(|e| e.to_string())?;
    // 用户刚刚证明了知道旧主密码，不要主动锁会话。
    // user_vault_key / app_key 在改密码流程中不变（INV-7），保持原样即可。
    // （final-review I4）
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
    let mut domain = dto_to_input(input)?;

    // MVP：前端 CipherInputDto 不管理 password_history，编辑 cipher 时
    // 直接保留数据库中已有的历史，避免每次保存都把 history 清成 []。
    // （final-review I3）
    if let Some(existing) = octopus_vault::storage::load_cipher(id, &key).map_err(|e| e.to_string())?
    {
        domain.password_history = existing.password_history;
    }

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
/// 触发时 emit `vault://autotype-triggered` 事件——前端（Task 21）接收后弹 cipher 选择浮窗。
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
                // TODO: Task 21 前端接收事件后弹选择浮窗，调 vault_detect_and_match +
                //       vault_autotype。当前先 emit 一个事件。
                let _ = app_handle.emit("vault://autotype-triggered", ());
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
