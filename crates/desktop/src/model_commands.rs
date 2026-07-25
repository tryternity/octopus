//! 模型管理页 Tauri 命令：列出可下载模型 + 下载（先探查）+ 完整性复核。
//!
//! v2（2026-06-22）就绪逻辑重构：
//! - 列表直读 DB（`list_all_local_asr_models`，**不过滤 is_enabled**），按 is_available 显示就绪。
//! - 点下载先 `resolve_model_dir` 探查：命中则自举 sha256 清单 + 置 true（**不重下**）；
//!   未命中才下载，下载后自举 + 置 true。
//! - `verify_model` 按 secret_key 清单复核，损坏置 false。
//! - Task 2 后：is_available 表「文件就绪可用」；is_enabled 表「激活」（每域仅 1 个=1）。
//!   写 DB（set_model_available → is_available）后 `reload_models_config` 让引擎下拉即时更新。
//!
//! manifest（文件清单 + sha256）逻辑下沉到 `octopus_asr_local::manifest`，与 cli `sync-models` 共用。
//! 复用 octopus-download crate（Downloader/DownloadTask/Progress/Hash）和 resolve_model_dir。

use serde::{Serialize, Deserialize};
use std::path::Path;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use octopus_asr_local::manifest::{bootstrap_manifest, Manifest};

use crate::runtime_config::SharedRuntimeConfig;

/// 一个可下载模型的列表项。
#[derive(Serialize)]
pub struct DownloadableModel {
    /// DB 行 id（switch_active_model 按 id 切激活）。
    pub id: i64,
    /// 引擎裸名（DB models.model_name）。
    pub name: String,
    /// HF repo（source），如 csukuangfj/sherpa-onnx-streaming-paraformer-zh。
    pub repo: String,
    /// 引擎族标签（DB category，如 paraformer/whisper/moonshine）。
    pub category: String,
    /// DB 中的描述（含尺寸信息）。
    pub description: String,
    /// 是否就绪（DB is_available）：true=文件完备可用，false=未就绪/未下载。
    pub is_available: bool,
    /// 是否激活（DB is_enabled）：每域仅 1 个=1。前端标 current 用。
    pub is_enabled: bool,
    /// 模型来源: 0=builtin 1=local 2=cloud。前端按此区分行为（builtin 不可删等）。
    pub source_type: i64,
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

/// 列出所有可下载的本地模型（含未就绪，按 is_available 显示就绪/下载）。
/// domain 参数："asr"（默认）| "translate" | "ocr"。
#[tauri::command]
pub fn list_downloadable_models(domain: Option<String>) -> Result<Vec<DownloadableModel>, String> {
    let domain = domain.unwrap_or_else(|| "asr".to_string());
    let rows = octopus_infra::db::list_local_models_by_domain(&domain)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        // is_available 由 sync_builtin_models_availability（启动时 sha256 校验）保证准确，
        // 不再用 resolve_model_dir 覆盖——目录存在但文件损坏时 is_available 应为 false。
        out.push(DownloadableModel {
            id: r.id,
            name: r.model_name,
            repo: r.source,
            category: r.category,
            description: r.description,
            is_available: r.is_available,
            is_enabled: r.is_enabled,
            source_type: r.source_type,
        });
    }
    Ok(out)
}

/// 模型文件信息（供 DownloadPopover 展示文件级列表 + 进度）。
#[derive(Serialize)]
pub struct ModelFile {
    /// manifest 里的相对路径（如 "model.int8.onnx"）
    pub path: String,
    /// 文件大小（字节，来自 manifest）
    pub size: u64,
    /// 文件存在且 sha256 校验通过 = true
    pub exists: bool,
}

/// sidecar 缓存：模型文件 SHA256 校验结果（避免每次 hover 都读整个文件算 hash）。
/// 存在模型目录下的 `.verified.json`，记录每文件的 size + mtime + sha256。
/// 后续校验先 stat() 比对 size+mtime（微秒级），不匹配才算 SHA256。
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(crate) struct VerifiedCache {
    files: std::collections::HashMap<String, VerifiedEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct VerifiedEntry {
    size: u64,
    mtime: u64,
    sha256: String,
}

/// 读模型目录下的 .verified.json 缓存。
pub(crate) fn load_verified_cache(dir: &Path) -> VerifiedCache {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".verified.json")).unwrap_or_default())
        .unwrap_or_default()
}

/// 写 .verified.json 缓存。
pub(crate) fn save_verified_cache(dir: &Path, cache: &VerifiedCache) {
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = std::fs::write(dir.join(".verified.json"), json);
    }
}

/// 校验单文件是否存在且完好——优先用 sidecar 缓存（stat 快检），不匹配才算 SHA256。
pub(crate) fn check_file_with_cache(dir: &Path, path: &str, expected_sha256: &str, cache: &mut VerifiedCache) -> bool {
    let full = dir.join(path);
    let meta = match std::fs::metadata(&full) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // size 快检——不匹配直接 false（不用算 hash）
    if size == 0 {
        return false;
    }

    // sidecar 快检——size + mtime 匹配则跳过 SHA256（文件没变过）
    if let Some(entry) = cache.files.get(path) {
        if entry.size == size && entry.mtime == mtime && entry.sha256 == expected_sha256 {
            return true;
        }
    }

    // 缓存未命中或不匹配——读文件算 SHA256
    let ok = octopus_asr_local::manifest::verify_file_sha256(&full, expected_sha256);
    if ok {
        cache.files.insert(path.to_string(), VerifiedEntry {
            size,
            mtime,
            sha256: expected_sha256.to_string(),
        });
    }
    ok
}

/// 列出某模型的所有文件（manifest 解析 + 逐文件校验，sidecar 缓存加速）。
/// 供 DownloadPopover 浮层展示文件级列表 + 「已存在=100%」状态。
///
/// 首次校验需读文件算 SHA256（26MB ~百毫秒），后续校验走 stat 快检（微秒级）。
#[tauri::command]
pub async fn list_model_files(repo: String) -> Result<Vec<ModelFile>, String> {
    tokio::task::spawn_blocking(move || {
        let (model_name, secret_key) = lookup_model_by_source(&repo)?;
        let _ = model_name;
        if secret_key.is_empty() {
            return Err(format!("模型 '{repo}' 无下载清单（secret_key 为空）"));
        }
        let manifest: Manifest = serde_json::from_str(&secret_key)
            .map_err(|e| format!("manifest 解析失败: {e:?}"))?;

        let dir = octopus_asr_local::config::resolve_model_dir(&repo).ok();

        let result: Vec<ModelFile> = if let Some(ref dir) = dir {
            let mut cache = load_verified_cache(dir);
            let out: Vec<ModelFile> = manifest
                .iter()
                .map(|(path, file)| {
                    let exists = check_file_with_cache(dir, path, &file.sha256, &mut cache);
                    ModelFile {
                        path: path.clone(),
                        size: file.size,
                        exists,
                    }
                })
                .collect();
            save_verified_cache(dir, &cache);
            out
        } else {
            manifest
                .iter()
                .map(|(path, file)| ModelFile { path: path.clone(), size: file.size, exists: false })
                .collect()
        };

        Ok(result)
    })
    .await
    .map_err(|e| format!("list_model_files 任务异常: {e}"))?
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

/// 下载模型（manifest 驱动）：先探查文件是否已就绪；未命中或损坏则读 secret_key manifest
/// 逐文件按 source URL 下载 + sha256 校验 → 置 is_available=true。
#[tauri::command]
pub async fn download_model(
    repo: String,
    _rc: State<'_, SharedRuntimeConfig>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // known_broken：探查阶段已校验过的损坏文件列表（Some = 部分损坏，下载循环跳过完好文件；
    // None = 全新下载或目录不存在，所有文件都要下）。
    let mut known_broken: Option<Vec<String>> = None;

    // 1. 探查：目录存在 → 校验完整性。
    //    全部完好 → 置可用返回；有损坏/缺失 → 记录 known_broken，fall through 到下载循环。
    //    目录不存在 → fall through 全量下载。
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
            let _ = app_handle.emit(
                "download-done",
                serde_json::json!({ "repo": &repo, "already_ready": true }),
            );
            return Ok(());
        }
        // 有 manifest → 校验所有文件 sha256
        let dir_clone = dir.clone();
        let key_clone = existing_key.clone();
        let broken = tokio::task::spawn_blocking(move || {
            let manifest: Manifest = serde_json::from_str(&key_clone)
                .map_err(|e| format!("manifest 解析失败: {e:?}"))?;
            Ok::<Vec<String>, String>(octopus_asr_local::manifest::verify_against_manifest(&dir_clone, &manifest))
        })
        .await
        .map_err(|e| format!("校验任务异常: {}", e))??;
        if broken.is_empty() {
            // 全部文件完好 → 置可用
            apply_model_state(&repo, None, true)?;
            let _ = app_handle.emit(
                "download-done",
                serde_json::json!({ "repo": &repo, "already_ready": true }),
            );
            return Ok(());
        }
        // 有损坏/缺失 → fall through 到下载循环，用 broken 集合跳过完好文件（不重复算 sha256）
        log::info!("[download_model] {} 有 {} 个文件损坏/缺失，重新下载: {:?}", repo, broken.len(), broken);
        known_broken = Some(broken);
    }

    // 2. 读 DB manifest
    let (_model_name, secret_key) = lookup_model_by_source(&repo)?;
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
    // 清空旧 .verified.json 缓存（下载会改文件 mtime，旧缓存过期）
    let _ = std::fs::remove_file(dest_base.join(".verified.json"));

    // 5. 并发下载（JoinSet + Semaphore 限 4 并发）
    use std::sync::Arc;
    use tokio::task::JoinSet;
    let dl = Arc::new(
        octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
            .map_err(|e| format!("初始化下载器失败: {e:?}"))?,
    );

    // 主进度 channel：每个并发 task 推 (file_path, Progress) tuple
    let (tx, mut rx) = mpsc::channel::<(String, octopus_download::Progress)>(64);

    // 进度转发——按 file 分发 emit（供前端文件级进度展示）
    let fwd_handle = app_handle.clone();
    let fwd_repo = repo.clone();
    tokio::spawn(async move {
        while let Some((file, p)) = rx.recv().await {
            let _ = fwd_handle.emit(
                "download-progress",
                serde_json::json!({
                    "repo": &fwd_repo,
                    "file": &file,
                    "downloaded": p.downloaded_bytes,
                    "total": p.total_bytes,
                    "speed": p.speed_bps,
                }),
            );
        }
    });

    let sem = Arc::new(tokio::sync::Semaphore::new(4));
    let mut join_set: JoinSet<std::result::Result<String, String>> = JoinSet::new();

    for (path, file) in manifest.iter() {
        // 增量下载：known_broken 非空时跳过完好文件
        if let Some(ref broken) = known_broken {
            if !broken.contains(path) {
                let _ = app_handle.emit(
                    "download-file",
                    serde_json::json!({ "repo": &repo, "file": path, "status": "skip" }),
                );
                continue;
            }
        }

        // emit start
        let _ = app_handle.emit(
            "download-file",
            serde_json::json!({ "repo": &repo, "file": path, "status": "start" }),
        );

        // 准备 task 参数
        let url = resolve_env_template_with(&file.source, &env_vars);
        let dest = dest_base.join(path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        let task = octopus_download::DownloadTask {
            url,
            mirrors: vec![],
            dest: dest.clone(),
            expected_hash: if file.sha256.is_empty() { None }
                else { Some(octopus_download::Hash::Sha256(file.sha256.clone())) },
            expected_size: if file.size > 0 { Some(file.size) } else { None },
            ..Default::default()
        };

        let permit = sem.clone().acquire_owned().await
            .map_err(|e| format!("并发信号量获取失败: {e}"))?;
        let task_dl = Arc::clone(&dl);
        let task_tx = tx.clone();
        let task_path = path.clone();
        let task_repo = repo.clone();
        let task_handle = app_handle.clone();

        join_set.spawn(async move {
            let _permit = permit; // RAII 限并发

            // 单文件 progress 转发（包装 file path 推到主 channel）
            let (prog_tx, mut prog_rx) = mpsc::channel::<octopus_download::Progress>(64);
            let fwd_tx = task_tx.clone();
            let fwd_path = task_path.clone();
            tokio::spawn(async move {
                while let Some(p) = prog_rx.recv().await {
                    let _ = fwd_tx.send((fwd_path.clone(), p)).await;
                }
            });

            match task_dl.download(&task, prog_tx, None).await {
                Ok(()) => {
                    let _ = task_handle.emit(
                        "download-file",
                        serde_json::json!({ "repo": &task_repo, "file": &task_path, "status": "done" }),
                    );
                    Ok(task_path)
                }
                Err(e) => {
                    let _ = task_handle.emit(
                        "download-file",
                        serde_json::json!({ "repo": &task_repo, "file": &task_path, "status": "error" }),
                    );
                    Err(format!("下载 {} 失败: {e:?}", task_path))
                }
            }
        });
    }
    drop(tx);

    // 等全部完成 + 收集错误
    let mut errors = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("task panic: {e}")),
        }
    }

    // 6. 结果处理
    if errors.is_empty() {
        // 下载成功 → 直接写 .verified.json（Downloader 内部已校验过 hash，不需重新校验）
        let mut cache = load_verified_cache(&dest_base);
        for (path, file) in manifest.iter() {
            let full_path = dest_base.join(path);
            if let Ok(meta) = std::fs::metadata(&full_path) {
                let mtime = meta.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()).unwrap_or(0);
                cache.files.insert(path.clone(), VerifiedEntry {
                    size: meta.len(), mtime, sha256: file.sha256.clone(),
                });
            }
        }
        save_verified_cache(&dest_base, &cache);
        apply_model_state(&repo, None, true)?;
        let _ = app_handle.emit(
            "download-done",
            serde_json::json!({ "repo": &repo, "already_ready": false }),
        );
        Ok(())
    } else {
        // 下载失败 → 回滚 is_available=false（文件不全/损坏不应标可用）
        let _ = apply_model_state(&repo, None, false);
        let _ = app_handle.emit(
            "download-done",
            serde_json::json!({ "repo": &repo, "error": errors.join("; ") }),
        );
        Err(errors.join("; "))
    }
}

/// 完整性复核：按 secret_key 清单 sha256 校验；清单空则自举；损坏置 false。
#[tauri::command]
pub async fn verify_model(model_name: String, repo: String, full: Option<bool>) -> Result<VerifyResult, String> {
    // full=true（手动校验按钮）→ 完整 SHA256；full=false/None（激活前）→ stat 快检
    let full = full.unwrap_or(false);
    tokio::task::spawn_blocking(move || verify_model_inner(model_name, &repo, full))
        .await
        .map_err(|e| format!("verify_model 任务异常: {}", e))?
}

fn verify_model_inner(model_name: String, repo: &str, full: bool) -> Result<VerifyResult, String> {
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
    // full=true（手动校验）：强制 SHA256 逐文件校验，不信任缓存。
    // full=false（激活前自动校验）：stat 快检（.verified.json 缓存），不匹配才算 SHA256。
    let manifest: Manifest = serde_json::from_str(&secret_key)
        .map_err(|e| format!("校验清单解析失败（可重新下载修复）: {e:?}"))?;
    let mut cache = load_verified_cache(&dir);
    let broken: Vec<String> = if full {
        // 强制完整校验——不读缓存，直接 SHA256，结果写回缓存
        manifest
            .iter()
            .filter_map(|(path, file)| {
                let full_path = dir.join(path);
                let ok = octopus_asr_local::manifest::verify_file_sha256(&full_path, &file.sha256);
                if ok {
                    // 更新缓存
                    if let Ok(meta) = std::fs::metadata(&full_path) {
                        let mtime = meta.modified().ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs()).unwrap_or(0);
                        cache.files.insert(path.clone(), VerifiedEntry {
                            size: meta.len(), mtime, sha256: file.sha256.clone(),
                        });
                    }
                    None
                } else {
                    Some(path.clone())
                }
            })
            .collect()
    } else {
        // stat 快检——缓存命中跳过 SHA256，不匹配才算
        manifest
            .iter()
            .filter_map(|(path, file)| {
                if check_file_with_cache(&dir, path, &file.sha256, &mut cache) {
                    None
                } else {
                    Some(path.clone())
                }
            })
            .collect()
    };
    save_verified_cache(&dir, &cache);
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

/// 查模型 domain（供按域 reload ACTIVE_ENGINES）。本地模型仅在 asr/translate/ocr 域。
fn get_model_domain(model_name: &str) -> Option<String> {
    // 先查 ASR（list_all_asr_engines 含全量 ASR，不限 is_available）
    if octopus_infra::db::list_all_asr_engines()
        .ok()?
        .iter()
        .any(|r| r.model_name == model_name)
    {
        return Some("asr".to_string());
    }
    // 非 ASR 本地模型——直接查 DB domain
    octopus_infra::db::get_model_domain_by_name(model_name).ok().flatten()
}

/// 按 domain 刷新 ACTIVE_ENGINES 缓存（review fix 问题 2）。
/// model 数据写路径（download/verify/edit/remove）后调用，确保推理路径读到最新状态。
fn reload_engine_cache(domain: &str) {
    if let Err(e) = octopus_asr_local::config::reload_active_engine(domain) {
        log::warn!("reload_active_engine('{}') 失败：{}", domain, e);
    }
}

/// 写 secret_key（可选）+ is_available（文件就绪）+ 按域刷新 ACTIVE_ENGINES 缓存。
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
    octopus_infra::db::set_model_available(&model_name, enabled).map_err(|e| e.to_string())?;
    // 按域刷新 ACTIVE_ENGINES 缓存（reload_models_config 已是 no-op）
    if let Some(domain) = get_model_domain(&model_name) {
        reload_engine_cache(&domain);
    }
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
///
/// 仅查 is_local=1 行（本地模型 manifest）。云端模型（is_local=0）的 secret_key
/// 请用 [`current_secret_key_any`]（follow-up #7 chokepoint 用，含 cloud 行）。
pub(crate) fn current_secret_key(model_name: &str) -> Result<String, String> {
    for domain in &["asr", "translate", "ocr"] {
        let rows = octopus_infra::db::list_local_models_by_domain(domain).map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.model_name == model_name) {
            return Ok(r.secret_key.clone());
        }
    }
    Err(format!("未找到模型 '{model_name}'"))
}

/// 读任意 domain / is_local 的模型 secret_key（DB raw 值，**不解密**）。
///
/// follow-up #7 引入：vault chokepoint `vault_secret_access::read_model_secret_key`
/// 需要查 cloud 行（is_local=0）——`current_secret_key` 仅查本地 manifest，不够用。
///
/// 流程：先按 model_name 查所有 cloud models（asr/llm/translate/ocr），再 fallback
/// 到本地 manifest。找不到返回 Err（让调用方决定）。
///
/// follow-up #10: 仅在 vault feature on 时被 `vault_secret_access::read_model_secret_key`
/// 调用——feature off 时 dead code，加 cfg_attr 让 dead_code lint 静默（保留函数以便
/// feature 切换时无需改动 model_commands）。
#[cfg_attr(not(feature = "vault"), allow(dead_code))]
pub(crate) fn current_secret_key_any(model_name: &str) -> Result<String, String> {
    // 1. cloud 行（is_local=0）：跨 4 个 domain 查
    for domain in &["asr", "llm", "translate", "ocr"] {
        let rows = octopus_infra::db::list_cloud_models_by_domain(domain)
            .map_err(|e| e.to_string())?;
        if let Some(r) = rows.iter().find(|r| r.model_name == model_name) {
            return Ok(r.secret_key.clone());
        }
    }
    // 2. fallback：本地 manifest（is_local=1）
    current_secret_key(model_name)
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

/// 删除本地模型：删除模型目录 + is_enabled=false（secret_key 保留，下次下载不需重新生成清单）。
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
    // LLM / Translate 云端模型（同为 OpenAI 兼容协议）：后端先测试连接，通过才保存
    // （is_enabled=1），失败返回错误（is_enabled=0 不入库）。
    if (input.domain == "llm" || input.domain == "translate") && !input.model_name.is_empty() {
        let test = test_llm_connection(&input.source, &input.secret_key, &input.model_name, input.is_thinking).await;
        if !test.ok {
            return Err(format!("模型测试失败，无法保存：{}", test.message));
        }
    }
    // M-CLOUDKEY-PLAINTEXT 修复（2026-07-25）：vault 已初始化时加密 secret_key
    // 再落盘——之前 add/edit 明文直接写 DB（只 migrate 覆盖 setup 存量，增量明文残留）。
    let encrypted_key = crate::vault_secret_access::encrypt_secret_global(&input.secret_key)?;
    let id = octopus_infra::db::insert_cloud_model(
        &input.domain, &input.provider, &input.category,
        &input.model_name, &input.source, &encrypted_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())?;
    // 按域刷新 ACTIVE_ENGINES 缓存（新增不影响激活态，但 reload 无害）
    reload_engine_cache(&input.domain);
    Ok(id)
}

#[tauri::command]
pub async fn edit_cloud_model(id: i64, input: CloudModelInput) -> Result<(), String> {
    // LLM / Translate 云端模型：后端先测试连接，通过才更新
    if (input.domain == "llm" || input.domain == "translate") && !input.model_name.is_empty() {
        // secret_key 为空表示编辑时未改 key，从 DB 取真实 key 测试。
        // E-EDIT-TEST-CIPHERTEXT 修复（2026-07-25）：M-CLOUDKEY-PLAINTEXT 修复后 DB 存
        // v1: 密文，get_model_source_key 是裸 SQL 不解密——必须过 try_decrypt_secret_global
        // 解密后再测连接，否则密文当 Bearer token 发云端 → 401 → 编辑被拒。
        // 与 action_bar_commands:973 / config:58 / engine_aliyun:115 三处读路径模式一致。
        let real_key = if input.secret_key.is_empty() {
            let raw = octopus_infra::db::get_model_source_key(id)
                .map(|(_, k)| k)
                .unwrap_or_default();
            crate::vault_secret_access::try_decrypt_secret_global(&raw)?
        } else {
            input.secret_key.clone()
        };
        let test = test_llm_connection(&input.source, &real_key, &input.model_name, input.is_thinking).await;
        if !test.ok {
            return Err(format!("模型测试失败，无法保存：{}", test.message));
        }
    }
    // M-CLOUDKEY-PLAINTEXT 修复：加密 secret_key 再落盘（空值原样返回，DB 层保持现有值）
    let encrypted_key = crate::vault_secret_access::encrypt_secret_global(&input.secret_key)?;
    octopus_infra::db::update_cloud_model(
        id, &input.provider, &input.category,
        &input.model_name, &input.source, &encrypted_key,
        input.is_streaming, input.is_thinking,
    ).map_err(|e| e.to_string())?;
    // 按域刷新 ACTIVE_ENGINES（编辑可能改了激活模型的 secret_key/source）
    reload_engine_cache(&input.domain);
    Ok(())
}

#[tauri::command]
pub fn remove_cloud_model(id: i64) -> Result<(), String> {
    // 删前查 domain 供 reload（删后查不到了）
    let domain = octopus_infra::db::get_model_by_id(id)
        .ok().flatten()
        .map(|r| r.domain);
    octopus_infra::db::delete_cloud_model(id).map_err(|e| e.to_string())?;
    // 按域刷新 ACTIVE_ENGINES（删除的可能是激活模型 → reload 后 fallback/None）
    if let Some(d) = domain {
        reload_engine_cache(&d);
    }
    Ok(())
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

/// 翻译云端模型列表项（前端 TranslateTab 云端 section 用）。
///
/// 字段与 LLM 云端模型对齐（同为 OpenAI 兼容协议），前端 `CloudModelForm` 的
/// translate 分支直接复用 llm 的 provider→base_url 自动填充逻辑。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateCloudModel {
    pub id: i64,
    pub provider: String,
    pub category: String,
    pub model_name: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB is_enabled（激活态）。供前端标 current（review fix 问题 3）。
    pub is_enabled: bool,
}

/// 列出所有翻译云端模型（domain='translate' AND is_local=0）。
///
/// translate_engine 配置项存 DB 行 id，前端激活/编辑/删除均按 id 操作，
/// 故这里必须返回 id（与 list_downloadable_models 只返回 model_name 不同）。
#[tauri::command]
pub fn list_translate_cloud_models() -> Result<Vec<TranslateCloudModel>, String> {
    let rows = octopus_infra::db::list_cloud_models_by_domain("translate")
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| TranslateCloudModel {
        id: r.id,
        provider: r.provider,
        category: r.category,
        model_name: r.model_name,
        source: r.source,
        secret_key: r.secret_key,
        is_streaming: r.is_streaming,
        is_thinking: r.is_thinking,
        is_enabled: r.is_enabled,
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

    /// 测试前切换到 in-memory DB，避免污染开发库 ~/.octopus/octopus.db。
    /// 详见架构文档「测试数据库隔离」。
    static TEST_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn ensure_test_db() {
        TEST_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
            // 触发 ensure_db 初始化 in-memory 连接（惰性首调会建表+seed）
            let _ = octopus_asr_local::db::ensure_db();
        });
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
