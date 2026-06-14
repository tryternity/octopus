// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::AppConfig;
use crate::config::PolishMode;
use crate::engine::TranscriptionEngine;
use crate::paste;
use crate::streaming_engine::StreamingSession;
use crate::transcript::Transcript;
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
    },
    /// 粘贴完成
    PasteDone,
    /// 润色完成
    PolishDone { result: Result<String, String> },
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
        vad: octopus_asr::vad::SileroVad,
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
        /// 引擎名（入库用）
        engine: String,
        /// "streaming" | "vad_segmented"
        engine_mode: String,
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
    ) -> Self {
        let (tx, rx): (Sender<Command>, Receiver<Command>) = mpsc::channel();
        let tx_self = tx.clone();

        let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine(&config);

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
                        handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                    }
                    Command::VadSegmentedTick => {
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
                    Command::TranscriptionDone { text, seq } => {
                        handle_transcription_done(
                            &mut stage,
                            text,
                            seq,
                            &config,
                            &app_handle,
                            &tx,
                        );
                    }
                    Command::PasteDone => {
                        // 入库（从 Pasting 取数据；用户编辑已反映到 polished_text）
                        if let Stage::Pasting {
                            id: _,
                            raw_text,
                            polished_text,
                            polish_status,
                            engine,
                            engine_mode,
                        } = &stage
                        {
                            let polish_model = if polish_status == "done" {
                                Some(config.llm_model.as_str())
                            } else {
                                None
                            };
                            // polished_text 仅 done 时入库（spec §5.2：polished 仅 done 有值）
                            let polished_for_db = if polish_status == "done" {
                                Some(polished_text.as_str())
                            } else {
                                None
                            };
                            if let Err(e) = octopus_asr::db::insert_transcription(
                                raw_text,
                                polished_for_db,
                                polish_status,
                                polish_model,
                                engine,
                                Some(engine_mode),
                            ) {
                                log::warn!("DB insert transcription failed: {}", e);
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

            if let Err(e) = audio.start() {
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
                    }
                }
            } else {
                // 非流式模式：使用 VAD 伪流式分段识别
                match octopus_asr::config::find_silero_vad() {
                    Ok(path) => match octopus_asr::vad::SileroVad::new(&path) {
                        Ok(vad) => {
                            crate::result_window::show_result(app_handle, "正在聆听…");
                            crate::tray::update_tray_label(
                                app_handle,
                                crate::tray::TrayState::Recording,
                            );

                            let tick_active = Arc::new(AtomicBool::new(true));
                            start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());

                            *stage = Stage::VadSegmented {
                                vad,
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

            // 停止录音
            let _ = audio.stop();

            // 排空剩余音频并发送识别
            let remaining = audio.drain_samples();
            if !remaining.is_empty() {
                audio_buffer.extend_from_slice(&remaining);
            }

            // 如果缓冲区有语音，发送最后一次识别
            if *has_speech && !audio_buffer.is_empty() {
                let mut send_buffer = overlap_tail.clone();
                send_buffer.extend_from_slice(audio_buffer);
                let speech_samples = filter_speech_from_buffer(&send_buffer);
                if !speech_samples.is_empty() {
                    let seq = *next_seq;
                    *next_seq += 1;
                    *active_count += 1;
                    spawn_offline_transcription_with_seq(
                        engine, config, tx, speech_samples, seq,
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
                    start_pasting(
                        stage,
                        &final_text,
                        tr,
                        &config.asr_engine,
                        "vad_segmented",
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
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // 显示最终结果（润色前的 display_text，含中间润色结果）
            crate::result_window::show_result(app_handle, &transcript.display_text());

            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            start_pasting(
                stage,
                &combined,
                tr,
                &config.asr_engine,
                "streaming",
                config,
                app_handle,
                tx,
            );
        }

        Stage::WaitingCompletion { .. } => {
            debug!("Toggle ignored: waiting for transcription completion");
        }

        Stage::Pasting { .. } => {
            debug!("Toggle ignored: busy pasting");
        }
    }
}

/// 开始粘贴阶段（支持最终润色）。`transcript` 移交进 Pasting 持 id（Task 6 用）。
fn start_pasting(
    stage: &mut Stage,
    text: &str,
    transcript: Transcript,
    engine: &str,
    engine_mode: &str,
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

    // 最终润色 + 状态（基于调用结果，非文本比较）
    let (final_text, polish_status) = match crate::config::llm_config(&config) {
        None => (text.to_string(), "off"),
        Some(llm_config) => match octopus_llm::polish(text, &llm_config) {
            Ok(polished) if !polished.is_empty() => {
                info!(
                    "Final polish: {} → {} chars",
                    text.chars().count(),
                    polished.chars().count()
                );
                (polished, "done")
            }
            Ok(_) => {
                warn!("Final polish returned empty, using original");
                (text.to_string(), "failed")
            }
            Err(e) => {
                warn!("Final polish failed: {}, using original", e);
                (text.to_string(), "failed")
            }
        },
    };

    crate::result_window::show_result(app_handle, &final_text);

    let id = transcript.id;
    *stage = Stage::Pasting {
        id,
        raw_text: transcript.db_text(),
        polished_text: if polish_status == "done" {
            final_text.clone()
        } else {
            String::new()
        },
        polish_status: polish_status.to_string(),
        engine: engine.to_string(),
        engine_mode: engine_mode.to_string(),
    };
    let config = config.clone();
    let tx_inner = tx.clone();
    let tx_fallback = tx.clone();
    let handle_for_closure = app_handle.clone();
    let text_to_paste = final_text;

    app_handle
        .run_on_main_thread(move || {
            if let Err(e) = paste::paste(&text_to_paste, &handle_for_closure, &config) {
                error!("Paste failed: {}", e);
            }
            let _ = tx_inner.send(Command::PasteDone);
        })
        .unwrap_or_else(|e| {
            error!("run_on_main_thread failed: {:?}", e);
            let _ = tx_fallback.send(Command::PasteDone);
        });
}

/// 消费已完成序号的结果，把新段追加到 Transcript。
fn consume_completed_results(
    completed_seq: &mut u64,
    completed_results: &mut HashMap<u64, String>,
    transcript: &mut Transcript,
) {
    while let Some(text) = completed_results.remove(completed_seq) {
        if !text.is_empty() {
            // 段间加逗号（已有文本且新段不以标点开头）
            if !transcript.full().is_empty()
                && !text.starts_with(|c: char| ",.，。！？!?\n".contains(c))
            {
                transcript.append_segment("，");
            }
            transcript.append_segment(&text);
        }
        *completed_seq += 1;
    }
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

        // 3. VAD 检测本段语音/静音比例
        let speech_ratio = compute_speech_ratio(vad, &samples);
        if speech_ratio >= 0.3 {
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

            // VAD 过滤语音片段
            let speech_samples = filter_speech_from_buffer(&send_buffer);
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
                    engine, config, tx, speech_samples, seq,
                );
            }
        }

        // 5. 更新 result window
        if !transcript.full().is_empty() {
            crate::result_window::update_result(app_handle, &transcript.display_text());
        }

        // 6. 检查润色（Task 5 接入；此处保留占位签名）
        check_and_trigger_polish(transcript, *silence_duration, config, tx);
    }
}

/// 计算音频片段中语音帧的比例
fn compute_speech_ratio(vad: &mut octopus_asr::vad::SileroVad, samples: &[f32]) -> f64 {
    let mut speech_chunks = 0usize;
    let mut silent_chunks = 0usize;

    for chunk in samples.chunks(VAD_CHUNK_SIZE) {
        if chunk.len() < VAD_CHUNK_SIZE {
            break;
        }
        match vad.compute(chunk) {
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

    let total = speech_chunks + silent_chunks;
    if total == 0 {
        return 0.0;
    }
    speech_chunks as f64 / total as f64
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
) {
    let engine = engine.clone();
    let language = config.language.clone();
    let asr_engine = config.asr_engine.clone();
    let tx = tx.clone();
    let samples_len = speech_samples.len();
    let duration = samples_len as f64 / 16000.0;

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for transcription");

        let start = Instant::now();
        let result = rt.block_on(engine.transcribe(&speech_samples, &language, &asr_engine));
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
        };
        let _ = tx.send(msg);
    });
}

/// 对缓冲区音频做 VAD 过滤（创建独立 VAD 实例，避免状态污染）
fn filter_speech_from_buffer(samples: &[f32]) -> Vec<f32> {
    match octopus_asr::config::find_silero_vad() {
        Ok(vad_path) => match octopus_asr::vad::SileroVad::new(&vad_path) {
            Ok(mut vad) => {
                let speech = octopus_asr::audio::filter_speech(samples, &mut vad, 480, 0.5);
                if speech.is_empty() {
                    debug!("VadSegmented: no speech detected in buffer");
                    Vec::new()
                } else {
                    speech
                }
            }
            Err(e) => {
                warn!("VAD init failed for buffer filter: {}, using raw samples", e);
                samples.to_vec()
            }
        },
        Err(e) => {
            warn!("VAD not found for buffer filter: {}, using raw samples", e);
            samples.to_vec()
        }
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

/// 检查润色条件并触发（停顿驱动润色，Task 5 实现）。
///
/// 当前为占位实现：Task 4 仅做文本流接入，润色触发逻辑下沉到 Task 5。
fn check_and_trigger_polish(
    transcript: &mut Transcript,
    _silence_duration: f64,
    _config: &AppConfig,
    _tx: &Sender<Command>,
) {
    // 占位：Task 5 实现停顿驱动润色（snapshot_for_polish + spawn_polish_thread）
    let _ = transcript;
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

            let speech_ratio = speech_chunks as f64 / total_chunks as f64;
            let chunk_duration = VAD_CHUNK_SIZE as f64 / 16000.0; // 每块时长（秒）

            if speech_ratio < 0.3 {
                // 本段大部分是静音 → 累积静音时长
                *silence_duration += total_chunks as f64 * chunk_duration;
            } else {
                // 本段包含语音 → 重置静音计时
                *silence_duration = 0.0;
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
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(t) => {
                    if !t.is_empty() {
                        info!("VadSegmented transcription seq={}: '{}'", seq, t);
                        completed_results.insert(seq, t);
                    }
                }
                Err(e) => error!("VadSegmented transcription seq={} failed: {}", seq, e),
            }

            // 消费连续序号的结果（追加到 Transcript）
            consume_completed_results(completed_seq, completed_results, transcript);

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
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(t) => {
                    if !t.is_empty() {
                        info!("WaitingCompletion transcription seq={}: '{}'", seq, t);
                        completed_results.insert(seq, t);
                    }
                }
                Err(e) => error!("WaitingCompletion transcription seq={} failed: {}", seq, e),
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
                    start_pasting(
                        stage,
                        &final_text,
                        tr,
                        &config.asr_engine,
                        "vad_segmented",
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
    _config: &AppConfig,
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
        Stage::Pasting { .. } => "Pasting",
    }
}
