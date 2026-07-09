//! macOS 常规窗口激活策略协调。
//!
//! settings / compact_editor 两个常规窗口开窗时把 app 升为 Regular
//!（Dock 显图标），关窗时降回 Accessory（纯托盘）。但关某一个时若其余常规窗口仍开着，
//! 不能直接降级——app 降为 Accessory 会令 macOS 收掉剩余的常规窗口。故关窗后仅当
//! 常规窗口**全无存活**才降级。

use tauri::{ActivationPolicy, Manager};

/// 常规窗口 label：任一存活 → app 须保持 Regular。
const REGULAR_WINDOWS: &[&str] = &[
    "settings_window",
    "compact_editor_window",
];

/// 某常规窗口关闭后调用：仅当无其他常规窗口存活时才切回 Accessory。
///
/// 必须在 `WindowEvent::Destroyed`（窗口已从 app 移除）里调用——此时被关窗口的
/// `get_webview_window` 已返回 None，故 `REGULAR_WINDOWS` 检查自然只看其余窗口。
pub fn restore_accessory_if_no_regular_window(app_handle: &tauri::AppHandle) {
    let any_alive = REGULAR_WINDOWS
        .iter()
        .any(|label| app_handle.get_webview_window(label).is_some());
    if !any_alive {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
    }
}

/// 全局热键触发浮窗（clipboard/result/action_bar）前调用：
/// 临时隐藏常规窗口（settings/compact_editor），避免 app 被激活时
/// 把这些窗口带到前台抢焦点。用户手动点 Dock 图标或托盘仍可恢复。
#[allow(dead_code)]
pub fn hide_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            if win.is_visible().unwrap_or(false) {
                let _ = win.hide();
            }
        }
    }
}

/// 浮窗操作完成后调用：恢复之前被隐藏的常规窗口。（保留供未来使用）
#[allow(dead_code)]
pub fn show_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            // 只 show 不 focus——不抢焦点
            let _ = win.show();
        }
    }
}

// ── 浮窗焦点协调（show 前隐藏 Regular，hide 后恢复 + 交还前台焦点）──

use parking_lot::Mutex;
use once_cell::sync::Lazy;

static TEMP_HIDDEN: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(Vec::new()));
static WAS_INACTIVE: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

#[cfg(target_os = "macos")]
struct SendApp(objc2::rc::Retained<objc2_app_kit::NSRunningApplication>);
#[cfg(target_os = "macos")]
unsafe impl Send for SendApp {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SendApp {}

#[cfg(target_os = "macos")]
static PREV_APP: Lazy<Mutex<Option<SendApp>>> = Lazy::new(|| Mutex::new(None));

/// 浮窗 show 前调用：app 非活跃时隐藏 Regular 窗口 + 记录前台 app。
/// 配合 `after_floating_window_hide` 形成闭环。
#[cfg(target_os = "macos")]
pub fn before_floating_window_show(app: &tauri::AppHandle) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWorkspace, NSRunningApplication};

    if let Some(mtm) = MainThreadMarker::new() {
        let app_ns = NSApplication::sharedApplication(mtm);
        let is_inactive = !app_ns.isActive();
        *WAS_INACTIVE.lock() = is_inactive;

        // 记录前台应用（焦点交还目标）
        let workspace = NSWorkspace::sharedWorkspace();
        if let Some(front_app) = workspace.frontmostApplication() {
            let curr = NSRunningApplication::currentApplication();
            if front_app.processIdentifier() != curr.processIdentifier() {
                *PREV_APP.lock() = Some(SendApp(front_app));
            }
        }

        // app 后台时临时隐藏 Regular 窗口
        if is_inactive {
            let mut hidden = Vec::new();
            for label in REGULAR_WINDOWS {
                if let Some(w) = app.get_webview_window(label) {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                        hidden.push(label.to_string());
                    }
                }
            }
            *TEMP_HIDDEN.lock() = hidden;
        }
    }
}

/// 浮窗 hide 后调用：交还前台焦点给原 app + 恢复被隐藏的 Regular 窗口。
#[cfg(target_os = "macos")]
pub fn after_floating_window_hide(app: &tauri::AppHandle) {
    let was_inactive = {
        let mut guard = WAS_INACTIVE.lock();
        std::mem::replace(&mut *guard, false)
    };

    if was_inactive {
        // 交还前台焦点给原 app
        let app_opt = {
            let mut guard = PREV_APP.lock();
            guard.take().map(|p| p.0)
        };
        if let Some(prev_app) = app_opt {
            let _ = prev_app.activateWithOptions(
                objc2_app_kit::NSApplicationActivationOptions(1 << 1),
            );
        } else {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSApplication;
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).deactivate();
            }
        }
    }

    // 恢复临时隐藏的 Regular 窗口（此时 app 已在后台，窗口温和恢复）
    let hidden = {
        let mut guard = TEMP_HIDDEN.lock();
        std::mem::take(&mut *guard)
    };
    for label in hidden {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }
}
