//! 剪贴板收藏（clipboard_favorites）表 CRUD。
//! Task 3 实现——当前是空 stub 让 Task 1 编译通过。

use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
pub struct ClipboardFavorite {
    pub id: String,
    pub history_id: String,
    pub is_deleted: i64,
    pub created_at: String,
    pub updated_at: String,
    pub sync_md5: Option<String>,
}

const COLS: &str = "id, history_id, is_deleted, created_at, updated_at, sync_md5";

fn parse_favorite(row: &rusqlite::Row) -> rusqlite::Result<ClipboardFavorite> {
    Ok(ClipboardFavorite {
        id: row.get(0)?,
        history_id: row.get(1)?,
        is_deleted: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        sync_md5: row.get(5)?,
    })
}

// ── _at 变体（接 &Connection，测试 + sync 用）──

pub(crate) fn insert_favorite_at(
    conn: &Connection,
    id: &str,
    history_id: &str,
    is_deleted: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO clipboard_favorites (id, history_id, is_deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, datetime('now'), datetime('now'))",
        params![id, history_id, is_deleted],
    )?;
    Ok(())
}

pub(crate) fn soft_delete_favorite_at(conn: &Connection, id: &str, epoch_secs: i64) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_favorites SET is_deleted = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![epoch_secs, id],
    )?;
    Ok(())
}

pub(crate) fn list_active_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM clipboard_favorites WHERE is_deleted = 0 ORDER BY created_at DESC"
    ))?;
    let rows = stmt.query_map([], parse_favorite)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn list_all_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites"))?;
    let rows = stmt.query_map([], parse_favorite)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ── pub 包装（走 ensure_db / with_db）──

use crate::db::{ensure_db, with_db};

pub fn insert_favorite(id: &str, history_id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_favorite_at(conn, id, history_id, 0))
}

pub fn soft_delete_favorite(id: &str, epoch_secs: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| soft_delete_favorite_at(conn, id, epoch_secs))
}

pub fn list_active_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_active_favorites_at)
}

pub fn list_all_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_all_favorites_at)
}

pub fn load_favorite(id: &str) -> Result<Option<ClipboardFavorite>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites WHERE id = ?1"))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_favorite(row)?)),
            None => Ok(None),
        }
    })
}

pub fn load_favorite_by_history(history_id: &str) -> Result<Option<ClipboardFavorite>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM clipboard_favorites WHERE history_id = ?1 ORDER BY is_deleted ASC LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![history_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_favorite(row)?)),
            None => Ok(None),
        }
    })
}

pub fn restore_favorite(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE clipboard_favorites SET is_deleted = 0, updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    })
}

// ── 剪贴板历史行读取 + UPSERT（剪贴板收藏同步用）──

/// `clipboard_history` 的可序列化镜像——只含 sync 跨设备传播的字段。
///
/// 与 `octopus_sync::clipboard::HistoryRowJson` 对齐——本 struct 不导出给 sync crate，
/// 而是作为 DB → sync 的中转：DB 读出 `HistoryRowData`，sync crate 用 `HistoryRowJson`
/// 序列化后加密写入 favorite 文件。字段集合相同（camelCase 由 sync 端 serde 处理）。
#[derive(Debug, Clone)]
pub struct HistoryRowData {
    pub id: String,
    pub item_type: String,
    pub content: String,
    pub ref_data: Option<String>,
    pub meta_info: Option<String>,
    pub is_rich: bool,
    pub created_at: String,
    pub segments: Option<String>,
}

/// 按 id 读单个 clipboard_history 行（含收藏文件所需全部字段）。不存在返 None。
pub fn load_clipboard_history_row(id: &str) -> Result<Option<HistoryRowData>> {
    ensure_db()?;
    with_db(|conn| load_clipboard_history_row_at(conn, id))
}

pub(crate) fn load_clipboard_history_row_at(
    conn: &Connection,
    id: &str,
) -> Result<Option<HistoryRowData>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, content, ref_data, meta_info, is_rich, created_at, segments
         FROM clipboard_history WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(HistoryRowData {
            id: row.get(0)?,
            item_type: row.get(1)?,
            content: row.get(2)?,
            ref_data: row.get(3)?,
            meta_info: row.get(4)?,
            is_rich: row.get::<_, i64>(5)? != 0,
            created_at: row.get(6)?,
            segments: row.get(7)?,
        })),
        None => Ok(None),
    }
}

/// sync pull 用——把远程拉来的历史行 UPSERT 进 clipboard_history（按 id 唯一）。
///
/// 不动 `is_favorite` / `is_deleted` / `has_thumbnail`（这些是本地状态，由调用方按需调
/// [`set_clipboard_is_favorite`] 等单独设置）。`created_at` 来自远程（跨设备一致），
/// 缺失时回退到 `datetime('now')`。
pub fn upsert_clipboard_history_sync(
    id: &str,
    item_type: &str,
    content: &str,
    ref_data: Option<&str>,
    meta_info: Option<&str>,
    is_rich: bool,
    created_at: &str,
    segments: Option<&str>,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        upsert_clipboard_history_sync_at(
            conn, id, item_type, content, ref_data, meta_info, is_rich, created_at, segments,
        )
    })
}

pub(crate) fn upsert_clipboard_history_sync_at(
    conn: &Connection,
    id: &str,
    item_type: &str,
    content: &str,
    ref_data: Option<&str>,
    meta_info: Option<&str>,
    is_rich: bool,
    created_at: &str,
    segments: Option<&str>,
) -> Result<()> {
    let created = if created_at.is_empty() {
        super::now_string()
    } else {
        created_at.to_string()
    };
    conn.execute(
        "INSERT INTO clipboard_history
            (id, item_type, content, ref_data, meta_info, is_rich, created_at, segments)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            item_type=excluded.item_type,
            content=excluded.content,
            ref_data=excluded.ref_data,
            meta_info=excluded.meta_info,
            is_rich=excluded.is_rich,
            segments=excluded.segments",
        params![id, item_type, content, ref_data, meta_info, is_rich as i64, created, segments],
    )?;
    Ok(())
}

/// 设置某历史行的 is_favorite 标记——sync pull favorite 后同步本地 favorite 状态用。
pub fn set_clipboard_is_favorite(id: &str, is_fav: bool) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE clipboard_history SET is_favorite = ?1 WHERE id = ?2",
            params![is_fav as i64, id],
        )?;
        Ok(())
    })
}

/// sync upsert（含 is_deleted + sync_md5）——对称 hotword upsert_hotword_set
pub fn upsert_favorite_sync(fav: &ClipboardFavorite) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM clipboard_favorites WHERE id = ?1",
                params![fav.id],
                |r| r.get(0),
            )
            .ok();
        if existing.is_some() {
            conn.execute(
                "UPDATE clipboard_favorites SET history_id=?1, is_deleted=?2, sync_md5=?3, updated_at=?4 WHERE id=?5",
                params![fav.history_id, fav.is_deleted, fav.sync_md5, fav.updated_at, fav.id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO clipboard_favorites (id, history_id, is_deleted, created_at, updated_at, sync_md5)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![fav.id, fav.history_id, fav.is_deleted, fav.created_at, fav.updated_at, fav.sync_md5],
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;

    fn setup() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    #[test]
    fn insert_and_list_favorite() {
        let conn = setup();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at) VALUES (?1, 'text', 'hello', '2026-08-05')",
            ["hist-uuid-1"],
        ).unwrap();
        insert_favorite_at(&conn, "fav-uuid-1", "hist-uuid-1", 0).unwrap();
        let favs = list_active_favorites_at(&conn).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].id, "fav-uuid-1");
        assert_eq!(favs[0].history_id, "hist-uuid-1");
    }

    #[test]
    fn soft_delete_and_tombstone() {
        let conn = setup();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at) VALUES (?1, 'text', 'hello', '2026-08-05')",
            ["hist-uuid-2"],
        ).unwrap();
        insert_favorite_at(&conn, "fav-uuid-2", "hist-uuid-2", 0).unwrap();
        soft_delete_favorite_at(&conn, "fav-uuid-2", 1722835200).unwrap();
        let active = list_active_favorites_at(&conn).unwrap();
        assert!(active.iter().all(|f| f.id != "fav-uuid-2"));
        let all = list_all_favorites_at(&conn).unwrap();
        assert!(all.iter().any(|f| f.id == "fav-uuid-2" && f.is_deleted > 0));
    }
}
