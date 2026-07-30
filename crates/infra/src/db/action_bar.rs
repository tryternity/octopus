// db/action_bar.rs —— Action Bar + Launcher Index + App Index + Search Frequency + Script Run。
//
// 表：action_bar_items / launcher_index / search_frequency / script_runs。

use super::{collect_rows, ensure_db, with_db, Connection, HashMap, Result, params};
use anyhow::Context;

// ── Action Bar 菜单项 ──

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarItem {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub icon: String,
    pub action_type: String,
    pub action_data: String,
    pub sort_order: i64,
    pub is_system: bool,
    pub is_enabled: bool,
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
    pub shortcut: String,
    pub agent: String,
    pub accepts: String,
    pub trigger_keyword: String,
    pub global_shortcut: String,
    pub need_voice: bool,
    /// JSON 数组字符串 ["com.apple.Safari"]，空串=全局项（所有 app 显示）。app-aware 绑定。
    pub app_bundle_ids: String,
}

const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, global_shortcut, need_voice, app_bundle_ids";

fn row_to_action_bar_item(row: &rusqlite::Row) -> rusqlite::Result<ActionBarItem> {
    Ok(ActionBarItem {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        icon: row.get(3)?,
        action_type: row.get(4)?,
        action_data: row.get(5)?,
        sort_order: row.get(6)?,
        is_system: row.get::<_, i32>(7)? != 0,
        is_enabled: row.get::<_, i32>(8)? != 0,
        is_async: row.get::<_, i32>(9)? != 0,
        write_output_to_clipboard: row.get::<_, i32>(10)? != 0,
        shortcut: row.get(11)?,
        agent: row.get(12)?,
        accepts: row.get(13)?,
        trigger_keyword: row.get(14)?,
        global_shortcut: row.get(15)?,
        need_voice: row.get::<_, i32>(16)? != 0,
        app_bundle_ids: row.get(17)?,
    })
}

/// 校验快捷键格式：空字符串或单个 a-z 字符。
/// 2026-07-23：数字不再允许（Alt+数字 1-9 改为定位菜单项，与字母执行快捷键区分）。
/// 旧 DB 中已有的数字 shortcut 不阻断（用户编辑时前端会过滤为字母）。
pub fn validate_shortcut(shortcut: &str) -> Result<()> {
    if shortcut.is_empty() {
        return Ok(());
    }
    if shortcut.len() == 1 && shortcut.chars().all(|c| c.is_ascii_lowercase()) {
        return Ok(());
    }
    anyhow::bail!("快捷键必须为空或单个 a-z 字符");
}

/// 检查快捷键是否已被其他项占用（排除指定 id）。返回冲突项（如有）。
pub(crate) fn check_shortcut_conflict_at(conn: &Connection, shortcut: &str, exclude_id: Option<i64>) -> Result<Option<ActionBarItem>> {
    if shortcut.is_empty() {
        return Ok(None);
    }
    let sql = match exclude_id {
        Some(_) => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1 AND id!=?2", ACTION_BAR_SELECT_COLS),
        None => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1", ACTION_BAR_SELECT_COLS),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = match exclude_id {
        Some(eid) => stmt.query_map(params![shortcut, eid], row_to_action_bar_item)?,
        None => stmt.query_map(params![shortcut], row_to_action_bar_item)?,
    };
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 浮窗用——只返回 is_enabled=1 的项。
pub fn list_action_bar_items() -> Result<Vec<ActionBarItem>> {
    ensure_db()?;
    with_db(list_action_bar_items_at)
}

pub(crate) fn list_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items WHERE is_enabled=1 ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

/// 设置页用——返回全部项（含禁用的）。
pub fn list_all_action_bar_items() -> Result<Vec<ActionBarItem>> {
    ensure_db()?;
    with_db(list_all_action_bar_items_at)
}

pub(crate) fn list_all_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn load_action_bar_item(id: i64) -> Result<Option<ActionBarItem>> {
    ensure_db()?;
    with_db(|conn| load_action_bar_item_at(conn, id))
}

pub(crate) fn load_action_bar_item_at(conn: &Connection, id: i64) -> Result<Option<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM action_bar_items WHERE id=?1", ACTION_BAR_SELECT_COLS)
    )?;
    let mut rows = stmt.query_map(params![id], row_to_action_bar_item)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

pub fn insert_action_bar_item(
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    is_enabled: bool,
    need_voice: bool,
    app_bundle_ids: &str,
) -> Result<i64> {
    ensure_db()?;
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, is_enabled, need_voice, app_bundle_ids))
}

pub(crate) fn insert_action_bar_item_at(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    is_enabled: bool,
    need_voice: bool,
    app_bundle_ids: &str,
) -> Result<i64> {
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, None)? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    let max_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) FROM action_bar_items WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, need_voice, app_bundle_ids)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?13, ?7, ?8, ?9, ?10, ?11, ?12, ?14, ?15)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, trigger_keyword, is_enabled as i32, need_voice as i32, app_bundle_ids],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_action_bar_item(
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    need_voice: bool,
    app_bundle_ids: &str,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, need_voice, app_bundle_ids))
}

pub(crate) fn update_action_bar_item_at(
    conn: &Connection,
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    need_voice: bool,
    app_bundle_ids: &str,
) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    if row.is_system && row.action_type != action_type {
        anyhow::bail!("系统内置菜单项不可更改动作类型");
    }
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, Some(id))? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    conn.execute(
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, shortcut=?8, agent=?9, accepts=?10, trigger_keyword=?11, need_voice=?12, app_bundle_ids=?13, updated_at=datetime('now') WHERE id=?14",
        params![title, icon, action_type, action_data, is_enabled as i32, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, trigger_keyword, need_voice as i32, app_bundle_ids, id],
    )?;
    Ok(())
}

pub fn delete_action_bar_item(id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_action_bar_item_at(conn, id))
}

pub(crate) fn delete_action_bar_item_at(conn: &Connection, id: i64) -> Result<()> {
    let is_system: i32 = conn.query_row(
        "SELECT is_system FROM action_bar_items WHERE id=?1", params![id], |r| r.get(0)
    ).context("菜单项不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置菜单项不可删除");
    }
    conn.execute("DELETE FROM action_bar_items WHERE id=?1 OR parent_id=?1", params![id])?;
    Ok(())
}

/// 设置菜单项的全局快捷键（Quick Execute silent 入口）。空串清除。
pub fn set_global_shortcut(id: i64, global_shortcut: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let rows = conn.execute(
            "UPDATE action_bar_items SET global_shortcut=?1, updated_at=datetime('now') WHERE id=?2",
            params![global_shortcut, id],
        )?;
        if rows == 0 {
            anyhow::bail!("菜单项不存在: {}", id);
        }
        Ok(())
    })
}

/// 查询所有注册了全局快捷键的菜单项（is_enabled + global_shortcut 非空）。
/// 启动时和设置变更后用于注册全局快捷键。
pub fn list_action_hotkeys() -> Result<Vec<ActionBarItem>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            &format!(
                "SELECT {} FROM action_bar_items WHERE global_shortcut != '' AND is_enabled = 1",
                ACTION_BAR_SELECT_COLS
            )
        )?;
        let rows = stmt.query_map([], row_to_action_bar_item)?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

/// direction < 0 = 上移，> 0 = 下移。交换同 parent 下相邻项的 sort_order。
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<()> {
    ensure_db()?;
    with_db(|conn| move_action_bar_item_at(conn, id, direction))
}

pub(crate) fn move_action_bar_item_at(conn: &Connection, id: i64, direction: i32) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;

    let neighbor_id: Option<i64> = if direction < 0 {
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order < ?2 ORDER BY sort_order DESC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    } else {
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order > ?2 ORDER BY sort_order ASC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    };

    if let Some(nid) = neighbor_id {
        let neighbor = load_action_bar_item_at(conn, nid)?.context("相邻项不存在")?;
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![neighbor.sort_order, id])?;
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![row.sort_order, nid])?;
    }
    Ok(())
}

// ── Launcher Index（统一启动器索引表：app + command）──────────────

/// 统一启动器索引表的一行（app + command 共用）。
///
/// - `type="app"`：应用索引缓存（来自文件系统扫描）；source 固定 `"applications"`，
///   alias 为本地化名、icon 为 base64 PNG，description/keywords 暂留空。
/// - `type="command"`：命令索引（brew/cargo/system 等）；alias/icon 留空，
///   source 为来源、description 为英文描述、keywords 为 LLM 生成的中英文关键字。
#[derive(Debug, Clone)]
pub struct LauncherRow {
    pub r#type: String,       // "app" | "command"
    pub name: String,
    pub path: String,
    pub alias: String,        // app 的本地化名，command 无
    pub icon: String,         // app 的 base64 icon，command 无
    pub source: String,       // command 的 brew/cargo/system，app 用 "applications"
    pub description: String,  // 英文描述
    pub keywords: String,     // LLM 生成的中英文关键字
    pub bundle_id: String,    // app 的 CFBundleIdentifier（app-aware 绑定 key），command 无
}

/// 按 type 加载启动器索引行（type='app' 返回全部应用缓存，type='command' 返回命令缓存）。
pub fn load_launcher_by_type(item_type: &str) -> Result<Vec<LauncherRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT type, name, path, alias, icon, source, description, keywords, bundle_id
             FROM launcher_index WHERE type = ?1",
        )?;
        let rows = stmt.query_map(params![item_type], |r| Ok(LauncherRow {
            r#type: r.get(0)?,
            name: r.get(1)?,
            path: r.get(2)?,
            alias: r.get(3)?,
            icon: r.get(4)?,
            source: r.get(5)?,
            description: r.get(6)?,
            keywords: r.get(7)?,
            bundle_id: r.get(8)?,
        }))?;
        Ok(collect_rows(rows, "load_launcher_by_type"))
    })
}

/// 按 type 全量替换启动器索引（事务原子：先删该 type 再插）。
///
/// **原子性保证**：DELETE + INSERT 在同一 `unchecked_transaction` 内，
/// 中途 INSERT 失败（如磁盘满）会回滚 DELETE，避免该 type 缓存被清空
/// 导致下次启动触发全量重扫 + 期间搜索无结果。
pub fn save_launcher_batch(item_type: &str, rows: &[LauncherRow]) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM launcher_index WHERE type = ?1", params![item_type])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO launcher_index
                 (type, name, path, alias, icon, source, description, keywords, bundle_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for r in rows {
                stmt.execute(params![
                    item_type, r.name, r.path, r.alias, r.icon, r.source, r.description, r.keywords, r.bundle_id,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}

/// 更新单个启动器项的 keywords（LLM 生成关键字后调用）。
/// 按 (type, path) 定位；同时刷新 updated_at。
pub fn update_launcher_keywords(item_type: &str, path: &str, keywords: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE launcher_index SET keywords = ?3, updated_at = datetime('now')
             WHERE type = ?1 AND path = ?2",
            params![item_type, path, keywords],
        )?;
        Ok(())
    })
}

// ── App Index Cache（应用索引缓存）——launcher_index 的 app wrapper ──────────
//
// load_app_index / save_app_index 是 search crate AppIndex::scan/rescan 的契约入口。
// 五元组 name/alias/path/icon/bundle_id，内部转 LauncherRow 读写 launcher_index
// 中 type='app' 的行——对 search crate 完全透明。

/// 从 DB 加载应用索引缓存。空表返回空 Vec（触发首次扫描）。
/// 返回 (name, alias, path, icon_base64, bundle_id)
pub fn load_app_index() -> Result<Vec<(String, String, String, String, String)>> {
    let rows = load_launcher_by_type("app")?;
    Ok(rows.into_iter().map(|r| (r.name, r.alias, r.path, r.icon, r.bundle_id)).collect())
}

/// 全量替换应用索引缓存（原子：DELETE 该 type + INSERT 在同一事务内）。
/// apps: (name, alias, path, icon_base64, bundle_id)
///
/// **原子性保证**：转 LauncherRow 后走 [`save_launcher_batch`]，DELETE + INSERT 同事务，
/// 中途 INSERT 失败（如磁盘满）会回滚 DELETE，避免 DB 变空表导致下次启动触发全量重扫
/// + 期间搜索无 app。
pub fn save_app_index(apps: &[(String, String, String, String, String)]) -> Result<()> {
    let launcher_rows: Vec<LauncherRow> = apps
        .iter()
        .map(|(name, alias, path, icon, bundle_id)| LauncherRow {
            r#type: "app".into(),
            name: name.clone(),
            path: path.clone(),
            alias: alias.clone(),
            icon: icon.clone(),
            source: "applications".into(),
            description: String::new(),
            keywords: String::new(),
            bundle_id: bundle_id.clone(),
        })
        .collect();
    save_launcher_batch("app", &launcher_rows)
}

// ── 搜索频次加权（search_frequency 表）───────────────────────────

/// 频次加权表的一行（search_frequency）。
#[derive(Debug, Clone)]
pub struct FreqRow {
    pub hit_count: i64,
    pub last_hit_ts: i64,
    pub query: String,
}

/// 记录一次搜索命中：hit_count+1，更新 query 和 last_hit_ts。
pub fn record_search_frequency(score_key: &str, query: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO search_frequency (score_key, query, hit_count, last_hit_ts)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(score_key) DO UPDATE SET
                hit_count = hit_count + 1,
                query = excluded.query,
                last_hit_ts = excluded.last_hit_ts",
            params![score_key, query, now],
        )
        .with_context(|| format!("record_search_frequency key={}", score_key))?;
        Ok(())
    })
}

/// 加载所有频次记录到内存 map（key → FreqRow）。
pub fn load_search_frequency() -> Result<HashMap<String, FreqRow>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT score_key, hit_count, last_hit_ts, query FROM search_frequency",
        )
        .context("load_search_frequency")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FreqRow {
                    hit_count: r.get::<_, i64>(1)?,
                    last_hit_ts: r.get::<_, i64>(2)?,
                    query: r.get::<_, String>(3)?,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    })
}

// ── Script Run（脚本执行记录）─────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRun {
    pub id: i64,
    pub item_id: i64,
    pub item_title: Option<String>,
    pub script_type: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_msg: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}

/// stdout/stderr 截断上限（64KB）
const SCRIPT_OUTPUT_LIMIT: usize = 65536;

pub fn insert_script_run(
    item_id: i64,
    script_type: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    error_msg: &str,
    started_at: &str,
    finished_at: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<i64> {
    let stdout_trunc: String = stdout.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    let stderr_trunc: String = stderr.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO script_runs (item_id, script_type, exit_code, stdout, stderr, error_msg, started_at, finished_at, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item_id, script_type, exit_code, stdout_trunc, stderr_trunc, error_msg, started_at, finished_at, duration_ms],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<ScriptRun>> {
    ensure_db()?;
    with_db(|conn| {
        let limit = limit.unwrap_or(100);
        let sql = if item_id.is_some() {
            "SELECT s.id, s.item_id, COALESCE(a.title, '已删除'), s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             WHERE s.item_id = ?2 ORDER BY s.started_at DESC LIMIT ?1"
        } else {
            "SELECT s.id, s.item_id, a.title, s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             ORDER BY s.started_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(iid) = item_id {
            stmt.query_map(params![limit, iid], row_to_script_run)?
        } else {
            stmt.query_map(params![limit], row_to_script_run)?
        };
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
}

fn row_to_script_run(row: &rusqlite::Row) -> rusqlite::Result<ScriptRun> {
    Ok(ScriptRun {
        id: row.get(0)?,
        item_id: row.get(1)?,
        item_title: row.get(2).ok(),
        script_type: row.get(3)?,
        exit_code: row.get(4)?,
        stdout: row.get(5)?,
        stderr: row.get(6)?,
        error_msg: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
        duration_ms: row.get(10)?,
    })
}

pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<()> {
    let keep = keep_recent.unwrap_or(100);
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "DELETE FROM script_runs WHERE id NOT IN (SELECT id FROM script_runs ORDER BY started_at DESC LIMIT ?1)",
            params![keep],
        )?;
        Ok(())
    })
}

/// 按 ID 批量删除执行记录。2026-07-17 新增——执行记录 TAB 的复选框删除。
pub fn delete_script_runs(ids: &[i64]) -> Result<()> {
    if ids.is_empty() { return Ok(()); }
    ensure_db()?;
    with_db(|conn| {
        // 逐条 DELETE（IDs 数量有限，100 条上限不需 IN 子句优化）
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM script_runs WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{init_test_db, INIT_SQL};
    use rusqlite::Connection;
    use std::sync::Once;

    /// 在内存 DB 上执行 INIT_SQL，返回初始化好的连接。
    fn open_init() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    /// 全局测试 DB 初始化（进程级 Once）。
    static TEST_DB_SETUP: Once = Once::new();
    fn setup_test_db() {
        TEST_DB_SETUP.call_once(|| {
            init_test_db();
        });
    }

    #[test]
    fn action_bar_shortcut_validate_and_conflict() {
        let conn = open_init();

        // 给 id=2（翻译）设快捷键 't'
        conn.execute("UPDATE action_bar_items SET shortcut='t' WHERE id=2", []).unwrap();

        // validate_shortcut: 合法（仅 a-z；数字 2026-07-23 起不再允许——留给 Alt+数字定位）
        assert!(validate_shortcut("").is_ok());
        assert!(validate_shortcut("t").is_ok());
        // validate_shortcut: 非法
        assert!(validate_shortcut("5").is_err());  // 数字不再允许
        assert!(validate_shortcut("T").is_err());  // 大写
        assert!(validate_shortcut("ab").is_err()); // 多字符
        assert!(validate_shortcut("-").is_err());  // 非法字符
        assert!(validate_shortcut(" ").is_err());  // 空格

        // check_shortcut_conflict: 't' 已被 id=2 占用
        let conflict = check_shortcut_conflict_at(&conn, "t", Some(5)).unwrap();
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().id, 2);

        // 排除自身——id=2 查 't' 不应冲突
        let self_ok = check_shortcut_conflict_at(&conn, "t", Some(2)).unwrap();
        assert!(self_ok.is_none());

        // 无冲突字符
        let free = check_shortcut_conflict_at(&conn, "z", None).unwrap();
        assert!(free.is_none());
    }

    #[test]
    fn action_bar_insert_with_shortcut() {
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "测试", "", "url", "", true, false, "q", "", "text", "", true, false, "",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.shortcut, "q");
    }

    #[test]
    fn action_bar_update_shortcut() {
        let conn = open_init();
        update_action_bar_item_at(
            &conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "p", "", "text", "", false, "",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, 5).unwrap().unwrap();
        assert_eq!(item.shortcut, "p");
    }

    #[test]
    fn action_bar_shortcut_conflict_rejected() {
        let conn = open_init();
        // id=2 设快捷键 't'
        update_action_bar_item_at(&conn, 2, "翻译", "globe", "ai", "auto_translate", true, true, false, "t", "", "text", "", false, "").unwrap();
        // id=5 也想用 't' → 应失败
        let result = update_action_bar_item_at(&conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "t", "", "text", "", false, "");
        assert!(result.is_err());
    }

    #[test]
    fn action_bar_items_has_agent_and_accepts_cols() {
        let conn = open_init();
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"agent".to_string()), "missing agent column: {:?}", cols);
        assert!(cols.contains(&"accepts".to_string()), "missing accepts column: {:?}", cols);
    }

    #[test]
    fn action_bar_item_has_agent_and_accepts_fields() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order)
             VALUES (NULL, '测试agent', 'bot', 'agent', '{{voice}}', 'claude', 'file', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.agent, "claude");
        assert_eq!(item.accepts, "file");
        assert_eq!(item.action_type, "agent");
    }

    #[test]
    fn action_bar_item_has_trigger_keyword() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, trigger_keyword, sort_order)
             VALUES (NULL, 'Quicklink测试', 'link', 'url', 'https://example.com/?q={query}', 'ql', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.trigger_keyword, "ql");
    }

    #[test]
    fn action_bar_trigger_keyword_defaults_empty() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order)
             VALUES (NULL, '普通菜单', 'bot', 'script', 'echo hi', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.trigger_keyword, "");
    }

    #[test]
    fn action_bar_non_submenu_accepts_default_text() {
        // db.sql 中非 submenu 类型 seed 项的 accepts 为 'text'（列默认值）。
        // 排除 v40/v43 外置 seed 注入的 Agent 子菜单（action_type='agent', accepts='file'）——
        // 它们有独立测试覆盖。
        let conn = open_init();
        let non_submenu: Vec<(String, String)> = conn.prepare(
            "SELECT action_type, accepts FROM action_bar_items
             WHERE action_type != 'submenu'
               AND title NOT IN ('PPT 大纲', 'PPT 制作') ORDER BY id"
        ).unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert!(non_submenu.len() > 0, "seed 应有非 submenu 项");
        for (atype, accepts) in &non_submenu {
            assert_eq!(accepts, "text", "{} 类型 accepts 应为 'text'，实际: {}", atype, accepts);
        }
    }

    /// 回归 S2：save_app_index 全量替换语义 + launcher_index wrapper 正确性。
    /// v36：save_app_index 是 launcher_index 的 wrapper（转 LauncherRow 后走
    /// save_launcher_batch）。原 v34 测试断言 UNIQUE 冲突回滚，但 v36 的 save_launcher_batch
    /// 用 INSERT OR REPLACE（按 brief），同 path 不再报错而是去重覆盖——故回归目标改为：
    /// (1) wrapper 经 launcher_index 正确写入（type='app'、source='applications'）；
    /// (2) 全量替换语义——新批次完全取代旧批次（DELETE 该 type + INSERT 在同事务），
    ///     旧 App1 应消失，新批次应用就位，不残留。
    #[test]
    fn save_app_index_atomic_on_failure() {
        setup_test_db();
        // 先写入 1 个合法应用
        save_app_index(&[("App1".into(), "应用1".into(), "/Applications/App1.app".into(), "icon1".into(), "com.app1".into())]).unwrap();
        let count: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(count, 1, "初始应有 1 条记录");

        // 全量替换为 2 个新应用（App1 不在新批次中 → 应被 DELETE 清掉）
        save_app_index(&[
            ("App2".into(), "应用2".into(), "/Applications/App2.app".into(), "icon2".into(), "com.app2".into()),
            ("App3".into(), "应用3".into(), "/Applications/App3.app".into(), "icon3".into(), "com.app3".into()),
        ]).unwrap();

        // 关键断言：全量替换——App1 应消失，新批次 2 条就位
        let count: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(count, 2, "全量替换后应有 2 条新记录，旧 App1 已清");
        let has_app1: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app' AND name='App1'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(has_app1, 0, "旧 App1 应被全量替换清除");

        // wrapper 字段映射正确：source='applications'、alias/icon 透传
        let (source, alias, icon): (String, String, String) = with_db(|c| c.query_row(
            "SELECT source, alias, icon FROM launcher_index WHERE type='app' AND name='App2'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(source, "applications", "app wrapper 应填 source='applications'");
        assert_eq!(alias, "应用2");
        assert_eq!(icon, "icon2");

        // load_app_index wrapper 读回（经 load_launcher_by_type("app") → 四元组）
        let loaded = load_app_index().unwrap();
        assert_eq!(loaded.len(), 2, "load_app_index 应返回 2 条");
    }

    #[test]
    fn action_bar_insert_agent_type_default_accepts() {
        // 通过 insert 插入 agent 类型——不传 accepts 时默认 'text'
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "我的agent", "bot", "agent", "{{voice}}", true, false, "", "claude", "file", "", true, false, "",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.accepts, "file");
        assert_eq!(item.agent, "claude");
    }

    #[test]
    fn action_bar_items_seed_has_10_items() {
        let conn = open_init();
        let items = list_all_action_bar_items_at(&conn).unwrap();
        assert!(items.len() >= 10, "expected >=10 seed items, got {}", items.len());
    }

    #[test]
    fn action_bar_items_list_enabled_filters_disabled() {
        let conn = open_init();
        let id = insert_action_bar_item_at(&conn, None, "测试禁用", "test", "url", "", true, false, "", "", "text", "", true, false, "").unwrap();
        update_action_bar_item_at(&conn, id, "测试禁用", "test", "url", "", false, true, false, "", "", "text", "", false, "").unwrap();
        let enabled = list_action_bar_items_at(&conn).unwrap();
        assert!(!enabled.iter().any(|i| i.id == id));
        let all = list_all_action_bar_items_at(&conn).unwrap();
        assert!(all.iter().any(|i| i.id == id));
        delete_action_bar_item_at(&conn, id).unwrap();
    }

    #[test]
    fn action_bar_items_system_item_cannot_delete() {
        let conn = open_init();
        let result = delete_action_bar_item_at(&conn, 1);
        assert!(result.is_err());
    }

    #[test]
    fn action_bar_items_move_swaps_order() {
        let conn = open_init();
        let id_a = insert_action_bar_item_at(&conn, None, "AAA", "test", "url", "", true, false, "", "", "text", "", true, false, "").unwrap();
        let id_b = insert_action_bar_item_at(&conn, None, "BBB", "test", "url", "", true, false, "", "", "text", "", true, false, "").unwrap();
        let a_before = load_action_bar_item_at(&conn, id_a).unwrap().unwrap();
        let b_before = load_action_bar_item_at(&conn, id_b).unwrap().unwrap();
        assert!(a_before.sort_order < b_before.sort_order);
        move_action_bar_item_at(&conn, id_a, 1).unwrap();
        let a_after = load_action_bar_item_at(&conn, id_a).unwrap().unwrap();
        assert_eq!(a_after.sort_order, b_before.sort_order);
        delete_action_bar_item_at(&conn, id_a).unwrap();
        delete_action_bar_item_at(&conn, id_b).unwrap();
    }

    // ── Task 1: search_frequency 表 + record/load fns ──

    /// record_search_frequency 写一行 → load_search_frequency 读回，验证字段。
    /// 再 record 同一 key → hit_count +1，query/last_hit_ts 更新。
    #[test]
    fn search_frequency_record_and_load_roundtrip() {
        setup_test_db();
        // 清理可能的旧数据（测试隔离）
        let _ = with_db(|conn| {
            conn.execute(
                "DELETE FROM search_frequency WHERE score_key LIKE 'test_%'",
                [],
            )?;
            Ok(())
        });
        record_search_frequency("test_key_1", "test_query").unwrap();
        let map = load_search_frequency().unwrap();
        let row = map.get("test_key_1").expect("应能读到刚写的记录");
        assert_eq!(row.hit_count, 1);
        assert_eq!(row.query, "test_query");
        assert!(row.last_hit_ts > 0);
        // 再 record 一次，hit_count 应 +1
        record_search_frequency("test_key_1", "test_query2").unwrap();
        let map = load_search_frequency().unwrap();
        assert_eq!(map.get("test_key_1").unwrap().hit_count, 2);
        assert_eq!(map.get("test_key_1").unwrap().query, "test_query2");
    }

    /// schema v35 迁移后 search_frequency 表应存在于 sqlite_master。
    #[test]
    fn search_frequency_table_exists_after_init() {
        setup_test_db();
        let exists: bool = with_db(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='search_frequency'",
            )?;
            let mut found = false;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                if r?.contains("search_frequency") {
                    found = true;
                }
            }
            Ok(found)
        })
        .unwrap_or(false);
        assert!(exists, "search_frequency 表应在 schema v35 后存在");
    }
}
