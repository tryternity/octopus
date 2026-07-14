//! 模型管理页 Tauri 命令：列出可下载模型 + 下载（先探查）+ 完整性复核。
//!
//! v2（2026-06-22）就绪逻辑重构：
//! - 列表直读 DB（`list_all_local_asr_models`，**不过滤 is_enabled**），按 is_enabled 显示就绪。
//! - 点下载先 `resolve_model_dir` 探查：命中则自举 sha256 清单 + 置 true（**不重下**）；
//!   未命中才下载，下载后自举 + 置 true。
//! - `verify_model` 按 secret_key 清单复核，损坏置 false。
//! - is_enabled 语义 = 文件就绪可用；写 DB 后 `reload_models_config` 让引擎下拉即时更新。
//!
//! manifest（文件清单 + sha256）逻辑下沉到 `octopus_asr_local::manifest`，与 cli `sync-models` 共用。
//! 复用阶段1 download crate（HfRequest/resolve_tasks/Downloader）和 resolve_model_dir。

use serde::{Serialize, Deserialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use octopus_asr_local::manifest::{bootstrap_manifest, verify_against_manifest, Manifest};

use crate::runtime_config::SharedRuntimeConfig;

/// 一个可下载模型的列表项。
#[derive(Serialize)]
pub struct DownloadableModel {
    /// 引擎裸名（DB models.model_name）。
    pub name: String,
    /// HF repo（source），如 csukuangfj/sherpa-onnx-streaming-paraformer-zh。
    pub repo: String,
    /// 引擎族标签（DB category，如 paraformer/whisper/moonshine）。
    pub category: String,
    /// DB 中的描述（含尺寸信息）。
    pub description: String,
    /// 是否就绪（DB is_enabled）：true=文件完备可用，false=未就绪/未下载。
    pub is_enabled: bool,
}

/// 完整性复核结果。
#[derive(Serialize)]
pub struct VerifyResult {
    pub ok: bool,
    /// true=本次新自举生成了清单（之前 secret_key 为空）。
    pub bootstrapped: bool,
    /// 损坏/缺失的文件相对路径。
    pub broken_files: Vec<String>,
    pub message: String,
}

/// 列出所有可下载的本地模型（含未就绪，按 is_enabled 显示就绪/下载）。
/// domain 参数："asr"（默认）| "translate" | "ocr"。
#[tauri::command]
pub fn list_downloadable_models(domain: Option<String>) -> Result<Vec<DownloadableModel>, String> {
    let domain = domain.unwrap_or_else(|| "asr".to_string());
    let rows = octopus_infra::db::list_local_models_by_domain(&domain)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        // 文件系统实际检查：目录存在 → is_enabled=true（覆盖 DB 未更新的情况）
        let is_ready = r.is_enabled || octopus_asr_local::config::resolve_model_dir(&r.source).is_ok();
        out.push(DownloadableModel {
            name: r.model_name,
            repo: r.source,
            category: r.category,
            description: r.description,
            is_enabled: is_ready,
        });
    }
    Ok(out)
}

/// 对 URL 字符串做 `{key}` → value 模板替换（读 DB env 变量）。
fn resolve_env_template(url: &str) -> String {
    let vars = match octopus_infra::db::list_env_vars() {
        Ok(v) => v,
        Err(_) => return url.to_string(),
    };
    let mut result = url.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, &value);
    }
    result
}

/// 设置下载镜像（写运行时 AppConfig + 持久化 DB）。
#[tauri::command]
pub fn set_download_mirror(value: String, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let mut cfg = rc.read().clone();
    cfg.download_mirror = value;
    *rc.write() = cfg.clone();
    octopus_infra::db::save_app_config(&cfg).map_err(|e| e.to_string())?;
    Ok(())
}

/// 下载 HF 模型（先探查）：命中则自举清单+置 true 不重下；未命中才下载后自举+置 true。
#[tauri::command]
pub async fn download_model(
    repo: String,
    _rc: State<'_, SharedRuntimeConfig>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // 1. 探查：文件已就绪（如用户 hf-cli 下过、在 cache）→ 自举清单 + 置 true，不重下。
    if let Ok(dir) = octopus_asr_local::config::resolve_model_dir(&repo) {
        // bootstrap_manifest 计算 SHA-256（230-740MB），移入 spawn_blocking
        let repo_clone = repo.clone();
        let manifest = tokio::task::spawn_blocking(move || bootstrap_manifest(&dir))
            .await
            .map_err(|e| format!("bootstrap 任务异常: {}", e))?
            .map_err(|e| format!("生成校验清单失败: {e:?}"))?;
        apply_model_state(&repo_clone, Some(&manifest), true)?;
        let _ = app_handle.emit(
            "download-done",
            serde_json::json!({ "repo": &repo, "already_ready": true }),
        );
        return Ok(());
    }

    // 2. 未命中：下载（复用阶段1 download crate）。
    // 变量模板替换：repo 中的 {huggingface} 等替换为 env 变量实际值
    let resolved_repo = resolve_env_template(&repo);
    let target_dir = octopus_infra::paths::octopus_config_home().join("models");
    let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
        .map_err(|e| format!("初始化下载器失败: {e:?}"))?;
    let client = dl.client().clone();
    let hf_req = octopus_download::HfRequest {
        repo: resolved_repo,
        include: Vec::new(),
        exclude: Vec::new(),
        source_url: None,
        target_dir,
    };
    let tasks = octopus_download::resolve_tasks(&client, hf_req)
        .await
        .map_err(|e| format!("解析仓库 '{repo}' 失败: {e:?}"))?;
    if tasks.is_empty() {
        return Err(format!("仓库 '{repo}' 无可下载文件"));
    }
    let total_files = tasks.len();

    // 进度转发：download crate 的 mpsc Progress → Tauri 事件。
    let (tx, mut rx) = mpsc::channel::<octopus_download::Progress>(64);
    let fwd_handle = app_handle.clone();
    let fwd_repo = repo.clone();
    tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            let _ = fwd_handle.emit(
                "download-progress",
                serde_json::json!({
                    "repo": fwd_repo,
                    "downloaded": p.downloaded_bytes,
                    "total": p.total_bytes,
                    "speed": p.speed_bps,
                }),
            );
        }
    });

    for (i, task) in tasks.into_iter().enumerate() {
        let _ = app_handle.emit(
            "download-file",
            serde_json::json!({
                "repo": &repo,
                "index": i + 1,
                "total": total_files,
                "file": task.dest.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            }),
        );
        dl.download(&task, tx.clone(), None)
            .await
            .map_err(|e| format!("下载文件失败: {e:?}"))?;
    }
    drop(tx); // 关闭 channel → 转发 task 退出

    // 3. 下载完成：自举清单 + 置 true + reload。
    let dir = octopus_asr_local::config::resolve_model_dir(&repo)
        .map_err(|e| format!("下载后定位目录失败: {e:?}"))?;
    let repo_clone = repo.clone();
    let manifest = tokio::task::spawn_blocking(move || bootstrap_manifest(&dir))
        .await
        .map_err(|e| format!("bootstrap 任务异常: {}", e))?
        .map_err(|e| format!("生成校验清单失败: {e:?}"))?;
    apply_model_state(&repo_clone, Some(&manifest), true)?;
    let _ = app_handle.emit(
        "download-done",
        serde_json::json!({ "repo": &repo, "already_ready": false }),
    );
    Ok(())
}

/// 完整性复核：按 secret_key 清单 sha256 校验；清单空则自举；损坏置 false。
#[tauri::command]
pub async fn verify_model(model_name: String, repo: String) -> Result<VerifyResult, String> {
    // SHA-256 校验 230-740MB 模型文件是 CPU+IO 密集——移入 spawn_blocking 防阻塞 UI 线程
    tokio::task::spawn_blocking(move || verify_model_inner(model_name, &repo))
        .await
        .map_err(|e| format!("verify_model 任务异常: {}", e))?
}

fn verify_model_inner(model_name: String, repo: &str) -> Result<VerifyResult, String> {
    let dir = match octopus_asr_local::config::resolve_model_dir(repo) {
        Ok(d) => d,
        Err(_) => {
            return Ok(VerifyResult {
                ok: false,
                bootstrapped: false,
                broken_files: vec![],
                message: "模型未下载，请先下载".into(),
            });
        }
    };

    let secret_key = current_secret_key(&model_name)?;
    // 清单空 → 自举生成 + 确保置 true。
    if secret_key.trim().is_empty() {
        let manifest = bootstrap_manifest(&dir).map_err(|e| format!("生成清单失败: {e:?}"))?;
        apply_model_state(&repo, Some(&manifest), true)?;
        return Ok(VerifyResult {
            ok: true,
            bootstrapped: true,
            broken_files: vec![],
            message: "已生成校验清单，模型就绪".into(),
        });
    }

    // 清单非空 → 复核。
    let manifest: Manifest = serde_json::from_str(&secret_key)
        .map_err(|e| format!("校验清单解析失败（可重新下载修复）: {e:?}"))?;
    let broken = verify_against_manifest(&dir, &manifest);
    if broken.is_empty() {
        apply_model_state(&repo, None, true)?;
        Ok(VerifyResult {
            ok: true,
            bootstrapped: false,
            broken_files: vec![],
            message: "校验通过，模型就绪".into(),
        })
    } else {
        apply_model_state(&repo, None, false)?;
        Ok(VerifyResult {
            ok: false,
            bootstrapped: false,
            broken_files: broken.clone(),
            message: format!("{} 个文件损坏/缺失：{}", broken.len(), broken.join(", ")),
        })
    }
}

// ── 内部辅助 ──

/// 写 secret_key（可选）+ is_enabled + reload 运行时 AsrConfig 缓存。
fn apply_model_state(repo: &str, manifest_json: Option<&str>, enabled: bool) -> Result<(), String> {
    let model_name = match lookup_model_name(repo) {
        Ok(name) => name,
        Err(_) => {
            log::info!("[model_commands] {} 未在 models 表中找到，跳过 DB 状态更新", repo);
            return Ok(());
        }
    };
    if let Some(json) = manifest_json {
        octopus_infra::db::set_model_secret_key(&model_name, json).map_err(|e| e.to_string())?;
    }
    octopus_infra::db::set_model_enabled(&model_name, enabled).map_err(|e| e.to_string())?;
    // reload 只对 ASR 有意义（翻译/OCR 不走 AsrConfig）
    octopus_asr_local::config::reload_models_config();
    Ok(())
}

/// 由 source（路径标识）反查 model_name，搜索所有 domain。
fn lookup_model_name(source: &str) -> Result<String, String> {
    for domain in &["asr", "translate", "ocr"] {
        let rows = octopus_infra::db::list_local_models_by_domain(domain).map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.source == source) {
            return Ok(r.model_name.clone());
        }
    }
    Err(format!("未找到 source='{source}' 的模型"))
}

/// 读某模型当前 secret_key（DB），搜索所有 domain。
fn current_secret_key(model_name: &str) -> Result<String, String> {
    for domain in &["asr", "translate", "ocr"] {
        let rows = octopus_infra::db::list_local_models_by_domain(domain).map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.model_name == model_name) {
            return Ok(r.secret_key.clone());
        }
    }
    Err(format!("未找到模型 '{model_name}'"))
}

/// 删除本地模型：删除模型目录 + is_enabled=false + secret_key 清空。
#[tauri::command]
pub async fn delete_model(repo: String) -> Result<(), String> {
    let dir = octopus_asr_local::config::resolve_model_dir(&repo)
        .map_err(|e| format!("模型目录不存在: {e:?}"))?;

    // 如果是软链，只删软链不删 HF cache 原文件
    tokio::task::spawn_blocking(move || {
        let meta = std::fs::symlink_metadata(&dir);
        if let Ok(m) = meta {
            if m.is_symlink() || m.file_type().is_symlink() {
                std::fs::remove_file(&dir)
                    .map_err(|e| format!("删除软链失败: {e:?}"))?;
            } else {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("删除目录失败: {e:?}"))?;
            }
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("delete_model 任务异常: {e}"))??;

    apply_model_state(&repo, None, false)?;
    Ok(())
}

// ── 云端模型 CRUD ──

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudModelInput {
    pub domain: String,
    pub provider: String,
    pub category: String,
    pub model_name: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrCloudPreset {
    pub provider: String,
    pub category: String,
    pub models: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderPreset {
    pub provider: String,
    pub base_url: String,
}

#[tauri::command]
pub fn add_cloud_model(input: CloudModelInput) -> Result<i64, String> {
    octopus_infra::db::insert_cloud_model(
        &input.domain, &input.provider, &input.category,
        &input.model_name, &input.source, &input.secret_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn edit_cloud_model(id: i64, input: CloudModelInput) -> Result<(), String> {
    octopus_infra::db::update_cloud_model(
        id, &input.provider, &input.category,
        &input.model_name, &input.source, &input.secret_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_cloud_model(id: i64, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    // 查被删模型信息（用于检查是否为当前激活）
    let rows = octopus_infra::db::list_all_local_asr_models().unwrap_or_default();
    // 也查云端模型——用通用 list_local_models_by_domain 分别查
    let mut all_rows = rows;
    for domain in &["asr", "llm"] {
        if let Ok(r) = octopus_infra::db::list_local_models_by_domain(domain) {
            all_rows.extend(r);
        }
    }
    // 注意：上面只查 is_local=1 的，但云端模型 is_local=0。
    // 用直接查 app_config 不行——需要在 with_db 中查 models 表。
    // 简化：直接删，不检查激活状态（前端会在删除前提示用户）。
    octopus_infra::db::delete_cloud_model(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_asr_cloud_presets() -> Result<Vec<AsrCloudPreset>, String> {
    let rows = octopus_infra::db::list_asr_cloud_presets().map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|(provider, category, models_str)| {
        let models: Vec<String> = models_str.split(';').map(|s| s.to_string()).collect();
        AsrCloudPreset { provider, category, models }
    }).collect())
}

#[tauri::command]
pub fn list_llm_provider_presets() -> Result<Vec<LlmProviderPreset>, String> {
    let rows = octopus_infra::db::list_llm_provider_presets().map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|(provider, base_url)| {
        LlmProviderPreset { provider, base_url }
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试前确保 DB 初始化（desktop bin 测试不自动调 ensure_db）。
    fn ensure_test_db() {
        let _ = octopus_asr_local::db::ensure_db();
    }

    /// resolve_env_template 应将 {key} 替换为 DB env 变量值。
    #[test]
    fn resolve_env_template_replaces_placeholders() {
        ensure_test_db();
        let result = resolve_env_template("{huggingface}/org/repo/resolve/main/model.onnx");
        assert!(!result.contains("{huggingface}"), "模板变量应被替换");
        assert!(result.contains("/org/repo/resolve/main/model.onnx"), "非模板部分应保留");
    }

    /// resolve_env_template 对无模板的 URL 应原样返回。
    #[test]
    fn resolve_env_template_passthrough_no_placeholders() {
        let url = "https://example.com/model.onnx";
        let result = resolve_env_template(url);
        assert_eq!(result, url);
    }

    /// resolve_env_template 多个不同变量都替换。
    #[test]
    fn resolve_env_template_multiple_vars() {
        ensure_test_db();
        let url = "{huggingface}/repo/resolve/main/model.onnx and {github}/org/repo";
        let result = resolve_env_template(url);
        assert!(!result.contains("{huggingface}"));
        assert!(!result.contains("{github}"));
    }
}
