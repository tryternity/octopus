//! Builtin 模型（source_type=0）检测与下载协调。
//!
//! 详见 spec `docs/superpowers/specs/2026-07-22-builtin-models.md §3。
//!
//! builtin 模型（当前仅 zipformer-small 兜底引擎）不随应用打包，首次启动时
//! 由 [`check_and_sync_builtins`] 检测本地文件缺失 → 同步 is_available →
//! 返回缺失列表供前端下载页展示。
//!
//! 下载完成后 `download_model` 自动 `set_model_available(name, true)`，引擎即用。

use serde::Serialize;

/// 单个 builtin 模型的检测信息（供前端下载页展示）。
#[derive(Debug, Serialize, Clone)]
pub struct BuiltinModelInfo {
    /// DB model_name（如 "zipformer-small"）
    pub name: String,
    /// DB source（路径标识，如 "asr/zipformer-small"）—— 传给 download_model 的 repo 参数
    pub source: String,
    /// 描述（DB description）
    pub description: String,
    /// 是否支持流式
    pub is_streaming: bool,
}

/// 校验单个 builtin 模型：目录存在 + manifest 所有文件 sha256 通过 → true。
///
/// 与 download_model 的完整性校验对齐——不只 stat 目录存在，逐文件 sha256 校验。
fn check_builtin_ready(source: &str, secret_key: &str) -> bool {
    let dir = match octopus_asr_local::config::resolve_model_dir(source) {
        Ok(d) => d,
        Err(_) => return false,
    };
    // 无 manifest → 只看目录存在（bootstrap 路径会补 manifest）
    let manifest: octopus_asr_local::manifest::Manifest = match serde_json::from_str(secret_key) {
        Ok(m) => m,
        Err(_) => return dir.exists(),
    };
    if manifest.is_empty() {
        return dir.exists();
    }
    // 逐文件 sha256 校验
    octopus_asr_local::manifest::verify_against_manifest(&dir, &manifest).is_empty()
}

/// 合并 sync + check：查 DB builtin 模型 → 逐个校验完整性 → 同步 is_available → 返回缺失列表。
///
/// 启动时调一次（sync_builtin_models_availability 和 check_builtin_models_missing 共享结果）。
/// 返回缺失的 builtin 模型信息列表（空 = 全部就绪）。
fn check_and_sync_builtins() -> Vec<BuiltinModelInfo> {
    let builtins = match octopus_infra::db::list_builtin_models() {
        Ok(rows) => rows,
        Err(e) => {
            // DB 失败 → 返回空（保守不阻断启动），但日志区分「查询失败」与「全部就绪」
            log::warn!("[builtin_models] 查询 builtin 模型失败（保守跳过下载页）: {e}");
            return Vec::new();
        }
    };

    builtins
        .into_iter()
        .filter_map(|r| {
            let ready = check_builtin_ready(&r.source, &r.secret_key);
            // 同步 is_available
            if ready != r.is_available {
                log::info!(
                    "[builtin_models] {} is_available {} → {}（完整性校验{}）",
                    r.model_name,
                    r.is_available as i32,
                    ready as i32,
                    if ready { "通过" } else { "未通过/缺失" }
                );
                if let Err(e) = octopus_infra::db::set_model_available(&r.model_name, ready) {
                    log::warn!("[builtin_models] set_model_available({}) 失败: {e}", r.model_name);
                }
            }
            if ready {
                None // 就绪 → 不返回
            } else {
                Some(BuiltinModelInfo {
                    name: r.model_name,
                    source: r.source,
                    description: r.description,
                    is_streaming: r.is_streaming,
                })
            }
        })
        .collect()
}

/// 启动时同步 builtin 模型的 is_available 状态（完整性校验）。
/// 在 preheat/load_active_engine 之前调用。结果丢弃（check_builtin_models_missing 会再调一次，开销可忽略）。
pub fn sync_builtin_models_availability() {
    let _ = check_and_sync_builtins();
}

/// 返回缺失的 builtin 模型列表（供下载页展示 + setup 判断是否弹窗）。
pub fn check_builtin_models_missing() -> Vec<BuiltinModelInfo> {
    check_and_sync_builtins()
}

/// Tauri 命令：返回缺失的 builtin 模型列表（供下载页 load）。
#[tauri::command]
pub fn check_builtin_models() -> Vec<BuiltinModelInfo> {
    check_builtin_models_missing()
}
