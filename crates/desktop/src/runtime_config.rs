//! 工具栏运行时可变配置：asr_engine + polish_mode 的共享镜像 + config.yaml 写回 + Tauri 命令。
//!
//! 与 OnceLock 缓存的 AppConfig 关系：AppConfig 是启动只读快照；RuntimeConfig 是这两个字段的
//! 可变运行时镜像。命令写 RuntimeConfig（即时生效）+ 写 config.yaml（重启生效）。

use serde::Serialize;
use std::sync::{Arc, RwLock};
use tauri::State;

use crate::config::PolishMode;

/// 运行时可变的两个配置字段。
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
}

impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
        }
    }
}

/// 挂 tauri::State 的共享句柄。
pub type SharedRuntimeConfig = Arc<RwLock<RuntimeConfig>>;

fn polish_mode_to_u8(m: PolishMode) -> u8 {
    match m {
        PolishMode::Disabled => 0,
        PolishMode::FinalOnly => 1,
        PolishMode::Intermediate => 2,
    }
}

fn u8_to_polish_mode(n: u8) -> Option<PolishMode> {
    match n {
        0 => Some(PolishMode::Disabled),
        1 => Some(PolishMode::FinalOnly),
        2 => Some(PolishMode::Intermediate),
        _ => None,
    }
}

fn category_str(c: octopus_asr::config::EngineCategory) -> &'static str {
    use octopus_asr::config::EngineCategory::*;
    match c {
        Whisper => "whisper",
        SenseVoice => "sensevoice",
        Paraformer => "paraformer",
        Qwen3Asr => "qwen3-asr",
        Zipformer => "zipformer",
    }
}

// ── config.yaml 写回 ──

/// 读当前 config.yaml → 覆盖 asr_engine → 序列化写回 ~/.octopus/config.yaml。
/// 写盘只影响下次重启读取；运行时生效走 RuntimeConfig。失败返回 Err（调用方 best-effort）。
pub fn persist_asr_engine(value: &str) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.asr_engine = value.to_string();
    write_config_yaml(&cfg)
}

/// 读当前 config.yaml → 覆盖 polish_mode → 序列化写回。
pub fn persist_polish_mode(value: u8) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.polish_mode = u8_to_polish_mode(value).ok_or_else(|| format!("polish_mode={} 非法", value))?;
    write_config_yaml(&cfg)
}

fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ── 命令返回 DTO ──

#[derive(Serialize)]
pub struct ToolbarState {
    pub asr_engine: String,
    pub polish_mode: u8,
}

#[derive(Serialize)]
pub struct EngineOption {
    pub name: String,
    pub category: String,
    pub current: bool,
}

// ── Tauri 命令 ──

#[tauri::command]
pub fn toolbar_state(rc: State<'_, SharedRuntimeConfig>) -> ToolbarState {
    let g = rc.read().unwrap();
    ToolbarState {
        asr_engine: g.asr_engine.clone(),
        polish_mode: polish_mode_to_u8(g.polish_mode),
    }
}

#[tauri::command]
pub fn list_asr_engines(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<EngineOption>, String> {
    let current_raw = rc.read().unwrap().asr_engine.clone();
    // 兜底：空 asr_engine → 当前生效 zipformer-small-ctc
    let current_effective = if current_raw.is_empty() {
        "zipformer-small-ctc".to_string()
    } else {
        current_raw
    };
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    Ok(engines
        .into_iter()
        .map(|e| EngineOption {
            current: e.name == current_effective,
            name: e.name,
            category: category_str(e.category).to_string(),
        })
        .collect())
}

#[tauri::command]
pub fn switch_asr_engine(name: String, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    // 校验：name 必须是 DB 已配置的引擎（不走兜底）
    let exists = octopus_asr::config::list_engines()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|e| e.name == name);
    if !exists {
        return Err(format!("引擎 '{}' 不存在，未切换", name));
    }
    {
        let mut g = rc.write().unwrap();
        g.asr_engine = name.clone();
    }
    if let Err(e) = persist_asr_engine(&name) {
        log::warn!(
            "写回 config.yaml 失败（asr_engine={}）：{} —— 本次仍生效，重启后回退",
            name,
            e
        );
    }
    Ok(())
}

#[tauri::command]
pub fn set_polish_mode(mode: u8, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let pm = u8_to_polish_mode(mode).ok_or_else(|| format!("polish_mode={} 非法（应为 0/1/2）", mode))?;
    {
        let mut g = rc.write().unwrap();
        g.polish_mode = pm;
    }
    if let Err(e) = persist_polish_mode(mode) {
        log::warn!(
            "写回 config.yaml 失败（polish_mode={}）：{} —— 本次仍生效，重启后回退",
            mode,
            e
        );
    }
    Ok(())
}

// ── 单测（纯逻辑，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_mirrors_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        cfg.asr_engine = "qwen3-asr-0.6B".into();
        cfg.polish_mode = PolishMode::Intermediate;
        let rc = RuntimeConfig::from_config(&cfg);
        assert_eq!(rc.asr_engine, "qwen3-asr-0.6B");
        assert_eq!(rc.polish_mode, PolishMode::Intermediate);
    }

    #[test]
    fn polish_mode_u8_round_trip() {
        for n in 0..=2u8 {
            let m = u8_to_polish_mode(n).unwrap();
            assert_eq!(polish_mode_to_u8(m), n);
        }
        assert!(u8_to_polish_mode(3).is_none());
        assert!(u8_to_polish_mode(99).is_none());
    }
}
