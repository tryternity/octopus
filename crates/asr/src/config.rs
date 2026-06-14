use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use octopus_infra::consts::SILERO_VAD_PATH;
use octopus_infra::octopus_config_home;

// ── Model config schema（DB models 表）──

/// 模型配置顶层结构（对应 DB models 表 domain='asr'；由 db::load_models 构造）
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub asr: AsrSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AsrSection {
    pub active: String,
    pub whisper: Option<HashMap<String, ModelEntry>>,
    pub sensevoice: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub paraformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default, rename = "qwen3-asr")]
    pub qwen3_asr: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub zipformer: Option<HashMap<String, ModelEntry>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModelEntry {
    pub source: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub description: String,
    /// Quantization preference: "int8" (default) or "fp32".
    /// Controls which ONNX file variant is loaded when multiple versions exist.
    #[serde(default)]
    pub quantization: String,
}

// ── Config loading ──

/// 运行时缓存：首次从 DB 读出后缓存，避免每次识别重复开连接查询。
/// 手编 DB models 表后需重启进程生效（与历史行为一致）。
static RUNTIME_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// 读取模型配置（唯一来源：~/.octopus/octopus.db 的 models 表）。
/// 首次调用 ensure_db（自动建表 + seed 默认引擎），读出后缓存到 OnceLock。
/// cli/server/desktop 三端统一走此路径，不再读 model.json。
pub fn load_config() -> Result<AppConfig> {
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

// ── App config (config.yaml) ──

/// config.yaml — application settings
#[derive(Debug, Deserialize, Default)]
pub struct AppYamlConfig {
    #[serde(default)]
    pub microphone: String,
}

/// 读取 ~/.octopus/config.yaml，文件不存在则返回默认值
pub fn load_app_config() -> Result<AppYamlConfig> {
    let config_path = octopus_config_home().join("config.yaml");
    if !config_path.exists() {
        return Ok(AppYamlConfig::default());
    }
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let config: AppYamlConfig = serde_yaml::from_str(&text)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;
    Ok(config)
}
