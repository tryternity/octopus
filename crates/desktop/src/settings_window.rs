//! 设置窗口：独立 Tauri 窗口，原生标题栏，800×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! macOS：打开设置窗口时切换到 Regular 激活策略（Dock 显示图标），
//! 关闭时切回 Accessory（仅托盘）。

use parking_lot::Mutex;
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
    if let Some(_) = app_handle.get_webview_window(WINDOW_LABEL) {
        // macOS: app 可能被其他应用遮挡——set_focus 仅设焦点不激活 app。
        // 需要切 Regular + 主线程 activate 才能把 app 带到前台。
        // 用 activation::activate_self 双保险（NSApplication + NSRunningApplication），
        // 应对托盘菜单关闭时 macOS 焦点恢复覆盖 activate 的"偶尔不激活"问题。
        #[cfg(target_os = "macos")]
        {
            let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
            let ah = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                crate::activation::activate_self();
                let _ = ah.get_webview_window(WINDOW_LABEL).map(|w| w.set_focus());
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app_handle.get_webview_window(WINDOW_LABEL).map(|w| w.set_focus());
        }
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
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        // activate_self + set_dock_icon 调 AppKit，必须在主线程执行。
        // open_settings 是 Tauri command（worker 线程），用 run_on_main_thread 调度。
        // ⚠️ 必须显式 activate——app 在 Accessory 模式下从托盘点击时，
        // macOS 不会自动把 app 带到前台（窗口创建了但不前置）。
        let _ = app_handle.run_on_main_thread(|| {
            crate::activation::activate_self();
            set_dock_icon();
        });
    }

    // 暂存初始页面，等前端 mount 后调 get_initial_page 拉取
    if let Some(page) = initial_page {
        *PENDING_PAGE.lock() = Some(page);
    }

    // 背景色 hex URL 注入——index.html 首帧即有色，零 CSS 依赖
    let url = if let Some(bg) = crate::theme::window_bg_hex(WINDOW_LABEL) {
        format!("index.html?bg={}", bg)
    } else {
        "index.html".to_string()
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
    crate::activation::restore_accessory_if_no_regular_window(app_handle);
}

/// macOS: 手动设置 Dock 图标（release 裸二进制无 .app bundle，Tauri 仅在
/// debug 模式自动设置）。必须在主线程调用（由 open_settings 通过
/// run_on_main_thread 调度）。
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const ICON_PNG: &[u8] = include_bytes!("../icons/icon.png");

    // 安全检查：仅主线程可调 AppKit
    let mtm = match MainThreadMarker::new() {
        Some(mtm) => mtm,
        None => {
            log::warn!("set_dock_icon called off main thread, skipping");
            return;
        }
    };
    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(app_icon) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { app.setApplicationIconImage(Some(&app_icon)) };
    }
}
