use anyhow::Result;
use rusqlite::{params, Connection};

use crate::model::*;

// FTS5 索引一致性由 db.sql 的触发器 clip_fts_ai/ad/au 增量同步（INSERT/DELETE/UPDATE
// 各自维护对应 fts 行），正常运行无需周期性全表 rebuild。本函数用于：
//   1. 首次启动 external content table 初始为空时的 populate
//   2. cleanup 删除行后（cleanup.rs 调用）
//
// **不再在启动时无条件调用**（2026-07-21 perf）：触发器在事务内执行，
// 事务原子性保证 FTS 与主表一致（除非 DB 文件物理损坏，rebuild 也救不回来）。
// 原 main.rs 启动 rebuild 在 10MB DB 上耗时 50-200ms，纯属冗余。
pub fn rebuild_fts_index(conn: &Connection) -> Result<()> {
    conn.execute("INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')", [])?;
    Ok(())
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

#[allow(dead_code)]
pub(crate) fn insert_asr_item(conn: &Connection, text: &str, engine: &str, model: &str, segments: Option<&str>) -> Result<i64> {
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

const SELECT_COLS: &str = "id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments, deleted_at";

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
        "SELECT c.id, c.item_type, c.content, c.ref_data, c.meta_info, c.is_favorite, c.is_rich, c.created_at, c.has_thumbnail, c.segments, c.deleted_at
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

/// 把 QueryFilter.filter 翻译为 SQL WHERE 子句（不含 WHERE 关键字）。
///
/// **INV-C4（回收站隔离）**：除 "trash" 外的所有 filter 都追加 `AND deleted_at IS NULL`，
/// 确保软删内容只在回收站 tab 出现。"trash" 反向过滤 `deleted_at IS NOT NULL`。
fn build_where(filter: &QueryFilter) -> String {
    // 非 trash 的基础条件
    let base = match filter.filter.as_str() {
        "all" | "" => String::new(),
        "asr" | "voice" => "item_type = 'voice'".to_string(),
        "ocr" => "item_type = 'ocr'".to_string(),
        "text" => "item_type = 'text'".to_string(),
        "image" => "item_type = 'image'".to_string(),
        "file" => "item_type = 'file'".to_string(),
        "favorite" => "is_favorite = 1".to_string(),
        "unfavorite" => "is_favorite = 0".to_string(),
        "trash" => return "deleted_at IS NOT NULL".to_string(),
        _ => String::new(),
    };
    // 追加 deleted_at IS NULL（INV-C4）
    if base.is_empty() {
        "deleted_at IS NULL".to_string()
    } else {
        format!("{} AND deleted_at IS NULL", base)
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
        deleted_at: row.get(10)?,
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
#[allow(dead_code)]
pub(crate) fn update_segments(conn: &Connection, id: i64, segments: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET segments = ? WHERE id = ?", params![segments, id])?;
    Ok(())
}

// ── DELETE（软删 + 永久删）──
//
// 删除语义（2026-07-22 v47 软删/回收站）：
//   - 图片（item_type='image'）：永远物理 DELETE（image_data 引用计数，软删行还在 → blob 泄漏）
//   - 文本类（text/voice/ocr/file）：软删（UPDATE deleted_at = now），进回收站，可还原
//
// delete_item / delete_items：前端默认删除入口，按 item_type 分流（image 物理 / 其他软删）。
// permanent_delete_item / permanent_delete_items / empty_trash：回收站永久删，物理 DELETE。

/// 判断 id 对应的 item_type 是否为图片。查不到返回 false（已删除的行不纠结）。
fn is_image_item(conn: &Connection, id: i64) -> bool {
    conn.query_row(
        "SELECT item_type FROM clipboard_history WHERE id = ?", params![id],
        |r| r.get::<_, String>(0),
    ).ok().map(|t| t == "image").unwrap_or(false)
}

/// 默认删除：图片→物理删（含 blob 清理）；其他→软删（进回收站）。
pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    if is_image_item(conn, id) {
        permanent_delete_item(conn, id)?;
    } else {
        soft_delete(conn, id)?;
    }
    Ok(())
}

/// 批量默认删除：图片→物理删；其他→软删。返回受影响行数。
pub fn delete_items(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() { return Ok(0); }
    let mut affected = 0;
    for &id in ids {
        if is_image_item(conn, id) {
            affected += permanent_delete_item(conn, id)? as usize;
        } else {
            affected += soft_delete(conn, id)? as usize;
        }
    }
    Ok(affected)
}

/// 软删单条：UPDATE deleted_at = now（仅对未删的行生效）。
pub fn soft_delete(conn: &Connection, id: i64) -> Result<usize> {
    let rows = conn.execute(
        "UPDATE clipboard_history SET deleted_at = datetime('now') WHERE id = ? AND deleted_at IS NULL",
        params![id],
    )?;
    Ok(rows)
}

/// 还原单条：清除 deleted_at。
pub fn restore_item(conn: &Connection, id: i64) -> Result<usize> {
    let rows = conn.execute(
        "UPDATE clipboard_history SET deleted_at = NULL WHERE id = ? AND deleted_at IS NOT NULL",
        params![id],
    )?;
    Ok(rows)
}

/// 批量还原。
pub fn restore_items(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() { return Ok(0); }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params_vec: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = conn.execute(
        &format!("UPDATE clipboard_history SET deleted_at = NULL WHERE id IN ({}) AND deleted_at IS NOT NULL", placeholders),
        params_vec.as_slice(),
    )?;
    Ok(rows)
}

/// 永久删单条：物理 DELETE + 图片 blob 清理。
pub fn permanent_delete_item(conn: &Connection, id: i64) -> Result<usize> {
    let ref_data: Option<String> = conn.query_row(
        "SELECT ref_data FROM clipboard_history WHERE id = ?", params![id],
        |r| r.get::<_, Option<String>>(0),
    ).ok().flatten();

    let rows = conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;

    if let Some(hash) = ref_data.as_deref() {
        delete_image_if_unreferenced(conn, hash);
    }
    Ok(rows)
}

/// 批量永久删：物理 DELETE IN (...) + 图片 blob 清理。
pub fn permanent_delete_items(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() { return Ok(0); }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params_vec: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = conn.execute(&format!("DELETE FROM clipboard_history WHERE id IN ({})", placeholders), params_vec.as_slice())?;
    if rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}

/// 清空回收站：永久删除所有 deleted_at IS NOT NULL 的行。
/// 物理 DELETE 触发 FTS trigger（clip_fts_ad）自动清索引。
pub fn empty_trash(conn: &Connection) -> Result<usize> {
    let rows = conn.execute("DELETE FROM clipboard_history WHERE deleted_at IS NOT NULL", [])?;
    if rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}

/// 永久删除回收站中满足以下**任一**条件的软删条目（由 scheduler 定时调用）：
///
/// 1. **TTL 超期**：`deleted_at` 超过 `ttl_days` 天
/// 2. **容量超限**：回收站总条数超过 `max_items`，删最老的（`deleted_at ASC`）超出部分
///
/// 物理 DELETE 触发 FTS trigger（clip_fts_ad）自动清索引。返回删除条数。
pub fn purge_trash(conn: &Connection, ttl_days: u64, max_items: u64) -> Result<usize> {
    let mut total_deleted = 0;

    // 条件 1：TTL 超期
    let ttl_deleted = conn.execute(
        "DELETE FROM clipboard_history WHERE deleted_at IS NOT NULL AND deleted_at < datetime('now', ?)",
        [format!("-{} days", ttl_days)],
    )?;
    if ttl_deleted > 0 {
        log::info!("[trash-purge] TTL 清理：删除 {} 条超期（>{} 天）", ttl_deleted, ttl_days);
    }
    total_deleted += ttl_deleted;

    // 条件 2：容量超限——删最老的超出部分
    let trash_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE deleted_at IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    if trash_count > max_items as i64 {
        let excess = trash_count - max_items as i64;
        let cap_deleted = conn.execute(
            "DELETE FROM clipboard_history WHERE id IN (
                SELECT id FROM clipboard_history
                WHERE deleted_at IS NOT NULL
                ORDER BY deleted_at ASC LIMIT ?
            )",
            [excess],
        )?;
        if cap_deleted > 0 {
            log::info!("[trash-purge] 容量清理：删除 {} 条最老（回收站 {} > 上限 {}）", cap_deleted, trash_count, max_items);
        }
        total_deleted += cap_deleted;
    }

    if total_deleted > 0 {
        cleanup_unreferenced_images(conn)?;
    }
    Ok(total_deleted)
}

/// 清空历史：图片→物理 DELETE；其他→软删 UPDATE。
/// keep_favorite=true 时跳过收藏项。
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let fav_clause = if keep_favorite { " AND is_favorite = 0" } else { "" };

    // 1. 图片物理删（含 blob 清理）
    let img_rows = conn.execute(
        &format!("DELETE FROM clipboard_history WHERE item_type = 'image'{} AND deleted_at IS NULL", fav_clause),
        [],
    )?;
    if img_rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }

    // 2. 文本类软删
    let text_rows = conn.execute(
        &format!("UPDATE clipboard_history SET deleted_at = datetime('now') WHERE item_type != 'image'{} AND deleted_at IS NULL", fav_clause),
        [],
    )?;
    Ok(img_rows + text_rows)
}

/// 按 filter（类型筛选）批量清理。复用 build_where 把 filter 转 SQL where，
/// keep_favorite=true 追加 AND is_favorite = 0。
/// 图片→物理 DELETE；其他→软删 UPDATE。
///
/// filter="trash" + keep_favorite → build_where 返回 "deleted_at IS NOT NULL"，
/// 此时清理=永久删回收站内容（物理 DELETE）。
/// filter="favorite" + keep_favorite=true → "is_favorite = 1 AND deleted_at IS NULL AND is_favorite = 0" 恒假，删 0 条
/// （收藏 tab 自然结果，前端禁用按钮，后端无需特判）。
pub fn clear_history_by_filter(conn: &Connection, filter: &str, keep_favorite: bool) -> Result<usize> {
    let qf = QueryFilter { filter: filter.to_string(), search: None, page: 1, size: 1 };
    let mut where_clause = build_where(&qf);
    if keep_favorite {
        where_clause.push_str(" AND is_favorite = 0");
    }

    // 回收站 tab 的 clear = 永久删（物理 DELETE）
    if filter == "trash" {
        let sql = format!("DELETE FROM clipboard_history WHERE {}", where_clause);
        let rows = conn.execute(&sql, [])?;
        if rows > 0 {
            cleanup_unreferenced_images(conn)?;
        }
        return Ok(rows);
    }

    // 1. 图片物理删（含 blob 清理）
    let img_sql = format!("DELETE FROM clipboard_history WHERE item_type = 'image' AND {}", where_clause);
    let img_rows = conn.execute(&img_sql, [])?;
    if img_rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }

    // 2. 文本类软删
    let text_sql = format!(
        "UPDATE clipboard_history SET deleted_at = datetime('now') WHERE item_type != 'image' AND {}",
        where_clause
    );
    let text_rows = conn.execute(&text_sql, [])?;
    Ok(img_rows + text_rows)
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

#[allow(dead_code)]
pub(crate) fn count_all(conn: &Connection) -> Result<i64> {
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
        let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
        let year_days = if leap { 366 } else { 365 };
        if remaining_days >= year_days { remaining_days -= year_days; year += 1; } else { break; }
    }
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
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
    fn rebuild_fts_after_drop_trigger_fixes_search() {
        // 验证 rebuild 命令在 FTS 落后时能恢复搜索能力。
        // 场景：模拟触发器失效后主表新增内容（FTS 未同步），rebuild 后应能搜到。
        let conn = open_test_db();
        // 先正常插入 1 条（走触发器）
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1, item_type: ItemType::Text, content: "synced".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        // 触发器失效后插入（绕过触发器）
        conn.execute("DROP TRIGGER clip_fts_ai", []).unwrap();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail)
             VALUES (2, 'text', 'orphan_needs_rebuild', NULL, NULL, 0, 0, '2024-01-01', 0)",
            [],
        ).unwrap();
        // rebuild 应恢复索引
        rebuild_fts_index(&conn).unwrap();
        // 搜 'orphan' 应能命中（rebuild 后 FTS 重建）
        let result = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("orphan".into()), page: 1, size: 10,
        }).unwrap();
        assert!(result.iter().any(|i| i.content.contains("orphan_needs_rebuild")),
            "rebuild should restore searchability");
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

    #[test]
    fn clear_history_by_filter_all_keep_favorite() {
        let conn = open_test_db();
        // 插 3 条文本（NewClipboardItem 无 is_favorite 字段，默认非收藏）
        for id in [1i64, 2, 3] {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id, item_type: ItemType::Text, content: format!("c{}", id),
                ref_data: None, meta_info: None, created_at: iso_now(),
                has_thumbnail: None, is_rich: false,
            }).unwrap();
        }
        toggle_favorite(&conn, 3).unwrap(); // id=3 设为收藏
        let deleted = clear_history_by_filter(&conn, "all", true).unwrap();
        assert_eq!(deleted, 2);
        let remaining = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 3);
        assert!(remaining[0].is_favorite);
    }

    #[test]
    fn clear_history_by_filter_text_only() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1, item_type: ItemType::Text, content: "text".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 2, item_type: ItemType::Image, content: String::new(),
            ref_data: Some("hash2".into()), meta_info: None, created_at: iso_now(),
            has_thumbnail: Some(1), is_rich: false,
        }).unwrap();
        insert_image_data(&conn, "hash2", &[1, 2, 3], &[4, 5, 6], 10, 10).unwrap();
        let deleted = clear_history_by_filter(&conn, "text", true).unwrap();
        assert_eq!(deleted, 1);
        let remaining = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].item_type, ItemType::Image);
    }

    #[test]
    fn clear_history_by_filter_favorite_deletes_zero() {
        let conn = open_test_db();
        for id in [1i64, 2] {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id, item_type: ItemType::Text, content: format!("c{}", id),
                ref_data: None, meta_info: None, created_at: iso_now(),
                has_thumbnail: None, is_rich: false,
            }).unwrap();
            toggle_favorite(&conn, id).unwrap(); // 两条都设收藏
        }
        let deleted = clear_history_by_filter(&conn, "favorite", true).unwrap();
        assert_eq!(deleted, 0);
        let remaining = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn clear_history_by_filter_keep_false_all() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1, item_type: ItemType::Text, content: "c1".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        toggle_favorite(&conn, 1).unwrap(); // 收藏条目 keep=false 时也应删
        let deleted = clear_history_by_filter(&conn, "all", false).unwrap();
        assert_eq!(deleted, 1);
        let remaining = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn clear_history_by_filter_image_cleans_blob() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 1, item_type: ItemType::Image, content: String::new(),
            ref_data: Some("hash1".into()), meta_info: None, created_at: iso_now(),
            has_thumbnail: Some(1), is_rich: false,
        }).unwrap();
        insert_image_data(&conn, "hash1", &[1, 2, 3], &[4, 5, 6], 10, 10).unwrap();
        let before: i64 = conn.query_row("SELECT COUNT(*) FROM image_data", [], |r| r.get(0)).unwrap();
        assert_eq!(before, 1);
        let deleted = clear_history_by_filter(&conn, "image", true).unwrap();
        assert_eq!(deleted, 1);
        // cleanup_unreferenced_images 应清掉孤立 blob
        let after: i64 = conn.query_row("SELECT COUNT(*) FROM image_data", [], |r| r.get(0)).unwrap();
        assert_eq!(after, 0);
    }
}
