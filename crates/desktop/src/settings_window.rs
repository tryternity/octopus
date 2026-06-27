//! 设置窗口：独立 Tauri 窗口，原生标题栏，800×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! macOS：打开设置窗口时切换到 Regular 激活策略（Dock 显示图标），
//! 关闭时切回 Accessory（仅托盘）。

use std::sync::Mutex;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_WIDTH: f64 = 800.0;
const SETTINGS_HEIGHT: f64 = 600.0;
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "settings_window";

/// 暂存初始页面（新建窗口时前端 mount 后主动拉取）。
static PENDING_PAGE: Mutex<Option<String>> = Mutex::new(None);

/// 打开设置窗口（单例：已存在则 set_focus）。
/// `initial_page`: 可选，指定初始页面（"history" | "clipboard" | "settings" | "models" | "prompts"）。
#[tauri::command]
pub fn open_settings(app_handle: tauri::AppHandle, initial_page: Option<String>) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        // 窗口已存在，直接 emit 让前端切页
        if let Some(ref page) = initial_page {
            let _ = app_handle.emit_to(WINDOW_LABEL, "settings://navigate", page);
        }
        return;
    }
    // macOS: 打开设置窗口 → Dock 显示图标 + 设置应用图标
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        set_dock_icon();
    }

    // 暂存初始页面，等前端 mount 后调 get_initial_page 拉取
    if let Some(page) = initial_page {
        *PENDING_PAGE.lock().unwrap() = Some(page);
    }

    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
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
    PENDING_PAGE.lock().unwrap().take()
}

/// 设置窗口关闭后回调：切回 Accessory（仅托盘）。
#[cfg(target_os = "macos")]
pub fn on_settings_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

/// macOS: 手动设置 Dock 图标（release 裸二进制无 .app bundle，Tauri 仅在
/// debug 模式自动设置）。
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const ICON_PNG: &[u8] = include_bytes!("../icons/icon.png");

    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(app_icon) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { app.setApplicationIconImage(Some(&app_icon)) };
    }
}
