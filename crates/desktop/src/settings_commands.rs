//! 设置窗口的 Tauri 命令：get_config / set_config / get_history。
//!
//! 与 runtime_config.rs 的区别：后者是工具栏专用命令（每个字段一个命令），
//! 本模块提供通用 get/set（方案 A），供设置窗口 GUI 表单使用。

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::runtime_config::SharedRuntimeConfig;
use crate::config::PolishMode;

// ── get_config 返回 DTO ──

#[derive(Serialize)]
pub struct ConfigResponse {
    pub config: Value,
    pub asr_engines: Vec<crate::runtime_config::EngineOption>,
    pub llm_models: Vec<crate::runtime_config::LlmOption>,
    pub microphones: Vec<String>,
    pub prompts: Vec<PromptInfo>,
    pub active_prompt_id: i64,
}

#[tauri::command]
pub fn get_config(rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let config_json = serde_json::to_value(&cfg).map_err(|e| e.to_string())?;

    let g = rc.read().unwrap();
    let engines = octopus_asr_local::config::list_engines().map_err(|e| e.to_string())?;
    let asr_engines = crate::runtime_config::build_asr_options_public(&g.asr_engine, engines);

    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);

    let microphones = list_microphones();

    let prompt_records = octopus_infra::db::list_prompts().map_err(|e| e.to_string())?;
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
        microphones,
        prompts,
        active_prompt_id,
    })
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
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let (old_shortcut, mut cfg) = {
        let g = rc.read().unwrap();
        (g.asr_shortcut.clone(), g.clone())
    };
    apply_config_value(&mut cfg, &key, &value)?;

    // 快捷键热重载：注册成功后才持久化（审查 Issue 3）。
    // 若先 save 再 register，注册失败时无效快捷键已写入 DB → 下次启动依然失败。
    if key == "asr_shortcut" && cfg.asr_shortcut != old_shortcut {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_shortcut.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::shortcut::register_shortcut(&app_handle, &cfg.asr_shortcut) {
            // 注册失败：尝试恢复旧快捷键，避免用户完全失去快捷键
            let _ = crate::shortcut::register_shortcut(&app_handle, &old_shortcut);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }

    {
        let mut g = rc.write().unwrap();
        *g = cfg.clone();
    }
    octopus_infra::db::save_app_config(&cfg).map_err(|e| e.to_string())?;

    // 刷新 ASR 侧 AppConfig 缓存（审查 二1）：denoise_mode / asr_hardware_accelerated
    // 等被 asr 的 load_app_config_cached 缓存（audio 每帧读 denoise、apply_session_acceleration
    // 读 hwaccel），不 reload 则改了也不生效（需重启）。从 DB 重读，set_config 罕见、可忽略成本。
    octopus_asr_local::config::reload_app_config();

    // 审查 三2：切 asr_engine 时后台预热本地引擎（避免首次 transcribe 懒加载卡顿）。
    if key == "asr_engine" {
        crate::runtime_config::preheat_local_engine(
            engine_manager.inner().clone(),
            &cfg.asr_engine,
            &cfg.engine_mode,
        );
    }

    // 运行时可变字段立即同步到 coordinator 的 config 快照，
    // 无需等下次 Toggle（用户在录音中改 polish_llm 等也能立即生效）
    if matches!(
        key.as_str(),
        "polish_llm" | "polish_mode" | "asr_correct" | "output_simplified" | "hide_toolbar"
    ) {
        coordinator.update_runtime();
    }

    // hide_toolbar / edit_shortcut 改变时通知 result window 刷新
    if key == "hide_toolbar" || key == "edit_shortcut" {
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
        "microphone" => {
            cfg.microphone = value.as_str().ok_or("microphone 需要字符串")?.to_string();
        }
        "asr_engine" => {
            let bare_name = value.as_str().ok_or("asr_engine 需要字符串")?;
            // 前端传裸 model_name，需构造 3-part spec
            cfg.asr_engine = build_asr_engine_spec(bare_name)?;
        }
        "polish_llm" => {
            let bare_name = value.as_str().ok_or("polish_llm 需要字符串")?;
            // 前端传裸 model_name，空串=不选择模型，其余构造 3-part spec
            cfg.polish_llm = build_polish_llm_spec(bare_name)?;
        }
        _ => return Err(format!("未知配置字段: {}", key)),
    }
    Ok(())
}

/// 将前端传来的裸 model_name 构造为 3-part ASR spec "{provider}:{category}:{model_name}"。
/// 兜底引擎固定 "local:zipformer:NAME"；其余查 DB 取 provider/category。
fn build_asr_engine_spec(bare_name: &str) -> Result<String, String> {
    use crate::runtime_config::FALLBACK_ASR_ENGINE;
    let engines = octopus_asr_local::config::list_engines().map_err(|e| e.to_string())?;
    if bare_name == FALLBACK_ASR_ENGINE {
        Ok(format!("local:zipformer:{}", bare_name))
    } else {
        let engine = engines.iter().find(|e| e.name == bare_name)
            .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;
        Ok(format!(
            "{}:{}:{}",
            engine.provider,
            octopus_asr_local::config::category_label(engine.category),
            bare_name
        ))
    }
}

/// 将前端传来的裸 model_name 构造为 3-part LLM spec "{provider}:{category}:{model_name}"。
/// 空串 = 「不选择模型」，直接返回空。
fn build_polish_llm_spec(bare_name: &str) -> Result<String, String> {
    if bare_name.is_empty() {
        Ok(String::new())
    } else {
        let model = octopus_infra::db::list_llm_models()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.model_name == bare_name)
            .ok_or_else(|| format!("润色模型 '{}' 不存在", bare_name))?;
        Ok(format!("{}:{}:{}", model.provider, model.category, model.model_name))
    }
}

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_history(ids: Vec<i64>) -> Result<usize, String> {
    // 删除转译记录，同步删除剪贴板中引用这些记录的条目
    let deleted = octopus_infra::db::delete_transcriptions(&ids).map_err(|e| e.to_string())?;
    let _ = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::delete_by_transcription_ids(conn, &ids)
    });
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
    let engines = octopus_asr_local::config::list_engines().map_err(|e| e.to_string())?;
    let engine = engines.iter().find(|e| e.name == bare_name)
        .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;

    if engine.is_local {
        return Err("本地模型无需连接测试".into());
    }

    // 远程引擎：从 DB 取配置（source = WS endpoint, secret_key = API Key）
    let asr_cfg = octopus_asr_local::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(&bare_name).model_name().to_string();
    let entry = asr_cfg.asr.aliyun.as_ref()
        .and_then(|m| m.get(model_name.as_str()))
        .ok_or_else(|| format!("远程 ASR 模型 '{}' 未在 DB 配置", bare_name))?;

    if entry.secret_key.is_empty() {
        return Err(format!("ASR 模型 '{}' 的 secret_key 为空", bare_name));
    }

    #[cfg(feature = "cloud")]
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = entry.source.clone().into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", entry.secret_key).parse().unwrap(),
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

/// 设置窗口返回的 prompt 信息。
#[derive(Serialize)]
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
    octopus_llm::set_system_prompt(&record.content);
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

/// 更新用户 prompt（拒绝 is_system=true）。
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
            octopus_llm::set_system_prompt(&rec.content);
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
            octopus_llm::set_system_prompt(&rec.content);
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
}
