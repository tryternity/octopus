// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::DesktopConfig;
use crate::engine::TranscriptionEngine;
use crate::paste;
use crate::streaming_engine::StreamingSession;
use log::{debug, error, info, warn};
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
    /// 转录完成（离线模式或远程模式使用）
    TranscriptionDone {
        text: Result<String, String>,
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
    /// 离线模式：录音中
    Recording,
    /// 离线模式：离线识别中
    Processing,
    /// 粘贴中
    Pasting,
}

/// VAD 静音判定阈值
const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// VAD 分块大小（采样点数）
const VAD_CHUNK_SIZE: usize = 512;
/// 插入标点的静音时长阈值（秒）
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;

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
                    Command::Cancel => {
                        handle_cancel(&mut stage, &audio, &app_handle);
                    }
                    Command::TranscriptionDone { text } => {
                        handle_transcription_done(
                            &mut stage,
                            text,
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
            info!("Toggle: starting {}", if use_streaming { "streaming" } else { "recording" });

            if let Err(e) = audio.start() {
                error!("Failed to start recording: {}", e);
                return;
            }

            if use_streaming {
                // 流式模式：创建 StreamingSession 并启动 tick 线程
                match StreamingSession::new(&config.asr_engine) {
                    Ok(streaming_engine) => {
                        // 流式模式：只显示 result window，不显示 overlay
                        crate::result_window::show_result(app_handle, "");
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
                // 离线模式：原有 Recording → Processing 流程
                crate::overlay::show_overlay(app_handle, "recording");
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);
                *stage = Stage::Recording;
            }
        }

        Stage::Recording => {
            // 离线模式：停止录音，开始离线识别
            info!("Toggle: stopping recording, starting offline transcription");
            *stage = Stage::Processing;
            crate::overlay::show_overlay(app_handle, "transcribing");
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Processing);

            let samples = audio.stop().unwrap_or_default();
            if samples.is_empty() {
                info!("No audio samples, returning to idle");
                *stage = Stage::Idle;
                crate::overlay::hide_overlay(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // VAD filter
            let speech_samples = filter_speech_samples(samples);

            if speech_samples.is_empty() {
                *stage = Stage::Idle;
                crate::overlay::hide_overlay(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // 离线识别线程
            spawn_offline_transcription(engine, config, tx, speech_samples);
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

            // 显示最终结果（result window 已在流式期间显示，这里更新最终文本）
            crate::result_window::show_result(app_handle, &combined);

            // 粘贴
            *stage = Stage::Pasting;
            let config = config.clone();
            let tx_inner = tx.clone();
            let tx_fallback = tx.clone();
            let handle_for_closure = app_handle.clone();
            let text_to_paste = combined;

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

        Stage::Processing | Stage::Pasting => {
            debug!("Toggle ignored: busy in {:?}", stage_name(stage));
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
        Stage::Recording => {
            info!("Cancel: stopping recording");
            let _ = audio.stop();
        }
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
        _ => {}
    }
    *stage = Stage::Idle;
    crate::overlay::hide_overlay(app_handle);
    crate::result_window::hide_result(app_handle);
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
}

/// 处理 TranscriptionDone 命令（离线模式）
fn handle_transcription_done(
    stage: &mut Stage,
    text: Result<String, String>,
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    match text {
        Ok(transcription) => {
            info!("Transcription: '{}'", transcription);
            if transcription.is_empty() {
                *stage = Stage::Idle;
                crate::overlay::hide_overlay(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }

            // 隐藏 overlay，显示结果窗口
            crate::overlay::hide_overlay(app_handle);
            crate::result_window::show_result(app_handle, &transcription);

            *stage = Stage::Pasting;
            let config = config.clone();
            let tx_inner = tx.clone();
            let tx_fallback = tx.clone();
            let handle_for_closure = app_handle.clone();

            app_handle
                .run_on_main_thread(move || {
                    if let Err(e) = paste::paste(&transcription, &handle_for_closure, &config) {
                        error!("Paste failed: {}", e);
                    }
                    let _ = tx_inner.send(Command::PasteDone);
                })
                .unwrap_or_else(|e| {
                    error!("run_on_main_thread failed: {:?}", e);
                    let _ = tx_fallback.send(Command::PasteDone);
                });
        }
        Err(e) => {
            error!("Transcription failed: {}", e);
            *stage = Stage::Idle;
            crate::overlay::hide_overlay(app_handle);
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
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

/// VAD 过滤语音片段
fn filter_speech_samples(samples: Vec<f32>) -> Vec<f32> {
    match octopus_asr::config::find_silero_vad() {
        Ok(vad_path) => match octopus_asr::vad::SileroVad::new(&vad_path) {
            Ok(mut vad) => {
                let speech = octopus_asr::audio::filter_speech(&samples, &mut vad, 480, 0.5);
                if speech.is_empty() {
                    info!("No speech detected");
                    Vec::new()
                } else {
                    speech
                }
            }
            Err(e) => {
                error!("VAD init failed: {}, using raw samples", e);
                samples
            }
        },
        Err(e) => {
            error!("VAD not found: {}, using raw samples", e);
            samples
        }
    }
}

/// 离线识别线程
fn spawn_offline_transcription(
    engine: &Arc<dyn TranscriptionEngine>,
    config: &DesktopConfig,
    tx: &Sender<Command>,
    speech_samples: Vec<f32>,
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
            "Transcription took {:.2}s (audio: {:.2}s, RTF: {:.2})",
            elapsed.as_secs_f64(),
            duration,
            elapsed.as_secs_f64() / duration
        );
        let msg = match result {
            Ok(text) => Command::TranscriptionDone { text: Ok(text) },
            Err(e) => Command::TranscriptionDone {
                text: Err(e.to_string()),
            },
        };
        let _ = tx.send(msg);
    });
}

fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Recording => "Recording",
        Stage::Streaming { .. } => "Streaming",
        Stage::Processing => "Processing",
        Stage::Pasting => "Pasting",
    }
}
