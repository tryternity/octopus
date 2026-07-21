//! 模拟键盘按键——统一基础能力，供 focus_tracker / paste / autotype 共用。
//!
//! macOS 实现用 `core-graphics` 的 CGEvent（与 `action_bar_commands::get_mouse_position`
//! 共用同一依赖），替代原 `focus_tracker.rs` 的 osascript（osascript 启动 ~200ms，
//! CGEvent 调用 < 5ms）。
//!
//! ⚠️ CGEvent.post() 在 AX 权限缺失时会静默失败（不报错但没发出去），故本模块主动
//! 调 `AXIsProcessTrustedWithOptions` 检查，缺失时 bail。
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

/// 发送「修饰键 + 字符键」组合。
///
/// macOS：CGEvent new_keyboard_event + set_flags + post(HID)。
/// 其他平台：no-op + warn（Windows/Linux 未来用 enigo 实现）。
pub fn send_key_combo(modifier: KeyModifier, key_code: u8) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        send_key_combo_macos(modifier, key_code)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (modifier, key_code);
        log::warn!("[keystroke] 模拟按键仅 macOS 支持");
        Ok(())
    }
}

/// Cmd+C（复制）。
pub fn copy() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::C)
}

/// Cmd+V（粘贴）。
pub fn paste() -> Result<()> {
    send_key_combo(KeyModifier::Command, keycodes::V)
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

#[cfg(target_os = "macos")]
fn send_key_combo_macos(modifier: KeyModifier, key_code: u8) -> Result<()> {
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

    send_one_key(&source, key_code as u16, true, flags)?;
    send_one_key(&source, key_code as u16, false, flags)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn send_one_key(
    source: &core_graphics::event_source::CGEventSource,
    key_code: u16,
    key_down: bool,
    flags: core_graphics::event::CGEventFlags,
) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    let event = CGEvent::new_keyboard_event(source.clone(), key_code, key_down)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_keyboard_event 失败 (key={:#x} down={})", key_code, key_down))?;
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// 检查 AX 权限。FFI 范式来自 `autotype/macos.rs:17-30`。
#[cfg(target_os = "macos")]
fn check_accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    }
    // null options = 不弹权限对话框，只查当前状态
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}
