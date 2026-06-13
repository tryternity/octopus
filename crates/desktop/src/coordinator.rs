// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::DesktopConfig;
use crate::engine::TranscriptionEngine;
use crate::paste;
use crate::streaming_engine::StreamingSession;
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
}

/// 协调器阶段
enum Stage {
    Idle,
    /// 流式识别：边录边识别
    Streaming {
        engine: StreamingSession,
        accumulated_text: String,
        streaming_active: Arc<AtomicBool>,
        /// VAD 实例，用于检测静音间隔
        vad: Option<octopus_asr::vad::SileroVad>,
        /// 累积静音时长（秒），超过阈值后恢复说话时插入标点
        silence_duration: f64,
    },
    /// VAD 伪流式：tick 驱动分段识别（非流式引擎使用）
    VadSegmented {
        vad: octopus_asr::vad::SileroVad,
        /// 音频累积缓冲区（16kHz mono f32）
        audio_buffer: Vec<f32>,
        /// 前一窗口末尾 0.2s 的 overlap 音频
        overlap_tail: Vec<f32>,
        /// 累积识别文本
        accumulated_text: String,
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
        accumulated_text: String,
        active_count: u32,
        completed_seq: u64,
        completed_results: HashMap<u64, String>,
    },
    /// 粘贴中
    Pasting,
}

/// VAD 静音判定阈值
const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// VAD 分块大小（采样点数）
const VAD_CHUNK_SIZE: usize = 512;
/// 插入标点的静音时长阈值（秒）
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;

/// VAD 伪流式 tick 间隔（毫秒）
const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 300;

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
        config: DesktopConfig,
        app_handle: tauri::AppHandle,
    ) -> Self {
        let (tx, rx): (Sender<Command>, Receiver<Command>) = mpsc::channel();
        let tx_self = tx.clone();

        let use_streaming = config.engine_mode == "embedded" && config.is_streaming_engine();

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
                        handle_streaming_tick(&mut stage, &audio, &app_handle);
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
                        info!("Paste complete, returning to idle");
                        stage = Stage::Idle;
                        crate::overlay::hide_overlay(&app_handle);
                        crate::result_window::clear_result(&app_handle);
                        crate::tray::update_tray_label(
                            &app_handle,
                            crate::tray::TrayState::Idle,
                        );
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
    config: &DesktopConfig,
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
                            accumulated_text: String::new(),
                            streaming_active,
                            vad,
                            silence_duration: 0.0,
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
                                accumulated_text: String::new(),
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
            accumulated_text,
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
            let text = accumulated_text.clone();
            let cseq = *completed_seq;
            let cresults = std::mem::take(completed_results);

            if active > 0 {
                *stage = Stage::WaitingCompletion {
                    accumulated_text: text,
                    active_count: active,
                    completed_seq: cseq,
                    completed_results: cresults,
                };
            } else {
                // 无进行中识别，直接粘贴
                start_pasting(stage, &text, config, app_handle, tx);
            }
        }

        Stage::Streaming {
            engine: streaming_engine,
            accumulated_text,
            streaming_active,
            ..
        } => {
            // 流式模式：停止流式，获取最终文本，粘贴
            info!("Toggle: stopping streaming, finalizing");

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
                    accumulated_text.clone()
                }
            };

            // 重置引擎
            streaming_engine.reset();

            // 停止录音
            let _ = audio.stop();

            // 合并最终文本
            let combined = if final_text.is_empty() {
                accumulated_text.clone()
            } else {
                final_text
            };

            info!("Final streaming text: '{}'", combined);

            if combined.is_empty() {
                *stage = Stage::Idle;
                crate::overlay::hide_overlay(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // 显示最终结果
            crate::result_window::show_result(app_handle, &combined);

            // 粘贴
            start_pasting(stage, &combined, config, app_handle, tx);
        }

        Stage::WaitingCompletion { .. } => {
            debug!("Toggle ignored: waiting for transcription completion");
        }

        Stage::Pasting => {
            debug!("Toggle ignored: busy pasting");
        }
    }
}

/// 开始粘贴阶段
fn start_pasting(
    stage: &mut Stage,
    text: &str,
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if text.is_empty() {
        *stage = Stage::Idle;
        crate::overlay::hide_overlay(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    crate::result_window::show_result(app_handle, text);

    *stage = Stage::Pasting;
    let config = config.clone();
    let tx_inner = tx.clone();
    let tx_fallback = tx.clone();
    let handle_for_closure = app_handle.clone();
    let text_to_paste = text.to_string();

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

/// 消费已完成序号的结果，返回新拼接的文本
fn consume_completed_results(
    completed_seq: &mut u64,
    completed_results: &mut HashMap<u64, String>,
    accumulated_text: &mut String,
) {
    while let Some(text) = completed_results.remove(completed_seq) {
        if !text.is_empty() {
            // 段间加逗号分隔（如果已有文本且新文本不以标点开头）
            if !accumulated_text.is_empty()
                && !text.starts_with(|c: char| ",.，。！？!?\n".contains(c))
            {
                accumulated_text.push('，');
            }
            accumulated_text.push_str(&text);
        }
        *completed_seq += 1;
    }
}

/// 处理 VadSegmentedTick 命令
fn handle_vad_segmented_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if let Stage::VadSegmented {
        vad,
        audio_buffer,
        overlap_tail,
        accumulated_text,
        silence_duration,
        has_speech,
        active_count,
        next_seq,
        completed_seq,
        completed_results,
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

        // 4. 判断是否发送识别（segment_duration 秒 / segment_silence 毫秒）
        let segment_samples = (config.segment_duration * 16000.0) as usize;
        let silence_ms = *silence_duration * 1000.0;
        let should_send = *has_speech
            && (audio_buffer.len() >= segment_samples || silence_ms >= config.segment_silence);

        if should_send {
            // 保存末尾作为下一段 overlap（segment_overlap 毫秒 → 采样点数）
            let overlap_samples = (config.segment_overlap * 16.0) as usize;
            let overlap_start = audio_buffer.len().saturating_sub(overlap_samples);
            *overlap_tail = audio_buffer[overlap_start..].to_vec();

            // 构建发送缓冲区：前一窗口 overlap + 当前缓冲区
            let mut send_buffer = overlap_tail.clone();
            send_buffer.extend_from_slice(audio_buffer);

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
                    "VadSegmented: sending segment seq={}, samples={}, active_count={}",
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
        if !accumulated_text.is_empty() {
            crate::result_window::update_result(app_handle, accumulated_text);
        }
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
    config: &DesktopConfig,
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

/// 处理 StreamingTick 命令
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    app_handle: &tauri::AppHandle,
) {
    if let Stage::Streaming {
        engine,
        accumulated_text,
        vad,
        silence_duration,
        ..
    } = stage
    {
        // 排空音频缓冲区
        let samples = audio.drain_samples();
        if samples.is_empty() {
            return;
        }

        // 用 VAD 检测本段音频中语音/静音的比例
        let was_silent = detect_silence_gap(vad, &samples, silence_duration);

        // 送入流式引擎（如果之前有足够长的静音间隔，插入逗号）
        match engine.accept_samples(&samples, was_silent) {
            Ok(Some(new_text)) => {
                *accumulated_text = new_text;
                debug!("Partial: '{}'", accumulated_text);

                // 更新 result window 显示
                crate::result_window::update_result(app_handle, accumulated_text);
            }
            Ok(None) => {
                // 没有新结果
            }
            Err(e) => {
                warn!("Streaming accept_samples error: {}", e);
            }
        }
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
                debug!(
                    "VAD: silence accumulated {:.2}s (speech_ratio={:.2})",
                    silence_duration, speech_ratio
                );
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
        Stage::VadSegmented { tick_active, .. } => {
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
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    match stage {
        Stage::VadSegmented {
            accumulated_text,
            active_count,
            completed_seq,
            completed_results,
            ..
        } => {
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(transcription) => {
                    info!("VadSegmented transcription seq={}: '{}'", seq, transcription);
                    if !transcription.is_empty() {
                        completed_results.insert(seq, transcription);
                    }
                }
                Err(e) => {
                    error!("VadSegmented transcription seq={} failed: {}", seq, e);
                }
            }

            // 消费连续序号的结果
            consume_completed_results(completed_seq, completed_results, accumulated_text);

            // 更新 result window
            if !accumulated_text.is_empty() {
                crate::result_window::update_result(app_handle, accumulated_text);
            }
        }

        Stage::WaitingCompletion {
            accumulated_text,
            active_count,
            completed_seq,
            completed_results,
        } => {
            *active_count = active_count.saturating_sub(1);

            match text {
                Ok(transcription) => {
                    info!("WaitingCompletion transcription seq={}: '{}'", seq, transcription);
                    if !transcription.is_empty() {
                        completed_results.insert(seq, transcription);
                    }
                }
                Err(e) => {
                    error!("WaitingCompletion transcription seq={} failed: {}", seq, e);
                }
            }

            // 消费连续序号的结果
            consume_completed_results(completed_seq, completed_results, accumulated_text);

            if *active_count == 0 {
                // 所有识别完成，拼接最终文本并粘贴
                let text = std::mem::take(accumulated_text);
                if !text.is_empty() {
                    // 追加句号
                    let final_text = if text.ends_with(|c: char| ",.，。！？!?\n".contains(c)) {
                        text
                    } else {
                        format!("{}。", text)
                    };
                    start_pasting(stage, &final_text, config, app_handle, tx);
                } else {
                    *stage = Stage::Idle;
                    crate::overlay::hide_overlay(app_handle);
                    crate::result_window::hide_result(app_handle);
                    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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

fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Streaming { .. } => "Streaming",
        Stage::VadSegmented { .. } => "VadSegmented",
        Stage::WaitingCompletion { .. } => "WaitingCompletion",
        Stage::Pasting => "Pasting",
    }
}
