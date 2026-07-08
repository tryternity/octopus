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
pub fn hide_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            if win.is_visible().unwrap_or(false) {
                let _ = win.hide();
            }
        }
    }
}

/// 浮窗操作完成后调用：恢复之前被隐藏的常规窗口。
pub fn show_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            // 只 show 不 focus——不抢焦点
            let _ = win.show();
        }
    }
}
