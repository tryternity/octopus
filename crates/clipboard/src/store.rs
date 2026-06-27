use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::model::*;

/// 删除计数器：累计达阈值后自动 rebuild FTS5 索引，防止影子表膨胀。
const FTS_REBUILD_THRESHOLD: u32 = 10;
static DELETE_COUNT: AtomicU32 = AtomicU32::new(0);

/// 重建 FTS5 索引。应用启动时调用一次。
pub fn rebuild_fts_index(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')",
        [],
    )?;
    Ok(())
}

/// 累加删除计数，达到阈值时自动 rebuild FTS5 索引。
fn track_deletes(conn: &Connection, deleted: u32) {
    let prev = DELETE_COUNT.fetch_add(deleted, Ordering::Relaxed);
    if prev + deleted >= FTS_REBUILD_THRESHOLD {
        DELETE_COUNT.store(0, Ordering::Relaxed);
        if let Err(e) = rebuild_fts_index(conn) {
            log::warn!("FTS5 rebuild failed: {}", e);
        } else {
            log::info!("FTS5 index rebuilt after {} deletes", prev + deleted);
        }
    }
}

/// 插入剪贴板条目（来自外部复制）。返回插入的 id。
pub fn insert_clipboard_item(conn: &Connection, item: &NewClipboardItem) -> Result<i64> {
    let id = item.id;
    conn.execute(
        "INSERT INTO clipboard_history
         (id, item_type, source, content, search_text, is_favorite, created_at,
          blob_hash, width, height, has_thumbnail, file_count, is_rich)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            item.item_type.as_str(),
            "clipboard",
            item.content,
            item.search_text,
            item.created_at,
            item.blob_hash,
            item.width,
            item.height,
            item.has_thumbnail.unwrap_or(0),
            item.file_count,
            item.is_rich,
        ],
    )
    .context("insert clipboard_history")?;
    Ok(id)
}

/// 插入 ASR 识别文本条目。返回插入的 id。
pub fn insert_asr_item(conn: &Connection, text: &str, asr_meta: AsrMeta) -> Result<i64> {
    let id = chrono_millis();
    conn.execute(
        "INSERT INTO clipboard_history
         (id, item_type, source, content, search_text, is_favorite, created_at,
          transcription_id, polish_status, engine, model)
         VALUES (?, 'text', 'asr', ?, ?, 0, ?, ?, ?, ?, ?)",
        params![
            id,
            text,
            text,
            iso_now(),
            asr_meta.transcription_id,
            asr_meta.polish_status,
            asr_meta.engine,
            asr_meta.model,
        ],
    )
    .context("insert asr clipboard_history")?;
    Ok(id)
}

/// 按 hash 去重查找。存在返回 id，不存在返回 None。
pub fn find_by_content_hash(conn: &Connection, blob_hash: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM clipboard_history WHERE blob_hash = ? LIMIT 1")?;
    let row = stmt.query_row(params![blob_hash], |r| r.get::<_, i64>(0));
    match row {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 按文本内容去重查找。存在返回 id。
pub fn find_by_text(conn: &Connection, text: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM clipboard_history WHERE content = ? AND source = 'clipboard' AND item_type = 'text' LIMIT 1"
    )?;
    let row = stmt.query_row(params![text], |r| r.get::<_, i64>(0));
    match row {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 更新条目的 created_at（重复复制时刷新到顶部）。
pub fn touch_created_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET created_at = ? WHERE id = ?",
        params![iso_now(), id],
    )?;
    Ok(())
}

/// 查询历史列表（带过滤 + 分页 + FTS5 搜索）。
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
        "SELECT id, item_type, source, content, is_favorite, created_at,
                blob_hash, width, height, has_thumbnail, file_count, is_rich,
                transcription_id, polish_status, engine, model
         FROM clipboard_history
         {}
         ORDER BY created_at DESC
         LIMIT ? OFFSET ?",
        if where_clause.is_empty() { String::new() } else { format!("WHERE {}", where_clause) }
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit, offset], row_to_item)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_with_search(
    conn: &Connection,
    search: &str,
    extra_where: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<ClipboardItem>> {
    let pattern = format!("%{}%", search);
    let sql = format!(
        "SELECT id, item_type, source, content, is_favorite, created_at,
                blob_hash, width, height, has_thumbnail, file_count, is_rich,
                transcription_id, polish_status, engine, model
         FROM clipboard_history
         WHERE search_text LIKE ?
         {}
         ORDER BY created_at DESC
         LIMIT ? OFFSET ?",
        if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![pattern, limit, offset],
        row_to_item,
    )?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn build_where(filter: &QueryFilter) -> String {
    let mut conditions: Vec<String> = Vec::new();

    match filter.filter.as_str() {
        "all" | "" => {}
        "asr" => { conditions.push("source = 'asr'".to_string()); }
        "text" => {
            conditions.push("item_type = 'text'".to_string());
            conditions.push("source = 'clipboard'".to_string());
        }
        "image" => { conditions.push("item_type = 'image'".to_string()); }
        "file" => { conditions.push("item_type = 'file'".to_string()); }
        "favorite" => { conditions.push("is_favorite = 1".to_string()); }
        _ => {}
    }

    conditions.join(" AND ")
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ClipboardItem> {
    let item_type_str: String = row.get(1)?;
    let source_str: String = row.get(2)?;
    let blob_hash: Option<String> = row.get(6)?;
    let width: Option<i64> = row.get(7)?;
    let height: Option<i64> = row.get(8)?;
    let has_thumb: Option<i64> = row.get(9)?;
    let file_count: Option<i64> = row.get(10)?;
    let transcription_id: Option<i64> = row.get(12)?;
    let polish_status: Option<String> = row.get(13)?;
    let engine: Option<String> = row.get(14)?;
    let model: Option<String> = row.get(15)?;

    let image_meta = blob_hash.as_ref().map(|h| ImageMeta {
        blob_hash: h.clone(),
        width: width.unwrap_or(0) as u32,
        height: height.unwrap_or(0) as u32,
        has_thumbnail: has_thumb.unwrap_or(0) == 1,
    });

    let file_meta = file_count.map(|c| FileMeta {
        file_count: c as usize,
        paths: Vec::new(),
    });

    let asr_meta = if transcription_id.is_some() {
        Some(AsrMeta {
            transcription_id: transcription_id.unwrap(),
            polish_status: polish_status.unwrap_or_default(),
            engine: engine.unwrap_or_default(),
            model: model.unwrap_or_default(),
        })
    } else {
        None
    };

    Ok(ClipboardItem {
        id: row.get(0)?,
        item_type: ItemType::from_str(&item_type_str),
        source: Source::from_str(&source_str),
        content: row.get(3)?,
        is_favorite: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        image_meta,
        file_meta,
        asr_meta,
        is_rich: row.get::<_, i64>(11)? != 0,
    })
}

/// 切换收藏状态。
pub fn toggle_favorite(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET is_favorite = CASE is_favorite WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
        params![id],
    )?;
    Ok(())
}

/// 删除单条。
pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;
    track_deletes(conn, 1);
    Ok(())
}

/// 清空历史（可选保留收藏）。
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let rows = if keep_favorite {
        conn.execute("DELETE FROM clipboard_history WHERE is_favorite = 0", [])?
    } else {
        conn.execute("DELETE FROM clipboard_history", [])?
    };
    if rows > 0 {
        track_deletes(conn, rows as u32);
    }
    Ok(rows)
}

/// 删除引用了指定 transcription_id 的所有剪贴板条目。
/// 用于 Settings 删除转译记录时同步清理剪贴板引用。
pub fn delete_by_transcription_ids(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = conn.execute(
        &format!("DELETE FROM clipboard_history WHERE transcription_id IN ({})", placeholders),
        params.as_slice(),
    )?;
    Ok(rows)
}

/// 获取所有图片 blob hash（用于孤立文件清理）。
pub fn get_referenced_blob_hashes(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT blob_hash FROM clipboard_history WHERE blob_hash IS NOT NULL"
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut set = HashSet::new();
    for row in rows {
        if let Ok(h) = row { set.insert(h); }
    }
    Ok(set)
}

/// 统计总数。
pub fn count_all(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM clipboard_history", [], |r| r.get(0))?;
    Ok(count)
}

// ── 构造新条目的辅助结构 ──

pub struct NewClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub content: String,
    pub search_text: String,
    pub created_at: String,
    pub blob_hash: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub has_thumbnail: Option<i64>,
    pub file_count: Option<i64>,
    pub is_rich: bool,
}

// ── 时间辅助 ──

pub fn chrono_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
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
        if remaining_days >= year_days {
            remaining_days -= year_days;
            year += 1;
        } else {
            break;
        }
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
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_query_text() {
        let conn = open_test_db();
        let item = NewClipboardItem {
            id: 1000, item_type: ItemType::Text,
            content: "hello world".into(), search_text: "hello world".into(),
            created_at: iso_now(), blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        };
        insert_clipboard_item(&conn, &item).unwrap();
        let result = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "hello world");
        assert_eq!(result[0].source, Source::Clipboard);
    }

    #[test]
    fn test_insert_asr() {
        let conn = open_test_db();
        insert_asr_item(&conn, "识别文本", AsrMeta {
            transcription_id: 12345, polish_status: "off".into(),
            engine: "sensevoice".into(), model: "".into(),
        }).unwrap();
        let result = query_history(&conn, &QueryFilter {
            filter: "asr".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, Source::Asr);
        assert_eq!(result[0].asr_meta.as_ref().unwrap().engine, "sensevoice");
        // 验证 transcription_id 正确写入并读回
        assert_eq!(result[0].asr_meta.as_ref().unwrap().transcription_id, 12345);
    }

    #[test]
    fn test_fts_search_chinese() {
        let conn = open_test_db();
        let item = NewClipboardItem {
            id: 2000, item_type: ItemType::Text,
            content: "今天天气很好".into(), search_text: "今天天气很好".into(),
            created_at: iso_now(), blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        };
        insert_clipboard_item(&conn, &item).unwrap();
        let result = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("天气".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "今天天气很好");
    }

    #[test]
    fn test_filter_by_type() {
        let conn = open_test_db();
        for (i, text) in ["text1", "text2"].iter().enumerate() {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id: 3000 + i as i64, item_type: ItemType::Text,
                content: (*text).into(), search_text: (*text).into(),
                created_at: iso_now(), blob_hash: None, width: None, height: None,
                has_thumbnail: None, file_count: None, is_rich: false,
            }).unwrap();
        }
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 3005, item_type: ItemType::Image,
            content: "abc123hash".into(), search_text: "".into(),
            created_at: iso_now(), blob_hash: Some("abc123hash".into()),
            width: Some(800), height: Some(600), has_thumbnail: Some(1),
            file_count: None, is_rich: false,
        }).unwrap();
        let text_only = query_history(&conn, &QueryFilter {
            filter: "text".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(text_only.len(), 2);
        let image_only = query_history(&conn, &QueryFilter {
            filter: "image".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(image_only.len(), 1);
    }

    #[test]
    fn test_toggle_favorite() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 4000, item_type: ItemType::Text, content: "fav".into(),
            search_text: "fav".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();
        toggle_favorite(&conn, 4000).unwrap();
        let fav = query_history(&conn, &QueryFilter {
            filter: "favorite".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(fav.len(), 1);
        assert!(fav[0].is_favorite);
    }

    #[test]
    fn test_clear_history_keep_favorite() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 5000, item_type: ItemType::Text, content: "a".into(),
            search_text: "a".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 5001, item_type: ItemType::Text, content: "b".into(),
            search_text: "b".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();
        toggle_favorite(&conn, 5000).unwrap();
        let deleted = clear_history(&conn, true).unwrap();
        assert_eq!(deleted, 1);
        let remaining = count_all(&conn).unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn test_dedup_by_hash() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 6000, item_type: ItemType::Image, content: "hash123".into(),
            search_text: "".into(), created_at: iso_now(),
            blob_hash: Some("hash123".into()), width: Some(100), height: Some(100),
            has_thumbnail: Some(1), file_count: None, is_rich: false,
        }).unwrap();
        let found = find_by_content_hash(&conn, "hash123").unwrap();
        assert_eq!(found, Some(6000));
        let not_found = find_by_content_hash(&conn, "nonexistent").unwrap();
        assert_eq!(not_found, None);
    }
}
