// src/coordinator.rs

use crate::audio::SharedAudioState;
use crate::config::DesktopConfig;
use crate::engine::TranscriptionEngine;
use crate::paste;
use log::{debug, error, info};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// 协调器命令
enum Command {
    /// 切换录音状态（开始/停止）
    Toggle,
    /// 取消当前操作
    Cancel,
    /// 转录完成
    TranscriptionDone { text: Result<String, String> },
    /// 粘贴完成
    PasteDone,
}

/// 协调器阶段
enum Stage {
    Idle,
    Recording,
    Processing,
    Pasting,
}

/// 录音生命周期协调器
/// 单线程串行化所有事件，消除竞态条件
///
/// `tx` is wrapped in `Mutex` to satisfy Tauri's `Send + Sync` requirement
/// for managed state.
pub struct Coordinator {
    tx: std::sync::Mutex<Sender<Command>>,
}

impl Coordinator {
    pub fn new(
        engine: Arc<dyn TranscriptionEngine>,
        audio: Arc<SharedAudioState>,
        config: DesktopConfig,
        app_handle: tauri::AppHandle,
    ) -> Self {
        let (tx, rx): (Sender<Command>, Receiver<Command>) = mpsc::channel();
        let tx_self = tx.clone();

        std::thread::spawn(move || {
            let mut stage = Stage::Idle;

            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Command::Toggle => match stage {
                        Stage::Idle => {
                            info!("Toggle: starting recording");
                            if let Err(e) = audio.start() {
                                error!("Failed to start recording: {}", e);
                                continue;
                            }
                            stage = Stage::Recording;
                            crate::overlay::show_overlay(&app_handle, "recording");
                            crate::tray::update_tray_label(
                                &app_handle,
                                crate::tray::TrayState::Recording,
                            );
                        }
                        Stage::Recording => {
                            info!("Toggle: stopping recording, starting transcription");
                            stage = Stage::Processing;
                            crate::overlay::show_overlay(&app_handle, "transcribing");
                            crate::tray::update_tray_label(
                                &app_handle,
                                crate::tray::TrayState::Processing,
                            );

                            let samples = audio.stop().unwrap_or_default();

                            if samples.is_empty() {
                                info!("No audio samples, returning to idle");
                                stage = Stage::Idle;
                                crate::overlay::hide_overlay(&app_handle);
                                crate::tray::update_tray_label(
                                    &app_handle,
                                    crate::tray::TrayState::Idle,
                                );
                                continue;
                            }

                            // VAD filter
                            let speech_samples = {
                                match octopus_asr::config::find_silero_vad() {
                                    Ok(vad_path) => {
                                        match octopus_asr::vad::SileroVad::new(&vad_path) {
                                            Ok(mut vad) => {
                                                let speech = octopus_asr::audio::filter_speech(
                                                    &samples, &mut vad, 480, 0.5,
                                                );
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
                                        }
                                    }
                                    Err(e) => {
                                        error!("VAD not found: {}, using raw samples", e);
                                        samples
                                    }
                                }
                            };

                            if speech_samples.is_empty() {
                                stage = Stage::Idle;
                                crate::overlay::hide_overlay(&app_handle);
                                crate::tray::update_tray_label(
                                    &app_handle,
                                    crate::tray::TrayState::Idle,
                                );
                                continue;
                            }

                            // Transcribe in a dedicated thread with its own tokio runtime
                            // (coordinator thread has no tokio runtime, so tokio::spawn would panic)
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
                                let result = rt.block_on(engine.transcribe(
                                    &speech_samples,
                                    &language,
                                    &asr_engine,
                                ));
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
                        Stage::Processing | Stage::Pasting => {
                            debug!("Toggle ignored: busy in {:?}", stage_name(&stage));
                        }
                    },
                    Command::Cancel => {
                        if matches!(stage, Stage::Recording) {
                            info!("Cancel: stopping recording");
                            let _ = audio.stop();
                        }
                        stage = Stage::Idle;
                        crate::overlay::hide_overlay(&app_handle);
                        crate::tray::update_tray_label(&app_handle, crate::tray::TrayState::Idle);
                    }
                    Command::TranscriptionDone { text } => match text {
                        Ok(transcription) => {
                            info!("Transcription: '{}'", transcription);
                            if transcription.is_empty() {
                                stage = Stage::Idle;
                                crate::overlay::hide_overlay(&app_handle);
                                crate::tray::update_tray_label(
                                    &app_handle,
                                    crate::tray::TrayState::Idle,
                                );
                                continue;
                            }

                            stage = Stage::Pasting;
                            let config = config.clone();
                            let tx_inner = tx.clone();
                            let tx_fallback = tx.clone();
                            let handle_for_closure = app_handle.clone();

                            app_handle
                                .run_on_main_thread(move || {
                                    if let Err(e) =
                                        paste::paste(&transcription, &handle_for_closure, &config)
                                    {
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
                            stage = Stage::Idle;
                            crate::overlay::hide_overlay(&app_handle);
                            crate::tray::update_tray_label(
                                &app_handle,
                                crate::tray::TrayState::Idle,
                            );
                        }
                    },
                    Command::PasteDone => {
                        info!("Paste complete, returning to idle");
                        stage = Stage::Idle;
                        crate::overlay::hide_overlay(&app_handle);
                        crate::tray::update_tray_label(&app_handle, crate::tray::TrayState::Idle);
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

fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Recording => "Recording",
        Stage::Processing => "Processing",
        Stage::Pasting => "Pasting",
    }
}
