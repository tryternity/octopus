//! Tauri 命令注册表（2026-07-29 从 main.rs::run() 的 invoke_handler! 宏提取）。
//!
//! Tauri 2 不支持合并多个 `generate_handler!` 的输出（Issue #15597），
//! 也不支持多次 `invoke_handler` 调用（后一次覆盖前一次）。命令列表必须在
//! 宏展开期一次性固定。这里用 `macro_rules!` 把整个 `generate_handler![...]` 块
//! 包成一个 `handler!()` 宏，main.rs 只需 `invoke_handler(handler!())`，
//! 348 行的命令清单从 run() 中隔离出来。
//!
//! ## 路径前缀
//! 命令路径**统一用 `crate::` 前缀**（不再裸 `module::cmd`）——宏定义在独立文件，
//! 裸路径会因 `mod` 上下文不同而找不到模块；`crate::` 绝对路径不受此限制。
//!
//! ## cfg 属性
//! 命令带 `#[cfg(feature = "vault")]` / `#[cfg(target_os = "macos")]` 守卫，
//! 在宏体内由编译器正常求值（macro_rules! 透传 token，cfg 在展开后求值）。

/// 生成 Tauri 命令处理器闭包——run() 里 `invoke_handler(handler!())` 调用。
#[macro_export]
macro_rules! handler {
    () => {
        tauri::generate_handler![
            // ── runtime_config（工具栏状态 / 引擎切换 / 模式开关）──
            crate::runtime_config::toolbar_state,
            crate::runtime_config::list_asr_engines,
            crate::runtime_config::switch_active_model,
            crate::runtime_config::set_polish_mode,
            crate::runtime_config::list_llm_models,
            crate::runtime_config::set_denoise_mode,
            crate::runtime_config::set_translate_mode,
            // ── coordinator（录音控制 / 编辑 / 翻译）──
            crate::coordinator::cancel_recording,
            crate::coordinator::discard_recording,
            crate::coordinator::polish_now,
            crate::coordinator::enter_edit_mode,
            crate::coordinator::commit_edit,
            crate::coordinator::set_caret,
            crate::coordinator::set_selection,
            crate::coordinator::set_translation_active,
            crate::coordinator::start_recording,
            // ── result_window ──
            crate::result_window::result_window_ready,
            crate::result_window::set_result_click_through,
            // ── settings_window / onboarding_window ──
            crate::settings_window::open_settings,
            crate::settings_window::get_initial_page,
            crate::onboarding_window::complete_onboarding,
            // ── settings_commands（配置 / 历史 / prompt / env）──
            crate::settings_commands::get_config,
            crate::settings_commands::set_config,
            crate::settings_commands::get_history,
            crate::settings_commands::delete_history,
            crate::settings_commands::check_shortcut,
            crate::settings_commands::test_llm_connection,
            crate::settings_commands::test_asr_connection,
            crate::settings_commands::list_prompts,
            crate::settings_commands::get_active_prompt,
            crate::settings_commands::set_active_prompt,
            crate::settings_commands::create_prompt,
            crate::settings_commands::update_prompt,
            crate::settings_commands::delete_prompt,
            crate::settings_commands::get_env_vars,
            crate::settings_commands::set_env_var,
            crate::settings_commands::delete_env_var_cmd,
            // ── model_commands / builtin_models / download_window ──
            crate::model_commands::list_downloadable_models,
            crate::model_commands::list_model_files,
            crate::model_commands::download_model,
            crate::builtin_models::check_builtin_models,
            crate::download_window::close_download_window,
            crate::model_commands::verify_model,
            crate::model_commands::delete_model,
            crate::model_commands::set_download_mirror,
            crate::model_commands::add_cloud_model,
            crate::model_commands::edit_cloud_model,
            crate::model_commands::remove_cloud_model,
            crate::model_commands::list_asr_cloud_presets,
            crate::model_commands::list_llm_provider_presets,
            crate::model_commands::list_translate_cloud_models,
            crate::model_commands::test_cloud_model,
            crate::model_commands::get_model_detail,
            // ── search_commands ──
            crate::search_commands::search_all,
            crate::search_commands::search_stream,
            crate::search_commands::record_search_hit,
            crate::search_commands::launch_app,
            crate::search_commands::open_file,
            crate::search_commands::open_url,
            crate::search_commands::reveal_path,
            crate::search_commands::reindex_apps,
            crate::search_commands::list_all_apps,
            // ── action_bar_commands（命令面板，第一部分）──
            crate::action_bar::action_bar_commands::list_prompt_files,
            crate::action_bar::action_bar_commands::open_file_in_editor,
            crate::action_bar::action_bar_commands::save_file,
            crate::action_bar::action_bar_commands::read_file_text,
            crate::action_bar::action_bar_commands::create_prompt_file,
            // ── hotword_commands ──
            crate::hotword_commands::list_hotword_sets,
            crate::hotword_commands::create_hotword_set,
            crate::hotword_commands::rename_hotword_set,
            crate::hotword_commands::delete_hotword_set,
            crate::hotword_commands::toggle_hotword_set,
            crate::hotword_commands::add_word_to_set,
            crate::hotword_commands::remove_word_from_set,
            crate::hotword_commands::list_hotword_hits,
            crate::hotword_commands::list_hotword_candidates,
            crate::hotword_commands::add_words_to_set,
            crate::hotword_commands::import_hotwords,
            crate::hotword_commands::export_hotwords,
            // ── clipboard_commands ──
            crate::clipboard_commands::query_clipboard_history,
            crate::clipboard_commands::toggle_clipboard_favorite,
            crate::clipboard_commands::delete_clipboard_item,
            crate::clipboard_commands::delete_clipboard_items,
            crate::clipboard_commands::clear_clipboard_history,
            crate::clipboard_commands::clear_clipboard_history_by_filter,
            crate::clipboard_commands::copy_clipboard_item,
            crate::clipboard_commands::clipboard_stats,
            crate::clipboard_commands::paste_clipboard_item,
            crate::clipboard_commands::save_image_item,
            crate::clipboard_commands::open_file_item,
            crate::clipboard_commands::ocr_image,
            crate::clipboard_commands::scan_qrcode_image,
            crate::clipboard_commands::insert_ocr_clipboard_item,
            crate::clipboard_commands::set_clipboard_item_text,
            crate::clipboard_commands::insert_clipboard_text_item,
            // ── clipboard_window ──
            crate::clipboard_window::clipboard_dock_expand,
            crate::clipboard_window::clipboard_dock_collapse,
            crate::clipboard_commands::get_image_thumb,
            crate::clipboard_commands::get_image_full,
            crate::clipboard_commands::save_image_dialog,
            crate::clipboard_commands::copy_image_to_clipboard,
            // ── screenshot_commands ──
            crate::screenshot_commands::start_screenshot,
            crate::screenshot_commands::confirm_screenshot,
            crate::screenshot_commands::cancel_screenshot,
            crate::screenshot_commands::get_screenshot_image,
            crate::screenshot_commands::get_screenshot_image_size,
            crate::screenshot_commands::show_screenshot_window,
            crate::screenshot_commands::confirm_screenshot_with_data,
            crate::screenshot_commands::save_screenshot_dialog,
            crate::screenshot_commands::ocr_screenshot,
            crate::screenshot_commands::scan_qrcode_screenshot,
            crate::screenshot_commands::get_last_screenshot_ocr,
            crate::screenshot_commands::start_scroll_recording,
            crate::screenshot_commands::stop_scroll_recording,
            crate::screenshot_commands::stop_scroll_recording_with_mode,
            crate::screenshot_commands::pin_screenshot,
            // ── compact_editor_commands ──
            crate::compact_editor_commands::open_compact_editor_tab,
            crate::compact_editor_commands::get_pending_compact_tabs,
            crate::compact_editor_commands::get_clipboard_item_text,
            crate::compact_editor_commands::get_clipboard_item_type,
            crate::compact_editor_commands::get_transcription_text,
            crate::compact_editor_commands::close_compact_editor,

            crate::coordinator::current_transcription_id,
            // ── theme ──
            crate::theme::list_themes,
            crate::theme::get_theme_id,
            // ── system_status_commands ──
            crate::system_status_commands::get_system_status,
            crate::system_status_commands::subscribe_system_status,
            crate::system_status_commands::unsubscribe_system_status,
            // ── action_bar_commands（命令面板，第二部分：命令项 CRUD + 执行）──
            crate::action_bar::action_bar_commands::trigger_action_bar,
            crate::action_bar::action_bar_commands::action_bar_show_result,
            crate::action_bar::action_bar_commands::translate_text,
            crate::action_bar::action_bar_commands::get_translate_result,
            crate::action_bar::action_bar_commands::forget_translate_result,
            crate::action_bar::action_bar_commands::action_bar_get_context,
            crate::action_bar::action_bar_commands::action_bar_dismiss,
            crate::action_bar::action_bar_commands::list_action_bar_items,
            crate::action_bar::action_bar_commands::create_action_bar_item,
            crate::action_bar::action_bar_commands::update_action_bar_item,
            crate::action_bar::action_bar_commands::set_global_shortcut,
            crate::action_bar::action_bar_commands::delete_action_bar_item,
            crate::action_bar::action_bar_commands::move_action_bar_item,
            crate::action_bar::action_bar_commands::execute_action_bar,
            crate::action_bar::action_bar_commands::list_script_runs,
            crate::action_bar::action_bar_commands::clear_script_runs,
            crate::action_bar::action_bar_commands::delete_script_runs,
            crate::action_bar::action_bar_commands::restore_prompt_from_seed,
            // ── action_bar_commands：agent 适配器 ──
            crate::action_bar::action_bar_commands::list_agent_adapters,
            crate::action_bar::action_bar_commands::create_agent_adapter,
            crate::action_bar::action_bar_commands::update_agent_adapter,
            crate::action_bar::action_bar_commands::delete_agent_adapter,
            crate::action_bar::action_bar_commands::set_default_agent,
            crate::action_bar::action_bar_commands::clear_default_agent,
            crate::action_bar::action_bar_commands::refresh_agent_detection,
            crate::action_bar::action_bar_commands::trigger_agent_voice,
            crate::action_bar::action_bar_commands::list_agent_tasks,
            crate::action_bar::action_bar_commands::delete_agent_task,
            crate::action_bar::action_bar_commands::retry_agent_task,
            // ── extensions ──
            crate::extensions::import_extension,
            crate::extensions::install_extension,
            crate::extensions::list_extensions,
            crate::extensions::delete_extension,
            crate::extensions::refresh_extensions,
            // ── vault（feature gate——feature off 时命令不注册）──
            // follow-up #10: 前端通过 is_vault_enabled() 检测后整段隐藏 vault UI。
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
            crate::feature_flags::is_vault_enabled,
            // ── translation_commands ──
            crate::translation_commands::discover_translation_models,
            crate::translation_commands::translate_status,
            // 临时性能打点（ASR Result 窗卡顿取证，根因定位后移除）
            crate::perf_log::perf_log_cmd,
            // ── 录屏（2026-07-25 screen record MVP，Task 10）──────────
            // 仅 macOS 编译；record_commands 模块整体 cfg(target_os = "macos")。
            // 5 个控制命令用 record_* 前缀避免与 coordinator::start_recording（ASR 录音）冲突。
            #[cfg(target_os = "macos")]
            crate::record_commands::list_record_displays,
            #[cfg(target_os = "macos")]
            crate::record_commands::list_record_windows,
            #[cfg(target_os = "macos")]
            crate::record_commands::list_microphones,
            #[cfg(target_os = "macos")]
            crate::record_commands::check_record_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::request_screen_record_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::open_privacy_settings,
            #[cfg(target_os = "macos")]
            crate::record_commands::check_microphone_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::request_microphone_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::check_accessibility_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::request_accessibility_permission,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_start,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_start_default,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_pause,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_resume,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_stop,
            #[cfg(target_os = "macos")]
            crate::record_commands::record_kill,
            // 录屏区域选区 picker（Cmd+Shift+R → area tab → 选择区域）
            #[cfg(target_os = "macos")]
            crate::record_area_picker::start_record_area_picker,
            #[cfg(target_os = "macos")]
            crate::record_area_picker::show_record_area_picker_window,
            #[cfg(target_os = "macos")]
            crate::record_area_picker::confirm_record_area_picker,
            #[cfg(target_os = "macos")]
            crate::record_area_picker::cancel_record_area_picker,
            // 标注 overlay（录屏开始后显示，A 键切标注/透传模式）
            #[cfg(target_os = "macos")]
            crate::record_annotation_window::set_annotation_passthrough,
            #[cfg(target_os = "macos")]
            crate::record_annotation_window::set_toolbar_zone,
            #[cfg(target_os = "macos")]
            crate::record_commands::list_recordings,
            #[cfg(target_os = "macos")]
            crate::record_commands::get_recording,
            #[cfg(target_os = "macos")]
            crate::record_commands::get_recording_thumbnail,
            #[cfg(target_os = "macos")]
            crate::record_commands::rename_recording,
            #[cfg(target_os = "macos")]
            crate::record_commands::toggle_recording_favorite,
            #[cfg(target_os = "macos")]
            crate::record_commands::delete_recording,
            #[cfg(target_os = "macos")]
            crate::record_commands::open_recording_file,
            #[cfg(target_os = "macos")]
            crate::record_commands::reveal_recording,
            #[cfg(target_os = "macos")]
            crate::record_commands::export_gif,
            #[cfg(target_os = "macos")]
            crate::record_commands::merge_audio_tracks,
            #[cfg(target_os = "macos")]
            crate::record_commands::generate_subtitle,
            #[cfg(target_os = "macos")]
            crate::record_commands::read_subtitle,
            #[cfg(target_os = "macos")]
            crate::record_commands::reveal_subtitle,
            #[cfg(target_os = "macos")]
            crate::record_commands::list_subtitle_llms,
            #[cfg(target_os = "macos")]
            crate::record_commands::check_ffmpeg,
            crate::record_commands::get_record_status,
        ]
    };
}
