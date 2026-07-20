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

    /// 模拟复制按键（Cmd+C / Ctrl+C）。
    pub fn simulate_copy(&self) {
        simulate_copy_platform();
    }
}

// ── macOS ──────────────────────────────────────────────────────────
// macOS 不需要焦点追踪：窗口 hide 后系统自动还焦点给上一个前台应用。
// 只需 hide + 300ms 延迟 + osascript 模拟 Cmd+V（与 clipboard_commands::paste_clipboard_item
// 的 sleep(300ms) 对齐；曾为 200ms，调优后延至 300ms 求稳，注释原写 200ms 已漂移）。

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
    // 三段式文本注入：切到 ASCII 输入源 → Cmd+V → guard drop 时恢复。
    // 避免 CJK IME composing 状态下粘贴出乱码（参考 VoxFlow VoxFlowTextInsertion）。
    let _ime_guard = crate::input_source::switch_to_ascii_for_paste();
    // 强制前台进程重新获取 key window 焦点，再 keystroke
    // 用 System Events 的 process 属性而非 application name（避免 -1728）
    let script = r#"tell application "System Events"
        set p to first process whose frontmost is true
        set frontmost of p to true
        delay 0.15
        keystroke "v" using command down
    end tell"#;
    log::info!("simulate_paste: osascript frontmost + keystroke");
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

#[cfg(target_os = "macos")]
fn simulate_copy_platform() {
    use std::process::Command;
    // 深度防御：热键触发后 octopus 可能短暂成为 frontmost。若抓到 octopus 自己，
    // Cmd+C 会发给 octopus webview 而非源应用。先确保 frontmost 是非 octopus 进程。
    // （注：Sublime 的"无选中复制当前行"是独立问题，由 detect_selection 的 Sublime
    // 插件分支处理，不靠此 Cmd+C。本保护针对其他应用的 octopus frontmost 边角场景。）
    //
    // 2026-07-20 perf：原固定 `delay 0.15` 在每个 Cmd+C 都等待（含 octopus 不是
    // frontmost 的正常情况），改为仅在「octopus 是 frontmost 需切焦点」时 delay。
    // 正常情况省 150ms。
    let script = r#"tell application "System Events"
        set p to first process whose frontmost is true
        if name of p is "octopus" then
            repeat with q in (every process whose background only is false)
                if name of q is not "octopus" and name of q is not "osascript" then
                    set frontmost of q to true
                    delay 0.15
                    exit repeat
                end if
            end repeat
        end if
        keystroke "c" using command down
    end tell"#;
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => {
            if !out.status.success() {
                log::warn!("simulate_copy failed: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
        Err(e) => log::warn!("simulate_copy: {}", e),
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

#[cfg(target_os = "windows")]
fn simulate_copy_platform() {}

// ── Linux ─────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn start_platform_listener() {
    log::info!("Focus tracker: Linux support not yet implemented");
}

#[cfg(target_os = "linux")]
fn restore_focus_platform() {}

#[cfg(target_os = "linux")]
fn simulate_paste_platform() {}

#[cfg(target_os = "linux")]
fn simulate_copy_platform() {}

// ── fallback ──

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn start_platform_listener() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn restore_focus_platform() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn simulate_paste_platform() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn simulate_copy_platform() {}
