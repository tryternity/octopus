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
#[cfg(test)]
fn resolve_env_template(url: &str) -> String {
    let vars = load_env_vars();
    resolve_env_template_with(url, &vars)
}

/// 加载 DB 中的 env 变量（category='env'），返回 (key, value) 列表。
fn load_env_vars() -> Vec<(String, String)> {
    octopus_infra::db::list_env_vars().unwrap_or_default()
}

/// 用给定的 env 变量列表做模板替换。
fn resolve_env_template_with(url: &str, vars: &[(String, String)]) -> String {
    let mut result = url.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, value);
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

/// 下载模型（manifest 驱动）：先探查文件是否已就绪；未命中则读 secret_key manifest
/// 逐文件按 source URL 下载 + sha256 校验 → 置 is_enabled=true。
#[tauri::command]
pub async fn download_model(
    repo: String,
    _rc: State<'_, SharedRuntimeConfig>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // 1. 探查：文件已就绪（如用户 hf-cli 下过、在 cache、或软链）→ 置 true，不重下。
    //    secret_key 非空时保留原值（含预填下载源）；空才 bootstrap 生成校验清单。
    if let Ok(dir) = octopus_asr_local::config::resolve_model_dir(&repo) {
        let existing_key = current_secret_key_for_source(&repo);
        if existing_key.is_empty() {
            // 无 manifest → bootstrap 生成
            let repo_clone = repo.clone();
            let manifest = tokio::task::spawn_blocking(move || bootstrap_manifest(&dir))
                .await
                .map_err(|e| format!("bootstrap 任务异常: {}", e))?
                .map_err(|e| format!("生成校验清单失败: {e:?}"))?;
            apply_model_state(&repo_clone, Some(&manifest), true)?;
        } else {
            // 已有 manifest → 只置 is_enabled=true，保留原 secret_key
            apply_model_state(&repo, None, true)?;
        }
        let _ = app_handle.emit(
            "download-done",
            serde_json::json!({ "repo": &repo, "already_ready": true }),
        );
        return Ok(());
    }

    // 2. 未命中：读 DB manifest，逐文件按 source URL 下载。
    let (model_name, secret_key) = lookup_model_by_source(&repo)?;
    if secret_key.is_empty() {
        return Err(format!("模型 '{repo}' 无下载清单（secret_key 为空）"));
    }
    let manifest: Manifest = serde_json::from_str(&secret_key)
        .map_err(|e| format!("manifest 解析失败: {e:?}"))?;
    if manifest.is_empty() {
        return Err(format!("模型 '{repo}' 下载清单为空"));
    }

    // 3. 解析 {huggingface} / {github} 模板变量
    let env_vars = load_env_vars();

    // 4. 目标目录：~/.octopus/models/{repo}/
    let dest_base = octopus_infra::paths::octopus_config_home().join("models").join(&repo);

    // 5. 逐文件下载
    let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
        .map_err(|e| format!("初始化下载器失败: {e:?}"))?;

    let total_files = manifest.len();
    let (tx, mut rx) = mpsc::channel::<octopus_download::Progress>(64);

    // 进度转发
    let fwd_handle = app_handle.clone();
    let fwd_repo = repo.clone();
    tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            let _ = fwd_handle.emit(
                "download-progress",
                serde_json::json!({
                    "repo": &fwd_repo,
                    "downloaded": p.downloaded_bytes,
                    "total": p.total_bytes,
                    "speed": p.speed_bps,
                }),
            );
        }
    });

    for (i, (path, file)) in manifest.iter().enumerate() {
        let _ = app_handle.emit(
            "download-file",
            serde_json::json!({
                "repo": &repo,
                "index": i + 1,
                "total": total_files,
                "file": path,
            }),
        );

        // 解析模板变量
        let url = resolve_env_template_with(&file.source, &env_vars);
        let dest = dest_base.join(path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        let task = octopus_download::DownloadTask {
            url: url.clone(),
            mirrors: vec![],
            dest: dest.clone(),
            expected_hash: if file.sha256.is_empty() { None }
                else { Some(octopus_download::Hash::Sha256(file.sha256.clone())) },
        };

        let (prog_tx, mut prog_rx) = mpsc::channel::<octopus_download::Progress>(64);
        let tx2 = tx.clone();
        tokio::spawn(async move {
            while let Some(p) = prog_rx.recv().await {
                let _ = tx2.send(p).await;
            }
        });

        dl.download(&task, prog_tx, None)
            .await
            .map_err(|e| format!("下载 {path} 失败: {e:?}"))?;
    }
    drop(tx);

    // 6. 置 is_enabled=true + emit done
    apply_model_state(&repo, None, true)?;
    let _ = app_handle.emit(
        "download-done",
        serde_json::json!({ "repo": &repo, "already_ready": false }),
    );
    let _ = model_name; // 避免 unused warning
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

/// 按 source（路径标识）反查 model_name + secret_key，搜索所有 domain。
fn lookup_model_by_source(source: &str) -> Result<(String, String), String> {
    for domain in &["asr", "translate", "ocr"] {
        let rows = octopus_infra::db::list_local_models_by_domain(domain).map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.source == source) {
            return Ok((r.model_name.clone(), r.secret_key.clone()));
        }
    }
    Err(format!("未找到 source='{source}' 的模型"))
}

/// 按 source（路径标识）查 secret_key，搜索所有 domain。找不到返回空串。
fn current_secret_key_for_source(source: &str) -> String {
    lookup_model_by_source(source)
        .map(|(_, key)| key)
        .unwrap_or_default()
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
    pub models: Vec<String>,
}

#[tauri::command]
pub async fn add_cloud_model(input: CloudModelInput) -> Result<i64, String> {
    // LLM 模型：后端先测试连接，通过才保存（is_enabled=1），失败返回错误（is_enabled=0 不入库）
    if input.domain == "llm" && !input.model_name.is_empty() {
        let test = test_llm_connection(&input.source, &input.secret_key, &input.model_name, input.is_thinking).await;
        if !test.ok {
            return Err(format!("模型测试失败，无法保存：{}", test.message));
        }
    }
    octopus_infra::db::insert_cloud_model(
        &input.domain, &input.provider, &input.category,
        &input.model_name, &input.source, &input.secret_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn edit_cloud_model(id: i64, input: CloudModelInput) -> Result<(), String> {
    // LLM 模型：后端先测试连接，通过才更新
    if input.domain == "llm" && !input.model_name.is_empty() {
        // secret_key 为空表示编辑时未改 key，从 DB 取真实 key 测试
        let real_key = if input.secret_key.is_empty() {
            octopus_infra::db::get_model_source_key(id)
                .map(|(_, k)| k)
                .unwrap_or_default()
        } else {
            input.secret_key.clone()
        };
        let test = test_llm_connection(&input.source, &real_key, &input.model_name, input.is_thinking).await;
        if !test.ok {
            return Err(format!("模型测试失败，无法保存：{}", test.message));
        }
    }
    octopus_infra::db::update_cloud_model(
        id, &input.provider, &input.category,
        &input.model_name, &input.source, &input.secret_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_cloud_model(id: i64) -> Result<(), String> {
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
    Ok(rows.into_iter().map(|r| {
        LlmProviderPreset { provider: r.provider, base_url: r.base_url, models: r.models }
    }).collect())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub ok: bool,
    pub message: String,
}

/// 内部 LLM 连接测试（发 chat completion + thinking disable）。
async fn test_llm_connection(source: &str, secret_key: &str, model_name: &str, is_thinking: bool) -> TestConnectionResult {
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", source.trim_end_matches('/'));
    let mut body = serde_json::json!({
        "model": model_name,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });
    if is_thinking {
        body["thinking"] = serde_json::json!({"type": "disabled"});
        body["enable_thinking"] = serde_json::json!(false);
    }
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", secret_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;

    match resp {
        Ok(r) => {
            if r.status().is_success() {
                TestConnectionResult { ok: true, message: format!("模型 {} 连接成功", model_name) }
            } else {
                let status = r.status().as_u16();
                let body = r.text().await.unwrap_or_default();
                let msg = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(String::from))
                    .unwrap_or(body);
                TestConnectionResult { ok: false, message: format!("HTTP {} — {}", status, msg) }
            }
        }
        Err(e) => TestConnectionResult { ok: false, message: format!("{}", e) },
    }
}

/// 测试云端模型连接（前端「测试连接」按钮调用）。
#[tauri::command]
pub async fn test_cloud_model(
    source: String,
    secret_key: String,
    model_name: Option<String>,
    is_thinking: Option<bool>,
) -> Result<TestConnectionResult, String> {
    if let Some(model) = model_name.filter(|m| !m.is_empty()) {
        return Ok(test_llm_connection(&source, &secret_key, &model, is_thinking.unwrap_or(false)).await);
    }
    // 无 model_name：只验证连通性
    let url = format!("{}/models", source.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", secret_key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    match resp {
        Ok(r) => {
            if r.status().is_success() {
                Ok(TestConnectionResult { ok: true, message: "连接成功".into() })
            } else {
                Ok(TestConnectionResult { ok: false, message: format!("HTTP {}", r.status().as_u16()) })
            }
        }
        Err(e) => Ok(TestConnectionResult { ok: false, message: format!("{}", e) }),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDetail {
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

/// 按 id 查模型详情（source + 真实 secret_key，用于编辑表单回填 + 连接测试）。
#[tauri::command]
pub fn get_model_detail(id: i64) -> Result<ModelDetail, String> {
    let (source, secret_key) = octopus_infra::db::get_model_source_key(id).map_err(|e| e.to_string())?;
    let (is_streaming, is_thinking) = octopus_infra::db::get_model_flags(id).unwrap_or((false, false));
    Ok(ModelDetail {
        source,
        secret_key,
        is_streaming,
        is_thinking,
    })
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
