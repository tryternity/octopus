//! AI 命令面板迷你浮窗——选中文本后热键触发，鼠标上方弹出。
//! 透明无边框 always_on_top，单例 show/hide toggle。

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "action_bar_window";

/// 创建窗口（应用启动时调用，visible=false）。
pub fn create_action_bar_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("action-bar.html".into()),
    )
    .title("")
    .inner_size(480.0, 76.0) // 宽 480（大气 + 不遮挡），初始高度由 resize effect 调整
    .decorations(false)
    .always_on_top(true)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build()
    .map_err(|e| {
        // P2-2 修复：原 `let _ = ...build()` 静默吞错——此后所有 show/hide 调用
        // get_webview_window 返回 None 静默 no-op，用户按热键无反应且无任何提示，
        // 极难定位。记 error 让启动日志至少能看出建窗失败。
        log::error!("[action-bar] 窗口创建失败: {e}");
        e
    })
    .ok();
}

/// 在指定坐标显示浮窗（鼠标上方）。emit 事件让前端刷新 context。
pub fn show_action_bar_window(app: &AppHandle, x: f64, y: f64) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));

        #[cfg(target_os = "macos")]
        { crate::activation::before_floating_window_show(app); }

        let _ = win.show();
        let _ = win.set_focus();

        #[cfg(target_os = "macos")]
        {
            // 诊断：set_focus 后窗口是否真的成为 key/main window。
            // 比读 NSApplication::isActive 更准确——isActive 是 app 级，isKeyWindow 是窗口级。
            // 若 isKeyWindow=false，说明 set_focus 没让透明窗口成 key（焦点问题的直接证据）。
            use objc2::msg_send;
            use objc2::runtime::AnyObject;
            if let Ok(ns_ptr) = win.ns_window() {
                let ns_win: *mut AnyObject = ns_ptr as *mut AnyObject;
                unsafe {
                    let is_key: bool = msg_send![ns_win, isKeyWindow];
                    let is_main: bool = msg_send![ns_win, isMainWindow];
                    log::info!("[action-bar][show] after set_focus: isKeyWindow={} isMainWindow={}", is_key, is_main);
                }
            }
        }

        // emit 携带 context payload——前端 refresh 直接用事件里的 context，
        // 不再依赖异步 invoke(get_context)（消除首屏竞态：窗口已 show 但 ctx Promise
        // 还在 pending 时用了陈旧 context state，导致"有选中却只显示输入框"）。
        let ctx = crate::action_bar_commands::snapshot_pending_context();
        let _ = app.emit("action-bar://show", &ctx);

        // 焦点时序诊断 + 巩固：gather_context 内 `subl --command` 激活 Sublime 可能是异步的，
        // 间歇性晚于 set_focus 抢走 key 状态（"偶尔没焦点"）。150/350ms 记录 isKeyWindow 定位
        // 抢焦点时点；150ms 若已失焦则 set_focus 巩固，覆盖 Sublime 延迟激活窗口。
        #[cfg(target_os = "macos")]
        {
            let app_clone = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let app1 = app_clone.clone();
                let _ = app_clone.run_on_main_thread(move || {
                    check_and_consolidate_focus(&app1, 150, true);
                });
                std::thread::sleep(std::time::Duration::from_millis(200));
                let app2 = app_clone.clone();
                let _ = app_clone.run_on_main_thread(move || {
                    check_and_consolidate_focus(&app2, 350, false);
                });
            });
        }
    }
}

/// 焦点诊断 + 巩固（须在主线程调用）：记录 isKeyWindow；若 consolidate 且已失焦则 set_focus 夺回。
///
/// **P1-1 修复（2026-07-17）**：开头加 `is_visible` 守卫——`get_webview_window` 对隐藏窗口
/// 仍返回 Some（窗口对象生命周期 ≠ 可见性），150ms 巩固线程若在用户 dismiss 后触发，
/// 原先会对已隐藏窗口 `set_focus()` 触发 NSWindow ordering / app 激活，把刚回到源 app
/// 的用户重新带到 octopus。加守卫后跳过已 dismiss 的窗口，保留对可见窗口的夺焦逻辑
/// （Sublime `subl --command` 延迟抢焦的核心修复）。
#[cfg(target_os = "macos")]
fn check_and_consolidate_focus(app: &AppHandle, at_ms: u64, consolidate: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        // 已 dismiss 的窗口跳过——consolidate=true 分支会 set_focus，对隐藏窗口夺焦是 bug
        if !win.is_visible().unwrap_or(false) {
            return;
        }
        if let Ok(ns_ptr) = win.ns_window() {
            let ns_win: *mut AnyObject = ns_ptr as *mut AnyObject;
            unsafe {
                let before: bool = msg_send![ns_win, isKeyWindow];
                if consolidate && !before {
                    let _ = win.set_focus();
                    let after: bool = msg_send![ns_win, isKeyWindow];
                    log::info!(
                        "[action-bar][focus@{}ms] lost → consolidate {}→{}",
                        at_ms, before, after
                    );
                } else {
                    log::info!("[action-bar][focus@{}ms] isKeyWindow={}", at_ms, before);
                }
            }
        }
    }
}

/// 隐藏浮窗。
pub fn hide_action_bar_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }

    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide(app); }
}

/// 注册全局热键。与 register_clipboard_shortcut 范式一致。
pub fn register_action_bar_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                // 已可见 → 隐藏（toggle 语义）+ 重置 guard；不可见 → 触发
                if app
                    .get_webview_window(WINDOW_LABEL)
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false)
                {
                    // 走统一收口 hide_action_bar_window（含切回 Accessory + 焦点协调），非裸 win.hide()——
                    // 否则 show 时切的 Regular policy 残留，Dock 图标常驻。
                    hide_action_bar_window(app);
                    // 重置 guard——防 webview 崩溃后 guard 永久卡死
                    crate::action_bar_commands::reset_trigger_guard();
                    return;
                }
                // guard 超时保护——如果上次触发超过 10s 仍未 finalize，强制重置
                crate::action_bar_commands::reset_trigger_guard_if_stale(10);
                crate::action_bar_commands::trigger_action_bar(app_handle.clone());
            }
        })
        .map_err(|e| format!("Failed to register action bar shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}
