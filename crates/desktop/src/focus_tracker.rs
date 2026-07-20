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
    // 2026-07-21 perf：原用 osascript（两次 osascript 进程，每次 ~200ms，共 ~400ms）。
    // 改用 NSRunningApplication（objc2-app-kit，直调 < 1ms）。
    //
    // 逻辑（与原 osascript 等价）：
    //   1. 读 frontmost app（NSWorkspace）
    //   2. 如果是 octopus 自己，找另一个非 octopus 的前台 app 来激活
    //   3. 否则，无条件 re-activate frontmost app（触发 windowDidBecomeKey，
    //      让窗口成为 key window——这对 CGEvent 发 Cmd+V 触发菜单快捷键至关重要）
    use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSApplicationActivationOptions};

    let workspace = NSWorkspace::sharedWorkspace();
    let frontmost = workspace.frontmostApplication();
    let frontmost_name = frontmost.as_ref().and_then(|a| a.localizedName()).map(|s| s.to_string()).unwrap_or_default();
    log::info!("restore_focus: current frontmost = {}", frontmost_name);

    // 判断是否是 octopus 自己
    let is_octopus = frontmost.as_ref()
        .and_then(|a| a.bundleIdentifier())
        .map(|s| s.to_string())
        .as_deref()
        .map(|bid| bid.contains("octopus"))
        .unwrap_or(false)
        || frontmost_name == "octopus";

    let target: Option<objc2::rc::Retained<NSRunningApplication>> = if is_octopus {
        // 找另一个非 octopus 的前台 app
        let apps = workspace.runningApplications();
        apps.into_iter()
            .find(|app| {
                let is_bg = app.activationPolicy() == objc2_app_kit::NSApplicationActivationPolicy::Prohibited;
                if is_bg { return false; }
                let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
                name != "octopus" && !name.is_empty()
            })
    } else {
        // 无条件 re-activate frontmost（触发 windowDidBecomeKey）
        frontmost.clone()
    };

    if let Some(app) = target {
        // NSApplicationActivateAllWindows = 1 << 0（项目统一值，见 activation.rs 注释）
        let _ = app.activateWithOptions(NSApplicationActivationOptions(1 << 0));
        let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
        log::info!("restore_focus: activated '{}'", name);
    } else {
        log::warn!("restore_focus: no target app to activate");
    }
}

#[cfg(target_os = "macos")]
fn simulate_paste_platform() {
    // 2026-07-20 perf：原 osascript（~200ms 启动）改用 keystroke 模块（CGEvent < 5ms）。
    // IME guard 仍保留——切到 ASCII 输入源再 paste，避免 CJK IME composing 状态下粘贴乱码。
    let _ime_guard = crate::input_source::switch_to_ascii_for_paste();

    // 切输入源可能短暂抢焦点（Carbon TIS API），等 50ms 让焦点稳定再发 keystroke
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 2026-07-21 三级 dispatch：
    //   1. WKWebView 嵌套 app（微信内置浏览器）→ osascript（~200ms，走 System Events 菜单路由）
    //   2. Electron app（豆包/ZCode）→ CGEventPostToPid（定向，< 5ms）
    //   3. 原生 app → CGEventPostToPid（或全局 post fallback）
    let frontmost = crate::app_context::macos_ax::frontmost_app();
    let result = dispatch_paste(frontmost);
    if let Err(e) = result {
        log::warn!("simulate_paste: {}", e);
    }
}

#[cfg(target_os = "macos")]
fn simulate_copy_platform() {
    // 2026-07-20 perf：原 osascript（~200ms 启动 + delay 0.15）改用 keystroke 模块（CGEvent < 5ms）。
    // 2026-07-21 三级 dispatch（跟 simulate_paste 同策略）：
    //   1. WKWebView 嵌套 app → osascript
    //   2. Electron app → CGEventPostToPid
    //   3. 原生 app → CGEventPostToPid
    let frontmost = crate::app_context::macos_ax::frontmost_app();
    let result = dispatch_copy(frontmost);
    if let Err(e) = result {
        log::warn!("simulate_copy: {}", e);
    }
}

/// 三级 dispatch：根据 frontmost app 类型选最佳按键发送方式。
///
/// 调用方先读 `frontmost_app()`（一次 IPC），传入这里复用，避免重复调用。
#[cfg(target_os = "macos")]
fn dispatch_copy(frontmost: Option<(i32, Option<String>, String)>) -> anyhow::Result<()> {
    match &frontmost {
        Some((_, bid, _)) if crate::keystroke::needs_osascript_fallback(bid.as_deref()) => {
            log::info!("simulate_copy: WKWebView app {:?} → osascript", bid);
            crate::keystroke::copy_via_osascript()
        }
        Some((pid, _, _)) => crate::keystroke::copy_to_pid(*pid),
        None => crate::keystroke::copy(),
    }
}

#[cfg(target_os = "macos")]
fn dispatch_paste(frontmost: Option<(i32, Option<String>, String)>) -> anyhow::Result<()> {
    match &frontmost {
        Some((_, bid, _)) if crate::keystroke::needs_osascript_fallback(bid.as_deref()) => {
            log::info!("simulate_paste: WKWebView app {:?} → osascript", bid);
            crate::keystroke::paste_via_osascript()
        }
        Some((pid, _, _)) => crate::keystroke::paste_to_pid(*pid),
        None => crate::keystroke::paste(),
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
