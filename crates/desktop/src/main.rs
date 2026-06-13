#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod db;
mod coordinator;
mod engine;
mod engine_embedded;
#[cfg(feature = "remote-grpc")]
mod engine_grpc;
#[cfg(feature = "remote-ws")]
mod engine_ws;
mod overlay;
mod paste;
mod result_window;
mod shortcut;
mod streaming_engine;
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

    // 初始化嵌入式 DB（建表 + 首次迁移 history.txt / model.json）
    // 失败仅告警，不阻断启动（存储禁用但应用可用）
    if let Err(e) = db::init() {
        log::error!("DB init failed: {}, storage disabled", e);
    }

    // 校验引擎模式
    if config.engine_mode == "embedded" && !config.is_streaming_engine() {
        log::info!(
            "引擎 '{}' 使用 VAD 分段伪流式模式",
            config.asr_engine
        );
    }

    // 润色配置校验
    if config.polish_enabled {
        if config.llm_secret_key.is_empty() {
            log::warn!("polish_enabled=true 但 llm_secret_key 为空，润色功能将不生效");
        } else {
            log::info!(
                "润色已启用: provider={}, model={}, interval={}s",
                config.llm_provider,
                config.llm_model,
                config.polish_interval
            );
        }
    }

    // 加载自定义润色 system prompt（~/.octopus/VOICE_POLISH.md）
    // 文件存在且非空时覆盖内置默认 prompt
    let prompt_path = octopus_asr::config::handy_home().join("VOICE_POLISH.md");
    if prompt_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&prompt_path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                octopus_llm::set_system_prompt_override(trimmed.to_string());
                log::info!("已加载自定义润色 prompt: {}", prompt_path.display());
            } else {
                log::warn!("VOICE_POLISH.md 内容为空，使用内置默认 prompt");
            }
        } else {
            log::warn!("读取 VOICE_POLISH.md 失败，使用内置默认 prompt");
        }
    }

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
                .level_for("reqwest", log::LevelFilter::Info)
                .level_for("hyper", log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // Initialize engine manager and preheat the active ASR model if embedded
            let engine_manager = Arc::new(octopus_asr::engine::AsrEngineManager::new());
            if config.engine_mode == "embedded" {
                info!("Preheating active ASR model in desktop: {}", config.asr_engine);
                let em = engine_manager.clone();
                let active_model = config.asr_engine.clone();
                std::thread::spawn(move || {
                    if let Err(e) = em.switch_model(&active_model) {
                        log::error!("Failed to preheat active ASR model {}: {}", active_model, e);
                    } else {
                        info!("Active ASR model {} preheated successfully", active_model);
                    }
                });
            }

            // 1. Create engine
            let engine: Arc<dyn TranscriptionEngine> = match config.engine_mode.as_str() {
                "embedded" => Arc::new(EmbeddedEngine::new(engine_manager.clone())),
                #[cfg(feature = "remote-ws")]
                "websocket" => Arc::new(engine_ws::WsRemoteEngine::new(&config.remote_url)),
                #[cfg(feature = "remote-grpc")]
                "grpc" => Arc::new(engine_grpc::GrpcRemoteEngine::new(&config.grpc_endpoint)),
                other => {
                    log::warn!("Unknown engine_mode '{}', falling back to embedded", other);
                    Arc::new(EmbeddedEngine::new(engine_manager.clone()))
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

            // 6. Create Result Window
            result_window::create_result_window(app.handle());

            // 7. Register global shortcut
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
