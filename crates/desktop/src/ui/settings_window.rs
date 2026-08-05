//! 设置窗口：独立 Tauri 窗口，原生标题栏，960×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! macOS：打开设置窗口时切换到 Regular 激活策略（Dock 显示图标），
//! 关闭时切回 Accessory（仅托盘）。

use parking_lot::Mutex;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_WIDTH: f64 = 960.0;
const SETTINGS_HEIGHT: f64 = 700.0;
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "settings_window";

/// 暂存初始页面（新建窗口时前端 mount 后主动拉取）。
static PENDING_PAGE: Mutex<Option<String>> = Mutex::new(None);

/// 打开设置窗口（单例：已存在则 set_focus）。
/// `initial_page`: 可选，指定初始页面（"history" | "clipboard" | "settings" | "models" | "prompts"）。
#[tauri::command]
pub fn open_settings(app_handle: tauri::AppHandle, initial_page: Option<String>) {
    if app_handle.get_webview_window(WINDOW_LABEL).is_some() {
        // macOS: app 可能被其他应用遮挡——set_focus 仅设焦点不激活 app。
        // 需要切 Regular + 主线程 activate 才能把 app 带到前台。
        // ensure_show=true：窗口可能在浮窗存活期间被 before_floating_window_show 临时隐藏
        //（settings_window 在 WINDOWS_TO_HIDE_ON_FLOAT 列表）。此时 set_focus 对 hidden
        // 窗口无效 → 补 w.show()（P2-1 修复，对齐 compact_editor_command 范式）。
        crate::platform::activation::focus_regular_window(&app_handle, WINDOW_LABEL, true);
        // 窗口已存在：暂存页面 + emit 让前端切页
        if let Some(ref page) = initial_page {
            *PENDING_PAGE.lock() = Some(page.clone());
            let _ = app_handle.emit("settings://navigate", page.clone());
        }
        return;
    }
    // macOS: 打开设置窗口 → Dock 显示图标 + 设置应用图标 + 激活到前台
    #[cfg(target_os = "macos")]
    {
        crate::platform::activation::activate_regular_for_new_window(&app_handle);
    }

    // 暂存初始页面，等前端 mount 后调 get_initial_page 拉取
    if let Some(page) = initial_page {
        *PENDING_PAGE.lock() = Some(page);
    }

    // 背景色 hex URL 注入——settings.html 首帧即有色，零 CSS 依赖
    let url = if let Some(bg) = crate::ui::theme::window_bg_hex(WINDOW_LABEL) {
        format!("settings.html?bg={}", bg)
    } else {
        "settings.html".to_string()
    };

    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("Octopus")
    .inner_size(SETTINGS_WIDTH, SETTINGS_HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}

/// 前端 mount 后调用，拉取并清除暂存的初始页面。
#[tauri::command]
pub fn get_initial_page() -> Option<String> {
    PENDING_PAGE.lock().take()
}

/// 设置窗口关闭后回调：仅当无其他常规窗口存活时才切回 Accessory（仅托盘）。
#[cfg(target_os = "macos")]
pub fn on_settings_closed(app_handle: &tauri::AppHandle) {
    crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
}
