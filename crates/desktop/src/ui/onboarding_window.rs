//! 首次启动权限引导窗口：独立 Tauri 窗口，原生标题栏，展示 3 个权限卡片。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! macOS：打开时切换到 Regular 激活策略（Dock 显示图标），关闭时切回 Accessory。
//! 与 settings_window 同模式（settings_window.rs）。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "onboarding_window";

/// 打开引导窗口（单例：已存在则 set_focus）。
/// 仅首次启动调用（AppConfig.onboarding_completed == false）。
pub fn open_onboarding(app_handle: &tauri::AppHandle) {
    if app_handle.get_webview_window(WINDOW_LABEL).is_some() {
        crate::platform::activation::focus_regular_window(app_handle, WINDOW_LABEL, false);
        return;
    }
    // macOS: 打开引导窗口 → Dock 显示图标 + 激活到前台
    #[cfg(target_os = "macos")]
    {
        crate::platform::activation::activate_regular_for_new_window(app_handle);
    }

    // 背景色 hex URL 注入（与 settings_window 一致，首帧即有色）
    let url = if let Some(bg) = crate::ui::theme::window_bg_hex(WINDOW_LABEL) {
        format!("onboarding.html?bg={}", bg)
    } else {
        "onboarding.html".to_string()
    };

    let _ = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("Octopus")
    .inner_size(720.0, 560.0)
    .min_inner_size(480.0, 400.0)
    .decorations(true)
    .visible(true)
    .build();
}

/// 引导窗口关闭后回调：切回 Accessory（仅托盘）。
#[cfg(target_os = "macos")]
pub fn on_onboarding_closed(app_handle: &tauri::AppHandle) {
    crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
}

/// 完成 onboarding：写 DB flag + 触发延迟的 recorder.open（首次启动时）+ 关窗。
///
/// `skip`: true=跳过（用户未授权全部权限就完成）；false=正常完成。
/// 无论授权状态如何都置 onboarding_completed=true（不再重复弹）。
#[tauri::command]
pub fn complete_onboarding(app_handle: tauri::AppHandle) {
    // 写 flag
    if let Err(e) = octopus_infra::db::save_config_key("onboarding_completed", "true") {
        log::error!("[onboarding] 写 onboarding_completed 失败: {e}");
    }
    // 关窗（Destroyed 事件触发 on_onboarding_closed restore Accessory）
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = win.close();
    }
}
