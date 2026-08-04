//! tick 线程 + pipeline 事件路由 + 看门狗（从 coordinator/mod.rs 提取，Task 1.3）。
//!
//! 三类 tick 线程（Streaming / VadSegmented / CloudStreaming）定时发 Command 驱动主循环；
//! `dispatch_tick` 是三命令合一的入口，调 pipeline.tick → `apply_pipeline_events` 统一路由；
//! `check_audio_stall` 是 cpal 断推看门狗（spec 2026-07-24-audio-watchdog §4.1）。

use crate::engine::audio::SharedAudioState;
use crate::core::config::AppConfig;
use crate::core::config::PolishMode;
use crate::engine::transcript::Transcript;
use log::{debug, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tauri::Emitter;
use super::{
    Command, Stage, RecordType, RestartStageKind,
    AUDIO_STALL_THRESHOLD,
    STREAMING_TICK_INTERVAL_MS,
    VAD_SEGMENTED_TICK_INTERVAL_MS,
    recording_mode,
};
#[cfg(feature = "cloud")]
use super::CLOUD_STREAMING_TICK_INTERVAL_MS;
// 通用工具 + 尚未搬出的 handler（仍在 mod.rs，pub(crate) 可见）
use super::paste::{stage_name, update_transcription_raw, active_asr_engine_name};
use super::polish::check_and_trigger_polish;
use super::lifecycle::finalize_after_stop;

/// hands-free 模式静音自动停止阈值（秒）。spec 2026-07-31 单键三模式。
///
/// hands-free 常驻录音期间，VAD 累积静音 ≥ 此值 → 自动发 `Command::HandsFreeStop`
/// （等价于用户按键停止）。避免用户开了 hands-free 后忘了关，一直占着麦。
pub(crate) const HANDS_FREE_SILENCE_TIMEOUT_SECS: f64 = 10.0;

/// 启动 VAD 伪流式 tick 线程
pub(crate) fn start_vad_segmented_tick_thread(tx: Sender<Command>, tick_active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while tick_active.load(Ordering::Relaxed) {
            if tick_active.load(Ordering::Relaxed)
                && tx.send(Command::VadSegmentedTick).is_err() {
                    break;
                }
            std::thread::sleep(std::time::Duration::from_millis(VAD_SEGMENTED_TICK_INTERVAL_MS));
        }
        debug!("VadSegmented tick thread exited");
    });
}

/// 判定激活 ASR 引擎是否为云端引擎（Aliyun、ByteDance、Tencent 或 Baidu）。
#[cfg(feature = "cloud")]
pub(crate) fn is_cloud_engine(_config: &AppConfig) -> bool {
    use octopus_asr_local::config::EngineCategory;
    let cat = octopus_asr_local::config::resolve_active_engine("asr")
        .ok()
        .and_then(|r| r.as_engine_category());
    matches!(
        cat,
        Some(EngineCategory::Aliyun)
            | Some(EngineCategory::ByteDance)
            | Some(EngineCategory::Tencent)
            | Some(EngineCategory::Baidu)
    )
}

/// 启动云端流式 tick 线程（首 tick 立即触发）
#[cfg(feature = "cloud")]
pub(crate) fn start_cloud_streaming_tick_thread(tx: Sender<Command>, tick_active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while tick_active.load(Ordering::Relaxed) {
            if tick_active.load(Ordering::Relaxed)
                && tx.send(Command::CloudStreamingTick).is_err()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(CLOUD_STREAMING_TICK_INTERVAL_MS));
        }
        debug!("CloudStreaming tick thread exited");
    });
}

/// 启动 tick 线程，定时发送 StreamingTick 命令
pub(crate) fn start_tick_thread(tx: Sender<Command>, streaming_active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while streaming_active.load(Ordering::Relaxed) {
            if streaming_active.load(Ordering::Relaxed)
                && tx.send(Command::StreamingTick).is_err() {
                    break;
                }
            std::thread::sleep(std::time::Duration::from_millis(STREAMING_TICK_INTERVAL_MS));
        }
        debug!("Streaming tick thread exited");
    });
}

/// tick 线程诊断打点（spec 2026-07-19-asr-edit-stall-observability）：
/// - 检测 `editing` 翻转（覆盖 5 处精确触发点之外的间接复位路径），翻转即打 `[STATE]`
/// - 距上次心跳 ≥ 1s 打 `[HEARTBEAT]`（1Hz 节流），证明 tick 线程在跑 + 当前 stage/editing
///
/// 调用方：三个 Tick 分支（StreamingTick / VadSegmentedTick / CloudStreamingTick）入口。
pub(crate) fn log_tick_heartbeat(
    stage: &Stage,
    editing: bool,
    last_editing_logged: &mut Option<bool>,
    hb_last: &mut std::time::Instant,
    hb_ticks: &mut u64,
) {
    // editing 翻转即打（错过快速 enter→commit 是要避免的，心跳节流兜底，但翻转必须立即落）
    if *last_editing_logged != Some(editing) {
        crate::core::perf_log::log(&format!(
            "[STATE] editing {} -> {} (stage={})",
            last_editing_logged.map(|b| b.to_string()).unwrap_or_else(|| "—".into()),
            editing,
            stage_name(stage),
        ));
        *last_editing_logged = Some(editing);
    }
    *hb_ticks += 1;
    if hb_last.elapsed() >= std::time::Duration::from_secs(1) {
        crate::core::perf_log::log(&format!(
            "[HEARTBEAT] stage={} editing={} ticks_in_window={}",
            stage_name(stage), editing, hb_ticks,
        ));
        *hb_last = std::time::Instant::now();
        *hb_ticks = 0;
    }
}

/// pipeline 事件 → 端动作（DB/emit/polish/错误上报）。2d 统一路由，消除三路径重复。（spec §3.5）
pub(crate) fn apply_pipeline_events(
    events: Vec<crate::engine::pipeline::PipelineEvent>,
    transcript: &mut Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    use crate::engine::pipeline::PipelineEvent;
    for ev in events {
        match ev {
            PipelineEvent::PersistRaw { engine_mode } => {
                if let Err(e) = update_transcription_raw(transcript, &active_asr_engine_name(), engine_mode) {
                    warn!("DB ({}) failed: {}", engine_mode, e);
                }
            }
            PipelineEvent::Emit { display, insertion, caret } => {
                // 把 pipeline 的 insertion 标志 + caret 偏移实传给 result_window（前端跳过 diverted 300ms 延迟
                // 立即渲染；insertion=true 时用 caret 定位闪烁光标，使其跟在最后插入的文字后右移）。
                if !display.is_empty() {
                    // 流式实时标记 Hotwords 段：drain corrector 多命中候选 → mark_hotwords。
                    let candidates = octopus_asr_local::corrector::drain_candidates();
                    if !candidates.is_empty() {
                        transcript.mark_hotwords(&candidates);
                    }
                    // 有 Hotwords 段（新标记或已有）→ 传 segments 保留下拉装饰。
                    // 无新候选时若已标 Hotwords 段，也须传——否则前端清空 segments state 装饰消失。
                    let segs = if transcript.has_hotwords() {
                        Some(transcript.segments_json())
                    } else {
                        None
                    };
                    crate::ui::result_window::update_result(app_handle, &display, insertion, caret, segs.as_deref());
                }
            }
            PipelineEvent::Polish { silence } => {
                check_and_trigger_polish(transcript, silence, config, tx);
            }
            PipelineEvent::Error(e) => {
                crate::ui::result_window::update_result(app_handle, &e, false, 0, None);
            }
            PipelineEvent::Speaking(speaking) => {
                crate::core::perf_log::log(&format!("[SPEAKING] emit {}", speaking));
                let _ = app_handle.emit("update-speaking", speaking);
            }
        }
    }
}

/// VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令合一的 dispatch（2d，spec §3.5）。
/// 各 Stage 变体调对应 pipeline 的 `tick` → `apply_pipeline_events` 统一路由。
/// WaitingCompletion 额外做 active_count==0 收尾判定（沿用 2c-3 既有逻辑）。
pub(crate) fn dispatch_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let samples = audio.drain_samples();
    match stage {
        Stage::Streaming { pipeline, transcript, .. } => {
            let events = pipeline.tick(&samples, transcript);
            // hands-free 静音超时：流式引擎也要检测（hands-free 可能用 streaming）。
            let hf_silence = if recording_mode() == 3 {
                Some(pipeline.silence_duration())
            } else {
                None
            };
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            // 看门狗：cpal 断推检测（spec 2026-07-24-audio-watchdog §4.1）。
            // WaitingCompletion 天然免疫（is_recording 已 false → stall=0）。
            if check_audio_stall(audio, stage) {
                let _ = tx.send(Command::RestartCapture { stage_kind: RestartStageKind::Streaming });
            }
            // hands-free 静音超时（同 VadSegmented 分支）。
            if let Some(sil) = hf_silence {
                if sil >= HANDS_FREE_SILENCE_TIMEOUT_SECS {
                    warn!(
                        "[hands-free] silence {:.1}s ≥ {}s (streaming), auto-stop",
                        sil, HANDS_FREE_SILENCE_TIMEOUT_SECS
                    );
                    let _ = tx.send(Command::HandsFreeStop);
                }
            }
        }
        Stage::VadSegmented { pipeline, transcript, .. } => {
            let events = pipeline.tick(&samples, transcript);
            // 读 silence_duration 放进局部变量，避免下方 check_audio_stall(stage) 与
            // pipeline 借用冲突（pipeline 是 &mut，stage 是 &，不可同时持有）。
            let hf_silence = if recording_mode() == 3 {
                Some(pipeline.silence_duration())
            } else {
                None
            };
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            if check_audio_stall(audio, stage) {
                let _ = tx.send(Command::RestartCapture { stage_kind: RestartStageKind::VadSegmented });
            }
            // hands-free 静音超时：常驻录音忘了关 → 自动停（spec 2026-07-31）。
            if let Some(sil) = hf_silence {
                if sil >= HANDS_FREE_SILENCE_TIMEOUT_SECS {
                    warn!(
                        "[hands-free] silence {:.1}s ≥ {}s, auto-stop",
                        sil, HANDS_FREE_SILENCE_TIMEOUT_SECS
                    );
                    let _ = tx.send(Command::HandsFreeStop);
                }
            }
        }
        Stage::WaitingCompletion { pipeline, transcript, tick_active } => {
            let events = pipeline.tick(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            // 所有在途段完成 → 收尾（停 tick 线程 + finalize）
            if pipeline.active_count() == 0 {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled, RecordType::Input));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
            // WaitingCompletion 不检测看门狗：is_recording 已 false（stop 时翻转），
            // sample_stall_duration 必返回 0；且此时本就在等在途段完成，不需要重连。
        }
        _ => {
            // tick 到达但 stage 不是活跃识别态——通常是异常路径（如 Polishing/Pasting 阶段
            // 还收到 Tick），打点帮助诊断"绿条为何不亮"是不是因为 stage 漂移。
            crate::core::perf_log::log(&format!(
                "[WARN] dispatch_tick stage={} not active, tick dropped (samples_drained={})",
                stage_name(stage),
                samples.len(),
            ));
        }
    }
}

/// 看门狗纯判定：audio stall 是否超阈值（spec 2026-07-24-audio-watchdog §4.1）。
/// 抽出为独立函数便于单测。命中时由调用方发 `Command::RestartCapture`。
pub(crate) fn check_audio_stall(audio: &Arc<SharedAudioState>, stage: &Stage) -> bool {
    let stall = audio.sample_stall_duration();
    if stall >= AUDIO_STALL_THRESHOLD {
        crate::core::perf_log::log(&format!(
            "[WATCHDOG] stall={:.1}s threshold={:.0}s stage={} samples_buffer=0 → restart",
            stall.as_secs_f64(), AUDIO_STALL_THRESHOLD.as_secs_f64(), stage_name(stage),
        ));
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 音频采集看门狗（spec 2026-07-24-audio-watchdog §4.1）──
    // check_audio_stall 是 sample_stall_duration() >= 阈值的薄封装 + 日志。
    // sample_stall_duration 的 4 种情形（未录/冷启动/断推/正常）在 audio.rs 测试模块覆盖。
    // 此处仅测不录音时的不触发（跨模块无法访问 audio 私有字段设置 stall 状态）。

    #[test]
    fn check_audio_stall_no_trigger_when_not_recording() {
        // is_recording=false → sample_stall_duration 返回 0 < 阈值 → 不触发
        let audio = Arc::new(SharedAudioState::new("test"));
        let stage = Stage::Idle;
        assert!(!check_audio_stall(&audio, &stage));
    }
}
