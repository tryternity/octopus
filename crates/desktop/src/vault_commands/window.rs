//! vault 热键注册 + 密码生成器独立浮窗（actionbar 触发场景）。

use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::autotype;

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
                // **修 e2e 时序 bug**（2026-07-19）：show VaultPicker **之前**先抓 URL。
                //
                // 原实现 show + set_focus 之后才 emit 让前端 detect URL——此时
                // VaultPicker 已抢前台，frontmost_bundle_id 取到 octopus-desktop 自己，
                // URL 检测必然失败 → 走 fallback 列出最近 20 条 cipher（用户看到全部密码）。
                //
                // 现在先抓 URL（此时浏览器还在前台），存入 picker_url_cache；
                // vault_detect_and_match 优先读缓存。失败也不阻塞——detect 端会 fallback。
                use tauri::Manager;
                let cached_url: Option<String> =
                    match crate::autotype::current_browser_url() {
                        Ok(Some(u)) if !u.is_empty() => Some(u),
                        _ => None,
                    };
                if let Some(cache) =
                    app_handle.try_state::<crate::vault_state::SharedPickerUrlCache>()
                {
                    if let Ok(mut guard) = cache.lock() {
                        *guard = cached_url.clone();
                    }
                }
                log::debug!(
                    "[vault-picker] 热键触发，预抓 URL: {:?}",
                    cached_url
                        .as_deref()
                        .map(|s| s.chars().take(80).collect::<String>())
                );

                // toggle 语义：已存在 → show + set_focus + 通知前端刷新；
                // 不存在 → 新建（前端 mount 后自动调 vault_detect_and_match）。
                if let Some(win) = app_handle.get_webview_window("vault_picker_window") {
                    let _ = win.show();
                    let _ = win.set_focus();
                    let _ = app_handle.emit("vault://picker-refresh", ());
                } else {
                    let _ = tauri::WebviewWindowBuilder::new(
                        &app_handle,
                        "vault_picker_window",
                        tauri::WebviewUrl::App("vault-picker.html".into()),
                    )
                    .title("Vault Auto-Type")
                    // 初始 400×200（locked/uninit 紧凑视图）。list 视图内容多时前端
                    // L1 修复（2026-07-24）：resizable(true) 让前端 setSize 生效
                    // （Tauri 2 resizable(false) 会忽略后续 setSize 调用）。
                    // 当前固定 320×360，但 resizable(true) 保证 setSize 不被吞。
                    // N4 加固（2026-07-24）：min_inner_size 防御——resizable(true)
                    // 理论上允许用户拖拽改尺寸，加下限防止缩到不可用。
                    .inner_size(320.0, 360.0)
                    .min_inner_size(320.0, 360.0)
                    .resizable(true)
                    .decorations(false)
                    .always_on_top(true)
                    .transparent(true)
                    .build();
                }
            }
        })
        .map_err(|e| format!("注册热键 '{}' 失败: {}", shortcut_str, e))?;
    Ok(())
}

// === 密码生成器独立浮窗（外壳 B：Actionbar 触发场景）===
//
// 与 CipherEditor Modal（外壳 A）渲染同一个 <PasswordGenerator> 主体，
// 但本场景生成后直接 Auto-type 到前台浏览器（不经 vault cipher）。
// 详见 spec §5.2「跨场景复用主体 + Modal/独立窗口外壳」。

/// 唤起密码生成器浮窗（Actionbar 内置按钮触发）。
///
/// 浮窗位置：优先跟随前台浏览器 frame（CGWindowList 读窗口），fallback 鼠标 → 屏幕顶部居中。
/// toggle 语义：已存在 → show + 移动到新位置；不存在 → 创建。
#[tauri::command]
pub fn open_password_generator(app: AppHandle) -> Result<(), String> {
    let pos = crate::password_generator_window::compute_window_position(&app);
    log::info!(
        "[password-generator] open: position=({:.0},{:.0}) source={:?}",
        pos.x, pos.y, pos.source
    );
    crate::password_generator_window::show_password_generator_window(&app, pos.x, pos.y);
    Ok(())
}

/// Auto-type 生成的密码到前台 app（password_generator_window 场景）。
///
/// 流程：
/// 1. hide password_generator_window → 浏览器回前台
/// 2. autotype_login("", password, true, None) —— sleep + verify_focused + 注入
///
/// **username 留空**：生成器场景没有 username（与 vault_autotype 不同）。
/// **press_enter=true**：用户主动点 Auto-type 通常需要立即提交表单。
///
/// 安全：verify_focused(None) 走最小防御（前台 ≠ octopus 自身）。若 hide 期间焦点
/// 被抢到第三方 app，密码会打到错误窗口——已知窗口（同 vault_autotype），详见 spec §4.5。
#[tauri::command]
pub fn password_generator_autotype(
    app: AppHandle,
    password: String,
) -> Result<(), String> {
    use tauri::Manager;
    // 1. hide 浮窗让浏览器回前台
    if let Some(win) = app.get_webview_window(crate::password_generator_window::WINDOW_LABEL) {
        let _ = win.hide();
    }
    // 2. sleep + verify_focused + 注入
    autotype::autotype_login("", &password, true, None)
        .map_err(|_| crate::vault_error::serialize(&crate::vault_error::VaultError::AutoTypeFailed))?;
    log::info!("[password-generator] autotype 完成（{} 字符）", password.len());
    Ok(())
}
