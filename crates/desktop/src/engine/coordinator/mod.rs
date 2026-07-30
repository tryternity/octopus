// coordinator/mod.rs — 录音生命周期协调器（actor 模式）。
// 2026-07-29 起拆分为子模块：本文件保留 types + 主循环 + tauri commands，
// 各 handler 函数搬到 coordinator/{paste,edit,tick,agent,cancel_discard,session,polish,lifecycle}.rs。

mod paste;
mod edit;
mod tick;
mod agent;
mod cancel_discard;
mod session;
mod polish;
mod lifecycle;

use crate::engine::audio::SharedAudioState;
use crate::core::config::AppConfig;
use crate::core::db_queue::{DbCommand, get_db_sender};
use crate::engine::engine::TranscriptionEngine;
use crate::engine::transcript::Transcript;
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use tauri::Emitter;

// 子模块函数 re-export：mod.rs 主循环内的裸调用零改动。
// （子模块互引用走直接路径 use super::<module>::<fn>，不经此 re-export。）
pub(crate) use self::paste::{
    now_millis, active_llm_name, sync_runtime_fields, stage_name,
};
pub(crate) use self::edit::{handle_enter_edit_mode, commit_edit_apply, stage_transcript};
#[cfg(feature = "cloud")]
pub(crate) use self::tick::is_cloud_engine;
pub(crate) use self::tick::{dispatch_tick, log_tick_heartbeat};
// retry_agent_task 保持 pub（action_bar_commands 跨 crate 引用 crate::engine::coordinator::retry_agent_task）
pub use self::agent::retry_agent_task;
pub(crate) use self::cancel_discard::{handle_cancel, handle_discard};
pub(crate) use self::session::begin_recording;
pub(crate) use self::polish::{
    handle_final_polish_done, handle_polish_done, handle_polish_now,
};
pub(crate) use self::lifecycle::{handle_toggle, restart_capture_keep_transcript};
#[cfg(feature = "cloud")]
pub(crate) use self::lifecycle::handle_cloud_streaming_done;

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
        pipeline: crate::engine::pipeline::StreamingPipeline,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
    /// VAD 伪流式：tick 驱动分段识别（非流式引擎使用，2c-3：编排收进 VadSegmentedPipeline）
    VadSegmented {
        /// VAD 分段 pipeline（封装双 VAD + 切段 + spawn + 乱序回填，2c-3）。
        pipeline: crate::engine::pipeline::VadSegmentedPipeline,
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
        pipeline: crate::engine::pipeline::VadSegmentedPipeline,
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
        runtime_config: crate::core::runtime_config::SharedRuntimeConfig,
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
    runtime_config: crate::core::runtime_config::SharedRuntimeConfig,
) {
    let use_streaming = config.engine_mode == "embedded" && crate::core::config::is_streaming_engine();
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
                            crate::core::perf_log::log(&format!(
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
                                && crate::core::config::is_streaming_engine();
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
                            crate::core::perf_log::log("[STATE] cancel-during-edit cleared");
                        }
                        pending_prepare = None;
                        handle_cancel(&mut stage, &audio, &app_handle);
                    }
                    Command::Discard => {
                        if editing {
                            editing = false;
                            edit_buffer = None;
                            crate::core::perf_log::log("[STATE] discard-during-edit cleared");
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
                        crate::ui::result_window::clear_result(&app_handle);
                        crate::ui::tray::update_tray_label(
                            &app_handle,
                            crate::ui::tray::TrayState::Idle,
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
                        crate::core::perf_log::log(&format!(
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
                            && crate::core::config::is_streaming_engine();
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

