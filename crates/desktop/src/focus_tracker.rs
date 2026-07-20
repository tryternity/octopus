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
    let frontmost_name = Command::new("osascript")
        .args(["-e", r#"tell application "System Events" to get name of first process whose frontmost is true"#])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    log::info!("restore_focus: current frontmost = {}", frontmost_name);

    // 2026-07-20 perf+fix：原仅当 octopus 是 frontmost 时才切换。
    // 但 macOS frontmost app ≠ key window holder——hide clipboard_window 后，即便 Sublime
    // 是 frontmost，它的窗口可能还不是 key window（CGEvent 发 Cmd+V 进 NSApp.sendEvent 队列
    // 但不触发 menu shortcut 匹配）。改为：无条件 set frontmost，触发目标 app 的
    // windowDidBecomeKey，让窗口成为 key window。
    let script = r#"tell application "System Events"
        set p to first process whose frontmost is true
        if name of p is "octopus" then
            repeat with q in (every process whose background only is false)
                if name of q is not "octopus" and name of q is not "osascript" then
                    set frontmost of q to true
                    set p to q
                    exit repeat
                end if
            end repeat
        else
            -- 无条件 re-set frontmost，触发 windowDidBecomeKey
            set frontmost of p to true
        end if
        return name of p
    end tell"#;
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => log::info!("restore_focus: activate result = '{}'", String::from_utf8_lossy(&out.stdout).trim()),
        Err(e) => log::warn!("restore_focus: osascript failed: {}", e),
    }
}

#[cfg(target_os = "macos")]
fn simulate_paste_platform() {
    // 2026-07-20 perf：原 osascript（~200ms 启动）改用 keystroke 模块（CGEvent < 5ms）。
    // IME guard 仍保留——切到 ASCII 输入源再 paste，避免 CJK IME composing 状态下粘贴乱码。
    let _ime_guard = crate::input_source::switch_to_ascii_for_paste();

    // 切输入源可能短暂抢焦点（Carbon TIS API），等 50ms 让焦点稳定再发 keystroke
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 2026-07-21 Electron 兼容：读 frontmost pid，用 post_to_pid 定向发 Cmd+V
    // （全局 CGEventPost(HID) 不触发 Electron/Chromium 的菜单快捷键）。
    // frontmost_app() 返回 None 时 fallback 到全局 post（极少发生）。
    let pid = crate::app_context::macos_ax::frontmost_app().map(|(pid, _, _)| pid);
    let result = match pid {
        Some(pid) => crate::keystroke::paste_to_pid(pid),
        None => crate::keystroke::paste(),
    };
    if let Err(e) = result {
        log::warn!("simulate_paste: {}", e);
    }
}

#[cfg(target_os = "macos")]
fn simulate_copy_platform() {
    // 2026-07-20 perf：原 osascript（~200ms 启动 + delay 0.15）改用 keystroke 模块（CGEvent < 5ms）。
    // 2026-07-21 Electron 兼容：跟 simulate_paste 一样，读 frontmost pid 用 post_to_pid 定向发 Cmd+C
    // （Electron app 不接收全局 CGEventPost(HID)）。
    // 已知限制：微信内置浏览器（WKWebView 嵌套）即使 post_to_pid 也不响应外部 Cmd+C——
    // WKWebView 有自己的事件处理，不响应注入的键盘事件。此场景需后续用 AX API 或其他方式处理。
    let pid = crate::app_context::macos_ax::frontmost_app().map(|(pid, _, _)| pid);
    let result = match pid {
        Some(pid) => crate::keystroke::copy_to_pid(pid),
        None => crate::keystroke::copy(),
    };
    if let Err(e) = result {
        log::warn!("simulate_copy: {}", e);
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
