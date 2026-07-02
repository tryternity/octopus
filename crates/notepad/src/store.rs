//! notes 表 CRUD + FTS5 搜索 + 排序分页。全部经 `octopus_infra::db::with_db`。
//! 时间戳助手复用 infra 风格（手写，无 chrono 依赖）。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::model::{Note, NoteFilter, NoteSource, NoteType};
use crate::serialize::extract_text;

// ── 时间辅助（与 infra/clipboard 一致的手写实现，避免 chrono 依赖）──

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

// ── CRUD ──

/// 列表查询（filter + FTS/LIKE 搜索 + 排序 + 分页）。
pub fn list_notes(filter: &NoteFilter) -> Result<Vec<Note>> {
    octopus_infra::db::with_db(|conn| list_notes_at(conn, filter))
}

pub fn list_notes_at(conn: &Connection, filter: &NoteFilter) -> Result<Vec<Note>> {
    let limit = if filter.limit > 0 { filter.limit } else { 50 };
    let offset = filter.offset.max(0);
    let where_clause = build_where(filter);

    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return query_with_search(conn, search, &where_clause, limit, offset);
        }
    }

    let sql = format!(
        "SELECT id, title, content_html, content_text, source, source_ref_id,
                is_pinned, is_favorite, created_at, updated_at, type
         FROM notes
         {}
         ORDER BY is_pinned DESC, updated_at DESC, id DESC
         LIMIT ? OFFSET ?",
        if where_clause.is_empty() { String::new() } else { format!("WHERE {}", where_clause) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit, offset], row_to_note)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_with_search(
    conn: &Connection,
    search: &str,
    extra_where: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Note>> {
    // <3 字符（trigram 无法成 token）→ LIKE fallback（title 或 content_text 子串）
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = format!(
            "SELECT id, title, content_html, content_text, source, source_ref_id,
                    is_pinned, is_favorite, created_at, updated_at, type
             FROM notes
             WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?)
             {}
             ORDER BY is_pinned DESC, updated_at DESC, id DESC
             LIMIT ? OFFSET ?",
            if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, pattern, limit, offset], row_to_note)?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }

    // ≥3 字符 → FTS5 phrase MATCH（title + content_text 联合索引）
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = format!(
        "SELECT n.id, n.title, n.content_html, n.content_text, n.source, n.source_ref_id,
                n.is_pinned, n.is_favorite, n.created_at, n.updated_at, n.type
         FROM notes_fts f JOIN notes n ON n.id = f.rowid
         WHERE notes_fts MATCH ?
         {}
         ORDER BY n.is_pinned DESC, n.updated_at DESC, n.id DESC
         LIMIT ? OFFSET ?",
        if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![phrase, limit, offset], row_to_note)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 计数（与 list_notes 同 filter/搜索逻辑，保证「共 N 条」一致）。
pub fn count_notes(filter: &NoteFilter) -> Result<i64> {
    octopus_infra::db::with_db(|conn| count_notes_at(conn, filter))
}

pub fn count_notes_at(conn: &Connection, filter: &NoteFilter) -> Result<i64> {
    let where_clause = build_where(filter);
    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return count_with_search(conn, search, &where_clause);
        }
    }
    let sql = if where_clause.is_empty() {
        "SELECT COUNT(*) FROM notes".to_string()
    } else {
        format!("SELECT COUNT(*) FROM notes WHERE {}", where_clause)
    };
    let count: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(count)
}

fn count_with_search(conn: &Connection, search: &str, extra_where: &str) -> Result<i64> {
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = if extra_where.is_empty() {
            "SELECT COUNT(*) FROM notes WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?)".to_string()
        } else {
            format!("SELECT COUNT(*) FROM notes WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?) AND {}", extra_where)
        };
        let count: i64 = conn.query_row(&sql, params![pattern, pattern], |r| r.get(0))?;
        return Ok(count);
    }
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = if extra_where.is_empty() {
        "SELECT COUNT(*) FROM notes_fts f JOIN notes n ON n.id = f.rowid WHERE notes_fts MATCH ?".to_string()
    } else {
        format!("SELECT COUNT(*) FROM notes_fts f JOIN notes n ON n.id = f.rowid WHERE notes_fts MATCH ? AND {}", extra_where)
    };
    let count: i64 = conn.query_row(&sql, params![phrase], |r| r.get(0))?;
    Ok(count)
}

fn build_where(filter: &NoteFilter) -> String {
    let mut conds: Vec<String> = Vec::new();
    if let Some(src) = filter.source {
        conds.push(format!("source = '{}'", src.as_str()));
    }
    if let Some(t) = filter.note_type {
        conds.push(format!("type = '{}'", t.as_str()));
    }
    if filter.favorite {
        conds.push("is_favorite = 1".to_string());
    }
    if filter.pinned {
        conds.push("is_pinned = 1".to_string());
    }
    conds.join(" AND ")
}

fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let source_str: String = row.get(4)?;
    let type_str: String = row.get(10)?;
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        content_html: row.get(2)?,
        content_text: row.get(3)?,
        note_type: NoteType::from_str(&type_str),
        source: NoteSource::from_str(&source_str),
        source_ref_id: row.get(5)?,
        is_pinned: row.get::<_, i64>(6)? != 0,
        is_favorite: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// 按 id 读取单条。
pub fn get_note(id: i64) -> Result<Option<Note>> {
    octopus_infra::db::with_db(|conn| get_note_at(conn, id))
}

pub fn get_note_at(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content_html, content_text, source, source_ref_id,
                is_pinned, is_favorite, created_at, updated_at, type
         FROM notes WHERE id = ?",
    )?;
    match stmt.query_row(params![id], row_to_note) {
        Ok(note) => Ok(Some(note)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 新建笔记。type=Html 时 content_text 由 body(html) 抽取；text/markdown 时 content_text=body 原文。
/// 返回新 id（AUTOINCREMENT last_insert_rowid）。
pub fn create_note(
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    octopus_infra::db::with_db(|conn| create_note_at(conn, source, source_ref_id, body, note_type))
}

pub fn create_note_at(
    conn: &Connection,
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    let (content_html, content_text) = split_body(body, note_type);
    let now = iso_now();
    conn.execute(
        "INSERT INTO notes (title, content_text, content_html, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at)
         VALUES (NULL, ?, ?, ?, ?, ?, 0, 0, ?, ?)",
        params![content_text, content_html, note_type.as_str(), source.as_str(), source_ref_id, now, now],
    )
    .context("insert note")?;
    Ok(conn.last_insert_rowid())
}

/// 按 type 拆 body → (content_html, content_text)。
/// Html：html 存原始，text 存抽取纯文本。Text/Markdown：text 存原文/源码，html 空。
fn split_body(body: &str, note_type: NoteType) -> (String, String) {
    match note_type {
        NoteType::Html => (body.to_string(), extract_text(body)),
        NoteType::Text | NoteType::Markdown => (String::new(), body.to_string()),
    }
}

/// 更新正文/标题。type=Html 时 content_text 由 body(html) 重抽；text/markdown 直存原文。updated_at = now。
/// title 空串 → 存 NULL（列表显示用 content_text 截取）。
pub fn update_note(id: i64, title: &str, body: &str, note_type: NoteType) -> Result<()> {
    octopus_infra::db::with_db(|conn| update_note_at(conn, id, title, body, note_type))
}

pub fn update_note_at(
    conn: &Connection,
    id: i64,
    title: &str,
    body: &str,
    note_type: NoteType,
) -> Result<()> {
    let (content_html, content_text) = split_body(body, note_type);
    let title_db: Option<&str> = if title.trim().is_empty() { None } else { Some(title) };
    conn.execute(
        "UPDATE notes SET title = ?, content_text = ?, content_html = ?, type = ?, updated_at = ? WHERE id = ?",
        params![title_db, content_text, content_html, note_type.as_str(), iso_now(), id],
    )?;
    Ok(())
}

/// 批量删除。返回实际删除行数。
pub fn delete_notes(ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    octopus_infra::db::with_db(|conn| delete_notes_at(conn, ids))
}

pub fn delete_notes_at(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let n = conn.execute(
        &format!("DELETE FROM notes WHERE id IN ({})", placeholders),
        params.as_slice(),
    )?;
    Ok(n)
}

pub fn toggle_pinned(id: i64) -> Result<()> {
    octopus_infra::db::with_db(|conn| toggle_pinned_at(conn, id))
}

pub fn toggle_pinned_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE notes SET is_pinned = CASE is_pinned WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
        params![id],
    )?;
    Ok(())
}

pub fn toggle_favorite(id: i64) -> Result<()> {
    octopus_infra::db::with_db(|conn| toggle_favorite_at(conn, id))
}

pub fn toggle_favorite_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE notes SET is_favorite = CASE is_favorite WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
        params![id],
    )?;
    Ok(())
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

    fn f() -> NoteFilter {
        NoteFilter {
            source: None,
            note_type: None,
            favorite: false,
            pinned: false,
            search: None,
            limit: 50,
            offset: 0,
        }
    }

    #[test]
    fn create_and_get_roundtrip() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Asr, Some(123), "<p>识别文本</p>", NoteType::Html).unwrap();
        assert!(id > 0);
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.content_html, "<p>识别文本</p>");
        assert_eq!(note.content_text, "识别文本");
        assert_eq!(note.source, NoteSource::Asr);
        assert_eq!(note.source_ref_id, Some(123));
        assert!(note.title.is_none());
    }

    #[test]
    fn update_rextracts_text_and_handles_title() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "", NoteType::Html).unwrap();
        update_note_at(&conn, id, "我的标题", "<p>第一段</p><p>第二段</p>", NoteType::Html).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.title.as_deref(), Some("我的标题"));
        assert_eq!(note.content_text, "第一段\n第二段");
        // 空标题 → NULL
        update_note_at(&conn, id, "   ", "<p>x</p>", NoteType::Html).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert!(note.title.is_none());
    }

    #[test]
    fn fts_search_three_chars() {
        let conn = open_test_db();
        create_note_at(&conn, NoteSource::Manual, None, "<p>今天天气很好</p>", NoteType::Html).unwrap();
        create_note_at(&conn, NoteSource::Manual, None, "<p>不相关内容</p>", NoteType::Html).unwrap();
        let mut filter = f();
        filter.search = Some("今天天气".into()); // ≥3 字符 → FTS
        let rows = list_notes_at(&conn, &filter).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content_text, "今天天气很好");
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 1);
    }

    #[test]
    fn like_fallback_short_query() {
        let conn = open_test_db();
        create_note_at(&conn, NoteSource::Manual, None, "<p>hello world</p>", NoteType::Html).unwrap();
        let mut filter = f();
        filter.search = Some("el".into()); // <3 字符 → LIKE
        let rows = list_notes_at(&conn, &filter).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn filter_by_source_and_favorite() {
        let conn = open_test_db();
        let a = create_note_at(&conn, NoteSource::Asr, None, "<p>a</p>", NoteType::Html).unwrap();
        let _b = create_note_at(&conn, NoteSource::Ocr, None, "<p>b</p>", NoteType::Html).unwrap();
        toggle_favorite_at(&conn, a).unwrap();

        let mut sf = f();
        sf.source = Some(NoteSource::Asr);
        assert_eq!(list_notes_at(&conn, &sf).unwrap().len(), 1);

        let mut ff = f();
        ff.favorite = true;
        assert_eq!(list_notes_at(&conn, &ff).unwrap().len(), 1);
    }

    #[test]
    fn pinned_sorts_first() {
        let conn = open_test_db();
        // 同一秒写入（iso_now 秒级精度），靠 is_pinned DESC 优先
        let first = create_note_at(&conn, NoteSource::Manual, None, "<p>first</p>", NoteType::Html).unwrap();
        let second = create_note_at(&conn, NoteSource::Manual, None, "<p>second</p>", NoteType::Html).unwrap();
        toggle_pinned_at(&conn, first).unwrap();
        let rows = list_notes_at(&conn, &f()).unwrap();
        // pinned 的 first 应在 second 之前（即便 second 更新更晚）
        assert_eq!(rows[0].id, first);
        assert_eq!(rows[1].id, second);
    }

    #[test]
    fn delete_batch_and_empty() {
        let conn = open_test_db();
        let ids: Vec<i64> = (0..3).map(|_| create_note_at(&conn, NoteSource::Manual, None, "<p>x</p>", NoteType::Html).unwrap()).collect();
        let n = delete_notes_at(&conn, &ids[0..2]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(count_notes_at(&conn, &f()).unwrap(), 1);
        assert_eq!(delete_notes_at(&conn, &[]).unwrap(), 0);
    }

    #[test]
    fn list_filter_by_note_type() {
        // 侧边栏 type tab 过滤：html/text/markdown 各一条，按 type 过滤应只返回对应类型
        let conn = open_test_db();
        create_note_at(&conn, NoteSource::Manual, None, "<p>富文本</p>", NoteType::Html).unwrap();
        create_note_at(&conn, NoteSource::Manual, None, "纯文本", NoteType::Text).unwrap();
        create_note_at(&conn, NoteSource::Manual, None, "# 标题", NoteType::Markdown).unwrap();
        create_note_at(&conn, NoteSource::Manual, None, "<p>又一富文本</p>", NoteType::Html).unwrap();

        // 全部（note_type=None）= 4
        assert_eq!(list_notes_at(&conn, &f()).unwrap().len(), 4);

        let mut only = |t: NoteType| {
            let mut filter = f();
            filter.note_type = Some(t);
            list_notes_at(&conn, &filter).unwrap()
        };
        // 各类型计数 + 类型一致
        let html = only(NoteType::Html);
        assert_eq!(html.len(), 2);
        assert!(html.iter().all(|n| n.note_type == NoteType::Html));
        assert_eq!(only(NoteType::Text).len(), 1);
        assert_eq!(only(NoteType::Markdown).len(), 1);

        // type 过滤 + 计数一致
        let mut cnt = f();
        cnt.note_type = Some(NoteType::Html);
        assert_eq!(count_notes_at(&conn, &cnt).unwrap(), 2);
    }

    #[test]
    fn fts_triggers_sync_on_update_and_delete() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>旧内容关键字</p>", NoteType::Html).unwrap();
        let mut filter = f();
        filter.search = Some("关键字".into());
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 1);
        // update 改掉关键字 → FTS 不再命中
        update_note_at(&conn, id, "", "<p>全新内容</p>", NoteType::Html).unwrap();
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 0);
        // delete → 计数归零
        delete_notes_at(&conn, &[id]).unwrap();
        let mut filter2 = f();
        filter2.search = Some("全新内容".into());
        assert_eq!(count_notes_at(&conn, &filter2).unwrap(), 0);
    }

    #[test]
    fn create_note_html_extracts_text() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>富文本<b>内容</b></p>", NoteType::Html).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.note_type, NoteType::Html);
        // html 存原始 body
        assert_eq!(note.content_html, "<p>富文本<b>内容</b></p>");
        // content_text 为抽取的纯文本（无标签）
        assert_eq!(note.content_text, "富文本内容");
    }

    #[test]
    fn create_note_text_stores_raw_no_extract() {
        let conn = open_test_db();
        let raw = "纯文本\n第二行 <不应被解析>";
        let id = create_note_at(&conn, NoteSource::Clipboard, None, raw, NoteType::Text).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.note_type, NoteType::Text);
        // text：content_html 空，content_text 存原文（尖括号不解析）
        assert_eq!(note.content_html, "");
        assert_eq!(note.content_text, raw);
    }

    #[test]
    fn create_note_markdown_stores_source() {
        let conn = open_test_db();
        let md = "# 标题\n\n正文 **加粗**";
        let id = create_note_at(&conn, NoteSource::Manual, None, md, NoteType::Markdown).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.note_type, NoteType::Markdown);
        // markdown：content_html 空，content_text 存源码（预览端渲染）
        assert_eq!(note.content_html, "");
        assert_eq!(note.content_text, md);
    }

    #[test]
    fn update_note_dispatches_by_type() {
        let conn = open_test_db();
        // 先建 html（content_html 有值，content_text 抽取）
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>html</p>", NoteType::Html).unwrap();
        // 更新为 text：content_html 清空，content_text 存原文
        update_note_at(&conn, id, "标题", "纯文本内容", NoteType::Text).unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.note_type, NoteType::Text);
        assert_eq!(note.content_html, "");
        assert_eq!(note.content_text, "纯文本内容");
        assert_eq!(note.title.as_deref(), Some("标题"));
    }
}
