//! Cancel / Discard 出口（从 coordinator/mod.rs 提取，Task 1.5）。
//!
//! - `handle_cancel`：Esc 取消——停录音 + 清 DB 脏数据（已 INSERT 的删除）+ 回 Idle。
//! - `handle_discard`：工具栏「关闭」——停录音 + finalize DB（保留识别历史，不粘贴）。

use crate::engine::audio::SharedAudioState;
use crate::core::config::AppConfig;
use crate::core::db_queue::{DbCommand, get_db_sender};
use crate::engine::transcript::Transcript;
use log::{debug, info, warn};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use super::Stage;
use super::agent::agent_task_id_in_stage;
use super::paste::{active_llm_name, now_millis};
use super::INSTANT_MODE;

/// 处理 Cancel 命令
pub(crate) fn handle_cancel(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    app_handle: &tauri::AppHandle,
) {
    match stage {
        Stage::Streaming {
            pipeline,
            streaming_active,
            ..
        } => {
            info!("Cancel: stopping streaming");
            streaming_active.store(false, Ordering::Relaxed);
            pipeline.reset();
            let _ = audio.stop();
        }
        Stage::VadSegmented {
            tick_active, ..
        } => {
            info!("Cancel: stopping VadSegmented");
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
        }
        Stage::WaitingCompletion { tick_active, .. } => {
            info!("Cancel: cancelling while waiting for transcription");
            // 2c-3：WaitingCompletion 现持 tick_active（VadSegmented move 过来），必须停；
            // 识别结果将被忽略，回到 Idle
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
        }
        Stage::Polishing { .. } => {
            info!("Cancel: cancelling while final polishing");
            // 润色结果将被忽略，回到 Idle
        }
        Stage::StoppingPolish { .. } => {
            info!("Cancel: cancelling while waiting for polish");
            // 立即润色结果将被忽略，回到 Idle
        }
        _ => {}
    }
    // 清理 agent task（AgentBridge 被 cancel 时标 failed）
    if let Some(tid) = agent_task_id_in_stage(stage) {
        let _ = octopus_infra::db::update_agent_task_status(&tid, "failed", "用户取消录音");
    }
    // 清理 DB 脏数据（审查 Issue 6）：Cancel = 丢弃，已 INSERT 的记录需删除
    let db_id_to_delete: Option<i64> = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. }
        | Stage::StoppingPolish { transcript, .. } => {
            if transcript.db_inserted() { Some(transcript.id) } else { None }
        }
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => {
            if transcript.db_inserted() { Some(transcript.id) } else { None }
        }
        Stage::Polishing { id, .. } | Stage::Pasting { id, .. } => Some(*id),
        _ => None,
    };
    if let Some(id) = db_id_to_delete {
        if let Err(e) = get_db_sender().send(DbCommand::Delete { id }) {
            warn!("Cancel: failed to queue DB delete for id={}: {}", id, e);
        } else {
            info!("Cancel: deleting abandoned DB record id={}", id);
        }
    }
    *stage = Stage::Idle;
    if INSTANT_MODE.swap(false, Ordering::Relaxed) {
        crate::ui::instant_overlay::hide_instant_overlay(app_handle);
    }
    crate::ui::result_window::hide_result(app_handle);
    crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
}
/// handle_discard 从当前 stage 提取的 DB finalize 数据。
/// （用 struct 而非 tuple，避免 clippy::type_complexity 且字段意义明确）
pub(super) struct DiscardDbInfo {
    pub id: i64,
    pub raw_text: String,
    pub segments: String,
    pub polished_text: Option<String>,
    pub polish_status: String,
    pub polish_model: Option<String>,
}

/// 处理 Discard 命令：停止录音 + finalize DB 记录（保留识别历史），
/// 但**不粘贴、不入剪贴板**。与 Cancel 的区别：Cancel 不 finalize DB。
/// 工具栏「关闭」按钮触发。
pub(crate) fn handle_discard(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    app_handle: &tauri::AppHandle,
    _config: &AppConfig,
) {
    // Pasting 阶段粘贴已在进行（enigo Cmd+V 已发或正发），无法撤回 → no-op
    if matches!(stage, Stage::Pasting { .. }) {
        debug!("Discard ignored during Pasting (paste in flight)");
        return;
    }

    // 清理 agent task（AgentBridge 被 discard 时标 failed）
    if let Some(tid) = agent_task_id_in_stage(stage) {
        let _ = octopus_infra::db::update_agent_task_status(&tid, "failed", "用户放弃录音");
    }

    // 从 transcript 提取 (polished_text, polish_status, polish_model) for Finalize：
    //   段模型下「已润色覆盖全部」= 无 Raw 段且非空 → 入库 "done" + finish_text；否则 "off"。
    // 修复：原版硬编码 None / "off"，把已完成的立即润色结果擦掉
    // （用户场景：立即润色→PolishDone 入库→点关闭→Finalize 覆盖 polished=None）。
    let polished_info = |t: &Transcript| -> (Option<String>, String, Option<String>) {
        let text = t.finish_text();
        if !text.is_empty() && !t.has_raw() {
            (Some(text), "done".to_string(), Some(active_llm_name()))
        } else {
            (None, "off".to_string(), None)
        }
    };

    // 从当前 stage 提取 DiscardDbInfo
    let db_info: Option<DiscardDbInfo> = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => {
            let (p, s, m) = polished_info(transcript);
            Some(DiscardDbInfo {
                id: transcript.id,
                raw_text: transcript.db_text(),
                segments: transcript.segments_json(),
                polished_text: p,
                polish_status: s,
                polish_model: m,
            })
        }
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => {
            let (p, s, m) = polished_info(transcript);
            Some(DiscardDbInfo {
                id: transcript.id,
                raw_text: transcript.db_text(),
                segments: transcript.segments_json(),
                polished_text: p,
                polish_status: s,
                polish_model: m,
            })
        }
        Stage::Polishing { id, raw_text, .. } => {
            // 最终润色中（非立即润色路径）：polished 尚未产出；无 transcript 引用，segments 空
            Some(DiscardDbInfo {
                id: *id,
                raw_text: raw_text.clone(),
                segments: "[]".to_string(),
                polished_text: None,
                polish_status: "off".to_string(),
                polish_model: None,
            })
        }
        Stage::StoppingPolish { transcript } => {
            let (p, s, m) = polished_info(transcript);
            Some(DiscardDbInfo {
                id: transcript.id,
                raw_text: transcript.db_text(),
                segments: transcript.segments_json(),
                polished_text: p,
                polish_status: s,
                polish_model: m,
            })
        }
        Stage::Idle => None,
        // Pasting 已在上面 early return
        Stage::Pasting { .. } => { log::error!("unexpected Pasting stage in discard result, ignoring"); None }
    };

    // 停止录音 + 引擎（与 handle_cancel 一致的停止逻辑）
    match stage {
        Stage::Streaming {
            pipeline,
            streaming_active,
            ..
        } => {
            info!("Discard: stopping streaming");
            streaming_active.store(false, Ordering::Relaxed);
            pipeline.reset();
            let _ = audio.stop();
        }
        Stage::VadSegmented { tick_active, .. } => {
            info!("Discard: stopping VadSegmented");
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
        }
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { .. } => {
            // session 已在 stop 路径移交给 close_async 任务、audio 已停。
            // 这里不粘贴：stage 即将落 Idle，close 完成后到达的
            // CloudStreamingDone 会被 handle_cloud_streaming_done 的非 CloudClosing
            // 分支忽略（honoring Discard）。close_async 自身仍会正常收尾释放 WS。
            info!("Discard: cloud close in flight, pending CloudStreamingDone will be ignored");
        }
        Stage::WaitingCompletion { tick_active, .. } => {
            info!("Discard: discarding while waiting for transcription");
            // 2c-3：WaitingCompletion 现持 tick_active（VadSegmented move 过来），必须停
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
        }
        Stage::Polishing { .. } => {
            info!("Discard: discarding while final polishing");
        }
        Stage::StoppingPolish { .. } => {
            info!("Discard: discarding while waiting for polish");
        }
        Stage::Idle => {}
        Stage::Pasting { .. } => { log::error!("unexpected Pasting stage in discard result, ignoring"); }
    }

    // finalize DB 记录（保留识别历史 + 已完成的润色结果，duration_ms 标记实际用时）
    if let Some(info) = db_info {
        if info.id > 0 {
            let duration_ms = now_millis() - info.id;
            let cmd = DbCommand::Finalize {
                id: info.id,
                raw_text: info.raw_text,
                segments: info.segments,
                polished_text: info.polished_text,
                polish_status: info.polish_status,
                polish_model: info.polish_model,
                duration_ms: Some(duration_ms),
            };
            if let Err(e) = get_db_sender().send(cmd) {
                warn!("Queue DB finalize (discard) failed: {}", e);
            }
        }
    }

    *stage = Stage::Idle;
    if INSTANT_MODE.swap(false, Ordering::Relaxed) {
        crate::ui::instant_overlay::hide_instant_overlay(app_handle);
    }
    crate::ui::result_window::hide_result(app_handle);
    crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
}
