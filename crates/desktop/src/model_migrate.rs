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
        let repos = extract_all_hf_repos_from_manifest(&manifest);
        // 多来源模型（opus-mt 双 repo / OCR 三 repo）不能单 repo 软链覆盖目标布局，
        // 跳过让 download_model 按 manifest 逐文件下载。
        if repos.len() != 1 {
            log::info!("[migrate] {} 有 {} 个来源，跳过软链（由 download_model 处理）",
                m.source, repos.len());
            continue;
        }
        if let Some(repo) = repos.into_iter().next() {
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

/// 从 manifest 解析所有不同的 HF repo（去重）。
/// 用于判断是否为单 repo 模型（可软链）还是多 repo（需逐文件下载）。
fn extract_all_hf_repos_from_manifest(manifest: &serde_json::Value) -> Vec<String> {
    let mut repos: Vec<String> = Vec::new();
    if let Some(obj) = manifest.as_object() {
        for (_path, meta) in obj {
            if let Some(source) = meta.get("source").and_then(|v| v.as_str()) {
                if let Some(repo) = parse_hf_repo_from_url(source) {
                    if !repos.contains(&repo) {
                        repos.push(repo);
                    }
                }
            }
        }
    }
    repos
}

/// 从单个 source URL 解析 HF repo（`{huggingface}/{owner}/{repo}/resolve/main/...`）。
fn parse_hf_repo_from_url(source: &str) -> Option<String> {
    let idx = source.find("/resolve/main/")?;
    let prefix = &source[..idx];
    // 去掉模板变量前缀（{*}）或 mirror host 前缀
    let repo_part = if let Some(brace) = prefix.rfind('}') {
        &prefix[brace + 1..]
    } else {
        // 已解析的 URL：去掉 https://host/ 前缀
        if let Some(slash) = prefix.find("://") {
            let after_scheme = &prefix[slash + 3..];
            if let Some(path_start) = after_scheme.find('/') {
                &after_scheme[path_start + 1..]
            } else {
                return None;
            }
        } else {
            prefix
        }
    };
    let parts: Vec<&str> = repo_part.split('/').collect();
    if parts.len() >= 2 {
        let n = parts.len();
        Some(format!("{}/{}", parts[n - 2], parts[n - 1]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_all_repos_single() {
        let json = serde_json::json!({
            "onnx/model.onnx": {
                "source": "{huggingface}/onnx-community/whisper-small.en/resolve/main/onnx/model.onnx",
                "sha256": "abc", "size": 123
            }
        });
        let repos = extract_all_hf_repos_from_manifest(&json);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0], "onnx-community/whisper-small.en");
    }

    #[test]
    fn extract_all_repos_multi() {
        let json = serde_json::json!({
            "zh-en/onnx/encoder.onnx": {
                "source": "{huggingface}/Xenova/opus-mt-zh-en/resolve/main/onnx/encoder.onnx",
                "sha256": "abc", "size": 123
            },
            "en-zh/onnx/encoder.onnx": {
                "source": "{huggingface}/Xenova/opus-mt-en-zh/resolve/main/onnx/encoder.onnx",
                "sha256": "def", "size": 456
            }
        });
        let repos = extract_all_hf_repos_from_manifest(&json);
        assert_eq!(repos.len(), 2);
        assert!(repos.contains(&"Xenova/opus-mt-zh-en".to_string()));
        assert!(repos.contains(&"Xenova/opus-mt-en-zh".to_string()));
    }

    #[test]
    fn extract_all_repos_github_only() {
        let json = serde_json::json!({
            "keys.txt": {
                "source": "{github}/PaddlePaddle/PaddleOCR/raw/main/dict.txt",
                "sha256": "abc", "size": 123
            }
        });
        let repos = extract_all_hf_repos_from_manifest(&json);
        assert!(repos.is_empty(), "GitHub-only manifest should have no HF repos");
    }

    #[test]
    fn extract_all_repos_resolved_url() {
        let json = serde_json::json!({
            "model.onnx": {
                "source": "https://hf-mirror.com/csukuangfj/sherpa-onnx-xxx/resolve/main/model.onnx",
                "sha256": "abc", "size": 123
            }
        });
        let repos = extract_all_hf_repos_from_manifest(&json);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0], "csukuangfj/sherpa-onnx-xxx");
    }
}
