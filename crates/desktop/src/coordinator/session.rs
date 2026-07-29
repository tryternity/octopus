//! 录音会话建立（从 coordinator/mod.rs 提取，Task 2.1）。
//!
//! `begin_recording` 是开录音总入口（cloud / streaming / vad 三分支分发），
//! 抽自 `handle_toggle` 的 Idle 分支，供 C3 两阶段 Toggle 的 StartRecording / FallbackStart 复用。
//! 三个 `prepare_*_session` 分别构造对应 pipeline + transcript + 启动 tick 线程，进入活跃 Stage。

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::engine::TranscriptionEngine;
use crate::pipeline::StreamingPipeline;
use crate::transcript::Transcript;
use octopus_asr_local::streaming_engine::StreamingSessionManager;
use log::{debug, error, info, warn};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use super::{Command, Stage, RecordType, FALLBACK_STREAMING_SPEC, set_current_transcription_id};
use super::paste::{now_millis, active_asr_engine_name};
use super::tick::{start_tick_thread, start_vad_segmented_tick_thread};
#[cfg(feature = "cloud")]
use super::tick::start_cloud_streaming_tick_thread;

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
        crate::result_window::show_result(app_handle, "");
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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
            crate::pipeline::vad_preroll(&mut vad);

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
                    crate::result_window::show_result(app_handle, "正在聆听…");
                    crate::result_window::update_result(app_handle, &show_text, false, 0);
                } else {
                    crate::result_window::show_result(app_handle, &show_text);
                }
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);

                let cloud_engine = crate::cloud_pipeline::CloudPipelineEngine::new(
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
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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
            error!("VAD init failed for cloud streaming: {}, falling back to VadSegmented", e);
            let _ = audio.stop();
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
                    crate::result_window::hide_result(app_handle);
                    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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
        crate::result_window::show_result(app_handle, "正在聆听…");
        crate::result_window::update_result(app_handle, &show_text, false, 0);
    } else {
        crate::result_window::show_result(app_handle, &show_text);
    }
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);

    // StreamingPipeline 内部构造 StreamingRunner（VAD + 预热，阶段2a/2b）
    let local_engine = match crate::pipeline::LocalPipelineEngine::from_session(streaming_engine, false) {
        Ok(e) => e,
        Err(e) => {
            error!("LocalPipelineEngine init failed: {}, abort streaming", e);
            let _ = audio.stop();
            crate::result_window::hide_result(app_handle);
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
            return;
        }
    };
    let pipeline = match StreamingPipeline::new(Box::new(local_engine)) {
        Ok(p) => p,
        Err(e) => {
            error!("StreamingPipeline init failed: {}, abort streaming", e);
            let _ = audio.stop();
            crate::result_window::hide_result(app_handle);
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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
    match crate::pipeline::VadSegmentedPipeline::new(
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
                crate::result_window::show_result(app_handle, "正在聆听…");
                crate::result_window::update_result(app_handle, &show_text, false, 0);
            } else {
                crate::result_window::show_result(app_handle, &show_text);
            }
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);

            let tick_active = Arc::new(AtomicBool::new(true));
            start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());

            *stage = Stage::VadSegmented {
                pipeline,
                transcript,
                tick_active,
            };
        }
        Err(e) => {
            error!("VAD init failed for VadSegmented: {}, falling back to offline", e);
            let _ = audio.stop();
        }
    }
}
