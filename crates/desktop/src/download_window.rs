//! 下载窗口：builtin 模型首次启动下载页。
//!
//! 独立 Tauri 窗口，原生标题栏，520×460 可调大小。
//! 单例管理：已打开则 set_focus，不重复创建。
//!
//! 详见 spec `docs/superpowers/specs/2026-07-22-builtin-models.md` §3.2。
//! 启动时 main.rs setup 检测 builtin 缺失 → [`create_download_window`]。
//! 用户点「后台下载」/「稍后下载」→ 前端 close 窗口，主窗口（action_bar）正常使用。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "download_window";
const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 460.0;
const MIN_WIDTH: f64 = 400.0;
const MIN_HEIGHT: f64 = 360.0;

/// 创建下载窗口（单例：已存在则 set_focus）。
/// 启动时由 setup 钩子调用——builtin 模型缺失时弹出。
pub fn create_download_window(app_handle: &tauri::AppHandle) {
    if app_handle.get_webview_window(WINDOW_LABEL).is_some() {
        // 已打开（如用户上次选「稍后下载」没关）—— 仅 focus
        let _ = app_handle.get_webview_window(WINDOW_LABEL).map(|w| w.set_focus());
        return;
    }

    // 背景色 hex URL 注入——download.html 首帧即有色，零 CSS 依赖
    let url = if let Some(bg) = crate::theme::window_bg_hex(WINDOW_LABEL) {
        format!("download.html?bg={}", bg)
    } else {
        "download.html".to_string()
    };

    let _ = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("Octopus - 内置模型下载")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}

/// 关闭下载窗口（前端「后台下载」/「稍后下载」按钮调用）。
#[tauri::command]
pub fn close_download_window(app_handle: tauri::AppHandle) {
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = win.close();
    }
}
