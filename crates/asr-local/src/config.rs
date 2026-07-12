use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use octopus_infra::consts::{DEFAULT_ASR_MODEL_DIR, SILERO_VAD_PATH};
use octopus_infra::octopus_config_home;

// ── Model config schema（DB models 表）──
pub use octopus_infra::db::{parse_model_spec, AsrConfig, AsrSection, ModelEntry, ModelSpec};

// ── Config loading ──

/// 运行时缓存：首次从 DB 读出后缓存，避免每次识别重复开连接查询。
/// 可重载（见 [`reload_models_config`]）：模型管理页 set_model_enabled 后调用，让
/// 引擎下拉即时反映新的就绪状态。
static RUNTIME_CONFIG: RwLock<Option<Arc<AsrConfig>>> = RwLock::new(None);

/// 读取模型配置（唯一来源：~/.octopus/octopus.db 的 models 表）。
/// 首次调用 ensure_db（自动建表 + seed 默认引擎），读出后缓存。
/// cli/server/desktop 三端统一走此路径，不再读 model.json。
pub fn load_config() -> Result<AsrConfig> {
    // 读锁：已缓存则 clone 返回
    if let Some(arc) = RUNTIME_CONFIG.read().unwrap().as_ref() {
        return Ok(arc.as_ref().clone());
    }
    crate::db::ensure_db()?;
    let cfg = crate::db::load_models()?;
    // 写锁：double-check，避免并发首次 miss 重复 load（load_models 幂等，双重保险）
    let mut slot = RUNTIME_CONFIG.write().unwrap();
    if slot.is_none() {
        *slot = Some(Arc::new(cfg.clone()));
    }
    Ok(cfg)
}

/// 重载 AsrConfig 缓存（models 表）：从 DB 重读替换。
///
/// desktop 在 set_model_enabled / set_model_secret_key 写 DB 后调用，让引擎下拉
/// （`list_engines` → `load_config`）即时反映新的 is_enabled。对齐 [`reload_app_config`]。
pub fn reload_models_config() {
    match crate::db::load_models() {
        Ok(c) => {
            *RUNTIME_CONFIG.write().unwrap() = Some(Arc::new(c));
            log::debug!("AsrConfig cache reloaded from DB");
        }
        Err(e) => {
            log::warn!("reload_models_config: 重载失败，保留旧缓存：{:?}", e);
        }
    }
}

// ── HF cache helpers ──

/// 模型路径查找——已抽取到 onnx-infra crate
pub use onnx_infra::{find_hf_cache, find_onnx_dir, resolve_model_dir};

// ── VAD model discovery ──

/// 定位 Silero VAD 模型：固定 ~/.octopus/models/silero_vad_v4.onnx（随应用打包）。
/// 不再读配置/HF 缓存——VAD 模型固定路径，唯一方案。
pub fn find_silero_vad() -> Result<PathBuf> {
    let vad = octopus_config_home().join(SILERO_VAD_PATH);
    if vad.exists() {
        return Ok(vad);
    }
    anyhow::bail!(
        "Silero VAD model not found at {}. 请随应用打包该文件。",
        vad.display()
    )
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

/// Resolve a model spec (e.g. "local:zipformer-small-ctc", "zipformer:zipformer-small-ctc",
/// or bare "zipformer-small-ctc") to its [`EngineCategory`] by looking up DB models.
/// Returns `None` if the spec doesn't match any enabled ASR model.
pub fn resolve_engine_category(spec: &str) -> Option<EngineCategory> {
    let config = load_config().ok()?;
    resolve_engine_in_config(&config, spec).map(|(cat, _, _)| cat)
}

// ── List all available engines ──

/// 可用引擎条目
pub struct EngineInfo {
    pub name: String,
    pub provider: String,
    pub category: EngineCategory,
    pub description: String,
    pub is_local: bool,
}

/// EngineCategory 对应的 provider 字符串（与 DB models.provider 一致，用于构造 3-part spec）。
/// 本地族 → "local"；Aliyun → "aliyun"；ByteDance → "bytedance"；Tencent → "tencent"。
fn provider_of(c: &EngineCategory) -> &'static str {
    match c {
        EngineCategory::Aliyun => "aliyun",
        EngineCategory::ByteDance => "bytedance",
        EngineCategory::Tencent => "tencent",
        EngineCategory::Baidu => "baidu",
        _ => "local",
    }
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

/// 排序：is_local 降序（true 在前）→ category 字母序 → name 字母序。
fn order_engine_infos(engines: &mut [EngineInfo]) {
    engines.sort_by(|a, b| {
        b.is_local
            .cmp(&a.is_local)
            .then_with(|| category_label(a.category).cmp(category_label(b.category)))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 从 DB models 表列出所有已配置的 ASR 引擎
pub fn list_engines() -> Result<Vec<EngineInfo>> {
    let config = load_config()?;
    let mut engines = Vec::new();

    // 复用 all_sections（与 resolve_engine_in_config 的 NameOnly 遍历共享同一 section 顺序，
    // 避免两份手写副本发散）。
    for (section, category) in all_sections(&config) {
        if let Some(map) = section {
            for (name, entry) in map {
                engines.push(EngineInfo {
                    name: name.clone(),
                    provider: provider_of(&category).to_string(),
                    category,
                    description: entry.description.clone(),
                    is_local: entry.is_local,
                });
            }
        }
    }

    order_engine_infos(&mut engines);
    Ok(engines)
}

// ── 全局默认引擎解析（config.yaml.asr_engine → 具体引擎 + 兜底）──

/// 解析后的引擎：name + category + entry 三件套。
#[derive(Debug, Clone)]
pub struct ResolvedEngine {
    pub name: String,
    pub category: EngineCategory,
    pub entry: ModelEntry,
}

/// 解析 config.yaml.asr_engine 为具体引擎（全局默认引擎入口）。
///
/// `asr_engine` 支持 spec 格式（见 [`parse_model_spec`]）：
/// - `"local:zipformer-small-ctc"` — is_local=true AND name
/// - `"zipformer:zipformer-small-ctc"` — category AND name
/// - `"zipformer-small-ctc"` — 向后兼容，仅按 name
///
/// 解析规则：
/// - 非空且命中 → 返回裸名（去掉前缀）+ category + entry
/// - 空 / 匹配不到 → 回退兜底引擎（zipformer-small-ctc）
///
/// 返回的 `ResolvedEngine.name` 始终是**裸名**（不含 `local:` / `category:` 前缀），
/// 下游引擎缓存和模型加载按裸名工作。
///
/// 仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。
/// 显式 spec 路径（cli `--model`、AsrEngineManager.switch_model、server 请求带 engine）
/// 直接走 `resolve_engine_in_config + pick_entry`，不经此函数、不走兜底。
pub fn resolve_active_engine(asr_engine: &str) -> Result<ResolvedEngine> {
    let cfg = load_config()?;

    // 0. 兜底引擎短路：asr_engine 裸名为 zipformer-small-ctc（无论 spec 格式还是裸名）
    //    时直接返回兜底——该引擎随应用本地打包，不依赖 DB 是否有条目，避免无谓 warning。
    let bare = parse_model_spec(asr_engine).model_name();
    if bare == FALLBACK_ASR_ENGINE_NAME {
        return Ok(fallback_engine(&cfg));
    }

    // 1. 显式配置命中
    if !asr_engine.is_empty() {
        if let Some((category, bare_name, entry)) = resolve_engine_in_config(&cfg, asr_engine) {
            return Ok(ResolvedEngine {
                name: bare_name.to_string(),
                category,
                entry: entry.clone(),
            });
        }
        log::warn!(
            "config.yaml asr_engine='{}' 在 DB models 表中未匹配到，回退兜底引擎",
            asr_engine
        );
    }

    // 2. 兜底：zipformer-small-ctc
    Ok(fallback_engine(&cfg))
}

/// 兜底引擎固定裸名。
const FALLBACK_ASR_ENGINE_NAME: &str = "zipformer-small-ctc";

/// 兜底引擎：优先 DB zipformer section 的 zipformer-small-ctc，否则硬构造（本地打包路径）。
fn fallback_engine(cfg: &AsrConfig) -> ResolvedEngine {
    if let Some(zf) = cfg.asr.zipformer.as_ref() {
        if let Some(entry) = zf.get("zipformer-small-ctc") {
            return ResolvedEngine {
                name: "zipformer-small-ctc".to_string(),
                category: EngineCategory::Zipformer,
                entry: entry.clone(),
            };
        }
    }
    // DB 无 zipformer section（极端情况）仍可用——靠本地打包路径硬构造
    ResolvedEngine {
        name: "zipformer-small-ctc".to_string(),
        category: EngineCategory::Zipformer,
        entry: ModelEntry {
            source: DEFAULT_ASR_MODEL_DIR.to_string(),
            language: "zh".to_string(),
            description: String::new(),
            secret_key: String::new(),
            is_local: true,
            is_enabled: true,
            is_streaming: true,
        },
    }
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
    let skip_coreml = app_cfg.asr_hardware_accelerated
        && resolve_engine_category(&app_cfg.asr_engine) == Some(EngineCategory::Qwen3Asr);
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
            is_local: true,
            is_enabled: true,
            is_streaming: false,
        }
    }

    /// 构造含 zipformer-small-ctc（本地路径）+ zipformer-multi（HF）的配置。
    fn cfg_with_zipformer() -> AsrConfig {
        let mut zip = HashMap::new();
        zip.insert("zipformer-small-ctc".to_string(), make_entry("models/zipformer"));
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
                is_local: false,
                is_enabled: true,
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
    fn order_engine_infos_sorts_is_local_desc_then_category_then_name() {
        use EngineCategory::*;
        let mut engines = vec![
            EngineInfo { name: "whisper-small".into(), provider: "local".into(), category: Whisper, is_local: false, description: String::new() },
            EngineInfo { name: "zipformer-multi".into(), provider: "local".into(), category: Zipformer, is_local: true, description: String::new() },
            EngineInfo { name: "paraformer-x".into(), provider: "local".into(), category: Paraformer, is_local: false, description: String::new() },
            EngineInfo { name: "zipformer-small-ctc".into(), provider: "local".into(), category: Zipformer, is_local: true, description: String::new() },
        ];
        order_engine_infos(&mut engines);
        let names: Vec<&str> = engines.iter().map(|e| e.name.as_str()).collect();
        // is_local=true 先（zipformer-multi < zipformer-small-ctc 按 name），再 false（paraformer < whisper 按 category 字母序）
        assert_eq!(names, vec!["zipformer-multi", "zipformer-small-ctc", "paraformer-x", "whisper-small"]);
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
        // DB 有 zipformer-small-ctc 条目 → 用 DB 的 source（用户手编仍生效）
        let cfg = cfg_with_zipformer();
        let r = fallback_engine(&cfg);
        assert_eq!(r.name, "zipformer-small-ctc");
        assert_eq!(r.category, EngineCategory::Zipformer);
        assert_eq!(r.entry.source, "models/zipformer");
    }

    #[test]
    fn fallback_hardcodes_when_section_absent() {
        // DB 无 zipformer section → 硬构造兜底（DEFAULT_ASR_MODEL_DIR 本地打包路径）
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
        let r = fallback_engine(&cfg);
        assert_eq!(r.name, "zipformer-small-ctc");
        assert_eq!(r.entry.source, DEFAULT_ASR_MODEL_DIR);
        assert_eq!(r.entry.language, "zh");
    }

    // ── ModelSpec 解析测试（3-part）──

    #[test]
    fn parse_spec_full_3part() {
        assert_eq!(
            parse_model_spec("local:zipformer:zipformer-small-ctc"),
            ModelSpec::Full { provider: "local", category: "zipformer", model_name: "zipformer-small-ctc" }
        );
    }

    #[test]
    fn parse_spec_bare_name() {
        assert_eq!(parse_model_spec("zipformer-small-ctc"), ModelSpec::NameOnly("zipformer-small-ctc"));
    }

    #[test]
    fn resolve_full_3part_finds_local_model() {
        let cfg = cfg_with_zipformer(); // make_entry sets is_local=true
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "local:zipformer:zipformer-small-ctc").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small-ctc");
        assert!(entry.is_local);
    }

    #[test]
    fn resolve_full_3part_aliyun_routes_to_aliyun_section() {
        // provider='aliyun' → Aliyun section，无论 category 字符串（Fun-ASR）
        let cfg = cfg_with_aliyun();
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "aliyun:Fun-ASR:fun-asr-2025-11-07").unwrap();
        assert_eq!(cat, EngineCategory::Aliyun);
        assert_eq!(name, "fun-asr-2025-11-07");
        assert!(!entry.is_local, "aliyun 模型非本地");
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
        // 裸名跨 section 搜，命中第一条匹配（不限 is_local）
        let cfg = cfg_with_zipformer();
        let (cat, name, _) = resolve_engine_in_config(&cfg, "zipformer-small-ctc").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small-ctc");
    }

    #[test]
    fn resolve_bare_name_finds_remote_aliyun() {
        // NameOnly 不再限 is_local——aliyun 云端条目也能命中
        let cfg = cfg_with_aliyun();
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "fun-asr-2025-11-07").unwrap();
        assert_eq!(cat, EngineCategory::Aliyun);
        assert_eq!(name, "fun-asr-2025-11-07");
        assert!(!entry.is_local);
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
}
