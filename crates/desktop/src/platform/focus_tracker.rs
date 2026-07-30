//! 全局窗口焦点追踪——记住"弹出剪贴板/结果窗口之前的那个前台应用"，
//! 双击粘贴/ASR 粘贴时用缓存的 pid 定向发送，避免切窗口后粘错。
//!
//! 平台策略：
//! - macOS：`save_frontmost_pid` 在操作开始时缓存前台 app 的 pid + bundle_id；
//!   `simulate_paste` / ASR paste 时用 `cached_pid()` 走 `paste_to_pid` 定向发送。
//!   无缓存时 fallback 到实时 `frontmost_app()` 检测（兼容旧逻辑）。
//! - Windows：SetWinEventHook + SetForegroundWindow + enigo Shift+Insert（Task 2）
//! - Linux：X11 focus event + XRaiseWindow + enigo Shift+Insert（Task 3）

use std::sync::Mutex;

/// 缓存的前台 app：(pid, bundle_id)。操作开始时缓存，粘贴时用。
/// 过滤自身（octopus）——缓存的是用户真正要粘贴到的目标 app。
static CACHED_PREV: Mutex<Option<(i32, String)>> = Mutex::new(None);

/// 缓存当前前台 app 的 pid + bundle_id（过滤自身）。
/// 在操作开始时调（如 ASR toggle 入口、剪贴板浮窗 show 前）。
#[cfg(target_os = "macos")]
pub fn save_frontmost_pid() {
    let frontmost = crate::platform::app_context::macos_ax::frontmost_app();
    if let Some((pid, bid, name)) = frontmost {
        // 过滤自身（octopus-desktop / octopus）
    if name != "octopus" && name != "octopus-desktop" && !name.starts_with("osascript") {
            let bundle = bid.unwrap_or_default();
            log::info!("[focus] cached frontmost: pid={} bundle={} name={}", pid, bundle, name);
            *CACHED_PREV.lock().unwrap() = Some((pid, bundle));
        } else {
            log::debug!("[focus] frontmost is self ({}), skip caching", name);
        }
    } else {
        log::debug!("[focus] frontmost_app() returned None, nothing to cache");
    }
}

/// 读缓存的 pid。粘贴时用（paste_to_pid 定向发送）。
pub fn cached_pid() -> Option<i32> {
    CACHED_PREV.lock().unwrap().as_ref().map(|(pid, _)| *pid)
}

/// 读缓存的 bundle_id。dispatch 路径选择用（如 WKWebView osascript fallback）。
pub fn cached_bundle_id() -> Option<String> {
    CACHED_PREV.lock().unwrap().as_ref().map(|(_, bid)| bid.clone())
}

/// 清理缓存。粘贴完成后调（下次操作重新缓存）。
pub fn clear_cached_pid() {
    *CACHED_PREV.lock().unwrap() = None;
}

pub struct FocusTracker;

/// simulate_copy 实际走的 dispatch 路径。调用方据此调整后续行为
/// （如 changeCount polling 超时——osascript 路径需要等 ~200ms+ WKWebView 回写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyDispatch {
    /// WKWebView 嵌套 app（微信等）→ osascript。慢路径（~200ms osascript + 异步回写剪贴板）。
    Osascript,
    /// 原生/Electron app → CGEventPostToPid。快路径（< 5ms，changeCount 通常 < 50ms 递增）。
    CGEvent,
}

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

    /// 模拟复制按键（Cmd+C / Ctrl+C）。返回实际走的 dispatch 路径。
    #[cfg(target_os = "macos")]
    pub fn simulate_copy(&self) -> CopyDispatch {
        simulate_copy_platform()
    }

    /// 非 macOS 平台暂未实现 dispatch 路径区分，统一返回 CGEvent 语义。
    #[cfg(not(target_os = "macos"))]
    pub fn simulate_copy(&self) {
        simulate_copy_platform()
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
    // 2026-07-21：osascript set frontmost 走 System Events 完整路径，触发 windowDidBecomeKey。
    // NSRunningApplication.activate 不触发 windowDidBecomeKey（app 级 ≠ window 级 key），
    // 故保留 osascript。
    //
    // 优化：去掉单独的「读 frontmost name」osascript（~200ms 纯日志用途），
    // 把 name 读取合并到 set frontmost 的同一个 osascript 脚本里（已有 return name of p）。
    // 省 ~200ms（两次 osascript → 一次）。
    use std::process::Command;

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
            set frontmost of p to true
        end if
        return name of p
    end tell"#;
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            log::info!("restore_focus: activated '{}'", name);
        }
        Err(e) => log::warn!("restore_focus: osascript failed: {}", e),
    }
}

#[cfg(target_os = "macos")]
fn simulate_paste_platform() {
    // 2026-07-20 perf：原 osascript（~200ms 启动）改用 keystroke 模块（CGEvent < 5ms）。
    // IME guard 仍保留——切到 ASCII 输入源再 paste，避免 CJK IME composing 状态下粘贴乱码。
    let _ime_guard = crate::platform::input_source::switch_to_ascii_for_paste();

    // 切输入源可能短暂抢焦点（Carbon TIS API），等 50ms 让焦点稳定再发 keystroke
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 2026-07-31：优先用缓存的 pid（预探测目标窗口），避免浮窗打开期间切窗口粘错。
    // 无缓存时 fallback 到实时 frontmost_app() 检测（兼容旧逻辑）。
    if let Some(pid) = cached_pid() {
        let bid = cached_bundle_id();
        if crate::platform::keystroke::needs_osascript_fallback(bid.as_deref()) {
            log::info!("simulate_paste: cached WKWebView app {:?} → osascript", bid);
            if let Err(e) = crate::platform::keystroke::paste_via_osascript() {
                log::warn!("simulate_paste (cached osascript): {}", e);
            }
        } else {
            log::info!("simulate_paste: cached pid={}", pid);
            if let Err(e) = crate::platform::keystroke::paste_to_pid(pid) {
                log::warn!("simulate_paste (cached pid): {}", e);
            }
        }
        // 用完即清（下次操作重新缓存）
        clear_cached_pid();
        return;
    }

    // 2026-07-21 三级 dispatch（无缓存时的 fallback）：
    //   1. WKWebView 嵌套 app（微信内置浏览器）→ osascript（~200ms，走 System Events 菜单路由）
    //   2. Electron app（豆包/ZCode）→ CGEventPostToPid（定向，< 5ms）
    //   3. 原生 app → CGEventPostToPid（或全局 post fallback）
    let frontmost = crate::platform::app_context::macos_ax::frontmost_app();
    let result = dispatch_paste(frontmost);
    if let Err(e) = result {
        log::warn!("simulate_paste: {}", e);
    }
}

#[cfg(target_os = "macos")]
fn simulate_copy_platform() -> CopyDispatch {
    // 2026-07-20 perf：原 osascript（~200ms 启动 + delay 0.15）改用 keystroke 模块（CGEvent < 5ms）。
    // 2026-07-21 三级 dispatch（跟 simulate_paste 同策略）：
    //   1. WKWebView 嵌套 app → osascript
    //   2. Electron app → CGEventPostToPid
    //   3. 原生 app → CGEventPostToPid
    //
    // 2026-07-21 fix：返回 dispatch 路径给调用方，让 detect_selection 据此选 polling 超时
    // （osascript 路径需要等 ~200ms+ WKWebView 回写，CGEvent 路径 80ms 足够）。
    let frontmost = crate::platform::app_context::macos_ax::frontmost_app();
    let (result, dispatch) = dispatch_copy(frontmost);
    if let Err(e) = result {
        log::warn!("simulate_copy: {} (dispatch={:?})", e, dispatch);
    }
    dispatch
}

/// 三级 dispatch：根据 frontmost app 类型选最佳按键发送方式。
///
/// 调用方先读 `frontmost_app()`（一次 IPC），传入这里复用，避免重复调用。
/// 返回 `(result, dispatch_path)`——dispatch_path 让调用方调整后续行为（如 polling 超时）。
#[cfg(target_os = "macos")]
fn dispatch_copy(
    frontmost: Option<(i32, Option<String>, String)>,
) -> (anyhow::Result<()>, CopyDispatch) {
    match &frontmost {
        Some((_, bid, _)) if crate::platform::keystroke::needs_osascript_fallback(bid.as_deref()) => {
            log::info!("simulate_copy: WKWebView app {:?} → osascript", bid);
            (crate::platform::keystroke::copy_via_osascript(), CopyDispatch::Osascript)
        }
        Some((pid, _, _)) => {
            (crate::platform::keystroke::copy_to_pid(*pid), CopyDispatch::CGEvent)
        }
        None => (crate::platform::keystroke::copy(), CopyDispatch::CGEvent),
    }
}

#[cfg(target_os = "macos")]
fn dispatch_paste(frontmost: Option<(i32, Option<String>, String)>) -> anyhow::Result<()> {
    match &frontmost {
        Some((_, bid, _)) if crate::platform::keystroke::needs_osascript_fallback(bid.as_deref()) => {
            log::info!("simulate_paste: WKWebView app {:?} → osascript", bid);
            crate::platform::keystroke::paste_via_osascript()
        }
        Some((pid, _, _)) => crate::platform::keystroke::paste_to_pid(*pid),
        None => crate::platform::keystroke::paste(),
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
