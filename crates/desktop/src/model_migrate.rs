//! 模型路径迁移：为已下载到 HF cache 的模型创建
//! `~/.octopus/models/{domain}/{name}/` → HF cache snapshot 的软链。
//!
//! DB v28 迁移后，模型 source 从 HF repo 改为路径标识（`asr/whisper-small`）。
//! 但实际文件仍在 HF cache 中。desktop 启动时幂等创建软链。

use anyhow::Result;

/// 幂等创建：读 DB 中所有 is_local=1 模型，
/// 从 manifest 解析 HF repo → 在 HF cache 找 snapshot → 创建软链。
pub fn create_model_symlinks() -> Result<()> {
    let models = match octopus_infra::db::list_all_local_asr_models() {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("list_all_local_asr_models 失败: {e:?}");
            return Ok(());
        }
    };
    // 也处理 translate/ocr domain
    let mut all_models = models;
    for domain in &["translate", "ocr"] {
        if let Ok(rows) = octopus_infra::db::list_local_models_by_domain(domain) {
            all_models.extend(rows);
        }
    }
    let base = octopus_infra::paths::octopus_config_home().join("models");

    for m in &all_models {
        if m.source.is_empty() || m.secret_key.is_empty() {
            continue;
        }
        let dest = base.join(&m.source);
        if dest.exists() {
            continue;
        }

        let manifest: serde_json::Value = match serde_json::from_str(&m.secret_key) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(repo) = extract_hf_repo_from_manifest(&manifest) {
            if let Ok(snapshot_dir) = octopus_asr_local::config::find_hf_cache(&repo) {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&snapshot_dir, &dest).ok();
                log::info!("[migrate] 软链 {} → {}", dest.display(), snapshot_dir.display());
            }
        }
    }
    Ok(())
}

/// 从 manifest 的 source URL 解析 HF repo（`{huggingface}/{owner}/{repo}/resolve/main/...`）。
fn extract_hf_repo_from_manifest(manifest: &serde_json::Value) -> Option<String> {
    let obj = manifest.as_object()?;
    for (_path, meta) in obj {
        let source = meta.get("source")?.as_str()?;
        if let Some(idx) = source.find("/resolve/main/") {
            let prefix = &source[..idx];
            // 去掉模板变量前缀（{*}）或 mirror host 前缀
            let repo_part = if let Some(brace) = prefix.rfind('}') {
                &prefix[brace + 1..]
            } else {
                // 已解析的 URL：去掉 https://host/ 前缀
                if let Some(slash) = prefix.find("://") {
                    let after_scheme = &prefix[slash + 3..];
                    // 跳过 host，取 path 的前两段（owner/repo）
                    if let Some(path_start) = after_scheme.find('/') {
                        &after_scheme[path_start + 1..]
                    } else {
                        continue;
                    }
                } else {
                    prefix
                }
            };
            let parts: Vec<&str> = repo_part.split('/').collect();
            if parts.len() >= 2 {
                let n = parts.len();
                return Some(format!("{}/{}", parts[n - 2], parts[n - 1]));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_repo_from_simple_manifest() {
        let json = serde_json::json!({
            "onnx/model.onnx": {
                "source": "{huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/model.onnx",
                "sha256": "abc", "size": 123
            }
        });
        assert_eq!(
            extract_hf_repo_from_manifest(&json),
            Some("onnx-community/whisper-small.en".to_string())
        );
    }

    #[test]
    fn extract_repo_from_multi_source_manifest() {
        let json = serde_json::json!({
            "zh-en/onnx/encoder.onnx": {
                "source": "{huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/encoder.onnx",
                "sha256": "abc", "size": 123
            }
        });
        assert_eq!(
            extract_hf_repo_from_manifest(&json),
            Some("Xenova/opus-mt-zh-en".to_string())
        );
    }

    #[test]
    fn extract_repo_returns_none_for_github_only() {
        let json = serde_json::json!({
            "keys.txt": {
                "source": "{github}/PaddlePaddle/PaddleOCR/raw/main/dict.txt",
                "sha256": "abc", "size": 123
            }
        });
        assert_eq!(extract_hf_repo_from_manifest(&json), None);
    }

    #[test]
    fn extract_repo_from_resolved_url() {
        let json = serde_json::json!({
            "model.onnx": {
                "source": "https://hf-mirror.com/csukuangfj/sherpa-onnx-xxx/resolve/main/model.onnx",
                "sha256": "abc", "size": 123
            }
        });
        assert_eq!(
            extract_hf_repo_from_manifest(&json),
            Some("csukuangfj/sherpa-onnx-xxx".to_string())
        );
    }
}
