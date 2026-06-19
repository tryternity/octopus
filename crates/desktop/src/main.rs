#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod coordinator;
#[cfg(feature = "dashscope")]
mod dashscope_stream;
mod engine;
#[cfg(feature = "dashscope")]
mod engine_dashscope;
mod engine_dispatch;
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
#[cfg(not(feature = "dashscope"))]
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
        "Config: engine={}, mode={}, asr_shortcut={}",
        config.asr_engine, config.engine_mode, config.asr_shortcut
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
        if config.polish_mode == PolishMode::Intermediate && config.polish_min_interval <= 0.0 {
            log::warn!(
                "polish_mode=2 但 polish_min_interval={}<=0，将使用下限 {}s",
                config.polish_min_interval,
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
                        "润色模式: {} (min_interval={}s, provider={}, model={})",
                        mode_str,
                        config.polish_min_interval,
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

    let mut app = tauri::Builder::default()
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
            coordinator::enter_edit_mode,
            coordinator::update_edit_buffer,
            coordinator::commit_edit,
            result_window::result_window_ready,
            settings_window::open_settings,
            settings_commands::get_config,
            settings_commands::set_config,
            settings_commands::get_history,
            settings_commands::delete_history,
            settings_commands::check_shortcut,
            settings_commands::test_llm_connection,
        ])
        .setup(move |app| {
            // Initialize engine manager
            let engine_manager = Arc::new(octopus_asr::engine::AsrEngineManager::new());

            // 一次性解析 config.asr_engine → ResolvedEngine，用于 preheat 判定。
            let resolved_engine = octopus_asr::config::resolve_active_engine(&config.asr_engine);

            // 云引擎判定（仅用于 preheat 守卫）：启动时 asr_engine 解析为 Aliyun → 跳过本地预热。
            // 运行时引擎路由由 DispatchEngine 按 spec 动态分发，不依赖此判定。
            #[cfg(feature = "dashscope")]
            let is_cloud_aliyun = resolved_engine.as_ref()
                .map(|r| r.category == octopus_asr::config::EngineCategory::Aliyun)
                .unwrap_or(false);

            // Preheat 仅本地 embedded 引擎（云引擎 DashscopeEngine 无需预热；跳过避免 switch_model 对 aliyun bail）
            let do_preheat = config.engine_mode == "embedded";
            #[cfg(feature = "dashscope")]
            let do_preheat = do_preheat && !is_cloud_aliyun;

            if do_preheat {
                let resolved_model = match &resolved_engine {
                    Ok(r) => r.name.clone(),
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

            // Create engine —— dashscope feature 下用 DispatchEngine（持有本地 + 云端两个实例，
            // 每次 transcribe 按 spec 动态路由），解决运行时切换云/本地引擎不匹配的问题。
            // 非 dashscope feature 仅本地引擎（embedded/websocket/grpc）。
            let engine: Arc<dyn TranscriptionEngine> = {
                #[cfg(feature = "dashscope")]
                {
                    Arc::new(engine_dispatch::DispatchEngine::new(engine_manager.clone()))
                }
                #[cfg(not(feature = "dashscope"))]
                {
                    build_local_engine(&config, &engine_manager)
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

            // 运行时共享配置——唯一真相源（Arc<RwLock<AppConfig>>）
            let runtime_config: runtime_config::SharedRuntimeConfig =
                std::sync::Arc::new(std::sync::RwLock::new(config.clone()));
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
            if let Err(e) = shortcut::register_shortcut(app.handle(), &config.asr_shortcut) {
                log::error!("Failed to register shortcut: {}. Use tray menu instead.", e);
            }

            info!("octopus-desktop initialized");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // macOS: 设为 Accessory 模式，不在 Dock 显示图标（纯托盘应用）
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(move |app, event| {
            // 设置窗口关闭 → macOS 切回 Accessory（仅托盘，Dock 图标消失）
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Destroyed,
                label,
                ..
            } = &event
            {
                if label == "settings_window" {
                    settings_window::on_settings_closed(app);
                }
            }
            // 应用退出前：排空后台 DB 写入队列，避免 Finalize 等命令入队未落库而丢失
            // （录音结束→Finalize 入队→立即退出，是 DB actor 最典型的丢数据路径）。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                coordinator::shutdown_db();
            }
        });
}

/// 按 `config.engine_mode` 构建本地 ASR 引擎（embedded / websocket / grpc）。
///
/// 仅在未启用 `dashscope` feature 时使用（dashscope 下由 DispatchEngine 统一路由）。
#[cfg(not(feature = "dashscope"))]
fn build_local_engine(
    config: &octopus_infra::config::AppConfig,
    engine_manager: &Arc<octopus_asr::engine::AsrEngineManager>,
) -> Arc<dyn TranscriptionEngine> {
    match config.engine_mode.as_str() {
        "embedded" => Arc::new(EmbeddedEngine::new(engine_manager.clone())),
        #[cfg(feature = "remote-ws")]
        "websocket" => Arc::new(engine_ws::WsRemoteEngine::new(&config.remote_url)),
        #[cfg(feature = "remote-grpc")]
        "grpc" => Arc::new(engine_grpc::GrpcRemoteEngine::new(&config.grpc_endpoint)),
        other => {
            log::warn!("Unknown engine_mode '{}', falling back to embedded", other);
            Arc::new(EmbeddedEngine::new(engine_manager.clone()))
        }
    }
}

fn main() {
    run();
}
