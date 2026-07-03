//! 精简编辑器窗口：独立 Tauri 窗口，原生标题栏，720×560 可调大小，居中。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 settings 对称。

use tauri::{WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 560.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";

/// 创建精简编辑器窗口（调用方已确保当前不存在同名窗口）。
///
/// ⚠️ 必须在主线程调用：内含 macOS AppKit 主线程操作（set_activation_policy +
/// set_dock_icon，后者用 `MainThreadMarker::new_unchecked` 强制假定主线程）。
/// 从 async worker 线程同步调用会导致整个应用僵死。若需从 worker 触发建窗，
/// 用 `app_handle.run_on_main_thread(...)` 投递（见 screenshot_commands::ocr_screenshot）。
pub fn create_compact_editor_window(app_handle: &tauri::AppHandle) {
    log::info!("[compact-editor] create start");
    // macOS：编辑窗口切 Regular 让 Dock 显示图标（与 settings 一致）。
    #[cfg(target_os = "macos")]
    {
        log::info!("[compact-editor] before set_activation_policy(Regular)");
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        log::info!("[compact-editor] after set_activation_policy(Regular)");
        log::info!("[compact-editor] before set_dock_icon");
        crate::settings_window::set_dock_icon();
        log::info!("[compact-editor] after set_dock_icon");
    }
    log::info!("[compact-editor] before build");
    let _ = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("编辑")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .center()
    .visible(true)
    .build();
    log::info!("[compact-editor] after build");
}

/// macOS: 精简编辑器窗口关闭后，仅当无其他常规窗口存活时才切回 Accessory（仅托盘）。
/// 与 settings_window::on_settings_closed 对称。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    crate::activation::restore_accessory_if_no_regular_window(app_handle);
}
