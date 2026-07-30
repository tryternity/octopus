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
#[serde(rename_all = "camelCase")]
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
/// stat 快检：用 .verified.json 缓存判断文件是否就绪（不读文件内容算 SHA256）。
///
/// 启动 sync 用此函数——stat 微秒级，不卡启动。
/// 缓存未命中（首次/文件变动）时回退 SHA256（首次校验后写入缓存）。
fn check_builtin_ready(source: &str, secret_key: &str) -> bool {
    let dir = match octopus_asr_local::config::resolve_model_dir(source) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let manifest: octopus_asr_local::manifest::Manifest = match serde_json::from_str(secret_key) {
        Ok(m) => m,
        Err(_) => return dir.exists(),
    };
    if manifest.is_empty() {
        return dir.exists();
    }
    // stat 快检：.verified.json 缓存命中 → 跳过 SHA256；未命中 → SHA256 + 写缓存
    let mut cache = crate::commands::model_commands::load_verified_cache(&dir);
    let all_ok = manifest
        .iter()
        .all(|(path, file)| {
            crate::commands::model_commands::check_file_with_cache(&dir, path, &file.sha256, &mut cache)
        });
    crate::commands::model_commands::save_verified_cache(&dir, &cache);
    all_ok
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

/// ASR 兜底引擎自动激活（2026-07-28）。
///
/// 条件：ASR 域无激活模型（is_enabled=1）+ zipformer-small 文件就绪（is_available=1）。
/// 满足时调 `switch_active_model("asr", zipformer_small_id)` 激活。
///
/// 场景：全新库 db.sql seed 把 is_enabled 全设 0。`resolve_active_engine` 有 runtime
/// fallback（不依赖 is_enabled），但 DB 显示未激活会让用户困惑（设置页无 current 标记、
/// tray 引擎名可能异常）。激活后 DB 反映真实使用状态。
///
/// 不激活的情况：
/// - ASR 域已有激活模型（用户在设置页激活了别的）→ 不覆盖用户选择
/// - zipformer-small 文件未就绪（is_available=0）→ 无法激活
///
/// 返回：Ok(true) = 已激活；Ok(false) = 无需激活（已有激活或兜底未就绪）；Err = DB 错误。
pub fn auto_activate_fallback_asr() -> Result<bool, String> {
    use octopus_infra::db;

    // 1. ASR 域已有激活模型 → 不覆盖
    if db::get_active_model("asr")
        .map_err(|e| format!("查询 ASR 激活模型失败: {e}"))?
        .is_some()
    {
        return Ok(false);
    }

    // 2. 查 zipformer-small（builtin 兜底引擎）是否就绪
    let fallback = db::list_builtin_models()
        .map_err(|e| format!("查询 builtin 模型失败: {e}"))?
        .into_iter()
        .find(|m| m.model_name == "zipformer-small" && m.is_available);
    let Some(fallback) = fallback else {
        return Ok(false); // 兜底未就绪（缺文件）→ 不激活，下载窗会提示
    };

    // 3. 激活兜底引擎
    db::switch_active_model("asr", fallback.id)
        .map_err(|e| format!("激活兜底引擎失败: {e}"))?;
    log::info!(
        "[startup] ASR 兜底引擎 zipformer-small 已自动激活（id={}，此前 ASR 域无激活模型）",
        fallback.id
    );

    // 4. 刷新 asr 的激活引擎缓存（让 resolve_active_engine 立即生效）
    octopus_asr_local::config::reload_active_engine("asr")
        .map_err(|e| format!("reload_active_engine 失败: {e}"))?;

    Ok(true)
}

/// Tauri 命令：返回缺失的 builtin 模型列表（供下载页 load）。
/// 不走启动缓存——下载页打开时可能文件状态已变（如用户刚点了下载），需实时查询。
/// DB 查询失败时返回 Err（前端 catch 显示错误，而非误报「已就绪」）。
#[tauri::command]
pub fn check_builtin_models() -> Result<Vec<BuiltinModelInfo>, String> {
    check_and_sync_builtins()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试前切换到 in-memory DB，避免污染开发库。
    static TEST_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn ensure_test_db() {
        TEST_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
            let _ = octopus_asr_local::db::ensure_db();
        });
    }

    /// 串行化测试——三个测试都操作全局 in-memory DB 的 models 表（switch_active_model
    /// / set_model_available），并发会互相干扰。用 Mutex 保证一次只跑一个。
    static TEST_SERIALIZER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 全新库（db.sql seed）后，ASR 域无激活模型 + zipformer-small 默认 is_available=0
    /// → auto_activate 应返回 Ok(false)（兜底未就绪，不激活）。
    #[test]
    fn auto_activate_skips_when_fallback_not_available() {
        let _s = test_lock();
        ensure_test_db();
        // 全新库 seed：zipformer-small is_available=0（ensure_builtin_seed 设的）
        // 确保无激活模型
        let _ = octopus_infra::db::switch_active_model("asr", -1);

        let activated = auto_activate_fallback_asr().expect("应不报错");
        assert!(!activated, "兜底未就绪时不应激活");

        // 验证仍无激活
        let active = octopus_infra::db::get_active_model("asr").unwrap();
        assert!(active.is_none(), "不应有激活模型");
    }

    /// zipformer-small 标记为 is_available=1（模拟文件就绪）+ ASR 无激活
    /// → auto_activate 应激活它（返回 Ok(true)）。
    #[test]
    fn auto_activate_when_fallback_ready_and_no_active() {
        let _s = test_lock();
        ensure_test_db();
        // 模拟文件就绪：把 zipformer-small is_available 置 1
        let fallback = octopus_infra::db::list_builtin_models()
            .unwrap()
            .into_iter()
            .find(|m| m.model_name == "zipformer-small")
            .expect("zipformer-small 应在 seed 中");
        octopus_infra::db::set_model_available("zipformer-small", true).unwrap();
        // 确保无激活
        let _ = octopus_infra::db::switch_active_model("asr", -1);

        let activated = auto_activate_fallback_asr().expect("应不报错");
        assert!(activated, "兜底就绪 + 无激活时应激活");

        // 验证激活的是 zipformer-small
        let active = octopus_infra::db::get_active_model("asr").unwrap().expect("应有激活");
        assert_eq!(active.model_name, "zipformer-small");

        // 清理：恢复 is_available=0 + 取消激活，避免污染其他测试
        let _ = octopus_infra::db::set_model_available("zipformer-small", false);
        let _ = octopus_infra::db::switch_active_model("asr", -1);
        let _ = fallback; // 避免 unused warning
    }

    /// ASR 域已有激活模型 → auto_activate 不应覆盖（返回 Ok(false)）。
    #[test]
    fn auto_activate_skips_when_already_has_active() {
        let _s = test_lock();
        ensure_test_db();
        // 先激活 zipformer-small（模拟用户已激活）
        let fallback = octopus_infra::db::list_builtin_models()
            .unwrap()
            .into_iter()
            .find(|m| m.model_name == "zipformer-small")
            .expect("zipformer-small 应在 seed 中");
        octopus_infra::db::set_model_available("zipformer-small", true).unwrap();
        octopus_infra::db::switch_active_model("asr", fallback.id).unwrap();

        // 再调 auto_activate——不应覆盖
        let activated = auto_activate_fallback_asr().expect("应不报错");
        assert!(!activated, "已有激活时不应覆盖");

        // 验证激活的仍是 zipformer-small（没被改动）
        let active = octopus_infra::db::get_active_model("asr").unwrap().expect("应有激活");
        assert_eq!(active.model_name, "zipformer-small");

        // 清理
        let _ = octopus_infra::db::set_model_available("zipformer-small", false);
        let _ = octopus_infra::db::switch_active_model("asr", -1);
    }
}
