// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::config::PolishMode;
use crate::engine::TranscriptionEngine;
use crate::paste;
use crate::transcript::Transcript;
use octopus_asr::streaming_engine::StreamingSession;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// 协调器命令
enum Command {
    /// 切换录音状态（开始/停止）
    Toggle,
    /// 取消当前操作
    Cancel,
    /// 流式识别 tick（定时触发，驱动音频采集和识别）
    StreamingTick,
    /// VAD 伪流式 tick（300ms 间隔，驱动分段识别）
    VadSegmentedTick,
    /// 转录完成（离线模式或远程模式使用，seq 用于顺序拼接）
    TranscriptionDone {
        text: Result<String, String>,
        seq: u64,
        session_id: i64,
    },
    /// 粘贴完成
    PasteDone,
    /// 润色完成
    PolishDone { result: Result<String, String> },
    /// 最终润色完成
    FinalPolishDone { result: Result<String, String> },
}

enum Stage {
    Idle,
    /// 流式识别：边录边识别
    Streaming {
        engine: StreamingSession,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
        /// VAD 实例，用于检测静音间隔
        vad: Option<octopus_asr::vad::SileroVad>,
        /// 累积静音时长（秒），超过阈值后恢复说话时插入标点
        silence_duration: f64,
        /// 是否已对当前静音进行了主动冲刷（避免重复冲刷）
        flushed: bool,
    },
    /// VAD 伪流式：tick 驱动分段识别（非流式引擎使用）
    VadSegmented {
        /// 检测用 VAD：逐 tick 喂入顺序音频，**有状态累积**（LSTM 跨 tick 续接），
        /// 用于 compute_speech_chunks 的语音/静音门控。语义为「流式检测」，
        /// 录音期间从不 reset（续接上下文使边界判定更稳）。
        vad: octopus_asr::vad::SileroVad,
        /// 过滤用 VAD：仅 filter_speech_from_buffer 用，**每段独立**。
        /// 与检测 VAD 分离：检测流喂入的音频与 send_buffer（overlap_tail + audio_buffer）
        /// 存在重叠，若共用一个有状态 VAD 会双重喂入 + 跨段污染 LSTM h/c → 段首 gating 失真。
        /// 故每次过滤前 reset() 归零，恢复「每段独立」语义（等价于旧代码每 buffer 新建 VAD），
        /// 同时避免每次切分重建 ONNX Session 的开销（实例在录音开始时一次性创建）。
        filter_vad: octopus_asr::vad::SileroVad,
        /// 音频累积缓冲区（16kHz mono f32）
        audio_buffer: Vec<f32>,
        /// 前一窗口末尾 0.2s 的 overlap 音频
        overlap_tail: Vec<f32>,
        transcript: Transcript,
        /// 当前静音持续时长（秒）
        silence_duration: f64,
        /// 缓冲区是否包含语音
        has_speech: bool,
        /// 正在进行的识别任务数
        active_count: u32,
        /// 下一个发送序号
        next_seq: u64,
        /// 已消费到的连续序号（completed_seq 之前的已拼接完毕）
        completed_seq: u64,
        /// 缓存乱序完成的识别结果
        completed_results: HashMap<u64, String>,
        /// tick 线程控制标志
        tick_active: Arc<AtomicBool>,
    },
    /// 等待所有识别完成
    WaitingCompletion {
        transcript: Transcript,
        active_count: u32,
        completed_seq: u64,
        completed_results: HashMap<u64, String>,
    },
    /// 最终润色中
    Polishing {
        id: i64,
        raw_text: String,
    },
    /// 粘贴中
    Pasting {
        /// 识别记录主键（Task 6 过程入库用）
        id: i64,
        /// 原生全文（入库用，不受编辑影响）
        raw_text: String,
        /// 展示/入库的修正版（初始=润色结果，用户编辑会更新）
        polished_text: String,
        /// "off" | "done" | "failed"
        polish_status: String,
    },
}

/// VAD 静音判定阈值
const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// VAD 分块大小（采样点数）
const VAD_CHUNK_SIZE: usize = 512;
/// 插入标点的静音时长阈值（秒）
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;

/// VAD 伪流式 tick 间隔（毫秒）
const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 300;

/// 中间润色最小间隔下限（秒）：polish_mode=2 且 polish_interval<=0 时回退到此值，避免每 tick 刷爆 LLM。
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
    tx: std::sync::Mutex<Sender<Command>>,
}

/// 流式识别 tick 间隔（毫秒）
const STREAMING_TICK_INTERVAL_MS: u64 = 600;

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

        let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine(&config);
        let mut config = config;
        let mut use_streaming = use_streaming;

        std::thread::spawn(move || {
            let mut stage = Stage::Idle;

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
                        // 仅在 Idle（开新会话）时同步运行时覆盖；STOP 时不动 asr_engine
                        // （否则会把"刚切换但本会话未用"的引擎名写进 DB 记录）
                        if matches!(stage, Stage::Idle) {
                            let rc = runtime_config.read().unwrap();
                            config.asr_engine = match octopus_asr::config::resolve_active_engine(&rc.asr_engine) {
                                Ok(resolved) => resolved.name,
                                Err(_) => "zipformer-small-ctc".to_string(),
                            };
                            config.polish_mode = rc.polish_mode;
                            drop(rc);
                            use_streaming = config.engine_mode == "embedded"
                                && crate::config::is_streaming_engine(&config);
                        }
                        handle_toggle(
                            &mut stage,
                            &audio,
                            &engine,
                            &config,
                            &app_handle,
                            &tx,
                            use_streaming,
                        );
                    }
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                    }
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        handle_vad_segmented_tick(
                            &mut stage,
                            &audio,
                            &engine,
                            &config,
                            &app_handle,
                            &tx,
                        );
                    }
                    Command::Cancel => {
                        handle_cancel(&mut stage, &audio, &app_handle);
                    }
                    Command::TranscriptionDone { text, seq, session_id } => {
                        handle_transcription_done(
                            &mut stage,
                            text,
                            seq,
                            session_id,
                            &config,
                            &app_handle,
                            &tx,
                        );
                    }
                    Command::PasteDone => {
                        // 入库 finalize（从 Pasting 取数据；用户编辑已反映到 polished_text）
                        if let Stage::Pasting {
                            id,
                            raw_text,
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
                        crate::overlay::hide_overlay(&app_handle);
                        crate::result_window::clear_result(&app_handle);
                        crate::tray::update_tray_label(
                            &app_handle,
                            crate::tray::TrayState::Idle,
                        );
                    }
                    Command::PolishDone { result } => {
                        handle_polish_done(&mut stage, result, &config, &app_handle, &tx);
                    }
                    Command::FinalPolishDone { result } => {
                        handle_final_polish_done(&mut stage, result, &config, &app_handle, &tx);
                    }
                }
            }
            debug!("Coordinator thread exited");
        });

        Self {
            tx: std::sync::Mutex::new(tx_self),
        }
    }

    /// 发送 toggle 命令
    pub fn toggle(&self) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::Toggle).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }

    /// 发送 cancel 命令
    pub fn cancel(&self) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::Cancel).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }
}

/// 前端命令：取消当前录音/处理（Esc 键）。
/// 停止麦克风采集、重置状态机为 Idle、隐藏 overlay 与结果窗口、托盘置 Idle。
#[tauri::command]
pub fn cancel_recording(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.cancel();
}

/// 处理 Toggle 命令
fn handle_toggle(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    use_streaming: bool,
) {
    match stage {
        Stage::Idle => {
            info!("Toggle: starting {}", if use_streaming { "streaming" } else { "VAD segmented" });

            if let Err(e) = audio.start(&config.microphone) {
                error!("Failed to start recording: {}", e);
                return;
            }

            if use_streaming {
                // 流式模式：创建 StreamingSession 并启动 tick 线程
                match StreamingSession::new(&config.asr_engine) {
                    Ok(streaming_engine) => {
                        // 流式模式：只显示 result window，不显示 overlay
                        crate::result_window::show_result(app_handle, "正在聆听…");
                        crate::tray::update_tray_label(
                            app_handle,
                            crate::tray::TrayState::Recording,
                        );

                        // 初始化 VAD（用于静音检测 + 标点）
                        let vad = match octopus_asr::config::find_silero_vad() {
                            Ok(path) => match octopus_asr::vad::SileroVad::new(&path) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    warn!("VAD init failed: {}, punctuation disabled", e);
                                    None
                                }
                            },
                            Err(e) => {
                                warn!("VAD not found: {}, punctuation disabled", e);
                                None
                            }
                        };

                        let streaming_active = Arc::new(AtomicBool::new(true));
                        start_tick_thread(tx.clone(), streaming_active.clone());

                        *stage = Stage::Streaming {
                            engine: streaming_engine,
                            transcript: Transcript::new(now_millis(), config.polish_mode),
                            streaming_active,
                            vad,
                            silence_duration: 0.0,
                            flushed: false,
                        };
                    }
                    Err(e) => {
                        error!("Failed to create streaming session: {}", e);
                        let _ = audio.stop();
                        crate::overlay::hide_overlay(app_handle);
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                    }
                }
            } else {
                // 非流式模式：使用 VAD 伪流式分段识别
                match octopus_asr::config::find_silero_vad() {
                    Ok(path) => match octopus_asr::vad::SileroVad::new(&path) {
                        Ok(vad) => {
                            // 第二个独立 VAD 实例用于过滤（每段 reset，避免与检测流共用造成
                            // LSTM 状态污染）。ONNX Session 在此一次性创建，过滤时只 reset 不重建。
                            // 同一路径 vad 已加载成功，filter_vad 失败属异常，直接放弃。
                            let filter_vad = match octopus_asr::vad::SileroVad::new(&path) {
                                Ok(v) => v,
                                Err(e) => {
                                    error!("filter_vad init failed: {}, abort VadSegmented", e);
                                    let _ = audio.stop();
                                    return;
                                }
                            };
                            crate::result_window::show_result(app_handle, "正在聆听…");
                            crate::tray::update_tray_label(
                                app_handle,
                                crate::tray::TrayState::Recording,
                            );

                            let tick_active = Arc::new(AtomicBool::new(true));
                            start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());

                            *stage = Stage::VadSegmented {
                                vad,
                                filter_vad,
                                audio_buffer: Vec::new(),
                                overlap_tail: Vec::new(),
                                transcript: Transcript::new(now_millis(), config.polish_mode),
                                silence_duration: 0.0,
                                has_speech: false,
                                active_count: 0,
                                next_seq: 0,
                                completed_seq: 0,
                                completed_results: HashMap::new(),
                                tick_active,
                            };
                        }
                        Err(e) => {
                            error!("VAD init failed for VadSegmented: {}, falling back to offline", e);
                            let _ = audio.stop();
                        }
                    },
                    Err(e) => {
                        error!("VAD not found for VadSegmented: {}, falling back to offline", e);
                        let _ = audio.stop();
                    }
                }
            }
        }

        Stage::VadSegmented {
            ref mut filter_vad,
            audio_buffer,
            overlap_tail,
            transcript,
            has_speech,
            active_count,
            next_seq,
            completed_seq,
            completed_results,
            tick_active,
            ..
        } => {
            // VAD 伪流式：停止 tick，发送剩余缓冲区，决定等待完成或直接粘贴
            info!("Toggle: stopping VadSegmented (active_count={})", active_count);

            // 停止 tick 线程
            tick_active.store(false, Ordering::Relaxed);

            // 停止录音并排空剩余音频
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                audio_buffer.extend_from_slice(&remaining);
            }

            // 如果缓冲区有语音，发送最后一次识别
            if *has_speech && !audio_buffer.is_empty() {
                let mut send_buffer = overlap_tail.clone();
                send_buffer.extend_from_slice(audio_buffer);
                let speech_samples = filter_speech_from_buffer(filter_vad, &send_buffer);
                if !speech_samples.is_empty() {
                    let seq = *next_seq;
                    *next_seq += 1;
                    *active_count += 1;
                    spawn_offline_transcription_with_seq(
                        engine, config, tx, speech_samples, seq, transcript.id,
                    );
                }
            }

            let active = *active_count;
            // 忽略中间润色的 pending 结果（最终润色会重新处理）
            transcript.clear_polish_pending();
            let cseq = *completed_seq;
            let cresults = std::mem::take(completed_results);

            if active > 0 {
                // 把 transcript 移入 WaitingCompletion（用临时占位避免部分移动）
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                *stage = Stage::WaitingCompletion {
                    transcript: tr,
                    active_count: active,
                    completed_seq: cseq,
                    completed_results: cresults,
                };
            } else {
                let final_text = if transcript.full().is_empty() {
                    String::new()
                } else if transcript
                    .full()
                    .ends_with(|c: char| ",.，。！？!?\n".contains(c))
                {
                    transcript.db_text()
                } else {
                    format!("{}。", transcript.db_text())
                };
                if final_text.is_empty() {
                    *stage = Stage::Idle;
                    crate::overlay::hide_overlay(app_handle);
                    crate::result_window::hide_result(app_handle);
                    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                } else {
                    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                    start_final_polish_or_paste(
                        stage,
                        &final_text,
                        tr,
                        config,
                        app_handle,
                        tx,
                    );
                }
            }
        }

        Stage::Streaming {
            engine: streaming_engine,
            transcript,
            streaming_active,
            ..
        } => {
            // 流式模式：停止流式，获取最终文本，粘贴
            info!("Toggle: stopping streaming, finalizing");

            transcript.clear_polish_pending();

            // 停止 tick
            streaming_active.store(false, Ordering::Relaxed);

            // 获取最终音频和识别结果
            let final_samples = audio.drain_samples();
            if !final_samples.is_empty() {
                if let Err(e) = streaming_engine.accept_samples(&final_samples, false) {
                    warn!("Error processing final samples: {}", e);
                }
            }

            let final_text = match streaming_engine.finish() {
                Ok(text) => text,
                Err(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.db_text()
                }
            };

            // 重置引擎
            streaming_engine.reset();

            // 停止录音
            let _ = audio.stop();

            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            let combined = transcript.db_text();

            info!("Final streaming text: '{}'", combined);

            if combined.is_empty() {
                *stage = Stage::Idle;
                crate::overlay::hide_overlay(app_handle);
                crate::result_window::hide_result(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // 显示最终结果（润色前的 display_text，含中间润色结果）
            crate::result_window::show_result(app_handle, &transcript.display_text());

            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            start_final_polish_or_paste(
                stage,
                &combined,
                tr,
                config,
                app_handle,
                tx,
            );
        }

        Stage::WaitingCompletion { .. } => {
            debug!("Toggle ignored: waiting for transcription completion");
        }

        Stage::Polishing { .. } => {
            debug!("Toggle ignored: busy polishing");
        }

        Stage::Pasting { .. } => {
            debug!("Toggle ignored: busy pasting");
        }
    }
}

/// 开始粘贴阶段（支持最终润色）。`transcript` 移交进 Pasting 持 id（Task 6 用）。
/// 开始最终润色或粘贴阶段（异步最终润色，防止阻塞协调器线程）。
fn start_final_polish_or_paste(
    stage: &mut Stage,
    text: &str,
    transcript: Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if text.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::overlay::hide_overlay(app_handle);
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

            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
            };

            let tx = tx.clone();
            let text_to_polish = text.to_string();
            std::thread::spawn(move || {
                let result = match octopus_llm::polish(&text_to_polish, &llm_config) {
                    Ok(polished) => {
                        if polished.is_empty() {
                            Err("Final polish returned empty".to_string())
                        } else {
                            Ok(polished)
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Command::FinalPolishDone { result });
            });
        }
    }
}

/// 执行真正的粘贴落库操作（在主线程进行）
fn do_paste(
    stage: &mut Stage,
    text_to_paste: &str,
    id: i64,
    raw_text: &str,
    polish_status: &str,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    crate::result_window::show_result(app_handle, text_to_paste);

    *stage = Stage::Pasting {
        id,
        raw_text: raw_text.to_string(),
        polished_text: if polish_status == "done" {
            text_to_paste.to_string()
        } else {
            String::new()
        },
        polish_status: polish_status.to_string(),
    };

    let config = config.clone();
    let tx_inner = tx.clone();
    let handle_for_closure = app_handle.clone();
    let text_to_paste = text_to_paste.to_string();

    tauri::async_runtime::spawn(async move {
        let res = tokio::task::spawn_blocking(move || {
            paste::paste(&text_to_paste, &handle_for_closure, &config)
        }).await;

        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("Paste failed: {}", e),
            Err(e) => error!("Paste task panicked: {:?}", e),
        }
        let _ = tx_inner.send(Command::PasteDone);
    });
}

/// 处理最终润色完成事件
fn handle_final_polish_done(
    stage: &mut Stage,
    result: Result<String, String>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let (id, raw_text) = match stage {
        Stage::Polishing { id, raw_text } => {
            (*id, raw_text.clone())
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
                "done",
                config,
                app_handle,
                tx,
            );
        }
        Err(e) => {
            warn!("Final polish failed: {}, using original", e);
            do_paste(
                stage,
                &raw_text,
                id,
                &raw_text,
                "failed",
                config,
                app_handle,
                tx,
            );
        }
    }
}

/// 消费已完成序号的结果，把新段追加到 Transcript。
fn consume_completed_results(
    completed_seq: &mut u64,
    completed_results: &mut HashMap<u64, String>,
    transcript: &mut Transcript,
) {
    while let Some(text) = completed_results.remove(completed_seq) {
        if !text.is_empty() {
            let overlap_len = get_overlap_len(transcript.full(), &text);
            if overlap_len > 0 {
                let suffix: String = text.chars().skip(overlap_len).collect();
                transcript.append_segment(&suffix);
            } else {
                // 段间加逗号（已有文本且新段不以标点开头）
                if !transcript.full().is_empty()
                    && !text.starts_with(|c: char| ",.，。！？!?\n".contains(c))
                {
                    transcript.append_segment("，");
                }
                transcript.append_segment(&text);
            }
        }
        *completed_seq += 1;
    }
}

fn get_overlap_len(existing: &str, incoming: &str) -> usize {
    let existing_trimmed = existing.trim_end_matches(|c: char| c.is_whitespace() || ",.，。！？!?、;；:：".contains(c));
    let existing_chars: Vec<char> = existing_trimmed.chars().collect();
    let incoming_chars: Vec<char> = incoming.chars().collect();
    let max_match = 8.min(existing_chars.len()).min(incoming_chars.len());
    let mut best_len = 0;
    for len in 1..=max_match {
        if existing_chars[existing_chars.len() - len..] == incoming_chars[..len] {
            best_len = len;
        }
    }
    best_len
}

/// 处理 VadSegmentedTick 命令
fn handle_vad_segmented_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if let Stage::VadSegmented {
        vad,
        filter_vad,
        audio_buffer,
        overlap_tail,
        transcript,
        silence_duration,
        has_speech,
        active_count,
        next_seq,
        ..
    } = stage
    {
        // 1. drain 音频
        let samples = audio.drain_samples();
        if samples.is_empty() {
            return;
        }

        // 2. 追加到缓冲区
        audio_buffer.extend_from_slice(&samples);

        // 3. VAD 检测本段语音帧数
        let speech_chunks = compute_speech_chunks(vad, &samples);
        if speech_chunks >= 2 {
            *silence_duration = 0.0;
            *has_speech = true;
        } else {
            let chunk_duration = samples.len() as f64 / 16000.0;
            *silence_duration += chunk_duration;
        }

        // 4. 判断是否发送识别：静音边界切分（主）/ 连续超时强制切断（兜底）
        let buffer_duration_s = audio_buffer.len() as f64 / 16000.0;
        let silence_ms = *silence_duration * 1000.0;
        let silence_cut = *has_speech && silence_ms >= config.segment_silence;
        let force_cut = *has_speech && buffer_duration_s >= config.segment_duration;
        let should_send = silence_cut || force_cut;

        if should_send {
            // 构建发送缓冲区：前一窗口 overlap（静音切分后为空）+ 当前缓冲区。
            // 先 clone 再更新 overlap_tail，确保用的是「上一窗口」末尾而非当前段末尾。
            let mut send_buffer = overlap_tail.clone();
            send_buffer.extend_from_slice(audio_buffer);

            // 仅强制切断保留下一段 overlap（语句被硬切，需重叠保证连贯）；
            // 静音切分是自然语句边界，下一段从干净开始，无需 overlap。
            if force_cut {
                let overlap_samples = (config.segment_overlap * 16.0) as usize;
                let overlap_start = audio_buffer.len().saturating_sub(overlap_samples);
                *overlap_tail = audio_buffer[overlap_start..].to_vec();
            } else {
                overlap_tail.clear();
            }

            // 重置缓冲区状态
            audio_buffer.clear();
            *has_speech = false;
            *silence_duration = 0.0;

            // VAD 过滤语音片段（用独立 filter_vad，每段 reset，不污染检测流）
            let speech_samples = filter_speech_from_buffer(filter_vad, &send_buffer);
            if !speech_samples.is_empty() {
                let seq = *next_seq;
                *next_seq += 1;
                *active_count += 1;
                debug!(
                    "VadSegmented: {} cut, seq={}, samples={}, active_count={}",
                    if force_cut { "force" } else { "silence" },
                    seq,
                    speech_samples.len(),
                    active_count
                );
                spawn_offline_transcription_with_seq(
                    engine, config, tx, speech_samples, seq, transcript.id,
                );
                // 段切分 + 有语音 → 触发停顿润色（传阈值，段边界即停顿点）
                check_and_trigger_polish(transcript, config.pause_polish_threshold_ms / 1000.0, config, tx);
            }
        }

        // 5. 更新 result window
        if !transcript.full().is_empty() {
            crate::result_window::update_result(app_handle, &transcript.display_text());
        }
    }
}

/// 计算音频片段中语音帧的数量
fn compute_speech_chunks(vad: &mut octopus_asr::vad::SileroVad, samples: &[f32]) -> usize {
    let mut speech_chunks = 0usize;

    for chunk in samples.chunks(VAD_CHUNK_SIZE) {
        if chunk.len() < VAD_CHUNK_SIZE {
            break;
        }
        match vad.compute(chunk) {
            Ok(prob) => {
                if prob >= VAD_SPEECH_THRESHOLD {
                    speech_chunks += 1;
                }
            }
            Err(_) => {
                // VAD 计算失败，保守认为有语音
                speech_chunks += 1;
            }
        }
    }
    speech_chunks
}

/// 启动 VAD 伪流式 tick 线程
fn start_vad_segmented_tick_thread(tx: Sender<Command>, tick_active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while tick_active.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(VAD_SEGMENTED_TICK_INTERVAL_MS));
            if tick_active.load(Ordering::Relaxed) {
                if tx.send(Command::VadSegmentedTick).is_err() {
                    break;
                }
            }
        }
        debug!("VadSegmented tick thread exited");
    });
}

/// 带 seq 序号的离线识别线程
fn spawn_offline_transcription_with_seq(
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    tx: &Sender<Command>,
    speech_samples: Vec<f32>,
    seq: u64,
    session_id: i64,
) {
    let engine = engine.clone();
    let language = config.language.clone();
    let asr_engine = config.asr_engine.clone();
    let tx = tx.clone();
    let samples_len = speech_samples.len();
    let duration = samples_len as f64 / 16000.0;

    // 复用 Tauri 全局异步运行时，避免 VadSegmented 每 ~300ms 分段都
    // 新建并销毁一个 current-thread Tokio Runtime 的开销。
    // engine.transcribe 的 Future 是 Send（#[async_trait]），且内部 CPU 密集
    // 推理已用 spawn_blocking 包裹，不阻塞 runtime worker。
    tauri::async_runtime::spawn(async move {
        let start = Instant::now();
        let result = engine.transcribe(&speech_samples, &language, &asr_engine).await;
        let elapsed = start.elapsed();
        info!(
            "Transcription seq={} took {:.2}s (audio: {:.2}s, RTF: {:.2})",
            seq,
            elapsed.as_secs_f64(),
            duration,
            elapsed.as_secs_f64() / duration.max(0.001)
        );
        let msg = Command::TranscriptionDone {
            text: match result {
                Ok(text) => Ok(text),
                Err(e) => Err(e.to_string()),
            },
            seq,
            session_id,
        };
        let _ = tx.send(msg);
    });
}

/// 对缓冲区音频做 VAD 过滤。
///
/// 使用 stage 的独立 `filter_vad`（与检测流分离），**过滤前先 reset() 归零 LSTM 状态**，
/// 使每段过滤处于「冷启动」语义——等价于旧代码每个 buffer 新建一个 VAD（h=0）。
/// 检测流（stage.vad）会按顺序逐 tick 喂入音频并累积状态，send_buffer 与之重叠，
/// 若共用会双重喂入 + 跨段污染 → 段首 gating 失真。ONNX Session 在录音开始时一次性
/// 创建，这里只 reset 不重建，兼顾正确性与性能。
fn filter_speech_from_buffer(
    filter_vad: &mut octopus_asr::vad::SileroVad,
    samples: &[f32],
) -> Vec<f32> {
    filter_vad.reset();
    let speech = octopus_asr::audio::filter_speech(samples, filter_vad, 480, 0.5);
    if speech.is_empty() {
        debug!("VadSegmented: no speech detected in buffer");
        Vec::new()
    } else {
        speech
    }
}

/// 启动润色线程
fn spawn_polish_thread(
    text: String,
    config: &AppConfig,
    tx: &Sender<Command>,
) {
    let llm_config = match crate::config::llm_config(&config) {
        Some(c) => c,
        None => return,
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish(&text, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => {
                log::warn!("Polish thread error: {}", e);
                Err(e.to_string())
            }
        };
        let _ = tx.send(Command::PolishDone { result });
    });
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
    // 无新增内容（increase 空）→ 跳过
    if !transcript.has_increase() {
        return;
    }
    // 停顿未达标 → 跳过（流式传真实 silence；伪流式传阈值自动达标）
    if silence_duration < config.pause_polish_threshold_ms / 1000.0 {
        return;
    }
    // 节流：距上次润色不足 interval（至少 MIN_POLISH_INTERVAL_SEC）→ 跳过
    if transcript.last_polish_time().elapsed().as_secs_f64()
        < config.polish_interval.max(MIN_POLISH_INTERVAL_SEC)
    {
        return;
    }
    // 快照（推进 raw_len，increase 清空）+ 标记 pending + 送 LLM 全量润色
    let snapshot = transcript.snapshot_for_polish();
    transcript.mark_polish_pending();
    spawn_polish_thread(snapshot, config, tx);
}

/// 处理 StreamingTick 命令
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if let Stage::Streaming {
        engine,
        transcript,
        vad,
        silence_duration,
        flushed,
        ..
    } = stage
    {
        let samples = audio.drain_samples();
        if samples.is_empty() {
            return;
        }

        let was_silent = detect_silence_gap(vad, &samples, silence_duration);
        if *silence_duration == 0.0 {
            *flushed = false;
        }

        match engine.accept_samples(&samples, was_silent) {
            Ok(Some(new_text)) => {
                transcript.set_full(&new_text);
                if let Err(e) =
                    update_transcription_raw(transcript, &config.asr_engine, "streaming")
                {
                    warn!("DB (streaming) failed: {}", e);
                }
                crate::result_window::update_result(app_handle, &transcript.display_text());
            }
            Ok(None) => {}
            Err(e) => warn!("Streaming accept_samples error: {}", e),
        }

        // 静音主动冲刷（>0.5s）
        if *silence_duration >= PUNCTUATION_SILENCE_THRESHOLD && !*flushed {
            match engine.flush() {
                Ok(Some(new_text)) => {
                    transcript.set_full(&new_text);
                    if let Err(e) =
                        update_transcription_raw(transcript, &config.asr_engine, "streaming")
                    {
                        warn!("DB (streaming) failed: {}", e);
                    }
                    debug!("Flushed: '{}'", transcript.full());
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
                Ok(None) => {}
                Err(e) => warn!("Streaming flush error: {}", e),
            }
            *flushed = true;
        }

        // 停顿润色（Task 5 接入；此处先保留 check_and_trigger_polish 占位签名）
        check_and_trigger_polish(transcript, *silence_duration, config, tx);
    }
}

/// 用 VAD 检测音频中的语音/静音。
///
/// 返回 `true` 表示之前累积了 >0.5s 的静音间隔（需要插入逗号）。
/// 同时更新 `silence_duration`：
/// - 本段音频大部分是静音 → 累加静音时长
/// - 本段音频包含语音 → 重置为 0
fn detect_silence_gap(
    vad: &mut Option<octopus_asr::vad::SileroVad>,
    samples: &[f32],
    silence_duration: &mut f64,
) -> bool {
    let prev_silence = *silence_duration;

    match vad {
        Some(v) => {
            let mut speech_chunks = 0usize;
            let mut silent_chunks = 0usize;

            for chunk in samples.chunks(VAD_CHUNK_SIZE) {
                if chunk.len() < VAD_CHUNK_SIZE {
                    break; // 不足一个完整块，跳过
                }
                match v.compute(chunk) {
                    Ok(prob) => {
                        if prob >= VAD_SPEECH_THRESHOLD {
                            speech_chunks += 1;
                        } else {
                            silent_chunks += 1;
                        }
                    }
                    Err(_) => {
                        // VAD 计算失败，保守认为有语音
                        speech_chunks += 1;
                    }
                }
            }

            let total_chunks = speech_chunks + silent_chunks;
            if total_chunks == 0 {
                return false;
            }

            let chunk_duration = VAD_CHUNK_SIZE as f64 / 16000.0; // 每块时长（秒）

            if speech_chunks >= 2 {
                // 本段包含足够的语音 → 重置静音计时
                *silence_duration = 0.0;
            } else {
                // 本段大部分是静音 → 累积静音时长
                *silence_duration += total_chunks as f64 * chunk_duration;
            }

            // 之前累积静音 > 阈值，且本段有语音 → 需要插入标点
            prev_silence >= PUNCTUATION_SILENCE_THRESHOLD
        }
        None => {
            // 无 VAD，不加标点
            false
        }
    }
}

/// 处理 Cancel 命令
fn handle_cancel(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    app_handle: &tauri::AppHandle,
) {
    match stage {
        Stage::Streaming {
            engine,
            streaming_active,
            ..
        } => {
            info!("Cancel: stopping streaming");
            streaming_active.store(false, Ordering::Relaxed);
            engine.reset();
            let _ = audio.stop();
        }
        Stage::VadSegmented {
            tick_active, ..
        } => {
            info!("Cancel: stopping VadSegmented");
            tick_active.store(false, Ordering::Relaxed);
            let _ = audio.stop();
        }
        Stage::WaitingCompletion { .. } => {
            info!("Cancel: cancelling while waiting for transcription");
            // 识别结果将被忽略，回到 Idle
        }
        Stage::Polishing { .. } => {
            info!("Cancel: cancelling while final polishing");
            // 润色结果将被忽略，回到 Idle
        }
        _ => {}
    }
    *stage = Stage::Idle;
    crate::overlay::hide_overlay(app_handle);
    crate::result_window::hide_result(app_handle);
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
}

/// 处理 TranscriptionDone 命令
fn handle_transcription_done(
    stage: &mut Stage,
    text: Result<String, String>,
    seq: u64,
    session_id: i64,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    match stage {
        Stage::VadSegmented {
            transcript,
            active_count,
            completed_seq,
            completed_results,
            ..
        } => {
            if transcript.id != session_id {
                debug!("TranscriptionDone seq={} for old session {} ignored (current: {})", seq, session_id, transcript.id);
                return;
            }
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(t) if !t.is_empty() => {
                    info!("VadSegmented transcription seq={}: '{}'", seq, t);
                    completed_results.insert(seq, t);
                }
                // 空结果：仍占位该 seq，避免 consume_completed_results 卡在缺失序号
                Ok(_) => {
                    completed_results.insert(seq, String::new());
                }
                // 识别失败：占位空串，保证 completed_seq 连续推进、后续有效段不积压丢失
                Err(e) => {
                    error!("VadSegmented transcription seq={} failed: {}", seq, e);
                    completed_results.insert(seq, String::new());
                }
            }

            // 消费连续序号的结果（追加到 Transcript）
            consume_completed_results(completed_seq, completed_results, transcript);
            if let Err(e) =
                update_transcription_raw(transcript, &config.asr_engine, "vad_segmented")
            {
                warn!("DB (vad_segmented) failed: {}", e);
            }

            if !transcript.full().is_empty() {
                crate::result_window::update_result(app_handle, &transcript.display_text());
            }
        }

        Stage::WaitingCompletion {
            transcript,
            active_count,
            completed_seq,
            completed_results,
        } => {
            if transcript.id != session_id {
                debug!("TranscriptionDone seq={} for old session {} ignored (current: {})", seq, session_id, transcript.id);
                return;
            }
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(t) if !t.is_empty() => {
                    info!("WaitingCompletion transcription seq={}: '{}'", seq, t);
                    completed_results.insert(seq, t);
                }
                // 空结果：仍占位该 seq，避免 consume_completed_results 卡在缺失序号
                Ok(_) => {
                    completed_results.insert(seq, String::new());
                }
                // 识别失败：占位空串，保证 completed_seq 连续推进、后续有效段不积压丢失
                Err(e) => {
                    error!("WaitingCompletion transcription seq={} failed: {}", seq, e);
                    completed_results.insert(seq, String::new());
                }
            }

            consume_completed_results(completed_seq, completed_results, transcript);

            if *active_count == 0 {
                let final_text = if transcript.full().is_empty() {
                    String::new()
                } else if transcript
                    .full()
                    .ends_with(|c: char| ",.，。！？!?\n".contains(c))
                {
                    transcript.db_text()
                } else {
                    format!("{}。", transcript.db_text())
                };
                if final_text.is_empty() {
                    *stage = Stage::Idle;
                    crate::overlay::hide_overlay(app_handle);
                    crate::result_window::hide_result(app_handle);
                    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                } else {
                    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                    start_final_polish_or_paste(
                        stage,
                        &final_text,
                        tr,
                        config,
                        app_handle,
                        tx,
                    );
                }
            }
        }

        _ => {
            // 其他阶段收到转录结果（可能是取消后延迟到达），忽略
            debug!("TranscriptionDone seq={} ignored in stage {:?}", seq, stage_name(stage));
        }
    }
}

/// 启动 tick 线程，定时发送 StreamingTick 命令
fn start_tick_thread(tx: Sender<Command>, streaming_active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while streaming_active.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(STREAMING_TICK_INTERVAL_MS));
            if streaming_active.load(Ordering::Relaxed) {
                if tx.send(Command::StreamingTick).is_err() {
                    break;
                }
            }
        }
        debug!("Streaming tick thread exited");
    });
}

/// 处理 PolishDone 命令：把润色结果写回 Transcript。
fn handle_polish_done(
    stage: &mut Stage,
    result: Result<String, String>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    _tx: &Sender<Command>,
) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("PolishDone ignored in stage {:?}", stage_name(stage));
            return;
        }
    };
    match result {
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                transcript.on_polish_failed();
                return;
            }
            transcript.on_polish_done(polished);
            // 中间润色入库 polished（polish_model 传 config.polish_llm，与 PasteDone 一致，便于统计）
            let cmd = DbCommand::UpdatePolished {
                id: transcript.id,
                text: transcript.polished().to_string(),
                status: "done".to_string(),
                model: Some(config.polish_llm.clone()),
            };
            if let Err(e) = get_db_sender().send(cmd) {
                warn!("Queue DB update_polished failed: {}", e);
            }
            if !transcript.full().is_empty() {
                crate::result_window::update_result(app_handle, &transcript.display_text());
            }
        }
        Err(e) => {
            warn!("Polish failed: {}, keeping previous", e);
            transcript.on_polish_failed();
        }
    }
}

fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Streaming { .. } => "Streaming",
        Stage::VadSegmented { .. } => "VadSegmented",
        Stage::WaitingCompletion { .. } => "WaitingCompletion",
        Stage::Polishing { .. } => "Polishing",
        Stage::Pasting { .. } => "Pasting",
    }
}

enum DbCommand {
    Insert {
        id: i64,
        text: String,
        engine: String,
        engine_mode: Option<String>,
    },
    UpdateRaw {
        id: i64,
        text: String,
    },
    UpdatePolished {
        id: i64,
        text: String,
        status: String,
        model: Option<String>,
    },
    Finalize {
        id: i64,
        raw_text: String,
        polished_text: Option<String>,
        polish_status: String,
        polish_model: Option<String>,
        duration_ms: Option<i64>,
    },
}

static DB_SENDER: std::sync::OnceLock<std::sync::mpsc::Sender<DbCommand>> = std::sync::OnceLock::new();

/// 关机标志：置位后后台线程排空队列再退出（避免入队未处理的命令丢失）。
static DB_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// 后台写线程句柄（shutdown_db 用于 join，等待排空完成）。
/// 用 `Mutex<Option<>>` 包裹：`JoinHandle::join` 需要所有权，shutdown 时 take 出来 join。
static DB_HANDLE: std::sync::OnceLock<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>> =
    std::sync::OnceLock::new();

/// 处理单条 DB 命令（主循环与关机排空共用）。
fn process_db_command(cmd: DbCommand) {
    match cmd {
        DbCommand::Insert { id, text, engine, engine_mode } => {
            if let Err(e) = octopus_asr::db::insert_transcription_at_id(
                id,
                &text,
                &engine,
                engine_mode.as_deref(),
            ) {
                warn!("Background DB insert failed: {}", e);
            }
        }
        DbCommand::UpdateRaw { id, text } => {
            if let Err(e) = octopus_asr::db::update_raw_text(id, &text) {
                warn!("Background DB update_raw_text failed: {}", e);
            }
        }
        DbCommand::UpdatePolished { id, text, status, model } => {
            if let Err(e) = octopus_asr::db::update_polished(
                id,
                &text,
                &status,
                model.as_deref(),
            ) {
                warn!("Background DB update_polished failed: {}", e);
            }
        }
        DbCommand::Finalize { id, raw_text, polished_text, polish_status, polish_model, duration_ms } => {
            if let Err(e) = octopus_asr::db::finalize_transcription(
                id,
                &raw_text,
                polished_text.as_deref(),
                &polish_status,
                polish_model.as_deref(),
                duration_ms,
            ) {
                warn!("Background DB finalize failed: {}", e);
            }
        }
    }
}

/// 排空队列中剩余命令（关机 / 断连后调用）。FIFO 顺序由 channel 保证。
fn drain_db_queue(rx: &std::sync::mpsc::Receiver<DbCommand>) {
    let mut drained = 0u32;
    while let Ok(cmd) = rx.try_recv() {
        process_db_command(cmd);
        drained += 1;
    }
    if drained > 0 {
        info!("DB drain: flushed {} queued command(s)", drained);
    }
}

/// 应用退出前调用：通知后台 DB 线程排空剩余命令并等待退出。
///
/// 背景：DB 写入为非阻塞 actor 模式（调用方 send 后即返回，真实落库在后台线程）。
/// 若不 drain，常见丢失路径为「录音结束 → Finalize 入队 → 用户立即退出 → 后台线程
/// 被进程 kill，队列里 Finalize 未落库」→ 该条记录停留在未 finalize 态。挂到 Tauri
/// `RunEvent::ExitRequested` 后即可保证关机前落库。仅当 actor 已初始化时才需要等待。
pub fn shutdown_db() {
    if DB_SENDER.get().is_some() {
        DB_SHUTDOWN.store(true, Ordering::SeqCst);
        if let Some(cell) = DB_HANDLE.get() {
            if let Some(handle) = cell.lock().unwrap().take() {
                let _ = handle.join();
            }
        }
        info!("Background DB writer drained and joined");
    }
}

fn get_db_sender() -> &'static std::sync::mpsc::Sender<DbCommand> {
    DB_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<DbCommand>();
        let handle = std::thread::spawn(move || {
            info!("Background DB writer thread started");
            loop {
                // 关机：先排空队列再退出（保留 FIFO 顺序的剩余命令）
                if DB_SHUTDOWN.load(Ordering::SeqCst) {
                    drain_db_queue(&rx);
                    break;
                }
                // recv_timeout：周期性唤醒以轮询关机标志（最长 200ms 延迟，退出场景可接受）
                match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(cmd) => process_db_command(cmd),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // 所有 Sender drop（理论上不会发生，DB_SENDER 为 &'static）；
                        // 防御性排空后退出
                        drain_db_queue(&rx);
                        break;
                    }
                }
            }
            info!("Background DB writer thread exiting");
        });
        let _ = DB_HANDLE.set(std::sync::Mutex::new(Some(handle)));
        tx
    })
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
            engine: engine.to_string(),
            engine_mode: Some(engine_mode.to_string()),
        };
        sender.send(cmd).map_err(|e| format!("Queue DB insert failed: {}", e))?;
        transcript.mark_db_inserted();
    } else {
        let cmd = DbCommand::UpdateRaw {
            id: transcript.id,
            text: transcript.db_text(),
        };
        sender.send(cmd).map_err(|e| format!("Queue DB update_raw failed: {}", e))?;
    }
    Ok(())
}

