//! 终端窗口：独立 Tauri 窗口，原生标题栏，承载内嵌终端（xterm.js + PTY）。
//!
//! **多实例**（2026-07-31）：每次打开都创建一个新窗口（label `terminal_<n>`，
//! capabilities 用 `terminal_*` 通配授权），不存在单例。托盘「新建终端」每点一次
//! 开一个新窗口；ActionBar agent 分支同样每次新开。
//!
//! macOS：开窗切 Regular（Dock 显图标），所有终端窗口关闭后切回 Accessory，
//! 与 settings / compact_editor 对称（注册在 activation::REGULAR_WINDOWS 用通配判断）。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use tauri::{Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};

/// 终端窗口 label 前缀（实际 label = `terminal_<n>`，capabilities 用 `terminal_*`）。
pub const WINDOW_LABEL_PREFIX: &str = "terminal_";

/// 前端 listener 注册后向本窗口 emit 的事件名。
///
/// 新建 agent 窗口时 React mount 是异步的，`terminal://new-tab` listener 在首帧
/// 才注册。后端若固定 sleep(250ms) 后 emit，慢 mount（冷启动/老机器/debug build）
/// 会撞上未注册的 listener → 事件被 Tauri 静默丢弃 → 空终端。
///
/// 改为「前端 ready 回拉」：前端 listener 注册成功后 emit 本事件，后端收到才 emit
/// new-tab；带 READY_TIMEOUT 兜底，超时则强制 emit 一次（避免 ready 永不到时命令彻底丢）。
const READY_EVENT: &str = "terminal://ready";
/// 等前端 ready 的超时。超过则强制 emit new-tab 一次（前端可能仍在 mount，事件可能
/// 丢失，但比「命令彻底丢」强——至少快 mount 的场景有救）。
const READY_TIMEOUT_MS: u64 = 5000;

/// ActionBar 联动事件 payload（emit "terminal://new-tab"）。
/// 前端 listen 后新开 tab，cwd 作为 shell 启动目录，command 写入 shell（如 `claude`）。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTabPayload {
    /// shell 启动目录（None 用 home）。
    pub cwd: Option<String>,
    /// 要执行的命令（如 "claude" / "codex"）。前端写入 PTY 后回车。
    pub command: Option<String>,
}

/// 窗口 label 单调递增计数器（从 1 开始，不复用——关闭的窗口 label 不回收）。
static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(1);

const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 640.0;
const MIN_WIDTH: f64 = 560.0;
const MIN_HEIGHT: f64 = 360.0;

/// 构造终端窗口初始 URL（纯函数，便于单测）。
///
/// - `cwd`：注入为 `?cwd=<encoded>`，前端 mount 后读 query 传给 pty_open。
/// - `bg`：背景色 hex，注入为 `&bg=<hex>`，terminal.html 同步设置避免白屏。
///
/// 无 cwd 时不带 query；有 bg 时追加。
pub fn build_initial_url(cwd: Option<&str>, bg: Option<&str>) -> String {
    let mut url = String::from("terminal.html");
    let mut has_query = false;
    if let Some(c) = cwd.filter(|s| !s.is_empty()) {
        url.push_str("?cwd=");
        url.push_str(&urlencode(c));
        has_query = true;
    }
    if let Some(b) = bg {
        let sep = if has_query { "&" } else { "?" };
        url.push_str(sep);
        url.push_str("bg=");
        url.push_str(b);
    }
    url
}

/// 简易 percent-encoding（路径含中文/空格时安全拼入 URL query）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 新窗口场景：等前端 listener 注册（`terminal://ready`）后再 emit new-tab，避免
/// 慢 mount 时事件被 Tauri 静默丢弃。带 READY_TIMEOUT_MS 兜底——超时则强制 emit 一次。
///
/// `fired` 防重复 emit（ready 先到则取消 watchdog；watchdog 先到则忽略后续 ready）。
/// 用法：建窗后直接调此函数，主调线程立即返回（once 注册 + watchdog spawn 后）。
fn emit_new_tab_on_ready(app_handle: tauri::AppHandle, payload: NewTabPayload) {
    let label = AGENT_WINDOW_LABEL.to_string();
    let fired = Arc::new(AtomicBool::new(false));

    // once 收到前端 ready → emit new-tab（若 watchdog 未抢先）。
    // 每个闭包各 clone 一份 app/label/payload，避免借用冲突。
    let fired_once = fired.clone();
    let app_once = app_handle.clone();
    let label_once = label.clone();
    let payload_once = payload.clone();
    let _once_id = app_handle.once(READY_EVENT, move |_e| {
        if fired_once.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            let _ = app_once.emit_to(&label_once, "terminal://new-tab", payload_once);
        }
    });

    // watchdog：超时则强制 emit（若 ready 未到）。
    let fired_wd = fired.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(READY_TIMEOUT_MS));
        if fired_wd.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            log::warn!(
                "[terminal] agent 窗口 {READY_TIMEOUT_MS}ms 内未收到 ready，强制 emit new-tab（前端 mount 可能超时）"
            );
            let _ = app_handle.emit_to(&label, "terminal://new-tab", payload);
        }
    });
}


/// 分配一个新的终端窗口 label（`terminal_<递增id>`）。
fn alloc_label() -> String {
    let id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
    format!("{WINDOW_LABEL_PREFIX}{id}")
}

/// 打开一个新的终端窗口（**每次调用都创建新窗口**，非单例）。
///
/// `cwd`：可选初始工作目录，注入窗口 URL query。
pub fn open_terminal_window(app_handle: &tauri::AppHandle, cwd: Option<&str>) -> Result<(), String> {
    // macOS：新建终端窗口 → Dock 显图标 + 激活到前台
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app_handle.run_on_main_thread(|| {
            crate::platform::activation::activate_self();
            crate::ui::settings_window::set_dock_icon();
        });
    }

    let label = alloc_label();
    let bg = crate::ui::theme::window_bg_hex(WINDOW_LABEL_PREFIX);
    let url = build_initial_url(cwd, bg.as_deref());
    log::info!("[terminal] create window label={} url={}", label, url);

    let win = WebviewWindowBuilder::new(app_handle, &label, WebviewUrl::App(url.into()))
        .title("终端")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .decorations(true)
        .resizable(true)
        .visible(false)
        .center()
        .build();

    match win {
        Ok(w) => {
            let _ = w.show();
            let _ = w.set_focus();
            log::info!("[terminal] window {} created", label);
            Ok(())
        }
        Err(e) => {
            log::error!("[terminal] create window failed: {e}");
            #[cfg(target_os = "macos")]
            {
                restore_accessory_if_no_terminal_window(app_handle);
            }
            Err(e.to_string())
        }
    }
}

/// agent 专用终端窗口的固定 label（单例——ActionBar agent 命令复用同一窗口）。
///
/// 与托盘「新建终端」的多实例（`terminal_<n>`）不同：agent 命令期望确定性——
/// 每次执行 agent 都聚焦同一个窗口并在其中新开 tab，而非每次弹新窗口。
/// 存在则聚焦 + 新 tab（emit_to 定向，非全局广播）；不存在才建窗。
pub const AGENT_WINDOW_LABEL: &str = "terminal_action_agent";

/// 打开 agent 专用终端窗口（单例）并在其中运行指定命令。
///
/// - 窗口已存在 → 聚焦 + emit_to 定向 "terminal://new-tab" { cwd, command }（新 tab）
/// - 窗口不存在 → 建窗（cwd 注入 URL）+ 延迟 emit_to 推送 command（兜底 mount 时序）
///
/// 用 emit_to 定向到 AGENT_WINDOW_LABEL，避免全局广播让其他终端窗口也开 tab。
pub fn open_terminal_with_command(
    app_handle: &tauri::AppHandle,
    cwd: Option<&str>,
    command: &str,
) -> Result<(), String> {
    // 单例：窗口已存在 → 聚焦 + 定向 emit 新 tab
    if let Some(win) = app_handle.get_webview_window(AGENT_WINDOW_LABEL) {
        #[cfg(target_os = "macos")]
        {
            let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
            let ah = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                crate::platform::activation::activate_self();
                let _ = ah.get_webview_window(AGENT_WINDOW_LABEL).map(|w| w.set_focus());
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = win.set_focus();
        }
        let _ = win.show();
        // 定向 emit：只发给 agent 窗口，不广播到其他终端窗口
        let _ = app_handle.emit_to(
            AGENT_WINDOW_LABEL,
            "terminal://new-tab",
            NewTabPayload {
                cwd: cwd.map(|s| s.to_string()),
                command: Some(command.to_string()),
            },
        );
        log::info!(
            "[terminal] agent window reused, new tab cwd={:?} command={}",
            cwd, command
        );
        return Ok(());
    }

    // 窗口不存在 → 建窗（单例 label）
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app_handle.run_on_main_thread(|| {
            crate::platform::activation::activate_self();
            crate::ui::settings_window::set_dock_icon();
        });
    }

    let bg = crate::ui::theme::window_bg_hex(WINDOW_LABEL_PREFIX);
    let url = build_initial_url(cwd, bg.as_deref());
    log::info!("[terminal] create agent window label={} url={}", AGENT_WINDOW_LABEL, url);

    let win = WebviewWindowBuilder::new(app_handle, AGENT_WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("Agent")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .decorations(true)
        .resizable(true)
        .visible(false)
        .center()
        .build();

    match win {
        Ok(w) => {
            let _ = w.show();
            let _ = w.set_focus();
            log::info!("[terminal] agent window created");
            // 新窗口 React mount 是异步的，listen 在首帧才注册。固定 sleep(250ms) 在
            // 慢 mount 时会丢事件。改用「前端 ready 回拉」：等 listener 注册成功后
            // 前端 emit terminal://ready，本端 once 接收后再 emit new-tab；带
            // READY_TIMEOUT_MS 兜底（超时强制 emit，至少快 mount 场景有救）。
            emit_new_tab_on_ready(
                app_handle.clone(),
                NewTabPayload {
                    cwd: cwd.map(|s| s.to_string()),
                    command: Some(command.to_string()),
                },
            );
            Ok(())
        }
        Err(e) => {
            log::error!("[terminal] create agent window failed: {e}");
            #[cfg(target_os = "macos")]
            {
                restore_accessory_if_no_terminal_window(app_handle);
            }
            Err(e.to_string())
        }
    }
}

/// 判断某 label 是否是终端窗口（`terminal_*` 前缀 + agent 单例 `terminal_action_agent`）。
pub fn is_terminal_window(label: &str) -> bool {
    label.starts_with(WINDOW_LABEL_PREFIX) || label == AGENT_WINDOW_LABEL
}

/// macOS：某终端窗口关闭后，仅当无其他常规窗口（含终端）存活时切回 Accessory。
#[cfg(target_os = "macos")]
pub fn on_terminal_closed(app_handle: &tauri::AppHandle) {
    restore_accessory_if_no_terminal_window(app_handle);
}

/// 若无终端窗口 + 无其他常规窗口存活 → 切回 Accessory。
#[cfg(target_os = "macos")]
fn restore_accessory_if_no_terminal_window(app_handle: &tauri::AppHandle) {
    crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn restore_accessory_if_no_terminal_window(_app_handle: &tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_no_cwd_no_bg() {
        assert_eq!(build_initial_url(None, None), "terminal.html");
        assert_eq!(build_initial_url(Some(""), None), "terminal.html");
    }

    #[test]
    fn build_url_with_cwd() {
        let url = build_initial_url(Some("/home/user/proj"), None);
        assert_eq!(url, "terminal.html?cwd=/home/user/proj");
    }

    #[test]
    fn build_url_cwd_percent_encoded() {
        let url = build_initial_url(Some("/Users/测试/my project"), None);
        assert!(url.starts_with("terminal.html?cwd="));
        assert!(url.contains("%E6%B5%8B%E8%AF%95"), "中文应编码，got: {url}");
        assert!(url.contains("%20"), "空格应编码，got: {url}");
    }

    #[test]
    fn build_url_with_bg_only() {
        let url = build_initial_url(None, Some("1e1e2e"));
        assert_eq!(url, "terminal.html?bg=1e1e2e");
    }

    #[test]
    fn build_url_cwd_and_bg() {
        let url = build_initial_url(Some("/tmp"), Some("1e1e2e"));
        assert_eq!(url, "terminal.html?cwd=/tmp&bg=1e1e2e");
    }

    #[test]
    fn urlencode_preserves_unreserved() {
        assert_eq!(urlencode("abcXYZ09-._~/"), "abcXYZ09-._~/");
    }

    #[test]
    fn urlencode_encodes_special() {
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("a?b=c"), "a%3Fb%3Dc");
    }

    #[test]
    fn alloc_label_is_unique_and_prefixed() {
        let a = alloc_label();
        let b = alloc_label();
        assert!(a.starts_with(WINDOW_LABEL_PREFIX), "got {a}");
        assert!(b.starts_with(WINDOW_LABEL_PREFIX), "got {b}");
        assert_ne!(a, b, "labels must be unique");
    }

    #[test]
    fn is_terminal_window_matches_prefix() {
        // terminal_* 多实例
        assert!(is_terminal_window("terminal_1"));
        assert!(is_terminal_window("terminal_42"));
        // agent 单例窗口
        assert!(is_terminal_window("terminal_action_agent"));
        // 非终端窗口
        assert!(!is_terminal_window("settings_window"));
        assert!(!is_terminal_window("compact_editor_window"));
    }
}
