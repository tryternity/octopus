//! Builtin 模型（source_type=0）检测与下载协调。
//!
//! 详见 spec `docs/superpowers/specs/2026-07-22-builtin-models.md` §3。
//!
//! builtin 模型（当前仅 zipformer-small-ctc 兜底引擎）不随应用打包，首次启动时
//! 由 [`check_builtin_models_missing`] 检测本地文件缺失 → 前端下载页展示 →
//! 用户点「后台下载」→ 复用 `model_commands::download_model`（manifest 驱动）。
//!
//! 下载完成后 `download_model` 自动 `set_model_available(name, true)`，引擎即用。

use serde::Serialize;

/// 单个 builtin 模型的检测信息（供前端下载页展示）。
#[derive(Debug, Serialize, Clone)]
pub struct BuiltinModelInfo {
    /// DB model_name（如 "zipformer-small-ctc"）
    pub name: String,
    /// DB source（路径标识，如 "models/zipformer"）—— 传给 download_model 的 repo 参数
    pub source: String,
    /// 描述（DB description）
    pub description: String,
    /// 是否支持流式
    pub is_streaming: bool,
}

/// 检查 builtin 模型本地文件是否缺失（resolve_model_dir 命中 = 已就绪）。
///
/// 返回缺失的 builtin 模型列表。空 Vec = 全部就绪（无需下载页）。
/// DB 查询失败时返回空 Vec（保守不阻断启动——与 ensure_db 失败不阻断一致）。
pub fn check_builtin_models_missing() -> Vec<BuiltinModelInfo> {
    let builtins = match octopus_infra::db::list_builtin_models() {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("[builtin_models] 查询 builtin 模型失败（保守跳过下载页）: {e}");
            return Vec::new();
        }
    };

    builtins
        .into_iter()
        .filter(|r| {
            // resolve_model_dir 命中 = 文件就绪；Err = 缺失
            octopus_asr_local::config::resolve_model_dir(&r.source).is_err()
        })
        .map(|r| BuiltinModelInfo {
            name: r.model_name,
            source: r.source,
            description: r.description,
            is_streaming: r.is_streaming,
        })
        .collect()
}

/// 同步 builtin 模型的 is_available 状态：文件就绪 → 置 1，缺失 → 置 0。
///
/// builtin 兜底引擎的 is_available 反映本地文件是否就绪（与其他 local 模型一致）。
/// 启动时调一次，确保 DB 状态与文件系统一致（如用户手动删了模型文件，
/// 下次启动 is_available 回 0 → check_builtin_models_missing 会弹下载窗）。
///
/// 文件已就绪但 is_available=0 的情况（如首次 ensure_builtin_seed 注入后文件恰好在）：
/// 此函数置 1，让 resolve_engine_any 能查到（它要求 is_available=1）。
pub fn sync_builtin_models_availability() {
    let builtins = match octopus_infra::db::list_builtin_models() {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("[builtin_models] sync availability 查询失败: {e}");
            return;
        }
    };

    for r in &builtins {
        let file_ready = octopus_asr_local::config::resolve_model_dir(&r.source).is_ok();
        if file_ready != r.is_available {
            log::info!(
                "[builtin_models] {} is_available {} → {}（文件{}）",
                r.model_name,
                r.is_available as i32,
                file_ready as i32,
                if file_ready { "就绪" } else { "缺失" }
            );
            if let Err(e) = octopus_infra::db::set_model_available(&r.model_name, file_ready) {
                log::warn!("[builtin_models] set_model_available({}) 失败: {e}", r.model_name);
            }
        }
    }
}

/// Tauri 命令：返回缺失的 builtin 模型列表（供下载页 load）。
///
/// 前端用法：`invoke("check_builtin_models")` → BuiltinModelInfo[]
/// 下载用：`invoke("download_model", { repo: info.source })`（复用 model_commands）
#[tauri::command]
pub fn check_builtin_models() -> Vec<BuiltinModelInfo> {
    check_builtin_models_missing()
}
