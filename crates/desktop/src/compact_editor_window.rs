//! 统一内容查看器窗口：独立 Tauri 窗口，原生标题栏，880×620 可调大小，居中。
//! 支持 tab 切换文本/图片/语音条目。窗口尺寸可调 + 记忆。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 settings 对称。

use tauri::{WebviewUrl, WebviewWindowBuilder, Manager};

const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 620.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";
const STATE_KEY: &str = "compact_editor_window_state";

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WindowState {
    width: f64,
    height: f64,
    x: f64,
    y: f64,
}

/// 读窗口状态记忆（DB app_config）。无记忆用默认 880×620 居中。
fn load_window_state() -> WindowState {
    octopus_infra::db::load_config_key(STATE_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 写窗口状态记忆到 DB。
fn save_window_state(state: &WindowState) {
    if let Ok(json) = serde_json::to_string(state) {
        let _ = octopus_infra::db::save_config_key(STATE_KEY, &json);
    }
}

/// 关窗时调用：保存当前窗口位置/大小到 DB。
pub fn on_compact_editor_save_state(app_handle: &tauri::AppHandle) {
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
            let state = WindowState {
                width: size.width as f64,
                height: size.height as f64,
                x: pos.x as f64,
                y: pos.y as f64,
            };
            log::info!("[compact-editor] save window state {:?}", state);
            save_window_state(&state);
        }
    }
}

/// 创建统一查看器窗口（调用方已确保当前不存在同名窗口）。
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

    let state = load_window_state();
    log::info!("[compact-editor] window state {:?}", state);

    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("查看")
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .visible(true);

    if state.width > 0.0 && state.height > 0.0 {
        builder = builder.inner_size(state.width, state.height).position(state.x, state.y);
    } else {
        builder = builder.inner_size(WIDTH, HEIGHT).center();
    }

    let _ = builder.build();
    log::info!("[compact-editor] after build");
}

/// macOS: 统一查看器窗口关闭后，仅当无其他常规窗口存活时才切回 Accessory（仅托盘）。
/// 同时保存窗口状态到 DB。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    on_compact_editor_save_state(app_handle);
    crate::activation::restore_accessory_if_no_regular_window(app_handle);
}
