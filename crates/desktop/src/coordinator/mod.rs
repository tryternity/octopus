// coordinator/mod.rs — 录音生命周期协调器（actor 模式）。
// 2026-07-29 起拆分为子模块：本文件保留 types + 主循环 + tauri commands，
// 各 handler 函数搬到 coordinator/{paste,edit,tick,agent,cancel_discard,session,polish,lifecycle}.rs。

mod paste;
mod edit;
mod tick;
mod agent;
mod cancel_discard;

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::config::PolishMode;
use crate::db_queue::{DbCommand, get_db_sender};
use crate::engine::TranscriptionEngine;
use crate::pipeline::StreamingPipeline;
use crate::transcript::Transcript;
use octopus_asr_local::streaming_engine::StreamingSessionManager;
use octopus_asr_local::streaming_runner::TranscriptEvent;
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use tauri::{Emitter, Manager};

// 子模块函数 re-export：mod.rs 内的裸调用（now_millis / do_paste 等）零改动。
pub(crate) use self::paste::{
    now_millis, active_asr_engine_name, active_llm_name, sync_runtime_fields,
    stage_name, do_paste, update_transcription_raw,
};
pub(crate) use self::edit::{handle_enter_edit_mode, commit_edit_apply, stage_transcript};
#[cfg(feature = "cloud")]
pub(crate) use self::tick::is_cloud_engine;
pub(crate) use self::tick::{
    start_vad_segmented_tick_thread, start_tick_thread, dispatch_tick, log_tick_heartbeat,
};
#[cfg(feature = "cloud")]
pub(crate) use self::tick::start_cloud_streaming_tick_thread;
pub(crate) use self::agent::dispatch_by_record_type;
// retry_agent_task 保持 pub（action_bar_commands 跨 crate 引用 crate::coordinator::retry_agent_task）
pub use self::agent::retry_agent_task;
pub(crate) use self::cancel_discard::{handle_cancel, handle_discard};

/// 当前/最近一次录音会话的 transcription_id。
/// 在会话起点（Transcript::new）写入，供 Result 窗口「存入记事本」溯源。
/// 不在 mem::replace（id=0 sentinel）处清除 → 保留最近有效 id，粘贴后短时间内仍可保存。
pub(crate) static CURRENT_TRANSCRIPTION_ID: AtomicI64 = AtomicI64::new(0);

pub(crate) fn set_current_transcription_id(id: i64) {
    CURRENT_TRANSCRIPTION_ID.store(id, Ordering::Relaxed);
}

/// 翻译模式激活标志——前端 enterTranslateMode/exitTranslateMode 设置。
/// finalize_after_stop 读取此标志：true 时对最终文本做同步翻译，粘贴译文而非原文。
pub(crate) static TRANSLATION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 前端命令：设置翻译模式激活状态。
#[tauri::command]
pub fn set_translation_active(active: bool) {
    TRANSLATION_ACTIVE.store(active, Ordering::Relaxed);
}

/// Result 窗口取当前/最近 transcription_id（无会话返回 None）。
#[tauri::command]
pub async fn current_transcription_id() -> Option<i64> {
    let id = CURRENT_TRANSCRIPTION_ID.load(Ordering::Relaxed);
    if id > 0 { Some(id) } else { None }
}

/// 录音类型——决定录音结束后 finalize 的回调路径。
#[derive(Clone, Debug)]
pub enum RecordType {
    /// 普通语音输入 → paste/剪贴板
    Input,
    /// agent 桥接 → 录音结果作为 task 注入 agent 命令
    AgentBridge { task_id: String },
}

impl Default for RecordType {
    fn default() -> Self { RecordType::Input }
}

/// 协调器命令
pub(crate) enum Command {
    /// 切换录音状态（开始/停止）
    Toggle,
    /// 取消当前操作（丢弃一切，包括 DB 记录不 finalize）
    Cancel,
    /// 放弃当前识别：停止录音 + finalize DB 记录（保留历史），
    /// 但不粘贴、不入剪贴板。工具栏「关闭」按钮触发。
    Discard,
    /// 流式识别 tick（定时触发，驱动音频采集和识别）
    StreamingTick,
    /// VAD 伪流式 tick（300ms 间隔，驱动分段识别）
    VadSegmentedTick,
    /// 云端流式 tick（VAD-gated per-utterance streaming）
    #[cfg(feature = "cloud")]
    CloudStreamingTick,
    /// 云端 close_async 收尾完成（审查 三1）：非阻塞 close 的结果回传，
    /// handle_cloud_streaming_done 据此 finalize（set_full + append partial + paste）。
    /// session_id = 发起 close 时的 transcript.id，跨会话护栏用（见 handler）。
    #[cfg(feature = "cloud")]
    CloudStreamingDone { text: Result<String, String>, session_id: i64 },
    /// 粘贴完成
    PasteDone,
    /// 润色完成（session_id = 发起润色时的 transcript.id，跨会话护栏用，见 handle_polish_done）
    PolishDone { result: Result<String, String>, session_id: i64 },
    /// 最终润色完成（session_id = 发起润色时的 transcript.id，跨会话护栏用，见 handler）
    FinalPolishDone { result: Result<String, String>, session_id: i64 },
    /// 立即润色（前端工具栏触发，忽略 polish_mode）
    PolishNow,
    /// 进入编辑态（前端 edit_shortcut/编辑按钮触发；ASR 硬暂停）
    EnterEditMode,
    /// 提交编辑（含 dirty ranges——用户明确编辑过的区间 + 光标/选区恢复信息）
    CommitEdit { text: String, dirty_ranges: Vec<(usize, usize)>, has_edited: bool, caret: Option<usize>, selection: Option<(usize, usize)> },
    /// 运行时配置更新——外部（设置窗口 / 工具栏）修改 RuntimeConfig 后，
    /// 通过此命令通知 coordinator 立即把变更同步到 config 快照（无需等 Toggle）。
    /// 用于 polish_llm / polish_mode / asr_correct / output_simplified / hide_toolbar 等
    /// 运行时可变字段。`asr_engine` 不在此列（引擎实例已创建，需 Toggle 重建）。
    UpdateRuntime,
    /// 光标定位：前端非编辑态点击 → char offset → set_caret（劈段/段界）。
    /// 非活跃 stage（无 transcript）时 no-op。
    SetCaret { offset: usize },
    /// 选中替换：前端非编辑态拖选 → char 范围 [start,end) → set_selection（记录待删范围 +
    /// 劈 caret 到 start，不立即删字，首个 delta 到达时真删）。非活跃 stage → 暂存 (text,start,end)，
    /// Toggle 开新会话时种子 transcript（跨会话选中替换）。
    SetSelection { start: usize, end: usize },
    /// 前端响应 prepare-record 事件：携带 prepare_id（跨会话/超时护栏）+ 前端缓存的选区。
    /// selection=None → 普通开录音；Some((text,start,end)) → 跨会话选中替换种子。
    /// C3：coordinator 校验 prepare_id 匹配 pending_prepare 后调 begin_recording。
    StartRecording { prepare_id: i64, selection: Option<(String, usize, usize)>, record_type: RecordType },
    /// 看门狗超时兜底：prepare-record 发出后 200ms 前端未响应 → 普通开录音（selection=None）。
    FallbackStart { prepare_id: i64 },
    /// action bar agent 录音（跳过 prepare-record 两阶段，无 selection）
    StartAgentRecording { task_id: String },
    /// 音频采集看门狗触发：cpal 断推（samples=0 持续 ≥ STALL_THRESHOLD）→ 自动重连。
    /// 停采集 + 保留 transcript + 重建 pipeline + 重 start，窗口不隐藏。
    /// spec 2026-07-24-audio-watchdog §4.2。stage_kind 标识触发时的活跃 Stage 类型。
    RestartCapture { stage_kind: RestartStageKind },
}

/// `Command::RestartCapture` 携带的活跃 stage 类型标识（看门狗分发用，spec 2026-07-24 §4.2）。
/// 注：不含 WaitingCompletion——该 stage `is_recording` 已 false（stop 时翻转），
/// `sample_stall_duration` 返回 0，看门狗天然不触发。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartStageKind {
    Streaming,
    VadSegmented,
}

pub(crate) enum Stage {
    Idle,
    /// 流式识别：边录边识别
    Streaming {
        /// 流式 pipeline（持 StreamingRunner + 承载 set_full 文本更新，spec §3.4）。
        pipeline: crate::pipeline::StreamingPipeline,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
    /// VAD 伪流式：tick 驱动分段识别（非流式引擎使用，2c-3：编排收进 VadSegmentedPipeline）
    VadSegmented {
        /// VAD 分段 pipeline（封装双 VAD + 切段 + spawn + 乱序回填，2c-3）。
        pipeline: crate::pipeline::VadSegmentedPipeline,
        transcript: Transcript,
        /// tick 线程控制标志（move 进 WaitingCompletion，finalize 时才停，plan 细化）。
        tick_active: Arc<AtomicBool>,
    },
    /// 云端流式停止后等待最终结果（审查 三1）：close 改非阻塞，结果由
    /// Command::CloudStreamingDone 回传。期间持有 transcript + current_partial，
    /// 等待 close_async 收尾；Toggle/Cancel 在此阶段被忽略（busy closing）。
    #[cfg(feature = "cloud")]
    CloudClosing {
        transcript: Transcript,
        current_partial: String,
    },
    /// 等待所有识别完成（2c-3：复用 VadSegmented pipeline，靠 tick 线程继续驱动 drain rx）
    WaitingCompletion {
        /// VadSegmented pipeline（从 VadSegmented move 过来；tick 空样本 drain rx 收尾）。
        pipeline: crate::pipeline::VadSegmentedPipeline,
        transcript: Transcript,
        /// tick 线程标志（VadSegmented move 过来；finalize 时 store(false) 停线程）。
        tick_active: Arc<AtomicBool>,
    },
    /// Toggle 停止录音后，仍有进行中的立即润色（PolishNow 未返回）。
    /// 持有 transcript 等待 `Command::PolishDone` 到达，再按 polish_mode 决定后续路径。
    /// 修复 bug：原实现直接 `clear_polish_pending` + 走 final 路径，
    /// 导致立即润色结果被 stage 切换丢弃 + 最终润色因 polish_mode=0 跳过 → 只粘贴原文。
    StoppingPolish {
        transcript: Transcript,
    },
    /// 最终润色中
    Polishing {
        id: i64,
        raw_text: String,
        /// 段 JSON（落库 segments 列；最终润色后 paste → Finalize 用）
        segments: String,
        /// 最终润色失败时的兜底粘贴文本（= 停止时的 display，含编辑；成功时不用）
        fallback_text: String,
    },
    /// 粘贴中
    Pasting {
        /// 识别记录主键（Task 6 过程入库用）
        id: i64,
        /// 原生全文（入库用，不受编辑影响）
        raw_text: String,
        /// 段 JSON（落库 segments 列；PasteDone finalize 用）
        segments: String,
        /// 展示/入库的修正版（初始=润色结果，用户编辑会更新）
        polished_text: String,
        /// "off" | "done" | "failed"
        polish_status: String,
    },
}

/// VAD 伪流式 tick 间隔（毫秒）
pub(crate) const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 100;

/// 云端流式 tick 间隔（毫秒）
#[cfg(feature = "cloud")]
pub(crate) const CLOUD_STREAMING_TICK_INTERVAL_MS: u64 = 100;

/// 中间润色最小间隔下限（秒）：polish_mode=2 且 polish_min_interval<=0 时回退到此值，避免每 tick 刷爆 LLM。
pub(crate) const MIN_POLISH_INTERVAL_SEC: f64 = 1.0;

/// ASR 兜底引擎 spec（与 runtime_config::FALLBACK_ASR_ENGINE 一致，3-part 格式供 active_session 用）。
pub(crate) const FALLBACK_STREAMING_SPEC: &str = "local:zipformer:zipformer-small-ctc";

/// 录音生命周期协调器
/// 单线程串行化所有事件，消除竞态条件
///
/// `tx` is wrapped in `Mutex` to satisfy Tauri's `Send + Sync` requirement
/// for managed state.
pub struct Coordinator {
    tx: parking_lot::Mutex<Sender<Command>>,
}

/// 流式识别 tick 间隔（毫秒）
pub(crate) const STREAMING_TICK_INTERVAL_MS: u64 = 200;

/// 音频采集看门狗阈值：cpal 回调距上次收到样本 ≥ 此值 → 判定断推 → 自动重连。
/// spec 2026-07-24-audio-watchdog §3.1。正常静音 cpal 仍推底噪 samples≠0，故此阈值不误判静音。
/// VadSegmented 100ms tick × 30 = 3s；Streaming 200ms tick × 15 = 3s。
pub(crate) const AUDIO_STALL_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(3);

/// 过程落库（update_text_segments）节流间隔：同条记录 ≥此值才 UPDATE，
/// Finalize 兜底完整写入（长录音避免每 changed≈每 tick 一次 UPDATE 的写放大）。
pub(crate) const DB_FLUSH_INTERVAL_MS: u64 = 500;

impl Coordinator {
    pub fn new(
        engine: Arc<dyn TranscriptionEngine>,
        audio: Arc<SharedAudioState>,
        config: AppConfig,
        app_handle: tauri::AppHandle,
        runtime_config: crate::runtime_config::SharedRuntimeConfig,
    ) -> Self {
        let (tx, rx): (Sender<Command>, Receiver<Command>) = mpsc::channel();
        let tx_self = tx.clone();

        build_coordinator_loop(rx, tx, audio, engine, config, app_handle, runtime_config);

        Self {
            tx: parking_lot::Mutex::new(tx_self),
        }
    }
}

fn build_coordinator_loop(
    rx: Receiver<Command>,
    tx: Sender<Command>,
    audio: Arc<SharedAudioState>,
    engine: Arc<dyn TranscriptionEngine>,
    mut config: AppConfig,
    app_handle: tauri::AppHandle,
    runtime_config: crate::runtime_config::SharedRuntimeConfig,
) {
    let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine();
    let mut use_streaming = use_streaming;
    #[cfg(feature = "cloud")]
    let mut use_cloud_streaming = false;

    std::thread::spawn(move || {
            let mut stage = Stage::Idle;
            // 编辑态：置位时 tick 跳过喂引擎、只排空丢弃音频（硬暂停）。
            let mut editing = false;
            // 编辑缓冲：前端 input 防抖推送的最新文本；Toggle-期间-编辑 时用作提交文本。
            let mut edit_buffer: Option<String> = None;
            // Idle 态跨会话选中替换：前端拖选时暂存 (text, start, end)，
            // Toggle 开新会话时种子 transcript（保留旧文本 + 删选区 + 新词插入）。
            // C3 两阶段 Toggle：Idle→Toggle 进等待，存 prepare_id 校验前端 StartRecording / 看门狗
            // FallbackStart。前端 200ms 内回推选区→StartRecording；超时→FallbackStart 普通开。
            let mut pending_prepare: Option<i64> = None;
            // 诊断打点局部状态（spec 2026-07-19-asr-edit-stall-observability）：
            // - last_heartbeat / ticks_since_heartbeat：1Hz 节流 `[HEARTBEAT]`，证明 tick 线程在跑
            // - last_editing_logged：检测 editing 翻转，打 `[STATE]`（与 5 处精确触发点互补，
            //   覆盖 EnterEditMode/CommitEdit/Toggle/Cancel/Discard 之外的间接复位路径）
            let mut hb_last = std::time::Instant::now();
            let mut hb_ticks: u64 = 0;
            let mut last_editing_logged: Option<bool> = None;

            loop {
                let cmd = match rx.recv() {
                    Ok(c) => c,
                    Err(_) => {
                        debug!("Coordinator channel closed, exiting");
                        break;
                    }
                };

                match cmd {
                    Command::Toggle => {
                        // 编辑态下停止：先用 edit_buffer 提交编辑，再走停止流程（spec §7）
                        if editing {
                            if let Some(text) = edit_buffer.take() {
                                commit_edit_apply(&mut stage, &text, &[], false, None, None, &app_handle);
                            }
                            editing = false;
                            crate::perf_log::log(&format!(
                                "[STATE] toggle-during-edit committed then stopping (stage={})",
                                stage_name(&stage),
                            ));
                        }
                        if !matches!(stage, Stage::Idle) {
                            // 活跃录音态 → 停录音（handle_toggle 走非 Idle 分支）
                            handle_toggle(
                                &mut stage,
                                &audio,
                                &config,
                                &app_handle,
                                &tx,
                            );
                        } else if pending_prepare.is_some() {
                            // 等待态（已 emit prepare-record）再按 Toggle → 取消等待。
                            // 看门狗的 FallbackStart 到达时 prepare_id 不匹配被丢弃，不会重复开录音。
                            pending_prepare = None;
                            debug!("Toggle: cancel pending prepare (user re-press)");
                        } else {
                            // Idle → 两阶段开录音：sync runtime + emit prepare-record + spawn 200ms 看门狗。
                            // 前端 listen prepare-record 后回推 currentSelectionRef（或 null）→ StartRecording；
                            // 200ms 未响应 → FallbackStart 普通开（selection=None）。
                            let rc = runtime_config.read();
                            // Task 2 后：激活引擎统一从 ACTIVE_ENGINES 缓存取（resolve_active_engine），
                            // 不再从 rc.asr_engine 读 + 写回校正。
                            // mic/engine_mode 开新会话时从 rc 拉最新（下次录音生效；
                            // mic/引擎不能会话中热切）。
                            config.microphone = rc.microphone.clone();
                            config.engine_mode = rc.engine_mode.clone();
                            sync_runtime_fields(&mut config, &rc);
                            drop(rc);
                            use_streaming = config.engine_mode == "embedded"
                                && crate::config::is_streaming_engine();
                            #[cfg(feature = "cloud")]
                            {
                                use_cloud_streaming = is_cloud_engine(&config);
                                // 云端流式优先于本地流式
                                if use_cloud_streaming {
                                    use_streaming = false;
                                }
                            }
                            let prepare_id = now_millis();
                            pending_prepare = Some(prepare_id);
                            let _ = app_handle.emit("prepare-record", prepare_id);
                            // 看门狗：200ms 后若仍在等待态（前端未回推），发 FallbackStart 兜底普通开。
                            let tx_clone = tx.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                let _ = tx_clone.send(Command::FallbackStart { prepare_id });
                            });
                            debug!("Toggle: Idle → pending prepare_id={}", prepare_id);
                        }
                    }
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        log_tick_heartbeat(&stage, editing, &mut last_editing_logged, &mut hb_last, &mut hb_ticks);
                        if editing {
                            audio.trim_buffer(5.0); // 编辑期保留最后 5 秒音频（恢复后送 ASR，VAD 截静音）
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    #[cfg(feature = "cloud")]
                    Command::CloudStreamingTick => {
                        {
                            let rc = runtime_config.read();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        log_tick_heartbeat(&stage, editing, &mut last_editing_logged, &mut hb_last, &mut hb_ticks);
                        if editing {
                            audio.trim_buffer(5.0);
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. }
                        | Stage::WaitingCompletion { transcript, .. } = &mut stage
                        {
                            transcript.set_mode(config.polish_mode);
                        }
                        log_tick_heartbeat(&stage, editing, &mut last_editing_logged, &mut hb_last, &mut hb_ticks);
                        if editing {
                            audio.trim_buffer(5.0);
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    Command::Cancel => {
                        // 编辑态下 Esc 取消：清 editing/edit_buffer（防残留导致下一会话 tick 永久 drain_samples 静音）
                        if editing {
                            editing = false;
                            edit_buffer = None;
                            crate::perf_log::log("[STATE] cancel-during-edit cleared");
                        }
                        pending_prepare = None;
                        handle_cancel(&mut stage, &audio, &app_handle);
                    }
                    Command::Discard => {
                        if editing {
                            editing = false;
                            edit_buffer = None;
                            crate::perf_log::log("[STATE] discard-during-edit cleared");
                        }
                        pending_prepare = None;
                        handle_discard(&mut stage, &audio, &app_handle, &config);
                    }
                    Command::RestartCapture { stage_kind } => {
                        // 看门狗触发：cpal 断推 → 自动重连（spec 2026-07-24-audio-watchdog §4.2）。
                        // stage_kind 用于校验触发时的 stage 与当前一致（跨命令竞态防护）。
                        let kind_matches = match (&stage, stage_kind) {
                            (Stage::Streaming { .. }, RestartStageKind::Streaming) => true,
                            (Stage::VadSegmented { .. }, RestartStageKind::VadSegmented) => true,
                            _ => false,
                        };
                        if !kind_matches {
                            warn!("[WATCHDOG] stage mismatch (current={}, expected_kind={:?}), skip restart",
                                stage_name(&stage), stage_kind);
                        } else {
                            restart_capture_keep_transcript(
                                &mut stage, &audio, &engine, &config, &app_handle, &tx, use_streaming,
                            );
                        }
                    }
                    Command::PasteDone => {
                        // 入库 finalize（从 Pasting 取数据；用户编辑已反映到 polished_text）
                        if let Stage::Pasting {
                            id,
                            raw_text,
                            segments,
                            polished_text,
                            polish_status,
                        } = &stage
                        {
                            let polish_model: Option<String> = if polish_status == "done" {
                                let name = active_llm_name();
                                if name.is_empty() { None } else { Some(name) }
                            } else {
                                None
                            };
                            // polished_text 仅 done 时入库（spec §5.2：polished 仅 done 有值）
                            let polished_for_db = if polish_status == "done" {
                                Some(polished_text.as_str())
                            } else {
                                None
                            };
                            let duration_ms = now_millis() - id;
                            let cmd = DbCommand::Finalize {
                                id: *id,
                                raw_text: raw_text.clone(),
                                segments: segments.clone(),
                                polished_text: polished_for_db.map(|s| s.to_string()),
                                polish_status: polish_status.clone(),
                                polish_model: polish_model.clone(),
                                duration_ms: Some(duration_ms),
                            };
                            if let Err(e) = get_db_sender().send(cmd) {
                                warn!("Queue DB finalize failed: {}", e);
                            }
                        }
                        info!("Paste complete, returning to idle");
                        stage = Stage::Idle;
                        crate::result_window::clear_result(&app_handle);
                        crate::tray::update_tray_label(
                            &app_handle,
                            crate::tray::TrayState::Idle,
                        );
                    }
                    Command::PolishDone { result, session_id } => {
                        handle_polish_done(&mut stage, result, session_id, &config, &app_handle, &tx);
                    }
                    Command::FinalPolishDone { result, session_id } => {
                        handle_final_polish_done(&mut stage, result, session_id, &config, &app_handle, &tx);
                    }
                    #[cfg(feature = "cloud")]
                    Command::CloudStreamingDone { text, session_id } => {
                        handle_cloud_streaming_done(&mut stage, text, session_id, &config, &app_handle, &tx);
                    }
                    Command::PolishNow => {
                        handle_polish_now(&mut stage, &config, &app_handle, &tx);
                    }
                    Command::EnterEditMode => {
                        handle_enter_edit_mode(&mut stage, &mut editing, &mut edit_buffer);
                    }
                    Command::CommitEdit { text, dirty_ranges, has_edited, caret, selection } => {
                        crate::perf_log::log(&format!(
                            "[STATE] commit_edit stage={} text_len={} has_edited={}",
                            stage_name(&stage), text.chars().count(), has_edited,
                        ));
                        commit_edit_apply(&mut stage, &text, &dirty_ranges, has_edited, caret, selection, &app_handle);
                        editing = false;
                        // 诊断（spec 2026-07-19 第二轮）：commit_edit_apply 后 transcript 状态
                        // 与 transcript.rs 的 [CARET] commit_edit 打点互补——这里 stage 维度，
                        // 那里 transcript 字段维度
                    }
                    Command::UpdateRuntime => {
                        // 设置窗口 / 工具栏改了 RuntimeConfig 字段——立即同步到 config 快照，
                        // 无需等下次 Toggle。用于 polish_mode 等运行时可变字段。
                        // Task 2 后 polish_llm 不在 config（走 switch_active_model + ACTIVE_ENGINES 缓存）。
                        let rc = runtime_config.read();
                        sync_runtime_fields(&mut config, &rc);
                        debug!("UpdateRuntime: active llm='{}', polish_mode={:?}",
                               active_llm_name(), config.polish_mode);
                    }
                    Command::SetCaret { offset } => {
                        // 非编辑态点击定位光标：调 stage_transcript.set_caret（劈段/段界/clamp）。
                        // 非活跃 stage（Idle/Polishing/Pasting 等）→ no-op。等待态（pending_prepare）→ no-op。
                        if !editing && pending_prepare.is_none() {
                            if let Some(t) = stage_transcript(&mut stage) {
                                t.set_caret(offset);
                            }
                        }
                    }
                    Command::SetSelection { start, end, .. } => {
                        // 非编辑态拖选：活跃 stage → set_selection（记录待删，不立即删——
                        // 保留浏览器原生高亮，用户可重新选择）。Idle/等待态 → no-op（C5：跨会话
                        // 选中替换改由前端 currentSelectionRef 在 prepare-record 推回，不再后端缓存）。
                        if !editing && pending_prepare.is_none() {
                            if let Some(t) = stage_transcript(&mut stage) {
                                t.set_selection(start, end);
                            }
                        }
                    }
                    Command::StartRecording { prepare_id, selection, record_type } => {
                        // 前端响应 prepare-record：校验 prepare_id 匹配 pending_prepare 后开录音。
                        // 不匹配（跨会话/超时后迟到/重复）→ 丢弃，防重复开录音。
                        if pending_prepare == Some(prepare_id) {
                            pending_prepare = None;
                            info!("StartRecording prepare_id={} selection={}", prepare_id, selection.is_some());
                            begin_recording(
                                &mut stage,
                                &audio,
                                &engine,
                                &config,
                                &app_handle,
                                &tx,
                                use_streaming,
                                selection,
                                record_type,
                                #[cfg(feature = "cloud")]
                                use_cloud_streaming,
                            );
                        } else {
                            debug!(
                                "StartRecording discarded: prepare_id mismatch (incoming={}, pending={:?})",
                                prepare_id, pending_prepare
                            );
                        }
                    }
                    Command::FallbackStart { prepare_id } => {
                        // 看门狗 200ms 超时兜底：前端未响应 prepare-record → 普通开录音（selection=None）。
                        if pending_prepare == Some(prepare_id) {
                            pending_prepare = None;
                            warn!("FallbackStart prepare_id={} (frontend did not respond in 200ms)", prepare_id);
                            begin_recording(
                                &mut stage,
                                &audio,
                                &engine,
                                &config,
                                &app_handle,
                                &tx,
                                use_streaming,
                                None,
                                RecordType::Input,
                                #[cfg(feature = "cloud")]
                                use_cloud_streaming,
                            );
                        } else {
                            debug!(
                                "FallbackStart discarded: prepare_id mismatch (incoming={}, pending={:?})",
                                prepare_id, pending_prepare
                            );
                        }
                    }
                    Command::StartAgentRecording { task_id } => {
                        if !matches!(stage, Stage::Idle) {
                            warn!("StartAgentRecording ignored: not Idle, marking task failed");
                            let _ = octopus_infra::db::update_agent_task_status(&task_id, "failed", "录音正在进行中，无法启动 agent 录音");
                            // 不占用结果窗（会被流式 update_result 覆盖），改 emit 事件让前端弹 toast
                            let _ = app_handle.emit("agent-task://error", "录音正在进行中，请先停止");
                            continue;
                        }
                        let rc = runtime_config.read();
                        // Task 2 后：激活引擎统一从 ACTIVE_ENGINES 缓存取，不再写回 config。
                        config.microphone = rc.microphone.clone();
                        config.engine_mode = rc.engine_mode.clone();
                        sync_runtime_fields(&mut config, &rc);
                        drop(rc);
                        use_streaming = config.engine_mode == "embedded"
                            && crate::config::is_streaming_engine();
                        #[cfg(feature = "cloud")]
                        {
                            use_cloud_streaming = is_cloud_engine(&config);
                            if use_cloud_streaming { use_streaming = false; }
                        }
                        info!("StartAgentRecording: task_id={}", task_id);
                        begin_recording(
                            &mut stage, &audio, &engine, &config, &app_handle, &tx,
                            use_streaming, None,
                            RecordType::AgentBridge { task_id },
                            #[cfg(feature = "cloud")] use_cloud_streaming,
                        );
                    }
                }
            }
            debug!("Coordinator thread exited");
        });
}

impl Coordinator {
    /// 发送 toggle 命令
    pub fn toggle(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::Toggle).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 发送 cancel 命令
    pub fn cancel(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::Cancel).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 发送 discard 命令（放弃当前识别：停止录音 + 保留 DB 记录，不粘贴不入剪贴板）
    pub fn discard(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::Discard).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 发送立即润色命令（工具栏按钮触发，忽略 polish_mode）
    pub fn polish_now(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::PolishNow).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 进入编辑态
    pub fn enter_edit_mode(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::EnterEditMode).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// action bar agent 录音触发：创建 agent task → 开始录音
    pub fn start_agent_recording(&self, task_id: String) {
        let tx = self.tx.lock();
        if tx.send(Command::StartAgentRecording { task_id }).is_err() {
            error!("Coordinator channel closed");
        }
    }

    /// 提交编辑（含 dirty ranges + 光标/选区恢复）
    pub fn commit_edit(&self, text: String, dirty_ranges: Vec<(usize, usize)>, has_edited: bool, caret: Option<usize>, selection: Option<(usize, usize)>) {
        let tx = self.tx.lock();
        if tx.send(Command::CommitEdit { text, dirty_ranges, has_edited, caret, selection }).is_err() {
            error!("Coordinator channel closed");
        }
    }

    /// 前端点击光标定位：char offset → 通过命令通道投递（stage 在 spawn 线程，
    /// 主线程不持有）。命令循环里 handle_set_caret 调 stage_transcript.set_caret。
    pub fn set_caret(&self, offset: usize) {
        let tx = self.tx.lock();
            if tx.send(Command::SetCaret { offset }).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 前端拖选选中替换：char 范围 [start,end) → 通过命令通道投递。命令循环里调
    /// stage_transcript.set_selection（记录待删 + 劈 caret 到 start）。首个 delta 到达时真删。
    pub fn set_selection(&self, start: usize, end: usize) {
        let tx = self.tx.lock();
            if tx.send(Command::SetSelection { start, end }).is_err() {
                error!("Coordinator channel closed");
            }
    }

    /// 前端响应 prepare-record：回推选区（或 None=普通开录音）触发实际开录音。
    /// prepare_id 跨会话/超时护栏（看门狗的 FallbackStart 也带同 id，校验后丢弃迟到者）。
    pub fn start_recording(&self, prepare_id: i64, selection: Option<(String, usize, usize)>) {
        let tx = self.tx.lock();
        if tx.send(Command::StartRecording { prepare_id, selection, record_type: RecordType::Input }).is_err() {
            error!("Coordinator channel closed");
        }
    }

    /// 通知 coordinator 重读 RuntimeConfig 同步可变字段到 config 快照。
    /// 设置窗口 / 工具栏改完 RuntimeConfig 后调用，让 polish_llm 等字段立即生效。
    pub fn update_runtime(&self) {
        let tx = self.tx.lock();
            if tx.send(Command::UpdateRuntime).is_err() {
                error!("Coordinator channel closed");
            }
    }
}

/// 前端命令：取消当前录音/处理（Esc 键）。
/// 停止麦克风采集、重置状态机为 Idle、隐藏结果窗口、托盘置 Idle。
#[tauri::command]
pub fn cancel_recording(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.cancel();
}

/// 前端命令：放弃当前识别（工具栏「关闭」按钮）。
/// 停止录音 + finalize DB 记录（保留历史），但不粘贴、不入剪贴板。
#[tauri::command]
pub fn discard_recording(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.discard();
}

/// 前端命令：立即润色（工具栏按钮触发，忽略 polish_mode）。
#[tauri::command]
pub fn polish_now(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.polish_now();
}

/// 前端命令：进入编辑态（edit_shortcut/编辑按钮触发）。
#[tauri::command]
pub fn enter_edit_mode(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.enter_edit_mode();
}

/// 前端命令：更新编辑缓冲（input 防抖推送）。
/// 前端命令：提交编辑（含 dirty ranges——用户编辑过的区间 + 光标/选区恢复）。
#[tauri::command]
pub fn commit_edit(
    coordinator: tauri::State<'_, Coordinator>,
    text: String,
    dirty_ranges: Vec<(usize, usize)>,
    has_edited: bool,
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
) {
    coordinator.commit_edit(text, dirty_ranges, has_edited, caret, selection);
}

/// 前端命令：非编辑态点击文本 → 定位光标（后续流式从该处插入）。
/// offset = 点击处在整篇文本的 code-point（char）偏移。
#[tauri::command]
pub fn set_caret(coordinator: tauri::State<'_, Coordinator>, offset: usize) {
    coordinator.set_caret(offset);
}

/// 前端命令：非编辑态拖选文本 → 选中替换（首个 delta 到达时删除 [start,end) 并从 start 插入）。
/// start/end = 选区在整篇文本的 code-point（char）偏移（左闭右开）。
/// text = 当前展示全文（Idle 态跨会话选中替换用：种子新会话 transcript）。
#[tauri::command]
pub fn set_selection(coordinator: tauri::State<'_, Coordinator>, start: usize, end: usize) {
    coordinator.set_selection(start, end);
}

/// 前端命令：响应 prepare-record 事件，回推选区（selection=null=普通开录音）触发实际开录音。
/// prepare_id 由 prepare-record 事件携带，coordinator 校验后防跨会话/超时重复开录音。
#[tauri::command]
pub fn start_recording(
    coordinator: tauri::State<'_, Coordinator>,
    prepare_id: i64,
    selection: Option<(String, usize, usize)>,
) {
    coordinator.start_recording(prepare_id, selection);
}

/// 实际开录音：从 Idle 进入活跃录音态（cloud / streaming / vad 三分支）。
/// 抽自 handle_toggle 的 Idle 分支，供 C3 两阶段 Toggle 的 StartRecording / FallbackStart 复用。
/// selection = 跨会话选中替换种子（None=普通开录音；Some((text,start,end)) → 种子 transcript）。
#[allow(clippy::too_many_arguments)]
fn begin_recording(
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
fn prepare_cloud_streaming_session(
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
fn prepare_streaming_session(
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
fn prepare_vad_segmented_session(
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

/// 处理 Toggle 命令（仅活跃态停录音；Idle 走主循环两阶段 → begin_recording）。
fn handle_toggle(
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
fn restart_capture_keep_transcript(
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

/// 开始粘贴阶段（支持最终润色）。`transcript` 移交进 Pasting 持 id（Task 6 用）。
/// 开始最终润色或粘贴阶段（异步最终润色，防止阻塞协调器线程）。
fn start_final_polish_or_paste(
    stage: &mut Stage,
    text: &str,
    mut transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if text.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    match crate::config::llm_config(config.polish_mode) {
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
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);
            crate::result_window::show_result(app_handle, "⏳ 最终润色中...");

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

/// 云端流式 finalize：把未提交的 partial 拼进 transcript，空则回 Idle，
/// 否则走与本地引擎一致的「最终润色或粘贴」流程。
///
/// 审查 三1：从 stop 路径（无 session）与 CloudStreamingDone 路径（close 完成后）
/// 共用，避免 finalize 逻辑重复。`transcript` / `current_partial` 为 owned（已从
/// stage 移出），`stage: &mut Stage` 仅用于写回 Idle/Polishing/Pasting，无别名冲突。
#[cfg(feature = "cloud")]
fn finalize_cloud(
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
fn handle_cloud_streaming_done(
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

/// 处理最终润色完成事件。
///
/// 跨会话护栏（与 PolishDone 同理）：最终润色 1~3s 窗口内 Cancel+重开会话 →
/// 新 Polishing。旧会话迟到的 FinalPolishDone 会匹配到新 Polishing，用新 id +
/// 旧润色文本 do_paste → 跨会话污染。session_id（= 发起润色时的 transcript.id）
/// 校验：与当前 polishing id 不符则丢弃，不动当前 stage。
fn handle_final_polish_done(
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
fn spawn_polish_thread(
    input: crate::transcript::PolishInput,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
    session_id: i64,
) {
    // 段模型多段润色：Edited 段 preserve=true（LLM 原样保留），其余待润色。
    let regions = polish_input_to_regions(&input);
    let llm_config = if ignore_mode {
        crate::config::llm_config_ignore_mode()
    } else {
        crate::config::llm_config(config.polish_mode)
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
fn polish_input_to_regions(input: &crate::transcript::PolishInput) -> Vec<octopus_llm::PolishRegion> {
    input.segments.iter().map(|s| octopus_llm::PolishRegion {
        preserve: s.kind == crate::transcript::SegmentKind::Edited,
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
    crate::perf_log::log(&format!(
        "[POLISH] auto-trigger t={} silence={:.2} mode={:?} segs={}",
        transcript.id, silence_duration, config.polish_mode, input.segments.len(),
    ));
    spawn_polish_thread(input, config, tx, false, transcript.id);
}

/// 处理 PolishDone 命令：把润色结果写回 Transcript。
fn handle_polish_done(
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
            crate::perf_log::log(&format!(
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
                    crate::perf_log::log("[POLISH] done stage=StoppingPolish result=empty → on_polish_failed");
                } else {
                    transcript.polish_apply(&polished);
                    crate::perf_log::log(&format!(
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
                crate::perf_log::log(&format!(
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
            crate::perf_log::log(&format!(
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
        crate::perf_log::log(&format!(
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
                crate::perf_log::log(&format!(
                    "[POLISH] done stage={} result=empty → on_polish_failed", sname,
                ));
            } else {
                // 段模型回填（polish_apply 内部按 edited 串匹配定位 + 间隙 Polished）
                transcript.polish_apply(&polished);
                crate::perf_log::log(&format!(
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
                    crate::result_window::update_result(app_handle, &transcript.display_text(), false, 0);
                }
            }
        }
        Err(e) => {
            warn!("Polish failed: {}, keeping previous", e);
            use tauri::Emitter;
            let _ = app_handle.emit("polish-error", &e);
            transcript.on_polish_failed();
            crate::perf_log::log(&format!(
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
fn handle_polish_now(
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
            crate::perf_log::log(&format!(
                "[POLISH] PolishNow-ignored stage={} (no transcript)", stage_name(stage),
            ));
            let _ = app_handle.emit("polish-done", ());
            return;
        }
    };
    if transcript.full().is_empty() {
        debug!("PolishNow skipped: transcript empty");
        crate::perf_log::log("[POLISH] PolishNow-skipped reason=empty");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    if transcript.polish_pending() {
        debug!("PolishNow skipped: polish already pending");
        crate::perf_log::log("[POLISH] PolishNow-skipped reason=already_pending");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    // 检查 LLM 配置是否存在（忽略 polish_mode，立即润色不看 mode）
    if crate::config::llm_config_ignore_mode().is_none() {
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
    crate::perf_log::log(&format!(
        "[POLISH] PolishNow-manual-trigger t={} chars={}",
        transcript.id,
        input.segments.iter().map(|s| s.text.chars().count()).sum::<usize>(),
    ));
    info!("PolishNow triggered, polishing {} chars", input.segments.iter().map(|s| s.text.chars().count()).sum::<usize>());
    spawn_polish_thread(input, config, tx, true, transcript.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TRANSLATION_ACTIVE 的 swap(false) 消费语义：首次读取 true 后自动归零，
    /// 确保多次 do_paste 调用只翻译一次（润色重试 / 跨阶段等场景）。
    #[test]
    fn translation_active_swap_consumes_once() {
        let flag = AtomicBool::new(true);
        // 首次 swap → 取回 true，flag 变 false
        assert!(flag.swap(false, Ordering::Relaxed));
        // 再次 swap → 取回 false（已消费）
        assert!(!flag.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn translation_active_default_false() {
        let flag = AtomicBool::new(false);
        assert!(!flag.swap(false, Ordering::Relaxed));
    }

    #[test]
    fn translation_active_set_and_clear() {
        let flag = AtomicBool::new(false);
        flag.store(true, Ordering::Relaxed);
        assert!(flag.swap(false, Ordering::Relaxed), "set true 后首次 swap 应为 true");
        flag.store(false, Ordering::Relaxed);
        assert!(!flag.swap(false, Ordering::Relaxed), "store false 后 swap 应为 false");
    }

    // ── RecordType ──

    #[test]
    fn record_type_default_is_input() {
        let rt = RecordType::default();
        assert!(matches!(rt, RecordType::Input));
    }

    #[test]
    fn record_type_agent_bridge_carries_task_id() {
        let rt = RecordType::AgentBridge { task_id: "abc-123".into() };
        match rt {
            RecordType::AgentBridge { task_id } => assert_eq!(task_id, "abc-123"),
            _ => panic!("应匹配 AgentBridge"),
        }
    }

    #[test]
    fn record_type_clone_preserves_task_id() {
        let rt = RecordType::AgentBridge { task_id: "xyz".into() };
        let rt2 = rt.clone();
        match rt2 {
            RecordType::AgentBridge { task_id } => assert_eq!(task_id, "xyz"),
            _ => panic!("clone 应保持变体"),
        }
    }

}

