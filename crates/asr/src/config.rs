use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use octopus_infra::consts::{DEFAULT_ASR_MODEL_DIR, SILERO_VAD_PATH};
use octopus_infra::octopus_config_home;

// ── Model config schema（DB models 表）──
pub use octopus_infra::db::{AsrConfig, AsrSection, ModelEntry};

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

/// Resolve an engine name (e.g. "paraformer-bilingual") to its category
/// by looking up which section in DB contains it.
/// Returns `None` if the engine name is not found in any section.
pub fn resolve_engine_category(engine_name: &str) -> Option<EngineCategory> {
    let config = load_config().ok()?;

    if config
        .asr
        .whisper
        .as_ref()
        .map_or(false, |m| m.contains_key(engine_name))
    {
        return Some(EngineCategory::Whisper);
    }
    if config
        .asr
        .sensevoice
        .as_ref()
        .map_or(false, |m| m.contains_key(engine_name))
    {
        return Some(EngineCategory::SenseVoice);
    }
    if config
        .asr
        .paraformer
        .as_ref()
        .map_or(false, |m| m.contains_key(engine_name))
    {
        return Some(EngineCategory::Paraformer);
    }
    if config
        .asr
        .qwen3_asr
        .as_ref()
        .map_or(false, |m| m.contains_key(engine_name))
    {
        return Some(EngineCategory::Qwen3Asr);
    }
    if config
        .asr
        .zipformer
        .as_ref()
        .map_or(false, |m| m.contains_key(engine_name))
    {
        return Some(EngineCategory::Zipformer);
    }
    None
}

// ── List all available engines ──

/// 可用引擎条目
pub struct EngineInfo {
    pub name: String,
    pub category: EngineCategory,
    pub description: String,
    pub is_local: bool,
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

    // 按 category 排序，同 category 内按 name 排序
    engines.sort_by(|a, b| {
        let cat_order = |c: &EngineCategory| -> u8 {
            match c {
                EngineCategory::SenseVoice => 0,
                EngineCategory::Whisper => 1,
                EngineCategory::Paraformer => 2,
                EngineCategory::Qwen3Asr => 3,
                EngineCategory::Zipformer => 4,
            }
        };
        cat_order(&a.category)
            .cmp(&cat_order(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });

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
/// 解析规则：
/// - `asr_engine` 非空且在 DB `models` 表按 name 命中 → 用命中项
/// - `asr_engine` 为空 / 匹配不到任何模型 → 回退兜底引擎（zipformer-small-ctc）
///
/// 兜底级联：先从 DB `asr.zipformer` section 查 key `"zipformer-small-ctc"`
/// （用户手编 source 仍生效）；查不到再硬构造（靠 DEFAULT_ASR_MODEL_DIR 本地打包路径）。
///
/// 仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。
/// 显式 name 路径（cli `--model`、AsrEngineManager.switch_model、server 请求带 engine）
/// 直接走 `resolve_engine_category + pick_entry`，不经此函数、不走兜底。
pub fn resolve_active_engine(asr_engine: &str) -> Result<ResolvedEngine> {
    let cfg = load_config()?;

    // 1. 显式配置命中
    if !asr_engine.is_empty() {
        if let Some(category) = resolve_engine_category(asr_engine) {
            if let Some(entry) = pick_entry(&cfg, category, asr_engine) {
                return Ok(ResolvedEngine {
                    name: asr_engine.to_string(),
                    category,
                    entry: entry.clone(),
                });
            }
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
}
