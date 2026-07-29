//! 编辑态处理（从 coordinator/mod.rs 提取，Task 1.2）。
//!
//! 用户进入编辑态（暂停 ASR）+ 提交编辑（劈段落库）的 handler。
//! 活跃会话走 transcript；Idle 态无 transcript，用 CURRENT_TRANSCRIPTION_ID 直接 UPDATE。

use crate::config::PolishMode;
use crate::db_queue::{DbCommand, get_db_sender};
use crate::engine::transcript::Transcript;
use log::{debug, info, warn};
use std::sync::atomic::Ordering;
use super::{
    Stage, RecordType, CURRENT_TRANSCRIPTION_ID,
};
// stage_name / now_millis 等通用工具在 paste 子模块，通过 mod.rs re-export 可见。
use super::paste::stage_name;

/// 进入编辑态：活跃会话（Streaming/VadSegmented/WaitingCompletion/CloudClosing）或 Idle。
/// 活跃态初始化 edit_buffer = 当前 display；Idle 态无 transcript，仅置 editing=true（见下注）。
pub(crate) fn handle_enter_edit_mode(stage: &mut Stage, editing: &mut bool, edit_buffer: &mut Option<String>) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        Stage::Idle => {
            // Idle 编辑：会话已 finalize，stage 无 transcript，但 Result 窗口仍展示最近会话文本，
            // 用户可对其修订。允许进入（editing=true）；edit_buffer 不在此初始化。
            // 提交走 commit_edit_apply 的 Idle 分支，用 CURRENT_TRANSCRIPTION_ID 直接 UPDATE 落库。
            *editing = true;
            crate::perf_log::log("[STATE] enter_edit stage=Idle transcript_id=—");
            info!("Entered edit mode in Idle (no active transcript)");
            return;
        }
        _ => {
            debug!("enter_edit_mode ignored in non-active stage");
            return;
        }
    };
    let transcript_id = transcript.id;
    *editing = true;
    *edit_buffer = Some(transcript.display_text());
    crate::perf_log::log(&format!(
        "[STATE] enter_edit stage={} transcript_id={}",
        stage_name(stage), transcript_id,
    ));
    info!("Entered edit mode (transcript id={})", transcript_id);
}

/// 提交编辑：写回 transcript（commit_edit + dirty ranges 劈段）+ 光标/选区恢复 + UPDATE edited_text + 刷新展示。
/// 活跃态走 transcript；Idle 态无 transcript，用 CURRENT_TRANSCRIPTION_ID 直接 UPDATE。
pub(crate) fn commit_edit_apply(
    stage: &mut Stage,
    text: &str,
    dirty_ranges: &[(usize, usize)],
    has_edited: bool,
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
    app_handle: &tauri::AppHandle,
) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        Stage::Idle => {
            // Idle 编辑落库：会话已 finalize，stage 无 transcript，用最近会话 id 直接 UPDATE。
            // 复用 Transcript::commit_edit 构造单 Edited 段，与活跃态落库语义一致（整篇压成 Edited）。
            // 注：id 来自静态 CURRENT_TRANSCRIPTION_ID——会话 finalize 后仍保留最近有效 id
            // （mem::replace 的 id=0 sentinel 不清此静态），供 Idle 编辑溯源。
            let id = CURRENT_TRANSCRIPTION_ID.load(Ordering::Relaxed);
            if id <= 0 {
                debug!("commit_edit in Idle but no current_transcription_id — 跳过落库");
                return;
            }
            let mut t = Transcript::new(id, PolishMode::Disabled, RecordType::Input);
            // 从 DB 恢复已有 segments（保留 Raw/Polished/Edited 标记，
            // 否则 rebuild_segments 的 old_segments 为空 → clean 区域全退化为 Raw）
            let db_json: Option<String> = octopus_infra::db::with_db(|conn| {
                Ok(conn.query_row(
                    "SELECT segments FROM clipboard_history WHERE id = ?1",
                    [&id],
                    |row| row.get::<_, Option<String>>(0),
                ).unwrap_or(None))
            }).unwrap_or(None);
            if let Some(ref json) = db_json {
                if !json.is_empty() && json != "[]" {
                    t.restore_segments(json);
                }
            }
            t.commit_edit(text, dirty_ranges, has_edited);
            if let Some((s, e)) = selection { t.set_selection(s, e); }
            else if let Some(c) = caret { t.set_caret(c); }
            let segments = t.segments_json();
            if let Err(e) = get_db_sender().send(DbCommand::UpdateEditedSegments {
                id,
                text: text.to_string(),
                segments,
            }) {
                warn!("Queue DB UpdateEditedSegments (idle) failed: {}", e);
            }
            crate::ui::result_window::update_result(app_handle, text, false, 0);
            info!("Edit committed in Idle (id={}, {} chars)", id, text.chars().count());
            return;
        }
        _ => {
            debug!("commit_edit ignored in non-active stage");
            return;
        }
    };
    transcript.commit_edit(text, dirty_ranges, has_edited);
    if let Some((s, e)) = selection { transcript.set_selection(s, e); }
    else if let Some(c) = caret { transcript.set_caret(c); }
    if transcript.db_inserted() {
        let id = transcript.id;
        let segments = transcript.segments_json();
        if let Err(e) = get_db_sender().send(DbCommand::UpdateEditedSegments {
            id,
            text: text.to_string(),
            segments,
        }) {
            warn!("Queue DB UpdateEditedSegments failed: {}", e);
        }
    }
    crate::ui::result_window::update_result(app_handle, &transcript.display_text(), false, 0);
    info!("Edit committed ({} chars)", text.chars().count());
}

/// 从 stage 中取出 transcript 的可变引用（用于 cancel edit 恢复展示）
pub(crate) fn stage_transcript(stage: &mut Stage) -> Option<&mut Transcript> {
    match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. }
        | Stage::StoppingPolish { transcript, .. } => Some(transcript),
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => Some(transcript),
        _ => None,
    }
}
