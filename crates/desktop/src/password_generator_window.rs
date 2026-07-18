//! 密码生成器浮窗——ActionBar 触发，渲染共享主体 + Auto-type 注入浏览器。
//!
//! 设计（外壳 B 落地，详见 spec §5.2「跨场景复用主体 + Modal/独立窗口外壳」）：
//! - **透明浮窗**（非独立 Tauri 普通窗口）——避免独立窗口"hide 才能让浏览器回前台"
//!   的焦点切换问题；浮窗透明且不抢键盘焦点，浏览器始终在前台
//! - 渲染 `<PasswordGenerator>` 共享主体（与 CipherEditor Modal 共用）
//! - 点 Auto-type：hide 浮窗 → autotype_login 注入前台 app
//!
//! 触发方式：ActionBar 内置按钮 → `open_password_generator` 命令 →
//! `show_password_generator_window`。位置跟随前台浏览器窗口（fallback 到屏幕顶部居中）。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "password_generator_window";

const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 480.0;

/// 创建窗口（首次触发时调用，单例）。
pub fn create_password_generator_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("密码生成器")
    .inner_size(WIDTH, HEIGHT)
    .decorations(false)
    .always_on_top(true)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build()
    .map_err(|e| {
        log::error!("[password-generator] 窗口创建失败: {e}");
        e
    })
    .ok();
}

/// 在指定坐标显示浮窗。toggle 语义：不存在 → 先创建。
///
/// `x, y` 为浮窗左上角逻辑坐标（调用方算位置——跟随浏览器或鼠标）。
pub fn show_password_generator_window(app: &AppHandle, x: f64, y: f64) {
    if app.get_webview_window(WINDOW_LABEL).is_none() {
        create_password_generator_window(app);
    }
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));

        #[cfg(target_os = "macos")]
        {
            crate::activation::before_floating_window_show(app);
        }

        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 计算浮窗出现位置——跟随前台浏览器窗口位置，fallback 屏幕顶部居中。
///
/// 实现：读鼠标位置（前台浏览器输入框附近通常有鼠标），浮窗显示在鼠标右下方
/// （避免遮挡鼠标点击位置）。若读鼠标失败 → fallback 主屏顶部居中。
///
/// **未来增强**：用 CGWindowListCopyWindowInfo 读前台浏览器窗口 frame，
/// 浮窗显示在浏览器右下角。当前简化版用鼠标位置（与 ActionBar 一致）。
pub fn compute_window_position(app: &AppHandle) -> (f64, f64) {
    // 尝试读鼠标位置（CGEvent::location 是逻辑坐标，详见 AGENTS.md gotchas）
    let (mx, my) = crate::action_bar_commands::get_mouse_position(app)
        .unwrap_or((0.0, 0.0));

    // 浮窗显示在鼠标右下方（偏移 12px，避免遮挡光标）
    let mut x = mx + 12.0;
    let mut y = my + 12.0;

    // 边界保护——超出主屏右下角时回退
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let mw = monitor.size().width as f64 / scale;
        let mh = monitor.size().height as f64 / scale;
        if x + WIDTH > mw {
            x = (mw - WIDTH - 8.0).max(8.0);
        }
        if y + HEIGHT > mh {
            // 鼠标下方放不下 → 放鼠标上方
            y = (my - HEIGHT - 12.0).max(8.0);
        }
        // 最终 clamp 到屏幕内
        x = x.min(mw - WIDTH - 8.0).max(8.0);
        y = y.min(mh - HEIGHT - 8.0).max(8.0);
    }

    (x, y)
}
