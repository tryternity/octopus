//! 图片预览窗口：动态创建（非预建隐藏窗）。
//!
//! 打开 → macOS 切 Regular（Dock 出现）；关闭 → RunEvent::Destroyed 路由回 Accessory。
//! 镜像 compact_editor_window 的激活策略与构建方式。

use tauri::{ActivationPolicy, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 620.0;
const MIN_WIDTH: f64 = 400.0;
const MIN_HEIGHT: f64 = 320.0;

pub const WINDOW_LABEL: &str = "image_preview_window";

pub fn create_image_preview_window(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Regular);
        crate::settings_window::set_dock_icon();
    }
    let _ = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
        .title("图片预览")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .decorations(true)
        .resizable(true)
        .center()
        .visible(true)
        .build();
}

/// 窗口销毁后恢复 Accessory（Dock 图标隐藏），与 compact_editor 一致。
#[cfg(target_os = "macos")]
pub fn on_image_preview_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
}
