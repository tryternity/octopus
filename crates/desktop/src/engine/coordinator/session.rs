//! 录音会话建立（从 coordinator/mod.rs 提取，Task 2.1）。
//!
//! `begin_recording` 是开录音总入口（cloud / streaming / vad 三分支分发），
//! 抽自 `handle_toggle` 的 Idle 分支，供 C3 两阶段 Toggle 的 StartRecording / FallbackStart 复用。
//! 三个 `prepare_*_session` 分别构造对应 pipeline + transcript + 启动 tick 线程，进入活跃 Stage。

use crate::engine::audio::SharedAudioState;
use crate::core::config::AppConfig;
use crate::engine::engine::TranscriptionEngine;
use crate::engine::pipeline::StreamingPipeline;
use crate::engine::transcript::Transcript;
use octopus_asr_local::streaming_engine::StreamingSessionManager;
use log::{debug, error, info, warn};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use super::{Command, Stage, RecordType, FALLBACK_STREAMING_SPEC, set_current_transcription_id, INSTANT_MODE};
use super::paste::{now_millis, active_asr_engine_name};
use super::tick::{start_tick_thread, start_vad_segmented_tick_thread};
#[cfg(feature = "cloud")]
use super::tick::start_cloud_streaming_tick_thread;

/// 开录音首帧展示：instant 模式 → instant 浮窗 listening 态；否则 → result_window。
///
/// instant 模式下不展示 selection 延续文本（浮窗只读、紧凑，仅作录音指示），
/// 保持与 talk 模式「只看状态、结果粘贴后即隐藏」的语义一致。
fn show_listening_start(app_handle: &tauri::AppHandle, show_text: &str, is_continuation: bool) {
    if INSTANT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        crate::ui::result_window::show_instant(app_handle, "listening", "");
        return;
    }
    if is_continuation {
        crate::ui::result_window::show_result(app_handle, "正在聆听…", None);
        crate::ui::result_window::update_result(app_handle, show_text, false, 0, None);
    } else {
        crate::ui::result_window::show_result(app_handle, show_text, None);
    }
}

/// 第三十三轮 P1-1：开录音失败时清 INSTANT_MODE + recording_mode。
///
/// set_recording_mode(1/2/3) 在 begin_recording 之前设（mod.rs:466/789/841），失败分支
/// 必须清回——否则 ptt.rs:306 next_on_keydown 读残留 mode → 走停止分支 → PTT 按键卡死；
/// INSTANT_MODE 残留致下次 Toggle 走错浮窗。对比：cancel/discard/PasteDone 等出口全清
/// （lifecycle.rs:430/539），唯独开录音失败路径漏（5 处）。此 helper 统一收口。
fn reset_mode_flags_on_start_failure() {
    INSTANT_MODE.store(false, std::sync::atomic::Ordering::Relaxed);
    super::set_recording_mode(0);
}

/// 实际开录音：从 Idle 进入活跃录音态（cloud / streaming / vad 三分支）。
/// 抽自 handle_toggle 的 Idle 分支，供 C3 两阶段 Toggle 的 StartRecording / FallbackStart 复用。
/// selection = 跨会话选中替换种子（None=普通开录音；Some((text,start,end)) → 种子 transcript）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_recording(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    use_streaming: bool,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,
    #[cfg(feature = "cloud")] use_cloud_streaming: bool,
) {
    info!("Toggle: starting {}", {
        #[cfg(feature = "cloud")]
        { if use_cloud_streaming { "cloud streaming" } else if use_streaming { "streaming" } else { "VAD segmented" } }
        #[cfg(not(feature = "cloud"))]
        { if use_streaming { "streaming" } else { "VAD segmented" } }
    });

    if let Err(e) = audio.start(&config.microphone) {
        error!("Failed to start recording: {}", e);
        // 弹出结果窗 + 红色错误提示，告知用户麦克风不可用
        let _ = app_handle.emit("mic-error", "麦克风不可用，请在系统设置中授权麦克风权限");
        if INSTANT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
            // instant 模式：错误也走 instant 浮窗（done 态展示提示文字）。
            crate::ui::result_window::show_instant(app_handle, "done", "麦克风不可用");
        } else {
            crate::ui::result_window::show_result(app_handle, "", None);
        }
        crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                        reset_mode_flags_on_start_failure();
        return;
    }

    #[cfg(feature = "cloud")]
    if use_cloud_streaming {
        prepare_cloud_streaming_session(stage, audio, config, app_handle, tx, selection, record_type.clone());
        return;
    }

    if use_streaming {
        prepare_streaming_session(stage, audio, engine, config, app_handle, tx, selection, record_type.clone());
    } else {
        prepare_vad_segmented_session(stage, audio, engine, config, app_handle, tx, selection, record_type);
    }
}

#[cfg(feature = "cloud")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_cloud_streaming_session(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,
) {
    match octopus_asr_local::config::create_silero_vad() {
        Ok(mut vad) => {
            crate::engine::pipeline::vad_preroll(&mut vad);

                // 跨会话选中替换：有 selection → 种子 transcript（保留旧文本 + 删选区）。
                // cloud 与本地 streaming/vad 共用 Stage::Streaming + Transcript，下游 paste 由
                // pending_delete 驱动（首个 delta → delete_range），三条路径必须对称植入，否则 cloud 退化为追加。
                let tid = now_millis();
                set_current_transcription_id(tid);
                let (transcript, show_text, is_continuation) = if let Some((text, s, e)) = selection {
                    let mut t = Transcript::new(tid, config.polish_mode, record_type.clone());
                    t.commit_edit(&text, &[], true);
                    t.set_selection(s, e);
                    debug!("[select] cross-session seeded (cloud) t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
                    (t, text, true)
                } else {
                    (Transcript::new(tid, config.polish_mode, record_type.clone()), "正在聆听…".to_string(), false)
                };
                if is_continuation {
                    // 延续态：展示旧文本但不走 show-result（前端会把非占位符当最终文本→清空 caret）。
                    show_listening_start(app_handle, &show_text, true);
                } else {
                    show_listening_start(app_handle, &show_text, false);
                }
                crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Recording);

                let cloud_engine = crate::engine::cloud_pipeline::CloudPipelineEngine::new(
                    vad,
                    active_asr_engine_name(),
                    config.language.clone(),
                    config.pause_polish_threshold_ms,
                );
                let pipeline = match StreamingPipeline::new(Box::new(cloud_engine)) {
                    Ok(p) => p,
                    Err(e) => {
                        error!("StreamingPipeline (cloud) init failed: {}, abort", e);
                        let _ = audio.stop();
                        crate::ui::result_window::hide_result(app_handle);
                        crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                        reset_mode_flags_on_start_failure();
                        return;
                    }
                };

                // cloud 用独立 100ms tick 线程（STREAMING=200/CLOUD=100，不可合并）
                let tick_active = Arc::new(AtomicBool::new(true));
                start_cloud_streaming_tick_thread(tx.clone(), tick_active.clone());

                *stage = Stage::Streaming {
                    pipeline,
                    transcript,
                    streaming_active: tick_active,
                };
        }
        Err(e) => {
            // 第三十四轮 P1：VAD init 失败（模型未下载/磁盘错误/ONNX init 失败）必须完整清理——
            // audio.stop() 单独不够：recording_mode(2/3)+INSTANT_MODE 残留 → ptt.rs on_keydown
            // 走停止分支 → "ignored: not recording" → PTT 按键卡死（第三十三轮 P1-1 原症状）。
            // 原注释 "falling back to VadSegmented" 误导——这里直接 abort，无 fallback。
            error!("VAD init failed for cloud streaming: {}, abort (no fallback)", e);
            let _ = audio.stop();
            crate::ui::result_window::hide_result(app_handle);
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
            reset_mode_flags_on_start_failure();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_streaming_session(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    _engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,
) {
    // 流式引擎复用（②）：从 StreamingSessionManager 取常驻引擎 Arc + reset 清状态，
    // 不再每次录音 StreamingSession::new 重载 Session。模型变更由 active_session 懒加载覆盖，
    // 故 switch_active_model 无需主动联动。streaming_manager 经 app_handle.state 取（main 注入）。
    let asr_engine = active_asr_engine_name();
    let streaming_manager = app_handle
        .state::<std::sync::Arc<StreamingSessionManager>>();
    let streaming_engine = match streaming_manager
        .active_session(&asr_engine, &config.language)
    {
        Ok(arc) => {
            arc.reset();
            arc
        }
        Err(e) => {
            warn!(
                "流式引擎 '{}' 取用失败 ({}), 降级到默认引擎 '{}'",
                asr_engine, e, FALLBACK_STREAMING_SPEC
            );
            match streaming_manager
                .active_session(FALLBACK_STREAMING_SPEC, &config.language)
            {
                Ok(arc) => {
                    arc.reset();
                    arc
                }
                Err(e2) => {
                    error!("默认流式引擎也失败: {}", e2);
                    let _ = audio.stop();
                    crate::ui::result_window::hide_result(app_handle);
                    crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                        reset_mode_flags_on_start_failure();
                    return;
                }
            }
        }
    };

    // 跨会话选中替换：有 selection → 种子 transcript（保留旧文本 + 删选区）
    let tid = now_millis();
    set_current_transcription_id(tid);
    let (transcript, show_text, is_continuation) = if let Some((text, s, e)) = selection {
        let mut t = Transcript::new(tid, config.polish_mode, record_type.clone());
        t.commit_edit(&text, &[], true);
        t.set_selection(s, e);
        debug!("[select] cross-session seeded t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
        (t, text, true)
    } else {
        (Transcript::new(tid, config.polish_mode, record_type.clone()), "正在聆听…".to_string(), false)
    };
    if is_continuation {
        // 延续态：展示旧文本但不走 show-result（前端会把非占位符当最终文本→清空 caret）。
        // 直接 update_result 展示旧文本，保持前端 displayedRef 同步，caret 由后续 update-result 驱动。
        show_listening_start(app_handle, &show_text, true);
    } else {
        show_listening_start(app_handle, &show_text, false);
    }
    crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Recording);

    // StreamingPipeline 内部构造 StreamingRunner（VAD + 预热，阶段2a/2b）
    // correct = asr_correct 且非英文（corrector 是中文拼音纠错器，英文无意义且可能扰动；
    // skip_corrector 流式引擎 trait 无此方法，zipformer/paraformer 都不 skip，暂不考虑）。
    let correct = config.asr_correct && !config.language.eq_ignore_ascii_case("en");
    let local_engine = match crate::engine::pipeline::LocalPipelineEngine::from_session(streaming_engine, correct) {
        Ok(e) => e,
        Err(e) => {
            error!("LocalPipelineEngine init failed: {}, abort streaming", e);
            let _ = audio.stop();
            crate::ui::result_window::hide_result(app_handle);
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                        reset_mode_flags_on_start_failure();
            return;
        }
    };
    let pipeline = match StreamingPipeline::new(Box::new(local_engine)) {
        Ok(p) => p,
        Err(e) => {
            error!("StreamingPipeline init failed: {}, abort streaming", e);
            let _ = audio.stop();
            crate::ui::result_window::hide_result(app_handle);
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                        reset_mode_flags_on_start_failure();
            return;
        }
    };

    let streaming_active = Arc::new(AtomicBool::new(true));
    start_tick_thread(tx.clone(), streaming_active.clone());

    *stage = Stage::Streaming {
        pipeline,
        transcript,
        streaming_active,
    };
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_vad_segmented_session(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,
) {
    // 非流式模式：使用 VAD 伪流式分段识别（2c-3：编排收进 VadSegmentedPipeline）
    match crate::engine::pipeline::VadSegmentedPipeline::new(
        engine.clone(),
        config.language.clone(),
        active_asr_engine_name(),
        config.segment_silence,
    ) {
        Ok(pipeline) => {
            // 跨会话选中替换（同 streaming 路径）
            let tid = now_millis();
            set_current_transcription_id(tid);
            let (transcript, show_text, is_continuation) = if let Some((text, s, e)) = selection {
                let mut t = Transcript::new(tid, config.polish_mode, record_type.clone());
                t.commit_edit(&text, &[], true);
                t.set_selection(s, e);
                debug!("[select] cross-session seeded (vad) t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
                (t, text, true)
            } else {
                (Transcript::new(tid, config.polish_mode, record_type.clone()), "正在聆听…".to_string(), false)
            };
            if is_continuation {
                show_listening_start(app_handle, &show_text, true);
            } else {
                show_listening_start(app_handle, &show_text, false);
            }
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Recording);

            let tick_active = Arc::new(AtomicBool::new(true));
            start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());

            *stage = Stage::VadSegmented {
                pipeline,
                transcript,
                tick_active,
            };
        }
        Err(e) => {
            // 第三十四轮 P1：VAD init 失败必须完整清理（同 cloud 路径）。
            // 原注释 "falling back to offline" 误导——这里直接 abort，无 fallback。
            error!("VAD init failed for VadSegmented: {}, abort (no fallback)", e);
            let _ = audio.stop();
            crate::ui::result_window::hide_result(app_handle);
            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
            reset_mode_flags_on_start_failure();
        }
    }
}
