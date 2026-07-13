//! macOS Accessibility (AXUIElement) C FFI 声明。
//!
//! AX 函数在 ApplicationServices/HIServices framework，返回 core-foundation 类型。
//! 项目已依赖 core-foundation 0.10，这里只声明 AX 特有的 extern。

#![cfg(target_os = "macos")]

use core_foundation::base::CFTypeRef;
use core_foundation::string::CFStringRef;

/// AXUIElement 不透明指针
pub type AXUIElementRef = *const std::ffi::c_void;
/// AXValue 不透明指针
pub type AXValueRef = *const std::ffi::c_void;

pub type AXError = i32;
pub type AXValueType = u32;

/// AXValue 类型枚举值（AXValue.h）
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
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXValueGetValue(
        value: AXValueRef,
        the_type: AXValueType,
        value_ptr: *mut std::ffi::c_void,
    ) -> AXError;
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;

    // kAX* 属性字符串是外部 CFStringRef 全局符号
    pub static kAXFocusedUIElementAttribute: CFStringRef;
    pub static kAXSelectedTextAttribute: CFStringRef;
    pub static kAXSelectedTextRangeAttribute: CFStringRef;
    pub static kAXValueAttribute: CFStringRef;
    pub static kAXTitleAttribute: CFStringRef;
    pub static kAXRoleAttribute: CFStringRef;
}
