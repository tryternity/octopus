//! 全局窗口焦点追踪——记住"弹出剪贴板窗口之前的那个前台应用"，
//! 双击粘贴时恢复焦点到该应用。
//!
//! 平台策略：
//! - macOS：不追踪 PID——窗口 hide 后 macOS 自动还焦点给上一个应用。
//!   只需 hide + 延迟 + osascript 模拟 Cmd+V。
//! - Windows：SetWinEventHook + SetForegroundWindow + enigo Shift+Insert（Task 2）
//! - Linux：X11 focus event + XRaiseWindow + enigo Shift+Insert（Task 3）

pub struct FocusTracker;

impl FocusTracker {
    pub fn new() -> Self {
        FocusTracker
    }

    /// 启动全局焦点监听。失败时不阻断应用（双击降级为只复制）。
    pub fn start(&self) -> anyhow::Result<()> {
        start_platform_listener();
        Ok(())
    }

    /// 恢复焦点到上一个前台窗口。
    pub fn restore_focus(&self) {
        restore_focus_platform();
    }

    /// 模拟粘贴按键。
    pub fn simulate_paste(&self) {
        simulate_paste_platform();
    }
}

// ── macOS ──────────────────────────────────────────────────────────
// macOS 不需要焦点追踪：窗口 hide 后系统自动还焦点给上一个前台应用。
// 只需 hide + 200ms 延迟 + osascript 模拟 Cmd+V。

#[cfg(target_os = "macos")]
fn start_platform_listener() {
    log::info!("Focus tracker: macOS (no-op, relies on window hide auto-restore)");
}

#[cfg(target_os = "macos")]
fn restore_focus_platform() {
    // octopus 是 Accessory 应用（无 Dock），窗口 hide 后 macOS 不自动还焦点。
    // 用 osascript 把焦点切回上一个前台应用。
    use std::process::Command;
    let script = r#"tell application "System Events"
        set frontMost to name of first process whose frontmost is true
        if frontMost is "octopus" then
            -- 焦点还在 octopus，切到上一个应用
            repeat with p in (every process whose background only is false)
                if name of p is not "octopus" and name of p is not "osascript" then
                    set frontmost of p to true
                    return
                end if
            end repeat
        end if
    end tell"#;
    let _ = Command::new("osascript").args(["-e", script]).output();
}

#[cfg(target_os = "macos")]
fn simulate_paste_platform() {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    log::info!("simulate_paste: enigo Cmd+V");
    match Enigo::new(&Settings::default()) {
        Ok(mut enigo) => {
            // macOS：用固定虚拟键码 kVK_ANSI_V=9（与 paste.rs 一致，绕开 enigo Carbon TIS 线程问题）
            let mod_key = Key::Meta;
            let v_key = Key::Other(9);
            let _ = enigo.key(mod_key, Direction::Press);
            let _ = enigo.key(v_key, Direction::Click);
            let _ = enigo.key(mod_key, Direction::Release);
            log::info!("simulate_paste: enigo Cmd+V done");
        }
        Err(e) => log::warn!("simulate_paste: enigo init failed: {}", e),
    }
}

// ── Windows ──────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn start_platform_listener() {
    log::info!("Focus tracker: Windows support not yet implemented");
}

#[cfg(target_os = "windows")]
fn restore_focus_platform() {}

#[cfg(target_os = "windows")]
fn simulate_paste_platform() {}

// ── Linux ─────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn start_platform_listener() {
    log::info!("Focus tracker: Linux support not yet implemented");
}

#[cfg(target_os = "linux")]
fn restore_focus_platform() {}

#[cfg(target_os = "linux")]
fn simulate_paste_platform() {}

// ── fallback ──

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn start_platform_listener() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn restore_focus_platform() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn simulate_paste_platform() {}
