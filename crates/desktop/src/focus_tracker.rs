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
    use std::process::Command;

    // 打印当前前台
    if let Ok(out) = Command::new("osascript")
        .args(["-e", r#"tell application "System Events" to get name of first process whose frontmost is true"#])
        .output()
    {
        log::info!("restore_focus: current frontmost = {}", String::from_utf8_lossy(&out.stdout).trim());
    }

    // 如果前台是 octopus，切到上一个应用
    let script = r#"tell application "System Events"
        set frontMost to name of first process whose frontmost is true
        if frontMost is "octopus" then
            repeat with p in (every process whose background only is false)
                if name of p is not "octopus" and name of p is not "osascript" then
                    set frontmost of p to true
                    return name of p
                end if
            end repeat
        end if
    end tell"#;
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => log::info!("restore_focus: switch result = {}", String::from_utf8_lossy(&out.stdout).trim()),
        Err(e) => log::warn!("restore_focus: osascript failed: {}", e),
    }
}

#[cfg(target_os = "macos")]
fn simulate_paste_platform() {
    use std::process::Command;
    // 先获取当前前台应用名，激活它再发 Cmd+V（确保 key window 正确）
    let script = r#"tell application "System Events"
        set appName to name of first process whose frontmost is true
    end tell
    tell application appName to activate
    delay 0.1
    tell application "System Events" to keystroke "v" using command down"#;
    log::info!("simulate_paste: osascript activate + Cmd+V");
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => {
            if !out.status.success() {
                log::warn!("simulate_paste failed: {}", String::from_utf8_lossy(&out.stderr));
            } else {
                log::info!("simulate_paste: done");
            }
        }
        Err(e) => log::warn!("simulate_paste: {}", e),
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
