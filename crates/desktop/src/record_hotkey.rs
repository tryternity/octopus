//! 录屏全局快捷键——注册 `Cmd+Shift+R`（toggle）+ `Esc`（stop）。
//!
//! 与 `action_hotkey.rs` 同模式（用 `tauri_plugin_global_shortcut`），区别：
//! - **硬编码两个快捷键**：不从 DB 读，启动时直接注册。
//! - **handler 内 spawn async**：`RecordSession` 内部 tokio Mutex，state 查询 / pause /
//!   resume / stop 全 async，handler 是同步 closure 必须包 `tauri::async_runtime::spawn`。
//!
//! toggle 语义（按当前 SessionState 分支）：
//! - `Idle` / `Starting` → 打开 Settings 跳转 recordings panel（MVP 没有独立配置浮窗，
//!   主会话决策：呼出 Settings 录屏页让用户配置后点开始）。
//! - `Recording` → pause。
//! - `Paused` → resume。
//! - `Stopping` → 忽略（停止过渡态不可打断）。
//!
//! Esc 语义：仅 `Recording` / `Paused` 时执行 stop（discard=false，正常入库），
//! 其他状态忽略——让 Esc 留给其他用途（CompactEditor 关闭、设置取消等）。
//!
//! **仅 macOS**：模块整体 `cfg(target_os = "macos")`，与 record_commands 对齐。

#![cfg(target_os = "macos")]

use octopus_record::SessionState;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 注册录屏全局快捷键（硬编码 Cmd+Shift+R + Esc）。
///
/// 与 `register_action_hotkeys` 不同：不维护 REGISTERED_SHORTCUTS 集合——本模块注册的
/// 两个快捷键是常量，重启时整体重新注册即可（global_shortcut plugin 用相同 shortcut
/// 再 on_shortcut 会覆盖，不会泄漏）。
pub fn register_record_hotkeys(app: &AppHandle) -> Result<(), String> {
    // ── Cmd+Shift+R：toggle ──────────────────────────────────────
    let toggle_sc: Shortcut = "CommandOrControl+Shift+R"
        .parse()
        .map_err(|e| format!("parse Cmd+Shift+R: {e}"))?;
    let app_clone = app.clone();
    app.global_shortcut()
        .on_shortcut(toggle_sc, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[record-hotkey] Cmd+Shift+R 触发");
            let app = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                handle_toggle(&app).await;
            });
        })
        .map_err(|e| format!("register Cmd+Shift+R: {e}"))?;

    // ── Esc：stop（仅 recording/paused 状态） ──────────────────────
    let esc_sc: Shortcut = "Escape"
        .parse()
        .map_err(|e| format!("parse Escape: {e}"))?;
    let app_clone = app.clone();
    app.global_shortcut()
        .on_shortcut(esc_sc, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[record-hotkey] Esc 触发");
            let app = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                handle_stop(&app).await;
            });
        })
        .map_err(|e| format!("register Escape: {e}"))?;

    log::info!("[record-hotkey] 已注册 Cmd+Shift+R (toggle) + Esc (stop)");
    Ok(())
}

/// Cmd+Shift+R toggle 处理：按当前 state 分支。
async fn handle_toggle(app: &AppHandle) {
    let session = match app.try_state::<octopus_record::RecordSession>() {
        Some(s) => s,
        None => {
            log::warn!("[record-hotkey] RecordSession state 未找到（toggle 忽略）");
            return;
        }
    };

    let state = session.state().await;
    log::info!("[record-hotkey] toggle: current state = {:?}", state);

    match state {
        SessionState::Idle | SessionState::Starting => {
            // MVP 决策：呼出 Settings 录屏页（没独立配置浮窗，用户在 Settings 配好源/音视频再点开始）
            crate::settings_window::open_settings(
                app.clone(),
                Some("recordings".to_string()),
            );
        }
        SessionState::Recording => {
            if let Err(e) = session.pause().await {
                log::warn!("[record-hotkey] pause 失败: {}", e);
            }
        }
        SessionState::Paused => {
            if let Err(e) = session.resume().await {
                log::warn!("[record-hotkey] resume 失败: {}", e);
            }
        }
        SessionState::Stopping => {
            // 停止过渡态不可打断，忽略
            log::info!("[record-hotkey] toggle 在 Stopping 态忽略");
        }
    }
}

/// Esc stop 处理：仅 recording/paused 时执行，否则忽略。
///
/// **注意**：仅调 `RecordSession::stop()`（discard=false，文件入库）。但与
/// `record_commands::record_stop` 不同——后者会写 RecordingMeta 入库（width/height 等
/// 字段来自前端），此处 hotkey 路径不掌握这些字段，**只 send stop 命令给 helper**
/// 让其停止写文件 + 退出；DB 入库由前端监听 SessionState 转回 Idle 后调
/// `record_stop` 命令完成（或由后续 follow-up 引入统一 on-stop hook 补齐）。
///
/// 这是当前 MVP 的妥协：hotkey stop 仅保证 helper 进程干净退出 + .mp4 落盘，
/// 不入库。主会话已确认「hotkey stop 暂不入库」属 follow-up，不在 Task 14 范围。
async fn handle_stop(app: &AppHandle) {
    let session = match app.try_state::<octopus_record::RecordSession>() {
        Some(s) => s,
        None => {
            log::warn!("[record-hotkey] RecordSession state 未找到（stop 忽略）");
            return;
        }
    };

    let state = session.state().await;
    match state {
        SessionState::Recording | SessionState::Paused => {
            log::info!("[record-hotkey] Esc → stop（state={:?}）", state);
            if let Err(e) = session.stop().await {
                log::warn!("[record-hotkey] stop 失败: {}", e);
            }
        }
        // Idle / Starting / Stopping：忽略（Esc 让给其他用途）
        _ => {
            log::info!("[record-hotkey] Esc 在非录制态忽略（state={:?}）", state);
        }
    }
}
