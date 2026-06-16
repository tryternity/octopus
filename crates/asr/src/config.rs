use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use octopus_infra::consts::{DEFAULT_ASR_MODEL_DIR, SILERO_VAD_PATH};
use octopus_infra::octopus_config_home;

// ── Model config schema（DB models 表）──
pub use octopus_infra::db::{parse_model_spec, AsrConfig, AsrSection, ModelEntry, ModelSpec};

// ── Config loading ──

/// 运行时缓存：首次从 DB 读出后缓存，避免每次识别重复开连接查询。
/// 手编 DB models 表后需重启进程生效（与历史行为一致）。
static RUNTIME_CONFIG: OnceLock<AsrConfig> = OnceLock::new();

/// 读取模型配置（唯一来源：~/.octopus/octopus.db 的 models 表）。
/// 首次调用 ensure_db（自动建表 + seed 默认引擎），读出后缓存到 OnceLock。
/// cli/server/desktop 三端统一走此路径，不再读 model.json。
pub fn load_config() -> Result<AsrConfig> {
    if let Some(cfg) = RUNTIME_CONFIG.get() {
        return Ok(cfg.clone());
    }
    crate::db::ensure_db()?;
    let cfg = crate::db::load_models()?;
    let _ = RUNTIME_CONFIG.set(cfg.clone());
    Ok(cfg)
}

// ── HF cache helpers ──

/// 根据 HF source（如 "onnx-community/whisper-small"）定位到本地缓存路径
pub fn find_hf_cache(source: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let model_dir_name = source.replace('/', "--");
    let model_dir = PathBuf::from(home)
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", model_dir_name));

    if !model_dir.exists() {
        anyhow::bail!(
            "HF cache not found for '{}'. Run: hf download {}",
            source,
            source
        );
    }
    find_latest_snapshot(&model_dir)
}

/// 在 HF 缓存路径中查找 onnx 子目录或直接返回根目录
pub fn find_onnx_dir(hf_path: &Path) -> PathBuf {
    let onnx = hf_path.join("onnx");
    if onnx.exists() {
        onnx
    } else {
        hf_path.to_path_buf()
    }
}

/// 解析模型目录：优先本地固定路径（随应用打包），回退 HF 缓存。
/// - source 为本地相对路径（如 "models/zipformer"）→ octopus_config_home/source
/// - source 为绝对路径 → 直接用
/// - 否则当 HF repo 名（如 "onnx-community/whisper-small"）→ find_hf_cache
pub fn resolve_model_dir(source: &str) -> Result<PathBuf> {
    // 1. octopus_config_home 下相对路径（随应用打包的小模型）
    let local = octopus_config_home().join(source);
    if local.is_dir() {
        return Ok(local);
    }
    // 2. 绝对路径
    let abs = PathBuf::from(source);
    if abs.is_dir() {
        return Ok(abs);
    }
    // 3. HF repo 名 → 缓存发现
    find_hf_cache(source)
}

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

// ── DeepFilterNet3 model discovery ──

/// DF3 模型 HF repo（唯一固定，不走 DB / 不切换）。
const DF3_HF_REPO: &str = "penta2himajin/deepfilternet3-onnx";
/// DF3 onnx 文件名（带 GRU 状态的流式版）。
const DF3_ONNX_FILE: &str = "dfn3.onnx";

/// 定位 DeepFilterNet3 模型：~/.cache/huggingface/hub/models--penta2himajin--deepfilternet3-onnx/snapshots/*/dfn3.onnx
/// 单一固定模型，不走 DB；缺失时提示下载命令。
///
/// 注：`find_hf_cache` 内部已调用 `find_latest_snapshot`，返回值即为最新 snapshot 目录，
/// 故这里直接 join `dfn3.onnx`，不再二次解析。
pub fn find_df3() -> Result<PathBuf> {
    let snapshot = find_hf_cache(DF3_HF_REPO)?;
    let onnx = snapshot.join(DF3_ONNX_FILE);
    if onnx.exists() {
        return Ok(onnx);
    }
    anyhow::bail!(
        "DeepFilterNet3 模型缺失，请先下载：hf download {}",
        DF3_HF_REPO
    )
}

// ── Internal helpers ──

fn find_latest_snapshot(model_dir: &Path) -> Result<PathBuf> {
    let snapshots = model_dir.join("snapshots");
    if !snapshots.exists() {
        anyhow::bail!("No snapshots dir in {}", model_dir.display());
    }
    let entries: Vec<_> = std::fs::read_dir(&snapshots)?
        .filter_map(|e| e.ok())
        .collect();
    entries
        .into_iter()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if m.is_dir() {
                Some((e.path(), m.modified().ok()?))
            } else {
                None
            }
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
        .context("No snapshots")
}

// ── Engine routing ──

/// ASR engine category, determined by which section in DB models table contains the engine name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineCategory {
    Whisper,
    SenseVoice,
    Paraformer,
    Qwen3Asr,
    Zipformer,
}

/// DB `models.category` 字符串 → EngineCategory 映射。
/// 仅映射 ASR 本地引擎类型（whisper/sensevoice/paraformer/qwen3-asr/zipformer），
/// 其他 category（如远程供应商 `aliyun`）返回 None。
fn engine_category_from_str(s: &str) -> Option<EngineCategory> {
    match s {
        "whisper" => Some(EngineCategory::Whisper),
        "sensevoice" => Some(EngineCategory::SenseVoice),
        "paraformer" => Some(EngineCategory::Paraformer),
        "qwen3-asr" => Some(EngineCategory::Qwen3Asr),
        "zipformer" => Some(EngineCategory::Zipformer),
        _ => None,
    }
}

/// 按固定顺序遍历 AsrConfig 的 5 个 section（用于 NameOnly / Local 查找）。
/// 顺序与 `resolve_engine_category` 原始逻辑一致。
fn all_sections<'a>(
    cfg: &'a AsrConfig,
) -> [(Option<&'a HashMap<String, ModelEntry>>, EngineCategory); 5] {
    [
        (cfg.asr.whisper.as_ref(), EngineCategory::Whisper),
        (cfg.asr.sensevoice.as_ref(), EngineCategory::SenseVoice),
        (cfg.asr.paraformer.as_ref(), EngineCategory::Paraformer),
        (cfg.asr.qwen3_asr.as_ref(), EngineCategory::Qwen3Asr),
        (cfg.asr.zipformer.as_ref(), EngineCategory::Zipformer),
    ]
}

/// 解析 spec 并在已加载配置中查找，返回 (category, 裸名, entry 引用)。
///
/// spec 格式见 [`parse_model_spec`]：
/// - `local:NAME` — 遍历所有 section，匹配 `is_local=true AND name`
/// - `CATEGORY:NAME` — CATEGORY 必须是已知引擎类型（whisper/sensevoice/...），精确查对应 section
/// - `NAME`（无冒号）— 遍历所有 section 按 name 查找（向后兼容）
pub fn resolve_engine_in_config<'a, 'b>(
    cfg: &'a AsrConfig,
    spec: &'b str,
) -> Option<(EngineCategory, &'b str, &'a ModelEntry)> {
    let parsed = parse_model_spec(spec);
    let name = parsed.name();
    match parsed {
        ModelSpec::Local(_) | ModelSpec::NameOnly(_) => {
            for (section, cat) in all_sections(cfg) {
                if let Some(map) = section {
                    if let Some(entry) = map.get(name) {
                        if entry.is_local {
                            return Some((cat, name, entry));
                        }
                    }
                }
            }
            None
        }
        ModelSpec::Category(cat_str, _) => {
            let cat = engine_category_from_str(cat_str)?;
            pick_entry(cfg, cat, name).map(|e| (cat, name, e))
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
    pub category: EngineCategory,
    pub description: String,
    pub is_local: bool,
}

/// EngineCategory → 小写 category 字符串（与 DB models.category 一致，用于排序与显示）。
fn category_label(c: &EngineCategory) -> &'static str {
    use EngineCategory::*;
    match c {
        Whisper => "whisper",
        SenseVoice => "sensevoice",
        Paraformer => "paraformer",
        Qwen3Asr => "qwen3-asr",
        Zipformer => "zipformer",
    }
}

/// 排序：is_local 降序（true 在前）→ category 字母序 → name 字母序。
fn order_engine_infos(engines: &mut [EngineInfo]) {
    engines.sort_by(|a, b| {
        b.is_local
            .cmp(&a.is_local)
            .then_with(|| category_label(&a.category).cmp(category_label(&b.category)))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 从 DB models 表列出所有已配置的 ASR 引擎
pub fn list_engines() -> Result<Vec<EngineInfo>> {
    let config = load_config()?;
    let mut engines = Vec::new();

    let sections: [(Option<&HashMap<String, ModelEntry>>, EngineCategory); 5] = [
        (config.asr.whisper.as_ref(), EngineCategory::Whisper),
        (config.asr.sensevoice.as_ref(), EngineCategory::SenseVoice),
        (config.asr.paraformer.as_ref(), EngineCategory::Paraformer),
        (config.asr.qwen3_asr.as_ref(), EngineCategory::Qwen3Asr),
        (config.asr.zipformer.as_ref(), EngineCategory::Zipformer),
    ];

    for (section, category) in sections {
        if let Some(map) = section {
            for (name, entry) in map {
                engines.push(EngineInfo {
                    name: name.clone(),
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
        EngineCategory::SenseVoice => cfg.asr.sensevoice.as_ref(),
        EngineCategory::Paraformer => cfg.asr.paraformer.as_ref(),
        EngineCategory::Qwen3Asr => cfg.asr.qwen3_asr.as_ref(),
        EngineCategory::Zipformer => cfg.asr.zipformer.as_ref(),
    }?;
    map.get(name)
}

/// 运行时缓存 config.yaml（AppConfig）：首次读取后缓存，避免每次引擎构建 session 时
/// 重复读文件 + 解析 yaml（paraformer 一次识别建 encoder+decoder 两个 session，
/// streaming 引擎更频繁）。手编 config.yaml 后需重启进程生效（与 RUNTIME_CONFIG 一致）。
static APP_CONFIG: OnceLock<octopus_infra::config::AppConfig> = OnceLock::new();

pub fn load_app_config_cached() -> &'static octopus_infra::config::AppConfig {
    APP_CONFIG.get_or_init(|| match octopus_infra::config::load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            log::warn!("Failed to load config.yaml, using defaults (ASR stays on CPU): {:?}", e);
            octopus_infra::config::AppConfig::default()
        }
    })
}

/// Apply hardware acceleration configuration (if enabled in config.yaml) to a SessionBuilder.
/// If the acceleration registration fails, it logs a warning and falls back to CPU.
pub fn apply_session_acceleration(builder: ort::session::builder::SessionBuilder) -> Result<ort::session::builder::SessionBuilder> {
    let app_cfg = load_app_config_cached();

    if !app_cfg.asr_hardware_accelerated {
        return Ok(builder);
    }

    let providers = vec![
        ort::ep::CUDAExecutionProvider::default().build(),
        ort::ep::DirectMLExecutionProvider::default().build(),
        ort::ep::CoreMLExecutionProvider::default().build(),
    ];

    log::info!("Attempting to build session with hardware acceleration execution providers");
    match builder.with_execution_providers(providers) {
        Ok(b) => {
            log::info!("Successfully registered EPs!");
            Ok(b)
        }
        Err(e) => {
            log::warn!("Failed to register hardware acceleration EPs: {:?}. Falling back to CPU.", e);
            ort::session::Session::builder().context("Failed to reconstruct fallback session builder")
        }
    }
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
                sensevoice: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: Some(zip),
            },
        }
    }

    #[test]
    fn order_engine_infos_sorts_is_local_desc_then_category_then_name() {
        use EngineCategory::*;
        let mut engines = vec![
            EngineInfo { name: "whisper-small".into(), category: Whisper, is_local: false, description: String::new() },
            EngineInfo { name: "zipformer-multi".into(), category: Zipformer, is_local: true, description: String::new() },
            EngineInfo { name: "paraformer-x".into(), category: Paraformer, is_local: false, description: String::new() },
            EngineInfo { name: "zipformer-small-ctc".into(), category: Zipformer, is_local: true, description: String::new() },
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
                sensevoice: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: None,
            },
        };
        let r = fallback_engine(&cfg);
        assert_eq!(r.name, "zipformer-small-ctc");
        assert_eq!(r.entry.source, DEFAULT_ASR_MODEL_DIR);
        assert_eq!(r.entry.language, "zh");
    }

    #[test]
    fn find_df3_missing_returns_download_hint() {
        // 模型未下载时，find_df3 应返回含 hf download 提示的 Err
        match crate::config::find_df3() {
            Ok(_) => { /* 模型存在，跳过缺失路径断言 */ }
            Err(e) => {
                let msg = format!("{:#}", e);
                assert!(
                    msg.contains("hf download penta2himajin/deepfilternet3-onnx"),
                    "缺失时应提示 hf download 命令，实际: {}",
                    msg
                );
            }
        }
    }

    // ── ModelSpec 解析测试 ──

    #[test]
    fn parse_spec_local_prefix() {
        assert_eq!(parse_model_spec("local:zipformer-small-ctc"), ModelSpec::Local("zipformer-small-ctc"));
    }

    #[test]
    fn parse_spec_category_prefix() {
        assert_eq!(
            parse_model_spec("zipformer:zipformer-small-ctc"),
            ModelSpec::Category("zipformer", "zipformer-small-ctc")
        );
    }

    #[test]
    fn parse_spec_bare_name() {
        assert_eq!(parse_model_spec("zipformer-small-ctc"), ModelSpec::NameOnly("zipformer-small-ctc"));
    }

    #[test]
    fn resolve_local_prefix_finds_local_model() {
        let cfg = cfg_with_zipformer(); // make_entry sets is_local=true
        let (cat, name, entry) = resolve_engine_in_config(&cfg, "local:zipformer-small-ctc").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small-ctc");
        assert!(entry.is_local);
    }

    #[test]
    fn resolve_category_prefix_matches_section() {
        let cfg = cfg_with_zipformer();
        let (cat, name, _) = resolve_engine_in_config(&cfg, "zipformer:zipformer-multi").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-multi");
    }

    #[test]
    fn resolve_category_prefix_wrong_category_returns_none() {
        let cfg = cfg_with_zipformer();
        // whisper:zipformer-multi → whisper section 不含此 name
        assert!(resolve_engine_in_config(&cfg, "whisper:zipformer-multi").is_none());
    }

    #[test]
    fn resolve_bare_name_equivalent_to_local() {
        // 裸名等价于 local: — make_entry 设 is_local=true，所以能命中
        let cfg = cfg_with_zipformer();
        let (cat, name, _) = resolve_engine_in_config(&cfg, "zipformer-small-ctc").unwrap();
        assert_eq!(cat, EngineCategory::Zipformer);
        assert_eq!(name, "zipformer-small-ctc");
    }

    #[test]
    fn resolve_bare_name_skips_non_local() {
        // 裸名等价 local:，is_local=false 的条目不应被命中
        let mut zip = HashMap::new();
        zip.insert(
            "zipformer-remote".to_string(),
            ModelEntry {
                source: "hf/zipformer-remote".to_string(),
                language: "zh".to_string(),
                description: String::new(),
                secret_key: String::new(),
                is_local: false,
                is_enabled: true,
                is_streaming: true,
            },
        );
        let cfg = AsrConfig {
            asr: AsrSection {
                whisper: None,
                sensevoice: None,
                paraformer: None,
                qwen3_asr: None,
                zipformer: Some(zip),
            },
        };
        assert!(
            resolve_engine_in_config(&cfg, "zipformer-remote").is_none(),
            "裸名等价 local:，is_local=false 的模型不应被命中"
        );
    }

    #[test]
    fn resolve_unknown_category_prefix_returns_none() {
        let cfg = cfg_with_zipformer();
        // aliyun 不是已知引擎 category → None
        assert!(resolve_engine_in_config(&cfg, "aliyun:zipformer-small-ctc").is_none());
    }

    #[test]
    fn engine_category_from_str_maps_five_types() {
        assert_eq!(engine_category_from_str("whisper"), Some(EngineCategory::Whisper));
        assert_eq!(engine_category_from_str("sensevoice"), Some(EngineCategory::SenseVoice));
        assert_eq!(engine_category_from_str("paraformer"), Some(EngineCategory::Paraformer));
        assert_eq!(engine_category_from_str("qwen3-asr"), Some(EngineCategory::Qwen3Asr));
        assert_eq!(engine_category_from_str("zipformer"), Some(EngineCategory::Zipformer));
        assert_eq!(engine_category_from_str("aliyun"), None);
    }
}
