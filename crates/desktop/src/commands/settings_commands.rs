//! 设置窗口的 Tauri 命令：get_config / set_config / get_history。
//!
//! 与 runtime_config.rs 的区别：后者是工具栏专用命令（每个字段一个命令），
//! 本模块提供通用 get/set（方案 A），供设置窗口 GUI 表单使用。

use serde::Serialize;
use serde_json::Value;
use crate::core::error_util::{e2s, e2s_ctx};
use tauri::{Manager, State};

use crate::core::runtime_config::SharedRuntimeConfig;
use crate::core::config::PolishMode;

// ── get_config 返回 DTO ──
//
// ConfigResponse 是多个独立查询的聚合（config JSON + asr/llm/ocr engines + microphones
// + prompts + active_prompt_id），保留为独立结构——非纯 casing mirror，DTO 消除范围外。

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    pub config: Value,
    pub asr_engines: Vec<crate::core::runtime_config::EngineOption>,
    pub llm_models: Vec<crate::core::runtime_config::LlmOption>,
    pub ocr_models: Vec<crate::core::runtime_config::OcrOption>,
    pub microphones: Vec<String>,
    pub prompts: Vec<octopus_infra::db::PromptRecord>,
    pub active_prompt_id: i64,
}

#[tauri::command]
pub async fn get_config(_rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    // DB 查询 + cpal 麦克风枚举移入 spawn_blocking——避免阻塞 UI 线程。
    // Task 2 后：激活引擎从 ACTIVE_ENGINES 缓存取，不再读 AppConfig 4 个字段。
    let result = tokio::task::spawn_blocking(move || -> Result<ConfigResponse, String> {
        let cfg = octopus_infra::config::load_config().map_err(e2s)?;
        let config_json = serde_json::to_value(&cfg).map_err(e2s)?;

        // 各数据源独立容错：一个表查询失败不拖垮其他数据源（fail-soft）。
        // app_config（load_config）失败是致命的 → 仍用 ? 传播。
        // models/prompts 查询失败 → 返回空数组 + log warn，让页面降级渲染而非白屏。
        let engines = octopus_asr_local::config::list_engines_from_db()
            .unwrap_or_else(|e| { log::warn!("get_config: ASR 引擎查询失败: {}", e); vec![] });
        let asr_engines = crate::core::runtime_config::build_asr_options_public(engines);

        let llms = octopus_infra::db::list_llm_models()
            .unwrap_or_else(|e| { log::warn!("get_config: LLM 查询失败: {}", e); vec![] });
        let llm_models = crate::core::runtime_config::build_llm_options_public(llms);

        let ocrs = octopus_infra::db::list_ocr_models()
            .unwrap_or_else(|e| { log::warn!("get_config: OCR 查询失败: {}", e); vec![] });
        let ocr_models = crate::core::runtime_config::build_ocr_options_public(ocrs);

        let microphones = list_microphones();

        let prompt_records = octopus_infra::db::list_prompts()
            .unwrap_or_else(|e| { log::warn!("get_config: prompts 查询失败: {}", e); vec![] });
        let active_prompt_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);

        Ok(ConfigResponse {
            config: config_json,
            asr_engines,
            llm_models,
            ocr_models,
            microphones,
            prompts: prompt_records,
            active_prompt_id,
        })
    })
    .await
    .map_err(|e| e2s_ctx("get_config 任务异常: {}", e))?;

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

/// 快捷键热重载通用 helper：注销旧键 + 注册新键，失败回滚旧键。
///
/// `register` 是各快捷键的注册函数（如 register_clipboard_shortcut）。
/// 收敛 set_config 中 4 处「unregister old + register new + 失败回滚」同构分支（2026-08-05）。
fn reload_global_shortcut(
    app_handle: &tauri::AppHandle,
    old: &str,
    new: &str,
    register: fn(&tauri::AppHandle, &str) -> Result<(), String>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Ok(old_sc) = old.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        let _ = app_handle.global_shortcut().unregister(old_sc);
    }
    if let Err(e) = register(app_handle, new) {
        // 注册失败：恢复旧快捷键，避免用户完全失去快捷键
        let _ = register(app_handle, old);
        return Err(format!("快捷键注册失败，配置未更改: {}", e));
    }
    Ok(())
}

#[tauri::command]
pub fn set_config(
    key: String,
    value: Value,
    rc: State<'_, SharedRuntimeConfig>,
    coordinator: State<'_, crate::engine::coordinator::Coordinator>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // record_* 配置项不在 AppConfig struct 里（走 app_config 表的泛型 key-value 存储），
    // 直接写 DB 后返回，不走 apply_config_value（会报「未知配置字段」）。
    // 包括：record_output_dir / record_reveal_after_stop / record_microphone_device 等。
    // 注意：record_shortcut 在 AppConfig struct 里（走 apply_config_value）；record_stop
    // 固定为 Escape 常量（record_hotkey.rs::STOP_SHORTCUT），不是配置项。
    if key.starts_with("record_") && key != "record_shortcut" {
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
        .map_err(|e| e2s_ctx("写入 DB 失败", e))?;
        return Ok(());
    }
    // subtitle_* 配置项同 record_* 范式：不在 AppConfig struct，走 app_config 表的泛型
    // key-value 存储（包括 subtitle_llm_polish_default / subtitle_polish_llm_key 等）。
    // 后端读取用 load_config_key（参见 record_* 的 load_config_key 用法），不走 AppConfig struct。
    if key.starts_with("subtitle_") {
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
        .map_err(|e| e2s_ctx("写入 DB 失败", e))?;
        return Ok(());
    }
    let (old_asr_sc, old_clipboard_sc, old_edit_global, old_screenshot_sc, old_action_bar_sc, old_vault_autotype_sc, old_record_sc, old_paste_stack_sc, mut cfg) = {
        let g = rc.read();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.screenshot_shortcut.clone(), g.action_bar_shortcut.clone(), g.vault_autotype_shortcut.clone(), g.record_shortcut.clone(), g.paste_stack_shortcut.clone(), g.clone())
    };
    // vault feature off 时 old_vault_autotype_sc 不被读；非 macOS 时 old_record_sc 不被读——
    // 统一标 unused 避免 warning。
    let _ = (&old_vault_autotype_sc, &old_record_sc);
    apply_config_value(&mut cfg, &key, &value)?;

    // 快捷键热重载：注册成功后才持久化（审查 Issue 3）。
    // 若先 save 再 register，注册失败时无效快捷键已写入 DB → 下次启动依然失败。
    // asr_shortcut 热重载：PTT 键（不再用 Tauri global-shortcut）。
    // unregister 旧 + register 新；失败回滚旧键（保证用户至少有可用 PTT 键）。
    if key == "asr_shortcut" && cfg.asr_shortcut != old_asr_sc {
        let _ = crate::platform::ptt::unregister_ptt(&app_handle);
        if let Err(e) = crate::platform::ptt::register_ptt(&app_handle, &cfg.asr_shortcut) {
            log::warn!("[set_config] register_ptt 新键失败，回滚旧键: {}", e);
            let _ = crate::platform::ptt::register_ptt(&app_handle, &old_asr_sc);
            return Err(format!("PTT 键注册失败，配置未更改: {}", e));
        }
    }

    // edit_global_shortcut 热重载：注册成功后才持久化（同 asr_shortcut 审查 Issue 3）。
    // 若先 save 再 register，注册失败时无效快捷键已写入 DB → 下次启动依然失败。
    if key == "edit_global_shortcut" && cfg.edit_global_shortcut != old_edit_global {
        reload_global_shortcut(&app_handle, &old_edit_global, &cfg.edit_global_shortcut, crate::ui::result_window::register_edit_global_shortcut)?;
    }

    // polish_global_shortcut 已删除（Task 2 后不再支持 polish 全局快捷键）。

    if key == "clipboard_shortcut" && cfg.clipboard_shortcut != old_clipboard_sc {
        reload_global_shortcut(&app_handle, &old_clipboard_sc, &cfg.clipboard_shortcut, crate::clipboard::clipboard_window::register_clipboard_shortcut)?;
    }

    // paste_stack_shortcut 热重载：与 clipboard_shortcut 同模式（注销旧 + 注册新）。
    if key == "paste_stack_shortcut" && cfg.paste_stack_shortcut != old_paste_stack_sc {
        reload_global_shortcut(&app_handle, &old_paste_stack_sc, &cfg.paste_stack_shortcut, crate::clipboard::clipboard_window::register_paste_stack_shortcut)?;
    }

    if key == "screenshot_shortcut" && cfg.screenshot_shortcut != old_screenshot_sc {
        reload_global_shortcut(&app_handle, &old_screenshot_sc, &cfg.screenshot_shortcut, crate::record::screenshot_commands::register_screenshot_shortcut)?;
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
        if let Err(e) = crate::record::record_hotkey::register_toggle_hotkey(
            &app_handle,
            &cfg.record_shortcut,
        ) {
            // 失败：恢复旧 toggle
            let _ = crate::record::record_hotkey::register_toggle_hotkey(
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
                if let Err(e) = crate::record::record_hotkey::register_stop_hotkey(&app_handle) {
                    log::warn!("[settings] 录制中改快捷键，ESC 重新注册失败（不影响录制）: {e}");
                }
            }
        }
        // 注册成功：更新 tray 用的快捷键镜像 + 刷新菜单文案（显示新快捷键）
        *crate::ui::tray::record_shortcut_mirror() = cfg.record_shortcut.clone();
        // 当前是否在录制决定显示「开始/停止」文案——读 session state（但这是 async，
        // tray 刷新在同步上下文。简化：默认显示「开始录屏 <新快捷键>」，
        // 若正在录制，下次 state 变化时 update_record_tray_label 会修正）。
        crate::ui::tray::update_record_tray_label(false);
    }

    if key == "action_bar_shortcut" && cfg.action_bar_shortcut != old_action_bar_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_action_bar_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::action_bar::action_bar_window::register_action_bar_shortcut(&app_handle, &cfg.action_bar_shortcut) {
            let _ = crate::action_bar::action_bar_window::register_action_bar_shortcut(&app_handle, &old_action_bar_sc);
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
        if let Err(e) = crate::vault::vault_commands::register_vault_autotype_shortcut(&app_handle, &cfg.vault_autotype_shortcut) {
            // 注册失败：恢复旧快捷键，避免用户完全失去快捷键
            let _ = crate::vault::vault_commands::register_vault_autotype_shortcut(&app_handle, &old_vault_autotype_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    {
        let mut g = rc.write();
        *g = cfg.clone();
    }
    octopus_infra::db::save_app_config(&cfg).map_err(e2s)?;

    // 快捷键类配置变更后刷新 tray 菜单文案（显示新快捷键）。
    // record_start 的文案由上方 update_record_tray_label 单独处理；这里覆盖其余项
    // （toggle / screenshot / clipboard）。
    if key.ends_with("_shortcut") {
        crate::ui::tray::rebuild_tray_labels(&cfg);
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
        // fuzzy_dialect 已迁移到 fuzzy_dialect_rules DB 表（2026-08-01），不再经 app_config 字符串。
        // 规则开关走 set_fuzzy_dialect_rule 命令。
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
            // PTT 键名白名单（与 platform::ptt::register_ptt 的 parse 对齐）。
            // 非法值拒绝（前端误传保护）——避免无意义字符串进入 DB。
            let v = value.as_str().ok_or("asr_shortcut 需要字符串")?;
            if !["OptRight", "CmdRight", "CtrlRight", "ShiftRight", "Fn"].contains(&v) {
                return Err("asr_shortcut 必须是 OptRight/CmdRight/CtrlRight/ShiftRight/Fn 之一".into());
            }
            cfg.asr_shortcut = v.to_string();
        }
        "edit_shortcut" => {
            cfg.edit_shortcut = value.as_str().ok_or("edit_shortcut 需要字符串")?.to_string();
        }
        "edit_global_shortcut" => {
            cfg.edit_global_shortcut = value.as_str().ok_or("edit_global_shortcut 需要字符串")?.to_string();
        }
        // Task 2 后：polish_global_shortcut 已从 AppConfig 删除（不再支持 polish 全局快捷键）。
        // Task 2 后：asr_engine / polish_llm / ocr_model / translate_engine 已从 AppConfig 删除，
        // 激活态统一走 switch_active_model 命令（DB models.is_enabled）。set_config 不再处理这 4 个 key。
        "clipboard_shortcut" => {
            cfg.clipboard_shortcut = value.as_str().ok_or("clipboard_shortcut 需要字符串")?.to_string();
        }
        "paste_stack_shortcut" => {
            cfg.paste_stack_shortcut = value.as_str().ok_or("paste_stack_shortcut 需要字符串")?.to_string();
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
        // 截图水印 4 字段（与 config.rs AppConfig 字段对齐）。
        // text/position 走字符串；opacity clamp 0-1；font_size max(1) 防 0/负数。
        "screenshot_watermark_text" => {
            cfg.screenshot_watermark_text = value.as_str().ok_or("screenshot_watermark_text 需要字符串")?.to_string();
        }
        "screenshot_watermark_density" => {
            let v = value.as_f64().ok_or("screenshot_watermark_density 需要数值")?;
            cfg.screenshot_watermark_density = (v as f32).clamp(0.0, 1.0);
        }
        "screenshot_watermark_angle" => {
            let v = value.as_f64().ok_or("screenshot_watermark_angle 需要数值")?;
            cfg.screenshot_watermark_angle = (v as f32).clamp(0.0, 360.0);
        }
        "screenshot_watermark_opacity" => {
            let v = value.as_f64().ok_or("screenshot_watermark_opacity 需要数值")?;
            cfg.screenshot_watermark_opacity = (v as f32).clamp(0.0, 1.0);
        }
        "screenshot_watermark_color" => {
            cfg.screenshot_watermark_color = value.as_str().ok_or("screenshot_watermark_color 需要字符串")?.to_string();
        }
        "screenshot_watermark_font_size" => {
            let v = value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
                .ok_or("screenshot_watermark_font_size 需要整数")?;
            if v < 1 { return Err("screenshot_watermark_font_size 必须 >= 1".into()); }
            cfg.screenshot_watermark_font_size = v as u32;
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
        // 终端字体偏好（无副作用：前端读 get_config 后直接套到 xterm CSS，不需重注册/重启）。
        "terminal_font_size" => {
            let v = value.as_f64().ok_or("terminal_font_size 需要数值")?;
            if v <= 0.0 { return Err("terminal_font_size 必须大于 0".into()); }
            cfg.terminal_font_size = v;
        }
        "terminal_font_family" => {
            cfg.terminal_font_family = value.as_str().ok_or("terminal_font_family 需要字符串")?.to_string();
        }
        _ => return Err(format!("未知配置字段: {}", key)),
    }
    Ok(())
}

/// 列出系统已安装的等宽字体（终端字体选择用）。
/// 优先用 fc-list（fontconfig），不可用时 fallback 到 macOS 常见白名单。
#[tauri::command]
pub fn list_monospace_fonts() -> Result<Vec<String>, String> {
    // 尝试 fc-list（homebrew fontconfig）——列出 spacing=mono 的字体族
    if let Ok(output) = std::process::Command::new("fc-list")
        .args([":spacing=mono", "family"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            return Ok(parse_monospace_fonts(&text));
        }
    }
    // fallback：macOS 自带 + 常见编程字体白名单
    Ok(vec![
        "Andale Mono".into(),
        "Courier New".into(),
        "Menlo".into(),
        "Monaco".into(),
        "PT Mono".into(),
        "SF Mono".into(),
    ])
}

/// 解析 fc-list 输出为去重排序后的等宽字体族列表（纯函数，便于单测）。
///
/// 处理步骤：
/// 1. 按行切分 + trim + 过滤空行
/// 2. 过滤 `.` 开头的系统隐藏/特殊字体（`.Apple Color Emoji UI` / `.LastResort` /
///    `.SF NS Mono` / `.Times LT MM` 等）——它们不是真实可用的等宽字体，xterm 选中后
///    渲染异常（字变小 + 间距大）
/// 3. sort + dedup
/// 4. 补 SF Mono / Monaco（fc-list 可能漏掉 macOS 特殊字体）
pub fn parse_monospace_fonts(raw_output: &str) -> Vec<String> {
    let mut fonts: Vec<String> = raw_output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with('.'))
        .collect();
    fonts.sort();
    fonts.dedup();
    // fc-list 可能漏掉 macOS 特殊字体（SF Mono），手动补
    for extra in ["SF Mono", "Monaco"] {
        if !fonts.iter().any(|f| f == extra) {
            fonts.push(extra.to_string());
        }
    }
    fonts.sort();
    fonts
}

#[tauri::command]
pub fn get_env_vars() -> Result<Vec<(String, String)>, String> {
    octopus_infra::db::list_env_vars().map_err(e2s)
}

#[tauri::command]
pub fn set_env_var(key: String, value: String) -> Result<(), String> {
    octopus_infra::db::save_env_var(&key, &value).map_err(e2s)
}

#[tauri::command]
pub fn delete_env_var_cmd(key: String) -> Result<bool, String> {
    octopus_infra::db::delete_env_var(&key).map_err(e2s)
}

// Task 2 后：build_asr_engine_spec / build_polish_llm_spec 已移除（激活态走
// switch_active_model 命令，不再经 set_config + spec 构造）。

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32, search: Option<String>) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset, search.as_deref()).map_err(e2s)
}

#[tauri::command]
pub fn delete_history(ids: Vec<String>, app_handle: tauri::AppHandle) -> Result<usize, String> {
    use tauri::Emitter;
    // 新 schema：transcriptions 已并入 clipboard_history，delete_transcriptions 直接删 voice 条目。
    let deleted = octopus_infra::db::delete_transcriptions(&ids).map_err(e2s)?;
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
        .map_err(|e| e2s_ctx("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker
    tauri::async_runtime::spawn_blocking(move || {
        octopus_llm::test_connection(&llm_cfg).map_err(|e| e2s_ctx("{}", e))
    })
    .await
    .map_err(|_| "测试线程异常终止".to_string())?
    .map(|_| "连接成功".to_string())
}

/// 测试 ASR 远程引擎连接是否可用。
/// 本地模型返回 Err 提示无需连接测试；远程模型（provider=aliyun）检查 secret_key + WS 连通性。
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    let engines = octopus_asr_local::config::list_engines_from_db().map_err(e2s)?;
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
        let secret_key_plain = crate::vault::vault_secret_access::try_decrypt_secret_global(
            &entry.secret_key,
        )
        .map_err(|_| "云端推理失败：保险库未解锁或密文损坏，请先解锁保险库".to_string())?;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = entry.source.clone().into_client_request()
            .map_err(|e| e2s_ctx("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", secret_key_plain)
                .parse()
                .map_err(|e| e2s_ctx("secret_key 含非法 HTTP header 字符: {}", e))?,
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

/// 列出所有润色 prompt（按 is_system 降序、id 升序）。
///
/// 直接返回内部 `PromptRecord`（已带 `rename_all = "camelCase"`，2026-07-27 DTO
/// 消除：原 `PromptInfo` 与 `PromptRecord` 字段 1:1 完全一致，纯冗余包装）。
#[tauri::command]
pub fn list_prompts() -> Result<Vec<octopus_infra::db::PromptRecord>, String> {
    octopus_infra::db::list_prompts().map_err(e2s)
}

/// 返回当前激活的 prompt id。
#[tauri::command]
pub fn get_active_prompt() -> Result<i64, String> {
    octopus_infra::db::load_active_prompt_id().map_err(e2s)
}

/// 设置激活 prompt（校验 id 存在 + 写 app_config + 调 set_system_prompt 即时生效）。
#[tauri::command]
pub fn set_active_prompt(id: i64) -> Result<(), String> {
    let record = octopus_infra::db::load_prompt(id)
        .map_err(e2s)?
        .ok_or_else(|| format!("prompt id={} 不存在", id))?;
    octopus_infra::db::save_active_prompt_id(id).map_err(e2s)?;
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
    app_bundle_ids: String,
    inject_context: bool,
) -> Result<i64, String> {
    if title.trim().is_empty() {
        return Err("title 不能为空".into());
    }
    let id = octopus_infra::db::insert_prompt(&title, &content, &description, &app_bundle_ids, inject_context)
        .map_err(e2s)?;
    crate::engine::coordinator::invalidate_route_cache();
    Ok(id)
}

/// 更新 prompt（允许 system prompt 编辑——配合「复原默认」按钮；is_system 字段在 SQL
/// UPDATE 中不被修改，系统/用户身份保持不变）。
///
/// 系统内置模板的路由字段（app_bundle_ids + inject_context）锁定不可改——保持全局
/// fallback 角色。用户传入的值被忽略，回写 DB 现有值（防御：即便绕过前端灰禁调用也无效）。
#[tauri::command]
pub fn update_prompt(
    id: i64,
    title: String,
    content: String,
    description: String,
    app_bundle_ids: String,
    inject_context: bool,
) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title 不能为空".into());
    }
    // 系统模板路由字段锁定：忽略传入值，用 DB 现有值回写
    let (app_bundle_ids, inject_context) =
        match octopus_infra::db::load_prompt(id).map_err(e2s)? {
            Some(rec) if rec.is_system => (rec.app_bundle_ids, rec.inject_context),
            _ => (app_bundle_ids, inject_context),
        };
    octopus_infra::db::update_prompt(id, &title, &content, &description, &app_bundle_ids, inject_context).map_err(e2s)?;
    crate::engine::coordinator::invalidate_route_cache();
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
    octopus_infra::db::delete_prompt(id).map_err(e2s)?;
    crate::engine::coordinator::invalidate_route_cache();
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
        // asr_shortcut 是 PTT 键名白名单（OptRight/CmdRight/...）
        apply_config_value(&mut cfg, "asr_shortcut", &json!("OptRight")).unwrap();
        assert_eq!(cfg.asr_shortcut, "OptRight");
        // 非白名单值应被拒绝
        assert!(apply_config_value(&mut cfg, "asr_shortcut", &json!("Ctrl+Alt+Z")).is_err());
        apply_config_value(&mut cfg, "microphone", &json!("External Mic")).unwrap();
        assert_eq!(cfg.microphone, "External Mic");
    }

    // fuzzy_dialect 测试已删除（迁移到 fuzzy_dialect_rules DB 表，不再经 app_config 字符串）

    // ── parse_monospace_fonts：fc-list 输出解析 + 过滤 ──

    #[test]
    fn parse_monospace_fonts_filters_dot_prefix() {
        // macOS fontconfig 会列出 .Apple Color Emoji UI / .LastResort / .SF NS Mono 等
        // 系统隐藏字体——非真实等宽，xterm 选中后渲染异常，必须过滤。
        let raw = "Menlo\n.Apple Color Emoji UI\nMonaco\n.SF NS Mono\n.LastResort";
        let fonts = parse_monospace_fonts(raw);
        assert!(!fonts.contains(&".Apple Color Emoji UI".to_string()));
        assert!(!fonts.contains(&".SF NS Mono".to_string()));
        assert!(!fonts.contains(&".LastResort".to_string()));
        assert!(fonts.contains(&"Menlo".to_string()));
        assert!(fonts.contains(&"Monaco".to_string()));
    }

    #[test]
    fn parse_monospace_fonts_sorts_and_dedups() {
        // fc-list 输出可能乱序 + 重复（多个 weight 同名）
        let raw = "Monaco\nMenlo\nMonaco\nMenlo\nAndale Mono";
        let fonts = parse_monospace_fonts(raw);
        // 去重
        let dedup_count = fonts.iter().filter(|f| **f == "Monaco").count();
        assert_eq!(dedup_count, 1);
        // 排序（字母序）——Andale Mono 在前
        assert_eq!(fonts[0], "Andale Mono");
    }

    #[test]
    fn parse_monospace_fonts_supplements_sf_mono_monaco() {
        // fc-list 可能漏掉 SF Mono（macOS 特殊字体），手动补
        let raw = "Menlo\nAndale Mono";
        let fonts = parse_monospace_fonts(raw);
        assert!(fonts.contains(&"SF Mono".to_string()), "应补 SF Mono");
        assert!(fonts.contains(&"Monaco".to_string()), "应补 Monaco");
    }

    #[test]
    fn parse_monospace_fonts_no_duplicate_supplement() {
        // fc-list 已列出 SF Mono 时，补丁不应重复加
        let raw = "Menlo\nSF Mono\nMonaco";
        let fonts = parse_monospace_fonts(raw);
        let sf_count = fonts.iter().filter(|f| **f == "SF Mono").count();
        let monaco_count = fonts.iter().filter(|f| **f == "Monaco").count();
        assert_eq!(sf_count, 1, "SF Mono 不应重复");
        assert_eq!(monaco_count, 1, "Monaco 不应重复");
    }

    #[test]
    fn parse_monospace_fonts_trims_whitespace() {
        // fc-list 输出可能有尾随空格/前导空格
        let raw = "  Menlo  \n\tMonaco\t\n  SF Mono";
        let fonts = parse_monospace_fonts(raw);
        assert!(fonts.contains(&"Menlo".to_string()));
        assert!(fonts.contains(&"Monaco".to_string()));
        // 不应含带空格的原始项
        assert!(!fonts.iter().any(|f| f.contains("  ") || f.starts_with(' ')));
    }

    #[test]
    fn parse_monospace_fonts_empty_input() {
        // fc-list 输出为空（极端情况）——只返回补丁的 SF Mono / Monaco
        let fonts = parse_monospace_fonts("");
        assert_eq!(fonts, vec!["Monaco".to_string(), "SF Mono".to_string()]);
    }

    #[test]
    fn parse_monospace_fonts_filters_empty_lines() {
        let raw = "\n\nMenlo\n\n\nMonaco\n";
        let fonts = parse_monospace_fonts(raw);
        assert!(fonts.contains(&"Menlo".to_string()));
        assert!(fonts.contains(&"Monaco".to_string()));
        // 不应含空串
        assert!(!fonts.iter().any(|f| f.is_empty()));
    }

    #[test]
    fn parse_monospace_fonts_preserves_spaces_in_names() {
        // 合法字体名含空格（如 "Courier New"）不应被破坏——只 trim 首尾
        let raw = "Courier New\nSF Mono";
        let fonts = parse_monospace_fonts(raw);
        assert!(fonts.contains(&"Courier New".to_string()), "含空格的字体名应保留");
    }
}
