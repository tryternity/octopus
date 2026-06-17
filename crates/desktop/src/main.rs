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
mod result_window;
mod runtime_config;
mod settings_commands;
mod settings_window;
mod shortcut;
mod tray;
mod transcript;

use coordinator::Coordinator;
use engine::TranscriptionEngine;
use engine_embedded::EmbeddedEngine;
use log::info;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 安装 panic hook：把 panic 信息（含 backtrace）打到 log，避免 tauri 默认吞掉。
    // SIGTRAP 类 trap（NSException / Cocoa assertion）不会被 Rust panic 捕获，
    // 但至少能区分 panic vs 真正的 trap。
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        log::error!("PANIC at {}: {}", location, msg);
        eprintln!("PANIC at {}: {}", location, msg);
        // 打印 backtrace
        let bt = std::backtrace::Backtrace::force_capture();
        log::error!("Backtrace:\n{}", bt);
        eprintln!("Backtrace:\n{}", bt);
    }));

    let config = octopus_infra::config::load_config().expect("Failed to load config");
    info!(
        "Config: engine={}, mode={}, shortcut={}",
        config.asr_engine, config.engine_mode, config.shortcut
    );

    // 初始化嵌入式 DB（建表 + seed 默认引擎）。asr 的 load_config 首次调用时也会
    // lazy init，这里显式预热（日志早出 + 错误前置）。模型配置唯一来源即此 DB。
    // 失败仅告警，不阻断启动（识别历史写入会失败，但应用可用）
    if let Err(e) = octopus_asr::db::ensure_db() {
        log::error!("DB init failed: {}, storage disabled", e);
    }

    // 校验引擎模式
    if config.engine_mode == "embedded" && !config::is_streaming_engine(&config) {
        log::info!(
            "引擎 '{}' 使用 VAD 分段伪流式模式",
            config.asr_engine
        );
    }

    // 润色配置校验（三档模式）
    use crate::config::PolishMode;
    if config.polish_mode != PolishMode::Disabled {
        if config.polish_mode == PolishMode::Intermediate && config.polish_interval <= 0.0 {
            log::warn!(
                "polish_mode=2 但 polish_interval={}<=0，将使用下限 {}s",
                config.polish_interval,
                coordinator::MIN_POLISH_INTERVAL_SEC
            );
        }
        match crate::config::llm_config(&config) {
            Some(llm_cfg) => {
                let mode_str = match config.polish_mode {
                    PolishMode::FinalOnly => "仅最终润色",
                    PolishMode::Intermediate => "中间+最终",
                    _ => unreachable!(),
                };
                if config.polish_mode == PolishMode::Intermediate {
                    log::info!(
                        "润色模式: {} (interval={}s, provider={}, model={})",
                        mode_str,
                        config.polish_interval,
                        llm_cfg.provider,
                        llm_cfg.model
                    );
                } else {
                    log::info!(
                        "润色模式: {} (provider={}, model={})",
                        mode_str,
                        llm_cfg.provider,
                        llm_cfg.model
                    );
                }
            }
            None => {
                log::warn!(
                    "polish_mode={:?} 但未找到有效的 LLM 配置（请检查 polish_llm: \"{}\" 或数据库中的 API Key 字段）",
                    config.polish_mode,
                    config.polish_llm
                );
            }
        }
    }

    // 加载自定义润色 system prompt（~/.octopus/VOICE_POLISH.md）
    // 文件存在且非空时覆盖内置默认 prompt
    let prompt_path = octopus_infra::octopus_config_home().join(octopus_infra::consts::VOICE_POLISH_FILE);
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
                // tract + df::tract（libDF/DF3 模型加载时的 codegen/declutter/shape 推断 DEBUG 极多，
                // df::tract 的 Init encoder / Start init ERB decoder / ERB decoder input 等 Info/Debug
                // 同样刷屏）一律压到 Warn。全局 level(Debug)，未列出的 target 默认走 Debug。
                .level_for("tract_core", log::LevelFilter::Warn)
                .level_for("tract_hir", log::LevelFilter::Warn)
                .level_for("tract_onnx", log::LevelFilter::Warn)
                .level_for("tract_linalg", log::LevelFilter::Warn)
                .level_for("df::tract", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_asr_engine,
            runtime_config::set_polish_mode,
            runtime_config::list_llm_models,
            runtime_config::switch_polish_llm,
            runtime_config::set_denoise_mode,
            coordinator::cancel_recording,
            coordinator::polish_now,
            result_window::result_window_ready,
            settings_window::open_settings,
            settings_commands::get_config,
            settings_commands::set_config,
            settings_commands::get_history,
        ])
        .setup(move |app| {
            // Initialize engine manager and preheat the active ASR model if embedded
            let engine_manager = Arc::new(octopus_asr::engine::AsrEngineManager::new());
            if config.engine_mode == "embedded" {
                let resolved_model = match octopus_asr::config::resolve_active_engine(&config.asr_engine) {
                    Ok(resolved) => resolved.name,
                    Err(_) => "zipformer-small-ctc".to_string(),
                };
                info!("Preheating active ASR model in desktop: {}", resolved_model);
                let em = engine_manager.clone();
                let active_model = resolved_model;
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

            // 2. Create AudioRecorder and open the device (graceful fallback if mic is missing)
            let audio_state = match audio::AudioRecorder::new(&config.microphone) {
                Ok(mut recorder) => {
                    if let Err(e) = recorder.open() {
                        log::error!("Failed to open audio device '{}': {}. Audio input will be silent.", config.microphone, e);
                    }
                    recorder.shared()
                }
                Err(e) => {
                    log::error!("Failed to initialize AudioRecorder: {}. Audio input will be silent.", e);
                    std::sync::Arc::new(audio::SharedAudioState::new(&config.microphone))
                }
            };

            // 工具栏运行时配置（asr_engine + polish_mode 的可变镜像），命令与 Coordinator 共享
            let runtime_config: runtime_config::SharedRuntimeConfig =
                std::sync::Arc::new(std::sync::RwLock::new(
                    runtime_config::RuntimeConfig::from_config(&config),
                ));
            app.manage(runtime_config.clone());

            // 3. Create Coordinator
            let coordinator = Coordinator::new(
                engine,
                audio_state,
                config.clone(),
                app.handle().clone(),
                runtime_config.clone(),
            );
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
        .run(|_app, event| {
            // 应用退出前：排空后台 DB 写入队列，避免 Finalize 等命令入队未落库而丢失
            // （录音结束→Finalize 入队→立即退出，是 DB actor 最典型的丢数据路径）。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                coordinator::shutdown_db();
            }
        });
}

fn main() {
    run();
}
