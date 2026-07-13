//! macOS Accessibility (AXUIElement) C FFI 声明。
//!
//! AX 函数在 ApplicationServices/HIServices framework。
//! kAX* 属性名用 CFString::new() 构造（extern static 在 Rust 链接器不可见）。

#![cfg(target_os = "macos")]

use core_foundation::base::CFTypeRef;
use core_foundation::string::CFString;

/// AXUIElement 不透明指针
pub type AXUIElementRef = *const std::ffi::c_void;
/// AXValue 不透明指针
pub type AXValueRef = *const std::ffi::c_void;

pub type AXError = i32;
pub type AXValueType = u32;

/// AXValue 类型枚举值（AXValue.h）
#[allow(non_upper_case_globals)]
pub const kAXValueCFRangeType: AXValueType = 4;

/// CFRange（值类型，用于 AXValueGetValue 解码选区范围）
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CFRange {
    pub location: i64,
    pub length: i64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: core_foundation::string::CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXValueGetValue(
        value: AXValueRef,
        the_type: AXValueType,
        value_ptr: *mut std::ffi::c_void,
    ) -> AXError;
    #[allow(dead_code)] // 预留：权限检查未启用
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
}

// AX 属性名 CFString——用 new() 构造（extern static 不可链接）
pub fn ax_focused_ui_element() -> CFString {
    CFString::new("AXFocusedUIElement")
}
pub fn ax_selected_text_range() -> CFString {
    CFString::new("AXSelectedTextRange")
}
pub fn ax_value() -> CFString {
    CFString::new("AXValue")
}
pub fn ax_title() -> CFString {
    CFString::new("AXTitle")
}
