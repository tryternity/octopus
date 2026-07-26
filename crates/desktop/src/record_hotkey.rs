//! 录屏全局快捷键——`Cmd+Shift+R`（toggle，可配置）+ `Esc`（stop，固定）。
//!
//! 与 `action_hotkey.rs` 同模式（`tauri_plugin_global_shortcut`），关键差异：
//! - **toggle 启动时注册**：从 config 读 `record_shortcut`，main.rs setup + settings 热重载
//! - **ESC 按需注册**：录制开始时 `register_stop_hotkey`，结束 `unregister_stop_hotkey`
//! - **handler 内 spawn async**：`RecordSession` 内部 tokio Mutex，state 查询 / pause /
//!   resume / stop 全 async，同步 closure 必须包 `tauri::async_runtime::spawn`
//!
//! ## ESC 生命周期（2026-07-26 修复）
//!
//! 旧实现启动时一次性注册 ESC 并常驻——但全局快捷键在系统层吞掉事件，导致 Screenshot /
//! RecordConfig / VaultPicker 等**所有**窗口的 DOM 级 ESC 监听器收不到事件。
//!
//! 现改为按需注册：
//! - **非录制态**：ESC 不注册，完全由各窗口 DOM 层处理（Screenshot 取消截图、modal 关闭等）
//! - **录制中（Recording/Paused）**：`start_with_config` 成功后 register；ESC 触发 stop 入库
//! - **录制结束**：`stop_and_store` 成功 / `record_kill` 后 unregister
//!
//! RecordAnnotation 录制中依赖全局 ESC 停止录制（`RecordAnnotation/index.tsx` 的 DOM 监听
//! 先退标注工具，tool=none 后再让全局 ESC 接管停止录制）——方案下"录制中 register"满足该依赖。
//!
//! **仅 macOS**：模块整体 `cfg(target_os = "macos")`，与 record_commands 对齐。

#![cfg(target_os = "macos")]

use octopus_record::SessionState;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 录屏停止快捷键固定为 Escape（octopus 全局通用停止键，不暴露为可配置项）。
/// **按需注册**——仅录制中生效，避免吞掉其他窗口的 DOM 级 ESC。
const STOP_SHORTCUT: &str = "Escape";

/// 注册录屏 toggle 快捷键（仅 toggle，不动 ESC）。
///
/// 启动 + 热重载调：
/// - main.rs setup：从 config 读 record_shortcut 注册
/// - settings_commands::set_config：用户改快捷键时重注册
///
/// 失败返 Err(String) 让调用方回滚（与 screenshot_commands::register_screenshot_shortcut 同范式）。
///
/// `toggle_sc`：idle→弹浮窗 / recording→pause / paused→resume（用户可配置）。
/// stop（ESC）由 `register_stop_hotkey` 单独管理，启动时不注册。
pub fn register_toggle_hotkey(app: &AppHandle, toggle_sc: &str) -> Result<(), String> {
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
    log::info!("[record-hotkey] toggle='{}' 已注册（ESC stop 按需注册）", toggle_sc);
    Ok(())
}

/// 注册 ESC stop 快捷键（仅 ESC，不动 toggle）。
///
/// 录制开始（`record_commands::start_with_config` 成功，进入 Recording）时调。
/// 非录制态不注册——避免全局快捷键在系统层吞掉 Screenshot/RecordConfig 等 DOM 级 ESC。
///
/// 重复调用安全：`on_shortcut` 对同一快捷键会覆盖旧 handler，不会叠加。
pub fn register_stop_hotkey(app: &AppHandle) -> Result<(), String> {
    let stop: Shortcut = STOP_SHORTCUT
        .parse()
        .map_err(|e| format!("parse record stop '{}': {}", STOP_SHORTCUT, e))?;
    let app_clone = app.clone();
    app.global_shortcut()
        .on_shortcut(stop, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[record-hotkey] ESC stop 触发");
            let app = app_clone.clone();
            tauri::async_runtime::spawn(async move {
                handle_stop(&app).await;
            });
        })
        .map_err(|e| format!("register record stop '{}': {}", STOP_SHORTCUT, e))?;
    log::info!("[record-hotkey] ESC stop 已注册（录制中）");
    Ok(())
}

/// 注销 ESC stop 快捷键。
///
/// 录制结束（`stop_and_store` 成功回到 Idle / `record_kill` 强杀）时调。
/// 释放全局 ESC，让其他窗口（Screenshot 取消截图等）的 DOM 级 ESC 重新生效。
///
/// 失败仅 warn 不阻断——录制停止本身不受影响，最坏情况是 ESC 残留（下次 start 会覆盖）。
/// 未注册时 unregister 是 no-op，不会报错。
pub fn unregister_stop_hotkey(app: &AppHandle) {
    match STOP_SHORTCUT.parse::<Shortcut>() {
        Ok(sc) => {
            if let Err(e) = app.global_shortcut().unregister(sc) {
                log::warn!("[record-hotkey] ESC stop 注销失败（不影响功能）: {e}");
            } else {
                log::info!("[record-hotkey] ESC stop 已注销（录制结束）");
            }
        }
        Err(e) => log::warn!("[record-hotkey] ESC stop 解析失败（无法注销）: {e}"),
    }
}

/// toggle 处理：按当前 state 分支。
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
            match crate::record_commands::stop_and_store(&session, app, false, None).await {
                Ok(Some(meta)) => {
                    log::info!(
                        "[record-hotkey] 录制已停止入库: id={} file={}",
                        meta.id,
                        meta.file_path
                    );
                    // 关闭标注 overlay（Source::Area 录制时才有）
                    crate::record_annotation_window::close_annotation_window(app);
                    // 关闭控制浮窗（Source::Display/Window 录制时才有）
                    crate::record_control_window::close_control_window(app);
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
        // Idle / Starting / Stopping：handler 不做事。
        // 注意：ESC 全局快捷键只在 Recording/Paused 时才注册（register_stop_hotkey），
        // 理论上不会到这里。但保留分支作为防御性兜底。
        _ => {
            log::info!("[record-hotkey] Esc 在非录制态忽略（state={:?}）", state);
        }
    }
}
