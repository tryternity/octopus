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
}

#[tauri::command]
pub fn get_config(rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let config_json = serde_json::to_value(&cfg).map_err(|e| e.to_string())?;

    let g = rc.read().unwrap();
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    let asr_engines = crate::runtime_config::build_asr_options_public(&g.asr_engine, engines);

    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);

    let microphones = list_microphones();

    Ok(ConfigResponse {
        config: config_json,
        asr_engines,
        llm_models,
        microphones,
    })
}

/// 枚举系统麦克风设备（cpal 跨平台）。
fn list_microphones() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// ── set_config 命令 ──

#[tauri::command]
pub fn set_config(
    key: String,
    value: Value,
    rc: State<'_, SharedRuntimeConfig>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let old_shortcut = {
        let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
        cfg.shortcut.clone()
    };
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    apply_config_value(&mut cfg, &key, &value)?;
    sync_runtime_config(&rc, &key, &cfg);
    write_config_yaml(&cfg)?;

    // 快捷键热重载：注销旧的 → 注册新的
    if key == "shortcut" && cfg.shortcut != old_shortcut {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_shortcut.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        crate::shortcut::register_shortcut(&app_handle, &cfg.shortcut)
            .map_err(|e| format!("快捷键已保存但注册失败: {}", e))?;
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
        "segment_duration" => {
            let v = value.as_f64().ok_or("segment_duration 需要数值")?;
            if v <= 0.0 { return Err("segment_duration 必须大于 0".into()); }
            cfg.segment_duration = v;
        }
        "segment_silence" => {
            let v = value.as_f64().ok_or("segment_silence 需要数值")?;
            if v <= 0.0 { return Err("segment_silence 必须大于 0".into()); }
            cfg.segment_silence = v;
        }
        "segment_overlap" => {
            let v = value.as_f64().ok_or("segment_overlap 需要数值")?;
            if v < 0.0 { return Err("segment_overlap 不能为负".into()); }
            cfg.segment_overlap = v;
        }
        "polish_interval" => {
            let v = value.as_f64().ok_or("polish_interval 需要数值")?;
            if v < 0.0 { return Err("polish_interval 不能为负".into()); }
            cfg.polish_interval = v;
        }
        "pause_polish_threshold_ms" => {
            let v = value.as_f64().ok_or("pause_polish_threshold_ms 需要数值")?;
            if v < 500.0 {
                return Err("pause_polish_threshold_ms 必须 >= 500（Active Flush 阈值）".into());
            }
            cfg.pause_polish_threshold_ms = v;
        }
        "shortcut" => {
            cfg.shortcut = value.as_str().ok_or("shortcut 需要字符串")?.to_string();
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
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    if bare_name == FALLBACK_ASR_ENGINE {
        Ok(format!("local:zipformer:{}", bare_name))
    } else {
        let engine = engines.iter().find(|e| e.name == bare_name)
            .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;
        Ok(format!(
            "{}:{}:{}",
            engine.provider,
            octopus_asr::config::category_label(engine.category),
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

/// 字段属于 RuntimeConfig 镜像范围的，同步更新。
fn sync_runtime_config(
    rc: &SharedRuntimeConfig,
    key: &str,
    cfg: &octopus_infra::config::AppConfig,
) {
    let mut g = rc.write().unwrap();
    match key {
        "asr_engine" => g.asr_engine = cfg.asr_engine.clone(),
        "polish_mode" => g.polish_mode = cfg.polish_mode,
        "polish_llm" => g.polish_llm = cfg.polish_llm.clone(),
        "denoise_mode" => g.denoise_mode = cfg.denoise_mode,
        "asr_correct" => g.asr_correct = cfg.asr_correct,
        "output_simplified" => g.output_simplified = cfg.output_simplified,
        "hide_toolbar" => g.hide_toolbar = cfg.hide_toolbar,
        _ => {}
    }
}

fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_history(ids: Vec<i64>) -> Result<usize, String> {
    octopus_infra::db::delete_transcriptions(&ids).map_err(|e| e.to_string())
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
        apply_config_value(&mut cfg, "segment_duration", &json!(10.0)).unwrap();
        assert_eq!(cfg.segment_duration, 10.0);
    }

    #[test]
    fn apply_config_invalid_f64_zero() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(0.0)).is_err());
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(-1.0)).is_err());
    }

    #[test]
    fn apply_config_pause_polish_threshold_must_ge_500() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(499.0)).is_err());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(500.0)).is_ok());
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
        apply_config_value(&mut cfg, "shortcut", &json!("Ctrl+Alt+Z")).unwrap();
        assert_eq!(cfg.shortcut, "Ctrl+Alt+Z");
        apply_config_value(&mut cfg, "microphone", &json!("External Mic")).unwrap();
        assert_eq!(cfg.microphone, "External Mic");
    }
}
