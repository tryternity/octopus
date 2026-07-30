//! vault 自动填写：autotype / 搜索 / URL 检测匹配 / 缓存 URL / 复制密码 / 复制用户名。

use std::sync::Arc;

use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use octopus_clipboard::ClipboardHandle;
use octopus_vault::types::{Cipher, CipherData, RepromptType};

use crate::core::runtime_config::SharedRuntimeConfig;
use crate::vault::vault_error::{self, VaultError};
use crate::vault::vault_state::SharedVaultSession;

use super::{cipher_to_dto, require_user_vault_key, AutoTypeMode, CipherDto};

/// `vault_detect_and_match` URL 匹配命中时的上限（follow-up #8）。
///
/// 同域可能挂很多 cipher（如多个测试账号），仍限制数量避免列表过长。
pub const VAULT_DETECT_MATCH_LIMIT: usize = 50;

// === Auto-Type 命令（Task 19） ===

/// `vault_autotype` 命令返回值（前端调用方唯一消费点，就近定义）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTypeResult {
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
    app: AppHandle,
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    clipboard: State<'_, Arc<ClipboardHandle>>,
    cipher_id: String,
    master_password: Option<String>,
    mode: Option<AutoTypeMode>,
) -> Result<AutoTypeResult, String> {
    let mode = mode.unwrap_or_default();
    log::info!(
        "[vault-autotype] invoke 进入：cipher_id={}，reprompt_required={}，mode={:?}",
        cipher_id,
        master_password.is_some(),
        mode
    );

    // **2026-07-20 e2e 修复**：hide VaultPicker 必须在后端做，不能由前端 await。
    //
    // 原前端流程 `await getCurrentWindow().hide(); await invoke("vault_autotype")`
    // 有竞态：hide() 让 webview 进入 terminated 状态，紧接着的 invoke 永远到不了
    // 后端（日志看到 web content process terminated 但没有 [vault-autotype] invoke）。
    // 偶尔能 work 是因为 webview 还没完全 terminate 时 invoke 跑完了。
    //
    // 修复：vault_autotype 命令自己拿 AppHandle 隐藏 VaultPicker，确保 hide 之后
    // 还有完整的 Rust 代码路径执行注入（不依赖 webview）。
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("vault_picker_window") {
        let _ = win.hide();
        log::debug!("[vault-autotype] VaultPicker 已 hide");
    }

    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;

    // 1. 取 cipher
    let cipher = octopus_vault::storage::load_cipher(&cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id.clone())))?;

    // 2. reprompt 强制校验（后端，不可绕过）—— cipher.reprompt == Password 时
    //    必须传 master_password 且密码正确；否则拒绝（防 DevTools / 篡改前端绕过）。
    //    不像首发版那样把 reprompt 委托给前端——前端校验是不可信的。
    if cipher.reprompt == RepromptType::Password {
        match master_password {
            Some(pwd) => {
                // 密码错或 vault 异常 → InvalidMasterPassword（user-safe 消息，不透传内部细节）
                octopus_vault::unlock::verify_master_password(Zeroizing::new(pwd)).map_err(|_| {
                    vault_error::serialize(&VaultError::InvalidMasterPassword)
                })?;
            }
            None => {
                return Err(vault_error::serialize(&VaultError::RepromptRequired));
            }
        }
    }

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
    // expected_bundle_id=None：最小防御，只校验前台不是 octopus 自身（防 VaultPicker
    // 未 hide 时密码打到 octopus 自己窗口的泄露）。完整白名单需前端在 hide 前调
    // url_detect 拿到浏览器 bundle_id 并传入，未来增强。
    //
    // mode（2026-07-20 三模式）：webmail SPA 的 Tab 切焦点不可靠，让用户据当前
    // 光标位置选合适模式。默认 PasswordOnly——最稳健。
    log::info!(
        "[vault-autotype] 调 autotype_login：mode={:?} username_len={} password_len={}",
        mode,
        username.len(),
        password.len()
    );
    match crate::vault::autotype::autotype_login_with_mode(&username, &password, mode, false, None) {
        Ok(()) => {
            log::info!("[vault-autotype] autotype_login Ok（已填充，mode={:?}）", mode);
            Ok(AutoTypeResult {
                filled: true,
                message: "已填充".into(),
                fallback_to_clipboard: false,
            })
        }
        Err(e) => {
            log::warn!("[vault-autotype] autotype_login 失败 → fallback 剪贴板：{}", e);
            // fallback：复制密码到剪贴板（必须先 suppress_next 防 watcher 入库）
            // 失败信息走 VaultError::AutoTypeFailed 的稳定 message，不透传内部细节。
            clipboard.suppress_next();
            let _ = crate::vault::autotype::copy_concealed(&password);
            Ok(AutoTypeResult {
                filled: false,
                message: VaultError::AutoTypeFailed.user_message().to_string(),
                fallback_to_clipboard: true,
            })
        }
    }
}

/// 模糊搜索 cipher（URL 检测失败时用户手动搜索用，2026-07-21 安全加固新增）。
///
/// 匹配 name / username / URIs，大小写不敏感，子串包含即命中。
/// 按 `updated_at DESC` 排序（最近用的排前面），限制 20 条避免大 vault 全量返回。
///
/// 安全语义：vault_detect_and_match URL 检测失败时返回空列表，用户必须在此
/// 主动输入搜索词——是有意识的选择，避免钓鱼场景下"顺手"误选密码。
#[tauri::command]
pub fn vault_search_ciphers(
    query: String,
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Ok(Vec::new());
    }
    let (ciphers, failures) = octopus_vault::storage::list_ciphers(&key)
        .map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!(
            "vault_search_ciphers: {} 条记录解密失败已跳过",
            failures.len()
        );
    }
    let mut filtered: Vec<Cipher> = ciphers
        .into_iter()
        .filter(|c| {
            if c.is_deleted {
                return false;
            }
            // name 匹配
            if c.name.to_lowercase().contains(&query_lower) {
                return true;
            }
            // username / URIs 匹配（从 LoginData 提取）
            #[allow(unreachable_patterns)]
            match &c.data {
                octopus_vault::types::CipherData::Login(l) => {
                    if let Some(u) = &l.username {
                        if u.to_lowercase().contains(&query_lower) {
                            return true;
                        }
                    }
                    l.uris.iter().any(|lu| {
                        lu.uri.to_lowercase().contains(&query_lower)
                    })
                }
                _ => false,
            }
        })
        .collect();
    // updated_at DESC（最近用的排前面）
    filtered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(filtered.into_iter().take(20).map(cipher_to_dto).collect())
}

/// 检测当前浏览器 URL + 返回匹配 cipher 列表。
/// URL 检测失败时返回空列表（2026-07-21 安全加固，原返回最近 20 条有钓鱼风险）。
/// URL 匹配命中时也限制数量（take 50，避免大域共享导致列表过长）。
///
/// **URL 来源**（2026-07-19 e2e 修复）：优先读 `picker_url_cache`——热键 callback 在
/// show VaultPicker **之前**抓的 URL（此时浏览器还前台）。缓存空（用户手动刷新 / 热键
/// callback 抓失败）才走 `current_browser_url()` 现抓——此时若 VaultPicker 在前台，
/// 会取到 octopus-desktop 自身，URL 检测失败走 fallback，符合"手动刷新无前台浏览器"语义。
#[tauri::command]
pub fn vault_detect_and_match(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    url_cache: State<'_, crate::vault::vault_state::SharedPickerUrlCache>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;

    // 优先读热键 callback 预抓的 URL（修 e2e 时序 bug）
    let cached_url: Option<String> = url_cache
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|s| !s.is_empty());
    // **2026-07-20 修正**：不在 detect_and_match 里清空 cache——
    // 因为 CreateCipherView（新建场景）也要读这个 cache 预填 URL，detect 提前清掉
    // 会让新建表单 URL 空。cache 在热键 callback 每次覆盖（新热键 → 新 URL），
    // 用户手动刷新（无新热键）会一直用旧 cache——可接受，因为浮窗显示期间用户
    // 几乎不会切浏览器 tab。

    let url_str = match cached_url {
        Some(u) => {
            log::debug!("vault_detect_and_match: 用热键预抓 URL");
            u
        }
        None => {
            log::debug!("vault_detect_and_match: 缓存空，现抓 URL");
            crate::vault::autotype::current_browser_url()
                .map_err(vault_error::to_tauri_error)?
                .unwrap_or_default()
        }
    };
    if url_str.is_empty() {
        // 2026-07-21 安全加固：URL 检测失败时不再返回 fallback 列表（防钓鱼）。
        // 原行为返回 last-20-used 让用户手动选——但钓鱼场景下用户可能误选密码
        // 注入到钓鱼站。现改为返回空列表，用户在 VaultPicker 输入搜索词后由
        // vault_search_ciphers 模糊匹配（用户主动搜索 = 有意识的选择，非"顺手"误选）。
        // 合法场景（桌面应用/不支持浏览器）仍可通过搜索找到密码。
        log::debug!("vault_detect_and_match: URL 检测失败，返回空列表（用户可搜索）");
        return Ok(Vec::new());
    }

    let url = url::Url::parse(&url_str).map_err(|e| vault_error::to_tauri_error(anyhow::anyhow!(e)))?;
    let (ciphers, failures) =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!(
            "vault_detect_and_match (url-match): {} 条记录解密失败已跳过",
            failures.len()
        );
    }

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

/// 取当前缓存的浏览器 URL（热键 callback 预抓的），用于「为当前站点新建 cipher」场景。
///
/// 2026-07-20 新增：VaultPicker 浮窗里点「为当前站点新建」时，前端需要拿到当前 URL
/// 预填到表单。复用 picker_url_cache（不重新抓——hide 浮窗后浏览器已非前台）。
///
/// 返回 `Option<String>`：Some(url) 有缓存，None 缓存空（用户可手动输 URL）。
/// **不清空缓存**——紧接着的 vault_detect_and_match 可能还要用（虽然新建场景通常
/// 已经过了 detect 阶段）。读 + clone 廉价。
#[tauri::command]
pub fn vault_get_cached_url(
    url_cache: State<'_, crate::vault::vault_state::SharedPickerUrlCache>,
) -> Result<Option<String>, String> {
    Ok(url_cache
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|s| !s.is_empty()))
}

/// 复制指定 cipher 的密码到 concealed 剪贴板。
///
/// `suppress_next()` 必须在 `copy_concealed` 之前调用——`copy_concealed` 直接写 NSPasteboard，
/// 绕过 `ClipboardHandle::write_text` 的自动 suppress，不手动抑制会让自身 clipboard_history
/// watcher 把密码写入 FTS 索引库。
#[tauri::command]
pub fn vault_copy_password(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    clipboard: State<'_, Arc<ClipboardHandle>>,
    cipher_id: String,
    master_password: Option<String>,
) -> Result<(), String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(&cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id.clone())))?;

    // reprompt 强制校验（修复 A：复制路径同样返回明文密码，必须与 vault_autotype 对称）。
    // DevTools 可直接 invoke('vault_copy_password', {cipherId: X}) 拿到明文，若不校验
    // 则攻击面从 autotype 平移到 copy。
    if cipher.reprompt == RepromptType::Password {
        match master_password {
            Some(pwd) => {
                octopus_vault::unlock::verify_master_password(Zeroizing::new(pwd)).map_err(|_| {
                    vault_error::serialize(&VaultError::InvalidMasterPassword)
                })?;
            }
            None => {
                return Err(vault_error::serialize(&VaultError::RepromptRequired));
            }
        }
    }

    // CipherData 当前仅 Login 单变体；保留 unreachable arm 以便未来扩展。
    #[allow(irrefutable_let_patterns)]
    if let CipherData::Login(l) = cipher.data {
        if let Some(pwd) = l.password {
            clipboard.suppress_next(); // BEFORE copy_concealed
            crate::vault::autotype::copy_concealed(&pwd).map_err(vault_error::to_tauri_error)?;
            return Ok(());
        }
    }
    Err(vault_error::serialize(&VaultError::InvalidInput(
        "无密码".into(),
    )))
}

/// 复制指定 cipher 的用户名到剪贴板。
///
/// 与 `vault_copy_password` 对称，但用户名通常不敏感——**不强制 reprompt**
/// （reprompt 保护的是密码等高敏感字段，用户名一般可见）。
///
/// 用户场景（2026-07-20 三段式 UI）：cipher 行的"用户名"段右侧 📋 图标。
#[tauri::command]
pub fn vault_copy_username(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    clipboard: State<'_, Arc<ClipboardHandle>>,
    cipher_id: String,
) -> Result<(), String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(&cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id.clone())))?;

    #[allow(irrefutable_let_patterns)]
    if let CipherData::Login(l) = cipher.data {
        if let Some(username) = l.username {
            // 用户名不算高敏感（不在 reprompt 保护范围），但走 concealed 写入避免
            // 进 clipboard_history FTS 索引库（被搜索到也是隐私泄露）。
            clipboard.suppress_next();
            crate::vault::autotype::copy_concealed(&username).map_err(vault_error::to_tauri_error)?;
            return Ok(());
        }
    }
    Err(vault_error::serialize(&VaultError::InvalidInput(
        "无用户名".into(),
    )))
}
