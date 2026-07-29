//! 搜索相关 Tauri 命令。

use octopus_search::{self, AppBrief, SearchBatch, SearchResult};
use tauri::Emitter;

/// 综合搜索。
#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<Vec<SearchResult>, String> {
    let query = query.trim().to_string();
    let engine = match octopus_search::get_engine() {
        Some(e) => e,
        None => return Err("搜索引擎未初始化".into()),
    };
    Ok(engine.search(&query, &tab).await)
}

/// 流式搜索：每个 Provider 完成立即 emit `search://batch` 事件，
/// 全部结束后 emit `search://done`。前端按 runId 匹配本次会话。
#[tauri::command]
pub async fn search_stream(
    query: String,
    tab: String,
    run_id: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let query = query.trim().to_string();
    let engine = match octopus_search::get_engine() {
        Some(e) => e,
        None => return Err("搜索引擎未初始化".into()),
    };
    engine
        .search_streaming(&query, &tab, &run_id, |batch: SearchBatch| {
            let _ = app.emit("search://batch", &batch);
        })
        .await;
    let _ = app.emit("search://done", &serde_json::json!({ "runId": run_id }));
    Ok(())
}

/// 记录搜索命中（频次加权用）。前端执行动作时调。
/// 传整个 result 对象的字段，后端构造 SearchResult 以算出与搜索时一致的 score_key。
#[tauri::command]
pub async fn record_search_hit(
    source: String,
    action_type: String,
    action_data: String,
    query: String,
) -> Result<(), String> {
    let engine = octopus_search::get_engine().ok_or("搜索引擎未初始化")?;
    let result = SearchResult {
        source,
        title: String::new(), // score_key 不用 title
        subtitle: String::new(),
        icon: None,
        action_type,
        action_data,
        score: 0,
    };
    engine.record_frequency(&result, &query);
    Ok(())
}

/// 启动应用。检查退出码——路径无效/应用已移动时返回错误而非静默成功。
#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    crate::platform::sys_open::open_with_default(&path)
}

/// 打开文件。检查退出码——路径无效/权限拒绝时返回错误。
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    crate::platform::sys_open::open_with_default(&path)
}

/// 打开 URL（默认浏览器）。检查退出码——无默认处理器时返回错误。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    crate::platform::sys_open::open_with_default(&url)
}

/// 在文件管理器中定位文件（macOS Finder / Windows Explorer / Linux xdg-open）。
/// command 回车时调——复制命令名 + 在 Finder 中显示命令文件位置。
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    crate::platform::sys_open::reveal_path(&path)
}

/// 强制重扫应用索引：刷新内存索引 + DB 缓存。
///
/// **诊断/兜底用途**——正常运行由后台 mtime 轮询线程自动触发（main.rs），
/// 用户无需手动调用。保留此命令作为后台扫描失效时的手动 fallback 和开发调试入口。
#[tauri::command]
pub fn reindex_apps() -> Result<usize, String> {
    let engine = octopus_search::get_engine().ok_or("搜索引擎未初始化")?;
    Ok(engine.refresh_app_index())
}

/// 列出全部已索引应用（name + bundle_id + icon），供 app-aware 菜单绑定的多选器 UI 使用。
/// 仅返回有 bundle_id 的应用（读不到 CFBundleIdentifier 的过滤掉）。
#[tauri::command]
pub fn list_all_apps() -> Result<Vec<AppBrief>, String> {
    let engine = octopus_search::get_engine().ok_or("搜索引擎未初始化")?;
    Ok(engine.all_apps())
}
