use anyhow::{Context, Result};
use rusqlite::{params, Connection};
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
    insert_with_unique_id(conn, |id| {
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
    })
}

/// 插入 OCR 识别文本条目（source='ocr'，复用 engine/model 列）。返回插入的 id。
pub fn insert_ocr_item(conn: &Connection, text: &str, ocr_meta: OcrMeta) -> Result<i64> {
    insert_with_unique_id(conn, |id| {
        conn.execute(
            "INSERT INTO clipboard_history
             (id, item_type, source, content, search_text, is_favorite, created_at,
              engine, model)
             VALUES (?, 'text', 'ocr', ?, ?, 0, ?, ?, ?)",
            params![id, text, text, iso_now(), ocr_meta.engine, ocr_meta.model],
        )
    })
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

/// 按文本内容 + 类型去重查找。存在返回 id。
///
/// 文件（paths_json）与文本（text）都走此函数，按各自 `item_type` 匹配——
/// 旧实现硬编码 `item_type = 'text'`，导致文件去重（item_type='file'）永远
/// 返回 None，连续复制同一文件会源源不断写入重复记录。
pub fn find_by_text(conn: &Connection, text: &str, item_type: ItemType) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM clipboard_history WHERE content = ? AND source = 'clipboard' AND item_type = ? LIMIT 1"
    )?;
    let row = stmt.query_row(params![text, item_type.as_str()], |r| r.get::<_, i64>(0));
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
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?",
        if where_clause.is_empty() { String::new() } else { format!("WHERE {}", where_clause) }
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit, offset], row_to_item)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 按 id 精确读取单条记录（id 是 INTEGER PRIMARY KEY，rowid O(1) 查找）。
/// 用于「按 id 操作单个条目」的命令（复制/粘贴/保存/打开/OCR/缩略图），
/// 避免调 query_history 反序列化整页（曾 size:1000）再 .find 的线性扫描。
pub fn get_item_by_id(conn: &Connection, id: i64) -> Result<Option<ClipboardItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, source, content, is_favorite, created_at,
                blob_hash, width, height, has_thumbnail, file_count, is_rich,
                transcription_id, polish_status, engine, model
         FROM clipboard_history
         WHERE id = ?",
    )?;
    match stmt.query_row(params![id], row_to_item) {
        Ok(item) => Ok(Some(item)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn query_with_search(
    conn: &Connection,
    search: &str,
    extra_where: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<ClipboardItem>> {
    // FTS5 trigram tokenizer 以「3 个 Unicode 字符」为一个 token；
    // 查询短于 3 字符（单个字母 / 两位字母 / 两个汉字如「天气」）无法形成 trigram，
    // MATCH 匹配空，故 fallback 回 LIKE 子串扫表。按字符数而非字节数判断。
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = format!(
            "SELECT id, item_type, source, content, is_favorite, created_at,
                    blob_hash, width, height, has_thumbnail, file_count, is_rich,
                    transcription_id, polish_status, engine, model
             FROM clipboard_history
             WHERE search_text LIKE ?
             {}
             ORDER BY created_at DESC, id DESC
             LIMIT ? OFFSET ?",
            if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, limit, offset], row_to_item)?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }

    // 走 FTS5（clipboard_history_fts 虚表，trigram tokenizer）：
    // 整个查询串包成 phrase（双引号包裹 + 内部 " 翻倍转义），等价子串匹配但命中索引，
    // 且屏蔽 OR/AND/*/:/空格 等 FTS5 查询语法干扰。extra_where 的 source/item_type/
    // is_favorite 仅存在于主表（虚表只暴露 search_text），裸用无歧义。
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = format!(
        "SELECT c.id, c.item_type, c.source, c.content, c.is_favorite, c.created_at,
                c.blob_hash, c.width, c.height, c.has_thumbnail, c.file_count, c.is_rich,
                c.transcription_id, c.polish_status, c.engine, c.model
         FROM clipboard_history_fts f
         JOIN clipboard_history c ON c.id = f.rowid
         WHERE f.search_text MATCH ?
         {}
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT ? OFFSET ?",
        if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![phrase, limit, offset], row_to_item)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 统计符合 filter（类型筛选 + 搜索）的条目数。与 [query_history] 走同一套
/// `build_where` / `LIKE-fallback` / `FTS5 MATCH` 逻辑，保证计数与展示列表一致
/// ——否则底部「共 N 条」会无视标签筛选/搜索框，恒显示全表总数。
/// page/size 对计数无意义，调用方传 1/1 占位即可。
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

/// 带搜索的计数：与 [query_with_search] 同一套 LIKE fallback / FTS5 MATCH，
/// 只是 SELECT COUNT(*) 不取行、不分页。
fn count_with_search(conn: &Connection, search: &str, extra_where: &str) -> Result<i64> {
    // < 3 字符（trigram 无法成 token）→ LIKE 子串扫表，与 query_with_search 一致
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = if extra_where.is_empty() {
            "SELECT COUNT(*) FROM clipboard_history WHERE search_text LIKE ?".to_string()
        } else {
            format!("SELECT COUNT(*) FROM clipboard_history WHERE search_text LIKE ? AND {}", extra_where)
        };
        let count: i64 = conn.query_row(&sql, params![pattern], |r| r.get(0))?;
        return Ok(count);
    }

    // >= 3 字符 → FTS5 phrase 命中索引，与 query_with_search 一致
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = if extra_where.is_empty() {
        "SELECT COUNT(*) FROM clipboard_history_fts f \
         JOIN clipboard_history c ON c.id = f.rowid \
         WHERE f.search_text MATCH ?"
            .to_string()
    } else {
        format!(
            "SELECT COUNT(*) FROM clipboard_history_fts f \
             JOIN clipboard_history c ON c.id = f.rowid \
             WHERE f.search_text MATCH ? AND {}",
            extra_where
        )
    };
    let count: i64 = conn.query_row(&sql, params![phrase], |r| r.get(0))?;
    Ok(count)
}

fn build_where(filter: &QueryFilter) -> String {
    let mut conditions: Vec<String> = Vec::new();

    match filter.filter.as_str() {
        "all" | "" => {}
        "asr" => { conditions.push("source = 'asr'".to_string()); }
        "ocr" => { conditions.push("source = 'ocr'".to_string()); }
        "text" => {
            conditions.push("item_type = 'text'".to_string());
            conditions.push("source = 'clipboard'".to_string());
        }
        "image" => { conditions.push("item_type = 'image'".to_string()); }
        "file" => { conditions.push("item_type = 'file'".to_string()); }
        "favorite" => { conditions.push("is_favorite = 1".to_string()); }
        "unfavorite" => { conditions.push("is_favorite = 0".to_string()); }
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
            engine: engine.clone().unwrap_or_default(),
            model: model.clone().unwrap_or_default(),
        })
    } else {
        None
    };

    let ocr_meta = if source_str == "ocr" {
        Some(OcrMeta {
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
        ocr_meta,
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

/// 更新条目的 search_text（OCR 场景：识别后让图片可搜索）。
pub fn update_search_text(conn: &Connection, id: i64, search_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET search_text = ? WHERE id = ?",
        params![search_text, id],
    )?;
    Ok(())
}

/// 更新条目的 content 与 search_text（精简编辑器：用户编辑文本后回写剪贴板条目）。
/// 两列同写：content 是展示/粘贴源，search_text 是 FTS5 索引源，编辑后须同步以保搜索命中。
pub fn update_content(conn: &Connection, id: i64, text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET content = ?, search_text = ? WHERE id = ?",
        params![text, text, id],
    )?;
    Ok(())
}

/// 删除单条。若被删的是图片且无其他条目引用同一 blob，顺带删除 image_data 行。
pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    let blob_hash: Option<String> = conn
        .query_row(
            "SELECT blob_hash FROM clipboard_history WHERE id = ?",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;
    track_deletes(conn, 1);

    if let Some(hash) = blob_hash {
        delete_image_if_unreferenced(conn, &hash);
    }

    Ok(())
}

/// 批量删除多条。单 SQL `DELETE ... IN (...)` + 一次 `track_deletes(总数)`（最多触发
/// 一次 FTS rebuild）+ 一次 `cleanup_unreferenced_images`。用于设置页批量删除，替代
/// 前端循环调单条 `delete_item`（每条独立事务，且 `track_deletes(1)` 累计每 10 条就
/// rebuild 一次 FTS——删 50 条会 rebuild 5 次）。
pub fn delete_items(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = conn.execute(
        &format!("DELETE FROM clipboard_history WHERE id IN ({})", placeholders),
        params.as_slice(),
    )?;
    if rows > 0 {
        track_deletes(conn, rows as u32);
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}

/// 清空历史（可选保留收藏）。删除后回收无引用的 image_data BLOB。
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
    if rows > 0 {
        // transcription_id 仅存在于 source='asr' 的 text 项（无 blob_hash），
        // 故无需 cleanup_unreferenced_images；但仍计入 FTS rebuild 阈值——与
        // delete_items / clear_history 一致，否则大批删转译记录后影子表碎片不回收。
        track_deletes(conn, rows as u32);
    }
    Ok(rows)
}

/// ── image_data 表 CRUD ──

/// 插入图片 BLOB（WebP 无损 + 缩略图）。
pub fn insert_image_data(
    conn: &Connection,
    hash: &str,
    webp_blob: &[u8],
    thumb_blob: &[u8],
    width: i64,
    height: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO image_data (hash, blob, thumb, image_type, width, height, created_at)\n         VALUES (?, ?, ?, 'webp', ?, ?, ?)",
        params![hash, webp_blob, thumb_blob, width, height, iso_now()],
    )?;
    Ok(())
}

/// 读取图片原图 BLOB。
pub fn get_image_blob(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT blob FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 读取缩略图 BLOB。
pub fn get_image_thumb(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT thumb FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 删除 image_data 中无引用的 BLOB（引用计数为 0）。返回删除行数。
///
/// 引用来源：剪贴板条目（clipboard_history.blob_hash）。无任何剪贴板条目引用才删。
/// （笔记内嵌图片功能随 notes 表于 v12→v13 迁移移除，不再有 note-img: 引用。）
pub fn cleanup_unreferenced_images(conn: &Connection) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM image_data WHERE hash NOT IN (\n            SELECT DISTINCT blob_hash FROM clipboard_history WHERE blob_hash IS NOT NULL\n        )",
        [],
    )?;
    Ok(deleted)
}

/// 删除指定 hash 的 image_data（如果无其他条目引用）。
///
/// 仅检查剪贴板条目引用（clipboard_history.blob_hash）——notes 表已移除，不再有 note-img: 引用。
fn delete_image_if_unreferenced(conn: &Connection, hash: &str) {
    let cb_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE blob_hash = ?",
            params![hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if cb_count == 0 {
        let _ = conn.execute("DELETE FROM image_data WHERE hash = ?", params![hash]);
    }
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

/// 用毫秒戳作主键插入；遇 UNIQUE 冲突（同毫秒并发 / 测试密集插入）自增重试，最多 1000 次。
/// clipboard_history.id 是毫秒戳，连续插入可能同毫秒撞主键——生产罕见（ASR/OCR 不毫秒级并发），
/// 但单元测试循环插多条必然命中，故统一在此兜底。
fn insert_with_unique_id<F>(conn: &Connection, mut insert_fn: F) -> Result<i64>
where
    F: FnMut(i64) -> rusqlite::Result<usize>,
{
    let base = chrono_millis();
    let mut id = base;
    loop {
        match insert_fn(id) {
            Ok(_) => return Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                id += 1;
                if id > base + 1000 {
                    anyhow::bail!("insert_with_unique_id: 主键冲突，1000 次自增重试仍失败");
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
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
    fn test_update_content() {
        // update_content 同时改写 content 与 search_text（OCR/剪贴板文本编辑后回写）。
        let conn = open_test_db();
        let id: i64 = 1700;
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: "原始文本".into(),
            search_text: "原始文本".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None, has_thumbnail: None,
            file_count: None, is_rich: false,
        }).unwrap();

        update_content(&conn, id, "改后文本").unwrap();

        // content 经 ClipboardItem 暴露
        let item = get_item_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(item.content, "改后文本");
        // search_text 不在 ClipboardItem 上，直接 SQL 断言
        let search: String = conn.query_row(
            "SELECT search_text FROM clipboard_history WHERE id = ?",
            params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(search, "改后文本");
    }

    #[test]
    fn test_find_by_text_file_dedup() {
        // 文件去重：find_by_text 按 ItemType 匹配（回归 v2 审计 1.3——
        // 旧实现硬编码 item_type='text'，文件去重永远 miss，连续复制同文件写重复记录）
        let conn = open_test_db();
        let paths_json = r#"["/tmp/a.txt"]"#;
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 9200, item_type: ItemType::File, content: paths_json.into(),
            search_text: "/tmp/a.txt".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None, has_thumbnail: None,
            file_count: Some(1), is_rich: false,
        }).unwrap();
        // 文件类型能查到（修复后按 ItemType::File 匹配）
        assert_eq!(find_by_text(&conn, paths_json, ItemType::File).unwrap(), Some(9200));
        // 文本类型查不到（旧 bug 下文件走 text 查询恒 None）
        assert_eq!(find_by_text(&conn, paths_json, ItemType::Text).unwrap(), None);
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
    fn test_insert_and_query_ocr() {
        let conn = open_test_db();
        let id = insert_ocr_item(&conn, "识别文本", OcrMeta {
            engine: "ocr_rs".into(), model: "m1".into(),
        }).unwrap();
        assert!(id > 0);
        let result = query_history(&conn, &QueryFilter {
            filter: "ocr".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, Source::Ocr);
        assert_eq!(result[0].content, "识别文本");
        let om = result[0].ocr_meta.as_ref().expect("ocr_meta 应填充");
        assert_eq!(om.engine, "ocr_rs");
        assert_eq!(om.model, "m1");
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
    fn test_fts_search_short_fallback() {
        // trigram 要求查询 >= 3 字节；2 字节 ASCII 走 LIKE fallback，仍能命中
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 7000, item_type: ItemType::Text,
            content: "hello world".into(), search_text: "hello world".into(),
            created_at: iso_now(), blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();
        let r = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("el".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].content, "hello world");
    }

    #[test]
    fn test_fts_search_phrase_substring() {
        // 含空格的跨词子串，包成 phrase 后按子串命中（不被 FTS AND 拆成两个 token）
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 7100, item_type: ItemType::Text,
            content: "hello world".into(), search_text: "hello world".into(),
            created_at: iso_now(), blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();
        let r = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("llo wor".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].content, "hello world");
        // 不存在的子串返回空（验证 MATCH 不会误命中）
        let r2 = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("xyzabc".into()), page: 1, size: 10,
        }).unwrap();
        assert!(r2.is_empty());
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

    #[test]
    fn test_get_item_by_id() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 8000, item_type: ItemType::Text, content: "单条读取".into(),
            search_text: "单条读取".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();

        // 命中：返回对应条目
        let item = get_item_by_id(&conn, 8000).unwrap();
        assert!(item.is_some());
        assert_eq!(item.unwrap().content, "单条读取");

        // 未命中：返回 None（而非报错）
        let missing = get_item_by_id(&conn, 9999).unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn test_delete_items() {
        let conn = open_test_db();
        for i in 0..3 {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id: 9000 + i, item_type: ItemType::Text, content: format!("d{}", i),
                search_text: format!("d{}", i), created_at: iso_now(),
                blob_hash: None, width: None, height: None,
                has_thumbnail: None, file_count: None, is_rich: false,
            }).unwrap();
        }
        // 批量删 2 条
        let deleted = delete_items(&conn, &[9000, 9001]).unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(count_all(&conn).unwrap(), 1);
        // 空切片是 noop
        assert_eq!(delete_items(&conn, &[]).unwrap(), 0);
        // 不存在的 id 删 0 条，不报错
        assert_eq!(delete_items(&conn, &[9999]).unwrap(), 0);
    }

    #[test]
    fn test_count_history_filter_and_search() {
        // count_history 必须与 query_history 走同一过滤/搜索逻辑，计数才与列表一致
        let conn = open_test_db();
        // 2 条短文本 + 1 条含「今天天气」的长文本
        for (i, t) in ["t1", "t2"].iter().enumerate() {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id: 9100 + i as i64, item_type: ItemType::Text,
                content: (*t).into(), search_text: (*t).into(),
                created_at: iso_now(), blob_hash: None, width: None, height: None,
                has_thumbnail: None, file_count: None, is_rich: false,
            }).unwrap();
        }
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: 9105, item_type: ItemType::Text,
            content: "今天天气很好".into(), search_text: "今天天气很好".into(),
            created_at: iso_now(), blob_hash: None, width: None, height: None,
            has_thumbnail: None, file_count: None, is_rich: false,
        }).unwrap();

        let f = |filter: &str, search: Option<&str>| QueryFilter {
            filter: filter.into(), search: search.map(Into::into), page: 1, size: 1,
        };

        // 全部 = 3
        assert_eq!(count_history(&conn, &f("all", None)).unwrap(), 3);
        // sanity：query_history（取够大 size，不分页）与 count_history 同条件计数一致
        let all_qf = QueryFilter { filter: "all".into(), search: None, page: 1, size: 50 };
        assert_eq!(
            query_history(&conn, &all_qf).unwrap().len(),
            count_history(&conn, &all_qf).unwrap() as usize
        );
        // FTS5 trigram 搜索「今天天气」（>=3 字符）= 1
        assert_eq!(count_history(&conn, &f("all", Some("今天天气"))).unwrap(), 1);
        // 短查询 LIKE fallback「t1」（2 字符）= 1
        assert_eq!(count_history(&conn, &f("all", Some("t1"))).unwrap(), 1);
        // 类型筛选 + 搜索组合：image 过滤下搜「今天天气」= 0
        assert_eq!(count_history(&conn, &f("image", Some("今天天气"))).unwrap(), 0);
    }

    #[test]
    fn test_like_search_stable_order_same_second() {
        // v3 审计 1.1：LIKE fallback 分支（<3 字符查询）补 id DESC 二级排序——
        // 同一秒（iso_now 秒级精度）写入多条命中记录时，须按 id DESC 稳定返回，
        // 否则分页会漏重/抖动。主分支与 MATCH 分支已加，LIKE 分支此前漏了。
        let conn = open_test_db();
        let ts = "2026-06-29 10:00:00"; // 手动指定同一秒，绕开 iso_now 实时
        for id in [100i64, 101, 102] {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id, item_type: ItemType::Text,
                content: format!("ab-{}", id), search_text: format!("ab-{}", id),
                created_at: ts.into(), blob_hash: None, width: None, height: None,
                has_thumbnail: None, file_count: None, is_rich: false,
            }).unwrap();
        }
        // "ab" = 2 字符 → 走 LIKE fallback 分支
        let r = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("ab".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(r.len(), 3);
        // 同 created_at 下按 id DESC 稳定排序：102, 101, 100
        assert_eq!(r[0].id, 102);
        assert_eq!(r[1].id, 101);
        assert_eq!(r[2].id, 100);
    }

    #[test]
    fn test_delete_by_transcription_ids() {
        // v3 审计 2.2：级联删除补 track_deletes——返回正确行数、残留 0、
        // 空/不存在 id 安全；rows>0 走 track_deletes 路径（覆盖 FTS 计数接入）。
        let conn = open_test_db();
        for tid in [1i64, 2, 3] {
            insert_asr_item(&conn, &format!("文本{}", tid), AsrMeta {
                transcription_id: tid, polish_status: "off".into(),
                engine: "sensevoice".into(), model: "".into(),
            }).unwrap();
        }
        // 删 2 个 transcription_id
        let deleted = delete_by_transcription_ids(&conn, &[1, 2]).unwrap();
        assert_eq!(deleted, 2);
        // 残留仅 tid=3
        let remaining = query_history(&conn, &QueryFilter {
            filter: "asr".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(remaining.len(), 1);
        // 空切片 noop
        assert_eq!(delete_by_transcription_ids(&conn, &[]).unwrap(), 0);
        // 不存在的 id 删 0 条（rows=0 不进 track_deletes 分支）
        assert_eq!(delete_by_transcription_ids(&conn, &[999]).unwrap(), 0);
        // 删最后 1 条（rows>0 进 track_deletes，累计未达阈值 10 不 rebuild，不报错）
        assert_eq!(delete_by_transcription_ids(&conn, &[3]).unwrap(), 1);
        assert_eq!(count_all(&conn).unwrap(), 0);
    }
}
