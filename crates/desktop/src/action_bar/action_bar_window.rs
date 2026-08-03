//! AI 命令面板迷你浮窗——选中文本后热键触发，鼠标上方弹出。
//! 透明无边框 always_on_top，单例 show/hide toggle。

use tauri::{AppHandle, Emitter, Manager};

use crate::ui::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "action_bar_window";

/// 创建窗口（应用启动时调用，visible=false）。
pub fn create_action_bar_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    // P2-2 修复：原 `let _ = ...build()` 静默吞错——此后所有 show/hide 调用
    // get_webview_window 返回 None 静默 no-op，用户按热键无反应且无任何提示，
    // 极难定位。记 error 让启动日志至少能看出建窗失败。
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "action-bar.html",
        title: "",
        inner_size: (480.0, 76.0), // 宽 480（大气 + 不遮挡），初始高度由 resize effect 调整
        visible: false,
        resizable: false,
        position: None,
        focused: None,
        accept_first_mouse: None,
    })
    .map_err(|e| log::error!("[action-bar] 窗口创建失败: {e}"));
}

/// 在指定坐标显示浮窗（鼠标上方）。emit 事件让前端刷新 context。
pub fn show_action_bar_window(app: &AppHandle, x: f64, y: f64) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));

        #[cfg(target_os = "macos")]
        {
            // action bar 不隐藏终端（hide_regular=false）：终端本来可见就保持可见，
            // 本来不可见就保持不可见——action bar 不该有副作用改变其他窗口可见性。
            crate::platform::activation::before_floating_window_show(app, false);
            // 轻量激活（activate_self_no_raise）：只 NSApplication::activate()，不调
            // activateWithOptions(ActivateAllWindows)——后者会 unhide+raise 所有窗口，
            // 把用户已隐藏/最小化的终端/编辑器强行抬到前台，盖住源 app（浏览器等）。
            // 轻量版激活 app（makeKeyAndOrderFront 有效）但不 unhide 其他窗口。
            crate::platform::activation::activate_self_no_raise();
        }

        let _ = win.show();

        #[cfg(target_os = "macos")]
        {
            // 强制拿 key window：app active 且终端（Regular main window）可见时，
            // Tauri 的 set_focus（内部 orderFront + makeKey）不够强制——AppKit 倾向
            // 保持 Regular main window 为 key，导致 action bar 拿不到焦点、终端闪一下。
            // 直接用 NSWindow makeKeyAndOrderFront: 更强制（让 floating panel 立即成 key）。
            use objc2_app_kit::NSWindow;
            if let Ok(ns_ptr) = win.ns_window() {
                if !ns_ptr.is_null() {
                    unsafe {
                        let ns_win = &*(ns_ptr as *const NSWindow);
                        ns_win.makeKeyAndOrderFront(None);
                    }
                }
            } else {
                let _ = win.set_focus(); // fallback：拿不到 ns_window 时退回 Tauri set_focus
            }
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = win.set_focus(); }

        #[cfg(target_os = "macos")]
        {
            // 诊断：makeKeyAndOrderFront 后窗口是否真的成为 key/main window。
            use objc2::msg_send;
            use objc2::runtime::AnyObject;
            if let Ok(ns_ptr) = win.ns_window() {
                let ns_win: *mut AnyObject = ns_ptr as *mut AnyObject;
                unsafe {
                    let is_key: bool = msg_send![ns_win, isKeyWindow];
                    let is_main: bool = msg_send![ns_win, isMainWindow];
                    log::info!("[action-bar][show] after makeKeyAndOrderFront: isKeyWindow={} isMainWindow={}", is_key, is_main);
                }
            }
        }

        // emit 携带 context payload——前端 refresh 直接用事件里的 context，
        // 不再依赖异步 invoke(get_context)（消除首屏竞态：窗口已 show 但 ctx Promise
        // 还在 pending 时用了陈旧 context state，导致"有选中却只显示输入框"）。
        let ctx = crate::action_bar::action_bar_commands::snapshot_pending_context();
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
    use objc2_app_kit::NSWindow;
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        // 已 dismiss 的窗口跳过——consolidate=true 分支会夺焦，对隐藏窗口夺焦是 bug
        if !win.is_visible().unwrap_or(false) {
            return;
        }
        if let Ok(ns_ptr) = win.ns_window() {
            if !ns_ptr.is_null() {
                unsafe {
                    let ns_win = &*(ns_ptr as *const NSWindow);
                    let before: bool = ns_win.isKeyWindow();
                    if consolidate && !before {
                        // 用 makeKeyAndOrderFront 巩固（与 show 时一致，比 set_focus 强制）
                        ns_win.makeKeyAndOrderFront(None);
                        let after: bool = ns_win.isKeyWindow();
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
}

/// 隐藏浮窗。
pub fn hide_action_bar_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }

    #[cfg(target_os = "macos")]
    { crate::platform::activation::after_floating_window_hide(app); }
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
                    crate::action_bar::action_bar_commands::reset_trigger_guard();
                    return;
                }
                // guard 超时保护——如果上次触发超过 10s 仍未 finalize，强制重置
                crate::action_bar::action_bar_commands::reset_trigger_guard_if_stale(10);
                crate::action_bar::action_bar_commands::trigger_action_bar(app_handle.clone());
            }
        })
        .map_err(|e| format!("Failed to register action bar shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}
