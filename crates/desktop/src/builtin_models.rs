//! Builtin 模型（source_type=0）检测与下载协调。
//!
//! 详见 spec `docs/superpowers/specs/2026-07-22-builtin-models.md §3。
//!
//! builtin 模型（当前仅 zipformer-small 兜底引擎）不随应用打包，首次启动时
//! 由 [`check_and_sync_builtins`] 检测本地文件缺失 → 同步 is_available →
//! 返回缺失列表供前端下载页展示。
//!
//! 下载完成后 `download_model` 自动 `set_model_available(name, true)`，引擎即用。

use std::sync::OnceLock;

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
/// DB 查询失败时返回 Err（供调用方区分「就绪」与「查询失败」）。
fn check_and_sync_builtins() -> Result<Vec<BuiltinModelInfo>, String> {
    let builtins = octopus_infra::db::list_builtin_models()
        .map_err(|e| {
            log::warn!("[builtin_models] 查询 builtin 模型失败: {e}");
            format!("查询内置模型失败: {e}")
        })?;

    let missing = builtins
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
        .collect();
    Ok(missing)
}

/// 启动阶段缓存：sync_builtin_models_availability 算一次并缓存，
/// check_builtin_models_missing（同一启动周期内）直接读缓存，避免重复 sha256。
/// check_builtin_models（Tauri 命令，下载页 invoke）不走缓存——用户可能在下载页
/// 操作后状态已变，需实时查询。
static STARTUP_CACHE: OnceLock<Vec<BuiltinModelInfo>> = OnceLock::new();

/// 启动时同步 builtin 模型的 is_available 状态（完整性校验）。
/// 在 preheat/load_active_engine 之前调用。结果缓存供 check_builtin_models_missing 复用。
/// DB 失败时保守不阻断（缓存空列表），日志记录。
pub fn sync_builtin_models_availability() {
    let result = check_and_sync_builtins().unwrap_or_else(|e| {
        log::warn!("[builtin_models] sync 失败（保守跳过，不阻断启动）: {e}");
        Vec::new()
    });
    let _ = STARTUP_CACHE.set(result);
}

/// 返回缺失的 builtin 模型列表（供 setup 判断是否弹窗）。
/// 读启动缓存（sync_builtin_models_availability 已计算），不重复 sha256。
pub fn check_builtin_models_missing() -> Vec<BuiltinModelInfo> {
    STARTUP_CACHE.get().cloned().unwrap_or_default()
}

/// Tauri 命令：返回缺失的 builtin 模型列表（供下载页 load）。
/// 不走启动缓存——下载页打开时可能文件状态已变（如用户刚点了下载），需实时查询。
/// DB 查询失败时返回 Err（前端 catch 显示错误，而非误报「已就绪」）。
#[tauri::command]
pub fn check_builtin_models() -> Result<Vec<BuiltinModelInfo>, String> {
    check_and_sync_builtins()
}
