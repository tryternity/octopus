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
#[cfg(feature = "vault")]
pub mod vault_secret_access;
#[cfg(feature = "vault")]
pub mod vault_error;
#[cfg(feature = "vault")]
pub mod vault_sync_commands;
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
mod bootstrap;
mod setup;
mod config;
mod clipboard_commands;
mod clipboard_queue;
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
mod error_util;
mod extensions;
mod file_watcher;
#[cfg(feature = "remote-grpc")]
mod engine_grpc;
#[cfg(feature = "remote-ws")]
mod engine_ws;
mod model_commands;
mod model_migrate;
mod builtin_models;
mod download_window;
mod search_commands;
mod hotword_commands;
mod input_source;
mod keystroke;
mod paste;
mod pin_window;
mod perf_log;
mod pipeline;
mod result_window;
mod screenshot_commands;
mod screenshot_geometry;
mod sys_open;
// 录屏（Task 10，2026-07-25 screen record MVP）：仅 macOS 编译。
// 模块内部 `#![cfg(target_os = "macos")]` 守护，windows/linux 编译时此 mod 整体为空，
// 对应 invoke_handler 注册项也用 cfg gate（见下方 generate_handler!）。
#[cfg(target_os = "macos")]
mod record_commands;
// 录屏全局快捷键（Task 14，2026-07-25）：Cmd+Shift+R toggle + Esc stop。
// 与 record_commands 同样仅 macOS 编译。
#[cfg(target_os = "macos")]
mod record_hotkey;
// 录屏配置浮窗（Cmd+Shift+R 弹出，选 display/window/area + 音频开关）。
// 仅 macOS（录屏 helper 只 mac 实现）。
#[cfg(target_os = "macos")]
mod record_window;
// 录屏区域选区 picker（多屏全屏透明覆盖，用户拖框选区域）。
// 仅 macOS。复用 screenshot 的窗口创建 + 坐标换算模式。
#[cfg(target_os = "macos")]
mod record_area_picker;
// 录屏标注 overlay 窗口（录屏开始后显示，普通 level 让 SCK 录到）。
// 仅 macOS。spike7/8 验证：SCK 录窗口 buffer，不录 always_on_top 浮层。
#[cfg(target_os = "macos")]
mod record_annotation_window;
#[cfg(target_os = "macos")]
mod record_control_window;
// 录屏音频元数据探测（Task 2.1，2026-07-27 录后合并 phase）：
// ffprobe 读 mp4 实际音轨 + 给后续 Task 2.2 写 metadata 用。仅 macOS
// （octopus-record + RawAudioTrack 只 mac 编译；与 record_commands.rs 同 gate）。
#[cfg(target_os = "macos")]
mod record_audio_probe;
mod runtime_config;
mod settings_commands;
mod settings_window;
mod onboarding_window;
mod system_status_commands;
mod focus_tracker;
mod shortcut;
mod theme;
mod tray;
mod subtitle_polish;
mod transcript;
mod translation_commands;
mod window_factory;
mod window_position;

use coordinator::Coordinator;
#[cfg(not(feature = "cloud"))]
use engine_embedded::EmbeddedEngine;
use log::info;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = bootstrap::bootstrap();

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
            onboarding_window::complete_onboarding,
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
            model_commands::list_model_files,
            model_commands::download_model,
            builtin_models::check_builtin_models,
            download_window::close_download_window,
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
            search_commands::list_all_apps,
            action_bar_commands::list_prompt_files,
            action_bar_commands::open_file_in_editor,
            action_bar_commands::save_file,
            action_bar_commands::read_file_text,
            action_bar_commands::create_prompt_file,
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
            clipboard_commands::scan_qrcode_image,
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
            screenshot_commands::get_screenshot_image_size,
            screenshot_commands::show_screenshot_window,
            screenshot_commands::confirm_screenshot_with_data,
            screenshot_commands::save_screenshot_dialog,
            screenshot_commands::ocr_screenshot,
            screenshot_commands::scan_qrcode_screenshot,
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
            action_bar_commands::restore_prompt_from_seed,
            action_bar_commands::list_agent_adapters,
            action_bar_commands::create_agent_adapter,
            action_bar_commands::update_agent_adapter,
            action_bar_commands::delete_agent_adapter,
            action_bar_commands::set_default_agent,
            action_bar_commands::clear_default_agent,
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
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_empty_trash,
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
            crate::vault_commands::vault_search_ciphers,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_get_cached_url,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_copy_password,
            #[cfg(feature = "vault")]
            crate::vault_commands::vault_copy_username,
            // 密码生成器独立浮窗（Actionbar 触发，外壳 B；详见 spec §5.2）
            #[cfg(feature = "vault")]
            crate::vault_commands::open_password_generator,
            #[cfg(feature = "vault")]
            crate::vault_commands::password_generator_autotype,
            // Vault Git 同步（2026-07-21 Phase 1）
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_status,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_test_connection,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_enable,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_now,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_disable,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_is_git_available,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_add_remote,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_remove_remote,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_list_remotes,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_clone,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_resolve_remote,
            #[cfg(feature = "vault")]
            crate::vault_sync_commands::vault_sync_resolve_local,
            // follow-up #10: feature probe（永远注册——前端据此刻画 vault UI）。
            feature_flags::is_vault_enabled,
            translation_commands::discover_translation_models,
            translation_commands::translate_status,
            // 临时性能打点（ASR Result 窗卡顿取证，根因定位后移除）
            perf_log::perf_log_cmd,
            // ── 录屏（2026-07-25 screen record MVP，Task 10）──────────
            // 仅 macOS 编译；record_commands 模块整体 cfg(target_os = "macos")。
            // 5 个控制命令用 record_* 前缀避免与 coordinator::start_recording（ASR 录音）冲突。
            #[cfg(target_os = "macos")]
            record_commands::list_record_displays,
            #[cfg(target_os = "macos")]
            record_commands::list_record_windows,
            #[cfg(target_os = "macos")]
            record_commands::list_microphones,
            #[cfg(target_os = "macos")]
            record_commands::check_record_permission,
            #[cfg(target_os = "macos")]
            record_commands::request_screen_record_permission,
            #[cfg(target_os = "macos")]
            record_commands::open_privacy_settings,
            #[cfg(target_os = "macos")]
            record_commands::check_microphone_permission,
            #[cfg(target_os = "macos")]
            record_commands::request_microphone_permission,
            #[cfg(target_os = "macos")]
            record_commands::check_accessibility_permission,
            #[cfg(target_os = "macos")]
            record_commands::request_accessibility_permission,
            #[cfg(target_os = "macos")]
            record_commands::record_start,
            #[cfg(target_os = "macos")]
            record_commands::record_start_default,
            #[cfg(target_os = "macos")]
            record_commands::record_pause,
            #[cfg(target_os = "macos")]
            record_commands::record_resume,
            #[cfg(target_os = "macos")]
            record_commands::record_stop,
            #[cfg(target_os = "macos")]
            record_commands::record_kill,
            // 录屏区域选区 picker（Cmd+Shift+R → area tab → 选择区域）
            #[cfg(target_os = "macos")]
            record_area_picker::start_record_area_picker,
            #[cfg(target_os = "macos")]
            record_area_picker::show_record_area_picker_window,
            #[cfg(target_os = "macos")]
            record_area_picker::confirm_record_area_picker,
            #[cfg(target_os = "macos")]
            record_area_picker::cancel_record_area_picker,
            // 标注 overlay（录屏开始后显示，A 键切标注/透传模式）
            #[cfg(target_os = "macos")]
            record_annotation_window::set_annotation_passthrough,
            #[cfg(target_os = "macos")]
            record_annotation_window::set_toolbar_zone,
            #[cfg(target_os = "macos")]
            record_commands::list_recordings,
            #[cfg(target_os = "macos")]
            record_commands::get_recording,
            #[cfg(target_os = "macos")]
            record_commands::get_recording_thumbnail,
            #[cfg(target_os = "macos")]
            record_commands::rename_recording,
            #[cfg(target_os = "macos")]
            record_commands::toggle_recording_favorite,
            #[cfg(target_os = "macos")]
            record_commands::delete_recording,
            #[cfg(target_os = "macos")]
            record_commands::open_recording_file,
            #[cfg(target_os = "macos")]
            record_commands::reveal_recording,
            #[cfg(target_os = "macos")]
            record_commands::export_gif,
            #[cfg(target_os = "macos")]
            record_commands::merge_audio_tracks,
            #[cfg(target_os = "macos")]
            record_commands::generate_subtitle,
            #[cfg(target_os = "macos")]
            record_commands::read_subtitle,
            #[cfg(target_os = "macos")]
            record_commands::reveal_subtitle,
            #[cfg(target_os = "macos")]
            record_commands::list_subtitle_llms,
            #[cfg(target_os = "macos")]
            record_commands::check_ffmpeg,
            record_commands::get_record_status,
        ])
        .setup(move |app| crate::setup::AppSetup::run(app, &config))
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
                } else if label == "onboarding_window" {
                    onboarding_window::on_onboarding_closed(app);
                }
            }
            // 应用退出前：排空后台 DB 写入队列，避免 Finalize 等命令入队未落库而丢失
            // （录音结束→Finalize 入队→立即退出，是 DB actor 最典型的丢数据路径）。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                db_queue::shutdown_db();
            }
        });
}

/// 启动时孤儿录屏文件清理。
///
/// 场景：上次录制 crash / 强制 kill 后，helper 写出的 .mp4 没入库就成了孤儿。
/// 启动时扫 `recordings/`，DB 不认得的文件直接删除（避免占用磁盘）。
///
/// 与 clipboard cleanup 同模式：通过 `octopus_infra::db::with_db` 拿连接，
/// 调 `RecordStore::list_all_file_paths` 取 DB 已知 file_path 集合，
/// 再扫目录比对。失败不阻塞启动（log::warn 继续）。
#[cfg(target_os = "macos")]
fn cleanup_orphan_recordings(conn: &rusqlite::Connection) {
    let store = octopus_record::RecordStore::new(conn);
    let known_files = match store.list_all_file_paths() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[record] 孤儿清理查询失败: {e}");
            return;
        }
    };

    let recordings_dir = octopus_infra::paths::recordings_dir();
    let entries = match std::fs::read_dir(&recordings_dir) {
        Ok(e) => e,
        Err(_) => return, // 目录不存在是正常的（首次启动或从未录制过）
    };

    // ⚠️ file_path 在 DB 里存的是绝对路径（2026-07-27 保存目录可配置后改），
    // list_all_file_paths 直接返回 DB 原值（绝对路径）。磁盘文件用 entry.path() 也是绝对路径，
    // 两者都是绝对路径，直接 to_string_lossy 比较即可。
    //
    // 曾有 bug（2026-07-28 e2e 发现）：旧代码 strip_prefix(octopus_root) 把磁盘文件转成相对路径
    // 再与 DB 的绝对路径比较 → 永远不匹配 → 所有录屏文件被当孤儿删掉（数据丢失）。
    for entry in entries.flatten() {
        let path = entry.path();
        let abs = path.to_string_lossy().to_string();
        if octopus_record::RecordStore::is_orphan(&abs, &known_files) {
            log::warn!("[record] 孤儿文件清理: {abs}");
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 按 `config.engine_mode` 构建本地 ASR 引擎（embedded / websocket / grpc）。
///
/// 仅在未启用 `dashscope` feature 时使用（dashscope 下由 DispatchEngine 统一路由）。
#[cfg(not(feature = "cloud"))]
fn build_local_engine(
    config: &octopus_infra::config::AppConfig,
    engine_manager: &std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
) -> std::sync::Arc<dyn crate::engine::TranscriptionEngine> {
    match config.engine_mode.as_str() {
        "embedded" => std::sync::Arc::new(EmbeddedEngine::new(engine_manager.clone())),
        #[cfg(feature = "remote-ws")]
        "websocket" => std::sync::Arc::new(engine_ws::WsRemoteEngine::new(&config.remote_url)),
        #[cfg(feature = "remote-grpc")]
        "grpc" => std::sync::Arc::new(engine_grpc::GrpcRemoteEngine::new(&config.grpc_endpoint)),
        other => {
            log::warn!("Unknown engine_mode '{}', falling back to embedded", other);
            std::sync::Arc::new(EmbeddedEngine::new(engine_manager.clone()))
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
