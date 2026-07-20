#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activation;
mod action_bar_window;
mod action_bar_commands;
// vault（Task 16+）：AppState + Tauri 命令 + 自动填写
// follow-up #10: vault feature gate——关闭后所有 vault 模块整体 cfg 掉。
// 例外：vault_secret_access **总是**编译（云端推理热路径 chokepoint，feature off 时
// 退化为返回 raw 原值的 no-op）。
#[cfg(feature = "vault")]
pub mod vault_state;
#[cfg(feature = "vault")]
pub mod vault_commands;
pub mod vault_secret_access;
#[cfg(feature = "vault")]
pub mod vault_error;
#[cfg(feature = "vault")]
pub mod autotype;
#[cfg(feature = "vault")]
pub mod password_generator_window;
mod overlay_window;
mod action_hotkey;
mod agent_adapter;
mod terminal_launcher;
mod finder_selection;
mod app_context;
mod audio;
mod config;
mod clipboard_commands;
mod compact_editor_commands;
mod compact_editor_window;

mod image_migration;
mod i18n;
mod clipboard_window;
mod clipboard_dock;
mod coordinator;
mod db_queue;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
mod engine_dispatch;
mod engine_embedded;
mod extensions;
mod file_watcher;
#[cfg(feature = "remote-grpc")]
mod engine_grpc;
#[cfg(feature = "remote-ws")]
mod engine_ws;
mod model_commands;
mod model_migrate;
mod search_commands;
mod hotword_commands;
mod input_source;
mod paste;
mod pin_window;
mod perf_log;
mod pipeline;
mod result_window;
mod screenshot_commands;
mod screenshot_geometry;
mod runtime_config;
mod settings_commands;
mod settings_window;
mod system_status_commands;
mod focus_tracker;
mod shortcut;
mod theme;
mod tray;
mod transcript;
mod translation_commands;
mod window_position;

use coordinator::Coordinator;
use engine::TranscriptionEngine;
#[cfg(not(feature = "cloud"))]
use engine_embedded::EmbeddedEngine;
use log::info;
use std::sync::Arc;
use tauri::{Emitter, Listener, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // panic hook：catch_unwind 已处理降级，panic 仅记 warning（不刷屏）
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        log::warn!("Recovered panic at {}: {}", location, msg);
    }));

    let config = octopus_infra::config::load_config().unwrap_or_else(|e| {
        log::warn!("config load failed ({}), using defaults", e);
        octopus_infra::config::AppConfig::default()
    });

    // 初始化嵌入式 DB（建表 + seed 默认引擎）。asr 的 load_config 首次调用时也会
    // lazy init，这里显式预热（日志早出 + 错误前置）。模型配置唯一来源即此 DB。
    // 失败仅告警，不阻断启动（识别历史写入会失败，但应用可用）
    if let Err(e) = octopus_asr_local::db::ensure_db() {
        log::error!("DB init failed: {}, storage disabled", e);
    }

    // Task 2 模型激活语义重构：启动时加载 4 域激活引擎到 ACTIVE_ENGINES 内存缓存。
    // 后续所有使用方（推理 / tray / 管理页 / 流式判定）经 resolve_active_engine(domain)
    // 纯读此缓存。ASR 域带兜底（zipformer-small-ctc），其余域无激活仅告警不阻断。
    for domain in ["asr", "llm", "ocr", "translate"] {
        match octopus_asr_local::config::load_active_engine(domain) {
            Ok(r) => info!("Active {} engine: {} [{}]", domain, r.name, r.provider),
            Err(e) => log::warn!("Active {} engine 未激活：{}", domain, e),
        }
    }
    info!(
        "Config: mode={}, asr_shortcut={}",
        config.engine_mode, config.asr_shortcut
    );
    // 创建模型路径软链（HF cache → ~/.octopus/models/{domain}/{name}/）
    if let Err(e) = model_migrate::create_model_symlinks() {
        log::warn!("模型路径迁移失败（非致命）: {e:?}");
    }
    // 初始化搜索引擎（应用索引 + 书签扫描）
    octopus_search::init_search_engine();

    // 校验引擎模式
    if config.engine_mode == "embedded" && !config::is_streaming_engine() {
        let active_name = octopus_asr_local::config::resolve_active_engine("asr")
            .map(|r| r.name)
            .unwrap_or_else(|_| "<未激活>".to_string());
        log::info!("引擎 '{}' 使用 VAD 分段伪流式模式", active_name);
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
        match crate::config::llm_config(config.polish_mode) {
            Some(llm_cfg) => {
                let mode_str = match config.polish_mode {
                    PolishMode::FinalOnly => "仅最终润色",
                    PolishMode::Intermediate => "中间+最终",
                    // Disabled 理论上不会进 Some(llm_cfg) 分支（llm_config 返回 None），
                    // 但显式列出避免新增变体时 unreachable! panic 扼杀启动。
                    PolishMode::Disabled => "已禁用",
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
                let active_llm = octopus_asr_local::config::resolve_active_engine("llm")
                    .map(|r| r.name)
                    .unwrap_or_default();
                log::warn!(
                    "polish_mode={:?} 但未找到有效的 LLM 配置（当前激活 LLM=\"{}\"，请检查 DB 中的 API Key 字段）",
                    config.polish_mode,
                    active_llm
                );
            }
        }
    }

    // 从 DB 加载激活的润色 prompt（prompts 表 active_polish_prompt 指向的记录）
    // 失败时 fallback 到 id=1（系统默认）
    let active_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    let prompt_content = match octopus_infra::db::load_prompt(active_id) {
        Ok(Some(p)) => p.content,
        Ok(None) => {
            log::warn!("active_polish_prompt id={} 不存在，fallback 到 id=1", active_id);
            let _ = octopus_infra::db::save_active_prompt_id(1);
            octopus_infra::db::load_prompt(1)
                .ok()
                .flatten()
                .map(|p| p.content)
                .unwrap_or_default()
        }
        Err(e) => {
            log::warn!("DB 加载 prompt 失败（id={}）：{} —— 使用空 content 降级", active_id, e);
            String::new()
        }
    };
    octopus_llm::set_system_prompt(&prompt_content);
    log::info!("已加载润色 prompt（active id={}）", active_id);

    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info!("Single instance: re-activated");
            if let Some(coordinator) = app.try_state::<Coordinator>() {
                coordinator.toggle();
            }
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
                .level_for("octopus_desktop::window_position", log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_active_model,
            runtime_config::set_polish_mode,
            runtime_config::list_llm_models,
            runtime_config::set_denoise_mode,
            runtime_config::set_translate_mode,
            coordinator::cancel_recording,
            coordinator::discard_recording,
            coordinator::polish_now,
            coordinator::enter_edit_mode,
            coordinator::commit_edit,
            coordinator::set_caret,
            coordinator::set_selection,
            coordinator::set_translation_active,
            coordinator::start_recording,
            result_window::result_window_ready,
            result_window::set_result_click_through,
            settings_window::open_settings,
            settings_window::get_initial_page,
            settings_commands::get_config,
            settings_commands::set_config,
            settings_commands::get_history,
            settings_commands::delete_history,
            settings_commands::check_shortcut,
            settings_commands::test_llm_connection,
            settings_commands::test_asr_connection,
            settings_commands::list_prompts,
            settings_commands::get_active_prompt,
            settings_commands::set_active_prompt,
            settings_commands::create_prompt,
            settings_commands::update_prompt,
            settings_commands::delete_prompt,
            settings_commands::get_env_vars,
            settings_commands::set_env_var,
            settings_commands::delete_env_var_cmd,
            model_commands::list_downloadable_models,
            model_commands::download_model,
            model_commands::verify_model,
            model_commands::delete_model,
            model_commands::set_download_mirror,
            model_commands::add_cloud_model,
            model_commands::edit_cloud_model,
            model_commands::remove_cloud_model,
            model_commands::list_asr_cloud_presets,
            model_commands::list_llm_provider_presets,
            model_commands::list_translate_cloud_models,
            model_commands::test_cloud_model,
            model_commands::get_model_detail,
            search_commands::search_all,
            search_commands::search_stream,
            search_commands::record_search_hit,
            search_commands::launch_app,
            search_commands::open_file,
            search_commands::open_url,
            search_commands::reveal_path,
            search_commands::reindex_apps,
            hotword_commands::list_hotword_sets,
            hotword_commands::create_hotword_set,
            hotword_commands::rename_hotword_set,
            hotword_commands::delete_hotword_set,
            hotword_commands::toggle_hotword_set,
            hotword_commands::add_word_to_set,
            hotword_commands::remove_word_from_set,
            hotword_commands::list_hotword_hits,
            hotword_commands::list_hotword_candidates,
            hotword_commands::add_words_to_set,
            hotword_commands::import_hotwords,
            hotword_commands::export_hotwords,
            clipboard_commands::query_clipboard_history,
            clipboard_commands::toggle_clipboard_favorite,
            clipboard_commands::delete_clipboard_item,
            clipboard_commands::delete_clipboard_items,
            clipboard_commands::clear_clipboard_history,
            clipboard_commands::clear_clipboard_history_by_filter,
            clipboard_commands::copy_clipboard_item,
            clipboard_commands::clipboard_stats,
            clipboard_commands::paste_clipboard_item,
            clipboard_commands::save_image_item,
            clipboard_commands::open_file_item,
            clipboard_commands::ocr_image,
            clipboard_commands::insert_ocr_clipboard_item,
            clipboard_commands::set_clipboard_item_text,
            clipboard_commands::insert_clipboard_text_item,
            clipboard_window::clipboard_dock_expand,
            clipboard_window::clipboard_dock_collapse,
            clipboard_commands::get_image_thumb,
            clipboard_commands::get_image_full,
            clipboard_commands::save_image_dialog,
            clipboard_commands::copy_image_to_clipboard,
            screenshot_commands::start_screenshot,
            screenshot_commands::confirm_screenshot,
            screenshot_commands::cancel_screenshot,
            screenshot_commands::get_screenshot_image,
            screenshot_commands::show_screenshot_window,
            screenshot_commands::confirm_screenshot_with_data,
            screenshot_commands::save_screenshot_dialog,
            screenshot_commands::ocr_screenshot,
            screenshot_commands::get_last_screenshot_ocr,
            screenshot_commands::start_scroll_recording,
            screenshot_commands::stop_scroll_recording,
            screenshot_commands::stop_scroll_recording_with_mode,
            screenshot_commands::pin_screenshot,
            compact_editor_commands::open_compact_editor_tab,
            compact_editor_commands::get_pending_compact_tabs,
            compact_editor_commands::get_clipboard_item_text,
            compact_editor_commands::get_clipboard_item_type,
            compact_editor_commands::get_transcription_text,
            compact_editor_commands::close_compact_editor,

            coordinator::current_transcription_id,
            theme::list_themes,
            theme::get_theme_id,
            system_status_commands::get_system_status,
            system_status_commands::subscribe_system_status,
            system_status_commands::unsubscribe_system_status,
            action_bar_commands::trigger_action_bar,
            action_bar_commands::action_bar_show_result,
            action_bar_commands::translate_text,
            action_bar_commands::get_translate_result,
            action_bar_commands::forget_translate_result,
            action_bar_commands::action_bar_get_context,
            action_bar_commands::action_bar_dismiss,
            action_bar_commands::list_action_bar_items,
            action_bar_commands::create_action_bar_item,
            action_bar_commands::update_action_bar_item,
            action_bar_commands::set_global_shortcut,
            action_bar_commands::delete_action_bar_item,
            action_bar_commands::move_action_bar_item,
            action_bar_commands::execute_action_bar,
            action_bar_commands::list_script_runs,
            action_bar_commands::clear_script_runs,
            action_bar_commands::delete_script_runs,
            action_bar_commands::list_agent_adapters,
            action_bar_commands::create_agent_adapter,
            action_bar_commands::update_agent_adapter,
            action_bar_commands::delete_agent_adapter,
            action_bar_commands::refresh_agent_detection,
            action_bar_commands::trigger_agent_voice,
            action_bar_commands::list_agent_tasks,
            action_bar_commands::delete_agent_task,
            action_bar_commands::retry_agent_task,
            extensions::import_extension,
            extensions::install_extension,
            extensions::list_extensions,
            extensions::delete_extension,
            extensions::refresh_extensions,
            // follow-up #10: vault feature gate——feature off 时这些命令不注册，
            // 前端通过 is_vault_enabled() 检测后整段隐藏 vault UI。
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_status,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_setup,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_unlock,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_lock,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_heartbeat,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_change_password,
            // 自动锁定超时配置（UI 在 VaultPanel 内联）
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_get_lock_timeout,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_set_lock_timeout,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_list_ciphers,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_get_cipher,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_create_cipher,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_update_cipher,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_delete_cipher,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_restore_cipher,
            // follow-up #6: folder CRUD
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_list_folders,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_create_folder,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_rename_folder,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_delete_folder,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_generate,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_evaluate_password,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_generate_totp,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_health_report,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_import_bitwarden,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_export,
            // Task 19: Auto-Type 命令
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_autotype,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_detect_and_match,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_copy_password,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_copy_username,
            // 密码生成器独立浮窗（Actionbar 触发，外壳 B；详见 spec §5.2）
            #[cfg(feature = "vault")]
            crate::vault_commands::open_password_generator,
            #[cfg(feature = "vault")]
            crate::vault_commands::password_generator_autotype,
            // follow-up #10: feature probe（永远注册——前端据此刻画 vault UI）。
            feature_flags::is_vault_enabled,
            translation_commands::discover_translation_models,
            translation_commands::translate_status,
            // 临时性能打点（ASR Result 窗卡顿取证，根因定位后移除）
            perf_log::perf_log_cmd,
        ])
        .setup(move |app| {
            // Initialize clipboard handle (clipboard-rs, replaces tauri-plugin-clipboard-manager)
            let clipboard_handle = Arc::new(
                octopus_clipboard::ClipboardHandle::new()
                    .map_err(|e| format!("Failed to init clipboard handle: {e}"))?,
            );
            app.manage(clipboard_handle.clone());

            // 启动时把 DB 的 clipboard_enabled 同步到运行时 flag。
            // ClipboardHandle::new() 默认 recording_enabled = true，而运行时改开关走的是
            // set_config 热重载——若不在此补一次性同步，用户关掉「剪贴板监听」并重启后，
            // watcher 又恢复录制（flag 回 true），但 DB 仍是 false，设置形同虚设。
            clipboard_handle.set_recording_enabled(config.clipboard_enabled);

            // 确保 extensions 目录存在
            let ext_dir = extensions::extensions_dir();
            if !ext_dir.exists() {
                let _ = std::fs::create_dir_all(&ext_dir);
            }

            // 启动时重建 FTS5 索引，清理上次运行遗留的空洞
            if let Err(e) = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::rebuild_fts_index(conn)
            }) {
                log::warn!("Startup FTS5 rebuild failed: {}", e);
            }

            // 启动时应用方言模糊规则（须先于热词装载：规则影响索引 key 归一化，
            // 先 set rules 再 reload_hotwords 建索引，最终索引用新规则）。
            octopus_asr_local::corrector::reload_fuzzy_dialect(&config.fuzzy_dialect);

            // 启动时装载 active 热词到 corrector（force init + reload 索引）。
            // 之后所有引擎纠错自动用上热词（候选有界，空热词即 no-op 零过纠）。
            match octopus_asr_local::db::list_active_hotword_words() {
                Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
                Err(e) => log::warn!("[hotword] 启动装载失败，纠错以空热词运行: {}", e),
            }

            // 迁移旧文件系统图片到 DB BLOB
            image_migration::migrate_images_to_db();

            // 启动时按配置执行自动清理（删除超期/超量非收藏记录 + 回收孤立 BLOB）。
            // clipboard_max_items / clipboard_max_age_days 此前是无处调用的摆设；
            // 此处接入让设置页"最大保留条数 / 自动清理天数"真正生效。
            // image_migration 已先迁入旧图片；run_cleanup 在有删除时内部重建 FTS。
            {
                let max_age = config.clipboard_max_age_days as u32;
                let max_items = config.clipboard_max_items as u32;
                if let Err(e) = octopus_infra::db::with_db(|conn| {
                    octopus_clipboard::cleanup::run_cleanup(conn, max_age, max_items)
                }) {
                    log::warn!("Startup clipboard cleanup failed: {}", e);
                }
            }

            // 后台定时清理（每小时）：从 DB 重读最新 config（用户可能在运行时改了限额）。
            // cleanup 在无删除时只做几次 COUNT，很轻；有删除才重建 FTS。
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                    let cfg = octopus_infra::config::load_config().unwrap_or_default();
                    let max_age = cfg.clipboard_max_age_days as u32;
                    let max_items = cfg.clipboard_max_items as u32;
                    if let Err(e) = octopus_infra::db::with_db(|conn| {
                        octopus_clipboard::cleanup::run_cleanup(conn, max_age, max_items)
                    }) {
                        log::warn!("Scheduled clipboard cleanup failed: {}", e);
                    }
                }
            });

            // 启动 notify-rs 文件监听：app 目录变化时秒级刷新索引。
            // macOS FSEvents 对 /System 等非用户目录可能漏事件——下面的轮询作为 fallback。
            file_watcher::start_app_watcher();

            // 应用索引后台自动刷新（mtime 轮询）：用户装卸应用后无需重启即可搜到。
            // 启动后延迟 30s（避开 ASR 预热等重活），之后每 10 分钟检测 /Applications 等
            // 目录 mtime，变化时才触发全量重扫（扫盘耗时数秒，仅在真实变化时发生）。
            // 内存索引通过 SearchEngine.app_index 的 RwLock 热替换，搜索走读锁零阻塞。
            std::thread::spawn(move || {
                // 启动后 30s 首次校准（检查 DB 缓存是否过期——新装/卸载 app），之后每 2 分钟。
                // 原方案靠目录 mtime 检测，但直接拷 .app 进 /Applications 不一定改目录 mtime，
                // 导致新装 app 搜不到。改用"文件系统 .app 数量 vs 索引数量"对比——数量变了就 rescan。
                std::thread::sleep(std::time::Duration::from_secs(30));
                let watch_dirs = ["/Applications", "/System/Applications", "/Applications/Utilities"];
                let home_apps = dirs::home_dir().map(|h| h.join("Applications"));
                // 快速计数：递归列出各目录下的 .app 数量（不提取 icon，毫秒级）
                let count_apps = || -> usize {
                    let mut total = 0;
                    for dir in &watch_dirs {
                        total += count_apps_in_dir(std::path::Path::new(dir), 0);
                    }
                    if let Some(ref home) = home_apps {
                        total += count_apps_in_dir(home, 0);
                    }
                    total
                };
                let mut last_count = count_apps();
                // 启动首次校准：DB 缓存的 app 数量 vs 文件系统实际数量
                if let Some(e) = octopus_search::get_engine() {
                    let cached = e.cached_app_count();
                    if cached != last_count {
                        log::info!("[search] 启动校准：DB 缓存 {} 个 app vs 文件系统 {} 个，重扫", cached, last_count);
                        let n = e.refresh_app_index();
                        log::info!("[search] 启动校准重扫完成: {} 个应用", n);
                    }
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(120));
                    let now_count = count_apps();
                    if now_count != last_count {
                        last_count = now_count;
                        log::info!("[search] 应用数量变化 ({}), 后台重扫", now_count);
                        if let Some(e) = octopus_search::get_engine() {
                            let n = e.refresh_app_index();
                            log::info!("[search] 后台重扫完成: {} 个应用", n);
                        }
                    }
                }
            });

            // 命令索引后台 LLM 关键字生成（独立 OS 线程，blocking HTTP 不阻塞 main）。
            // 扫描 PATH 产生的命令只有英文 description（whatis/brew desc），中文用户搜不到——
            // 这里逐个调 LLM 生成中英文关键字，写回 DB 缓存 + 内存索引。增量：每生成一条立即落盘，
            // 崩溃不丢全部进度。LLM 是 reqwest::blocking，但本线程本身就是独立 OS 线程，直接同步调即可
            // （无 async runtime，不能 spawn_blocking）。
            //
            // config 每轮从 DB 重读（与 cleanup 线程同模式）——用户可能运行时改 polish_llm 配置。
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(60)); // 启动 60s 后开始（避开 ASR 预热等重活）
                loop {
                    let engine = match octopus_search::get_engine() {
                        Some(e) => e,
                        None => {
                            std::thread::sleep(std::time::Duration::from_secs(300));
                            continue;
                        }
                    };
                    let pending = engine.commands_needing_keywords();
                    if pending.is_empty() {
                        std::thread::sleep(std::time::Duration::from_secs(600)); // 无待生成，10 分钟后再查
                        continue;
                    }
                    // 每轮重读 config：polish_llm 可能在运行时被改过。
                    let _config = octopus_infra::config::load_config().unwrap_or_default();
                    let llm_config = match crate::config::llm_config_ignore_mode() {
                        Some(c) => c,
                        None => {
                            std::thread::sleep(std::time::Duration::from_secs(600)); // LLM 未配置，10 分钟后重试
                            continue;
                        }
                    };
                    let system = "你是命令行工具专家。为给定命令生成简短的中英文搜索关键字，用空格分隔。只输出关键字，不要解释。包含：命令功能、同义词、中文翻译。限 30 字以内。";
                    let mut generated = 0;
                    for (name, path, desc) in pending.iter().take(20) { // 每轮最多 20 个
                        let user = format!("命令: {}\n英文描述: {}", name, desc);
                        match octopus_llm::chat_text_with_prompt(system, &user, &llm_config, None) {
                            Ok(keywords) => {
                                let keywords = keywords.trim();
                                if !keywords.is_empty() {
                                    engine.update_command_keywords(path, keywords);
                                    generated += 1;
                                }
                            }
                            Err(e) => log::warn!("[search] 命令 LLM 关键字生成失败 ({}): {}", name, e),
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500)); // 防限流
                    }
                    log::info!("[search] 命令 LLM 关键字: 本轮生成 {} 个", generated);
                    std::thread::sleep(std::time::Duration::from_secs(30)); // 轮间隔
                }
            });

            // Start focus tracker (macOS no-op, Windows/Linux TODO)
            let focus_tracker = std::sync::Arc::new(focus_tracker::FocusTracker::new());
            if let Err(e) = focus_tracker.start() {
                log::warn!("Focus tracker not available: {}", e);
            }
            app.manage(focus_tracker);

            // Start clipboard watcher (background thread, clipboard-rs)
            {
                let app_handle_for_watcher = app.handle().clone();
                let watcher_handle = clipboard_handle.clone();
                match octopus_clipboard::ClipboardWatcher::start(watcher_handle, move || {
                    octopus_clipboard::watcher::handle_clipboard_change(
                        app_handle_for_watcher
                            .state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>()
                            .inner(),
                    );
                    let _ = app_handle_for_watcher.emit("clipboard://changed", ());
                }) {
                    Ok(watcher) => { app.manage(watcher); }
                    Err(e) => log::error!("Failed to start clipboard watcher: {}", e),
                }
            }

            // Register clipboard window global shortcut (from config)
            if !config.clipboard_shortcut.is_empty() {
                if let Err(e) = clipboard_window::register_clipboard_shortcut(app.handle(), &config.clipboard_shortcut) {
                    log::error!("Failed to register clipboard shortcut: {}", e);
                }
            }

            // Register screenshot global shortcut (from config)
            if !config.screenshot_shortcut.is_empty() {
                if let Err(e) = screenshot_commands::register_screenshot_shortcut(app.handle(), &config.screenshot_shortcut) {
                    log::error!("Failed to register screenshot shortcut: {}", e);
                }
            }

            // Create + register action bar window (AI command palette)
            action_bar_window::create_action_bar_window(app.handle());
            overlay_window::create_overlay_window(app.handle());
            action_hotkey::register_action_hotkeys(app.handle());
            if !config.action_bar_shortcut.is_empty() {
                if let Err(e) = action_bar_window::register_action_bar_shortcut(app.handle(), &config.action_bar_shortcut) {
                    log::error!("Failed to register action bar shortcut: {}", e);
                }
            }

            // vault Auto-Type 热键（默认 Cmd+Shift+L）—— Task 19
            // follow-up #10: vault feature gate——feature off 时整段跳过（命令模块不存在）。
            #[cfg(feature = "vault")]
            {
                if !config.vault_autotype_shortcut.is_empty() {
                    if let Err(e) = crate::vault_commands::register_vault_autotype_shortcut(
                        app.handle(),
                        &config.vault_autotype_shortcut,
                    ) {
                        log::warn!("注册 vault autotype 热键失败: {}", e);
                    }
                }

                // 密码生成器不再注册全局热键——已改为 CipherEditor 内嵌按钮。
                // AppConfig.vault_generator_shortcut 字段保留仅为兼容旧 DB，不再消费。
            }

            // Initialize engine manager
            let engine_manager = Arc::new(octopus_asr_local::engine::AsrEngineManager::new());

            // 一次性解析激活 ASR 引擎 → ResolvedEngine，用于 preheat 判定。
            let resolved_engine = octopus_asr_local::config::resolve_active_engine("asr");

            // 云引擎判定（仅用于 preheat 守卫）：启动时激活引擎为 Aliyun → 跳过本地预热。
            // 运行时引擎路由由 DispatchEngine 按 spec 动态分发，不依赖此判定。
            #[cfg(feature = "cloud")]
            let is_cloud_aliyun = resolved_engine.as_ref()
                .map(|r| r.as_engine_category() == Some(octopus_asr_local::config::EngineCategory::Aliyun))
                .unwrap_or(false);

            // Preheat 仅本地 embedded 离线引擎：
            // - 云引擎 AliyunEngine 无需本地预热（跳过避免 switch_model 对 aliyun bail）；
            // - 流式引擎（is_streaming）走 StreamingSessionManager，录制时不经过离线 AsrEngineManager，
            //   若预热离线版会把同一模型的离线 ONNX Session 常驻在 AsrEngineManager 里却从不使用，
            //   与流式 Session 并存 → 双重加载浪费内存（~100-300MB）。流式引擎在首次录音时由
            //   prepare_streaming_session 懒加载进 StreamingSessionManager，无需启动预热。
            let do_preheat = config.engine_mode == "embedded"
                && !config::is_streaming_engine();
            #[cfg(feature = "cloud")]
            let do_preheat = do_preheat && !is_cloud_aliyun;

            // 系统状态页：创建 registry + sampler，manage 为 State，启动采样循环 + 注入模型 probe。
            // 必须在 preheat spawn 之前——set_probe 同步完成后，预加载模型的加载才会被探针捕获，
            // 否则启动预热的 ASR/VAD 可能抢在注入前加载而漏进 registry。
            {
                let registry = Arc::new(system_status_commands::ModelMemoryRegistry::new());
                let sampler = Arc::new(system_status_commands::SystemStatusSampler::new(registry));
                app.manage(sampler.clone());
                sampler.start(app.handle().clone());
            }

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
                    // 预加载 VAD session 到全局缓存：首次 Toggle 命中缓存，消除录音启动延迟。
                    // 失败不影响启动（首次录音时 new() 会懒加载重试）。
                    if let Ok(vad_path) = octopus_asr_local::config::find_silero_vad() {
                        match octopus_asr_local::vad::SileroVad::new(&vad_path) {
                            Ok(_) => info!("VAD session preheated"),
                            Err(e) => log::warn!(
                                "VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e
                            ),
                        }
                    }
                });
            }

            // Create engine —— aliyun feature 下用 DispatchEngine（持有本地 + 云端两个实例，
            // 每次 transcribe 按 spec 动态路由），解决运行时切换云/本地引擎不匹配的问题。
            // 非 aliyun feature 仅本地引擎（embedded/websocket/grpc）。
            let engine: Arc<dyn TranscriptionEngine> = {
                #[cfg(feature = "cloud")]
                {
                    Arc::new(engine_dispatch::DispatchEngine::new(engine_manager.clone()))
                }
                #[cfg(not(feature = "cloud"))]
                {
                    build_local_engine(&config, &engine_manager)
                }
            };

            // 暴露 engine_manager 为 State（审查 三2）：switch_asr_engine / set_config 切引擎时
            // 后台 switch_model 预热需要它。DispatchEngine 持有的是 clone，此处再 clone 托管。
            app.manage(engine_manager.clone());

            // 流式引擎复用 manager（②）：desktop 录音 reset() 复用常驻 StreamingSession，
            // 避免每次录音重载 ONNX Session。对齐离线 engine_manager 的注入方式。
            let streaming_manager = Arc::new(
                octopus_asr_local::streaming_engine::StreamingSessionManager::new(),
            );
            app.manage(streaming_manager);

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
                std::sync::Arc::new(parking_lot::RwLock::new(config.clone()));
            app.manage(runtime_config.clone());

            // vault AppState：进程内持有解锁态的 user_vault_key / app_key。
            // 先 bootstrap_app_key（用 K_machine 尝试解 app_key）再 manage——
            // 这样从 Tauri State 取到 session 时 app_key 已就位（若本机已初始化）。
            //
            // follow-up #10: vault feature gate——feature off 时整段跳过：
            //   - 不 manage SharedVaultSession（vault_state 模块未编入）
            //   - 不 set_global_session（try_global_session 返回 None →
            //     vault_secret_access::try_decrypt_secret_global 退化为 raw passthrough）
            #[cfg(feature = "vault")]
            {
                let vault_session: vault_state::SharedVaultSession = std::sync::Arc::new(
                    parking_lot::RwLock::new(vault_state::VaultSession::default()),
                );
                vault_state::bootstrap_app_key(&vault_session);
                app.manage(vault_session.clone());
                // VaultPicker URL 缓存：热键触发时（show 浮窗之前）抓 URL 存入，
                // vault_detect_and_match 优先读此缓存（修 e2e 发现的抢前台 bug）。
                let picker_url_cache: vault_state::SharedPickerUrlCache =
                    std::sync::Arc::new(std::sync::Mutex::new(None));
                app.manage(picker_url_cache);
                // follow-up #7：注入进程级全局 session 句柄，供 cloud 推理热路径
                // （AliyunEngine::transcribe / config::llm_config_ignore_mode / 云端翻译）
                // 解密 v1: 前缀的 secret_key。
                vault_state::set_global_session(vault_session);
            }

            // 3. Create Coordinator
            let coordinator = Coordinator::new(
                engine,
                audio_state,
                config.clone(),
                app.handle().clone(),
                runtime_config.clone(),
            );
            app.manage(coordinator);

            // 4. Initialize i18n + Create Tray
            i18n::init(&config.ui_language);
            if let Err(e) = tray::create_tray(app.handle(), &config) {
                log::error!("Tray init failed ({}), running without tray menu", e);
            }

            // 4.1 Listen for locale changes → rebuild tray menu labels
            {
                let app_handle = app.handle().clone();
                app.listen("locale-changed", move |_event| {
                    let cfg = octopus_infra::config::load_config().unwrap_or_default();
                    i18n::reload(&cfg.ui_language);
                    tray::rebuild_tray_labels();
                    let _ = app_handle; // keep handle alive
                });
            }

            // 5. Create Result Window
            result_window::create_result_window(app.handle());

            // 6. Register global shortcut
            if let Err(e) = shortcut::register_shortcut(app.handle(), &config.asr_shortcut) {
                log::error!("Failed to register shortcut: {}. Use tray menu instead.", e);
            }

            // 6.1 Register global edit shortcut（跨应用唤起结果窗 + toggle 编辑）
            if let Err(e) = result_window::register_edit_global_shortcut(app.handle(), &config.edit_global_shortcut) {
                log::error!("Failed to register global edit shortcut: {}", e);
            }

            // 6.2 Register global polish shortcut（跨应用 show 结果窗 + 立即润色）
            if let Err(e) = result_window::register_polish_global_shortcut(app.handle(), &config.polish_global_shortcut) {
                log::error!("Failed to register global polish shortcut: {}", e);
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
            // 统一查看器窗口关窗前保存状态（Destroyed 时窗口已销毁，get_webview_window 返回 None）
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { .. },
                label,
                ..
            } = &event
            {
                if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_save_state(app);
                }
            }
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
                } else if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_closed(app);

                }
            }
            // 应用退出前：排空后台 DB 写入队列，避免 Finalize 等命令入队未落库而丢失
            // （录音结束→Finalize 入队→立即退出，是 DB actor 最典型的丢数据路径）。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                db_queue::shutdown_db();
            }
        });
}

/// 按 `config.engine_mode` 构建本地 ASR 引擎（embedded / websocket / grpc）。
///
/// 仅在未启用 `dashscope` feature 时使用（dashscope 下由 DispatchEngine 统一路由）。
#[cfg(not(feature = "cloud"))]
fn build_local_engine(
    config: &octopus_infra::config::AppConfig,
    engine_manager: &Arc<octopus_asr_local::engine::AsrEngineManager>,
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

/// 递归计数目录下的 .app 数量（深度 ≤2，不进入 .app 包内部）。
/// 用于后台轮询快速检测新装/卸载的 app（不提取 icon，毫秒级）。
fn count_apps_in_dir(dir: &std::path::Path, depth: u32) -> usize {
    const MAX_DEPTH: u32 = 2;
    if depth > MAX_DEPTH {
        return 0;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            count += 1;
        } else if path.is_dir() {
            count += count_apps_in_dir(&path, depth + 1);
        }
    }
    count
}

fn main() {
    run();
}

/// follow-up #10: cargo feature 探针模块。
///
/// 放独立子模块是为了避开 tauri::command 宏与同模块 generate_handler! 之间的
/// 「macro-expanded macro_export 不能被绝对路径引用」限制（issue #52234）。
/// 命令本身永远注册，不被 vault feature gate——前端据此决定是否渲染 vault UI。
mod feature_flags {
    /// 返回编译期 `cfg!(feature = "vault")`。
    ///
    /// 前端 Settings/index.tsx / App.tsx 启动时 invoke 此命令，按返回值决定是否渲染
    /// VaultPanel nav / vault_picker_window 路由。
    #[tauri::command]
    pub fn is_vault_enabled() -> bool {
        cfg!(feature = "vault")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// follow-up #10: 验证 is_vault_enabled 与 cfg!(feature = "vault") 一致。
        ///
        /// 此测试在两条 feature 路径下都编译（is_vault_enabled 始终注册）。
        /// feature on → true；feature off → false。
        #[test]
        fn test_is_vault_enabled_reflects_cfg() {
            assert_eq!(is_vault_enabled(), cfg!(feature = "vault"));
        }
    }
}
