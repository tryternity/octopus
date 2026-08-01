//! 应用感知润色模板路由。按前台 app bundle_id 选模板（app_bundle_ids 关联），
//! 无关联 → 默认（active_polish_prompt）。结果缓存，模板 CRUD 时 invalidate。
//!
//! 见 spec docs/superpowers/specs/2026-08-01-app-aware-polish-design.md。

use std::collections::HashMap;
use std::sync::RwLock;

use once_cell::sync::Lazy;

use crate::commands::settings_commands::read_prompt_file;

/// 路由缓存：bundle_id → prompt_id。模板 CRUD 时 invalidate。
/// bundle_id 为 None（无 app 信息）时跳过缓存（每次直接查默认）。
static ROUTE_CACHE: Lazy<RwLock<HashMap<String, i64>>> = Lazy::new(|| RwLock::new(HashMap::new()));

/// 模板 CRUD 后调，清空整个缓存。
pub(crate) fn invalidate_route_cache() {
    ROUTE_CACHE.write().unwrap().clear();
    log::debug!("[prompt-route] cache invalidated");
}

/// 解析润色模板的结果：(prompt 文件内容文本, inject_context 标志)。
/// prompt 文件内容 = read_prompt_file(content 文件名引用)。
pub(crate) struct ResolvedPrompt {
    /// 模板规则文本（已从文件读出）
    pub content: String,
    pub inject_context: bool,
    /// 模板显示名（用于浮窗「润色中」文案；降级时为空串）
    pub template_title: String,
    /// 是否命中 app 关联模板（true=显示 app 名；false=默认模板，只显示模板名）
    pub route_hit: bool,
}

/// 按前台 app bundle_id 解析润色模板。
/// - 有 bundle_id 且关联了模板 → 取该模板（updated_at 最新）
/// - 否则 → active_polish_prompt（用户激活的默认模板）
///
/// 返回 ResolvedPrompt。读文件失败时 content="" 作为降级（LLM 用空 system prompt）。
///
/// perf 打点：路由决策路径（cache-hit / db-hit / default）记 perf_log，
/// 便于排查「为何没用我绑的模板」。bundle_id=None（无 app 信息）也记 default。
pub(crate) fn resolve_polish_prompt(bundle_id: Option<&str>) -> ResolvedPrompt {
    if let Some(bid) = bundle_id {
        // 1. 缓存命中
        if let Some(&cached_id) = ROUTE_CACHE.read().unwrap().get(bid) {
            if let Some(rec) = load_record(cached_id) {
                perf_log_route("cache-hit", Some(bid), rec.id, &rec.title);
                return resolve_record(&rec, true);
            }
        }
        // 2. 查 DB（app_bundle_ids LIKE）
        if let Ok(Some(rec)) = octopus_infra::db::find_prompt_by_bundle_id(bid) {
            // 写缓存
            ROUTE_CACHE.write().unwrap().insert(bid.to_string(), rec.id);
            perf_log_route("db-hit", Some(bid), rec.id, &rec.title);
            return resolve_record(&rec, true);
        }
        // 3. 无关联 → 默认模板（不写缓存，bundle_id 可能是任何未关联 app）
    }
    // 4. 默认：active_polish_prompt
    let default_id = octopus_infra::db::load_active_prompt_id().unwrap_or(1);
    match load_record(default_id) {
        Some(rec) => {
            perf_log_route("default", bundle_id, rec.id, &rec.title);
            resolve_record(&rec, false)
        }
        None => {
            crate::core::perf_log::log(&format!(
                "[POLISH] route source=default bundle={:?} (load_record 失败，空 prompt 降级)", bundle_id
            ));
            ResolvedPrompt {
                content: String::new(),
                inject_context: false,
                template_title: String::new(),
                route_hit: false,
            }
        }
    }
}

/// 路由决策 perf 打点。source: cache-hit / db-hit / default。
fn perf_log_route(source: &str, bundle_id: Option<&str>, prompt_id: i64, title: &str) {
    crate::core::perf_log::log(&format!(
        "[POLISH] route source={} bundle={:?} prompt_id={} title={}",
        source, bundle_id, prompt_id, title
    ));
}

fn load_record(id: i64) -> Option<octopus_infra::db::PromptRecord> {
    octopus_infra::db::load_prompt(id).ok().flatten()
}

fn resolve_record(rec: &octopus_infra::db::PromptRecord, route_hit: bool) -> ResolvedPrompt {
    ResolvedPrompt {
        content: read_prompt_file(&rec.content),
        inject_context: rec.inject_context,
        template_title: rec.title.clone(),
        route_hit,
    }
}
