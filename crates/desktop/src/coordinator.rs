// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::config::PolishMode;
use crate::db_queue::{DbCommand, get_db_sender};
use crate::engine::TranscriptionEngine;
use crate::paste;
use crate::pipeline::StreamingPipeline;
use crate::transcript::Transcript;
use octopus_asr_local::streaming_engine::StreamingSessionManager;
use octopus_asr_local::streaming_runner::TranscriptEvent;
use log::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use tauri::{Emitter, Manager};

/// 当前/最近一次录音会话的 transcription_id。
/// 在会话起点（Transcript::new）写入，供 Result 窗口「存入记事本」溯源。
/// 不在 mem::replace（id=0 sentinel）处清除 → 保留最近有效 id，粘贴后短时间内仍可保存。
static CURRENT_TRANSCRIPTION_ID: AtomicI64 = AtomicI64::new(0);

pub(crate) fn set_current_transcription_id(id: i64) {
    CURRENT_TRANSCRIPTION_ID.store(id, Ordering::Relaxed);
}

/// Result 窗口取当前/最近 transcription_id（无会话返回 None）。
#[tauri::command]
pub async fn current_transcription_id() -> Option<i64> {
    let id = CURRENT_TRANSCRIPTION_ID.load(Ordering::Relaxed);
    if id > 0 { Some(id) } else { None }
}

/// 协调器命令
enum Command {
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
    StartRecording { prepare_id: i64, selection: Option<(String, usize, usize)> },
    /// 看门狗超时兜底：prepare-record 发出后 200ms 前端未响应 → 普通开录音（selection=None）。
    FallbackStart { prepare_id: i64 },
}

enum Stage {
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
const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 100;

/// 云端流式 tick 间隔（毫秒）
#[cfg(feature = "cloud")]
const CLOUD_STREAMING_TICK_INTERVAL_MS: u64 = 100;

/// 中间润色最小间隔下限（秒）：polish_mode=2 且 polish_min_interval<=0 时回退到此值，避免每 tick 刷爆 LLM。
pub(crate) const MIN_POLISH_INTERVAL_SEC: f64 = 1.0;

/// 当前 Unix 毫秒时间戳（作 Transcript id / DB 主键）。
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 录音生命周期协调器
/// 单线程串行化所有事件，消除竞态条件
///
/// `tx` is wrapped in `Mutex` to satisfy Tauri's `Send + Sync` requirement
/// for managed state.
pub struct Coordinator {
    tx: parking_lot::Mutex<Sender<Command>>,
}

/// 流式识别 tick 间隔（毫秒）
const STREAMING_TICK_INTERVAL_MS: u64 = 200;

/// 过程落库（update_text_segments）节流间隔：同条记录 ≥此值才 UPDATE，
/// Finalize 兜底完整写入（长录音避免每 changed≈每 tick 一次 UPDATE 的写放大）。
const DB_FLUSH_INTERVAL_MS: u64 = 500;

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
    let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine(&config);
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
                            config.asr_engine = match octopus_asr_local::config::resolve_active_engine(&rc.asr_engine) {
                                Ok(_) => rc.asr_engine.clone(),
                                Err(_) => "local:zipformer:zipformer-small-ctc".to_string(),
                            };
                            // mic/engine_mode 开新会话时从 rc 拉最新（与 asr_engine 同策略，下次录音生效；
                            // mic/引擎不能会话中热切）。
                            config.microphone = rc.microphone.clone();
                            config.engine_mode = rc.engine_mode.clone();
                            sync_runtime_fields(&mut config, &rc);
                            drop(rc);
                            use_streaming = config.engine_mode == "embedded"
                                && crate::config::is_streaming_engine(&config);
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
                        }
                        pending_prepare = None;
                        handle_cancel(&mut stage, &audio, &app_handle);
                    }
                    Command::Discard => {
                        if editing {
                            editing = false;
                            edit_buffer = None;
                        }
                        pending_prepare = None;
                        handle_discard(&mut stage, &audio, &app_handle, &config);
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
                            let polish_model = if polish_status == "done" {
                                Some(config.polish_llm.as_str())
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
                                polish_model: polish_model.map(|s| s.to_string()),
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
                        commit_edit_apply(&mut stage, &text, &dirty_ranges, has_edited, caret, selection, &app_handle);
                        editing = false;
                    }
                    Command::UpdateRuntime => {
                        // 设置窗口 / 工具栏改了 RuntimeConfig 字段——立即同步到 config 快照，
                        // 无需等下次 Toggle。用于 polish_llm 等运行时可变字段。
                        // asr_engine 不在此路径（需要重建引擎实例，必须走 Toggle）。
                        let rc = runtime_config.read();
                        sync_runtime_fields(&mut config, &rc);
                        debug!("UpdateRuntime: polish_llm='{}', polish_mode={:?}",
                               config.polish_llm, config.polish_mode);
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
                    Command::StartRecording { prepare_id, selection } => {
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
        if tx.send(Command::StartRecording { prepare_id, selection }).is_err() {
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

/// 把共享 AppConfig 的运行时可变字段同步到 coordinator 的 config 快照。
///
/// 与 Toggle 时的同步逻辑共用，确保两条路径同步内容一致。
/// 不含 `asr_engine`（需重建引擎实例，只能 Toggle 时切），也不含 `denoise_mode`
/// （音频处理路径有独立 cfg 读取，会话中切换影响降噪器状态）。
fn sync_runtime_fields(config: &mut AppConfig, shared: &AppConfig) {
    config.polish_mode = shared.polish_mode;
    config.polish_llm = shared.polish_llm.clone();
    config.asr_correct = shared.asr_correct;
    config.output_simplified = shared.output_simplified;
    config.hide_toolbar = shared.hide_toolbar;
    config.edit_shortcut = shared.edit_shortcut.clone();
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
        return;
    }

    #[cfg(feature = "cloud")]
    if use_cloud_streaming {
        prepare_cloud_streaming_session(stage, audio, config, app_handle, tx, selection);
        return;
    }

    if use_streaming {
        prepare_streaming_session(stage, audio, engine, config, app_handle, tx, selection);
    } else {
        prepare_vad_segmented_session(stage, audio, engine, config, app_handle, tx, selection);
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
) {
    match octopus_asr_local::config::find_silero_vad() {
        Ok(path) => match octopus_asr_local::vad::SileroVad::new(&path) {
            Ok(mut vad) => {
                crate::pipeline::vad_preroll(&mut vad);

                // 跨会话选中替换：有 selection → 种子 transcript（保留旧文本 + 删选区）。
                // cloud 与本地 streaming/vad 共用 Stage::Streaming + Transcript，下游 paste 由
                // pending_delete 驱动（首个 delta → delete_range），三条路径必须对称植入，否则 cloud 退化为追加。
                let tid = now_millis();
                set_current_transcription_id(tid);
                let (transcript, show_text, is_continuation) = if let Some((text, s, e)) = selection {
                    let mut t = Transcript::new(tid, config.polish_mode);
                    t.commit_edit(&text, &[], true);
                    t.set_selection(s, e);
                    debug!("[select] cross-session seeded (cloud) t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
                    (t, text, true)
                } else {
                    (Transcript::new(tid, config.polish_mode), "正在聆听…".to_string(), false)
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
                    config.asr_engine.clone(),
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
        },
        Err(e) => {
            error!("VAD not found for cloud streaming: {}, falling back to VadSegmented", e);
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
) {
    // 流式引擎复用（②）：从 StreamingSessionManager 取常驻引擎 Arc + reset 清状态，
    // 不再每次录音 StreamingSession::new 重载 Session。模型变更由 active_session 懒加载覆盖，
    // 故 switch_asr_engine 无需主动联动。streaming_manager 经 app_handle.state 取（main 注入）。
    const FALLBACK_STREAMING_SPEC: &str = "local:zipformer:zipformer-small-ctc";
    let streaming_manager = app_handle
        .state::<std::sync::Arc<StreamingSessionManager>>();
    let streaming_engine = match streaming_manager
        .active_session(&config.asr_engine, &config.language)
    {
        Ok(arc) => {
            arc.reset();
            arc
        }
        Err(e) => {
            warn!(
                "流式引擎 '{}' 取用失败 ({}), 降级到默认引擎 '{}'",
                config.asr_engine, e, FALLBACK_STREAMING_SPEC
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
        let mut t = Transcript::new(tid, config.polish_mode);
        t.commit_edit(&text, &[], true);
        t.set_selection(s, e);
        debug!("[select] cross-session seeded t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
        (t, text, true)
    } else {
        (Transcript::new(tid, config.polish_mode), "正在聆听…".to_string(), false)
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
) {
    // 非流式模式：使用 VAD 伪流式分段识别（2c-3：编排收进 VadSegmentedPipeline）
    match crate::pipeline::VadSegmentedPipeline::new(
        engine.clone(),
        config.language.clone(),
        config.asr_engine.clone(),
        config.segment_silence,
    ) {
        Ok(pipeline) => {
            // 跨会话选中替换（同 streaming 路径）
            let tid = now_millis();
            set_current_transcription_id(tid);
            let (transcript, show_text, is_continuation) = if let Some((text, s, e)) = selection {
                let mut t = Transcript::new(tid, config.polish_mode);
                t.commit_edit(&text, &[], true);
                t.set_selection(s, e);
                debug!("[select] cross-session seeded (vad) t={} range=[{},{}] text_len={}", tid, s, e, text.chars().count());
                (t, text, true)
            } else {
                (Transcript::new(tid, config.polish_mode), "正在聆听…".to_string(), false)
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
                    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
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
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
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
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
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

/// Toggle 停止录音后的统一收尾：决定走 final 路径还是等待 pending 立即润色。
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
fn finalize_after_stop(
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
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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

    match crate::config::llm_config(config) {
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

/// 执行真正的粘贴落库操作（在主线程进行）
#[allow(clippy::too_many_arguments)]
fn do_paste(
    stage: &mut Stage,
    text_to_paste: &str,
    id: i64,
    raw_text: &str,
    segments: &str,
    polish_status: &str,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    crate::result_window::show_result(app_handle, text_to_paste);

    *stage = Stage::Pasting {
        id,
        raw_text: raw_text.to_string(),
        segments: segments.to_string(),
        polished_text: if polish_status == "done" {
            text_to_paste.to_string()
        } else {
            String::new()
        },
        polish_status: polish_status.to_string(),
    };

    let config = config.clone();
    let tx_inner = tx.clone();
    let clipboard_handle = app_handle
        .state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>()
        .inner()
        .clone();
    let text_to_paste = text_to_paste.to_string();

    let app_handle_emit = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let res = tokio::task::spawn_blocking(move || {
            // 录音过程中 insert_transcription_at_id 已在 clipboard_history 创建了 voice 条目（id=tid）。
            // paste 时只需 touch_created_at 把它顶到列表顶部，不重复创建。
            // tid=0（sentinel，无有效会话）时跳过——cancel 后 paste 的边界场景。
            let touched = if id > 0 {
                octopus_infra::db::with_db(|conn| {
                    octopus_clipboard::store::touch_created_at(conn, id)
                })
            } else {
                Ok(())
            };
            if let Err(e) = &touched {
                warn!("Clipboard history touch_created_at failed: {}", e);
            }

            // ASR 记录已入库：主动广播 clipboard://changed。paste 路径写剪贴板时
            // 会设 suppress_flag，watcher 的 on_clipboard_change 命中
            // check_and_clear_suppress 后直接 return（不调 on_change 闭包），
            // emit 不会自然触发——前端浮窗/设置面板收不到通知，ASR 记录无法即时渲染。
            if touched.is_ok() {
                let _ = app_handle_emit.emit("clipboard://changed", ());
            }

            paste::paste(&text_to_paste, &clipboard_handle, &config)
        }).await;

        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("Paste failed: {}", e),
            Err(e) => error!("Paste task panicked: {:?}", e),
        }
        let _ = tx_inner.send(Command::PasteDone);
    });
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
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    // 确保 DB 记录已 INSERT：CloudStreaming 只在 Finished 事件时 INSERT，
    // 如果整个录音过程从未触发 Finished（用户没停顿够就 Toggle stop），
    // 记录从未创建——后续 Finalize（UPDATE）会静默 0 行，数据丢失。
    if let Err(e) = update_transcription_raw(&mut transcript, &config.asr_engine, "streaming") {
        warn!("CloudStreaming finalize INSERT failed: {}", e);
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
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
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

/// 启动 VAD 伪流式 tick 线程
fn start_vad_segmented_tick_thread(tx: Sender<Command>, tick_active: Arc<AtomicBool>) {
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

// ── CloudStreaming（aliyun feature）──

/// 判定 config.asr_engine 是否为云端引擎（Aliyun、ByteDance、Tencent 或 Baidu）。
#[cfg(feature = "cloud")]
fn is_cloud_engine(config: &AppConfig) -> bool {
    use octopus_asr_local::config::EngineCategory;
    let cat = octopus_asr_local::config::resolve_engine_category(&config.asr_engine);
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
fn start_cloud_streaming_tick_thread(tx: Sender<Command>, tick_active: Arc<AtomicBool>) {
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
        crate::config::llm_config_ignore_mode(config)
    } else {
        crate::config::llm_config(config)
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
fn check_and_trigger_polish(
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
    spawn_polish_thread(input, config, tx, false, transcript.id);
}

/// 处理 Cancel 命令
fn handle_cancel(
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
    crate::result_window::hide_result(app_handle);
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
}

/// handle_discard 从当前 stage 提取的 DB finalize 数据。
/// （用 struct 而非 tuple，避免 clippy::type_complexity 且字段意义明确）
struct DiscardDbInfo {
    id: i64,
    raw_text: String,
    segments: String,
    polished_text: Option<String>,
    polish_status: String,
    polish_model: Option<String>,
}

/// 处理 Discard 命令：停止录音 + finalize DB 记录（保留识别历史），
/// 但**不粘贴、不入剪贴板**。与 Cancel 的区别：Cancel 不 finalize DB。
/// 工具栏「关闭」按钮触发。
fn handle_discard(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    app_handle: &tauri::AppHandle,
    config: &AppConfig,
) {
    // Pasting 阶段粘贴已在进行（enigo Cmd+V 已发或正发），无法撤回 → no-op
    if matches!(stage, Stage::Pasting { .. }) {
        debug!("Discard ignored during Pasting (paste in flight)");
        return;
    }

    // 从 transcript 提取 (polished_text, polish_status, polish_model) for Finalize：
    //   段模型下「已润色覆盖全部」= 无 Raw 段且非空 → 入库 "done" + finish_text；否则 "off"。
    // 修复：原版硬编码 None / "off"，把已完成的立即润色结果擦掉
    // （用户场景：立即润色→PolishDone 入库→点关闭→Finalize 覆盖 polished=None）。
    let polished_info = |t: &Transcript| -> (Option<String>, String, Option<String>) {
        let text = t.finish_text();
        if !text.is_empty() && !t.has_raw() {
            (Some(text), "done".to_string(), Some(config.polish_llm.clone()))
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
    crate::result_window::hide_result(app_handle);
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
}

/// 启动 tick 线程，定时发送 StreamingTick 命令
fn start_tick_thread(tx: Sender<Command>, streaming_active: Arc<AtomicBool>) {
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
            use tauri::Emitter;
            let _ = app_handle.emit("polish-done", ());
            return;
        }
        // 写入润色结果
        match result {
            Ok(polished) => {
                if polished.is_empty() {
                    warn!("Polish returned empty, keeping previous");
                    transcript.on_polish_failed();
                } else {
                    transcript.polish_apply(&polished);
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
                            model: Some(config.polish_llm.clone()),
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
                transcript.on_polish_failed();
            }
        }
        use tauri::Emitter;
        let _ = app_handle.emit("polish-done", ());
        // PolishDone 处理完成（pending 已清），走 final 路径
        let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
        finalize_after_stop(stage, tr, config, app_handle, _tx);
        return;
    }

    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        _ => {
            debug!("PolishDone ignored: stage={} 不是录音/等待阶段，润色结果丢弃", stage_name(stage));
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
        use tauri::Emitter;
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    match result {
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                transcript.on_polish_failed();
            } else {
                // 段模型回填（polish_apply 内部按 edited 串匹配定位 + 间隙 Polished）
                transcript.polish_apply(&polished);
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
                        model: Some(config.polish_llm.clone()),
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
            transcript.on_polish_failed();
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
            let _ = app_handle.emit("polish-done", ());
            return;
        }
    };
    if transcript.full().is_empty() {
        debug!("PolishNow skipped: transcript empty");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    if transcript.polish_pending() {
        debug!("PolishNow skipped: polish already pending");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    // 检查 LLM 配置是否存在（忽略 polish_mode，立即润色不看 mode）
    if crate::config::llm_config_ignore_mode(config).is_none() {
        warn!("PolishNow: no LLM config available");
        crate::result_window::show_result(app_handle, "未配置润色模型");
        let _ = app_handle.emit("polish-done", ());
        return;
    }
    // 确保 DB 记录已 INSERT：CloudStreaming 路径只在 Finished 事件时 INSERT，
    // 如果从未触发 Finished，PolishDone 的 UpdatePolished（UPDATE）会静默 0 行。
    // 本地路径中 Streaming/VadSegmented 已在 accept_samples 时 INSERT，此处 no-op。
    if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "streaming") {
        warn!("PolishNow ensure INSERT failed: {}", e);
    }
    let input = transcript.take_polish_input();
    info!("PolishNow triggered, polishing {} chars", input.segments.iter().map(|s| s.text.chars().count()).sum::<usize>());
    spawn_polish_thread(input, config, tx, true, transcript.id);
}

/// 进入编辑态：活跃会话（Streaming/VadSegmented/WaitingCompletion/CloudClosing）或 Idle。
/// 活跃态初始化 edit_buffer = 当前 display；Idle 态无 transcript，仅置 editing=true（见下注）。
fn handle_enter_edit_mode(stage: &mut Stage, editing: &mut bool, edit_buffer: &mut Option<String>) {
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
            info!("Entered edit mode in Idle (no active transcript)");
            return;
        }
        _ => {
            debug!("enter_edit_mode ignored in non-active stage");
            return;
        }
    };
    *editing = true;
    *edit_buffer = Some(transcript.display_text());
    info!("Entered edit mode (transcript id={})", transcript.id);
}

/// 提交编辑：写回 transcript（commit_edit + dirty ranges 劈段）+ 光标/选区恢复 + UPDATE edited_text + 刷新展示。
/// 活跃态走 transcript；Idle 态无 transcript，用 CURRENT_TRANSCRIPTION_ID 直接 UPDATE。
fn commit_edit_apply(
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
            let mut t = Transcript::new(id, PolishMode::Disabled);
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
            crate::result_window::update_result(app_handle, text, false, 0);
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
    crate::result_window::update_result(app_handle, &transcript.display_text(), false, 0);
    info!("Edit committed ({} chars)", text.chars().count());
}

/// 从 stage 中取出 transcript 的可变引用（用于 cancel edit 恢复展示）
fn stage_transcript(stage: &mut Stage) -> Option<&mut Transcript> {
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

fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Streaming { .. } => "Streaming",
        Stage::VadSegmented { .. } => "VadSegmented",
        Stage::WaitingCompletion { .. } => "WaitingCompletion",
        Stage::StoppingPolish { .. } => "StoppingPolish",
        Stage::Polishing { .. } => "Polishing",
        Stage::Pasting { .. } => "Pasting",
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { .. } => "CloudClosing",
    }
}

/// pipeline 事件 → 端动作（DB/emit/polish/错误上报）。2d 统一路由，消除三路径重复。（spec §3.5）
fn apply_pipeline_events(
    events: Vec<crate::pipeline::PipelineEvent>,
    transcript: &mut Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    use crate::pipeline::PipelineEvent;
    for ev in events {
        match ev {
            PipelineEvent::PersistRaw { engine_mode } => {
                if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, engine_mode) {
                    warn!("DB ({}) failed: {}", engine_mode, e);
                }
            }
            PipelineEvent::Emit { display, insertion, caret } => {
                // 把 pipeline 的 insertion 标志 + caret 偏移实传给 result_window（前端跳过 diverted 300ms 延迟
                // 立即渲染；insertion=true 时用 caret 定位闪烁光标，使其跟在最后插入的文字后右移）。
                if !display.is_empty() {
                    crate::result_window::update_result(app_handle, &display, insertion, caret);
                }
            }
            PipelineEvent::Polish { silence } => {
                check_and_trigger_polish(transcript, silence, config, tx);
            }
            PipelineEvent::Error(e) => {
                crate::result_window::update_result(app_handle, &e, false, 0);
            }
            PipelineEvent::Speaking(speaking) => {
                let _ = app_handle.emit("update-speaking", speaking);
            }
        }
    }
}

/// VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令合一的 dispatch（2d，spec §3.5）。
/// 各 Stage 变体调对应 pipeline 的 `tick` → `apply_pipeline_events` 统一路由。
/// WaitingCompletion 额外做 active_count==0 收尾判定（沿用 2c-3 既有逻辑）。
fn dispatch_tick(
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
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::VadSegmented { pipeline, transcript, .. } => {
            let events = pipeline.tick(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::WaitingCompletion { pipeline, transcript, tick_active } => {
            let events = pipeline.tick(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            // 所有在途段完成 → 收尾（停 tick 线程 + finalize）
            if pipeline.active_count() == 0 {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
        _ => {}
    }
}

/// 首次有文本 INSERT，否则 UPDATE raw_text。DB 失败返回 Err 供调用方 warn（不阻塞识别）。
/// 用 Transcript.db_inserted() 区分首次与后续（避免「UPDATE 0 行无法判断」歧义）。
fn update_transcription_raw(
    transcript: &mut Transcript,
    engine: &str,
    engine_mode: &str,
) -> Result<(), String> {
    if transcript.full().is_empty() {
        return Ok(());
    }
    let sender = get_db_sender();
    if !transcript.db_inserted() {
        let cmd = DbCommand::Insert {
            id: transcript.id,
            text: transcript.db_text(),
            segments: transcript.segments_json(),
            engine: engine.to_string(),
            engine_mode: Some(engine_mode.to_string()),
        };
        sender.send(cmd).map_err(|e| format!("Queue DB insert failed: {}", e))?;
        transcript.mark_db_inserted();
        transcript.mark_db_written();
    } else {
        // 落库节流：距上次落库 < DB_FLUSH_INTERVAL_MS 则跳过本次 UPDATE（Finalize 兜底完整写入，
        // 最坏落后一帧文本；避免长录音每 changed≈每 tick 一次 UPDATE 的写放大）。
        if !transcript.db_flush_due(std::time::Duration::from_millis(DB_FLUSH_INTERVAL_MS)) {
            return Ok(());
        }
        let cmd = DbCommand::UpdateTextSegments {
            id: transcript.id,
            text: transcript.db_text(),
            segments: transcript.segments_json(),
        };
        sender
            .send(cmd)
            .map_err(|e| format!("Queue DB update_text_segments failed: {}", e))?;
        transcript.mark_db_written();
    }
    Ok(())
}

