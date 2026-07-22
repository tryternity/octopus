//! 下载窗口：builtin 模型首次启动下载页。
//!
//! 独立 Tauri 窗口，原生标题栏，520×460 可调大小。
//! 单例管理：已打开则 set_focus，不重复创建。
//!
//! 详见 spec `docs/superpowers/specs/2026-07-22-builtin-models.md §3.2。
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
    // 单例检查 + focus（合并为一次 get_webview_window）
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_focus();
        return;
    }

    // 背景色 hex URL 注入——download.html 首帧即有色，零 CSS 依赖
    let url = if let Some(bg) = crate::theme::window_bg_hex(WINDOW_LABEL) {
        format!("download.html?bg={}", bg)
    } else {
        "download.html".to_string()
    };

    // 建窗失败记录日志（而非静默吞没）——builtin 缺失却无下载窗时便于排查
    if let Err(e) = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("Octopus - 内置模型下载")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build()
    {
        log::error!("[download_window] 建窗失败（builtin 模型缺失但无法展示下载页）: {e}");
    }
}

/// 关闭下载窗口（前端「后台下载」/「稍后下载」按钮调用）。
#[tauri::command]
pub fn close_download_window(app_handle: tauri::AppHandle) {
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = win.close();
    }
}
