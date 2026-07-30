use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use octopus_infra::paths::octopus_config_home;

/// 根据 HF source（如 "onnx-community/whisper-small"）定位到本地缓存路径
pub fn find_hf_cache(source: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let model_dir_name = source.replace('/', "--");
    let model_dir = PathBuf::from(home)
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", model_dir_name));

    if !model_dir.exists() {
        anyhow::bail!(
            "模型 '{}' 未在 ~/.octopus/models/ 或 HF cache 找到。请运行 `octopus-cli download {}` 下载。",
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

/// 前 3 级模型目录查找（基于给定 octopus_home，可单测；不依赖全局 `$HOME`）。
///
/// 1. `octopus_home/<source>`（随包小模型，如 `models/vad.onnx`，见 `VAD_OVERRIDE_PATH`）
/// 2. 绝对路径（`source` 本身是绝对路径）
/// 3. `octopus_home/models/<source>`（download 下的 HF 模型，source 如 `onnx-community/whisper-small`）
///
/// 返回 `None` 表示前 3 级全 miss，调用方应回退第 4 级 HF cache（`find_hf_cache`）。
fn resolve_local_in(source: &str, octopus_home: &Path) -> Option<PathBuf> {
    // 1. octopus_home 下相对路径（随应用打包的小模型）
    let local = octopus_home.join(source);
    if local.is_dir() {
        return Some(local);
    }
    // 2. 绝对路径（join 绝对路径会覆盖 base，等效直接判断 source 本身）
    let abs = PathBuf::from(source);
    if abs.is_dir() {
        return Some(abs);
    }
    // 3. download 下的 HF 模型（~/.octopus/models/<source>）
    let downloaded = octopus_home.join("models").join(source);
    if downloaded.is_dir() {
        return Some(downloaded);
    }
    None
}

/// 解析模型目录：前 3 级本地查找（随包 / 绝对路径 / download 下载），回退 HF 缓存。
/// - source 为 domain/name 路径标识（如 "asr/zipformer-small"）→ ~/.octopus/models/<source>
/// - source 为绝对路径 → 直接用
/// - source 为 HF repo 名（如 "onnx-community/whisper-small"）→ 优先 ~/.octopus/models/<source>（download 下到这里），
///   否则 find_hf_cache（兼容已用 hf-cli 下的 ~/.cache/huggingface）
pub fn resolve_model_dir(source: &str) -> Result<PathBuf> {
    if let Some(p) = resolve_local_in(source, octopus_config_home()) {
        return Ok(p);
    }
    find_hf_cache(source)
}

/// 取 HF cache 中最新 snapshot 目录
pub fn find_latest_snapshot(model_dir: &Path) -> Result<PathBuf> {
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
