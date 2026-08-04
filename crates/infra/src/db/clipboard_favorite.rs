//! 剪贴板收藏（clipboard_favorites）表 CRUD。
//!
//! 极简 4 字段——`history_id` 直接作主键（= clipboard_history.id，一对一），
//! `is_deleted` 是 tombstone（0=active / >0=epoch 秒），`updated_at` 给 sync 比时间戳，
//! `sync_md5` 存 history 内容指纹（history 行内容可编辑，sync 用它检测变化）。
//! 内容真相在 clipboard_history，favorite 只是「收藏」状态标记 + 同步锚点。

use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
pub struct ClipboardFavorite {
    pub history_id: String,        // PK = clipboard_history.id
    pub is_deleted: i64,           // 0=active，>0=epoch 秒 tombstone
    pub updated_at: String,
    pub sync_md5: Option<String>,  // history 内容指纹（检测 history 行编辑）
}

const COLS: &str = "history_id, is_deleted, updated_at, sync_md5";

fn parse_favorite(row: &rusqlite::Row) -> rusqlite::Result<ClipboardFavorite> {
    Ok(ClipboardFavorite {
        history_id: row.get(0)?,
        is_deleted: row.get(1)?,
        updated_at: row.get(2)?,
        sync_md5: row.get(3)?,
    })
}

// ── _at 变体（接 &Connection，测试 + sync 用）──

pub(crate) fn insert_favorite_at(conn: &Connection, history_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO clipboard_favorites (history_id, is_deleted, updated_at, sync_md5)
         VALUES (?1, 0, datetime('now'), NULL)",
        params![history_id],
    )?;
    Ok(())
}

pub(crate) fn soft_delete_favorite_at(
    conn: &Connection,
    history_id: &str,
    epoch_secs: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_favorites SET is_deleted = ?1, updated_at = datetime('now') WHERE history_id = ?2",
        params![epoch_secs, history_id],
    )?;
    Ok(())
}

pub(crate) fn list_active_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM clipboard_favorites WHERE is_deleted = 0"
    ))?;
    let rows = stmt.query_map([], parse_favorite)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn list_all_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites"))?;
    let rows = stmt.query_map([], parse_favorite)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn load_favorite_at(
    conn: &Connection,
    history_id: &str,
) -> Result<Option<ClipboardFavorite>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites WHERE history_id = ?1"))?;
    let mut rows = stmt.query(params![history_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(parse_favorite(row)?)),
        None => Ok(None),
    }
}

pub(crate) fn restore_favorite_at(conn: &Connection, history_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_favorites SET is_deleted = 0, updated_at = datetime('now') WHERE history_id = ?1",
        params![history_id],
    )?;
    Ok(())
}

/// sync pull 用——按 history_id UPSERT，显式带 updated_at + sync_md5（来自远程文件，
/// 不写 datetime('now')）。sync_md5 由调用方从 history_row 内容算好后传入。
pub(crate) fn upsert_favorite_sync_at(
    conn: &Connection,
    fav: &ClipboardFavorite,
) -> Result<()> {
    conn.execute(
        "INSERT INTO clipboard_favorites (history_id, is_deleted, updated_at, sync_md5)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(history_id) DO UPDATE SET
            is_deleted=excluded.is_deleted,
            updated_at=excluded.updated_at,
            sync_md5=excluded.sync_md5",
        params![fav.history_id, fav.is_deleted, fav.updated_at, fav.sync_md5],
    )?;
    Ok(())
}

/// sync push 后用——更新 sync_md5（export 后写入磁盘指纹，下次 merge 据此比对冲突）。
pub(crate) fn set_sync_md5_at(conn: &Connection, history_id: &str, md5: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_favorites SET sync_md5 = ?1 WHERE history_id = ?2",
        params![md5, history_id],
    )?;
    Ok(())
}

// ── pub 包装（走 ensure_db / with_db）──

use crate::db::{ensure_db, with_db};

pub fn insert_favorite(history_id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_favorite_at(conn, history_id))
}

pub fn soft_delete_favorite(history_id: &str, epoch_secs: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| soft_delete_favorite_at(conn, history_id, epoch_secs))
}

pub fn list_active_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_active_favorites_at)
}

pub fn list_all_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_all_favorites_at)
}

pub fn load_favorite(history_id: &str) -> Result<Option<ClipboardFavorite>> {
    ensure_db()?;
    with_db(|conn| load_favorite_at(conn, history_id))
}

pub fn restore_favorite(history_id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| restore_favorite_at(conn, history_id))
}

/// sync upsert——显式带 is_deleted + updated_at + sync_md5（来自远程文件，不写 datetime('now')）。
pub fn upsert_favorite_sync(fav: &ClipboardFavorite) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_favorite_sync_at(conn, fav))
}

/// sync push 后用——更新 sync_md5（export 后写入磁盘指纹）。
pub fn set_sync_md5(history_id: &str, md5: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| set_sync_md5_at(conn, history_id, md5))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;

    fn setup() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    fn insert_history(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at) VALUES (?1, 'text', 'hello', '2026-08-05')",
            [id],
        )
        .unwrap();
    }

    #[test]
    fn insert_and_list_favorite() {
        let conn = setup();
        insert_history(&conn, "hist-1");
        insert_favorite_at(&conn, "hist-1").unwrap();
        let favs = list_active_favorites_at(&conn).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].history_id, "hist-1");
        assert_eq!(favs[0].is_deleted, 0);
    }

    #[test]
    fn soft_delete_and_tombstone() {
        let conn = setup();
        insert_history(&conn, "hist-2");
        insert_favorite_at(&conn, "hist-2").unwrap();
        soft_delete_favorite_at(&conn, "hist-2", 1722835200).unwrap();
        let active = list_active_favorites_at(&conn).unwrap();
        assert!(active.iter().all(|f| f.history_id != "hist-2"));
        let all = list_all_favorites_at(&conn).unwrap();
        assert!(all
            .iter()
            .any(|f| f.history_id == "hist-2" && f.is_deleted > 0));
    }

    #[test]
    fn restore_favorite_works() {
        let conn = setup();
        insert_history(&conn, "hist-3");
        insert_favorite_at(&conn, "hist-3").unwrap();
        soft_delete_favorite_at(&conn, "hist-3", 1700000000).unwrap();
        restore_favorite_at(&conn, "hist-3").unwrap();
        let active = list_active_favorites_at(&conn).unwrap();
        assert!(active.iter().any(|f| f.history_id == "hist-3"));
    }

    #[test]
    fn upsert_sync_insert_and_update() {
        let conn = setup();
        insert_history(&conn, "hist-4");
        // 第一次：INSERT（远程值，sync_md5 = 内容指纹）
        upsert_favorite_sync_at(
            &conn,
            &ClipboardFavorite {
                history_id: "hist-4".into(),
                is_deleted: 0,
                updated_at: "2026-08-01 10:00:00".into(),
                sync_md5: Some("md5a".into()),
            },
        )
        .unwrap();
        let fav = load_favorite_at(&conn, "hist-4").unwrap().unwrap();
        assert_eq!(fav.is_deleted, 0);
        assert_eq!(fav.updated_at, "2026-08-01 10:00:00");
        assert_eq!(fav.sync_md5.as_deref(), Some("md5a"));

        // 第二次：UPDATE（远程改了 is_deleted + updated_at + sync_md5）
        upsert_favorite_sync_at(
            &conn,
            &ClipboardFavorite {
                history_id: "hist-4".into(),
                is_deleted: 1700000123,
                updated_at: "2026-08-02 11:00:00".into(),
                sync_md5: Some("md5b".into()),
            },
        )
        .unwrap();
        let fav = load_favorite_at(&conn, "hist-4").unwrap().unwrap();
        assert_eq!(fav.is_deleted, 1700000123);
        assert_eq!(fav.updated_at, "2026-08-02 11:00:00");
        assert_eq!(fav.sync_md5.as_deref(), Some("md5b"));
    }

    #[test]
    fn set_sync_md5_updates_existing_row() {
        let conn = setup();
        insert_history(&conn, "hist-5");
        insert_favorite_at(&conn, "hist-5").unwrap();
        // insert 后 sync_md5 = NULL
        let fav = load_favorite_at(&conn, "hist-5").unwrap().unwrap();
        assert!(fav.sync_md5.is_none(), "新建 favorite sync_md5 应为 NULL");

        // set_sync_md5 写入指纹
        set_sync_md5_at(&conn, "hist-5", "fingerprint").unwrap();
        let fav = load_favorite_at(&conn, "hist-5").unwrap().unwrap();
        assert_eq!(fav.sync_md5.as_deref(), Some("fingerprint"));
    }
}
