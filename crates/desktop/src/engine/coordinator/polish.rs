//! 润色（polish）相关逻辑（从 coordinator/mod.rs 提取，Task 2.2）。
//!
//! 三类润色触发路径集中于此：
//! - **最终润色**：`start_final_polish_or_paste`（停止录音后）/ `handle_final_polish_done`
//!   （异步结果回传）—— polish_mode 决定是否润色，否则直接粘贴。
//! - **中间润色**：`check_and_trigger_polish`（tick 停顿/段边界触发，mode=2 only）/
//!   `handle_polish_done`（结果写回 transcript）/ `spawn_polish_thread`（LLM 异步）。
//! - **立即润色**：`handle_polish_now`（前端按钮，忽略 polish_mode）。
//! - `polish_input_to_regions`：transcript 段快照 → octopus_llm 多段润色输入（共用）。

use crate::core::config::AppConfig;
use crate::core::config::PolishMode;
use crate::core::db_queue::{DbCommand, get_db_sender};
use crate::engine::transcript::Transcript;
use log::{debug, info, warn};
use std::sync::mpsc::Sender;
use super::{Command, Stage, RecordType, MIN_POLISH_INTERVAL_SEC, set_recording_mode};
use super::paste::{
    do_paste, stage_name, active_llm_name, active_asr_engine_name, update_transcription_raw,
};
// finalize_after_stop 在 lifecycle.rs（Task 2.3 搬入）。
use super::lifecycle::finalize_after_stop;

/// 开始粘贴阶段（支持最终润色）。`transcript` 移交进 Pasting 持 id（Task 6 用）。
/// 开始最终润色或粘贴阶段（异步最终润色，防止阻塞协调器线程）。
pub(crate) fn start_final_polish_or_paste(
    stage: &mut Stage,
    text: &str,
    mut transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if text.is_empty() {
        *stage = Stage::Idle;
        crate::ui::result_window::hide_result(app_handle);
        crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
        set_recording_mode(0);  // 回 Idle
        return;
    }

    match crate::core::config::llm_config(config.polish_mode) {
        None => {
            // 无需润色，直接粘贴
            do_paste(
                stage,
                text,
                transcript.id,
                &transcript.db_text(),
                &transcript.segments_json(),
                "off",
                config,
                app_handle,
                tx,
            );
        }
        Some(llm_config) => {
            // 进入异步润色状态
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Processing);
            if super::INSTANT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                crate::ui::result_window::show_instant(app_handle, "polishing", "");
            } else {
                crate::ui::result_window::show_result(app_handle, "⏳ 最终润色中...");
            }

            let id = transcript.id;
            let raw_text = transcript.db_text();
            let segments = transcript.segments_json();
            // 段模型多段润色：Edited preserve，其余润色（与 spawn_polish_thread 共用转换）。
            let input = transcript.take_polish_input();
            let regions = polish_input_to_regions(&input);

            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
                segments: segments.clone(),
                // Part A 后 text = finish_text（段模型含 edited/raw 全部）或 raw-with-」，失败时 paste 它
                fallback_text: text.to_string(),
            };

            let tx = tx.clone();
            // 跨会话护栏：最终润色 1~3s 窗口内 Cancel+重开会话 → 新 Polishing。旧会话
            // 迟到的 FinalPolishDone 会匹配到新 Polishing，用新 id + 旧润色文本 do_paste
            // → 跨会话污染。带 session_id（= 本会话 transcript.id），handler 校验当前
            // polishing id 是否匹配，否则丢弃。
            let session_id = id;
            std::thread::spawn(move || {
                // catch_unwind 兜底：polish_regions 内部 panic（JSON 反序列化 / 网络库内部）
                // 会让线程静默死亡，FinalPolishDone 永不发送 → 永久卡在 Stage::Polishing
                // （该 stage 忽略所有快捷键与录音触发，需重启恢复）。捕获 panic 后发 Err，
                // coordinator 走与润色失败相同的降级路径（用 fallback_text 粘贴）。
                let inner = || match octopus_llm::polish_regions(&regions, &llm_config) {
                    Ok(polished) => {
                        if polished.is_empty() {
                            Err("Final polish returned empty".to_string())
                        } else {
                            Ok(polished)
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(inner)).unwrap_or_else(
                        |p| {
                            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = p.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "final polish panicked".to_string()
                            };
                            Err(format!("Final polish panicked: {}", msg))
                        },
                    );
                let _ = tx.send(Command::FinalPolishDone { result, session_id });
            });
        }
    }
}

/// 处理最终润色完成事件。
///
/// 跨会话护栏（与 PolishDone 同理）：最终润色 1~3s 窗口内 Cancel+重开会话 →
/// 新 Polishing。旧会话迟到的 FinalPolishDone 会匹配到新 Polishing，用新 id +
/// 旧润色文本 do_paste → 跨会话污染。session_id（= 发起润色时的 transcript.id）
/// 校验：与当前 polishing id 不符则丢弃，不动当前 stage。
pub(crate) fn handle_final_polish_done(
    stage: &mut Stage,
    result: Result<String, String>,
    session_id: i64,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let (id, raw_text, segments, fallback_text) = match stage {
        Stage::Polishing {
            id,
            raw_text,
            segments,
            fallback_text,
        } => {
            if *id != session_id {
                debug!(
                    "FinalPolishDone session_id mismatch (polish={}, polishing={}) — 跨会话护栏，丢弃",
                    session_id, id
                );
                return;
            }
            (*id, raw_text.clone(), segments.clone(), fallback_text.clone())
        }
        _ => {
            debug!("FinalPolishDone ignored in stage {:?}", stage_name(stage));
            return;
        }
    };

    match result {
        Ok(polished) => {
            info!(
                "Final polish: {} → {} chars",
                raw_text.chars().count(),
                polished.chars().count()
            );
            do_paste(
                stage,
                &polished,
                id,
                &raw_text,
                &segments,
                "done",
                config,
                app_handle,
                tx,
            );
        }
        Err(e) => {
            warn!("Final polish failed: {}, using fallback (display)", e);
            use tauri::Emitter;
            let _ = app_handle.emit("polish-error", &e);
            do_paste(
                stage,
                &fallback_text,
                id,
                &raw_text,
                &segments,
                "failed",
                config,
                app_handle,
                tx,
            );
        }
    }
}

/// 启动润色线程
/// `ignore_mode`=true 时跳过 polish_mode 检查（供「立即润色」用）。
/// `input.segments` 转多段润色协议（Edited 段 preserve 原样保留，其余润色，spec §12 / §2.C）。
/// `session_id` = 发起润色时的 transcript.id，原样塞进 PolishDone 回传，供 handle_polish_done
/// 做跨会话护栏（审查 一1：润色线程不持 transcript 引用，回来时当前 transcript 可能已是新会话）。
pub(crate) fn spawn_polish_thread(
    input: crate::engine::transcript::PolishInput,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
    session_id: i64,
) {
    // 段模型多段润色：Edited 段 preserve=true（LLM 原样保留），其余待润色。
    let regions = polish_input_to_regions(&input);
    let llm_config = if ignore_mode {
        crate::core::config::llm_config_ignore_mode()
    } else {
        crate::core::config::llm_config(config.polish_mode)
    };
    let llm_config = match llm_config {
        Some(c) => c,
        None => return,
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish_regions(&regions, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => {
                log::warn!("Polish thread error: {}", e);
                Err(e.to_string())
            }
        };
        let _ = tx.send(Command::PolishDone { result, session_id });
    });
}

/// 把 transcript 的 PolishInput（segments 快照）转成 octopus_llm 多段润色输入。
/// Edited 段 preserve=true（人工校对，原样保留）；Raw/Polished 段 preserve=false（待润色）。
/// 两处润色触发点（spawn_polish_thread + 最终润色内联）共用，避免折叠逻辑重复。
pub(crate) fn polish_input_to_regions(input: &crate::engine::transcript::PolishInput) -> Vec<octopus_llm::PolishRegion> {
    input.segments.iter().map(|s| octopus_llm::PolishRegion {
        preserve: s.kind == crate::engine::transcript::SegmentKind::Edited,
        text: s.text.clone(),
    }).collect()
}

/// 停顿驱动润色：流式 silence≥阈值 / 伪流式段边界 → 对完整 ASR 全量润色（mode=2 only）。
///
/// - 流式由调用方传当前真实 silence_duration；
/// - 伪流式在段切分后调用，传 PAUSE_POLISH_THRESHOLD_SEC（段边界即停顿点，自动达标）。
pub(crate) fn check_and_trigger_polish(
    transcript: &mut Transcript,
    silence_duration: f64,
    config: &AppConfig,
    tx: &Sender<Command>,
) {
    // 仅 mode=2（中间润色）；有 pending 或无文本 → 跳过
    if config.polish_mode != PolishMode::Intermediate
        || transcript.polish_pending()
        || transcript.full().is_empty()
    {
        return;
    }
    // 无 Raw 段（无待润色的新语音）→ 跳过（段模型：has_raw 替代旧 has_increase）
    if !transcript.has_raw() {
        return;
    }
    // 有待删选区（用户拖选尚未说话）→ 跳过：take_polish_input 会消费 pending_delete
    // 提前删选区，违背「说话才删」。等用户开口（首个 delta 消费 pending_delete）后再润色。
    if transcript.has_pending_delete() {
        return;
    }
    // 停顿未达标 → 跳过（流式传真实 silence；伪流式传阈值自动达标）
    if silence_duration < config.pause_polish_threshold_ms / 1000.0 {
        return;
    }
    // 节流：距上次润色不足 interval（至少 MIN_POLISH_INTERVAL_SEC）→ 跳过
    if transcript.last_polish_time().elapsed().as_secs_f64()
        < config.polish_min_interval.max(MIN_POLISH_INTERVAL_SEC)
    {
        return;
    }
    // 取润色输入（段模型快照）+ 标记 pending（take_polish_input 内部已置 pending）+ 送 LLM
    let input = transcript.take_polish_input();
    // 诊断（spec 2026-07-19 第二轮）：自动润色触发，验证假设 A
    crate::core::perf_log::log(&format!(
        "[POLISH] auto-trigger t={} silence={:.2} mode={:?} segs={}",
        transcript.id, silence_duration, config.polish_mode, input.segments.len(),
    ));
    spawn_polish_thread(input, config, tx, false, transcript.id);
}

/// 处理 PolishDone 命令：把润色结果写回 Transcript。
pub(crate) fn handle_polish_done(
    stage: &mut Stage,
    result: Result<String, String>,
    session_id: i64,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    _tx: &Sender<Command>,
) {
    // StoppingPolish 特殊处理：PolishDone 到达后走 final 路径（需 owned transcript）
    if let Stage::StoppingPolish { transcript } = stage {
        // 跨会话护栏
        if transcript.id != session_id {
            warn!(
                "PolishDone discarded: session_id mismatch (polish={}, transcript={}) — 跨会话护栏",
                session_id, transcript.id
            );
            crate::core::perf_log::log(&format!(
                "[POLISH] done stage=StoppingPolish discarded_reason=session_mismatch polish_sid={} cur_id={}",
                session_id, transcript.id,
            ));
            use tauri::Emitter;
            let _ = app_handle.emit("polish-done", ());
            return;
        }
        // 写入润色结果
        match result {
            Ok(polished) => {
                if polished.is_empty() {
                    warn!("Polish returned empty, keeping previous");
                    use tauri::Emitter;
                    let _ = app_handle.emit("polish-error", "LLM 返回空结果（可能是思考模型未关闭 thinking）");
                    transcript.on_polish_failed();
                    crate::core::perf_log::log("[POLISH] done stage=StoppingPolish result=empty → on_polish_failed");
                } else {
                    transcript.polish_apply(&polished);
                    crate::core::perf_log::log(&format!(
                        "[POLISH] done stage=StoppingPolish result=ok polished_len={}", polished.chars().count(),
                    ));
                    let cmd = if transcript.has_edit() {
                        DbCommand::UpdateEditedSegments {
                            id: transcript.id,
                            text: transcript.finish_text(),
                            segments: transcript.segments_json(),
                        }
                    } else {
                        DbCommand::UpdatePolished {
                            id: transcript.id,
                            text: transcript.finish_text(),
                            status: "done".to_string(),
                            model: Some(active_llm_name()),
                            segments: transcript.segments_json(),
                        }
                    };
                    if let Err(e) = get_db_sender().send(cmd) {
                        warn!("Queue DB update_polish_result failed: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Polish failed: {}, keeping previous", e);
                use tauri::Emitter;
                let _ = app_handle.emit("polish-error", &e);
                transcript.on_polish_failed();
                crate::core::perf_log::log(&format!(
                    "[POLISH] done stage=StoppingPolish result=err err_len={}", e.chars().count(),
                ));
            }
        }
        use tauri::Emitter;
        let _ = app_handle.emit("polish-done", ());
        // PolishDone 处理完成（pending 已清），走 final 路径
        let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
        finalize_after_stop(stage, tr, config, app_handle, _tx);
        return;
    }

    // 在借用 transcript 之前算出 stage_name，避免后续打点同时借 stage（不可变）与 transcript（可变）
    let sname = stage_name(stage);
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        _ => {
            debug!("PolishDone ignored: stage={} 不是录音/等待阶段，润色结果丢弃", sname);
            crate::core::perf_log::log(&format!(
                "[POLISH] done stage={} ignored_reason=not_recording_stage", sname,
            ));
            use tauri::Emitter;
            let _ = app_handle.emit("polish-done", ());
            return;
        }
    };
    // 跨会话护栏（审查 一1）：润色线程不携带 transcript 引用，PolishDone 回到 coordinator 时
    // 当前 transcript 可能已是新会话（用户在 1~3s 润色窗口内 Esc+Toggle 重开）。session_id
    // 不符即丢弃，防止旧会话润色结果污染新会话 transcript + 写错 DB 行（UpdatePolished/UpdateEdited）。
    if transcript.id != session_id {
        warn!(
            "PolishDone discarded: session_id mismatch (polish={}, transcript={}) — 跨会话护栏",
            session_id, transcript.id
        );
        crate::core::perf_log::log(&format!(
            "[POLISH] done stage={} discarded_reason=session_mismatch polish_sid={} cur_id={}",
            sname, session_id, transcript.id,
        ));
        use tauri::Emitter;
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    match result {
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                use tauri::Emitter;
                let _ = app_handle.emit("polish-error", "LLM 返回空结果（可能是思考模型未关闭 thinking）");
                transcript.on_polish_failed();
                crate::core::perf_log::log(&format!(
                    "[POLISH] done stage={} result=empty → on_polish_failed", sname,
                ));
            } else {
                // 段模型回填（polish_apply 内部按 edited 串匹配定位 + 间隙 Polished）
                transcript.polish_apply(&polished);
                crate::core::perf_log::log(&format!(
                    "[POLISH] done stage={} result=ok polished_len={}", sname, polished.chars().count(),
                ));
                // 含 Edited 段→UpdateEditedSegments（保持 edited/text/segments 一致）；否则 UpdatePolished（现状）
                let cmd = if transcript.has_edit() {
                    DbCommand::UpdateEditedSegments {
                        id: transcript.id,
                        text: transcript.finish_text(),
                        segments: transcript.segments_json(),
                    }
                } else {
                    // 中间润色入库 polished（polish_model 传 config.polish_llm，与 PasteDone 一致，便于统计）
                    DbCommand::UpdatePolished {
                        id: transcript.id,
                        text: transcript.finish_text(),
                        status: "done".to_string(),
                        model: Some(active_llm_name()),
                        segments: transcript.segments_json(),
                    }
                };
                if let Err(e) = get_db_sender().send(cmd) {
                    warn!("Queue DB update_polish_result failed: {}", e);
                }
                if !transcript.full().is_empty() {
                    crate::ui::result_window::update_result(app_handle, &transcript.display_text(), false, 0);
                }
            }
        }
        Err(e) => {
            warn!("Polish failed: {}, keeping previous", e);
            use tauri::Emitter;
            let _ = app_handle.emit("polish-error", &e);
            transcript.on_polish_failed();
            crate::core::perf_log::log(&format!(
                "[POLISH] done stage={} result=err err_len={}", sname, e.chars().count(),
            ));
        }
    }
    // 通知前端：润色完成（成功/失败均通知，前端恢复「立即润色」按钮）
    use tauri::Emitter;
    let _ = app_handle.emit("polish-done", ());
}

/// 处理立即润色命令：不管 polish_mode，取当前完整 ASR 文本送 LLM 润色。
/// 仅在 Streaming / VadSegmented 阶段生效（需有 transcript）；其他阶段忽略。
/// 与 `check_and_trigger_polish` 区别：不检查 mode/threshold/interval/has_raw，
/// 直接快照全量文本送 LLM。
pub(crate) fn handle_polish_now(
    stage: &mut Stage,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    use tauri::Emitter;
    // 所有早退路径都 emit polish-done 恢复前端按钮——
    // 否则用户点了「立即润色」后按钮 disabled=true 永久卡死，直到下次录音才恢复
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        _ => {
            debug!("PolishNow ignored in stage {:?}", stage_name(stage));
            crate::core::perf_log::log(&format!(
                "[POLISH] PolishNow-ignored stage={} (no transcript)", stage_name(stage),
            ));
            let _ = app_handle.emit("polish-done", ());
            return;
        }
    };
    if transcript.full().is_empty() {
        debug!("PolishNow skipped: transcript empty");
        crate::core::perf_log::log("[POLISH] PolishNow-skipped reason=empty");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    if transcript.polish_pending() {
        debug!("PolishNow skipped: polish already pending");
        crate::core::perf_log::log("[POLISH] PolishNow-skipped reason=already_pending");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    // 检查 LLM 配置是否存在（忽略 polish_mode，立即润色不看 mode）
    if crate::core::config::llm_config_ignore_mode().is_none() {
        warn!("PolishNow: no LLM config available");
        // 不覆盖浮窗识别文本——以 polish-error 红色气泡提示，保留原文显示
        let _ = app_handle.emit("polish-error", "未配置润色模型");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    // 确保 DB 记录已 INSERT：CloudStreaming 路径只在 Finished 事件时 INSERT，
    // 如果从未触发 Finished，PolishDone 的 UpdatePolished（UPDATE）会静默 0 行。
    // 本地路径中 Streaming/VadSegmented 已在 accept_samples 时 INSERT，此处 no-op。
    if let Err(e) = update_transcription_raw(transcript, &active_asr_engine_name(), "streaming") {
        warn!("PolishNow ensure INSERT failed: {}", e);
    }
    let input = transcript.take_polish_input();
    // 诊断（spec 2026-07-19 第二轮）：手动润色触发，验证假设 G（编辑期间 PolishNow → PolishDone 覆盖用户编辑）
    crate::core::perf_log::log(&format!(
        "[POLISH] PolishNow-manual-trigger t={} chars={}",
        transcript.id,
        input.segments.iter().map(|s| s.text.chars().count()).sum::<usize>(),
    ));
    info!("PolishNow triggered, polishing {} chars", input.segments.iter().map(|s| s.text.chars().count()).sum::<usize>());
    // 通知前端「润色开始」（按钮变灰）——后端发起的润色（如 toggle 中按 alt 键）绕过
    // 前端 polishNow()，前端 setPolishLoading(true) 不会被触发。emit polish-started 让
    // 前端无论从哪发起都能反馈润色态。polish-done/polish-error 恢复。
    let _ = app_handle.emit("polish-started", ());
    spawn_polish_thread(input, config, tx, true, transcript.id);
}
