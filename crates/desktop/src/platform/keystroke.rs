//! 模拟键盘按键——统一基础能力，供 focus_tracker / paste / autotype 共用。
//!
//! macOS 实现用 `core-graphics` 的 CGEvent（与 `action_bar_commands::get_mouse_position`
//! 共用同一依赖），替代原 `focus_tracker.rs` 的 osascript（osascript 启动 ~200ms，
//! CGEvent 调用 < 5ms）。
//!
//! ⚠️ CGEvent.post() 在 AX 权限缺失时会静默失败（不报错但没发出去），故本模块主动
//! 调 `AXIsProcessTrustedWithOptions` 检查，缺失时 bail。
//!
//! **Electron 兼容（2026-07-21）**：Electron app（豆包/VS Code 等）不接收 CGEvent.post(HID)
//! 发的菜单快捷键——它们的 Chromium 事件处理路径跟原生 app 不同。改用 `CGEventPostToPid(pid)`
//! 定向发给目标进程，绕过全局事件路由，Electron app 也能接收。
//!
//! **WKWebView 嵌套兼容（2026-07-21）**：微信内置浏览器等 WKWebView 嵌套组件不响应
//! 外部注入的 CGEvent（不论全局 post 还是 post_to_pid）。此类 app 回退到 osascript
//! `keystroke`（通过 System Events 高层 API 走完整菜单路由，~200ms 但兼容性好）。
//! 由 [`WKWEBVIEW_FALLBACK_APPS`] bundle id 列表驱动。
//!
//! 其他平台 no-op + warn。

use anyhow::Result;

/// macOS virtual keycodes（US 布局，CGKeyCode 低 8 位）。
/// 详见 https://opensource.apple.com/source/IOHIDFamily/IOHIDFamily-700/IOHIDSystem/IOKit/hidsystem/IOLLEvent.h
#[allow(dead_code)]  // 常量库，按需用
pub mod keycodes {
    pub const A: u8 = 0x00;
    pub const S: u8 = 0x01;
    pub const D: u8 = 0x02;
    pub const F: u8 = 0x03;
    pub const H: u8 = 0x04;
    pub const Z: u8 = 0x06;
    pub const X: u8 = 0x07;
    pub const C: u8 = 0x08;
    pub const V: u8 = 0x09;
    pub const B: u8 = 0x0B;
    pub const Q: u8 = 0x0C;
    pub const W: u8 = 0x0D;
    pub const E: u8 = 0x0E;
    pub const R: u8 = 0x0F;
    pub const T: u8 = 0x11;
    pub const Y: u8 = 0x10;
    pub const RETURN: u8 = 0x24;
    pub const TAB: u8 = 0x30;
    pub const SPACE: u8 = 0x31;
    pub const DELETE: u8 = 0x33;
    pub const ESCAPE: u8 = 0x35;
    pub const COMMAND: u8 = 0x37;
    pub const SHIFT: u8 = 0x38;
    pub const OPTION: u8 = 0x3A;
    pub const CONTROL: u8 = 0x3B;
}

/// 修饰键组合。
#[allow(dead_code)]  // 按需用，目前只用 Command
#[derive(Clone, Copy, Debug)]
pub enum KeyModifier {
    None,
    Command,
    Shift,
    Control,
    Option,
    CommandShift,
}

/// 需要 osascript fallback 的 app bundle id 列表。
///
/// 这些 app 内嵌 WKWebView 或类似组件，不响应外部注入的 CGEvent（不论全局 post
/// 还是 post_to_pid）。osascript `keystroke` 通过 System Events 走完整菜单路由，
/// 兼容性最好但慢（~200ms）。
///
/// 2026-07-21 实测：微信（`com.tencent.xinWeChat`）内置浏览器选中文字时，
/// CGEventPostToPid 的 Cmd+C 不触发复制（changeCount 不变），osascript 正常。
const WKWEBVIEW_FALLBACK_APPS: &[&str] = &[
    "com.tencent.xinWeChat",  // 微信（内置浏览器 WKWebView 嵌套）
];

/// 检查 bundle id 是否需要 osascript fallback（WKWebView 嵌套 app）。
pub fn needs_osascript_fallback(bundle_id: Option<&str>) -> bool {
    match bundle_id {
        Some(bid) => WKWEBVIEW_FALLBACK_APPS.iter().any(|&fallback| bid == fallback),
        None => false,
    }
}

/// 发送「修饰键 + 字符键」组合——全局广播（`CGEventPost(HID)`）。
///
/// 适用于原生 macOS app（Sublime/iTerm2 等）。
/// **Electron app**（豆包/VS Code）不接收全局 CGEvent，需用 [`send_key_combo_to_pid`]。
/// **WKWebView 嵌套 app**（微信内置浏览器）不响应任何 CGEvent，需用 [`send_via_osascript`]。
pub fn send_key_combo(modifier: KeyModifier, key_code: u8) -> Result<()> {
    send_key_combo_impl(modifier, key_code, None)
}

/// 发送「修饰键 + 字符键」组合——定向发给指定进程（`CGEventPostToPid`）。
///
/// **Electron 兼容**：Electron app 不接收 `CGEventPost(HID)` 的全局事件——
/// Chromium 的事件处理路径与原生 app 不同，全局 CGEvent 发的菜单快捷键不触发。
/// `CGEventPostToPid` 绕过全局事件路由，直接投递到目标进程的 HID 队列。
///
/// `pid` 传 0 等同于全局广播（fallback 行为）。
pub fn send_key_combo_to_pid(modifier: KeyModifier, key_code: u8, pid: i32) -> Result<()> {
    if pid <= 0 {
        return send_key_combo(modifier, key_code);
    }
    send_key_combo_impl(modifier, key_code, Some(pid))
}

/// 通过 osascript 发送按键（System Events `keystroke`）。
///
/// **WKWebView 嵌套 app 兼容**：微信内置浏览器等不响应 CGEvent（不论全局 post
/// 还是 post_to_pid），osascript 通过 System Events 高层 API 走完整菜单路由。
///
/// 代价：osascript 进程启动 ~200ms（vs CGEvent < 5ms）。`Command::output()` 同步
/// 等待 osascript 进程结束——而 System Events 的 keystroke 同步让目标 app 处理完
/// 才返回，所以本函数返回时复制/粘贴动作已完成。
///
/// **关键约束（2026-07-21 踩坑，已验证）**：
/// 必须用完整 `tell application "System Events" ... end tell` block，但 **不要**
/// 用 `tell frontProc` 显式包裹 `keystroke`。验证过的最佳版本：
///   set frontProc to first process whose frontmost is true
///   keystroke "c" using command down  ← 在 System Events 块作用域内，不显式 tell frontProc
/// 加 `tell frontProc ... end tell` 反而让 keystroke 失效（changeCount +0）——
/// 推测：`tell frontProc` 把命令绑定到 process 的 AX 上下文，但 WKWebView 嵌套层
/// 的 menu bar 不归 WeChat process 的 AX 拥有，所以 Cmd+C 没触发菜单。
///
/// `key_char` 是按键字符（如 "c" / "v"），`using` 是修饰键（如 "command down"）。
#[cfg(target_os = "macos")]
pub fn send_via_osascript(key_char: &str, using: &str) -> Result<()> {
    use std::process::Command;
    // 不用 tell frontProc 包裹 keystroke！见函数 doc 注释的踩坑记录。
    // 完整 tell block + set frontProc（用于诊断）+ keystroke 在 System Events 作用域内。
    let script = format!(
        r#"tell application "System Events"
            set frontProc to first process whose frontmost is true
            set procName to name of frontProc
            keystroke "{key}" using {using}
            return procName
        end tell"#,
        key = key_char, using = using
    );
    let out = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| anyhow::anyhow!("osascript 启动失败: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("osascript keystroke 失败: {}", stderr);
    }
    let proc_name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    log::info!("[osascript] keystroke '{}' using {} → frontmost='{}'", key_char, using, proc_name);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn send_via_osascript(_key_char: &str, _using: &str) -> Result<()> {
    Ok(())
}

/// Cmd+C（复制）——全局广播。
pub fn copy() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::C)
}

/// Cmd+V（粘贴）——全局广播。
pub fn paste() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::V)
}

/// Cmd+V（粘贴）——定向发给指定 pid（Electron app 用）。
pub fn paste_to_pid(pid: i32) -> Result<()> {
    send_key_combo_to_pid(KeyModifier::Command, keycodes::V, pid)
}

/// Cmd+C（复制）——定向发给指定 pid（Electron app 用）。
pub fn copy_to_pid(pid: i32) -> Result<()> {
    send_key_combo_to_pid(KeyModifier::Command, keycodes::C, pid)
}

/// Cmd+C（复制）——osascript（WKWebView 嵌套 app 用）。
pub fn copy_via_osascript() -> Result<()> {
    #[cfg(target_os = "macos")]
    { send_via_osascript("c", "command down") }
    #[cfg(not(target_os = "macos"))]
    { Ok(()) }
}

/// Cmd+V（粘贴）——osascript（WKWebView 嵌套 app 用）。
pub fn paste_via_osascript() -> Result<()> {
    #[cfg(target_os = "macos")]
    { send_via_osascript("v", "command down") }
    #[cfg(not(target_os = "macos"))]
    { Ok(()) }
}

/// Cmd+X（剪切）。
#[allow(dead_code)]
pub fn cut() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::X)
}

/// Cmd+A（全选）。
#[allow(dead_code)]
pub fn select_all() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::A)
}

// ── macOS 实现 ────────────────────────────────────────────────────

fn send_key_combo_impl(modifier: KeyModifier, key_code: u8, pid: Option<i32>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        send_key_combo_macos(modifier, key_code, pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (modifier, key_code, pid);
        log::warn!("[keystroke] 模拟按键仅 macOS 支持");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn send_key_combo_macos(modifier: KeyModifier, key_code: u8, pid: Option<i32>) -> Result<()> {
    use core_graphics::event::CGEventFlags;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    if !check_accessibility_trusted() {
        anyhow::bail!("Accessibility 权限未授予，CGEvent.post 会静默失败");
    }

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource 创建失败"))?;

    let flags = match modifier {
        KeyModifier::None => CGEventFlags::CGEventFlagNull,
        KeyModifier::Command => CGEventFlags::CGEventFlagCommand,
        KeyModifier::Shift => CGEventFlags::CGEventFlagShift,
        KeyModifier::Control => CGEventFlags::CGEventFlagControl,
        KeyModifier::Option => CGEventFlags::CGEventFlagAlternate,
        KeyModifier::CommandShift => {
            CGEventFlags::CGEventFlagCommand | CGEventFlags::CGEventFlagShift
        }
    };

    send_one_key(&source, key_code as u16, true, flags, pid)?;
    send_one_key(&source, key_code as u16, false, flags, pid)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn send_one_key(
    source: &core_graphics::event_source::CGEventSource,
    key_code: u16,
    key_down: bool,
    flags: core_graphics::event::CGEventFlags,
    pid: Option<i32>,
) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    let event = CGEvent::new_keyboard_event(source.clone(), key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_keyboard_event 失败 (key={:#x} down={})", key_code, key_down))?;
    event.set_flags(flags);
    match pid {
        Some(pid) => event.post_to_pid(pid),
        None => event.post(CGEventTapLocation::HID),
    }
    Ok(())
}

/// 检查 AX 权限（委托 app_context::ffi 统一入口，去重 3 处 extern 声明）。
#[cfg(target_os = "macos")]
fn check_accessibility_trusted() -> bool {
    crate::platform::app_context::ffi::is_accessibility_trusted()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_osascript_fallback_wechat() {
        assert!(needs_osascript_fallback(Some("com.tencent.xinWeChat")));
    }

    #[test]
    fn test_needs_osascript_fallback_sublime() {
        assert!(!needs_osascript_fallback(Some("com.sublimetext.4")));
    }

    #[test]
    fn test_needs_osascript_fallback_doubao() {
        assert!(!needs_osascript_fallback(Some("com.electron.doubao")));
    }

    #[test]
    fn test_needs_osascript_fallback_none() {
        assert!(!needs_osascript_fallback(None));
    }

    #[test]
    fn test_wkwebview_fallback_list_contains_wechat() {
        assert!(WKWEBVIEW_FALLBACK_APPS.contains(&"com.tencent.xinWeChat"));
        assert!(!WKWEBVIEW_FALLBACK_APPS.contains(&"com.sublimetext.4"));
    }
}

