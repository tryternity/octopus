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
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    pub fn AXValueGetTypeID() -> core_foundation::base::CFTypeID;
}

// ── Accessibility 权限 ─────────────────────────────────────────────
// macOS 辅助功能（AX）权限是 TCC 运行期授权——非 sandbox app 不走 entitlement，
// 必须由 AXIsProcessTrustedWithOptions 主动触发系统弹窗。
// app_context / autotype / keystroke / paste 均依赖 AX 权限。

/// 静默检查 AX 权限（不弹窗）。keystroke / autotype / app_context 共用。
pub fn is_accessibility_trusted() -> bool {
    // null options = 不弹权限对话框，只查当前状态
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// 主动触发 AX 权限请求弹窗（异步，与 cpal 触发麦克风 TCC 弹窗同范式）。
///
/// 传 `kAXTrustedCheckOptionPrompt=true` 的 CFDictionary 给
/// `AXIsProcessTrustedWithOptions`——首次调用触发 TCC 弹窗（"打开系统设置"），
/// 引导用户到 "系统设置 > 隐私与安全 > 辅助功能" 授权。
///
/// 返回值是**当前**授权状态：弹窗异步，不改变返回值，故首次几乎一定返 false。
/// 本函数语义是"踢一脚系统弹窗"，不是判定——判定请用 `is_accessibility_trusted`。
pub fn prompt_accessibility_permission() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    // kAXTrustedCheckOptionPrompt 用 new() 构造——与同文件 ax_focused_ui_element()
    // 同范式（extern static kAX* 在 Rust 链接器不可见，HIServices 不导出符号）。
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let options: CFDictionary<CFString, CFBoolean> =
        CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);

    // as_CFTypeRef() 是 get-rule（不转移所有权）；options 活到函数返回，调用期间有效。
    unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) }
}

/// 缓存 AXValue 的 CFTypeID（进程内不变）。
fn cached_ax_value_type_id() -> core_foundation::base::CFTypeID {
    use std::sync::OnceLock;
    static ID: OnceLock<core_foundation::base::CFTypeID> = OnceLock::new();
    *ID.get_or_init(|| unsafe { AXValueGetTypeID() })
}

/// AXValue 的 CFTypeID（用于 is_cf_value 类型守卫）。
pub fn ax_value_type_id() -> core_foundation::base::CFTypeID {
    cached_ax_value_type_id()
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
pub fn ax_role() -> CFString {
    CFString::new("AXRole")
}
pub fn ax_children() -> CFString {
    CFString::new("AXChildren")
}

// ── 屏幕录制权限（CoreGraphics）──────────────────────────────────────
// CGPreflightScreenCaptureAccess / CGRequestScreenCaptureAccess 在 CoreGraphics framework，
// macOS 10.15+。必须在主进程调用——helper 子进程调用不触发 TCC 弹窗（打包版根因）。

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 静默检查屏幕录制权限（不弹窗）。
pub fn is_screen_capture_trusted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 主动触发屏幕录制权限请求弹窗（异步，与 AX prompt 同范式）。
///
/// 必须在**主进程**调用——helper 子进程调此函数不触发 TCC 弹窗（macOS 限制）。
/// 返回值是当前状态（弹窗异步，首次几乎一定 false）——语义是"踢一脚弹窗"。
/// 打包版 .app 主进程调用能正常触发系统「打开系统设置」对话框。
pub fn prompt_screen_capture_permission() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}
