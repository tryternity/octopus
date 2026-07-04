use anyhow::Result;
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::model::*;

const FTS_REBUILD_THRESHOLD: u32 = 10;
static DELETE_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn rebuild_fts_index(conn: &Connection) -> Result<()> {
    conn.execute("INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')", [])?;
    Ok(())
}

fn track_deletes(conn: &Connection, deleted: u32) {
    let prev = DELETE_COUNT.fetch_add(deleted, Ordering::Relaxed);
    if prev + deleted >= FTS_REBUILD_THRESHOLD {
        DELETE_COUNT.store(0, Ordering::Relaxed);
        if let Err(e) = rebuild_fts_index(conn) {
            log::warn!("FTS5 rebuild failed: {}", e);
        }
    }
}

// ── INSERT ──

pub fn insert_clipboard_item(conn: &Connection, item: &NewClipboardItem) -> Result<i64> {
    let meta_json = item.meta_info.as_ref().map(|m| serde_json::to_string(m).unwrap_or_default());
    let (content, ref_data) = match &item.item_type {
        ItemType::Image => (String::new(), item.ref_data.clone()),
        ItemType::File => (String::new(), item.ref_data.clone()),
        _ => (item.content.clone(), None),
    };
    conn.execute(
        "INSERT INTO clipboard_history (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)",
        params![
            item.id, item.item_type.as_str(), content, ref_data, meta_json,
            item.is_rich, item.created_at, item.has_thumbnail.unwrap_or(0),
        ],
    )?;
    Ok(item.id)
}

pub fn insert_asr_item(conn: &Connection, text: &str, engine: &str, model: &str, segments: Option<&str>) -> Result<i64> {
    let meta = MetaInfo {
        engine: Some(engine.to_string()),
        model: Some(model.to_string()),
        char_count: Some(text.chars().count()),
        ..Default::default()
    };
    let meta_json = serde_json::to_string(&meta).unwrap_or_default();
    insert_with_unique_id(|id| {
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments)
             VALUES (?, 'voice', ?, NULL, ?, 0, 0, ?, 0, ?)",
            params![id, text, meta_json, iso_now(), segments],
        )
    })
}

pub fn insert_ocr_item(conn: &Connection, text: &str, engine: &str, model: &str) -> Result<i64> {
    let meta = MetaInfo {
        engine: Some(engine.to_string()),
        model: Some(model.to_string()),
        char_count: Some(text.chars().count()),
        ..Default::default()
    };
    let meta_json = serde_json::to_string(&meta).unwrap_or_default();
    insert_with_unique_id(|id| {
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail)
             VALUES (?, 'ocr', ?, NULL, ?, 0, 0, ?, 0)",
            params![id, text, meta_json, iso_now()],
        )
    })
}

// ── DEDUP ──

pub fn find_by_content_hash(conn: &Connection, hash: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM clipboard_history WHERE ref_data = ? AND item_type = 'image' LIMIT 1")?;
    match stmt.query_row(params![hash], |r| r.get::<_, i64>(0)) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn find_by_text(conn: &Connection, text: &str, item_type: ItemType) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM clipboard_history WHERE content = ? AND item_type = ? LIMIT 1"
    )?;
    match stmt.query_row(params![text, item_type.as_str()], |r| r.get::<_, i64>(0)) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn touch_created_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET created_at = ? WHERE id = ?", params![iso_now(), id])?;
    Ok(())
}

// ── QUERY ──

const SELECT_COLS: &str = "id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments";

pub fn query_history(conn: &Connection, filter: &QueryFilter) -> Result<Vec<ClipboardItem>> {
    let limit = filter.size.max(1) as i64;
    let offset = ((filter.page.saturating_sub(1)) * filter.size) as i64;
    let where_clause = build_where(filter);

    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return query_with_search(conn, search, &where_clause, limit, offset);
        }
    }

    let sql = format!(
        "SELECT {} FROM clipboard_history {} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        SELECT_COLS,
        if where_clause.is_empty() { String::new() } else { format!(" WHERE {}", where_clause) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit, offset], row_to_item)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn get_item_by_id(conn: &Connection, id: i64) -> Result<Option<ClipboardItem>> {
    let sql = format!("SELECT {} FROM clipboard_history WHERE id = ?", SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    match stmt.query_row(params![id], row_to_item) {
        Ok(item) => Ok(Some(item)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn query_with_search(conn: &Connection, search: &str, extra_where: &str, limit: i64, offset: i64) -> Result<Vec<ClipboardItem>> {
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = format!(
            "SELECT {} FROM clipboard_history WHERE content LIKE ?{} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
            SELECT_COLS,
            if extra_where.is_empty() { String::new() } else { format!(" AND {}", extra_where) }
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, limit, offset], row_to_item)?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = format!(
        "SELECT c.id, c.item_type, c.content, c.ref_data, c.meta_info, c.is_favorite, c.is_rich, c.created_at, c.has_thumbnail, c.segments
         FROM clipboard_history_fts f JOIN clipboard_history c ON c.id = f.rowid
         WHERE f.content MATCH ?{} ORDER BY c.created_at DESC, c.id DESC LIMIT ? OFFSET ?",
        if extra_where.is_empty() { String::new() } else { format!(" AND {}", extra_where) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![phrase, limit, offset], row_to_item)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn count_history(conn: &Connection, filter: &QueryFilter) -> Result<i64> {
    let where_clause = build_where(filter);
    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return count_with_search(conn, search, &where_clause);
        }
    }
    let sql = if where_clause.is_empty() {
        "SELECT COUNT(*) FROM clipboard_history".to_string()
    } else {
        format!("SELECT COUNT(*) FROM clipboard_history WHERE {}", where_clause)
    };
    let count: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(count)
}

fn count_with_search(conn: &Connection, search: &str, extra_where: &str) -> Result<i64> {
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = if extra_where.is_empty() {
            "SELECT COUNT(*) FROM clipboard_history WHERE content LIKE ?".to_string()
        } else {
            format!("SELECT COUNT(*) FROM clipboard_history WHERE content LIKE ? AND {}", extra_where)
        };
        return Ok(conn.query_row(&sql, params![pattern], |r| r.get(0))?);
    }
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = if extra_where.is_empty() {
        "SELECT COUNT(*) FROM clipboard_history_fts f JOIN clipboard_history c ON c.id = f.rowid WHERE f.content MATCH ?".to_string()
    } else {
        format!("SELECT COUNT(*) FROM clipboard_history_fts f JOIN clipboard_history c ON c.id = f.rowid WHERE f.content MATCH ? AND {}", extra_where)
    };
    Ok(conn.query_row(&sql, params![phrase], |r| r.get(0))?)
}

fn build_where(filter: &QueryFilter) -> String {
    match filter.filter.as_str() {
        "all" | "" => String::new(),
        "asr" | "voice" => "item_type = 'voice'".to_string(),
        "ocr" => "item_type = 'ocr'".to_string(),
        "text" => "item_type = 'text'".to_string(),
        "image" => "item_type = 'image'".to_string(),
        "file" => "item_type = 'file'".to_string(),
        "favorite" => "is_favorite = 1".to_string(),
        "unfavorite" => "is_favorite = 0".to_string(),
        _ => String::new(),
    }
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ClipboardItem> {
    let item_type_str: String = row.get(1)?;
    let meta_json: Option<String> = row.get(4)?;
    let meta_info = meta_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<MetaInfo>(s).ok());

    Ok(ClipboardItem {
        id: row.get(0)?,
        item_type: ItemType::from_str(&item_type_str),
        content: row.get(2)?,
        ref_data: row.get(3)?,
        meta_info,
        is_favorite: row.get::<_, i64>(5)? != 0,
        is_rich: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
        has_thumbnail: row.get::<_, i64>(8)? != 0,
        segments: row.get(9)?,
    })
}

// ── UPDATE ──

pub fn toggle_favorite(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET is_favorite = CASE is_favorite WHEN 0 THEN 1 ELSE 0 END WHERE id = ?", params![id])?;
    Ok(())
}

pub fn update_content(conn: &Connection, id: i64, text: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET content = ? WHERE id = ?", params![text, id])?;
    Ok(())
}

/// 更新 voice 条目的 segments（润色/编辑后段模型更新）
pub fn update_segments(conn: &Connection, id: i64, segments: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET segments = ? WHERE id = ?", params![segments, id])?;
    Ok(())
}

// ── DELETE ──

pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    let ref_data: Option<String> = conn.query_row(
        "SELECT ref_data FROM clipboard_history WHERE id = ?", params![id],
        |r| r.get::<_, Option<String>>(0),
    ).ok().flatten();

    conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;
    track_deletes(conn, 1);

    if let Some(hash) = ref_data.as_deref() {
        delete_image_if_unreferenced(conn, hash);
    }
    Ok(())
}

pub fn delete_items(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() { return Ok(0); }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params_vec: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = conn.execute(&format!("DELETE FROM clipboard_history WHERE id IN ({})", placeholders), params_vec.as_slice())?;
    if rows > 0 {
        track_deletes(conn, rows as u32);
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}

pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let rows = if keep_favorite {
        conn.execute("DELETE FROM clipboard_history WHERE is_favorite = 0", [])?
    } else {
        conn.execute("DELETE FROM clipboard_history", [])?
    };
    if rows > 0 {
        track_deletes(conn, rows as u32);
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}

// ── image_data CRUD ──

pub fn insert_image_data(conn: &Connection, hash: &str, webp_blob: &[u8], thumb_blob: &[u8], width: i64, height: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO image_data (hash, blob, thumb, image_type, width, height, created_at) VALUES (?, ?, ?, 'webp', ?, ?, ?)",
        params![hash, webp_blob, thumb_blob, width, height, iso_now()],
    )?;
    Ok(())
}

pub fn get_image_blob(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT blob FROM image_data WHERE hash = ?")?;
    match stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0)) {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_image_thumb(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT thumb FROM image_data WHERE hash = ?")?;
    match stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0)) {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn cleanup_unreferenced_images(conn: &Connection) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM image_data WHERE hash NOT IN (SELECT DISTINCT ref_data FROM clipboard_history WHERE item_type = 'image' AND ref_data IS NOT NULL)",
        [],
    )?;
    Ok(deleted)
}

fn delete_image_if_unreferenced(conn: &Connection, hash: &str) {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE ref_data = ? AND item_type = 'image'",
        params![hash], |r| r.get(0),
    ).unwrap_or(0);
    if count == 0 {
        let _ = conn.execute("DELETE FROM image_data WHERE hash = ?", params![hash]);
    }
}

pub fn count_all(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM clipboard_history", [], |r| r.get(0))?)
}

// ── 构造新条目的辅助结构 ──

pub struct NewClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub content: String,
    pub ref_data: Option<String>,
    pub meta_info: Option<MetaInfo>,
    pub created_at: String,
    pub has_thumbnail: Option<i64>,
    pub is_rich: bool,
}

// ── 时间辅助 ──

pub fn chrono_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn insert_with_unique_id<F>(mut insert_fn: F) -> Result<i64>
where F: FnMut(i64) -> rusqlite::Result<usize>,
{
    let base = chrono_millis();
    let mut id = base;
    loop {
        match insert_fn(id) {
            Ok(_) => return Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => {
                id += 1;
                if id > base + 1000 { anyhow::bail!("insert_with_unique_id: 主键冲突，1000 次自增重试仍失败"); }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s)
}

pub fn epoch_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as u32;
    let remainder = (secs % 86400) as u32;
    let h = remainder / 3600;
    let mi = (remainder % 3600) / 60;
    let s = remainder % 60;
    let mut year = 1970u32;
    let mut remaining_days = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let year_days = if leap { 366 } else { 365 };
        if remaining_days >= year_days { remaining_days -= year_days; year += 1; } else { break; }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &md in &month_days {
        if remaining_days < md { break; }
        remaining_days -= md;
        month += 1;
    }
    (year, month, remaining_days + 1, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let sql = include_str!("../../infra/src/db.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_query_text() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1000, item_type: ItemType::Text, content: "hello world".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        let result = query_history(&conn, &QueryFilter { filter: "all".into(), search: None, page: 1, size: 10 }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "hello world");
        assert_eq!(result[0].item_type, ItemType::Text);
    }

    #[test]
    fn test_insert_voice() {
        let conn = open_test_db();
        insert_asr_item(&conn, "识别文本", "sensevoice", "", None).unwrap();
        let result = query_history(&conn, &QueryFilter { filter: "voice".into(), search: None, page: 1, size: 10 }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "识别文本");
        assert_eq!(result[0].item_type, ItemType::Voice);
        assert!(result[0].meta_info.is_some());
        assert_eq!(result[0].meta_info.as_ref().unwrap().engine.as_deref(), Some("sensevoice"));
    }

    #[test]
    fn test_update_content() {
        let conn = open_test_db();
        let id = 1700;
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: "原始".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        update_content(&conn, id, "改后").unwrap();
        let item = get_item_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(item.content, "改后");
    }

    #[test]
    fn test_filter_by_type() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1, item_type: ItemType::Text, content: "text1".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        insert_asr_item(&conn, "voice1", "engine", "", None).unwrap();
        let text_only = query_history(&conn, &QueryFilter { filter: "text".into(), search: None, page: 1, size: 10 }).unwrap();
        let voice_only = query_history(&conn, &QueryFilter { filter: "voice".into(), search: None, page: 1, size: 10 }).unwrap();
        assert_eq!(text_only.len(), 1);
        assert_eq!(voice_only.len(), 1);
    }
}
