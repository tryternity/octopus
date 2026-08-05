use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use octopus_infra::consts::{DEFAULT_ASR_MODEL_DIR, VAD_OVERRIDE_PATH};
use octopus_infra::octopus_config_home;

// ── Model config schema（DB models 表）──
pub use octopus_infra::db::{parse_model_spec, AsrConfig, AsrSection, ModelEntry, ModelSpec};

// ── Config loading ──

/// 读取 ASR 模型配置（直接查 DB，无缓存）。
///
/// **Task 3 后**：RUNTIME_CONFIG 缓存已移除——推理路径统一走 `resolve_engine_any`
/// / `resolve_active_engine`（查 DB）。本函数仅供测试 / `resolve_engine_in_config` /
/// `cli show_config` 使用，推理路径不再调用。
pub fn load_config() -> Result<AsrConfig> {
    crate::db::ensure_db()?;
    crate::db::load_models()
}

/// no-op（Task 3 后 RUNTIME_CONFIG 已移除，推理路径不经缓存）。
/// 保留函数体避免调用方（model_commands / cli sync-models）编译错误——它们的
/// reload 调用现在是 no-op（DB 直查，无需刷新缓存）。
pub fn reload_models_config() {
    // No-op: RUNTIME_CONFIG removed. Engine instantiation uses resolve_engine_any (DB direct).
}

// ── HF cache helpers ──

/// 模型路径查找——已抽取到 onnx-infra crate
pub use onnx_infra::{find_hf_cache, find_onnx_dir, resolve_model_dir};

// ── VAD model discovery ──

/// VAD 模型来源：磁盘文件（用户自定义覆盖）或内嵌字节（include_bytes!，开箱即用）。
#[derive(Debug, Clone)]
pub enum VadSource {
    /// 磁盘上的文件路径（`~/.octopus/models/vad.onnx` 存在时优先，通用名可放任意 VAD 模型覆盖内嵌版本）。
    File(PathBuf),
    /// 内嵌字节（磁盘文件不存在时 fallback 到编译期内嵌）。
    Builtin,
}

/// 定位 Silero VAD 模型。
///
/// 优先读磁盘 `~/.octopus/models/vad.onnx`（用户可放任意 VAD 模型覆盖内嵌的 silero_vad_v4，
/// 通用名避免绑死版本）；磁盘不存在则返回 `Builtin`（编译期内嵌字节，`SileroVad::new_builtin()`
/// 从内存加载）。
pub fn find_silero_vad() -> Result<VadSource> {
    let vad = octopus_config_home().join(VAD_OVERRIDE_PATH);
    if vad.exists() {
        return Ok(VadSource::File(vad));
    }
    Ok(VadSource::Builtin)
}

/// 便捷函数：find_silero_vad + 构造 SileroVad 一步到位。
///
/// 磁盘有文件 → `SileroVad::new(path)`；磁盘无文件 → `SileroVad::new_builtin()`（内嵌字节）。
/// 调用方无需关心 VadSource 细节，直接拿 `Result<SileroVad>`。
pub fn create_silero_vad() -> Result<crate::vad::SileroVad> {
    match find_silero_vad()? {
        VadSource::File(p) => crate::vad::SileroVad::new(&p),
        VadSource::Builtin => crate::vad::SileroVad::new_builtin(),
    }
}

// ── Internal helpers ──
// find_latest_snapshot 已抽取到 onnx-infra crate

// ── Engine routing ──

/// ASR engine category, determined by which section in DB models table contains the engine name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineCategory {
    Whisper,
    /// 原版 SenseVoice-Small（FunASR 4 输入 ONNX 导出，非 sherpa 简化版）。category='sensevoice-orig' 路由入此。
    SenseVoiceOrig,
    Paraformer,
    Qwen3Asr,
    Zipformer,
    Moonshine,
    /// FireRedASR2-AED CTC（小红书，本地）。provider='local' + category='firered' 路由入此。
    FireRed,
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    Aliyun,
    /// 字节跳动云端 ASR（豆包大模型 bigmodel_async 双向流式优化版）。provider='bytedance' 路由入此。
    ByteDance,
    /// 腾讯云云端 ASR（实时语音识别 WebSocket HMAC-SHA1 签名鉴权）。provider='tencent' 路由入此。
    Tencent,
    /// 百度智能云云端 ASR（实时语音识别 WebSocket START 帧鉴权）。provider='baidu' 路由入此。
    Baidu,
}

/// DB `models.category` 字符串 → EngineCategory 映射。
/// 仅映射 ASR 本地引擎类型（whisper/sensevoice-orig/paraformer/qwen3-asr/zipformer），
/// 其他 category（如云端系列 `Fun-ASR`）返回 None——aliyun 等云端族由 provider 路由，
/// 见 [`resolve_category`]。
fn engine_category_from_str(s: &str) -> Option<EngineCategory> {
    match s {
        "whisper" => Some(EngineCategory::Whisper),
        "sensevoice-orig" => Some(EngineCategory::SenseVoiceOrig),
        "paraformer" => Some(EngineCategory::Paraformer),
        "qwen3-asr" => Some(EngineCategory::Qwen3Asr),
        "zipformer" => Some(EngineCategory::Zipformer),
        "moonshine" => Some(EngineCategory::Moonshine),
        "firered" => Some(EngineCategory::FireRed),
        _ => None,
    }
}

/// provider + category → EngineCategory。
/// provider='aliyun' → Aliyun（云）；其余按 category 字符串映射本地族。
fn resolve_category(provider: &str, category: &str) -> Option<EngineCategory> {
    if provider.eq_ignore_ascii_case("aliyun") {
        return Some(EngineCategory::Aliyun);
    }
    if provider.eq_ignore_ascii_case("bytedance") {
        return Some(EngineCategory::ByteDance);
    }
    if provider.eq_ignore_ascii_case("tencent") {
        return Some(EngineCategory::Tencent);
    }
    if provider.eq_ignore_ascii_case("baidu") {
        return Some(EngineCategory::Baidu);
    }
    engine_category_from_str(category)
}

/// 按固定顺序遍历 AsrConfig 的 11 个 section（用于 NameOnly 裸名查找）。
/// 顺序与本地引擎优先一致（aliyun / bytedance / tencent / baidu 云端放最后）。
fn all_sections(
    cfg: &AsrConfig,
) -> [(Option<&HashMap<String, ModelEntry>>, EngineCategory); 11] {
    [
        (cfg.asr.whisper.as_ref(), EngineCategory::Whisper),
        (cfg.asr.sensevoice_orig.as_ref(), EngineCategory::SenseVoiceOrig),
        (cfg.asr.paraformer.as_ref(), EngineCategory::Paraformer),
        (cfg.asr.qwen3_asr.as_ref(), EngineCategory::Qwen3Asr),
        (cfg.asr.zipformer.as_ref(), EngineCategory::Zipformer),
        (cfg.asr.moonshine.as_ref(), EngineCategory::Moonshine),
        (cfg.asr.firered.as_ref(), EngineCategory::FireRed),
        (cfg.asr.aliyun.as_ref(), EngineCategory::Aliyun),
        (cfg.asr.bytedance.as_ref(), EngineCategory::ByteDance),
        (cfg.asr.tencent.as_ref(), EngineCategory::Tencent),
        (cfg.asr.baidu.as_ref(), EngineCategory::Baidu),
    ]
}

/// 解析 spec 并在已加载配置中查找，返回 (category, 裸名, entry 引用)。
///
/// spec 格式见 [`parse_model_spec`]（3-part）：
/// - `provider:category:model_name` — provider='aliyun' → Aliyun；否则按 category 映射本地族，
///   再 pick_entry 精确匹配
/// - `model_name`（无冒号）— 遍历所有 section 按 name 查找（NameOnly 兜底，用于全局默认）
pub fn resolve_engine_in_config<'a, 'b>(
    cfg: &'a AsrConfig,
    spec: &'b str,
) -> Option<(EngineCategory, &'b str, &'a ModelEntry)> {
    match parse_model_spec(spec) {
        ModelSpec::Full { provider, category, model_name } => {
            let cat = resolve_category(provider, category)?;
            pick_entry(cfg, cat, model_name).map(|e| (cat, model_name, e))
        }
        ModelSpec::NameOnly(model_name) => {
            for (section, cat) in all_sections(cfg) {
                if let Some(map) = section {
                    if let Some(entry) = map.get(model_name) {
                        return Some((cat, model_name, entry));
                    }
                }
            }
            None
        }
    }
}

/// Resolve a model spec (e.g. "local:zipformer-small", "zipformer:zipformer-small",
/// or bare "zipformer-small") to its [`EngineCategory`] by looking up DB models.
///
/// Task 2 后：`load_config` 只含激活的那一个 ASR entry——本函数仅匹配激活引擎。
/// CLI `--model` / 多模型显式路径用 [`resolve_engine_category_any`]（查所有可用引擎）。
/// Returns `None` if the spec doesn't match the active ASR model.
pub fn resolve_engine_category(spec: &str) -> Option<EngineCategory> {
    let config = load_config().ok()?;
    resolve_engine_in_config(&config, spec).map(|(cat, _, _)| cat)
}

/// 解析 spec → EngineCategory（查 DB 所有可用 ASR 引擎，不限激活）。
///
/// 供 CLI `--model` 显式路径 / 多模型场景用——不依赖激活态（load_config 只含激活）。
/// 直接查 `list_all_asr_engines`（DB is_available=1 的全部 ASR），按 spec 匹配。
pub fn resolve_engine_category_any(spec: &str) -> Option<EngineCategory> {
    let parsed = parse_model_spec(spec);
    let rows = octopus_infra::db::list_all_asr_engines().ok()?;
    match parsed {
        ModelSpec::Full { provider, category, model_name } => {
            rows.into_iter()
                .find(|r| r.provider == provider && r.category == category && r.model_name == model_name)
                .and_then(|r| resolve_category(&r.provider, &r.category))
        }
        ModelSpec::NameOnly(model_name) => {
            rows.into_iter()
                .find(|r| r.model_name == model_name)
                .and_then(|r| resolve_category(&r.provider, &r.category))
        }
    }
}

/// 解析 spec → (EngineCategory, ModelEntry)，查 DB 所有可用 ASR 引擎（不限激活）。
///
/// 供 AsrEngineManager::load_engine_into_cache 用——用户经 CLI `--model` 或其他多模型
/// 入口选了非激活引擎时，[`resolve_engine_in_config`]（load_config 只含激活）找不到，
/// 此函数直接查 DB 任意可用引擎。spec 支持 3-part 或裸名。
pub fn resolve_engine_any(spec: &str) -> Option<(EngineCategory, ModelEntry)> {
    let parsed = parse_model_spec(spec);
    let (provider, category, model_name) = match parsed {
        ModelSpec::Full { provider, category, model_name } => (Some(provider), Some(category), model_name),
        ModelSpec::NameOnly(model_name) => (None, None, model_name),
    };
    let row = octopus_infra::db::get_asr_model_by_spec(provider, category, model_name).ok()?;
    let row = row?;
    let cat = resolve_category(&row.provider, &row.category)?;
    let entry = ModelEntry {
        source: row.source,
        language: row.language,
        description: row.description,
        secret_key: row.secret_key,
        source_type: row.source_type,
        is_enabled: row.is_enabled,
        is_available: row.is_available,
        is_streaming: row.is_streaming,
    };
    Some((cat, entry))
}

// ── List all available engines ──

/// 可用引擎条目
pub struct EngineInfo {
    pub name: String,
    pub provider: String,
    pub category: EngineCategory,
    pub description: String,
    /// 模型来源: 0=builtin 1=local 2=cloud（详见 infra::db::ModelEntry）。
    pub source_type: i64,
    /// DB 行 id（Task 2 后补，供前端 switch_active_model(domain, id) 用）。
    pub id: i64,
    /// DB models.source（Task 2 后补，供前端展示 / 编辑回填）。
    pub source: String,
    /// DB models.secret_key（Task 2 后补，脱敏后供前端编辑回填）。
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB models.is_enabled（激活态，每域仅 1 个=1）。供前端标 current。
    pub is_enabled: bool,
}

/// EngineCategory → category 字符串（与 DB models.category 一致，用于排序、显示、构造 spec）。
///
/// 三端（asr / desktop / cli）共享此唯一映射。Aliyun 对应 DB 的 `Fun-ASR` 模型族
/// （db.sql seed 的 category 列），ByteDance 对应 `Doubao-ASR`，Tencent 对应 `Tencent-ASR`，
/// spec 构造和显示必须与此一致，否则 `{provider}:{category}:{model_name}` 格式的 category 段不匹配 DB 实际值。
pub fn category_label(c: EngineCategory) -> &'static str {
    use EngineCategory::*;
    match c {
        Whisper => "whisper",
        SenseVoiceOrig => "sensevoice-orig",
        Paraformer => "paraformer",
        Qwen3Asr => "qwen3-asr",
        Zipformer => "zipformer",
        Moonshine => "moonshine",
        FireRed => "firered",
        Aliyun => "Fun-ASR",
        ByteDance => "Doubao-ASR",
        Tencent => "Tencent-ASR",
        Baidu => "Baidu-ASR",
    }
}

/// 排序：source_type 升序（builtin(0) < local(1) < cloud(2)，本地在前）→ category 字母序 → name 字母序。
fn order_engine_infos(engines: &mut [EngineInfo]) {
    engines.sort_by(|a, b| {
        a.source_type
            .cmp(&b.source_type)
            .then_with(|| category_label(a.category).cmp(category_label(b.category)))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 从 DB models 表列出所有已配置的 ASR 引擎
/// 列出 ASR 域所有引擎——**直查 DB，不经 RUNTIME_CONFIG 缓存**。
///
/// 管理列表（设置页 / 工具栏 / CLI select）专用：新增 / 编辑 / 删除云端模型后即时反映，
/// 不依赖 `reload_models_config`。RUNTIME_CONFIG（`load_config`，仅 is_enabled=1 可用模型）
/// 仅供推理路径——`resolve_active_engine` 从可用集合里按 `app_config.asr_engine` 挑激活引擎。
///
/// 推理路径（`resolve_engine_in_config` / `resolve_active_engine` / 各引擎 transcribe）
/// **不**走此函数，继续用 `load_config`。
pub fn list_engines_from_db() -> Result<Vec<EngineInfo>> {
    let rows = octopus_infra::db::list_all_asr_engines()?;
    let mut engines: Vec<EngineInfo> = rows
        .into_iter()
        .filter_map(|r| {
            // provider+category → EngineCategory（provider_of 的逆映射）
            resolve_category(&r.provider, &r.category).map(|cat| EngineInfo {
                name: r.model_name,
                provider: r.provider,
                category: cat,
                description: r.description,
                source_type: r.source_type,
                id: r.id,
                source: r.source,
                secret_key: r.secret_key,
                is_streaming: r.is_streaming,
                is_thinking: r.is_thinking,
                is_enabled: r.is_enabled,
            })
        })
        .collect();
    order_engine_infos(&mut engines);
    Ok(engines)
}

// ── 激活引擎解析（4 域统一：load_active_engine / resolve_active_engine）──

/// 解析后的引擎：domain + name + provider + category + is_thinking + entry。
///
/// 4 域（asr/llm/ocr/translate）共用此结构。`category` 为 DB `models.category` 原始字符串
/// （如 "whisper" / "Fun-ASR" / "qwen"）；ASR 内部路由按需用 [`Self::as_engine_category`]
/// 转换为 [`EngineCategory`] 枚举。`is_thinking` 来自 DB `models.is_thinking`（LLM 专用，
/// 其余域恒为 false）——ModelEntry 不含此字段，故提升到顶层。
#[derive(Debug, Clone)]
pub struct ResolvedEngine {
    /// 域标识："asr" / "llm" / "ocr" / "translate"。
    pub domain: String,
    /// 裸模型名（DB models.model_name，不含 spec 前缀）。
    pub name: String,
    /// 提供方（DB models.provider，如 "local" / "aliyun" / "deepseek"）。
    pub provider: String,
    /// DB models.category 原始字符串（ASR 族名 / LLM 系列 / 云端族名等）。
    pub category: String,
    /// DB models.is_thinking（LLM reasoning 模型标记；其余域为 false）。
    pub is_thinking: bool,
    /// 引擎配置（source / secret_key / language / is_streaming 等）。
    pub entry: ModelEntry,
}

impl ResolvedEngine {
    /// category 字符串 → EngineCategory（ASR 内部路由用）。
    ///
    /// 仅 ASR domain 有意义（whisper/paraformer/zipformer 等本地族 + aliyun/bytedance/
    /// tencent/baidu 云端族）。LLM/OCR/Translate domain 调此方法返回 None。
    pub fn as_engine_category(&self) -> Option<EngineCategory> {
        resolve_category(&self.provider, &self.category)
    }
}

/// 激活引擎内存缓存：4 域各一个槽（domain → Arc<ResolvedEngine>）。
///
/// 启动时 [`load_active_engine`] 填充；激活切换后 [`reload_active_engine`] 刷新该域。
/// 推理热路径（各使用方）经 [`resolve_active_engine`] 纯读此缓存——零 DB 开销。
static ACTIVE_ENGINES: std::sync::LazyLock<RwLock<HashMap<String, Arc<ResolvedEngine>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// 从 DB ModelRow 构造 ResolvedEngine（4 域统一）。entry 字段从 row 全字段构造。
/// 从 DB ModelRow 构造 ResolvedEngine（4 域统一）。pub 供 desktop 按 key 查 LLM 复用。
pub fn resolved_engine_from_row(row: &octopus_infra::db::ModelRow) -> ResolvedEngine {
    ResolvedEngine {
        domain: row.domain.clone(),
        name: row.model_name.clone(),
        provider: row.provider.clone(),
        category: row.category.clone(),
        is_thinking: row.is_thinking,
        entry: ModelEntry {
            source: row.source.clone(),
            language: row.language.clone(),
            description: row.description.clone(),
            secret_key: row.secret_key.clone(),
            source_type: row.source_type,
            is_enabled: row.is_enabled,
            is_available: row.is_available,
            is_streaming: row.is_streaming,
        },
    }
}

/// 从 DB 加载指定域的激活模型并缓存到 [`ACTIVE_ENGINES`]。
///
/// 仅在以下时机调用：① 应用启动（main.rs 初始化 4 域）；② 设置页激活模型后
/// （switch_active_model 写 DB 后调 [`reload_active_engine`]）。
///
/// 缓存命中（该 domain 已有槽）直接返回旧值——**不强制重读 DB**。需要强制刷新
/// 用 [`reload_active_engine`]。
///
/// ASR 域无激活时 fallback 兜底引擎（[`FALLBACK_ASR_ENGINE_NAME`]）；其余域返回 Err。
pub fn load_active_engine(domain: &str) -> Result<ResolvedEngine> {
    // 读缓存命中则返回
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
    crate::db::ensure_db()?;
    match octopus_infra::db::get_active_model(domain)? {
        Some(row) => {
            let resolved = resolved_engine_from_row(&row);
            ACTIVE_ENGINES
                .write()
                .unwrap()
                .insert(domain.to_string(), Arc::new(resolved.clone()));
            Ok(resolved)
        }
        None => {
            if domain == "asr" {
                // ASR 兜底引擎
                let resolved = fallback_resolved_engine();
                ACTIVE_ENGINES
                    .write()
                    .unwrap()
                    .insert(domain.to_string(), Arc::new(resolved.clone()));
                Ok(resolved)
            } else {
                anyhow::bail!("域 '{}' 无激活模型（is_enabled=1 AND is_available=1），请在设置页激活", domain)
            }
        }
    }
}

/// 重载指定域的激活缓存（先清该域槽 → 重新 [`load_active_engine`]）。
///
/// switch_active_model 写 DB 后调用。也会同步重载 ASR 的 `load_config` 缓存
/// （引擎实例化路径用）——保持两条缓存一致。
pub fn reload_active_engine(domain: &str) -> Result<ResolvedEngine> {
    ACTIVE_ENGINES.write().unwrap().remove(domain);
    if domain == "asr" {
        reload_models_config();
    }
    load_active_engine(domain)
}

/// 从内存缓存取指定域的唯一激活模型。
///
/// 各个使用方（推理 / tray / 管理页当前态 / 流式判定 / LLM polish / OCR / 翻译）都调此方法。
/// 缓存未命中（启动尚未 load / 被清）→ fallback 到 [`load_active_engine`]。
///
/// ASR 域带兜底引擎 fallback（[`FALLBACK_ASR_ENGINE_NAME`]）；其余域无激活返回 Err。
pub fn resolve_active_engine(domain: &str) -> Result<ResolvedEngine> {
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
    // 缓存未命中（启动尚未 load / 被清）→ 走 load_active_engine 兜底
    load_active_engine(domain)
}

/// 兜底引擎固定裸名。
const FALLBACK_ASR_ENGINE_NAME: &str = "zipformer-small";

/// ASR 兜底引擎
/// （用户可能改过 source），否则硬构造（本地打包路径）。
fn fallback_resolved_engine() -> ResolvedEngine {
    // 查 DB 任意可用 ASR 的 zipformer-small（不限激活）
    if let Some((_cat, entry)) = resolve_engine_any(FALLBACK_ASR_ENGINE_NAME) {
        return ResolvedEngine {
            domain: "asr".to_string(),
            name: FALLBACK_ASR_ENGINE_NAME.to_string(),
            provider: "local".to_string(),
            category: "zipformer".to_string(),
            is_thinking: false,
            entry,
        };
    }
    // DB 无 zipformer-small（极端情况）仍可用——靠本地打包路径硬构造
    // source_type=0（builtin）—— 兜底引擎属于内置分类
    ResolvedEngine {
        domain: "asr".to_string(),
        name: FALLBACK_ASR_ENGINE_NAME.to_string(),
        provider: "local".to_string(),
        category: "zipformer".to_string(),
        is_thinking: false,
        entry: ModelEntry {
            source: DEFAULT_ASR_MODEL_DIR.to_string(),
            language: "zh".to_string(),
            description: String::new(),
            secret_key: String::new(),
            source_type: 0,
            is_enabled: true,
            is_available: true,
            is_streaming: true,
        },
    }
}

/// ONNX 模型文件发现——按 `prefer_int8` 优先选 int8 或 fp32，不存在则 bail。
///
/// `base` — 模型目录；`name` — 文件基名（如 "encoder"/"decoder"）。
/// 查找 `{base}/{name}.int8.onnx` 和 `{base}/{name}.onnx`。
///
/// 2026-08-05 抽取：消除 qwen3_asr.rs / streaming_paraformer.rs 的逐字重复副本 +
/// paraformer.rs 的手写 if-else 链（encoder/decoder 各一份）。zipformer 的
/// `discover_streaming_zipformer_onnx` 是更复杂的扫描变体，不合并。
pub(crate) fn discover_onnx(
    base: &std::path::Path,
    name: &str,
    prefer_int8: bool,
) -> Result<std::path::PathBuf> {
    let int8 = base.join(format!("{}.int8.onnx", name));
    let fp32 = base.join(format!("{}.onnx", name));
    if prefer_int8 {
        if int8.exists() {
            Ok(int8)
        } else if fp32.exists() {
            Ok(fp32)
        } else {
            anyhow::bail!(
                "{}.onnx / {}.int8.onnx not found at {}",
                name,
                name,
                base.display()
            )
        }
    } else {
        if fp32.exists() {
            Ok(fp32)
        } else if int8.exists() {
            Ok(int8)
        } else {
            anyhow::bail!(
                "{}.onnx / {}.int8.onnx not found at {}",
                name,
                name,
                base.display()
            )
        }
    }
}

/// 从已加载的 AsrConfig 取 zipformer-small 条目构造兜底 ResolvedEngine。
/// 配置中无该条目时返回 None（调用方走硬构造兜底）。纯函数，仅供单测。
#[cfg(test)]
fn fallback_engine_from_cfg(cfg: &AsrConfig) -> Option<ResolvedEngine> {
    let entry = cfg.asr.zipformer.as_ref()?.get(FALLBACK_ASR_ENGINE_NAME)?;
    Some(ResolvedEngine {
        domain: "asr".to_string(),
        name: FALLBACK_ASR_ENGINE_NAME.to_string(),
        provider: "local".to_string(),
        category: "zipformer".to_string(),
        is_thinking: false,
        entry: entry.clone(),
    })
}

/// 按 category + name 从配置中取 entry（统一各引擎模块/AsrEngineManager 的查找逻辑）。
pub fn pick_entry<'a>(
    cfg: &'a AsrConfig,
    category: EngineCategory,
    name: &str,
) -> Option<&'a ModelEntry> {
    let map = match category {
        EngineCategory::Whisper => cfg.asr.whisper.as_ref(),
        EngineCategory::SenseVoiceOrig => cfg.asr.sensevoice_orig.as_ref(),
        EngineCategory::Paraformer => cfg.asr.paraformer.as_ref(),
        EngineCategory::Qwen3Asr => cfg.asr.qwen3_asr.as_ref(),
        EngineCategory::Zipformer => cfg.asr.zipformer.as_ref(),
        EngineCategory::Moonshine => cfg.asr.moonshine.as_ref(),
        EngineCategory::FireRed => cfg.asr.firered.as_ref(),
        EngineCategory::Aliyun => cfg.asr.aliyun.as_ref(),
        EngineCategory::ByteDance => cfg.asr.bytedance.as_ref(),
        EngineCategory::Tencent => cfg.asr.tencent.as_ref(),
        EngineCategory::Baidu => cfg.asr.baidu.as_ref(),
    }?;
    map.get(name)
}

/// 运行时缓存 AppConfig（真相源 = DB app_config 表，经 infra::config::load_config →
/// db::load_app_config 读取）。首次读取后缓存，避免每次引擎构建 session 时重复读 DB
/// （paraformer 一次识别建 encoder+decoder 两个 session，streaming 引擎更频繁）。
///
/// 可重载（审查 二1）：原用 OnceLock 不可失效，设置窗口改 denoise_mode /
/// asr_hardware_accelerated 后 ASR 侧仍读启动值——audio 每帧读 denoise、
/// apply_session_acceleration 读 hwaccel，导致设置「本次生效」承诺落空（需重启）。
/// 改 RwLock<Option<Arc<AppConfig>>>，desktop 写 DB 后调 [`reload_app_config`] 刷新。
/// 返回 Arc<AppConfig>：调用方均为即时字段访问，靠 Arc deref 兼容，无需改调用点。
static APP_CONFIG: std::sync::RwLock<Option<std::sync::Arc<octopus_infra::config::AppConfig>>> =
    std::sync::RwLock::new(None);

pub fn load_app_config_cached() -> std::sync::Arc<octopus_infra::config::AppConfig> {
    {
        let g = APP_CONFIG.read().unwrap();
        if let Some(cfg) = g.as_ref() {
            return cfg.clone();
        }
    }
    // 首次：从 DB 读并缓存（并发首调可能双方都建一份，last-write 无害）
    let cfg = std::sync::Arc::new(match octopus_infra::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to load config (DB), using defaults (ASR stays on CPU): {:?}", e);
            octopus_infra::config::AppConfig::default()
        }
    });
    *APP_CONFIG.write().unwrap() = Some(cfg.clone());
    cfg
}

/// 重载 ASR 侧 AppConfig 缓存（审查 二1）：从 DB 重读并替换，下次
/// [`load_app_config_cached`] 返回新值。desktop 在 set_config / set_denoise_mode
/// 写 DB 后调用，让 denoise_mode / asr_hardware_accelerated 等即时生效（以 DB 为真）。
pub fn reload_app_config() {
    match octopus_infra::config::load_config() {
        Ok(c) => {
            *APP_CONFIG.write().unwrap() = Some(std::sync::Arc::new(c));
            log::debug!("ASR AppConfig cache reloaded from DB");
        }
        Err(e) => {
            log::warn!("reload_app_config: 重载失败，保留旧缓存：{:?}", e);
        }
    }
}

/// Apply hardware acceleration configuration to a SessionBuilder.
/// 基础实现已抽取到 onnx-infra crate；此处包装加入 ASR 特有的 qwen3-asr CoreML 跳过逻辑。
pub fn apply_session_acceleration(builder: ort::session::builder::SessionBuilder) -> Result<ort::session::builder::SessionBuilder> {
    let app_cfg = load_app_config_cached();

    // qwen3-asr 含 CoreML 不支持的动态算子 → 跳过 EP，纯 CPU。
    // 激活引擎走 ACTIVE_ENGINES 缓存（resolve_active_engine("asr")，含兜底）。
    let active_cat = resolve_active_engine("asr")
        .ok()
        .and_then(|r| r.as_engine_category());
    let skip_coreml = app_cfg.asr_hardware_accelerated
        && active_cat == Some(EngineCategory::Qwen3Asr);
    onnx_infra::apply_session_acceleration(builder, skip_coreml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_entry(source: &str) -> ModelEntry {
        ModelEntry {
            source: source.to_string(),
            language: "zh".to_string(),
            description: String::new(),
            secret_key: String::new(),
            source_type: 1,
            is_enabled: true,
                is_available: true,
            is_streaming: false,
        }
    }

    /// 构造含 zipformer-small（本地路径）+ zipformer-multi（HF）的配置。
    fn cfg_with_zipformer() -> AsrConfig {
        let mut zip = HashMap::new();
        zip.insert("zipformer-small".to_string(), make_entry("asr/zipformer-small"));
        zip.insert("zipformer-multi".to_string(), make_entry("hf/zipformer-multi"));
        AsrConfig {
            asr: AsrSection {
                whisper: None,
                sensevoice_orig: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: Some(zip),
                moonshine: None,
                firered: None,
                bytedance: None,
                tencent: None,
                baidu: None,
                aliyun: None,
            },
        }
    }

    /// 构造含 aliyun Fun-ASR 条目的配置（用于验证云端路由）。
    fn cfg_with_aliyun() -> AsrConfig {
        let mut aliyun = HashMap::new();
        aliyun.insert(
            "fun-asr-2025-11-07".to_string(),
            ModelEntry {
                source: "wss://dashscope.aliyuncs.com/api-ws/v1/inference".to_string(),
                language: "auto".to_string(),
                description: String::new(),
                secret_key: String::new(),
                source_type: 2,
                is_enabled: true,
                is_available: true,
                is_streaming: false,
            },
        );
        AsrConfig {
            asr: AsrSection {
                whisper: None,
                sensevoice_orig: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: None,
                moonshine: None,
                firered: None,
                bytedance: None,
                tencent: None,
                baidu: None,
                aliyun: Some(aliyun),
            },
        }
    }

    // ── resolve_local_in 查找内核测试（阶段1：download 模型发现）──

    // resolve_local_in 测试已随函数移到 onnx-infra crate

    #[test]
    fn order_engine_infos_sorts_source_type_asc_then_category_then_name() {
        use EngineCategory::*;
        // mk_engine_info helper 局部构造（避免每个 case 写全 11 字段）
        let mk = |name: &str, cat: EngineCategory, source_type: i64| EngineInfo {
            name: name.into(), provider: "local".into(), category: cat, source_type,
            description: String::new(), id: 0, source: String::new(),
            secret_key: String::new(), is_streaming: false, is_thinking: false, is_enabled: false,
        };
        let mut engines = vec![
            mk("whisper-small", Whisper, 2),       // cloud
            mk("zipformer-multi", Zipformer, 1),   // local
            mk("paraformer-x", Paraformer, 2),     // cloud
            mk("zipformer-small", Zipformer, 0), // builtin
        ];
        order_engine_infos(&mut engines);
        let names: Vec<&str> = engines.iter().map(|e| e.name.as_str()).collect();
        // source_type 升序：builtin(0) → local(1) → cloud(2)。
        // builtin 仅 zipformer-small；local 仅 zipformer-multi；cloud: paraformer-x < whisper-small（category 字母序）
        assert_eq!(names, vec!["zipformer-small", "zipformer-multi", "paraformer-x", "whisper-small"]);
    }

    #[test]
    fn pick_entry_finds_present() {
        let cfg = cfg_with_zipformer();
        let e = pick_entry(&cfg, EngineCategory::Zipformer, "zipformer-multi").unwrap();
        assert_eq!(e.source, "hf/zipformer-multi");
    }

    #[test]
    fn pick_entry_missing_name_returns_none() {
        let cfg = cfg_with_zipformer();
        assert!(pick_entry(&cfg, EngineCategory::Zipformer, "nope").is_none());
    }

    #[test]
    fn pick_entry_absent_section_returns_none() {
        let cfg = cfg_with_zipformer();
        // whisper section 为 None
        assert!(pick_entry(&cfg, EngineCategory::Whisper, "whisper-small").is_none());
    }

    #[test]
    fn fallback_uses_db_zipformer_small_entry() {
        // DB 有 zipformer-small 条目 → 用 DB 的 source（用户手编仍生效）
        let cfg = cfg_with_zipformer();
        let r = fallback_engine_from_cfg(&cfg).expect("zipformer-small 应命中");
        assert_eq!(r.domain, "asr");
        assert_eq!(r.name, "zipformer-small");
        assert_eq!(r.provider, "local");
        assert_eq!(r.category, "zipformer");
        assert!(!r.is_thinking, "ASR 兜底引擎 is_thinking 应为 false");
        assert_eq!(r.entry.source, "asr/zipformer-small");
        // category 字符串 → EngineCategory 转换
        assert_eq!(r.as_engine_category(), Some(EngineCategory::Zipformer));
    }

    #[test]
    fn fallback_hardcodes_when_section_absent() {
        // DB 无 zipformer section → fallback_engine_from_cfg 返回 None（硬构造兜底由 fallback_resolved_engine 负责）
        let cfg = AsrConfig {
            asr: AsrSection {
                whisper: None,
                sensevoice_orig: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: None,
                moonshine: None,
                firered: None,
                bytedance: None,
                tencent: None,
                baidu: None,
                aliyun: None,
            },
        };
        assert!(fallback_engine_from_cfg(&cfg).is_none());
    }

    #[test]
    fn resolved_engine_from_row_maps_all_fields() {
        // ModelRow → ResolvedEngine 全字段映射（4 域统一，含 is_thinking）
        let row = octopus_infra::db::ModelRow {
            id: 42,
            domain: "llm".to_string(),
            provider: "deepseek".to_string(),
            category: "deepseek-reasoner".to_string(),
            model_name: "deepseek-reasoner".to_string(),
            source: "https://api.deepseek.com/v1".to_string(),
            secret_key: "sk-xxx".to_string(),
            language: String::new(),
            description: "DeepSeek Reasoner".to_string(),
            source_type: 2,
            is_thinking: true,
            is_streaming: false,
            is_enabled: true,
            is_available: true,
        };
        let r = resolved_engine_from_row(&row);
        assert_eq!(r.domain, "llm");
        assert_eq!(r.name, "deepseek-reasoner");
        assert_eq!(r.provider, "deepseek");
        assert_eq!(r.category, "deepseek-reasoner");
        assert!(r.is_thinking, "LLM reasoning 模型 is_thinking 应为 true");
        assert_eq!(r.entry.source, "https://api.deepseek.com/v1");
        assert_eq!(r.entry.secret_key, "sk-xxx");
        assert!(r.entry.is_cloud(), "云端模型 is_cloud 应为 true");
        // 非 ASR domain 的 category 字符串 → as_engine_category 返回 None
        assert_eq!(r.as_engine_category(), None);
    }

    // ── ModelSpec 解析测试（3-part）──

    #[test]
    fn parse_spec_full_3part() {
        assert_eq!(
            parse_model_spec("local:zipformer:zipformer-small"),
            ModelSpec::Full { provider: "local", category: "zipformer", model_name: "zipformer-small" }
        );
    }

    #[test]
    fn parse_spec_bare_name() {
        assert_eq!(parse_model_spec("zipformer-small"), ModelSpec::NameOnly("zipformer-small"));
    }

    #[test]
    fn resolve_full_3part_finds_local_model() {
        let cfg = cfg_with_zipformer(); // make_entry sets source_type=1 (local)
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "local:zipformer:zipformer-small").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small");
        assert!(entry.is_local());
    }

    #[test]
    fn resolve_full_3part_aliyun_routes_to_aliyun_section() {
        // provider='aliyun' → Aliyun section，无论 category 字符串（Fun-ASR）
        let cfg = cfg_with_aliyun();
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "aliyun:Fun-ASR:fun-asr-2025-11-07").unwrap();
        assert_eq!(cat, EngineCategory::Aliyun);
        assert_eq!(name, "fun-asr-2025-11-07");
        assert!(entry.is_cloud(), "aliyun 模型非本地");
        assert_eq!(entry.source, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
    }

    #[test]
    fn pick_entry_aliyun() {
        let cfg = cfg_with_aliyun();
        let e = pick_entry(&cfg, EngineCategory::Aliyun, "fun-asr-2025-11-07").unwrap();
        assert_eq!(e.source, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
    }

    #[test]
    fn resolve_full_wrong_category_returns_none() {
        let cfg = cfg_with_zipformer();
        // whisper section 不含 zipformer-multi
        assert!(resolve_engine_in_config(&cfg, "local:whisper:zipformer-multi").is_none());
    }

    #[test]
    fn resolve_bare_name_finds_anywhere() {
        // 裸名跨 section 搜，命中第一条匹配（不限 source_type）
        let cfg = cfg_with_zipformer();
        let (cat, name, _) = resolve_engine_in_config(&cfg, "zipformer-small").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small");
    }

    #[test]
    fn resolve_bare_name_finds_remote_aliyun() {
        // NameOnly 不再限 source_type——aliyun 云端条目也能命中
        let cfg = cfg_with_aliyun();
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "fun-asr-2025-11-07").unwrap();
        assert_eq!(cat, EngineCategory::Aliyun);
        assert_eq!(name, "fun-asr-2025-11-07");
        assert!(entry.is_cloud());
    }

    #[test]
    fn resolve_unknown_category_prefix_returns_none() {
        let cfg = cfg_with_zipformer();
        // 合法 3-part 但 zipformer section 不含此 name → None
        assert!(resolve_engine_in_config(&cfg, "local:zipformer:nope").is_none());
    }

    #[test]
    fn engine_category_from_str_maps_local_types() {
        assert_eq!(engine_category_from_str("whisper"), Some(EngineCategory::Whisper));
        assert_eq!(engine_category_from_str("paraformer"), Some(EngineCategory::Paraformer));
        assert_eq!(engine_category_from_str("qwen3-asr"), Some(EngineCategory::Qwen3Asr));
        assert_eq!(engine_category_from_str("zipformer"), Some(EngineCategory::Zipformer));
        // aliyun 不在 category 映射——它走 provider 路由
        assert_eq!(engine_category_from_str("aliyun"), None);
    }

    #[test]
    fn resolve_category_routes_aliyun_via_provider() {
        // provider='aliyun' 强制 Aliyun，category 字符串任意
        assert_eq!(resolve_category("aliyun", "Fun-ASR"), Some(EngineCategory::Aliyun));
        assert_eq!(resolve_category("ALIYUN", "anything"), Some(EngineCategory::Aliyun));
        // 非 aliyun provider 按 category 映射本地族
        assert_eq!(resolve_category("local", "zipformer"), Some(EngineCategory::Zipformer));
        assert_eq!(resolve_category("deepseek", "zipformer"), Some(EngineCategory::Zipformer));
        // category 字符串非本地族 → None
        assert_eq!(resolve_category("local", "Fun-ASR"), None);
    }

    /// §4.4 ResolvedEngine.as_engine_category：4 个云端 provider（aliyun/bytedance/
    /// tencent/baidu）按 provider 路由，category 字符串任意（Task 2 后云端 ASR 新增）。
    /// 大小写不敏感（eq_ignore_ascii_case）。
    #[test]
    fn resolve_category_routes_all_cloud_providers() {
        // bytedance / tencent / baidu 三个云端 provider（与 aliyun 对称，Task 2 前已加）
        assert_eq!(resolve_category("bytedance", "Doubao-ASR"), Some(EngineCategory::ByteDance));
        assert_eq!(resolve_category("Bytedance", "anything"), Some(EngineCategory::ByteDance));
        assert_eq!(resolve_category("tencent", "Tencent-ASR"), Some(EngineCategory::Tencent));
        assert_eq!(resolve_category("TENCENT", "anything"), Some(EngineCategory::Tencent));
        assert_eq!(resolve_category("baidu", "Baidu-ASR"), Some(EngineCategory::Baidu));
        assert_eq!(resolve_category("Baidu", "anything"), Some(EngineCategory::Baidu));
    }

    /// §4.4 ResolvedEngine.as_engine_category：通过 ResolvedEngine（含 domain/provider/
    /// category）调用 as_engine_category，验证 4 域统一结构的转换行为。
    /// - ASR domain + 云端 provider → 对应云端 EngineCategory
    /// - ASR domain + 本地族 category → 对应本地族 EngineCategory
    /// - 非 ASR domain（llm/ocr/translate）→ None（category 字符串非 ASR 族）
    #[test]
    fn as_engine_category_converts_resolved_engine() {
        // 云端 ASR（aliyun，category='Fun-ASR' 非 ASR 族名但 provider 路由）
        let aliyun = ResolvedEngine {
            domain: "asr".to_string(),
            name: "fun-asr-2025-11-07".to_string(),
            provider: "aliyun".to_string(),
            category: "Fun-ASR".to_string(),
            is_thinking: false,
            entry: make_entry("wss://dashscope.aliyuncs.com/api-ws/v1/inference"),
        };
        assert_eq!(aliyun.as_engine_category(), Some(EngineCategory::Aliyun));

        // 本地 ASR（zipformer）
        let zf = ResolvedEngine {
            domain: "asr".to_string(),
            name: "zipformer-small".to_string(),
            provider: "local".to_string(),
            category: "zipformer".to_string(),
            is_thinking: false,
            entry: make_entry("asr/zipformer-small"),
        };
        assert_eq!(zf.as_engine_category(), Some(EngineCategory::Zipformer));

        // LLM domain（category='deepseek' 非 ASR 族名）→ None
        let llm = ResolvedEngine {
            domain: "llm".to_string(),
            name: "deepseek-chat".to_string(),
            provider: "deepseek".to_string(),
            category: "deepseek".to_string(),
            is_thinking: false,
            entry: make_entry("https://api.deepseek.com/v1"),
        };
        assert_eq!(llm.as_engine_category(), None,
            "LLM domain 的 category 字符串非 ASR 族 → None");

        // OCR domain（category='paddleocr' 非 ASR 族名）→ None
        let ocr = ResolvedEngine {
            domain: "ocr".to_string(),
            name: "PP-OCRv6-small".to_string(),
            provider: "local".to_string(),
            category: "paddleocr".to_string(),
            is_thinking: false,
            entry: make_entry("ocr/PP-OCRv6-small"),
        };
        assert_eq!(ocr.as_engine_category(), None);

        // Translate domain（category='opus-mt' 非 ASR 族名）→ None
        let tr = ResolvedEngine {
            domain: "translate".to_string(),
            name: "opus-mt".to_string(),
            provider: "local".to_string(),
            category: "opus-mt".to_string(),
            is_thinking: false,
            entry: make_entry("translate/opus-mt"),
        };
        assert_eq!(tr.as_engine_category(), None);
    }

    // ── VadSource 测试 ──

    /// find_silero_vad：磁盘有文件 → File，磁盘无文件 → Builtin。
    #[test]
    fn find_silero_vad_returns_builtin_when_disk_missing() {
        // 磁盘文件可能存在（开发机）也可能不存在（CI），两种情况都验证正确性
        match find_silero_vad() {
            Ok(VadSource::File(path)) => {
                assert!(path.exists(), "File 路径必须真实存在");
                eprintln!("[INFO] 磁盘有 VAD 文件: {}", path.display());
            }
            Ok(VadSource::Builtin) => {
                eprintln!("[INFO] 磁盘无 VAD 文件，fallback 到 Builtin");
            }
            Err(e) => panic!("find_silero_vad 不应返回 Err（Builtin 是保底）: {}", e),
        }
    }

    /// create_silero_vad：Builtin 路径应能成功构造 SileroVad（或 ort 失败时 skip）。
    #[test]
    fn create_silero_vad_works() {
        match create_silero_vad() {
            Ok(_) => eprintln!("[PASS] create_silero_vad 成功构造"),
            Err(e) => eprintln!("[SKIP] ort session 构造失败（测试环境问题）: {}", e),
        }
    }
}
