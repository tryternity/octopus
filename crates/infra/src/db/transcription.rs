// db/transcription.rs —— 识别历史（clipboard_history 表 item_type='voice'）CRUD + FTS5 搜索。

use super::{collect_rows, ensure_db, now_string, with_db, Connection, Result, params};

// ── 识别历史写入（desktop coordinator 用）──

/// 首次有 ASR 文本时插入（应用写入毫秒戳 id）。
/// `text` = finish_text 扁平（落 content 列）；
/// `segments` = transcript.segments_json()（段 JSON 真相源）。
/// 新 schema：写入 clipboard_history（item_type='voice'），meta_info JSON 存 engine/engine_mode/char_count。
pub fn insert_transcription_at_id(
    id: &str,
    text: &str,
    segments: &str,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let created_at = now_string();
        let char_count = text.chars().count() as i64;
        let mut meta = serde_json::Map::new();
        meta.insert("engine".into(), serde_json::Value::String(engine.to_string()));
        meta.insert("char_count".into(), serde_json::Value::Number(char_count.into()));
        meta.insert("polished".into(), serde_json::Value::Bool(false));
        if let Some(mode) = engine_mode.filter(|m| !m.is_empty()) {
            meta.insert("asr_mode".into(), serde_json::Value::String(mode.to_string()));
        }
        let meta_json = serde_json::to_string(&serde_json::Value::Object(meta))?;
        conn.execute(
            "INSERT INTO clipboard_history
                (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments)
             VALUES (?1, 'voice', ?2, NULL, ?3, 0, 0, ?4, 0, ?5)
             ON CONFLICT(id) DO UPDATE SET content=?2, segments=?5, meta_info=?3",
            params![id, text, meta_json, created_at, segments],
        )?;
        Ok(())
    })
}

/// 流式分段后更新 text/segments（完整 ASR 扁平 + 段 JSON）。
/// 新 schema：UPDATE clipboard_history SET content + segments + meta_info.char_count。
pub fn update_text_segments(id: &str, text: &str, segments: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let char_count = text.chars().count() as i64;
        conn.execute(
            "UPDATE clipboard_history SET content=?1, segments=?2,
                meta_info=json_set(COALESCE(meta_info,'{}'),'$.char_count',?3)
             WHERE id=?4",
            params![text, segments, char_count, id],
        )?;
        Ok(())
    })
}

/// 增量更新 meta_info 的单个 JSON key（诊断/辅助字段用，不覆盖 engine/char_count 等）。
///
/// 用 `json_set(COALESCE(meta_info,'{}'), '$.key', value)` 增量写入：
/// - meta_info 为 NULL 时 `'{}'` 兜底，避免 `json_set(NULL, ...)` 返 NULL 丢全部 meta
/// - 只动 `$.{key}` 一个路径，其余 meta 字段（engine/char_count/polished 等）原样保留
///
/// 用途：云端 ASR close 异常时写 `cloud_close_error`（`finalize_cloud` Err 路径落库便于诊断），
/// 未来也可写其他诊断/辅助字段。
pub fn update_meta_field(id: &str, key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        update_meta_field_at(conn, id, key, value)?;
        Ok(())
    })
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。返回实际更新的行数。
pub(crate) fn update_meta_field_at(
    conn: &Connection,
    id: &str,
    key: &str,
    value: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE clipboard_history
         SET meta_info=json_set(COALESCE(meta_info,'{}'), ?1, ?2)
         WHERE id=?3",
        params![format!("$.{}", key), value, id],
    )?)
}

/// 停顿润色后更新 polish_status/polish_model + segments/text 列。
/// `text` = 润色后扁平（与 segments 段拼接一致）；`segments` = segments_json（润色后段，Polished/Edited）。
/// 新 schema：UPDATE clipboard_history content + segments + meta_info（polished/polish_model）。
pub fn update_polished(
    id: &str,
    polish_status: &str,
    polish_model: Option<&str>,
    segments: &str,
    text: &str,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let polished = polish_status == "done";
        conn.execute(
            "UPDATE clipboard_history SET content=?1, segments=?2,
                meta_info=json_set(COALESCE(meta_info,'{}'),
                    '$.polished', ?3,
                    '$.polish_model', ?4,
                    '$.char_count', ?5)
             WHERE id=?6",
            params![text, segments, polished, polish_model, text.chars().count() as i64, id],
        )?;
        Ok(())
    })
}

/// 用户提交编辑 / 中间润色折回后更新 edited/text/segments。
/// `text` = finish_text 扁平；`segments` = segments_json（commit_edit 路径写单条 Edited 段）。
/// 新 schema：UPDATE clipboard_history content + segments。
pub fn update_edited_segments(id: &str, text: &str, segments: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        update_edited_segments_at(conn, id, text, segments)?;
        Ok(())
    })
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。返回实际更新的行数。
pub(crate) fn update_edited_segments_at(
    conn: &Connection,
    id: &str,
    text: &str,
    segments: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE clipboard_history SET content=?1, segments=?2 WHERE id=?3",
        params![text, segments, id],
    )?)
}

/// 识别结束 finalize：写最终 text/segments/status/char_count/duration_ms。
/// `text` = transcript.db_text()（finish_text 扁平，最终展示文本）；`segments` = segments_json（最终段）。
/// 新 schema：UPDATE clipboard_history content + segments + meta_info。
pub fn finalize_transcription(
    id: &str,
    text: &str,
    segments: &str,
    polish_status: &str,
    polish_model: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let char_count = text.chars().count() as i64;
        let polished = polish_status == "done";
        conn.execute(
            "UPDATE clipboard_history SET content=?1, segments=?2,
                meta_info=json_set(COALESCE(meta_info,'{}'),
                    '$.polished', ?3,
                    '$.polish_model', ?4,
                    '$.char_count', ?5,
                    '$.duration_ms', ?6)
             WHERE id=?7",
            params![text, segments, polished, polish_model, char_count, duration_ms, id],
        )?;
        Ok(())
    })
}

/// 历史识别记录（设置窗口识别记录页用）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionRecord {
    pub id: String,
    pub created_at: String,
    pub engine: String,
    pub polish_status: String,
    pub duration_ms: Option<i64>,
    /// 段 JSON（[{kind, text}]，段模型真相源）。
    pub segments: Option<String>,
    /// finish_text 扁平（search/clipboard/history 直读展示）。
    pub text: Option<String>,
}

/// 分页查询历史识别记录（按 id 降序 = 最新在前）。可选搜索关键词。
/// 新 schema：从 clipboard_history WHERE item_type='voice' 读，engine/polish_status/duration_ms 从 meta_info JSON 提取。
pub fn list_transcriptions(limit: u32, offset: u32, search: Option<&str>) -> Result<Vec<TranscriptionRecord>> {
    ensure_db()?;
    with_db(|conn| list_transcriptions_search_at(conn, limit, offset, search))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
/// search = None / "" → 全列；>=3 字符走 FTS5 MATCH（倒排索引）；<3 字符回退 LIKE（trigram 无法生成 3-gram）。
pub(crate) fn list_transcriptions_search_at(
    conn: &Connection,
    limit: u32,
    offset: u32,
    search: Option<&str>,
) -> Result<Vec<TranscriptionRecord>> {
    if let Some(q) = search.filter(|s| !s.is_empty()) {
        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<TranscriptionRecord> {
            Ok(TranscriptionRecord {
                id: row.get(0)?, created_at: row.get(1)?, engine: row.get(2)?,
                polish_status: row.get(3)?, duration_ms: row.get(4)?,
                segments: row.get(5)?, text: row.get(6)?,
            })
        };
        let select_cols = "SELECT c.id, c.created_at,
                COALESCE(json_extract(c.meta_info, '$.engine'), '') as engine,
                CASE WHEN json_extract(c.meta_info, '$.polished') = 1 THEN 'done' ELSE 'off' END as polish_status,
                CAST(json_extract(c.meta_info, '$.duration_ms') AS INTEGER) as duration_ms,
                c.segments, c.content
         FROM clipboard_history c";

        if q.chars().count() >= 3 {
            // FTS5 MATCH：trigram tokenizer 对 >=3 字符生成 3-gram 做倒排索引查找（子串语义）
            // id 改 TEXT(UUID) 后不能用 id=rowid 关联，改用隐式 rowid JOIN（FTS5 content 表的 rowid = clipboard_history 隐式 rowid）
            let escaped = escape_fts5_match(q);
            let mut stmt = conn.prepare(&format!(
                "{select_cols}
                 WHERE c.item_type = 'voice'
                   AND c.rowid IN (SELECT rowid FROM clipboard_history_fts
                              WHERE clipboard_history_fts MATCH ?1)
                 ORDER BY c.created_at DESC, c.id DESC LIMIT ?2 OFFSET ?3"
            ))?;
            let rows = stmt.query_map(params![escaped, limit, offset], row_mapper)?;
            return Ok(collect_rows(rows, "fts5 search"));
        }
        // <3 字符回退 LIKE：trigram 无法生成 3-gram，MATCH 会无结果
        let pattern = format!("%{}%", q);
        let mut stmt = conn.prepare(&format!(
            "{select_cols}
             WHERE c.item_type = 'voice' AND c.content LIKE ?1
             ORDER BY c.created_at DESC, c.id DESC LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![pattern, limit, offset], row_mapper)?;
        return Ok(collect_rows(rows, "like search"));
    }
    list_transcriptions_at(conn, limit, offset)
}

/// 转义 FTS5 MATCH 查询：用双引号包裹为 phrase，内部双引号双写。
/// trigram tokenizer 对 phrase 做连续 3-gram 匹配，语义等价子串匹配。
pub(crate) fn escape_fts5_match(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// 批量删除识别记录（按 id）。返回实际删除的行数。
/// 新 schema：DELETE FROM clipboard_history WHERE id IN (...)。
pub fn delete_transcriptions(ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    ensure_db()?;
    with_db(|conn| delete_transcriptions_at(conn, ids))
}

pub(crate) fn delete_transcriptions_at(conn: &Connection, ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> = ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();
    let sql = format!("DELETE FROM clipboard_history WHERE id IN ({})", placeholders);
    let n = conn.execute(&sql, params.as_slice())?;
    Ok(n)
}

pub(crate) fn list_transcriptions_at(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TranscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at,
                COALESCE(json_extract(meta_info, '$.engine'), '') as engine,
                CASE WHEN json_extract(meta_info, '$.polished') = 1 THEN 'done' ELSE 'off' END as polish_status,
                CAST(json_extract(meta_info, '$.duration_ms') AS INTEGER) as duration_ms,
                segments, content
         FROM clipboard_history WHERE item_type = 'voice'
         ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2"
    )?;
    let rows = stmt.query_map(params![limit, offset], |row| {
        Ok(TranscriptionRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            engine: row.get(2)?,
            polish_status: row.get(3)?,
            duration_ms: row.get(4)?,
            segments: row.get(5)?,
            text: row.get(6)?,
        })
    })?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;
    use rusqlite::Connection;

    /// 在内存 DB 上执行 INIT_SQL，返回初始化好的连接。
    fn open_init() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    #[test]
    fn update_and_finalize_round_trip() {
        let conn = open_init();
        // 新 schema：voice 条目存 clipboard_history，content=text，segments=段 JSON，meta_info JSON 存 engine/polished/char_count/duration_ms。
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES ('test-100', 'voice', '首段', '[{\"kind\":\"raw\",\"text\":\"首段\"}]', '{\"engine\":\"sensevoice\",\"polished\":false,\"char_count\":2}', '2026-06-14 00:00:00')",
            [],
        )
        .unwrap();
        // 流式补段 → 更新 content/segments
        conn.execute(
            "UPDATE clipboard_history SET content='首段二段', segments='[{\"kind\":\"raw\",\"text\":\"首段二段\"}]',
                meta_info=json_set(meta_info,'$.char_count',4) WHERE id='test-100'",
            [],
        )
        .unwrap();
        // finalize → 写最终 content/segments/meta_info
        conn.execute(
            "UPDATE clipboard_history SET content='润色', segments='[{\"kind\":\"polished\",\"text\":\"润色\"}]',
                meta_info=json_set(meta_info,'$.polished',1,'$.char_count',2,'$.duration_ms',5000) WHERE id='test-100'",
            [],
        )
        .unwrap();

        let (text, segments, polished, dur): (String, String, i64, Option<i64>) = conn
            .query_row(
                "SELECT content, segments, json_extract(meta_info,'$.polished'), json_extract(meta_info,'$.duration_ms') FROM clipboard_history WHERE id='test-100'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(text, "润色");
        assert!(segments.contains("\"kind\":\"polished\""));
        assert_eq!(polished, 1);
        assert_eq!(dur, Some(5000));
    }

    #[test]
    fn list_transcriptions_returns_records_descending() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES ('test-100', 'voice', '你好', '{\"engine\":\"whisper\",\"polished\":false}', '2026-06-17 10:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES ('test-200', 'voice', '你好，世界。', '[{\"kind\":\"polished\",\"text\":\"你好，世界。\"}]', '{\"engine\":\"qwen3\",\"polished\":true}', '2026-06-17 11:00:00')",
            [],
        )
        .unwrap();
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "test-200", "最新在前（created_at 降序）");
        assert_eq!(rows[1].id, "test-100");
        assert_eq!(rows[0].text.as_deref(), Some("你好，世界。"));
        assert_eq!(rows[0].polish_status, "done");
        let page1 = list_transcriptions_at(&conn, 1, 0).unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].id, "test-200");
        let page2 = list_transcriptions_at(&conn, 1, 1).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, "test-100");
        let page3 = list_transcriptions_at(&conn, 10, 2).unwrap();
        assert!(page3.is_empty());
    }

    #[test]
    fn delete_transcriptions_removes_specified_ids() {
        let conn = open_init();
        for &(id, eng, txt) in &[
            ("test-100", "whisper", "你好"),
            ("test-200", "qwen3", "你好世界"),
            ("test-300", "sensevoice", "测试"),
        ] {
            conn.execute(
                "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
                 VALUES (?1, 'voice', ?2, ?3, '2026-06-17 10:00:00')",
                params![id, txt, format!("{{\"engine\":\"{}\",\"polished\":false}}", eng)],
            )
            .unwrap();
        }
        let n = conn
            .execute(
                "DELETE FROM clipboard_history WHERE id IN (?,?)",
                params!["test-200", "test-300"],
            )
            .unwrap();
        assert_eq!(n, 2);
        let remaining = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "test-100");
    }

    #[test]
    fn delete_transcriptions_at_empty_is_noop() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES ('test-100', 'voice', '你好', '{\"engine\":\"whisper\",\"polished\":false}', '2026-06-17 10:00:00')",
            [],
        )
        .unwrap();
        // 空列表不执行 SQL，不报错
        let n = delete_transcriptions_at(&conn, &[]).unwrap();
        assert_eq!(n, 0);
        let remaining = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn delete_transcriptions_at_via_internal_fn() {
        let conn = open_init();
        for &(id, txt) in &[("test-100", "你好"), ("test-200", "世界")] {
            conn.execute(
                "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
                 VALUES (?, 'voice', ?, '{\"engine\":\"test\",\"polished\":false}', '2026-06-17 10:00:00')",
                params![id, txt],
            )
            .unwrap();
        }
        let n = delete_transcriptions_at(&conn, &["test-100".to_string(), "test-200".to_string()]).unwrap();
        assert_eq!(n, 2);
        assert!(list_transcriptions_at(&conn, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn update_edited_text_persists_and_lists() {
        let conn = open_init();
        // id="test-100"：将被编辑的记录
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES ('test-100', 'voice', '润色稿', '[{\"kind\":\"polished\",\"text\":\"润色稿\"}]', '{\"engine\":\"whisper\",\"polished\":true}', '2026-06-18 10:00:00')",
            [],
        )
        .unwrap();
        // id="test-200"：未编辑的对照记录
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES ('test-200', 'voice', '另一条', '{\"engine\":\"qwen3\",\"polished\":false}', '2026-06-18 11:00:00')",
            [],
        )
        .unwrap();

        // 走真实 update_edited_segments_at（而非裸 SQL），断言返回行数 1
        let segs = r#"[{"kind":"edited","text":"手改文本"}]"#;
        let n = update_edited_segments_at(&conn, "test-100", "手改文本", segs).unwrap();
        assert_eq!(n, 1);

        // 经 list_transcriptions_at 回读，同时验证 list 列序映射正确
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "test-200", "最新在前（created_at 降序）");
        assert_eq!(rows[1].id, "test-100");
        assert_eq!(rows[1].text.as_deref(), Some("手改文本"));
        assert_eq!(rows[1].segments.as_deref(), Some(segs));
        // 未编辑记录：text 仍是原值
        assert_eq!(rows[0].text.as_deref(), Some("另一条"));

        // 不存在的 id：返回 0 行更新
        let missing = update_edited_segments_at(&conn, "test-9999", "无效", "[]").unwrap();
        assert_eq!(missing, 0);
    }

    // ── FTS5 搜索（trigram MATCH >=3 char，LIKE 回退 <3 char）──

    /// 辅助：插入 voice 行，返回连接
    fn open_with_voice(rows: &[(&str, &str)]) -> Connection {
        let conn = open_init();
        for &(id, text) in rows {
            conn.execute(
                "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
                 VALUES (?1, 'voice', ?2, '{\"engine\":\"test\"}', '2026-07-05 10:00:00')",
                params![id, text],
            ).unwrap();
        }
        conn
    }

    #[test]
    fn fts5_search_long_query_uses_match() {
        let conn = open_with_voice(&[
            ("test-100", "今天的会议纪要很详细"),
            ("test-200", "明天去爬山"),
        ]);
        // 4 字符 → FTS5 MATCH 路径
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("会议纪要")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "test-100");
        assert_eq!(rows[0].text.as_deref(), Some("今天的会议纪要很详细"));
    }

    #[test]
    fn fts5_search_short_query_falls_back_to_like() {
        let conn = open_with_voice(&[
            ("test-100", "你好世界"),
            ("test-200", "再见"),
        ]);
        // 2 字符 → LIKE 回退（trigram 无法生成 3-gram）
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("你好")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "test-100");
    }

    #[test]
    fn fts5_search_special_chars_no_panic() {
        let conn = open_with_voice(&[("test-100", "test*result"), ("test-200", "a\"quoted\"b")]);
        // 含 FTS5 特殊字符的查询不应 panic 或 SQL 错误
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("test*resu")).unwrap();
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("AND")).unwrap();
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("quoted")).unwrap();
    }

    #[test]
    fn fts5_search_empty_content_not_indexed() {
        let conn = open_with_voice(&[("test-100", ""), ("test-200", "有内容的记录")]);
        // 空 content 不索引，但搜索应正常返回有内容的行
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("有内容的")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "test-200");
    }

    #[test]
    fn fts5_backfill_sql_is_idempotent() {
        // 验证 backfill SQL 本身的正确性与幂等性（实际触发器行为由 FTS5 外部内容表保证）
        let conn = open_with_voice(&[("test-100", "历史遗留的会议记录"), ("test-200", "另一条记录")]);
        // backfill SQL（id 改 TEXT(UUID) 后用隐式 rowid 关联 FTS5，而非 id）
        let backfill = "INSERT INTO clipboard_history_fts(rowid, content)
             SELECT rowid, content FROM clipboard_history
             WHERE content != ''
               AND rowid NOT IN (SELECT rowid FROM clipboard_history_fts)";
        // 触发器已索引这些行（NOT IN 排除）→ backfill 不插入（幂等）
        conn.execute_batch(backfill).unwrap();
        // 隐式 rowid 不再等于业务 id——按 content 的非空行计数（两条 voice 都有内容）
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clipboard_history_fts WHERE content != ''", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "行已在索引中，backfill 幂等不重复");
        // backfill 后搜索仍正常
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("会议记录")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "test-100");
    }

    #[test]
    fn fts5_escape_wraps_in_phrase() {
        assert_eq!(escape_fts5_match("会议纪要"), "\"会议纪要\"");
        assert_eq!(escape_fts5_match("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_fts5_match("AND"), "\"AND\"");
    }

    /// update_meta_field_at 增量写 meta JSON：新 key 加入 + 原 engine/char_count 不丢。
    /// 用例：模拟云端 close 异常后写 cloud_close_error 诊断字段。
    #[test]
    fn update_meta_field_increments_json() {
        let conn = open_init();
        // 建记录，meta_info 已含 engine/char_count（模拟 insert_transcription_at_id 后的状态）
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES ('test-100', 'voice', '你好',
                '{\"engine\":\"baidu\",\"polished\":false,\"char_count\":2}',
                '2026-08-03 00:00:00')",
            [],
        )
        .unwrap();
        // 写诊断字段
        let rows = update_meta_field_at(&conn, "test-100", "cloud_close_error", "cloud close 超时（8s）").unwrap();
        assert_eq!(rows, 1, "应更新 1 行");
        // 读回：cloud_close_error 已加，engine/char_count 原样保留
        let (engine, char_count, cloud_err): (String, i64, String) = conn
            .query_row(
                "SELECT json_extract(meta_info,'$.engine'),
                        json_extract(meta_info,'$.char_count'),
                        json_extract(meta_info,'$.cloud_close_error')
                 FROM clipboard_history WHERE id='test-100'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(engine, "baidu", "原 engine 不丢");
        assert_eq!(char_count, 2, "原 char_count 不丢");
        assert_eq!(cloud_err, "cloud close 超时（8s）");
    }

    /// update_meta_field_at 对 NULL meta_info 兜底 '{}'：不会丢全部 meta。
    #[test]
    fn update_meta_field_handles_null_meta() {
        let conn = open_init();
        // meta_info 为 NULL（异常状态）
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES ('test-200', 'voice', 'test', NULL, '2026-08-03 00:00:00')",
            [],
        )
        .unwrap();
        update_meta_field_at(&conn, "test-200", "cloud_close_error", "err").unwrap();
        let cloud_err: String = conn
            .query_row(
                "SELECT json_extract(meta_info,'$.cloud_close_error')
                 FROM clipboard_history WHERE id='test-200'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(cloud_err, "err", "NULL meta 兜底空对象后能写入");
    }
}
