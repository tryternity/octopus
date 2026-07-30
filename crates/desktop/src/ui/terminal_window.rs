//! 终端窗口：独立 Tauri 窗口，原生标题栏，承载内嵌终端（xterm.js + PTY）。
//!
//! 单例管理：已打开则 show + focus（并 emit `terminal://new-tab` 让前端新开 tab），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与
//! settings / compact_editor 对称（注册在 activation::REGULAR_WINDOWS）。
//!
//! 与 compact_editor_window 的差异：
//! - 无 tab URL 注入（前端 mount 后自建首个 tab，无需预置数据）
//! - 无窗口状态记忆（终端尺寸由前端 fitAddon 管理，不需 DB 持久化）
//! - `open_terminal_with_command`：ActionBar agent 分支用——开窗 + emit 命令，
//!   前端 listen 后新 tab + 写命令到 shell

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 终端窗口 label（capabilities/activation 都用这个名字）。
pub const WINDOW_LABEL: &str = "terminal_window";

const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 640.0;
const MIN_WIDTH: f64 = 560.0;
const MIN_HEIGHT: f64 = 360.0;

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
/// 仅对非保留字符做编码，与 shell_init 脚本里的 urlencode 语义一致。
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

/// 打开终端窗口（单例：已存在则 show + focus）。
///
/// `cwd`：可选初始工作目录。ActionBar 联动时传 agent 项目目录。
/// 返回 Ok(()) 表示窗口已就绪（新建或聚焦），Err 表示创建失败。
pub fn open_terminal_window(app_handle: &tauri::AppHandle, cwd: Option<&str>) -> Result<(), String> {
    // 单例：已存在则 show + focus +（若有 cwd）emit 让前端新开 tab
    if let Some(win) = app_handle.get_webview_window(WINDOW_LABEL) {
        #[cfg(target_os = "macos")]
        {
            let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
            let ah = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                crate::platform::activation::activate_self();
                let _ = ah.get_webview_window(WINDOW_LABEL).map(|w| w.set_focus());
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = win.set_focus();
        }
        let _ = win.show();
        // 已打开时，cwd 非空 → 通知前端新开 tab（而非替换当前 tab）
        if let Some(c) = cwd.filter(|s| !s.is_empty()) {
            let _ = app_handle.emit(
                "terminal://new-tab",
                NewTabPayload {
                    cwd: Some(c.to_string()),
                    command: None,
                },
            );
        }
        log::info!("[terminal] focused existing window");
        return Ok(());
    }

    // macOS：新建终端窗口 → Dock 显图标 + 激活到前台
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app_handle.run_on_main_thread(|| {
            crate::platform::activation::activate_self();
            crate::ui::settings_window::set_dock_icon();
        });
    }

    let bg = crate::ui::theme::window_bg_hex(WINDOW_LABEL);
    let url = build_initial_url(cwd, bg.as_deref());
    log::info!("[terminal] create window url={}", url);

    let win = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::App(url.into()))
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
            log::info!("[terminal] window created");
            Ok(())
        }
        Err(e) => {
            log::error!("[terminal] create window failed: {e}");
            // 创建失败时回退 Accessory（上面切了 Regular）
            #[cfg(target_os = "macos")]
            {
                crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
            }
            Err(e.to_string())
        }
    }
}

/// 打开终端窗口并在其中运行指定命令（ActionBar agent 分支用）。
///
/// 流程：open_terminal_window（新建或聚焦）→ emit "terminal://new-tab" { cwd, command }。
/// 前端 listen 后新开 tab，shell 启动后写入 command + 回车。
pub fn open_terminal_with_command(
    app_handle: &tauri::AppHandle,
    cwd: Option<&str>,
    command: &str,
) -> Result<(), String> {
    open_terminal_window(app_handle, cwd)?;
    let _ = app_handle.emit(
        "terminal://new-tab",
        NewTabPayload {
            cwd: cwd.map(|s| s.to_string()),
            command: Some(command.to_string()),
        },
    );
    log::info!(
        "[terminal] open_terminal_with_command cwd={:?} command={}",
        cwd, command
    );
    Ok(())
}

/// macOS：终端窗口关闭后，仅当无其他常规窗口存活时切回 Accessory。
#[cfg(target_os = "macos")]
pub fn on_terminal_closed(app_handle: &tauri::AppHandle) {
    crate::platform::activation::restore_accessory_if_no_regular_window(app_handle);
}

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
        // 中文 + 空格路径需 percent-encode
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
}
