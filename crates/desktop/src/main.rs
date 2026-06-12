#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod coordinator;
mod engine;
mod engine_embedded;
#[cfg(feature = "remote-grpc")]
mod engine_grpc;
#[cfg(feature = "remote-ws")]
mod engine_ws;
mod overlay;
mod paste;
mod shortcut;
mod tray;

use coordinator::Coordinator;
use engine::TranscriptionEngine;
use engine_embedded::EmbeddedEngine;
use log::info;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = config::load_desktop_config().expect("Failed to load config");
    info!(
        "Config: engine={}, mode={}, shortcut={}",
        config.asr_engine, config.engine_mode, config.shortcut
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info!("Single instance: re-activated");
            if let Some(coordinator) = app.try_state::<Coordinator>() {
                coordinator.toggle();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_os::init())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Debug)
                .level_for("enigo", log::LevelFilter::Warn)
                .level_for("tao", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // 1. Create engine
            let engine: Arc<dyn TranscriptionEngine> = match config.engine_mode.as_str() {
                "embedded" => Arc::new(EmbeddedEngine),
                #[cfg(feature = "remote-ws")]
                "websocket" => Arc::new(engine_ws::WsRemoteEngine::new(&config.remote_url)),
                #[cfg(feature = "remote-grpc")]
                "grpc" => Arc::new(engine_grpc::GrpcRemoteEngine::new(&config.grpc_endpoint)),
                other => {
                    log::warn!("Unknown engine_mode '{}', falling back to embedded", other);
                    Arc::new(EmbeddedEngine)
                }
            };

            // 2. Create AudioRecorder and open the device
            let mut recorder = audio::AudioRecorder::new(&config.microphone)
                .expect("Failed to create AudioRecorder");
            recorder.open().expect("Failed to open audio device");
            let audio_state = recorder.shared();

            // Recorder must stay alive for the stream to remain active.
            // Leak it intentionally — it lives for the entire app lifetime.
            // The stream callback holds Arc<SharedAudioState> independently,
            // so the recorder itself only needs to not be dropped.
            std::mem::forget(recorder);

            // 3. Create Coordinator
            let coordinator =
                Coordinator::new(engine, audio_state, config.clone(), app.handle().clone());
            app.manage(coordinator);

            // 4. Create Tray
            tray::create_tray(app.handle(), &config);

            // 5. Create Overlay
            overlay::create_overlay(app.handle(), &config);

            // 6. Register global shortcut
            if let Err(e) = shortcut::register_shortcut(app.handle(), &config.shortcut) {
                log::error!("Failed to register shortcut: {}. Use tray menu instead.", e);
            }

            info!("octopus-desktop initialized");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {});
}

fn main() {
    run();
}
