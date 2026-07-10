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
//!
//! ⚠️ **线程安全**：TIS API **必须在主线程调用**——在非主线程调用会触发 SIGTRAP
//! （`Trace/BPT trap: 5`），与 enigo `UCKeyTranslate` 的崩溃同源。粘贴路径
//! （`paste_via_clipboard` / `simulate_paste_platform`）都在非主线程执行
//! （`spawn_blocking` / `std::thread::spawn`），因此所有 TIS 调用必须经 GCD
//! `dispatch_sync_f` 调度到主线程。

// ── macOS ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::ffi::c_void;
    use std::ptr;
    use std::time::Duration;

    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;

    /// 切换输入源后等待 IME 稳定的延迟。经验值：50ms 足够 Carbon 注册切换。
    /// 在调用线程 sleep（不占主线程），dispatch_sync_f 返回后再等。
    const SWITCH_SETTLE_DELAY: Duration = Duration::from_millis(50);

    // TIS API 符号位于 Carbon.framework（HIToolbox）。
    extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, propertyKey: *const c_void) -> CFTypeRef;
        fn TISSelectInputSource(source: CFTypeRef) -> i32;
        fn TISCreateInputSourceList(properties: CFTypeRef, includeAllInstalled: u8) -> CFTypeRef;

        fn CFArrayGetCount(array: CFTypeRef) -> i64;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: i64) -> *const c_void;

        // GCD（Grand Central Dispatch）——把 TIS 调用调度到主线程。
        // `_dispatch_main_q` 是主队列的全局符号（dispatch_get_main_queue() 宏展开的目标）。
        fn dispatch_sync_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
        static _dispatch_main_q: c_void;
    }

    /// 返回 GCD 主队列指针。
    fn main_queue() -> *const c_void {
        unsafe { &_dispatch_main_q as *const _ as *const c_void }
    }

    // ── TIS 调用的上下文（经 dispatch_sync_f 传递到主线程）──

    /// switch_to_ascii 的工作上下文。
    struct SwitchCtx {
        /// 传出：原输入源的 retained ref（+1，需 CFRelease）。
        previous: CFTypeRef,
        /// 传出：是否成功切换。
        switched: bool,
        /// 传出：当前输入源 ID（日志用）。
        cur_id: [u8; 64],
        cur_id_len: usize,
    }

    extern "C" fn do_switch(ctx: *mut c_void) {
        unsafe {
            let cx = &mut *(ctx as *mut SwitchCtx);
            let current = TISCopyCurrentKeyboardInputSource();
            if current.is_null() {
                cx.switched = false;
                return;
            }

            let id = input_source_id(current).unwrap_or_default();
            write_id(&mut cx.cur_id, &mut cx.cur_id_len, &id);

            if is_ascii_id(&id) {
                CFRelease(current);
                cx.switched = false; // 已是 ASCII，无需切换
                return;
            }

            if !select_ascii_source() {
                CFRelease(current);
                cx.switched = false;
                return;
            }

            cx.previous = current;
            cx.switched = true;
        }
    }

    /// restore 的上下文（就是 previous ref 本身）。
    extern "C" fn do_restore(ctx: *mut c_void) {
        unsafe {
            let prev = ctx as CFTypeRef;
            if TISSelectInputSource(prev) != 0 {
                log::warn!("input_source: restore failed (TISSelectInputSource != 0)");
            }
            CFRelease(prev);
        }
    }

    // ── RAII Guard ──

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
            let mut cx = SwitchCtx {
                previous: ptr::null(),
                switched: false,
                cur_id: [0u8; 64],
                cur_id_len: 0,
            };

            // dispatch_sync_f：阻塞调用线程，在主线程执行 do_switch，完成后返回。
            // 不会死锁：粘贴路径在 spawn_blocking / std::thread::spawn，绝非主线程。
            unsafe {
                dispatch_sync_f(
                    main_queue(),
                    &mut cx as *mut SwitchCtx as *mut c_void,
                    do_switch,
                );
            }

            if cx.switched {
                let id = read_id(&cx.cur_id, cx.cur_id_len);
                log::debug!("input_source: switched {} -> ASCII for paste", id);
                std::thread::sleep(SWITCH_SETTLE_DELAY);
                Some(InputSourceGuard {
                    previous: cx.previous,
                })
            } else {
                let id = read_id(&cx.cur_id, cx.cur_id_len);
                if !id.is_empty() && is_ascii_id(&id) {
                    log::debug!("input_source: already ASCII ({}), skip", id);
                } else if !id.is_empty() {
                    log::warn!(
                        "input_source: no ASCII source available (current={}), paste with current IME",
                        id
                    );
                } else {
                    log::debug!("input_source: current source is null, skip");
                }
                None
            }
        }
    }

    impl Drop for InputSourceGuard {
        fn drop(&mut self) {
            unsafe {
                dispatch_sync_f(
                    main_queue(),
                    self.previous as *mut c_void,
                    do_restore,
                );
            }
        }
    }

    // ── 辅助函数（在主线程上下文内调用）──

    /// 判断输入源 ID 是否为 ASCII 布局（ABC / US）。
    fn is_ascii_id(id: &str) -> bool {
        id == "com.apple.keylayout.ABC" || id == "com.apple.keylayout.US"
    }

    /// 读取输入源的 InputSourceID 属性。
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
    unsafe fn select_ascii_source() -> bool {
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

    // ── 小工具：固定大小 buffer 传字符串（避免在 dispatch 上下文分配 String）──

    fn write_id(buf: &mut [u8; 64], len: &mut usize, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(63);
        buf[..n].copy_from_slice(&bytes[..n]);
        buf[n] = 0;
        *len = n;
    }

    fn read_id(buf: &[u8; 64], len: usize) -> String {
        if len == 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..len]).into_owned()
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

        #[test]
        fn write_read_id_roundtrip() {
            let mut buf = [0u8; 64];
            let mut len = 0;
            write_id(&mut buf, &mut len, "com.apple.keylayout.ABC");
            assert_eq!(read_id(&buf, len), "com.apple.keylayout.ABC");
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
