//! 模型管理页 Tauri 命令：列出可下载模型 + 下载（进度推前端）。
//!
//! 与 settings_commands.rs 分离为独立模块——降低与 setting-ui2 分支在
//! settings_commands.rs 上的合并冲突；模型页前端 JS 在独立 models.js。
//!
//! 复用阶段1 的 download crate（HfRequest / resolve_tasks / Downloader）和
//! resolve_model_dir 第3级（~/.octopus/models/<source>）。

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use crate::runtime_config::SharedRuntimeConfig;

/// 一个可下载模型的列表项。
#[derive(Serialize)]
pub struct DownloadableModel {
    /// 引擎裸名（如 zipformer-multi），与 DB models 表 model_name 一致。
    pub name: String,
    /// HF repo（entry.source），如 csukuangfj/sherpa-onnx-streaming-paraformer-zh。
    pub repo: String,
    /// 引擎族标签，如 zipformer/paraformer/whisper/moonshine/sensevoice。
    pub category: String,
    /// DB 中的描述（含尺寸信息，如 "paraformer-streaming, 230M"）。
    pub description: String,
    /// resolve_model_dir 任一级命中即 true（已就绪可用）。
    pub downloaded: bool,
}

/// 判定 source 是否为可下载的 HF repo。
/// 排除：随包打包（`models/` 前缀）、绝对路径、云端协议（http/wss）、空。
fn is_hf_repo(source: &str) -> bool {
    !source.is_empty()
        && !source.starts_with("models/") // 随包小模型（如 models/zipformer）
        && !source.starts_with('/') // 绝对路径（用户自定义本地模型）
        && !source.starts_with("http") // http(s) 云端
        && !source.starts_with("wss") // wss 云端
        && source.contains('/') // HF repo 至少 owner/name
}

/// 列出所有可下载的本地 ASR 模型（排除随包兜底与非 HF repo 的 source）。
#[tauri::command]
pub fn list_downloadable_models() -> Result<Vec<DownloadableModel>, String> {
    let cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for e in engines {
        if !e.is_local {
            continue;
        }
        // 裸名解析到 entry 拿 source（resolve_engine_in_config 的 NameOnly 分支遍历所有 section）。
        let Some((_cat, _name, entry)) =
            octopus_asr::config::resolve_engine_in_config(&cfg, &e.name)
        else {
            continue;
        };
        if !is_hf_repo(&entry.source) {
            continue;
        }
        // 先 clone 文本字段，再 move e.category（category_label 按值取）。
        let name = e.name.clone();
        let description = e.description.clone();
        let category = octopus_asr::config::category_label(e.category).to_string();
        let downloaded = octopus_asr::config::resolve_model_dir(&entry.source).is_ok();
        out.push(DownloadableModel {
            name,
            repo: entry.source.clone(),
            category,
            description,
            downloaded,
        });
    }
    Ok(out)
}

/// 设置下载镜像（写运行时 AppConfig + 持久化 DB）。
///
/// 独立命令而非复用 settings_commands::set_config——后者 apply_config_value 的
/// 字段分发无 download_mirror（阶段1 只加了字段未加分发），加进去会改动 settings_commands.rs，
/// 与 setting-ui2 分支冲突；download_mirror 属下载域，放本模块更内聚。
#[tauri::command]
pub fn set_download_mirror(value: String, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let mut cfg = rc.read().unwrap().clone();
    cfg.download_mirror = value;
    // 写运行时快照（download_model 读 rc 拿镜像）+ 持久化 DB，对称既有 set_config。
    *rc.write().unwrap() = cfg.clone();
    octopus_infra::db::save_app_config(&cfg).map_err(|e| e.to_string())?;
    Ok(())
}

/// 下载 HF 模型到 ~/.octopus/models/<repo>/，进度以 Tauri 事件推前端。
///
/// 镜像优先级：config.download_mirror（空 = 官方源 huggingface.co）。
/// 多文件仓库逐一下载：每文件 emit `download-file`（文件序号），文件内字节进度 emit `download-progress`。
#[tauri::command]
pub async fn download_model(
    repo: String,
    rc: State<'_, SharedRuntimeConfig>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let mirror = {
        let g = rc.read().unwrap();
        let m = g.download_mirror.trim().to_string();
        if m.is_empty() {
            None
        } else {
            Some(m)
        }
    };
    let target_dir = octopus_infra::paths::octopus_config_home().join("models");

    let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
        .map_err(|e| format!("初始化下载器失败: {e:?}"))?;
    // 复用 Downloader 内部的 reqwest::Client（已配好超时），免在 desktop 直接依赖 reqwest。
    let client = dl.client().clone();

    let hf_req = octopus_download::HfRequest {
        repo: repo.clone(),
        include: Vec::new(),
        exclude: Vec::new(),
        source_url: mirror,
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
    // tx clone 给每个文件下载；全部 drop 后 channel 关闭、转发 task 的 recv 返回 None 自然退出。
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_hf_repo_real_repos() {
        assert!(is_hf_repo(
            "k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13"
        ));
        assert!(is_hf_repo("csukuangfj/sherpa-onnx-streaming-paraformer-zh"));
        assert!(is_hf_repo("onnx-community/whisper-small.en"));
    }

    #[test]
    fn is_hf_repo_excludes_bundled() {
        // 随包打包的小模型：models/ 前缀
        assert!(!is_hf_repo("models/zipformer"));
        assert!(!is_hf_repo("models/silero_vad_v4.onnx"));
    }

    #[test]
    fn is_hf_repo_excludes_absolute_and_remote() {
        assert!(!is_hf_repo("/Users/x/models/foo")); // 绝对路径
        assert!(!is_hf_repo("https://x.com/m")); // http 云端
        assert!(!is_hf_repo("wss://dashscope.aliyuncs.com/api-ws/v1/inference")); // wss 云端
    }

    #[test]
    fn is_hf_repo_excludes_empty() {
        assert!(!is_hf_repo(""));
    }
}
