//! macOS 输入源（IME）切换——粘贴前临时切到 ASCII 输入源，避免 CJK IME 干扰 Cmd+V。
//!
//! 背景：CJK 输入法（中文/日文/韩文）在 composing 状态下，模拟 Cmd+V 粘贴可能导致
//! 乱码或字符丢失（IME 把粘贴内容当作 composing 输入处理）。解法（参考 VoxFlow
//! VoxFlowTextInsertion 模块）：粘贴前临时切换到 ASCII 输入源（如 ABC）→ 模拟 Cmd+V
//! → 完成后恢复原输入源。
//!
//! 实现：macOS Carbon TIS（Text Input Source）API via FFI。Carbon framework 的
//! HIToolbox 提供 `TISCopyCurrentKeyboardInputSource` / `TISSelectInputSource` 等
//! C API。TISInputSourceRef 遵循 Core Foundation 保留计数（retain/release）。

// ── macOS ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::ffi::c_void;
    use std::ptr;
    use std::thread;
    use std::time::Duration;

    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;

    /// 切换输入源后等待 IME 稳定的延迟。经验值：50ms 足够 Carbon 注册切换。
    const SWITCH_SETTLE_DELAY: Duration = Duration::from_millis(50);

    // TIS API 符号位于 Carbon.framework（HIToolbox）。
    // CFArray* / CFRelease 符号由 core-foundation crate 传递链接的 CoreFoundation 提供。
    extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, propertyKey: *const c_void) -> CFTypeRef;
        fn TISSelectInputSource(source: CFTypeRef) -> i32;
        fn TISCreateInputSourceList(properties: CFTypeRef, includeAllInstalled: u8) -> CFTypeRef;

        fn CFArrayGetCount(array: CFTypeRef) -> i64;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: i64) -> *const c_void;
    }

    /// RAII guard：构造时切到 ASCII 输入源，drop 时恢复原输入源。
    pub struct InputSourceGuard {
        /// 构造时 `TISCopyCurrentKeyboardInputSource` 返回的 retained ref（+1）。
        /// drop 时用它恢复 + CFRelease 释放。
        previous: CFTypeRef,
    }

    impl InputSourceGuard {
        /// 切换到 ASCII 输入源（ABC / US），返回 guard。
        ///
        /// 返回 `None` 的情况（调用方无需恢复，不产生额外延迟）：
        /// - 当前输入源已是 ASCII（ABC / US）
        /// - 找不到可用的 ASCII 输入源
        /// - TISSelectInputSource 失败
        pub fn switch_to_ascii() -> Option<Self> {
            unsafe {
                let current = TISCopyCurrentKeyboardInputSource();
                if current.is_null() {
                    log::debug!("input_source: current source is null, skip");
                    return None;
                }

                // 已是 ASCII 输入源 → 无需切换（省 50ms 延迟）
                let cur_id = input_source_id(current).unwrap_or_default();
                if is_ascii_id(&cur_id) {
                    CFRelease(current);
                    log::debug!("input_source: already ASCII ({}), skip", cur_id);
                    return None;
                }

                // 找到并选中 ASCII 输入源
                if !select_ascii_source() {
                    log::warn!("input_source: no ASCII source available, paste with current IME");
                    CFRelease(current);
                    return None;
                }

                log::debug!(
                    "input_source: switched {} -> ASCII for paste",
                    cur_id
                );
                thread::sleep(SWITCH_SETTLE_DELAY);
                Some(InputSourceGuard { previous: current })
            }
        }
    }

    impl Drop for InputSourceGuard {
        fn drop(&mut self) {
            unsafe {
                if TISSelectInputSource(self.previous) != 0 {
                    log::warn!("input_source: restore failed (TISSelectInputSource != 0)");
                } else {
                    log::debug!("input_source: restored previous source");
                }
                CFRelease(self.previous);
            }
        }
    }

    /// 判断输入源 ID 是否为 ASCII 布局（ABC / US）。
    /// 现代 macOS 默认 ASCII 布局是 `com.apple.keylayout.ABC`，
    /// 旧版或自定义键盘可能用 `com.apple.keylayout.US`。
    fn is_ascii_id(id: &str) -> bool {
        id == "com.apple.keylayout.ABC" || id == "com.apple.keylayout.US"
    }

    /// 读取输入源的 InputSourceID 属性（如 "com.apple.keylayout.ABC"、
    /// "com.apple.inputmethod.SCIM.ITABC"）。Get rule——不释放。
    unsafe fn input_source_id(source: CFTypeRef) -> Option<String> {
        let key = CFString::new("TISPropertyInputSourceID");
        let val = TISGetInputSourceProperty(source, key.as_concrete_TypeRef() as *const c_void);
        if val.is_null() {
            return None;
        }
        let s = CFString::wrap_under_get_rule(val as *const _);
        Some(s.to_string())
    }

    /// 在已启用的输入源列表中找 ABC / US 并选中。
    /// 返回是否成功选中。
    unsafe fn select_ascii_source() -> bool {
        // includeAllInstalled=0 → 仅返回当前启用的输入源（用户实际可用的）
        let arr = TISCreateInputSourceList(ptr::null(), 0);
        if arr.is_null() {
            return false;
        }
        let count = CFArrayGetCount(arr);
        let mut found = false;
        for i in 0..count {
            let src = CFArrayGetValueAtIndex(arr, i);
            if src.is_null() {
                continue;
            }
            let id = input_source_id(src).unwrap_or_default();
            if is_ascii_id(&id) {
                found = TISSelectInputSource(src) == 0;
                break;
            }
        }
        CFRelease(arr);
        found
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ascii_id_detection() {
            assert!(is_ascii_id("com.apple.keylayout.ABC"));
            assert!(is_ascii_id("com.apple.keylayout.US"));
            assert!(!is_ascii_id("com.apple.inputmethod.SCIM.ITABC"));
            assert!(!is_ascii_id("com.apple.keylayout.Pinyin"));
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos_impl::InputSourceGuard;

/// 切换到 ASCII 输入源，返回 RAII guard（drop 时恢复原输入源）。
///
/// 仅 macOS 有效；其他平台 / 当前已是 ASCII / 切换失败时返回 `None`。
/// 调用方只需 `let _g = switch_to_ascii_for_paste();`——guard 在粘贴完成
/// 后 drop 自动恢复。
pub fn switch_to_ascii_for_paste() -> Option<InputSourceGuard> {
    #[cfg(target_os = "macos")]
    {
        InputSourceGuard::switch_to_ascii()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub struct InputSourceGuard;
