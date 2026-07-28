//! 工具栏运行时可变配置：SharedRuntimeConfig = Arc<RwLock<AppConfig>>（完整配置唯一真相源）
//! + config.yaml 写回 + Tauri 命令。命令直接读写共享 AppConfig（即时生效）+ `persist_*`
//!   写 config.yaml（重启生效）。取代旧 RuntimeConfig 部分镜像——详见下方 type 定义注释。

use serde::Serialize;
use std::sync::Arc;
use parking_lot::RwLock;
use tauri::State;

use crate::config::PolishMode;

/// 运行时可变的完整应用配置——唯一真相源。
/// 启动时从 config.yaml 加载，set_config / switch_* 命令直接修改此结构；
/// toolbar_state / coordinator 都从这里读（coordinator clone 快照）。
/// 取代旧 RuntimeConfig 部分镜像，消除字段同步遗漏 bug。
pub type SharedRuntimeConfig = Arc<RwLock<octopus_infra::config::AppConfig>>;

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
/// - source_type != 2（builtin/local）→ "本地:{category}:{name}"
/// - source_type == 2（cloud）        → "{provider}:{category}:{name}"
///
/// 远程引擎保留 provider 前缀以区分 deepseek 直连（provider=deepseek）与 aliyun
/// 代管同名模型（provider=aliyun，category 同为 deepseek 系列）——provider 不同但
/// category 相同，前缀用 provider 才能让用户分辨供应商。本地引擎 provider 恒为
/// "local" 无信息量，故用 "本地" 前缀。
fn engine_label(source_type: i64, category: &str, provider: &str, name: &str) -> String {
    if source_type == 2 {
        format!("{}:{}:{}", provider, category, name)
    } else {
        format!("本地:{}:{}", category, name)
    }
}

/// ASR 兜底引擎名（固定首项，不依赖 DB 存在）。
pub(crate) const FALLBACK_ASR_ENGINE: &str = "zipformer-small";

/// API Key 脱敏：显示前 4 位 + ****** + 后 4 位（长度 <= 8 时全掩码）。
fn mask_key(key: &str) -> String {
    if key.is_empty() { return String::new(); }
    if key.len() <= 8 { return "********".to_string(); }
    let chars: Vec<char> = key.chars().collect();
    let n = chars.len();
    format!("{}********{}", chars[..4].iter().collect::<String>(), chars[n-4..].iter().collect::<String>())
}

/// 构造 ASR 选项列表（纯逻辑）：兜底固定第一，DB 同名去重，current 按 current_effective 标记。
/// current_effective 为空时视作兜底。current 可能为 3-part spec（"provider:category:name"）或裸名，
/// 统一用 parse_model_spec 提取裸 model_name 后比较。
/// 构造 ASR 选项列表（纯逻辑）：兜底固定第一，DB 同名去重，current 直接用
/// `EngineInfo.is_enabled` 字段（来自 DB models.is_enabled）。
///
/// Task 2 后：不再接收外部 `current_effective` spec 字符串做 name 匹配——EngineInfo
/// 自带 is_enabled，直接用它标 current。兜底 zipformer-small 的 current 判定：
/// DB 无任何 ASR is_enabled=1 时，fallback 引擎视为当前（与 resolve_active_engine
/// 的 ASR 兜底语义对称）。
fn build_asr_options(engines: Vec<octopus_asr_local::config::EngineInfo>) -> Vec<EngineOption> {
    // 是否有 DB 激活的 ASR 模型（is_enabled=1）
    let has_active = engines.iter().any(|e| e.is_enabled);

    let mut options = Vec::with_capacity(engines.len() + 1);
    // 兜底固定第一。current：DB 无激活时 fallback 引擎视为当前。
    // source_type=0（builtin）—— 兜底引擎是内置分类
    options.push(EngineOption {
        id: 0,
        name: FALLBACK_ASR_ENGINE.to_string(),
        provider: "local".to_string(),
        category: "zipformer".to_string(),
        source_type: 0,
        current: !has_active,
        label: engine_label(0, "zipformer", "local", FALLBACK_ASR_ENGINE),
        source: String::new(),
        secret_key: String::new(),
        is_streaming: true,
        is_thinking: false,
    });

    // DB 模型（跳过同名兜底，避免重复）
    for e in engines {
        if e.name == FALLBACK_ASR_ENGINE {
            continue;
        }
        let cat = octopus_asr_local::config::category_label(e.category);
        options.push(EngineOption {
            id: e.id,
            current: e.is_enabled,
            name: e.name.clone(),
            provider: e.provider.clone(),
            category: cat.to_string(),
            source_type: e.source_type,
            label: engine_label(e.source_type, cat, &e.provider, &e.name),
            source: e.source.clone(),
            secret_key: mask_key(&e.secret_key),
            is_streaming: e.is_streaming,
            is_thinking: e.is_thinking,
        });
    }
    options
}

// ── 配置持久化（DB app_config 表）──

// 注：persist_asr_engine / persist_polish_llm 已移除（Task 2 后激活态存 DB models.is_enabled，
// 不再写 app_config）。polish_mode / denoise_mode 仍在 app_config。

pub fn persist_polish_mode(value: u8) -> Result<(), String> {
    octopus_infra::db::save_config_key("polish_mode", &value.to_string()).map_err(|e| e.to_string())
}

pub fn persist_denoise_mode(value: u8) -> Result<(), String> {
    octopus_infra::db::save_config_key("denoise_mode", &value.to_string()).map_err(|e| e.to_string())
}

// ── 命令返回 DTO ──

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
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
    /// 结果展示区编辑 toggle 快捷键（Tauri Accelerator 字符串，默认 "Cmd+Enter"，进入/保存同键）。
    /// 仅结果窗聚焦时生效。
    pub edit_shortcut: String,
    /// 翻译自动档（记忆档位）："manual" / "4s" / "8s" / "12s"。DB 无值时默认 "manual"。
    pub translate_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineOption {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub category: String,
    pub current: bool,
    /// 模型来源: 0=builtin 1=local 2=cloud（详见 infra::db::ModelEntry）。
    pub source_type: i64,
    pub label: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

/// LLM 润色模型菜单项（与 EngineOption 同构，current 标记当前选中的 polish_llm）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmOption {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub category: String,
    /// 模型来源: 0=builtin 1=local 2=cloud。
    pub source_type: i64,
    pub current: bool,
    pub label: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

/// OCR 模型菜单项（与 LlmOption 同构，current 标记当前选中的 ocr_model）。
/// 与 LLM 的区别：不做「不选择模型」首项——OCR 必须有一个模型。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrOption {
    pub name: String,
    pub provider: String,
    pub label: String,
    pub current: bool,
    /// 模型来源: 0=builtin 1=local 2=cloud。
    pub source_type: i64,
}

/// 构造 LLM 选项列表（纯逻辑）：首项固定「不选择模型」（name 空 = 无激活），
/// 其后为 DB 可用的 LLM。current 直接用 DB 行的 `is_enabled` 字段（每域仅 1 个=1，
/// 无激活时首项「不选择模型」标 current）。
///
/// Task 2 后：不再接收外部 `current` spec 字符串做 name 匹配——DB 行自带 is_enabled，
/// 直接用它标 current。避免同 model_name 不同 provider 都标 current 的 bug
/// （如 aliyun:deepseek-v4-flash 激活时 deepseek:deepseek-v4-flash 不应被标 current）。
fn build_llm_options(llms: Vec<octopus_infra::db::LlmModelInfo>) -> Vec<LlmOption> {
    // 是否有激活模型（is_enabled=1）——无激活时首项「不选择模型」标 current
    let has_active = llms.iter().any(|m| m.is_enabled);
    let mut options = Vec::with_capacity(llms.len() + 1);
    // 首项：「不选择模型」（name 空）。无激活时为选中态。source_type=2（cloud 占位，非本地）
    options.push(LlmOption {
        id: 0,
        name: String::new(),
        provider: String::new(),
        category: String::new(),
        source_type: 2,
        current: !has_active,
        label: "不选择模型".to_string(),
        source: String::new(),
        secret_key: String::new(),
        is_streaming: false,
        is_thinking: false,
    });
    for m in llms {
        let label = engine_label(m.source_type, &m.category, &m.provider, &m.model_name);
        options.push(LlmOption {
            id: m.id,
            current: m.is_enabled,
            label,
            name: m.model_name.clone(),
            provider: m.provider.clone(),
            category: m.category.clone(),
            source_type: m.source_type,
            source: m.source.clone(),
            secret_key: mask_key(&m.secret_key),
            is_streaming: m.is_streaming,
            is_thinking: m.is_thinking,
        });
    }
    options
}

/// 公开包装（供 settings_commands 调用）。
pub fn build_asr_options_public(
    engines: Vec<octopus_asr_local::config::EngineInfo>,
) -> Vec<EngineOption> {
    build_asr_options(engines)
}

pub fn build_llm_options_public(llms: Vec<octopus_infra::db::LlmModelInfo>) -> Vec<LlmOption> {
    build_llm_options(llms)
}

/// 构造 OCR 选项列表（纯逻辑）：DB 可用的 OCR 模型，current 直接用 DB 行的 `is_enabled`。
/// 不做「不选择」首项（OCR 必须有一个）。label 优先 description，空则 model_name。
///
/// Task 2 后：不再接收外部 `current` 字符串做 name 匹配——DB 行自带 is_enabled。
fn build_ocr_options(ocrs: Vec<octopus_infra::db::OcrModelInfo>) -> Vec<OcrOption> {
    ocrs.into_iter()
        .map(|m| OcrOption {
            current: m.is_enabled,
            label: if m.description.is_empty() {
                m.model_name.clone()
            } else {
                m.description
            },
            name: m.model_name,
            provider: "local".to_string(),
            source_type: m.source_type,
        })
        .collect()
}

/// 公开包装（供 settings_commands 调用）。
pub fn build_ocr_options_public(ocrs: Vec<octopus_infra::db::OcrModelInfo>) -> Vec<OcrOption> {
    build_ocr_options(ocrs)
}

// ── Tauri 命令 ──

#[tauri::command]
pub fn toolbar_state(rc: State<'_, SharedRuntimeConfig>) -> ToolbarState {
    let g = rc.read();
    // hide_toolbar / edit_shortcut 等所有字段均从共享 AppConfig 读——set_config 写镜像后立即生效。
    let edit_shortcut = g.edit_shortcut.clone();
    // Task 2 后：激活引擎从 ACTIVE_ENGINES 内存缓存取（DB is_enabled=1 为真）。
    // asr_engine 字段保留供前端兼容，值改为激活引擎的裸 model_name。
    let asr_engine = octopus_asr_local::config::resolve_active_engine("asr")
        .map(|r| r.name)
        .unwrap_or_default();
    // polish_llm_valid = LLM 域有激活模型（resolve_active_engine 命中）。
    let polish_llm_valid = octopus_asr_local::config::resolve_active_engine("llm").is_ok();
    let translate_mode = resolve_translate_mode(
        octopus_infra::db::load_config_key("translate_mode").ok().flatten()
    );
    ToolbarState {
        asr_engine,
        polish_mode: polish_mode_to_u8(g.polish_mode),
        hide_toolbar: g.hide_toolbar,
        denoise_mode: g.denoise_mode,
        polish_llm_valid,
        edit_shortcut,
        translate_mode,
    }
}

#[tauri::command]
pub fn list_asr_engines(_rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<EngineOption>, String> {
    // Task 2 后：current 直接用 DB 行的 is_enabled（build_asr_options 内部处理）。
    let engines = octopus_asr_local::config::list_engines_from_db().map_err(|e| e.to_string())?;
    Ok(build_asr_options(engines))
}

/// 后台预热本地 ASR 引擎（审查 三2）。switch_active_model 切 ASR 引擎后调用——
/// 否则首次 transcribe 在 spawn_blocking 懒加载模型（反序列化 + ONNX session 创建，1~数秒卡顿）。
/// 仅 embedded 非 cloud 引擎：engine_mode≠embedded 不走本地模型；cloud（aliyun）switch_model 会 bail。
pub fn preheat_local_engine(
    engine_manager: std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
    engine_mode: &str,
) {
    if engine_mode != "embedded" {
        return;
    }
    let resolved = match octopus_asr_local::config::resolve_active_engine("asr") {
        Ok(r) => r,
        Err(_) => return,
    };
    #[cfg(feature = "cloud")]
    if resolved.as_engine_category() == Some(octopus_asr_local::config::EngineCategory::Aliyun) {
        return;
    }
    let name = resolved.name.clone();
    std::thread::spawn(move || match engine_manager.switch_model(&name) {
        Ok(_) => log::info!("Preheated ASR model '{}' (runtime switch)", name),
        Err(e) => log::warn!("Preheat '{}' failed（首次录音懒加载重试）: {}", name, e),
    });
}

/// 统一激活模型（4 域）：DB switch_active_model + 重载 ACTIVE_ENGINES 该域缓存。
///
/// 取代原 switch_asr_engine / switch_polish_llm / set_config(ocr_model/translate_engine)
/// 4 个分散命令——4 域统一。ASR 域额外刷新 tray 标签 + 后台预热本地引擎。
#[tauri::command]
pub fn switch_active_model(
    domain: String,
    id: i64,
    app_handle: tauri::AppHandle,
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
) -> Result<(), String> {
    octopus_infra::db::switch_active_model(&domain, id).map_err(|e| e.to_string())?;
    // 重载该域激活缓存（reload_active_engine 清槽 + 重 load）。
    // 失败仅告警（DB 已切换成功，缓存下次 resolve 会 fallback 重 load）。
    //
    // **并发不变量（code review Issue #2）**：DB 是真相源，reload 从 DB 读回（而非
    // 从入参 id 构造缓存）。两个 switch 并发时：DB UPDATE 经 with_db 的
    // ReentrantMutex 串行化（last-writer-wins）；两条 reload 都从 DB 读到最新值，
    // 缓存最终 = DB 最终。**不要**优化成「按入参 id 直接写缓存」——那会引入真正的
    // race（thread A 的 reload 可能写到 thread B 的 id 之前）。
    if let Err(e) = octopus_asr_local::config::reload_active_engine(&domain) {
        log::warn!("switch_active_model: reload_active_engine('{}') 失败：{}", domain, e);
    }
    // ASR 域额外：刷新 tray + 后台预热 + emit 事件
    if domain == "asr" {
        let engine_mode = octopus_infra::config::load_config()
            .map(|c| c.engine_mode)
            .unwrap_or_else(|_| "embedded".to_string());
        crate::tray::update_tray_engine_label(&app_handle, "", &engine_mode);
        preheat_local_engine(engine_manager.inner().clone(), &engine_mode);
        let _ = tauri::Emitter::emit(&app_handle, "config-changed", ());
    }
    Ok(())
}

#[tauri::command]
pub fn set_polish_mode(mode: u8, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let pm = u8_to_polish_mode(mode).ok_or_else(|| format!("polish_mode={} 非法（应为 0/1/2）", mode))?;
    {
        let mut g = rc.write();
        g.polish_mode = pm;
    }
    if let Err(e) = persist_polish_mode(mode) {
        log::warn!(
            "写回 DB 失败（polish_mode={}）：{} —— 本次仍生效，重启后回退",
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
        let mut g = rc.write();
        g.denoise_mode = mode;
    }
    if let Err(e) = persist_denoise_mode(mode) {
        log::warn!(
            "写回 DB 失败（denoise_mode={}）：{} —— ASR 缓存以 DB 为准，本次可能不生效",
            mode,
            e
        );
    }
    // 刷新 ASR 侧 AppConfig 缓存（审查 二1）：audio 每帧经 load_app_config_cached 读 denoise_mode，
    // 不 reload 则改了也不生效（需重启）。reload 以 DB 为真——persist 成功即本次生效。
    octopus_asr_local::config::reload_app_config();
    Ok(())
}

/// 校验翻译档位字符串（manual/4s/8s/12s）。合法返回 true。
fn validate_translate_mode(mode: &str) -> bool {
    matches!(mode, "manual" | "4s" | "8s" | "12s")
}

/// 从 DB 读取的 translate_mode（Option<String>）解析为合法值——非法或 None 时默认 "manual"。
fn resolve_translate_mode(raw: Option<String>) -> String {
    raw.filter(|s| validate_translate_mode(s))
        .unwrap_or_else(|| "manual".to_string())
}

/// 设置翻译自动档位（manual/4s/8s/12s）。纯持久化到 DB，翻译节流逻辑在前端。
#[tauri::command]
pub fn set_translate_mode(mode: String) -> Result<(), String> {
    if !validate_translate_mode(&mode) {
        return Err(format!("translate_mode='{}' 非法（应为 manual/4s/8s/12s）", mode));
    }
    octopus_infra::db::save_config_key("translate_mode", &mode).map_err(|e| e.to_string())
}

/// 列出所有可用的 LLM 润色模型，并标记当前激活的（DB is_enabled=1）。
#[tauri::command]
pub fn list_llm_models(_rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<LlmOption>, String> {
    // Task 2 后：current 直接用 DB 行的 is_enabled（build_llm_options 内部处理）。
    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    Ok(build_llm_options(llms))
}

// ── 单测（纯逻辑，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;

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
        use octopus_asr_local::config::EngineCategory;
        // 场景 1：whisper-small 激活（is_enabled=true）→ 兜底非 current，whisper-small current
        let engines = vec![
            mk_engine("whisper-small", "bigmodel", EngineCategory::Whisper, 2, true),
        ];
        let opts = build_asr_options(engines);
        assert_eq!(opts[0].name, "zipformer-small");
        assert_eq!(opts[0].label, "本地:zipformer:zipformer-small");
        assert_eq!(opts[0].source_type, 0, "兜底引擎 source_type 应为 0 (builtin)");
        assert!(!opts[0].current, "有激活模型 → 兜底非当前");
        assert_eq!(opts[1].name, "whisper-small");
        assert!(opts[1].current, "whisper-small is_enabled=true → current");
        // 远程引擎 label = "{provider}:{category}:{name}"（保留 provider 区分供应商）
        assert_eq!(opts[1].label, "bigmodel:whisper:whisper-small");

        // 场景 2：无激活模型（全 is_enabled=false）→ 兜底为当前
        let opts2 = build_asr_options(vec![]);
        assert_eq!(opts2.len(), 1);
        assert_eq!(opts2[0].name, "zipformer-small");
        assert!(opts2[0].current, "无激活 → 兜底当前");

        // 场景 3：DB 已含兜底 → 去重（只一个 zipformer-small，且在首位）
        let engines3 = vec![
            mk_engine("zipformer-small", "local", EngineCategory::Zipformer, 1, false),
            mk_engine("whisper-small", "local", EngineCategory::Whisper, 2, true),
        ];
        let opts3 = build_asr_options(engines3);
        assert_eq!(
            opts3.iter().filter(|o| o.name == "zipformer-small").count(),
            1,
            "DB 已含兜底时去重"
        );
        assert_eq!(opts3[0].name, "zipformer-small");
        assert!(!opts3[0].current, "DB 有 whisper-small 激活，兜底非 current");
        assert!(opts3[1].current, "whisper-small 激活");
    }

    #[test]
    fn build_asr_options_uses_is_enabled_not_name_match() {
        // Task 2 后修复的核心 bug：同名不同 provider 的 ASR 模型按 DB is_enabled 标 current，
        // 不再用 current spec 字符串按 name 匹配（避免同 name 都标 current）。
        use octopus_asr_local::config::EngineCategory;
        let engines = vec![
            mk_engine("deepseek-v4-flash", "aliyun", EngineCategory::Aliyun, 2, true),
            mk_engine("deepseek-v4-flash", "deepseek", EngineCategory::Aliyun, 2, false),
        ];
        let opts = build_asr_options(engines);
        let currents: Vec<_> = opts.iter().filter(|o| o.current).collect();
        assert_eq!(currents.len(), 1, "同 name 不同 provider 只应有一个 current（is_enabled 精确）");
        assert_eq!(currents[0].provider, "aliyun", "current 应是 is_enabled=true 的 aliyun 行");
    }

    #[test]
    fn build_llm_options_marks_current_and_labels() {
        
        let llms = vec![
            mk_llm("glm-4-flashx", "bigmodel", "glm", 2, true),
            mk_llm("ollama-local", "ollama", "qwen", 1, false),
        ];
        let opts = build_llm_options(llms);
        assert_eq!(opts.len(), 3);
        // opts[0] = 首项「不选择模型」，有激活时非 current
        assert_eq!(opts[0].label, "不选择模型");
        assert_eq!(opts[0].name, "");
        assert!(!opts[0].current, "有激活 → 无模型项非 current");
        // opts[1] = glm current（is_enabled=true）；远程 label = "{provider}:{category}:{name}"
        assert!(opts[1].current);
        assert_eq!(opts[1].label, "bigmodel:glm:glm-4-flashx");
        // opts[2] = ollama（is_enabled=false）；本地 label = "本地:{category}:{name}"
        assert!(!opts[2].current);
        assert_eq!(opts[2].label, "本地:qwen:ollama-local");
    }

    #[test]
    fn build_llm_options_none_current_when_no_active() {
        // 需求：无激活 LLM（全 is_enabled=false）→ 首项「无模型」标 current。
        
        let llms = vec![
            mk_llm("glm-4-flashx", "bigmodel", "glm", 2, false),
        ];
        let opts = build_llm_options(llms);
        assert!(opts[0].current, "无激活 → 无模型 current");
        assert_eq!(opts[0].name, "");
        assert!(!opts[1].current);
    }

    // ── translate_mode 校验 ──
    #[test]
    fn validate_translate_mode_accepts_known_values() {
        assert!(validate_translate_mode("manual"));
        assert!(validate_translate_mode("4s"));
        assert!(validate_translate_mode("8s"));
        assert!(validate_translate_mode("12s"));
    }

    #[test]
    fn validate_translate_mode_rejects_unknown() {
        // 已移除的档位
        assert!(!validate_translate_mode("15s"), "15s 已移除");
        // 非法值
        assert!(!validate_translate_mode(""));
        assert!(!validate_translate_mode("off"));
        assert!(!validate_translate_mode("3s"));
        assert!(!validate_translate_mode("16s"));
        assert!(!validate_translate_mode("auto"));
        assert!(!validate_translate_mode("0"));
    }

    #[test]
    fn resolve_translate_mode_defaults_to_manual() {
        assert_eq!(resolve_translate_mode(None), "manual");
        assert_eq!(resolve_translate_mode(Some(String::new())), "manual");
        assert_eq!(resolve_translate_mode(Some("garbage".into())), "manual");
        assert_eq!(resolve_translate_mode(Some("15s".into())), "manual", "已移除的 15s 应回退 manual");
    }

    #[test]
    fn resolve_translate_mode_preserves_valid() {
        assert_eq!(resolve_translate_mode(Some("manual".into())), "manual");
        assert_eq!(resolve_translate_mode(Some("4s".into())), "4s");
        assert_eq!(resolve_translate_mode(Some("8s".into())), "8s");
        assert_eq!(resolve_translate_mode(Some("12s".into())), "12s");
    }

    // ── Task 2 后：同名不同 provider 只标一个 current（DB is_enabled 精确）──

    /// 同 model_name 不同 provider，只 is_enabled=true 的标 current。
    /// 这正是用户报告的 bug 场景（aliyun:deepseek-v4-flash 激活时 deepseek:deepseek-v4-flash
    /// 不应被标 current）。
    #[test]
    fn build_llm_options_is_enabled_precise_current() {
        
        let llms = vec![
            mk_llm("deepseek-v4-flash", "aliyun", "deepseek", 2, true),
            mk_llm("deepseek-v4-flash", "deepseek", "deepseek", 2, false),
        ];
        let opts = build_llm_options(llms);
        let currents: Vec<_> = opts.iter().filter(|o| o.current).collect();
        assert_eq!(currents.len(), 1, "同名不同 provider 只应有一个 current");
        assert_eq!(currents[0].provider, "aliyun", "current 应是 is_enabled=true 的 aliyun 行");
    }

    /// EngineOption 包含 provider 字段。
    #[test]
    fn engine_option_has_provider_field() {
        use octopus_asr_local::config::EngineCategory;
        let engines = vec![
            mk_engine("whisper-small", "aliyun", EngineCategory::Whisper, 2, false),
        ];
        let opts = build_asr_options(engines);
        assert_eq!(opts[1].provider, "aliyun", "EngineOption 应包含 provider 字段");
    }

    /// LlmOption 包含 provider 字段。
    #[test]
    fn llm_option_has_provider_field() {
        
        let llms = vec![
            mk_llm("test", "bigmodel", "glm", 2, false),
        ];
        let opts = build_llm_options(llms);
        assert_eq!(opts[1].provider, "bigmodel", "LlmOption 应包含 provider 字段");
    }

    /// build_ocr_options 用 DB is_enabled 标 current（Task 2 后）。
    #[test]
    fn build_ocr_options_uses_is_enabled_for_current() {
        let ocrs = vec![
            octopus_infra::db::OcrModelInfo {
                model_name: "PP-OCRv6-small".into(), description: "v6".into(), source_type: 1, is_enabled: true,
            },
            octopus_infra::db::OcrModelInfo {
                model_name: "PP-OCRv5".into(), description: "v5".into(), source_type: 1, is_enabled: false,
            },
        ];
        let opts = build_ocr_options(ocrs);
        assert_eq!(opts.len(), 2);
        assert!(opts.iter().find(|o| o.name == "PP-OCRv6-small").unwrap().current);
        assert!(!opts.iter().find(|o| o.name == "PP-OCRv5").unwrap().current);
    }

    // ── 测试 helper：减少 EngineInfo / LlmModelInfo 字面量样板 ──

    /// 构造 EngineInfo（默认 id=0, source/secret_key 空, is_streaming=false, is_thinking=false）。
    fn mk_engine(
        name: &str,
        provider: &str,
        category: octopus_asr_local::config::EngineCategory,
        source_type: i64,
        is_enabled: bool,
    ) -> octopus_asr_local::config::EngineInfo {
        octopus_asr_local::config::EngineInfo {
            name: name.into(),
            provider: provider.into(),
            category,
            description: String::new(),
            source_type,
            id: 0,
            source: String::new(),
            secret_key: String::new(),
            is_streaming: false,
            is_thinking: false,
            is_enabled,
        }
    }

    /// 构造 LlmModelInfo（默认 id=0, source/secret_key 空, is_streaming=false, is_thinking=false）。
    fn mk_llm(
        model_name: &str,
        provider: &str,
        category: &str,
        source_type: i64,
        is_enabled: bool,
    ) -> octopus_infra::db::LlmModelInfo {
        octopus_infra::db::LlmModelInfo {
            model_name: model_name.into(),
            provider: provider.into(),
            category: category.into(),
            source_type,
            id: 0,
            source: String::new(),
            secret_key: String::new(),
            is_streaming: false,
            is_thinking: false,
            is_enabled,
        }
    }
}
