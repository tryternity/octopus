use anyhow::Result;
use rusqlite::{params, Connection};

use crate::model::*;

// ── INSERT ──

pub fn insert_clipboard_item(conn: &Connection, item: &NewClipboardItem) -> Result<String> {
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
    Ok(item.id.clone())
}

#[allow(dead_code)]
pub(crate) fn insert_asr_item(conn: &Connection, text: &str, engine: &str, model: &str, segments: Option<&str>) -> Result<String> {
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

pub fn insert_ocr_item(conn: &Connection, text: &str, engine: &str, model: &str) -> Result<String> {
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

pub fn find_by_content_hash(conn: &Connection, hash: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT id FROM clipboard_history WHERE ref_data = ? AND item_type = 'image' LIMIT 1")?;
    match stmt.query_row(params![hash], |r| r.get::<_, String>(0)) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn find_by_text(conn: &Connection, text: &str, item_type: ItemType) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM clipboard_history WHERE content = ? AND item_type = ? LIMIT 1"
    )?;
    match stmt.query_row(params![text, item_type.as_str()], |r| r.get::<_, String>(0)) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn touch_created_at(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET created_at = ? WHERE id = ?", params![iso_now(), id])?;
    Ok(())
}

// ── QUERY ──

const SELECT_COLS: &str = "id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments, is_deleted";

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

pub fn get_item_by_id(conn: &Connection, id: &str) -> Result<Option<ClipboardItem>> {
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
        "SELECT c.id, c.item_type, c.content, c.ref_data, c.meta_info, c.is_favorite, c.is_rich, c.created_at, c.has_thumbnail, c.segments, c.is_deleted
         FROM clipboard_history_fts f JOIN clipboard_history c ON c.rowid = f.rowid
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
        "SELECT COUNT(*) FROM clipboard_history_fts f JOIN clipboard_history c ON c.rowid = f.rowid WHERE f.content MATCH ?".to_string()
    } else {
        format!("SELECT COUNT(*) FROM clipboard_history_fts f JOIN clipboard_history c ON c.rowid = f.rowid WHERE f.content MATCH ? AND {}", extra_where)
    };
    Ok(conn.query_row(&sql, params![phrase], |r| r.get(0))?)
}

/// 把 QueryFilter.filter 翻译为 SQL WHERE 子句（不含 WHERE 关键字）。
///
/// **INV-C4（回收站隔离）**：除 "trash" 外的所有 filter 都追加 `AND is_deleted = 0`，
/// 确保软删内容只在回收站 tab 出现。"trash" 反向过滤 `is_deleted = 1`。
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
        "trash" => return "is_deleted = 1".to_string(),
        _ => String::new(),
    };
    // 追加 is_deleted = 0（INV-C4）
    if base.is_empty() {
        "is_deleted = 0".to_string()
    } else {
        format!("{} AND is_deleted = 0", base)
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
        is_deleted: row.get::<_, i32>(10)? != 0,
    })
}

// ── UPDATE ──

pub fn toggle_favorite(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET is_favorite = CASE is_favorite WHEN 0 THEN 1 ELSE 0 END WHERE id = ?", params![id])?;
    Ok(())
}

pub fn update_content(conn: &Connection, id: &str, text: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET content = ? WHERE id = ?", params![text, id])?;
    Ok(())
}

/// 更新 voice 条目的 segments（润色/编辑后段模型更新）
#[allow(dead_code)]
pub(crate) fn update_segments(conn: &Connection, id: &str, segments: &str) -> Result<()> {
    conn.execute("UPDATE clipboard_history SET segments = ? WHERE id = ?", params![segments, id])?;
    Ok(())
}

// ── DELETE（软删 + 永久删）──
//
// 删除语义（2026-07-29 重构，策略反转）：
//   - 语音（item_type='voice'）：软删（UPDATE is_deleted = 1），is_deleted=1 不可见
//     —— voice 软删内容主要用于热词挖掘（INV-C1：list_recent_text 不过滤 is_deleted）
//        及后续优化语音识别准确性，非用户可还原的回收站
//     —— voice 软删数据 ≤ VOICE_TRASH_MAX（500）条，超出按 created_at 物理删最老的（INV-1）
//       2026-08-02 从 100 提升到 500：bigram 上下文打分需要更丰富的 ASR 语料，
//       软删 voice 是高质量语料来源（INV-C1），更多保留 → bigram 统计更稳。
//     —— 回收站概念不暴露给用户：无 trash tab / 无还原 / 无清空回收站命令
//   - 其他（text/ocr/image/file）：物理 DELETE（image 另做 blob 清理）
//
// delete_item / delete_items：前端默认删除入口，按 item_type 分流（voice 软删 / 其他物理删）。
// permanent_delete_item：delete_item/delete_items 内部复用的物理删实现（image 含 blob 清理）。

/// voice 软删回收站上限。超出部分按 created_at 物理删最老的。
/// 2026-08-02 从 100 提升到 500——bigram 上下文打分需要更丰富 ASR 语料。
pub const VOICE_TRASH_MAX: u32 = 500;

/// 软删 voice 后保证回收站 voice ≤ max_trash 条（INV-1）。
///
/// 若回收站内 voice 数量超过上限，按 `created_at ASC` 物理删最老的至恰好等于上限。
/// 返回被物理删的条数（调用方可据此决定是否重建 FTS——DB trigger 已保证一致性，
/// store 层不主动重建）。
pub fn enforce_voice_trash_limit(conn: &Connection, max_trash: u32) -> Result<usize> {
    let trash_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE item_type = 'voice' AND is_deleted = 1",
        [], |r| r.get(0),
    )?;
    if trash_count <= max_trash as i64 {
        return Ok(0);
    }
    let excess = trash_count - max_trash as i64;
    let deleted = conn.execute(
        "DELETE FROM clipboard_history WHERE id IN (
            SELECT id FROM clipboard_history
            WHERE item_type = 'voice' AND is_deleted = 1
            ORDER BY created_at ASC LIMIT ?
        )",
        [excess],
    )?;
    if deleted > 0 {
        log::info!(
            "[voice-trash] 回收站 voice 容量清理：物理删 {} 条最老（{} > 上限 {}）",
            deleted, trash_count, max_trash
        );
    }
    Ok(deleted)
}

/// voice content 字符数低于此值的直接物理删（不进软删回收站）——太短的碎片
///（如「嗯」「好的」）对 bigram 语料无价值甚至有害（噪声 bigram 对）。
/// 阈值 5 字符（SQLite length() 对 TEXT 返回字符数），覆盖实测的「燃」「休」类碎片。
const VOICE_SOFT_DELETE_MIN_LEN: usize = 5;

/// 判断 id 是否为 voice 且 content 足够长（值得软删保留作 bigram 语料）。
/// voice 且 content 长度 >= VOICE_SOFT_DELETE_MIN_LEN → true（软删）；
/// voice 但太短 / 非 voice / 查不到 → false（物理删）。
///
/// 第二十九轮补充 P2-C2：原 unwrap_or(false) 把 DB 错误（锁竞争/IO/损坏）并入 false
/// → delete_item 走 permanent_delete_item → voice 永久删除不可恢复（失去 bigram 语料）。
/// 删除应 fail-safe——DB 错误时保守返 true（软删优先，宁可多保留也不误删）。
fn is_voice_worth_keeping(conn: &Connection, id: &str) -> bool {
    match conn.query_row(
        "SELECT item_type, length(content) FROM clipboard_history WHERE id = ?",
        params![id],
        |r| {
            let item_type: String = r.get(0)?;
            let len: i64 = r.get(1).unwrap_or(0);
            Ok(item_type == "voice" && len as usize >= VOICE_SOFT_DELETE_MIN_LEN)
        },
    ) {
        Ok(worth) => worth,
        Err(rusqlite::Error::QueryReturnedNoRows) => false, // 查不到→物理删（行不存在）
        Err(e) => {
            // DB 错误——保守软删（fail-safe：宁可多保留也不误删 voice 语料）
            log::warn!("[clipboard] is_voice_worth_keeping DB 错误（保守软删）：{}", e);
            true
        }
    }
}

/// 默认删除：voice 且够长→软删（进回收站 + enforce 上限）；voice 太短/其他→物理删。
pub fn delete_item(conn: &Connection, id: &str) -> Result<()> {
    if is_voice_worth_keeping(conn, id) {
        soft_delete(conn, id)?;
        enforce_voice_trash_limit(conn, VOICE_TRASH_MAX)?;
    } else {
        permanent_delete_item(conn, id)?;
    }
    Ok(())
}

/// 批量默认删除：voice 且够长→软删；voice 太短/其他→物理删。返回受影响行数。
/// voice 批量软删后统一 enforce 一次上限（而非每条 enforce，减少 DB 往返）。
pub fn delete_items(conn: &Connection, ids: &[String]) -> Result<usize> {
    if ids.is_empty() { return Ok(0); }
    let mut affected = 0;
    let mut had_voice = false;
    for id in ids {
        if is_voice_worth_keeping(conn, id) {
            affected += soft_delete(conn, id)? as usize;
            had_voice = true;
        } else {
            affected += permanent_delete_item(conn, id)? as usize;
        }
    }
    if had_voice {
        enforce_voice_trash_limit(conn, VOICE_TRASH_MAX)?;
    }
    Ok(affected)
}

/// 软删单条：UPDATE is_deleted = 1（仅对未删的行生效）。
pub fn soft_delete(conn: &Connection, id: &str) -> Result<usize> {
    let rows = conn.execute(
        "UPDATE clipboard_history SET is_deleted = 1 WHERE id = ? AND is_deleted = 0",
        params![id],
    )?;
    Ok(rows)
}

/// 永久删单条：物理 DELETE + 图片 blob 清理。
pub fn permanent_delete_item(conn: &Connection, id: &str) -> Result<usize> {
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

/// 清空历史：voice→软删 UPDATE（进回收站 + enforce 上限）；其他→物理 DELETE。
/// keep_favorite=true 时跳过收藏项。
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let fav_clause = if keep_favorite { " AND is_favorite = 0" } else { "" };
    // 复用 clear_voice_aware（2026-08-05 抽取，消除与 clear_history_by_filter 的四步流程重复）
    let where_clause = format!("is_deleted = 0{}", fav_clause);
    clear_voice_aware(conn, &where_clause, &where_clause)
}

/// 按 filter（类型筛选）批量清理。复用 build_where 把 filter 转 SQL where，
/// keep_favorite=true 追加 AND is_favorite = 0。
/// voice→软删 UPDATE（进回收站 + enforce 上限）；其他→物理 DELETE。
///
/// filter="trash" + keep_favorite → build_where 返回 "is_deleted = 1"，
/// 此时清理=永久删回收站内容（物理 DELETE）。
/// filter="favorite" + keep_favorite=true → "is_favorite = 1 AND is_deleted = 0 AND is_favorite = 0" 恒假，删 0 条
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

    // 复用 clear_voice_aware（2026-08-05 抽取）
    clear_voice_aware(conn, &where_clause, &where_clause)
}

/// voice 软删感知的批量清理四步流程（clear_history / clear_history_by_filter 共用）。
///
/// `select_where` — 用于 voice 软删 UPDATE 的 WHERE（含 is_deleted=0 等过滤）
/// `delete_where` — 用于非 voice 物理删 DELETE 的 WHERE（通常同 select_where）
///
/// 四步：
/// 1. 非 voice 物理删（image 含 blob 清理）
/// 2a. voice 软删（进回收站）
/// 2b. 回收站太短 voice 物理删（< VOICE_SOFT_DELETE_MIN_LEN，对 bigram 无价值）
/// 3. voice 回收站容量上限（INV-1，enforce_voice_trash_limit）
fn clear_voice_aware(conn: &Connection, select_where: &str, delete_where: &str) -> Result<usize> {
    // 1. 非 voice 全部物理删（image 含 blob 清理）
    let non_voice_rows = conn.execute(
        &format!("DELETE FROM clipboard_history WHERE item_type != 'voice' AND {}", delete_where),
        [],
    )?;
    if non_voice_rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }

    // 2a. 所有匹配的 voice 软删（进回收站）
    let voice_rows = conn.execute(
        &format!("UPDATE clipboard_history SET is_deleted = 1 WHERE item_type = 'voice' AND {}", select_where),
        [],
    )?;

    // 2b. 回收站里太短的 voice（< VOICE_SOFT_DELETE_MIN_LEN）→ 物理删（对 bigram 语料无价值）
    let short_voice = conn.execute(
        &format!(
            "DELETE FROM clipboard_history WHERE item_type = 'voice' AND length(content) < {min} AND is_deleted = 1",
            min = VOICE_SOFT_DELETE_MIN_LEN,
        ),
        [],
    )?;
    if short_voice > 0 {
        log::info!("[voice-trash] 清空历史：{} 条过短 voice 物理删（< {} 字符）", short_voice, VOICE_SOFT_DELETE_MIN_LEN);
    }

    // 3. voice 回收站容量上限（INV-1）
    if voice_rows > 0 {
        enforce_voice_trash_limit(conn, VOICE_TRASH_MAX)?;
    }

    Ok(non_voice_rows + short_voice + voice_rows)
}

// ── image_data CRUD ──

/// 存储图片：原图写文件系统（`<screens_dir>/<hash>.jpg`）+ 缩略图存 DB。
/// 签名保持 image_blob 参数（调用方不变），内部改写文件。
pub fn insert_image_data(conn: &Connection, hash: &str, image_blob: &[u8], thumb_blob: &[u8], width: i64, height: i64) -> Result<()> {
    // 1. 原图写文件系统
    crate::image::save_image_to_file(hash, image_blob)?;
    // 2. 缩略图 + 尺寸存 DB（无 blob/image_type 列）
    conn.execute(
        "INSERT OR REPLACE INTO image_data (hash, thumb, width, height, created_at) VALUES (?, ?, ?, ?, ?)",
        params![hash, thumb_blob, width, height, iso_now()],
    )?;
    Ok(())
}

/// 读原图（从文件系统）。签名不变（调用方零改），内部从 fs::read 读。
pub fn get_image_blob(_conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    crate::image::read_image_file(hash)
}

pub fn get_image_thumb(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT thumb FROM image_data WHERE hash = ?")?;
    match stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0)) {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 清理无引用的图片：DB 查无引用 hash → 删文件 + 删 DB 行。
pub fn cleanup_unreferenced_images(conn: &Connection) -> Result<usize> {
    // 查无引用的 hash（clipboard_history 无对应 image 行的）
    let mut stmt = conn.prepare(
        "SELECT hash FROM image_data WHERE hash NOT IN (
            SELECT DISTINCT ref_data FROM clipboard_history
            WHERE item_type = 'image' AND ref_data IS NOT NULL
        )",
    )?;
    let orphan_hashes: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut deleted = 0;
    for hash in &orphan_hashes {
        crate::image::delete_image_file(hash);
        conn.execute("DELETE FROM image_data WHERE hash = ?", params![hash])?;
        deleted += 1;
    }
    Ok(deleted)
}

/// 单条图片删除：引用计数归零时删文件 + DB 行。
fn delete_image_if_unreferenced(conn: &Connection, hash: &str) {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE ref_data = ? AND item_type = 'image'",
        params![hash], |r| r.get(0),
    ).unwrap_or(0);
    if count == 0 {
        crate::image::delete_image_file(hash);
        let _ = conn.execute("DELETE FROM image_data WHERE hash = ?", params![hash]);
    }
}

#[allow(dead_code)]
pub(crate) fn count_all(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM clipboard_history", [], |r| r.get(0))?)
}

// ── 构造新条目的辅助结构 ──

pub struct NewClipboardItem {
    pub id: String,
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

/// 生成唯一 UUID 主键并插入：主键冲突（极小概率）时换一个 UUID 重试。
fn insert_with_unique_id<F>(mut insert_fn: F) -> Result<String>
where F: FnMut(&str) -> rusqlite::Result<usize>,
{
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        match insert_fn(&id) {
            Ok(_) => return Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => continue,
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
        let sql = octopus_infra::resources::db_schema_sql();
        conn.execute_batch(sql).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_query_text() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-1000".into(), item_type: ItemType::Text, content: "hello world".into(),
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
        let id = "test-1700";
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: id.into(), item_type: ItemType::Text, content: "原始".into(),
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
            id: "test-1".into(), item_type: ItemType::Text, content: "text1".into(),
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
        for id in ["test-1", "test-2", "test-3"] {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id: id.into(), item_type: ItemType::Text, content: format!("c{}", id),
                ref_data: None, meta_info: None, created_at: iso_now(),
                has_thumbnail: None, is_rich: false,
            }).unwrap();
        }
        toggle_favorite(&conn, "test-3").unwrap(); // id="test-3" 设为收藏
        let deleted = clear_history_by_filter(&conn, "all", true).unwrap();
        assert_eq!(deleted, 2);
        let remaining = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: None, page: 1, size: 10,
        }).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "test-3");
        assert!(remaining[0].is_favorite);
    }

    #[test]
    fn clear_history_by_filter_text_only() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-1".into(), item_type: ItemType::Text, content: "text".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-2".into(), item_type: ItemType::Image, content: String::new(),
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
        for id in ["test-1", "test-2"] {
            insert_clipboard_item(&conn, &NewClipboardItem {
                id: id.into(), item_type: ItemType::Text, content: format!("c{}", id),
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
            id: "test-1".into(), item_type: ItemType::Text, content: "c1".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        toggle_favorite(&conn, "test-1").unwrap(); // 收藏条目 keep=false 时也应删
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
            id: "test-1".into(), item_type: ItemType::Image, content: String::new(),
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

    // ── 软删策略重构（2026-07-29）：仅 voice 软删 + 回收站 100 条上限 ──

    /// 插入一条 voice，返回其 id。created_at 用 epoch 控制时间顺序（老化测试用）。
    /// content 自动填充至 >= VOICE_SOFT_DELETE_MIN_LEN（测试软删逻辑时不受短语音物理删干扰）。
    fn insert_voice_at(conn: &Connection, id: &str, text: &str, age_seconds: u64) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().saturating_sub(age_seconds))
            .unwrap_or(0);
        let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(secs);
        let created = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s);
        // 确保 content 字符数足够（>= VOICE_SOFT_DELETE_MIN_LEN），避免被短语音物理删逻辑误删
        let padded = if text.chars().count() < VOICE_SOFT_DELETE_MIN_LEN {
            format!("测试语音记录{}", text) // 6 字 + text → >= 5 chars
        } else {
            text.to_string()
        };
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at, is_rich)
             VALUES (?, 'voice', ?, ?, 0)",
            params![id, padded, created],
        ).unwrap();
    }

    fn voice_trash_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE item_type = 'voice' AND is_deleted = 1",
            [], |r| r.get(0),
        ).unwrap()
    }

    #[test]
    fn delete_voice_soft_deletes() {
        let conn = open_test_db();
        insert_voice_at(&conn, "test-100", "语音A", 0);
        delete_item(&conn, "test-100").unwrap();
        // 行还在，is_deleted=1（软删进回收站）
        let deleted_flag: i64 = conn.query_row(
            "SELECT is_deleted FROM clipboard_history WHERE id = 'test-100'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(deleted_flag, 1);
    }

    #[test]
    fn delete_short_voice_physical_not_soft_delete() {
        // 短 voice（< 5 字符）直接物理删，不进回收站（bigram 语料无价值）
        let conn = open_test_db();
        // 直接插入短 content（不经 insert_voice_at padding）
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at, is_rich)
             VALUES ('test-300', 'voice', '嗯', '2026-01-01 00:00:00', 0)",
            [],
        ).unwrap();
        delete_item(&conn, "test-300").unwrap();
        // 物理删——行不存在（不是 is_deleted=1 软删）
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-300'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "短 voice 应物理删，不应软删保留");
    }

    #[test]
    fn delete_text_physical() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-200".into(), item_type: ItemType::Text, content: "文本".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        delete_item(&conn, "test-200").unwrap();
        // 物理删——行不存在
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-200'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_image_physical_with_blob_cleanup() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-300".into(), item_type: ItemType::Image, content: String::new(),
            ref_data: Some("hash300".into()), meta_info: None, created_at: iso_now(),
            has_thumbnail: Some(1), is_rich: false,
        }).unwrap();
        insert_image_data(&conn, "hash300", &[1], &[2], 10, 10).unwrap();
        delete_item(&conn, "test-300").unwrap();
        // 行删 + blob 清理（回归：image 仍物理删）
        let row_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-300'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(row_count, 0);
        let blob_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM image_data WHERE hash = 'hash300'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(blob_count, 0);
    }

    #[test]
    fn voice_trash_limit_enforced_on_delete() {
        let max = VOICE_TRASH_MAX as i64;
        // 回收站已有 max 条 voice（age max..1 秒，id 越大越新），
        // 再软删 1 条 → 最老 1 条被物理删，回收站恰好 max 条。
        let conn = open_test_db();
        for i in 0..max {
            insert_voice_at(&conn, &format!("test-1000-{}", i), &format!("旧{}", i), (max - i) as u64);
        }
        // 先把它们标为已软删（模拟回收站现状）
        conn.execute("UPDATE clipboard_history SET is_deleted = 1", []).unwrap();
        assert_eq!(voice_trash_count(&conn), max);

        // 插一条新 voice 并软删（触发 enforce）
        insert_voice_at(&conn, "test-2000", "新删", 0);
        delete_item(&conn, "test-2000").unwrap();

        // INV-1：回收站恰好 max 条
        assert_eq!(voice_trash_count(&conn), max);
        // 最老的（id="test-1000-0", age=100s）被物理删
        let oldest: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-1000-0'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(oldest, 0, "最老的 voice 应被物理删");
        // 新删的还在回收站
        let newest: i64 = conn.query_row(
            "SELECT is_deleted FROM clipboard_history WHERE id = 'test-2000'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(newest, 1);
    }

    #[test]
    fn voice_trash_limit_below_threshold_noop() {
        // 回收站 < 100 条 → enforce 不删任何行
        let conn = open_test_db();
        for i in 0..50 {
            insert_voice_at(&conn, &format!("test-1000-{}", i), &format!("v{}", i), 50 - i as u64);
        }
        conn.execute("UPDATE clipboard_history SET is_deleted = 1", []).unwrap();
        insert_voice_at(&conn, "test-2000", "触发", 0);
        delete_item(&conn, "test-2000").unwrap();
        // 51 条全在（50 旧 + 1 新），未被 enforce 删除
        assert_eq!(voice_trash_count(&conn), 51);
    }

    #[test]
    fn clear_history_soft_deletes_only_voice() {
        // 清空历史：voice 软删，text/image 物理删
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-1".into(), item_type: ItemType::Text, content: "t".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "test-2".into(), item_type: ItemType::Image, content: String::new(),
            ref_data: Some("h2".into()), meta_info: None, created_at: iso_now(),
            has_thumbnail: Some(1), is_rich: false,
        }).unwrap();
        insert_image_data(&conn, "h2", &[1], &[2], 10, 10).unwrap();
        insert_voice_at(&conn, "test-3", "语音", 0);

        clear_history(&conn, false).unwrap();

        // text/image 物理删（行不在）
        let text_row: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(text_row, 0);
        let image_row: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE id = 'test-2'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(image_row, 0);
        // voice 软删（行在，is_deleted=1）
        let voice_flag: i64 = conn.query_row(
            "SELECT is_deleted FROM clipboard_history WHERE id = 'test-3'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(voice_flag, 1);
    }

    #[test]
    fn clear_history_voice_trash_limit_enforced() {
        let max = VOICE_TRASH_MAX as i64;
        let extra = 5; // 超 max 多少条
        // 清空历史时若 voice 进回收站后超 max → enforce 物理删最老至恰好 max
        let conn = open_test_db();
        // max+extra 条 voice（id=test-1000-0 最新 age=0 ... 最老 age 最大）
        for i in 0..max + extra {
            insert_voice_at(&conn, &format!("test-1000-{}", i), &format!("v{}", i), i as u64 * 2);
        }
        clear_history(&conn, false).unwrap();
        // 全部进回收站后 max+extra > max，enforce 删 extra 条最老的 → 恰好 max
        assert_eq!(voice_trash_count(&conn), max);
        // 最老 extra 条被物理删
        for i in max..(max + extra) {
            let id = format!("test-1000-{}", i);
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM clipboard_history WHERE id = ?", params![id], |r| r.get(0),
            ).unwrap();
            assert_eq!(count, 0, "i={} 应被 enforce 物理删", i);
        }
        // 第 extra+1 老（test-1000-{max-1}）保留在回收站
        let kept: i64 = conn.query_row(
            "SELECT is_deleted FROM clipboard_history WHERE id = ?", params![format!("test-1000-{}", max - 1)], |r| r.get(0),
        ).unwrap();
        assert_eq!(kept, 1);
    }

    /// 第十七轮 P1-1 回归：FTS5 JOIN 必须用 c.rowid = f.rowid（不是 c.id = f.rowid）。
    /// schema v59 把 clipboard_history.id 从 INTEGER 改 TEXT(UUID)，FTS5 trigger 用隐式 rowid，
    /// 旧 JOIN c.id = f.rowid 因 TEXT≠INTEGER 类型不匹配 → 搜索恒空。
    #[test]
    fn test_fts5_search_finds_inserted_content() {
        let conn = open_test_db();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "uuid-ftstest-001".into(), item_type: ItemType::Text,
            content: "测试中文搜索功能".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        insert_clipboard_item(&conn, &NewClipboardItem {
            id: "uuid-ftstest-002".into(), item_type: ItemType::Text,
            content: "另一个不相干的条目".into(),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();

        // ≥3 字符走 FTS5 MATCH 路径——修复前恒返回空，修复后应找到 1 条
        let result = query_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("中文搜索".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(result.len(), 1, "P1-1: FTS5 搜索应找到「测试中文搜索功能」");
        assert_eq!(result[0].content, "测试中文搜索功能");

        // count 也走 FTS5 MATCH
        let count = count_history(&conn, &QueryFilter {
            filter: "all".into(), search: Some("中文搜索".into()), page: 1, size: 10,
        }).unwrap();
        assert_eq!(count, 1, "P1-1: FTS5 count 应为 1");
    }
}
