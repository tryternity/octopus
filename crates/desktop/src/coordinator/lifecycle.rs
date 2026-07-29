//! 录音生命周期核心：停止 / finalize / 看门狗重连（从 coordinator/mod.rs 提取，Task 2.3）。
//!
//! - `handle_toggle`：活跃态停录音（VadSegmented/Streaming 分支排空 + finalize；
//!   cloud 走 close_async 非阻塞）。
//! - `restart_capture_keep_transcript`：cpal 断推看门狗——中断+重启录音，复用 transcript。
//! - `finalize_after_stop`：停止后统一收尾（立即润色在途→StoppingPolish；否则润色或粘贴）。
//! - `finalize_cloud`（#[cfg(cloud)]）：云端 finalize，拼 partial + 走润色/粘贴。
//! - `handle_cloud_streaming_done`（#[cfg(cloud)]）：close_async 结果回传 + 跨会话护栏。

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::config::PolishMode;
use crate::engine::TranscriptionEngine;
use crate::pipeline::StreamingPipeline;
use crate::transcript::Transcript;
use octopus_asr_local::streaming_engine::StreamingSessionManager;
use octopus_asr_local::streaming_runner::TranscriptEvent;
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use super::{Command, Stage, RecordType, TRANSLATION_ACTIVE};
use super::paste::{stage_name, do_paste, active_asr_engine_name};
#[cfg(feature = "cloud")]
use super::paste::update_transcription_raw;
use super::tick::{start_tick_thread, start_vad_segmented_tick_thread};
use super::agent::dispatch_by_record_type;
use super::polish::start_final_polish_or_paste;

/// 处理 Toggle 命令（仅活跃态停录音；Idle 走主循环两阶段 → begin_recording）。
pub(crate) fn handle_toggle(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    match stage {
        Stage::Idle => {
            // 不可达：主循环 Toggle 在 Idle 走两阶段（emit prepare-record → StartRecording），
            // 仅在活跃态调 handle_toggle 停录音。保留 no-op 分支使 match 穷尽。
        }

        Stage::VadSegmented { .. } => {
            // mem::replace 取出 owned 部件，避开 &mut stage 借用冲突（2c-3）
            let (mut pipeline, mut transcript, tick_active) =
                match std::mem::replace(stage, Stage::Idle) {
                    Stage::VadSegmented { pipeline, transcript, tick_active } => {
                        (pipeline, transcript, tick_active)
                    }
                    _ => {
                        log::error!("unexpected stage in handle_toggle VadSegmented, falling back to Idle");
                        return;
                    }
                };
            info!("Toggle: stopping VadSegmented (active_count={})", pipeline.active_count());

            // 停止录音并排空剩余音频（tail 喂入 pipeline 触发最后一轮切段）。
            // 2d：tick 事件流；事件丢弃（现状 stop 无 DB/emit，副作用靠 finalize）。
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                let _ = pipeline.tick(&remaining, &mut transcript);
            }
            pipeline.finish(&mut transcript);  // drain 在途段

            if pipeline.active_count() > 0 {
                // 还有识别在跑：pipeline + tick_active move 进 WaitingCompletion，
                // tick 线程不停（收尾靠 tick 继续发 VadSegmentedTick drain rx）
                *stage = Stage::WaitingCompletion { pipeline, transcript, tick_active };
            } else {
                // 全部完成：停 tick 线程 + finalize（pipeline drop）
                tick_active.store(false, Ordering::Relaxed);
                finalize_after_stop(stage, transcript, config, app_handle, tx);
            }
        }


        Stage::Streaming {
            pipeline,
            transcript,
            streaming_active,
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            let _ = audio.stop();

            #[cfg(feature = "cloud")]
            if pipeline.is_cloud() {
                // cloud: tick(tail) 喂入 push_pcm + finish（不发 Finish——Finish 由 close_async 发，避免重复）。
                // 2d：tick 事件流；事件丢弃（现状 stop 无 DB/emit，副作用靠 finalize_cloud）。
                if !final_samples.is_empty() {
                    let _ = pipeline.tick(&final_samples, transcript);
                }
                let _ = pipeline.finish();
                let partial = pipeline.current_partial().to_string();
                if let Some(handle) = pipeline.take_close_handle() {
                    // spawn close_async，结果以 Command::CloudStreamingDone 回来；期间进 CloudClosing
                    // 审查 三1：close 改非阻塞——原 sess.close(&rt) block_on 最多卡 coordinator 8s。
                    // Toggle/Cancel 在 CloudClosing 阶段被忽略（busy closing），不阻塞主线程。
                    let rt = tauri::async_runtime::handle();
                    let tx_clone = tx.clone();
                    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
                    // 跨会话护栏：close 在飞期间 Cancel/Discard 会把 stage 清回 Idle（绕过 Toggle
                    // 的"忙"保护），用户可立刻重开云端会话 → 新 CloudClosing。旧会话迟到的
                    // CloudStreamingDone 会匹配到新 CloudClosing。带 session_id（= 本会话
                    // transcript.id），handler 校验当前 closing transcript.id 是否匹配，否则丢弃。
                    let session_id = tr.id;
                    rt.spawn(async move {
                        // 看门狗：close 超时也必须发 CloudStreamingDone，否则 stage 永久卡死
                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            handle.close_async(),
                        )
                        .await;
                        let text_result = match result {
                            Ok(Ok(text)) => Ok(text),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err("cloud close timeout (30s)".to_string()),
                        };
                        let _ = tx_clone.send(Command::CloudStreamingDone {
                            text: text_result,
                            session_id,
                        });
                    });
                    *stage = Stage::CloudClosing {
                        transcript: tr,
                        current_partial: partial,
                    };
                    return;
                }
                // 无活跃 session：无需等 close，直接 finalize_cloud（无标点补全，服务端已分句）
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
                finalize_cloud(stage, tr, partial, config, app_handle, tx);
                return;
            }

            // local: tick(tail) accept + finish flush（tail 经 push_samples 喂入；finish Final 覆盖）。
            // 2d：tick 事件流；事件丢弃（现状 stop 无 DB/emit，副作用靠 finalize_after_stop）。
            if !final_samples.is_empty() {
                let _ = pipeline.tick(&final_samples, transcript);
            }
            let final_text = match pipeline.finish() {
                TranscriptEvent::Final(text) => text,
                TranscriptEvent::Error(e) => {
                    error!("Streaming finish failed: {}", e);
                    // 引擎兜底：finish_text（段模型已含 edited/raw 全部）
                    transcript.finish_text()
                }
                _ => transcript.finish_text(),
            };
            pipeline.reset();
            if !final_text.is_empty() {
                transcript.apply_engine_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }

        Stage::WaitingCompletion { .. } => {
            debug!("Toggle ignored: waiting for transcription completion");
        }

        Stage::Polishing { .. } => {
            debug!("Toggle ignored: busy polishing");
        }

        Stage::StoppingPolish { .. } => {
            debug!("Toggle ignored: waiting for polish to complete");
        }

        Stage::Pasting { .. } => {
            debug!("Toggle ignored: busy pasting");
        }

        // 审查 三1：close 在飞（close_async 未回），Toggle 忽略——close 完成后
        // CloudStreamingDone 会自动 finalize + 粘贴，无需 Toggle 介入。期间不阻塞主线程。
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { .. } => {
            debug!("Toggle ignored: cloud closing in flight");
        }
    }
}

/// 音频采集看门狗：cpal 断推后自动重连（spec 2026-07-24-audio-watchdog §4.2）。
///
/// 语义：**中断 + 重启录音，复用 transcript**——两次录音的文本拼在一起，识别框不隐藏。
/// 区别于 `handle_toggle`（停止→finalize→粘贴）和 `begin_recording`（新建 transcript）。
///
/// 流程：
/// 1. 停 tick 线程 + `audio.stop()` 取尾部 + 喂尾给旧 pipeline + `finish` flush 在途 partial
/// 2. 取出 owned transcript（保留，不交给 finalize）
/// 3. `transcript.reset_engine_baseline()` 清引擎基准（与重建 pipeline 空状态对齐）
/// 4. `audio.start()` 重连 cpal——失败则二次降级（mic-error + finalize 粘贴已识别文本）
/// 5. 引擎 Arc 取用 + reset + 新建 pipeline + transcript 放回 Stage + 重启 tick 线程
/// 6. `update_result` 刷新显示（窗口一直可见）+ emit `mic-reconnecting`
///
/// cloud 引擎（`Stage::Streaming` 且 `is_cloud()`）不在此处理——cloud 断流走独立 WS 重试，
/// 触发时 no-op + warn。
#[allow(clippy::too_many_arguments)]
pub(crate) fn restart_capture_keep_transcript(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    use_streaming: bool,
) {
    info!("[WATCHDOG] restart_capture triggered, stage={}", stage_name(stage));

    // ── 停止阶段：取出 transcript（保留）──
    let mut transcript = match std::mem::replace(stage, Stage::Idle) {
        Stage::Streaming { mut pipeline, transcript, streaming_active } => {
            // cloud 引擎不自动重连（独立 WS 连接，断流语义不同）
            if pipeline.is_cloud() {
                warn!("[WATCHDOG] cloud engine stall, skip restart (cloud 有独立重试)");
                // 还原 stage，让 cloud 自己的错误处理接管
                *stage = Stage::Streaming { pipeline, transcript, streaming_active };
                return;
            }
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            let _ = audio.stop();
            // tail 喂入 + finish flush 在途 partial（同 handle_toggle，但不 apply_engine_full 不 finalize）
            if !final_samples.is_empty() {
                let _ = pipeline.tick(&final_samples, &mut Transcript::new(0, PolishMode::Disabled, RecordType::Input));
            }
            let _ = pipeline.finish();
            transcript
        }
        Stage::VadSegmented { mut pipeline, mut transcript, tick_active } => {
            tick_active.store(false, Ordering::Relaxed);
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                let _ = pipeline.tick(&remaining, &mut transcript);
            }
            pipeline.finish(&mut transcript);
            transcript
        }
        Stage::WaitingCompletion { mut pipeline, mut transcript, tick_active } => {
            // WaitingCompletion：stop 后在途段识别中，此时 is_recording 已 false，
            // 正常不应触发看门狗（sample_stall_duration 返回 0）。防御性处理：同 VadSegmented。
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
            pipeline.finish(&mut transcript);
            transcript
        }
        other => {
            // 非活跃 stage（Idle/Polishing/Pasting 等）收到 RestartCapture——异常，还原 + warn
            warn!("[WATCHDOG] unexpected stage {} for restart, ignoring", stage_name(&other));
            *stage = other;
            return;
        }
    };

    // ── 清引擎基准（与重建 pipeline 空状态对齐，spec §3.5）──
    transcript.reset_engine_baseline();
    let show_text = transcript.display_text();

    // ── 重连阶段 ──
    if let Err(e) = audio.start(&config.microphone) {
        // 二次失败降级：mic-error + finalize 粘贴已识别文本（spec §3.3）
        error!("[WATCHDOG] 重连失败: {}, 降级 finalize", e);
        let _ = app_handle.emit("mic-error", "麦克风采集中断，自动重连失败，请检查设备后重试");
        finalize_after_stop(stage, transcript, config, app_handle, tx);
        return;
    }

    // 展示旧文本（窗口一直可见，is_continuation 路径——不走 show-result else 清空 caret）
    let show_placeholder = if show_text.is_empty() { "正在聆听…" } else { "🎙️ 麦克风重连中…" };
    crate::result_window::show_result(app_handle, show_placeholder);
    if !show_text.is_empty() {
        crate::result_window::update_result(app_handle, &show_text, false, 0);
    }
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);
    let _ = app_handle.emit("mic-reconnecting", ());

    // 重建 pipeline（复用常驻引擎，不重载模型）+ transcript 放回 Stage + 重启 tick
    if use_streaming {
        let asr_engine = active_asr_engine_name();
        let streaming_manager = app_handle
            .state::<std::sync::Arc<StreamingSessionManager>>();
        let streaming_engine = match streaming_manager
            .active_session(&asr_engine, &config.language)
        {
            Ok(arc) => { arc.reset(); arc }
            Err(e) => {
                error!("[WATCHDOG] 流式引擎取用失败: {}, 降级 finalize", e);
                let _ = audio.stop();
                let _ = app_handle.emit("mic-error", "麦克风采集中断，引擎重连失败");
                finalize_after_stop(stage, transcript, config, app_handle, tx);
                return;
            }
        };
        let local_engine = match crate::pipeline::LocalPipelineEngine::from_session(streaming_engine, false) {
            Ok(e) => e,
            Err(e) => {
                error!("[WATCHDOG] LocalPipelineEngine init failed: {}", e);
                let _ = audio.stop();
                finalize_after_stop(stage, transcript, config, app_handle, tx);
                return;
            }
        };
        let pipeline = match StreamingPipeline::new(Box::new(local_engine)) {
            Ok(p) => p,
            Err(e) => {
                error!("[WATCHDOG] StreamingPipeline init failed: {}", e);
                let _ = audio.stop();
                finalize_after_stop(stage, transcript, config, app_handle, tx);
                return;
            }
        };
        let streaming_active = Arc::new(AtomicBool::new(true));
        start_tick_thread(tx.clone(), streaming_active.clone());
        *stage = Stage::Streaming { pipeline, transcript, streaming_active };
    } else {
        match crate::pipeline::VadSegmentedPipeline::new(
            engine.clone(),
            config.language.clone(),
            active_asr_engine_name(),
            config.segment_silence,
        ) {
            Ok(pipeline) => {
                let tick_active = Arc::new(AtomicBool::new(true));
                start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());
                *stage = Stage::VadSegmented { pipeline, transcript, tick_active };
            }
            Err(e) => {
                error!("[WATCHDOG] VAD init failed: {}, 降级 finalize", e);
                let _ = audio.stop();
                finalize_after_stop(stage, transcript, config, app_handle, tx);
            }
        }
    }
    info!("[WATCHDOG] restart_capture done, stage={}", stage_name(stage));
}


///
/// **修复 bug**：原实现直接 `transcript.clear_polish_pending()` 后走 final 路径，
/// 导致：(1) 立即润色的 `PolishDone` 回来时 stage 已切换 → 结果被丢弃；
/// (2) 若 `polish_mode=0`，最终润色被跳过 → 只粘贴原文，DB 也只存原文。
///
/// 现在的语义：若仍有 pending 的立即润色，进入 `StoppingPolish` 持有 transcript，
/// `PolishDone` 到达后在 `handle_polish_done` 中走 final 路径，把立即润色结果纳入最终文本。
///
/// **优化**：若无 Raw 段且非空（has_raw=false），立即润色已覆盖全部文本，
/// 跳过最终润色（mode=1/2 也跳过），直接 paste，避免平白多一次 LLM 调用。
pub(crate) fn finalize_after_stop(
    stage: &mut Stage,
    mut transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 0. flush 滞留 diverted（引擎 end-of-stream 纠正）：stop 后不再有 apply 补发，
    //    不 flush 会被 finish_text 读取时静默丢弃（末尾文字丢失）。
    transcript.flush_diverted();
    // 1. 立即润色仍在途：等其完成再走 final 路径（避免丢弃润色结果）
    if transcript.polish_pending() {
        info!("Toggle stop: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }
    // 2. 无 pending：检查是否可以跳过最终润色
    //    段模型下「已润色覆盖全部」= 无 Raw 段且非空（has_raw=false）。
    let skip_final_polish = !transcript.finish_text().is_empty() && !transcript.has_raw();
    // 3. 句末标点补全 + finish_text 计算（与原 final 路径一致）
    let combined = if transcript.full().is_empty() {
        String::new()
    } else if transcript
        .full()
        .ends_with(|c: char| ",.，。！？!?\n".contains(c))
    {
        transcript.db_text()
    } else {
        format!("{}。", transcript.db_text())
    };
    if combined.is_empty() {
        // 统一分流：AgentBridge 空文本标 failed
        dispatch_by_record_type(&transcript, "", app_handle);
        TRANSLATION_ACTIVE.store(false, Ordering::Relaxed);
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    // 统一分流：AgentBridge 非空 → execute_agent_task
    // AgentBridge 用 db_text() 不追加句号（句号是 paste 逻辑，不适合 agent task）
    if dispatch_by_record_type(&transcript, &transcript.db_text(), app_handle) {
        *stage = Stage::Idle;
        return;
    }

    crate::result_window::show_result(app_handle, &transcript.display_text());
    if skip_final_polish {
        // 立即润色已覆盖全部文本，直接 paste（polish_status="done"）
        info!("Toggle stop: skip final polish (polished covers all, no increase)");
        let display = transcript.display_text();
        let raw = transcript.db_text();
        let segs = transcript.segments_json();
        do_paste(stage, &display, transcript.id, &raw, &segs, "done", config, app_handle, tx);
    } else {
        // 走原 final 路径（按 polish_mode 决定是否润色）
        start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
    }
}

/// 云端流式 finalize：把未提交的 partial 拼进 transcript，空则回 Idle，
/// 否则走与本地引擎一致的「最终润色或粘贴」流程。
///
/// 审查 三1：从 stop 路径（无 session）与 CloudStreamingDone 路径（close 完成后）
/// 共用，避免 finalize 逻辑重复。`transcript` / `current_partial` 为 owned（已从
/// stage 移出），`stage: &mut Stage` 仅用于写回 Idle/Polishing/Pasting，无别名冲突。
#[cfg(feature = "cloud")]
pub(crate) fn finalize_cloud(
    stage: &mut Stage,
    mut transcript: Transcript,
    current_partial: String,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // flush 滞留 diverted（cloud close 返回整段最终文本，常与 tentative partial 发散→diverted）：
    // 不 flush 会被下方 db_text() 读取时丢弃。
    transcript.flush_diverted();
    // 即使无 session 或 close 无返回，也提交未 commit 的 partial
    if !current_partial.is_empty() {
        let sep = octopus_asr_local::sentence_separator(&config.language);
        if !transcript.full().is_empty() && !transcript.full().ends_with(sep) {
            transcript.append_segment(sep);
        }
        transcript.append_segment(&current_partial);
    }

    let combined = transcript.db_text();
    if combined.is_empty() {
        dispatch_by_record_type(&transcript, "", app_handle);
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    // 确保 DB 记录已 INSERT（在 dispatch 之前——AgentBridge 也应进 ASR 历史）
    if let Err(e) = update_transcription_raw(&mut transcript, &active_asr_engine_name(), "streaming") {
        warn!("CloudStreaming finalize INSERT failed: {}", e);
    }

    // 统一分流：AgentBridge → execute_agent_task
    if dispatch_by_record_type(&transcript, &combined, app_handle) {
        *stage = Stage::Idle;
        return;
    }

    // 立即润色仍在途：进 StoppingPolish 等 PolishDone
    // （CloudStreaming 的 partial 已 append 到 transcript.full，不会再增长）
    if transcript.polish_pending() {
        info!("CloudStreaming finalize: polish_pending=true, entering StoppingPolish");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
        crate::result_window::show_result(app_handle, "⏳ 等待润色完成...");
        *stage = Stage::StoppingPolish { transcript };
        return;
    }

    crate::result_window::show_result(app_handle, &transcript.display_text());
    start_final_polish_or_paste(stage, &combined, transcript, config, app_handle, tx);
}

/// 处理云端 close（close_async）异步完成结果。
///
/// 审查 三1：stop 路径 spawn 了 close_async，结果经 `Command::CloudStreamingDone`
/// 回到 coordinator 主线程。仅在 `Stage::CloudClosing` 时处理；close 返回的整段文本
/// set_full 覆盖 transcript，随后 finalize 落库。
///
/// 跨会话护栏：close 在飞期间 Cancel/Discard 会把 stage 清回 Idle（绕过 Toggle 的
/// "忙"保护），用户可立刻重开云端会话 → 新 CloudClosing。旧会话迟到的
/// CloudStreamingDone 会匹配到新 CloudClosing，set_full 覆盖新 transcript。session_id
///（= 发起 close 时的 transcript.id）校验：与当前 closing transcript.id 不符则丢弃，
/// 不动当前 stage。
#[cfg(feature = "cloud")]
pub(crate) fn handle_cloud_streaming_done(
    stage: &mut Stage,
    text: Result<String, String>,
    session_id: i64,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let (transcript, partial) = match stage {
        Stage::CloudClosing { transcript, current_partial } => {
            if transcript.id != session_id {
                warn!(
                    "CloudStreamingDone session_id mismatch (close={}, closing={}) — 跨会话护栏，丢弃",
                    session_id, transcript.id
                );
                return;
            }
            // close 返回的是整个 session 的完整文本，非空则 apply_engine_full 喂回（前缀追加；diverted 重算基准）
            match &text {
                Ok(text) if !text.is_empty() => { transcript.apply_engine_full(text); }
                Ok(_) => {}
                Err(e) => warn!("CloudStreaming close WSS failed: {}", e),
            }
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
            let p = std::mem::take(current_partial);
            (tr, p)
        }
        _ => {
            warn!("CloudStreamingDone received but stage != CloudClosing, ignoring");
            return;
        }
    };
    finalize_cloud(stage, transcript, partial, config, app_handle, tx);
}
