//! 密码生成器浮窗——ActionBar 触发，渲染共享主体 + Auto-type 注入浏览器。
//!
//! 设计（外壳 B 落地，详见 spec §5.2「跨场景复用主体 + Modal/独立窗口外壳」）：
//! - **透明浮窗**（非独立 Tauri 普通窗口）——避免独立窗口"hide 才能让浏览器回前台"
//!   的焦点切换问题；浮窗透明且不抢键盘焦点，浏览器始终在前台
//! - 渲染 `<PasswordGenerator>` 共享主体（与 CipherEditor Modal 共用）
//! - 点 Auto-type：hide 浮窗 → autotype_login 注入前台 app
//!
//! 触发方式：ActionBar 内置按钮 → `open_password_generator` 命令 →
//! `show_password_generator_window`。位置跟随前台浏览器窗口（fallback 到鼠标位置）。

use tauri::{AppHandle, Manager};
use crate::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "password_generator_window";

const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 480.0;

/// 已知浏览器 owner name（app display name，CGWindowList 给的是 owner name 非 bundle id）。
///
/// ⚠️ **与 `crates/desktop/src/autotype/url_detect.rs` 的 `script_for_browser` 是
/// 两套独立列表**（那里按 bundle id 匹配，这里按 owner name）——新增浏览器时需同步两处。
/// 未来可统一抽成 `BROWSERS: &[(bundle_id, owner_name)]` 常量源，当前轻量处理。
const BROWSER_OWNER_NAMES: &[&str] = &[
    "Chrome",
    "Google Chrome",
    "Microsoft Edge",
    "Brave Browser",
    "Safari",
    "Firefox",
    "Arc",
];

/// 创建窗口（首次触发时调用，单例）。
pub fn create_password_generator_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "password-generator.html",
        title: "密码生成器",
        inner_size: (WIDTH, HEIGHT),
        visible: false,
        resizable: false,
        position: None,
        focused: None,
        accept_first_mouse: None,
    })
    .map_err(|e| log::error!("[password-generator] 窗口创建失败: {e}"));
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

/// 浮窗 frame 算法结果——携带左上角坐标 + 来源（用于诊断日志）。
#[derive(Debug, Clone, Copy)]
pub struct WindowPosition {
    pub x: f64,
    pub y: f64,
    pub source: PositionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSource {
    /// 跟随前台浏览器窗口（理想情况）
    BrowserFrame,
    /// 跟随鼠标（fallback：未找到前台浏览器 / 读 frame 失败）
    MousePosition,
    /// 屏幕顶部居中（fallback：鼠标位置也读不到）
    ScreenTopCenter,
}

/// 计算浮窗出现位置——优先跟随前台浏览器窗口，fallback 鼠标 → 屏幕顶部居中。
///
/// 实现（2026-07-19 增强）：
/// 1. 读前台浏览器 frame（CGWindowListCopyWindowInfo + owner name 白名单匹配）
/// 2. 浮窗显示在浏览器右下角（不遮挡浏览器输入框——通常在左上/中央）
/// 3. 读不到浏览器 → fallback 鼠标位置（CGEvent::location）
/// 4. 鼠标也读不到 → fallback 主屏顶部居中
///
/// 所有路径都做屏幕边界保护。
pub fn compute_window_position(app: &AppHandle) -> WindowPosition {
    // 主屏边界（用于所有路径的 clamp）
    let (screen_w, screen_h) = primary_screen_size(app).unwrap_or((1440.0, 900.0));

    // 1. 尝试读前台浏览器 frame
    if let Some((bx, by, bw, bh)) = frontmost_browser_frame() {
        // 浮窗放浏览器右下角（不遮挡浏览器输入框——通常左上/中央）
        // 偏移 16px 让浮窗不贴浏览器边缘
        let prefer_x = bx + bw - WIDTH - 16.0;
        let prefer_y = by + bh - HEIGHT - 16.0;
        let x = prefer_x.max(8.0).min(screen_w - WIDTH - 8.0);
        let y = prefer_y.max(8.0).min(screen_h - HEIGHT - 8.0);
        return WindowPosition { x, y, source: PositionSource::BrowserFrame };
    }

    // 2. fallback：鼠标位置（前台浏览器输入框附近通常有鼠标）
    if let Some((mx, my)) = crate::action_bar::action_bar_commands::get_mouse_position(app) {
        // 浮窗显示在鼠标右下方（偏移 12px，避免遮挡光标）
        let mut x = mx + 12.0;
        let mut y = my + 12.0;
        if x + WIDTH > screen_w {
            x = (mx - WIDTH - 12.0).max(8.0);
        }
        if y + HEIGHT > screen_h {
            y = (my - HEIGHT - 12.0).max(8.0);
        }
        let x = x.min(screen_w - WIDTH - 8.0).max(8.0);
        let y = y.min(screen_h - HEIGHT - 8.0).max(8.0);
        return WindowPosition { x, y, source: PositionSource::MousePosition };
    }

    // 3. fallback：屏幕顶部居中
    let x = ((screen_w - WIDTH) / 2.0).max(8.0);
    let y = 32.0;
    WindowPosition { x, y, source: PositionSource::ScreenTopCenter }
}

/// 读主屏逻辑尺寸（用于边界 clamp）。
fn primary_screen_size(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok()??;
    let scale = monitor.scale_factor();
    let w = monitor.size().width as f64 / scale;
    let h = monitor.size().height as f64 / scale;
    Some((w, h))
}

/// 读前台浏览器窗口 frame（CGWindowListCopyWindowInfo + owner name 白名单）。
///
/// macOS CGWindowList API 不直接给 bundle id，但给 owner name（= app display name，
/// 多数情况 = app 名固定）。我们用 `BROWSER_OWNER_NAMES` 白名单匹配。
///
/// 返回 `(x, y, w, h)`，全为**逻辑像素**（CGWindowList 返回的是 points，已除 scale）。
///
/// **复用**：与 screenshot_commands.rs:748 `get_window_pid_at_point` 同一 API 范式，
/// 但本函数找前台（layer 最小=最上层）的浏览器窗口，而非坐标命中。
#[cfg(target_os = "macos")]
fn frontmost_browser_frame() -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::display::CGDisplay;

    // kCGWindowListOptionOnScreenOnly = 1<<0 = 只列屏幕上的窗口（按 layer 升序）
    let windows = CGDisplay::window_list_info(
        core_graphics::display::kCGWindowListOptionOnScreenOnly,
        None,
    )?;

    // 遍历找第一个 owner name ∈ 白名单 的窗口（数组已按 layer 升序，第一个匹配即最上层浏览器）
    for item in windows.iter() {
        let dict_ref = *item as CFTypeRef;
        if dict_ref.is_null() {
            continue;
        }
        let dict: CFDictionary<CFString, CFTypeRef> =
            unsafe { TCFType::wrap_under_get_rule(dict_ref as *const _) };

        // 读 owner name
        let key_owner = CFString::new("kCGWindowOwnerName");
        let owner_ptr: CFTypeRef = *dict.get(&key_owner);
        if owner_ptr.is_null() {
            continue;
        }
        let owner_name: CFString = unsafe { TCFType::wrap_under_get_rule(owner_ptr as *const _) };
        let owner_str = owner_name.to_string();
        if !BROWSER_OWNER_NAMES.iter().any(|n| *n == owner_str.as_str()) {
            continue;
        }

        // 读 bounds（CGWindowList 给的是 NSDictionary：{X, Y, Width, Height}，全 points）
        let key_bounds = CFString::new("kCGWindowBounds");
        let bounds_ptr: CFTypeRef = *dict.get(&key_bounds);
        if bounds_ptr.is_null() {
            continue;
        }
        let bdict: CFDictionary<CFString, CFTypeRef> =
            unsafe { TCFType::wrap_under_get_rule(bounds_ptr as *const _) };
        let get_f64 = |key: &str| -> f64 {
            let k = CFString::new(key);
            let ptr: CFTypeRef = *bdict.get(&k);
            if ptr.is_null() {
                return 0.0;
            }
            let n: CFNumber = unsafe { TCFType::wrap_under_get_rule(ptr as *const _) };
            n.to_f64().unwrap_or(0.0)
        };
        let (x, y, w, h) = (get_f64("X"), get_f64("Y"), get_f64("Width"), get_f64("Height"));

        // 过滤掉过小的窗口（可能是浏览器的小弹窗/工具窗口）
        if w < 200.0 || h < 200.0 {
            continue;
        }

        return Some((x, y, w, h));
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn frontmost_browser_frame() -> Option<(f64, f64, f64, f64)> {
    None
}
