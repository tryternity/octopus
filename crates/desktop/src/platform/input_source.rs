//! macOS 输入源（IME）切换——粘贴前临时切到 ASCII 输入源，避免 CJK IME 干扰 Cmd+V。
//!
//! 背景：CJK 输入法（中文/日文/韩文）在 composing 状态下，模拟 Cmd+V 粘贴可能导致
//! 乱码或字符丢失。解法：粘贴前临时切换到 ASCII 输入源（如 ABC）→ 模拟 Cmd+V
//! → 完成后恢复原输入源。
//!
//! ⚠️ **实现历史与踩坑**：
//! 1. 初版直接在调用线程调 Carbon TIS API → SIGTRAP（TIS 要求主线程）
//! 2. 二版用 GCD `dispatch_sync_f` 调度到主线程 → 仍然 SIGTRAP（Tokio spawn_blocking
//!    线程上下文与 libdispatch 主队列检测冲突）
//! 3. 终版用 `osascript` 在独立进程切换——完全绕开 TIS FFI 和线程问题

// ── macOS ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::process::Command;

    /// RAII guard：构造时切到 ASCII 输入源，drop 时恢复原输入源。
    pub struct InputSourceGuard {
        /// 原输入源的 ID（如 "com.apple.inputmethod.SCIM.ITABC"）。
        previous_id: String,
    }

    impl InputSourceGuard {
        /// 切换到 ASCII 输入源（ABC / US），返回 guard。
        ///
        /// 用 osascript 在独立进程内调用 Carbon TIS API——独立进程的 main thread
        /// 天然满足 TIS 的线程要求，完全绕开 paste 调用线程的限制。
        ///
        /// 返回 `None`：当前已是 ASCII / 切换失败 / 非 macOS。
        pub fn switch_to_ascii() -> Option<Self> {
            // 1. 读当前输入源 ID
            let current = match read_current_source_id() {
                Some(id) => id,
                None => {
                    log::debug!("input_source: cannot read current source, skip");
                    return None;
                }
            };

            // 已是 ASCII → 无需切换
            if is_ascii_id(&current) {
                log::debug!("input_source: already ASCII ({}), skip", current);
                return None;
            }

            // 2. 切到 ABC
            if !select_source("com.apple.keylayout.ABC") {
                // ABC 不行试 US
                if !select_source("com.apple.keylayout.US") {
                    log::warn!(
                        "input_source: failed to select ABC/US, paste with current IME ({})",
                        current
                    );
                    return None;
                }
            }

            log::debug!("input_source: {} -> ASCII for paste", current);
            // osascript 进程退出 = 切换已完成（synchronous），但仍留短暂时间给 Carbon
            std::thread::sleep(std::time::Duration::from_millis(50));
            Some(InputSourceGuard {
                previous_id: current,
            })
        }
    }

    impl Drop for InputSourceGuard {
        fn drop(&mut self) {
            if !select_source(&self.previous_id) {
                log::warn!(
                    "input_source: restore to {} failed",
                    self.previous_id
                );
            } else {
                log::debug!("input_source: restored to {}", self.previous_id);
            }
        }
    }

    /// 判断输入源 ID 是否为 ASCII 布局。
    fn is_ascii_id(id: &str) -> bool {
        id == "com.apple.keylayout.ABC" || id == "com.apple.keylayout.US"
    }

    /// 用 osascript 读取当前键盘输入源 ID。
    /// JXA（JavaScript for Automation）调 Carbon TIS API。
    fn read_current_source_id() -> Option<String> {
        // JXA 调用 ObjC bridge → Carbon TIS
        let script = r#"
ObjC.import('Carbon')
const src = $.TISCopyCurrentKeyboardInputSource();
const idRef = $.TISGetInputSourceProperty(src, $('TISPropertyInputSourceID').jsObject);
const id = ObjC.unwrap(ObjC.unwrap(idRef).toString).js;
id;
"#;
        let out = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", script])
            .output()
            .ok()?;
        if !out.status.success() {
            log::debug!(
                "input_source: osascript read failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// 用 osascript 切换到指定输入源 ID。
    fn select_source(id: &str) -> bool {
        // JXA：枚举已启用的输入源列表，匹配 ID 后 Select
        let script = format!(
            r#"
ObjC.import('Carbon')
const list = $.TISCreateInputSourceList(null, false).jsObject;
const count = $.CFArrayGetCount(list);
let ok = false;
for (let i = 0; i < count; i++) {{
    const src = $.CFArrayGetValueAtIndex(list, i);
    const idRef = $.TISGetInputSourceProperty(src, $('TISPropertyInputSourceID').jsObject);
    const srcId = ObjC.unwrap(ObjC.unwrap(idRef).toString).js;
    if (srcId === '{id}') {{
        const status = $.TISSelectInputSource(src);
        ok = (status === 0);
        break;
    }}
}}
ok ? 'true' : 'false';
"#,
            id = id
        );
        let out = match Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                log::debug!("input_source: osascript select failed: {}", e);
                return false;
            }
        };
        if !out.status.success() {
            log::debug!(
                "input_source: osascript select error: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return false;
        }
        String::from_utf8_lossy(&out.stdout).trim() == "true"
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
