//! 设置窗口的 Tauri 命令：get_config / set_config / get_history。
//!
//! 与 runtime_config.rs 的区别：后者是工具栏专用命令（每个字段一个命令），
//! 本模块提供通用 get/set（方案 A），供设置窗口 GUI 表单使用。

use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State};

use crate::runtime_config::SharedRuntimeConfig;
use crate::config::PolishMode;

// ── get_config 返回 DTO ──

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    pub config: Value,
    pub asr_engines: Vec<crate::runtime_config::EngineOption>,
    pub llm_models: Vec<crate::runtime_config::LlmOption>,
    pub ocr_models: Vec<crate::runtime_config::OcrOption>,
    pub microphones: Vec<String>,
    pub prompts: Vec<PromptInfo>,
    pub active_prompt_id: i64,
}

#[tauri::command]
pub async fn get_config(_rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    // DB 查询 + cpal 麦克风枚举移入 spawn_blocking——避免阻塞 UI 线程。
    // Task 2 后：激活引擎从 ACTIVE_ENGINES 缓存取，不再读 AppConfig 4 个字段。
    let result = tokio::task::spawn_blocking(move || -> Result<ConfigResponse, String> {
        let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
        let config_json = serde_json::to_value(&cfg).map_err(|e| e.to_string())?;

        // 各数据源独立容错：一个表查询失败不拖垮其他数据源（fail-soft）。
        // app_config（load_config）失败是致命的 → 仍用 ? 传播。
        // models/prompts 查询失败 → 返回空数组 + log warn，让页面降级渲染而非白屏。
        let engines = octopus_asr_local::config::list_engines_from_db()
            .unwrap_or_else(|e| { log::warn!("get_config: ASR 引擎查询失败: {}", e); vec![] });
        let asr_engines = crate::runtime_config::build_asr_options_public(engines);

        let llms = octopus_infra::db::list_llm_models()
            .unwrap_or_else(|e| { log::warn!("get_config: LLM 查询失败: {}", e); vec![] });
        let llm_models = crate::runtime_config::build_llm_options_public(llms);

        let ocrs = octopus_infra::db::list_ocr_models()
            .unwrap_or_else(|e| { log::warn!("get_config: OCR 查询失败: {}", e); vec![] });
        let ocr_models = crate::runtime_config::build_ocr_options_public(ocrs);

        let microphones = list_microphones();

        let prompt_records = octopus_infra::db::list_prompts()
            .unwrap_or_else(|e| { log::warn!("get_config: prompts 查询失败: {}", e); vec![] });
        let prompts = prompt_records
            .into_iter()
            .map(|r| PromptInfo {
                id: r.id,
                title: r.title,
                content: r.content,
                description: r.description,
                is_system: r.is_system,
            })
            .collect();
        let active_prompt_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);

        Ok(ConfigResponse {
            config: config_json,
            asr_engines,
            llm_models,
            ocr_models,
            microphones,
            prompts,
            active_prompt_id,
        })
    })
    .await
    .map_err(|e| format!("get_config 任务异常: {}", e))?;

    result
}

/// 枚举系统麦克风设备（cpal 跨平台）。
fn list_microphones() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => {
            let mut mics: Vec<String> = devices.filter_map(|d| d.name().ok()).collect();
            mics.sort();
            mics
        }
        Err(_) => Vec::new(),
    }
}

// ── set_config 命令 ──

#[tauri::command]
pub fn set_config(
    key: String,
    value: Value,
    rc: State<'_, SharedRuntimeConfig>,
    coordinator: State<'_, crate::coordinator::Coordinator>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // record_* 配置项不在 AppConfig struct 里（走 app_config 表的泛型 key-value 存储），
    // 直接写 DB 后返回，不走 apply_config_value（会报「未知配置字段」）。
    // 包括：record_output_dir / record_reveal_after_stop / record_microphone_device 等。
    if key.starts_with("record_") && key != "record_shortcut" && key != "record_stop_shortcut" {
        let val_str = match &value {
            Value::Bool(b) => b.to_string(),
            Value::String(s) => s.clone(),
            _ => value.to_string(),
        };
        octopus_infra::db::with_db(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
                rusqlite::params![&key, &val_str],
            )?;
            Ok(())
        })
        .map_err(|e| format!("写入 DB 失败: {e}"))?;
        return Ok(());
    }
    let (old_asr_sc, old_clipboard_sc, old_edit_global, old_polish_global, old_screenshot_sc, old_action_bar_sc, old_vault_autotype_sc, old_record_sc, mut cfg) = {
        let g = rc.read();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.polish_global_shortcut.clone(), g.screenshot_shortcut.clone(), g.action_bar_shortcut.clone(), g.vault_autotype_shortcut.clone(), g.record_shortcut.clone(), g.clone())
    };
    // vault feature off 时 old_vault_autotype_sc 不被读；非 macOS 时 old_record_sc 不被读——
    // 统一标 unused 避免 warning。
    let _ = (&old_vault_autotype_sc, &old_record_sc);
    apply_config_value(&mut cfg, &key, &value)?;

    // 快捷键热重载：注册成功后才持久化（审查 Issue 3）。
    // 若先 save 再 register，注册失败时无效快捷键已写入 DB → 下次启动依然失败。
    if key == "asr_shortcut" && cfg.asr_shortcut != old_asr_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_asr_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::shortcut::register_shortcut(&app_handle, &cfg.asr_shortcut) {
            // 注册失败：尝试恢复旧快捷键，避免用户完全失去快捷键
            let _ = crate::shortcut::register_shortcut(&app_handle, &old_asr_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    // edit_global_shortcut 热重载：注册成功后才持久化（同 asr_shortcut 审查 Issue 3）。
    // 若先 save 再 register，注册失败时无效快捷键已写入 DB → 下次启动依然失败。
    if key == "edit_global_shortcut" && cfg.edit_global_shortcut != old_edit_global {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_edit_global.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::result_window::register_edit_global_shortcut(&app_handle, &cfg.edit_global_shortcut) {
            let _ = crate::result_window::register_edit_global_shortcut(&app_handle, &old_edit_global);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    // polish_global_shortcut 热重载：注册成功后才持久化（同 asr/edit_global 审查 Issue 3）。
    if key == "polish_global_shortcut" && cfg.polish_global_shortcut != old_polish_global {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_polish_global.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::result_window::register_polish_global_shortcut(&app_handle, &cfg.polish_global_shortcut) {
            let _ = crate::result_window::register_polish_global_shortcut(&app_handle, &old_polish_global);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    if key == "clipboard_shortcut" && cfg.clipboard_shortcut != old_clipboard_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_clipboard_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::clipboard_window::register_clipboard_shortcut(&app_handle, &cfg.clipboard_shortcut) {
            // 注册失败：恢复旧快捷键，避免用户完全失去快捷键
            let _ = crate::clipboard_window::register_clipboard_shortcut(&app_handle, &old_clipboard_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    if key == "screenshot_shortcut" && cfg.screenshot_shortcut != old_screenshot_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_screenshot_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::screenshot_commands::register_screenshot_shortcut(&app_handle, &cfg.screenshot_shortcut) {
            let _ = crate::screenshot_commands::register_screenshot_shortcut(&app_handle, &old_screenshot_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    // 录屏 toggle 快捷键热重载（仅 macOS——record_hotkey 模块 cfg-gate）。
    // stop 快捷键固定 ESC，按需注册（start 时 register，stop 时 unregister），不参与热重载。
    #[cfg(target_os = "macos")]
    if key == "record_shortcut" && cfg.record_shortcut != old_record_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        // 注销旧 toggle（ESC stop 由 register_stop_hotkey / unregister_stop_hotkey 单独管理）
        if let Ok(old) = old_record_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        // 注册新 toggle（register_toggle_hotkey 只动 toggle，不动 ESC）
        if let Err(e) = crate::record_hotkey::register_toggle_hotkey(
            &app_handle,
            &cfg.record_shortcut,
        ) {
            // 失败：恢复旧 toggle
            let _ = crate::record_hotkey::register_toggle_hotkey(
                &app_handle,
                &old_record_sc,
            );
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
        // 若当前正在录制，重新注册 ESC（register_toggle 不动 ESC；但旧 ESC 可能因
        // 上面 unregister 失败而残留——register_stop_hotkey 对同一快捷键是覆盖语义，安全）
        let session = app_handle.try_state::<octopus_record::RecordSession>();
        if let Some(s) = session {
            let in_recording = tauri::async_runtime::block_on(async {
                matches!(
                    s.state().await,
                    octopus_record::SessionState::Recording
                        | octopus_record::SessionState::Paused
                )
            });
            if in_recording {
                if let Err(e) = crate::record_hotkey::register_stop_hotkey(&app_handle) {
                    log::warn!("[settings] 录制中改快捷键，ESC 重新注册失败（不影响录制）: {e}");
                }
            }
        }
        // 注册成功：更新 tray 用的快捷键镜像 + 刷新菜单文案（显示新快捷键）
        *crate::tray::record_shortcut_mirror() = cfg.record_shortcut.clone();
        // 当前是否在录制决定显示「开始/停止」文案——读 session state（但这是 async，
        // tray 刷新在同步上下文。简化：默认显示「开始录屏 <新快捷键>」，
        // 若正在录制，下次 state 变化时 update_record_tray_label 会修正）。
        crate::tray::update_record_tray_label(false);
    }

    if key == "action_bar_shortcut" && cfg.action_bar_shortcut != old_action_bar_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_action_bar_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::action_bar_window::register_action_bar_shortcut(&app_handle, &cfg.action_bar_shortcut) {
            let _ = crate::action_bar_window::register_action_bar_shortcut(&app_handle, &old_action_bar_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    // vault_autotype_shortcut 热重载（2026-07-20 配置化）。
    // vault feature off 时整段跳过——register_vault_autotype_shortcut 不存在。
    #[cfg(feature = "vault")]
    if key == "vault_autotype_shortcut" && cfg.vault_autotype_shortcut != old_vault_autotype_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_vault_autotype_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::vault_commands::register_vault_autotype_shortcut(&app_handle, &cfg.vault_autotype_shortcut) {
            // 注册失败：恢复旧快捷键，避免用户完全失去快捷键
            let _ = crate::vault_commands::register_vault_autotype_shortcut(&app_handle, &old_vault_autotype_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    {
        let mut g = rc.write();
        *g = cfg.clone();
    }
    octopus_infra::db::save_app_config(&cfg).map_err(|e| e.to_string())?;

    // fuzzy_dialect 热重载：改方言规则后重建 corrector 热词索引（规则变 key 必变）。
    if key == "fuzzy_dialect" {
        octopus_asr_local::corrector::reload_fuzzy_dialect(&cfg.fuzzy_dialect);
    }

    // 刷新 ASR 侧 AppConfig 缓存（审查 二1）：denoise_mode / asr_hardware_accelerated
    // 等被 asr 的 load_app_config_cached 缓存（audio 每帧读 denoise、apply_session_acceleration
    // 读 hwaccel），不 reload 则改了也不生效（需重启）。从 DB 重读，set_config 罕见、可忽略成本。
    octopus_asr_local::config::reload_app_config();

    // 运行时可变字段立即同步到 coordinator 的 config 快照，
    // 无需等下次 Toggle（用户在录音中改 polish_mode 等也能立即生效）。
    // Task 2 后 asr_engine / polish_llm 不在 set_config 列表（走 switch_active_model）。
    if matches!(
        key.as_str(),
        "polish_mode" | "asr_correct" | "output_simplified" | "hide_toolbar"
    ) {
        coordinator.update_runtime();
    }

    // clipboard_enabled 热重载：翻转 watcher 运行时 flag（无需 stop/restart watcher）。
    if key == "clipboard_enabled" {
        use tauri::Manager;
        app_handle
            .state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>()
            .set_recording_enabled(cfg.clipboard_enabled);
    }

    // 所有配置变更都通知前端刷新——emit 开销极低，前端收到后幂等地重读 get_config。
    // 不再逐字段维护 emit 白名单（与 load/save 手动枚举同反模式，已踩坑多次）。
    {
        use tauri::Emitter;
        let _ = app_handle.emit("config-changed", ());
    }

    Ok(())
}

/// 按字段名校验类型/范围并赋值到 AppConfig。非法值返回 Err。
fn apply_config_value(
    cfg: &mut octopus_infra::config::AppConfig,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    match key {
        "language" => {
            let v = value.as_str().ok_or("language 需要字符串")?;
            if !["auto", "zh", "en", "ja", "ko"].contains(&v) {
                return Err(format!("language 非法值 '{}'（应为 auto/zh/en/ja/ko）", v));
            }
            cfg.language = v.to_string();
        }
        "ui_language" => {
            let v = value.as_str().ok_or("ui_language 需要字符串")?;
            if !["zh-CN", "en"].contains(&v) {
                return Err(format!("ui_language 非法值 '{}'（应为 zh-CN/en）", v));
            }
            cfg.ui_language = v.to_string();
        }
        "engine_mode" => {
            let v = value.as_str().ok_or("engine_mode 需要字符串")?;
            if !["embedded", "websocket", "grpc"].contains(&v) {
                return Err(format!("engine_mode 非法值 '{}'（应为 embedded/websocket/grpc）", v));
            }
            cfg.engine_mode = v.to_string();
        }
        "polish_mode" => {
            let v = value.as_u64().ok_or("polish_mode 需要 0/1/2")? as u8;
            cfg.polish_mode = match v {
                0 => PolishMode::Disabled,
                1 => PolishMode::FinalOnly,
                2 => PolishMode::Intermediate,
                _ => return Err(format!("polish_mode={} 非法（应为 0/1/2）", v)),
            };
        }
        "denoise_mode" => {
            let v = value.as_u64().ok_or("denoise_mode 需要 0/1/2")? as u8;
            if v > 2 {
                return Err(format!("denoise_mode={} 非法（应为 0/1/2）", v));
            }
            cfg.denoise_mode = v;
        }
        "asr_hardware_accelerated" => {
            cfg.asr_hardware_accelerated = value.as_bool().ok_or("asr_hardware_accelerated 需要 bool")?;
        }
        "asr_correct" => {
            cfg.asr_correct = value.as_bool().ok_or("asr_correct 需要 bool")?;
        }
        "fuzzy_dialect" => {
            // 逗号分隔 token 子集，与 hotword::parse_dialect 对齐；非法 token 拒绝（前端误传保护）。
            let v = value.as_str().ok_or("fuzzy_dialect 需要字符串")?;
            for tok in v.split(',').map(|t| t.trim()) {
                if tok.is_empty() {
                    continue;
                }
                if !matches!(tok, "f/h" | "hu/wu" | "n/l" | "r/l") {
                    return Err(format!(
                        "fuzzy_dialect 非法 token '{}'（应为 f/h、hu/wu、n/l、r/l 子集）",
                        tok
                    ));
                }
            }
            cfg.fuzzy_dialect = v.to_string();
        }
        "output_simplified" => {
            cfg.output_simplified = value.as_bool().ok_or("output_simplified 需要 bool")?;
        }
        "hide_toolbar" => {
            cfg.hide_toolbar = value.as_bool().ok_or("hide_toolbar 需要 bool")?;
        }
        "segment_silence" => {
            let v = value.as_f64().ok_or("segment_silence 需要数值")?;
            if v <= 0.0 { return Err("segment_silence 必须大于 0".into()); }
            cfg.segment_silence = v;
        }
        "polish_min_interval" => {
            let v = value.as_f64().ok_or("polish_min_interval 需要数值")?;
            if v < 0.0 { return Err("polish_min_interval 不能为负".into()); }
            cfg.polish_min_interval = v;
        }
        "pause_polish_threshold_ms" => {
            let v = value.as_f64().ok_or("pause_polish_threshold_ms 需要数值")?;
            if v < 600.0 {
                return Err("pause_polish_threshold_ms 必须 >= 600（需大于句间停顿最大值）".into());
            }
            cfg.pause_polish_threshold_ms = v;
        }
        "asr_shortcut" => {
            cfg.asr_shortcut = value.as_str().ok_or("asr_shortcut 需要字符串")?.to_string();
        }
        "edit_shortcut" => {
            cfg.edit_shortcut = value.as_str().ok_or("edit_shortcut 需要字符串")?.to_string();
        }
        "edit_global_shortcut" => {
            cfg.edit_global_shortcut = value.as_str().ok_or("edit_global_shortcut 需要字符串")?.to_string();
        }
        "polish_global_shortcut" => {
            cfg.polish_global_shortcut = value.as_str().ok_or("polish_global_shortcut 需要字符串")?.to_string();
        }
        // Task 2 后：asr_engine / polish_llm / ocr_model / translate_engine 已从 AppConfig 删除，
        // 激活态统一走 switch_active_model 命令（DB models.is_enabled）。set_config 不再处理这 4 个 key。
        "clipboard_shortcut" => {
            cfg.clipboard_shortcut = value.as_str().ok_or("clipboard_shortcut 需要字符串")?.to_string();
        }
        "clipboard_max_items" => {
            let v = value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
                .ok_or("clipboard_max_items 需要整数")?;
            if v < 10 { return Err("clipboard_max_items 必须 >= 10".into()); }
            cfg.clipboard_max_items = v;
        }
        "clipboard_max_age_days" => {
            let v = value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
                .ok_or("clipboard_max_age_days 需要整数")?;
            if v < 1 { return Err("clipboard_max_age_days 必须 >= 1".into()); }
            cfg.clipboard_max_age_days = v;
        }
        "clipboard_enabled" => {
            cfg.clipboard_enabled = value.as_bool().ok_or("clipboard_enabled 需要 bool")?;
        }
        "clipboard_theme" => {
            cfg.clipboard_theme = value.as_str().ok_or("clipboard_theme 需要字符串")?.to_string();
        }
        "action_bar_shortcut" => {
            cfg.action_bar_shortcut = value.as_str().ok_or("action_bar_shortcut 需要字符串")?.to_string();
        }
        "action_bar_search_engine" => {
            cfg.action_bar_search_engine = value.as_str().ok_or("action_bar_search_engine 需要字符串")?.to_string();
        }
        "screenshot_shortcut" => {
            cfg.screenshot_shortcut = value.as_str().ok_or("screenshot_shortcut 需要字符串")?.to_string();
        }
        "record_shortcut" => {
            cfg.record_shortcut = value.as_str().ok_or("record_shortcut 需要字符串")?.to_string();
        }
        "vault_autotype_shortcut" => {
            cfg.vault_autotype_shortcut = value.as_str().ok_or("vault_autotype_shortcut 需要字符串")?.to_string();
        }
        "switch_input_source_on_paste" => {
            cfg.switch_input_source_on_paste = value.as_bool().ok_or("switch_input_source_on_paste 需要 bool")?;
        }
        "microphone" => {
            cfg.microphone = value.as_str().ok_or("microphone 需要字符串")?.to_string();
        }
        _ => return Err(format!("未知配置字段: {}", key)),
    }
    Ok(())
}

#[tauri::command]
pub fn get_env_vars() -> Result<Vec<(String, String)>, String> {
    octopus_infra::db::list_env_vars().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_env_var(key: String, value: String) -> Result<(), String> {
    octopus_infra::db::save_env_var(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_env_var_cmd(key: String) -> Result<bool, String> {
    octopus_infra::db::delete_env_var(&key).map_err(|e| e.to_string())
}

// Task 2 后：build_asr_engine_spec / build_polish_llm_spec 已移除（激活态走
// switch_active_model 命令，不再经 set_config + spec 构造）。

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32, search: Option<String>) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset, search.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_history(ids: Vec<i64>, app_handle: tauri::AppHandle) -> Result<usize, String> {
    use tauri::Emitter;
    // 新 schema：transcriptions 已并入 clipboard_history，delete_transcriptions 直接删 voice 条目。
    let deleted = octopus_infra::db::delete_transcriptions(&ids).map_err(|e| e.to_string())?;
    if deleted > 0 {
        let _ = app_handle.emit("clipboard://changed", ());
    }
    Ok(deleted)
}

/// 检查快捷键是否可注册（是否被其他应用占用）。
/// 尝试注册 → 立即注销 → 返回结果。不改变当前已注册的快捷键。
#[tauri::command]
pub fn check_shortcut(
    shortcut: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let sc: Shortcut = shortcut
        .parse()
        .map_err(|e| format!("快捷键格式无效 '{}': {}", shortcut, e))?;
    app_handle
        .global_shortcut()
        .on_shortcut(sc, |_app, _scut, _event| {})
        .map_err(|e| format!("快捷键 '{}' 注册失败，可能被其他应用占用: {}", shortcut, e))?;
    // 注册成功 → 立即注销，仅做检测
    let _ = app_handle.global_shortcut().unregister(sc);
    Ok(())
}

/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// spec 为 polish_llm 配置值（3-part spec 或裸名），从 DB 加载配置后测试连通性。
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String> {
    if spec.is_empty() {
        return Err("未选择润色模型".into());
    }
    let llm_cfg = octopus_infra::db::load_llm_model(&spec)
        .map_err(|e| format!("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker
    tauri::async_runtime::spawn_blocking(move || {
        octopus_llm::test_connection(&llm_cfg).map_err(|e| format!("{}", e))
    })
    .await
    .map_err(|_| "测试线程异常终止".to_string())?
    .map(|_| "连接成功".to_string())
}

/// 测试 ASR 远程引擎连接是否可用。
/// 本地模型返回 Err 提示无需连接测试；远程模型（provider=aliyun）检查 secret_key + WS 连通性。
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    let engines = octopus_asr_local::config::list_engines_from_db().map_err(|e| e.to_string())?;
    let engine = engines.iter().find(|e| e.name == bare_name)
        .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;

    if engine.source_type != 2 {
        return Err("本地模型无需连接测试".into());
    }

    // 远程引擎：从 DB 取配置（resolve_engine_any 查任意可用 ASR）
    let (_cat, entry) = octopus_asr_local::config::resolve_engine_any(&bare_name)
        .ok_or_else(|| format!("远程 ASR 模型 '{}' 未在 DB 配置", bare_name))?;

    if entry.secret_key.is_empty() {
        return Err(format!("ASR 模型 '{}' 的 secret_key 为空", bare_name));
    }

    #[cfg(feature = "cloud")]
    {
        // follow-up #7：secret_key 可能是 v1: 加密格式（vault 启用后 Task 20 迁移过），
        // 透明解密得到明文 API Key。本地 / 未迁移明文 → no-op 返回原值。
        // 安全修复 #5：vault 启用但解密失败 → Err，不把密文当 bearer 发到云端。
        let secret_key_plain = crate::vault_secret_access::try_decrypt_secret_global(
            &entry.secret_key,
        )
        .map_err(|_| "云端推理失败：保险库未解锁或密文损坏，请先解锁保险库".to_string())?;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = entry.source.clone().into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", secret_key_plain)
                .parse()
                .map_err(|e| format!("secret_key 含非法 HTTP header 字符: {}", e))?,
        );
        // 直接在 tauri::async_runtime 上 await，不再 thread::spawn + Runtime::new + block_on
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio_tungstenite::connect_async(req),
        ).await {
            Ok(Ok(_)) => Ok("连接成功".into()),
            Ok(Err(e)) => Err(format!("WS 连接失败: {}", e)),
            Err(_) => Err("WS 连接超时（3s）".into()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        Err("远程 ASR 连接测试需要 aliyun feature".into())
    }
}

// ── 润色 prompt 管理（设置窗口 prompt 管理页）──

/// 读润色 prompt 文件内容。content 是文件名（不含扩展名）。
/// 路径：~/.octopus/.sync/prompts/polish/<content>.md
/// 失败时返回空串（降级——不让润色功能完全卡死）。
pub fn read_prompt_file(content: &str) -> String {
    let path = octopus_infra::paths::octopus_config_home()
        .join(".sync")
        .join("prompts")
        .join("polish")
        .join(format!("{}.md", content));
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// 设置窗口返回的 prompt 信息。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}

/// 列出所有润色 prompt（按 is_system 降序、id 升序）。
#[tauri::command]
pub fn list_prompts() -> Result<Vec<PromptInfo>, String> {
    let records = octopus_infra::db::list_prompts().map_err(|e| e.to_string())?;
    Ok(records
        .into_iter()
        .map(|r| PromptInfo {
            id: r.id,
            title: r.title,
            content: r.content,
            description: r.description,
            is_system: r.is_system,
        })
        .collect())
}

/// 返回当前激活的 prompt id。
#[tauri::command]
pub fn get_active_prompt() -> Result<i64, String> {
    octopus_infra::db::load_active_prompt_id().map_err(|e| e.to_string())
}

/// 设置激活 prompt（校验 id 存在 + 写 app_config + 调 set_system_prompt 即时生效）。
#[tauri::command]
pub fn set_active_prompt(id: i64) -> Result<(), String> {
    let record = octopus_infra::db::load_prompt(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("prompt id={} 不存在", id))?;
    octopus_infra::db::save_active_prompt_id(id).map_err(|e| e.to_string())?;
    let file_content = read_prompt_file(&record.content);
    octopus_llm::set_system_prompt(&file_content);
    log::info!("激活润色 prompt: id={} title={}", id, record.title);
    Ok(())
}

/// 新建用户 prompt（校验 title 非空）。返回新 id。
#[tauri::command]
pub fn create_prompt(
    title: String,
    content: String,
    description: String,
) -> Result<i64, String> {
    if title.trim().is_empty() {
        return Err("title 不能为空".into());
    }
    octopus_infra::db::insert_prompt(&title, &content, &description)
        .map_err(|e| e.to_string())
}

/// 更新 prompt（允许 system prompt 编辑——配合「复原默认」按钮；is_system 字段在 SQL
/// UPDATE 中不被修改，系统/用户身份保持不变）。
#[tauri::command]
pub fn update_prompt(
    id: i64,
    title: String,
    content: String,
    description: String,
) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title 不能为空".into());
    }
    octopus_infra::db::update_prompt(id, &title, &content, &description).map_err(|e| e.to_string())?;
    // 若更新的是当前激活 prompt，同步刷新 system_prompt
    let active = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    if active == id {
        if let Ok(Some(rec)) = octopus_infra::db::load_prompt(id) {
            octopus_llm::set_system_prompt(&read_prompt_file(&rec.content));
        }
    }
    Ok(())
}

/// 删除用户 prompt（拒绝 is_system=true；若删的是激活项，回退到 id=1）。
#[tauri::command]
pub fn delete_prompt(id: i64) -> Result<(), String> {
    let active = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    octopus_infra::db::delete_prompt(id).map_err(|e| e.to_string())?;
    // 删除激活项 → fallback 到 id=1
    if active == id {
        log::warn!("删除了激活 prompt id={}，回退到 id=1", id);
        let _ = octopus_infra::db::save_active_prompt_id(1);
        if let Ok(Some(rec)) = octopus_infra::db::load_prompt(1) {
            octopus_llm::set_system_prompt(&read_prompt_file(&rec.content));
        }
    }
    Ok(())
}

// ── 单测（纯逻辑校验，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_config_valid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "asr_correct", &json!(true)).unwrap();
        assert!(cfg.asr_correct);
        apply_config_value(&mut cfg, "asr_correct", &json!(false)).unwrap();
        assert!(!cfg.asr_correct);
    }

    #[test]
    fn apply_config_invalid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "asr_correct", &json!("yes")).is_err());
    }

    #[test]
    fn apply_config_valid_f64() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "segment_silence", &json!(450.0)).unwrap();
        assert_eq!(cfg.segment_silence, 450.0);
    }

    #[test]
    fn apply_config_invalid_f64_zero() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "segment_silence", &json!(0.0)).is_err());
        assert!(apply_config_value(&mut cfg, "segment_silence", &json!(-1.0)).is_err());
    }

    #[test]
    fn apply_config_pause_polish_threshold_must_ge_600() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(599.0)).is_err());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(500.0)).is_err());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(600.0)).is_ok());
    }

    #[test]
    fn apply_config_valid_polish_mode() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        for n in 0..=2u8 {
            apply_config_value(&mut cfg, "polish_mode", &json!(n)).unwrap();
        }
        assert!(apply_config_value(&mut cfg, "polish_mode", &json!(3)).is_err());
    }

    #[test]
    fn apply_config_valid_language() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "language", &json!("zh")).unwrap();
        assert_eq!(cfg.language, "zh");
        assert!(apply_config_value(&mut cfg, "language", &json!("fr")).is_err());
    }

    #[test]
    fn apply_config_unknown_key() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "nonexistent_field", &json!(1)).is_err());
    }

    #[test]
    fn apply_config_string_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "asr_shortcut", &json!("Ctrl+Alt+Z")).unwrap();
        assert_eq!(cfg.asr_shortcut, "Ctrl+Alt+Z");
        apply_config_value(&mut cfg, "microphone", &json!("External Mic")).unwrap();
        assert_eq!(cfg.microphone, "External Mic");
    }

    #[test]
    fn apply_config_valid_fuzzy_dialect() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "fuzzy_dialect", &json!("")).unwrap();
        assert_eq!(cfg.fuzzy_dialect, "");
        apply_config_value(&mut cfg, "fuzzy_dialect", &json!("f/h,hu/wu,n/l")).unwrap();
        assert_eq!(cfg.fuzzy_dialect, "f/h,hu/wu,n/l");
        // r/l 单独合法
        apply_config_value(&mut cfg, "fuzzy_dialect", &json!("r/l")).unwrap();
        assert_eq!(cfg.fuzzy_dialect, "r/l");
        // 四组组合合法
        apply_config_value(&mut cfg, "fuzzy_dialect", &json!("f/h,hu/wu,n/l,r/l")).unwrap();
        assert_eq!(cfg.fuzzy_dialect, "f/h,hu/wu,n/l,r/l");
    }

    #[test]
    fn apply_config_invalid_fuzzy_dialect() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        // 非 token（fh 而非 f/h）
        assert!(apply_config_value(&mut cfg, "fuzzy_dialect", &json!("fh")).is_err());
        // 含非法 token
        assert!(apply_config_value(&mut cfg, "fuzzy_dialect", &json!("f/h,bad")).is_err());
        // 非字符串
        assert!(apply_config_value(&mut cfg, "fuzzy_dialect", &json!(123)).is_err());
    }
}
