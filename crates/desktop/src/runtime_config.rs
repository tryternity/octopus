//! 工具栏运行时可变配置：asr_engine + polish_mode 的共享镜像 + config.yaml 写回 + Tauri 命令。
//!
//! 与 OnceLock 缓存的 AppConfig 关系：AppConfig 是启动只读快照；RuntimeConfig 是这两个字段的
//! 可变运行时镜像。命令写 RuntimeConfig（即时生效）+ 写 config.yaml（重启生效）。

use serde::Serialize;
use std::sync::{Arc, RwLock};
use tauri::State;

use crate::config::PolishMode;

/// 运行时可变的配置字段。
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
    pub polish_llm: String,
    pub denoise_mode: u8,
    pub asr_correct: bool,
    pub output_simplified: bool,
    pub hide_toolbar: bool,
}

impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
            polish_llm: cfg.polish_llm.clone(),
            denoise_mode: cfg.denoise_mode,
            asr_correct: cfg.asr_correct,
            output_simplified: cfg.output_simplified,
            hide_toolbar: cfg.hide_toolbar,
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

/// 统一显示文本（3-part）：
/// - is_local=true  → "本地:{category}:{name}"
/// - is_local=false → "{provider}:{category}:{name}"
///
/// 远程引擎保留 provider 前缀以区分 deepseek 直连（provider=deepseek）与 aliyun
/// 代管同名模型（provider=aliyun，category 同为 deepseek 系列）——provider 不同但
/// category 相同，前缀用 provider 才能让用户分辨供应商。本地引擎 provider 恒为
/// "local" 无信息量，故用 "本地" 前缀。
fn engine_label(is_local: bool, category: &str, provider: &str, name: &str) -> String {
    if is_local {
        format!("本地:{}:{}", category, name)
    } else {
        format!("{}:{}:{}", provider, category, name)
    }
}

/// ASR 兜底引擎名（固定首项，不依赖 DB 存在）。
pub(crate) const FALLBACK_ASR_ENGINE: &str = "zipformer-small-ctc";

/// 构造 ASR 选项列表（纯逻辑）：兜底固定第一，DB 同名去重，current 按 current_effective 标记。
/// current_effective 为空时视作兜底。current 可能为 3-part spec（"provider:category:name"）或裸名，
/// 统一用 parse_model_spec 提取裸 model_name 后比较。
fn build_asr_options(
    current_effective: &str,
    engines: Vec<octopus_asr::config::EngineInfo>,
) -> Vec<EngineOption> {
    let effective_bare = octopus_infra::db::parse_model_spec(current_effective).model_name();
    let effective = if effective_bare.is_empty() {
        FALLBACK_ASR_ENGINE
    } else {
        effective_bare
    };
    let mut options = Vec::with_capacity(engines.len() + 1);
    // 兜底固定第一
    options.push(EngineOption {
        name: FALLBACK_ASR_ENGINE.to_string(),
        category: "zipformer".to_string(),
        is_local: true,
        current: effective == FALLBACK_ASR_ENGINE,
        label: engine_label(true, "zipformer", "local", FALLBACK_ASR_ENGINE),
    });
    // DB 模型（跳过同名兜底，避免重复）
    for e in engines {
        if e.name == FALLBACK_ASR_ENGINE {
            continue;
        }
        let cat = octopus_asr::config::category_label(e.category);
        options.push(EngineOption {
            current: e.name == effective,
            name: e.name.clone(),
            category: cat.to_string(),
            is_local: e.is_local,
            label: engine_label(e.is_local, cat, &e.provider, &e.name),
        });
    }
    options
}

/// 校验引擎名可切换：兜底名恒允许（不依赖 DB），其余须在 engines 列表中。
fn validate_switch(name: &str, engines: &[octopus_asr::config::EngineInfo]) -> Result<(), String> {
    if name == FALLBACK_ASR_ENGINE {
        return Ok(());
    }
    if engines.iter().any(|e| e.name == name) {
        Ok(())
    } else {
        Err(format!("引擎 '{}' 不存在，未切换", name))
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

/// 读当前 config.yaml → 覆盖 polish_llm → 序列化写回 ~/.octopus/config.yaml。
pub fn persist_polish_llm(value: &str) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.polish_llm = value.to_string();
    write_config_yaml(&cfg)
}

/// 读当前 config.yaml → 覆盖 denoise_mode → 序列化写回。
pub fn persist_denoise_mode(value: u8) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.denoise_mode = value;
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
    /// 工具栏是否自动隐藏（true=hover 显隐，false=始终显示）。
    pub hide_toolbar: bool,
    /// 降噪模式：0=无，1=轻度，2=深度
    pub denoise_mode: u8,
    /// 当前 polish_llm 是否有效（裸名非空且在 DB 启用 LLM 列表中）。
    /// false → 无模型状态，前端 `#tool-llm` 图标置灰。DB 查询失败保守为 false。
    pub polish_llm_valid: bool,
}

#[derive(Serialize)]
pub struct EngineOption {
    pub name: String,
    pub category: String,
    pub current: bool,
    pub is_local: bool,
    pub label: String,
}

/// LLM 润色模型菜单项（与 EngineOption 同构，current 标记当前选中的 polish_llm）。
#[derive(Serialize)]
pub struct LlmOption {
    pub name: String,
    pub category: String,
    pub is_local: bool,
    pub current: bool,
    pub label: String,
}

/// 构造 LLM 选项列表（纯逻辑）：首项固定「不选择模型」（name 空 = polish_llm 置空），
/// 其后为 DB 启用的 LLM。current 按 polish_llm 裸 model_name 标记；current 无效（空 / 不在 DB）
/// 时首项「不选择模型」标 current（DB 找不到回退无模型）。current 可能为 3-part spec
/// （"provider:category:model_name"）或裸名，统一用 parse_model_spec 提取 model_name 后比较。
fn build_llm_options(current: &str, llms: Vec<octopus_infra::db::LlmModelInfo>) -> Vec<LlmOption> {
    let current_bare = octopus_infra::db::parse_model_spec(current).model_name();
    // current 有效 = 裸名非空且在 DB 列表中；否则回退无模型（首项 current）。
    let current_valid = !current_bare.is_empty() && llms.iter().any(|m| m.model_name == current_bare);
    let mut options = Vec::with_capacity(llms.len() + 1);
    // 首项：「不选择模型」（name 空）。current 无效时为选中态。
    options.push(LlmOption {
        name: String::new(),
        category: String::new(),
        is_local: false,
        current: !current_valid,
        label: "不选择模型".to_string(),
    });
    for m in llms {
        let label = engine_label(m.is_local, &m.category, &m.provider, &m.model_name);
        options.push(LlmOption {
            current: m.model_name == current_bare,
            label,
            name: m.model_name,
            category: m.category,
            is_local: m.is_local,
        });
    }
    options
}

/// 公开包装（供 settings_commands 调用）。
pub fn build_asr_options_public(
    current_effective: &str,
    engines: Vec<octopus_asr::config::EngineInfo>,
) -> Vec<EngineOption> {
    build_asr_options(current_effective, engines)
}

pub fn build_llm_options_public(
    current: &str,
    llms: Vec<octopus_infra::db::LlmModelInfo>,
) -> Vec<LlmOption> {
    build_llm_options(current, llms)
}

// ── Tauri 命令 ──

#[tauri::command]
pub fn toolbar_state(rc: State<'_, SharedRuntimeConfig>) -> ToolbarState {
    let g = rc.read().unwrap();
    // hide_toolbar 是启动只读配置（不参与运行时切换），从 AppConfig 缓存读
    let hide_toolbar = octopus_asr::config::load_app_config_cached().hide_toolbar;
    // polish_llm 有效 = 裸名非空且在 DB 启用 LLM 列表中（DB 查询失败保守为 false）。
    let polish_llm = g.polish_llm.clone();
    let polish_llm_valid = {
        let bare = octopus_infra::db::parse_model_spec(&polish_llm).model_name();
        !bare.is_empty()
            && octopus_infra::db::list_llm_models()
                .map(|llms| llms.iter().any(|m| m.model_name == bare))
                .unwrap_or(false)
    };
    ToolbarState {
        asr_engine: g.asr_engine.clone(),
        polish_mode: polish_mode_to_u8(g.polish_mode),
        hide_toolbar,
        denoise_mode: g.denoise_mode,
        polish_llm_valid,
    }
}

#[tauri::command]
pub fn list_asr_engines(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<EngineOption>, String> {
    let current_raw = rc.read().unwrap().asr_engine.clone();
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    Ok(build_asr_options(&current_raw, engines))
}

/// 切换 ASR 引擎：先校验 DB 存在（或兜底），再构造 spec（"PREFIX:NAME"）写 RuntimeConfig（即时）+ config.yaml（持久）。
#[tauri::command]
pub fn switch_asr_engine(
    name: String,
    rc: State<'_, SharedRuntimeConfig>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    validate_switch(&name, &engines)?;
    // 构造 3-part spec "{provider}:{category}:{model_name}"：
    // 兜底固定 local:zipformer:NAME；其余查 DB 取 provider/category。
    let spec = if name == FALLBACK_ASR_ENGINE {
        format!("local:zipformer:{}", name)
    } else {
        let engine = engines.iter().find(|e| e.name == name)
            .ok_or_else(|| format!("引擎 '{}' 不存在，未切换", name))?;
        format!("{}:{}:{}", engine.provider, octopus_asr::config::category_label(engine.category), name)
    };
    {
        let mut g = rc.write().unwrap();
        g.asr_engine = spec.clone();
    }
    let engine_mode = match octopus_infra::config::load_config() {
        Ok(cfg) => cfg.engine_mode,
        Err(_) => "embedded".to_string(),
    };
    crate::tray::update_tray_engine_label(&app_handle, &name, &engine_mode);

    if let Err(e) = persist_asr_engine(&spec) {
        log::warn!(
            "写回 config.yaml 失败（asr_engine={}）：{} —— 本次仍生效，重启后回退",
            spec,
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

/// 切换降噪模式（0=无，1=轻度，2=深度）。写 RuntimeConfig（即时）+ config.yaml（持久）。
#[tauri::command]
pub fn set_denoise_mode(mode: u8, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    if mode > 2 {
        return Err(format!("denoise_mode={} 非法（应为 0/1/2）", mode));
    }
    {
        let mut g = rc.write().unwrap();
        g.denoise_mode = mode;
    }
    if let Err(e) = persist_denoise_mode(mode) {
        log::warn!(
            "写回 config.yaml 失败（denoise_mode={}）：{} —— 本次仍生效，重启后回退",
            mode,
            e
        );
    }
    Ok(())
}

/// 列出所有启用的 LLM 润色模型，并标记当前 polish_llm。
#[tauri::command]
pub fn list_llm_models(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<LlmOption>, String> {
    let current = rc.read().unwrap().polish_llm.clone();
    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    Ok(build_llm_options(&current, llms))
}

/// 切换润色模型：先校验 DB 存在，再构造 spec（"PREFIX:NAME"）写 RuntimeConfig（即时）+ config.yaml（持久）。
/// 空 name = 选择「不选择模型」：polish_llm 置空，不查 DB（无模型 = 不润色）。
///
/// 同步通过 `coordinator.update_runtime()` 立即生效——用户可在录音中切换，
/// 下次点「立即润色」或停顿润色就用新模型。
#[tauri::command]
pub fn switch_polish_llm(
    name: String,
    rc: State<'_, SharedRuntimeConfig>,
    coordinator: State<'_, crate::coordinator::Coordinator>,
) -> Result<(), String> {
    let spec = if name.is_empty() {
        // 「不选择模型」：polish_llm 置空
        String::new()
    } else {
        let model = octopus_infra::db::list_llm_models()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.model_name == name)
            .ok_or_else(|| format!("润色模型 '{}' 不存在，未切换", name))?;
        // 构造 3-part spec "{provider}:{category}:{model_name}"
        format!("{}:{}:{}", model.provider, model.category, model.model_name)
    };
    {
        let mut g = rc.write().unwrap();
        g.polish_llm = spec.clone();
    }
    if let Err(e) = persist_polish_llm(&spec) {
        log::warn!(
            "写回 config.yaml 失败（polish_llm={}）：{} —— 本次仍生效，重启后回退",
            spec,
            e
        );
    }
    // 立即同步到 coordinator 的 config 快照（无需 Toggle）
    coordinator.update_runtime();
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
        cfg.asr_correct = true;
        cfg.output_simplified = false;
        cfg.hide_toolbar = false;
        let rc = RuntimeConfig::from_config(&cfg);
        assert_eq!(rc.asr_engine, "qwen3-asr-0.6B");
        assert_eq!(rc.polish_mode, PolishMode::Intermediate);
        assert!(rc.asr_correct);
        assert!(!rc.output_simplified);
        assert!(!rc.hide_toolbar);
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

    #[test]
    fn build_asr_options_injects_fallback_first_and_dedups() {
        use octopus_asr::config::{EngineCategory, EngineInfo};
        // 场景 1：DB 无兜底 → 注入到首位
        let engines = vec![
            EngineInfo { name: "whisper-small".into(), provider: "bigmodel".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
        ];
        let opts = build_asr_options("whisper-small", engines);
        assert_eq!(opts[0].name, "zipformer-small-ctc");
        assert_eq!(opts[0].label, "本地:zipformer:zipformer-small-ctc");
        assert!(opts[0].is_local);
        assert!(!opts[0].current, "current=whisper-small，兜底非当前");
        assert_eq!(opts[1].name, "whisper-small");
        assert!(opts[1].current);
        // 远程引擎 label = "{provider}:{category}:{name}"（保留 provider 区分供应商）
        assert_eq!(opts[1].label, "bigmodel:whisper:whisper-small");

        // 场景 2：current 为空 → 兜底为当前
        let opts2 = build_asr_options("", vec![]);
        assert_eq!(opts2.len(), 1);
        assert_eq!(opts2[0].name, "zipformer-small-ctc");
        assert!(opts2[0].current, "空 asr_engine → 兜底当前");

        // 场景 3：DB 已含兜底 → 去重（只一个 zipformer-small-ctc，且在首位）
        let engines3 = vec![
            EngineInfo { name: "zipformer-small-ctc".into(), provider: "local".into(), category: EngineCategory::Zipformer, is_local: true, description: String::new() },
            EngineInfo { name: "whisper-small".into(), provider: "local".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
        ];
        let opts3 = build_asr_options("zipformer-small-ctc", engines3);
        assert_eq!(
            opts3.iter().filter(|o| o.name == "zipformer-small-ctc").count(),
            1,
            "DB 已含兜底时去重"
        );
        assert_eq!(opts3[0].name, "zipformer-small-ctc");
        assert!(opts3[0].current);
    }

    #[test]
    fn build_asr_options_current_in_spec_format() {
        // asr_engine 存为 3-part spec 时，build_asr_options 应正确提取裸名标记 current
        use octopus_asr::config::{EngineCategory, EngineInfo};
        let mk = |name: &str, cat: EngineCategory, is_local: bool| EngineInfo {
            name: name.into(), provider: "local".into(), category: cat, is_local, description: String::new(),
        };
        // local spec 格式
        let opts = build_asr_options("local:zipformer:zipformer-small-ctc", vec![
            mk("zipformer-small-ctc", EngineCategory::Zipformer, true),
            mk("whisper-small", EngineCategory::Whisper, false),
        ]);
        assert!(opts[0].current, "local: 3-part spec 应正确标记 current");

        // aliyun spec 格式（云端）
        let opts2 = build_asr_options("aliyun:Fun-ASR:fun-asr-x", vec![
            mk("zipformer-small-ctc", EngineCategory::Zipformer, true),
            mk("whisper-small", EngineCategory::Whisper, false),
            EngineInfo { name: "fun-asr-x".into(), provider: "aliyun".into(), category: EngineCategory::Aliyun, is_local: false, description: String::new() },
        ]);
        // opts2 = [注入兜底, whisper-small, fun-asr-x]
        assert_eq!(opts2.len(), 3);
        assert_eq!(opts2[2].name, "fun-asr-x");
        assert!(opts2[2].current, "aliyun: 3-part spec 应正确标记 current");
    }

    #[test]
    fn build_llm_options_marks_current_and_labels() {
        use octopus_infra::db::LlmModelInfo;
        let llms = vec![
            LlmModelInfo { model_name: "glm-4-flashx".into(), provider: "bigmodel".into(), category: "glm".into(), is_local: false },
            LlmModelInfo { model_name: "ollama-local".into(), provider: "ollama".into(), category: "qwen".into(), is_local: true },
        ];
        let opts = build_llm_options("glm-4-flashx", llms);
        assert_eq!(opts.len(), 3);
        // opts[0] = 首项「不选择模型」，current 有效时非 current
        assert_eq!(opts[0].label, "不选择模型");
        assert_eq!(opts[0].name, "");
        assert!(!opts[0].current, "current=glm 有效 → 无模型项非 current");
        // opts[1] = glm current；远程 label = "{provider}:{category}:{name}"
        assert!(opts[1].current);
        assert_eq!(opts[1].label, "bigmodel:glm:glm-4-flashx");
        // opts[2] = ollama；本地 label = "本地:{category}:{name}"
        assert!(!opts[2].current);
        assert_eq!(opts[2].label, "本地:qwen:ollama-local");
    }

    #[test]
    fn build_llm_options_current_in_spec_format() {
        // polish_llm 存为 3-part spec 时，build_llm_options 应正确提取 model_name 标记 current
        use octopus_infra::db::LlmModelInfo;
        let llms = vec![
            LlmModelInfo { model_name: "glm-4-flashx".into(), provider: "bigmodel".into(), category: "glm".into(), is_local: false },
            LlmModelInfo { model_name: "ollama-local".into(), provider: "ollama".into(), category: "qwen".into(), is_local: true },
        ];
        // 3-part spec 格式
        let opts = build_llm_options("bigmodel:glm:glm-4-flashx", llms.clone());
        assert!(!opts[0].current, "无模型项非 current");
        assert!(opts[1].current, "3-part spec 应正确标记 current（glm）");

        // ollama 本地 3-part spec
        let opts2 = build_llm_options("ollama:qwen:ollama-local", llms);
        assert!(opts2[2].current, "3-part spec 应正确标记 current（ollama）");
    }

    #[test]
    fn build_llm_options_none_current_when_polish_llm_empty_or_not_in_db() {
        // 需求：polish_llm 空 / 裸名不在 DB / spec 格式不命中 → 首项「无模型」标 current（DB 回退）。
        use octopus_infra::db::LlmModelInfo;
        let llms = vec![
            LlmModelInfo { model_name: "glm-4-flashx".into(), provider: "bigmodel".into(), category: "glm".into(), is_local: false },
        ];
        // current 空 → 无模型 current
        let opts = build_llm_options("", llms.clone());
        assert!(opts[0].current, "空 polish_llm → 无模型 current");
        assert_eq!(opts[0].name, "");
        assert!(!opts[1].current);

        // current 裸名不在 DB → 无模型 current
        let opts2 = build_llm_options("ghost-model", llms.clone());
        assert!(opts2[0].current, "polish_llm 不在 DB → 无模型 current");
        assert!(!opts2[1].current);

        // current 3-part spec 但 model_name 不命中 DB → 无模型 current
        let opts3 = build_llm_options("bigmodel:glm:ghost-model", llms);
        assert!(opts3[0].current, "3-part spec 但不命中 DB → 无模型 current");
    }

    #[test]
    fn validate_switch_allows_fallback_even_when_absent() {
        use octopus_asr::config::{EngineCategory, EngineInfo};
        let engines = vec![
            EngineInfo { name: "whisper-small".into(), provider: "local".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
        ];
        // 兜底名即使不在 engines 也允许
        assert!(validate_switch("zipformer-small-ctc", &engines).is_ok());
        // 在列表中的允许
        assert!(validate_switch("whisper-small", &engines).is_ok());
        // 不在列表且非兜底 → 拒绝
        assert!(validate_switch("nonexistent", &engines).is_err());
    }
}
