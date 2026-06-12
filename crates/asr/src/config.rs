use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Global base dir ──

/// $HOME/.octopus — 全局根目录，所有配置和模型都基于此
static HANDY_HOME: Lazy<PathBuf> = Lazy::new(|| {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".octopus")
});

/// 获取 ~/.octopus 路径
pub fn handy_home() -> &'static Path {
    HANDY_HOME.as_path()
}

// ── Model config schema (model.json) ──

/// model.json 顶层结构
#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub vad: Option<VadSection>,
    pub asr: AsrSection,
}

#[derive(Debug, Deserialize)]
pub struct VadSection {
    pub active: String,
    pub silero: Option<HashMap<String, SimpleModelEntry>>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct SimpleModelEntry {
    pub source: String,
    #[serde(default)]
    pub description: String,
}

// ── Config loading ──

/// 读取 ~/.octopus/model.json
pub fn load_config() -> Result<AppConfig> {
    let config_path = handy_home().join("model.json");
    if !config_path.exists() {
        anyhow::bail!(
            "Model config not found at {}. Please create it.",
            config_path.display()
        );
    }
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let config: AppConfig = serde_json::from_str(&text)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;
    Ok(config)
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

// ── VAD model discovery ──

/// 定位 Silero VAD 模型
/// 1. 从 model.json vad.active 对应的 source 查找 HF 缓存（active 非空时）
/// 2. Fallback 到 ~/.octopus/models/ 下 of the VAD 模型
pub fn find_silero_vad() -> Result<PathBuf> {
    // 1. 从 config 读取 active VAD 模型（active 为空则跳过）
    if let Ok(config) = load_config() {
        if let Some(vad_cfg) = &config.vad {
            if !vad_cfg.active.is_empty() {
                if let Some(silero_map) = &vad_cfg.silero {
                    if let Some(entry) = silero_map.get(&vad_cfg.active) {
                        if let Ok(hf_path) = find_hf_cache(&entry.source) {
                            let onnx_dir = find_onnx_dir(&hf_path);
                            for name in ["silero_vad_v4.onnx", "model.onnx", "model_int8.onnx"] {
                                let p = onnx_dir.join(name);
                                if p.exists() {
                                    return Ok(p);
                                }
                            }
                            if let Ok(entries) = std::fs::read_dir(&onnx_dir) {
                                for e in entries.flatten() {
                                    let n = e.file_name().to_string_lossy().to_string();
                                    if n.ends_with(".onnx") {
                                        return Ok(e.path());
                                    }
                                }
                            }
                            eprintln!("Warning: VAD model from config not found at {}, falling back to default", hf_path.display());
                        }
                    }
                }
            }
        }
    }

    // 2. Fallback: ~/.octopus/models/silero_vad_v4.onnx
    let default_vad = handy_home().join("models/silero_vad_v4.onnx");
    if default_vad.exists() {
        return Ok(default_vad);
    }

    anyhow::bail!(
        "Silero VAD model not found. Checked:\n  Config HF cache\n  {}",
        default_vad.display()
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

/// ASR engine category, determined by which section of model.json contains the engine name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineCategory {
    Whisper,
    SenseVoice,
    Paraformer,
    Qwen3Asr,
    Zipformer,
}

/// Resolve an engine name (e.g. "paraformer-bilingual") to its category
/// by looking up which section in model.json contains it.
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

/// 从 model.json 中列出所有已配置的 ASR 引擎
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
    let config_path = handy_home().join("config.yaml");
    if !config_path.exists() {
        return Ok(AppYamlConfig::default());
    }
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let config: AppYamlConfig = serde_yaml::from_str(&text)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;
    Ok(config)
}
