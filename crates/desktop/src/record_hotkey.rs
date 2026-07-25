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
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 注册录屏全局快捷键（参数化，支持 config-driven 热重载）。
///
/// 与 `screenshot_commands::register_screenshot_shortcut` 同范式：
/// - main.rs setup 启动注册 + settings_commands::set_config 热重载共用
/// - 函数签名接 `&str` 参数，失败返 `Err(String)` 让调用方回滚
///
/// `toggle_sc`：idle→弹浮窗 / recording→pause / paused→resume
/// `stop_sc`：仅 recording/paused 时停止入库
pub fn register_record_hotkeys(
    app: &AppHandle,
    toggle_sc: &str,
    stop_sc: &str,
) -> Result<(), String> {
    // ── toggle 快捷键 ─────────────────────────────────────────────
    let toggle: Shortcut = toggle_sc
        .parse()
        .map_err(|e| format!("parse record toggle '{}': {}", toggle_sc, e))?;
    let app_clone = app.clone();
    app.global_shortcut()
        .on_shortcut(toggle, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[record-hotkey] toggle 触发");
            let app = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                handle_toggle(&app).await;
            });
        })
        .map_err(|e| format!("register record toggle '{}': {}", toggle_sc, e))?;

    // ── stop 快捷键（仅 recording/paused 状态） ────────────────────
    let stop: Shortcut = stop_sc
        .parse()
        .map_err(|e| format!("parse record stop '{}': {}", stop_sc, e))?;
    let app_clone = app.clone();
    app.global_shortcut()
        .on_shortcut(stop, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[record-hotkey] stop 触发");
            let app = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                handle_stop(&app).await;
            });
        })
        .map_err(|e| format!("register record stop '{}': {}", stop_sc, e))?;

    log::info!("[record-hotkey] 已注册 toggle='{}' stop='{}'", toggle_sc, stop_sc);
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
            // 用户决策（2026-07-25 修订）：Cmd+Shift+R 弹出配置浮窗让用户选源（display/window/area）。
            // 之前一版是「直接用默认配置开录」，但用户反馈需要选具体显示器/窗口/区域，
            // 所以改成浮窗交互（spec §8.1 配置浮窗，record_window.rs + RecordConfig.tsx）。
            //
            // 录制中再按 Cmd+Shift+R 走 pause/resume toggle（下方分支）；Esc 停止（handle_stop）。
            crate::record_window::show_record_window(app);
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
/// 调 `record_commands::stop_and_store`——读 session.last_start_request 拿 start 时
/// 的 recording_id / source / video / audio 字段，组装 RecordingMeta 入库。
/// 这样 hotkey/tray/前端 record_stop 三条路径都走同一个入库逻辑。
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
            log::info!("[record-hotkey] Esc → stop + 入库（state={:?}）", state);
            match crate::record_commands::stop_and_store(&session, false, None).await {
                Ok(Some(meta)) => {
                    log::info!(
                        "[record-hotkey] 录制已停止入库: id={} file={}",
                        meta.id,
                        meta.file_path
                    );
                    // 通知前端刷新历史列表
                    let _ = app.emit("record://stopped", &meta);
                }
                Ok(None) => log::info!("[record-hotkey] stop 返回 None（discard？）"),
                Err(e) => {
                    log::error!("[record-hotkey] stop + 入库失败: {e}");
                    let _ = app.emit("record://stop-failed", &e);
                }
            }
        }
        // Idle / Starting / Stopping：忽略（Esc 让给其他用途）
        _ => {
            log::info!("[record-hotkey] Esc 在非录制态忽略（state={:?}）", state);
        }
    }
}
