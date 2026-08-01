// db/hotword.rs —— HotwordSet（hotword_sets 元数据）+ HotwordWord（hotword_words 每词一条）
//   + hotword_hits 全局命中 + recent_text 挖掘源。
// v57（2026-08-01）：words_text 列移除，词数据迁到 hotword_words 表（每词一条记录）。

use super::{ensure_db, with_db, Connection, Result, params};

// ── HotwordSet（热词版本/场景元数据）──────────────────────────────
// v57 起 set 只存元数据（id/name/enabled/timestamps/sync_md5），词数据在 hotword_words。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordSet {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    /// md5 内容指纹（set 元数据的指纹；词级指纹在 hotword_words.sync_md5）。
    pub sync_md5: Option<String>,
}

const HOTWORD_SET_COLS: &str = "id, name, enabled, created_at, updated_at, sync_md5";

/// 单个热词词典（版本）的词数上限（2026-08-01）。
///
/// 限制理由：① 加载时 `HotwordIndex::from_words` 构建 O(N) 索引；② fuzzy 搜索
/// `match_score` 逐词 O(N) 匹配。词数过大影响启动 + 搜索性能。3000 覆盖典型场景
/// （专业术语/专有名词），超出建议用户另建新词典分摊。
pub const HOTWORD_SET_MAX_WORDS: usize = 3000;

/// 校验 set 的词数是否超容量上限。统计 hotword_words 中该 set 的活跃词（is_deleted=0）。
fn ensure_within_capacity(conn: &Connection, set_id: &str, adding: usize) -> Result<()> {
    let cur: i64 = conn.query_row(
        "SELECT COUNT(*) FROM hotword_words WHERE set_id=?1 AND is_deleted=0",
        params![set_id],
        |r| r.get(0),
    )?;
    if cur + adding as i64 > HOTWORD_SET_MAX_WORDS as i64 {
        anyhow::bail!(
            "词典容量已满（{} 词上限），建议另建新词典分摊（当前 {} 词，再加 {} 词）",
            HOTWORD_SET_MAX_WORDS, cur, adding
        );
    }
    Ok(())
}

fn row_to_hotword_set(row: &rusqlite::Row) -> rusqlite::Result<HotwordSet> {
    Ok(HotwordSet {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        sync_md5: row.get(5)?,
    })
}

/// 列出全部版本（按 name 升序——UUID 字符串排序无意义，按 name 对用户友好）。设置页渲染用。
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>> {
    ensure_db()?;
    with_db(|conn| list_hotword_sets_at(conn))
}

pub(crate) fn list_hotword_sets_at(conn: &Connection) -> Result<Vec<HotwordSet>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {c} FROM hotword_sets ORDER BY name ASC",
        c = HOTWORD_SET_COLS
    ))?;
    let rows = stmt.query_map([], row_to_hotword_set)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 单条查询（rename/toggle 后回读、命令层透传用）。
pub fn get_hotword_set(id: &str) -> Result<HotwordSet> {
    ensure_db()?;
    with_db(|conn| get_hotword_set_at(conn, id))
}

pub(crate) fn get_hotword_set_at(conn: &Connection, id: &str) -> Result<HotwordSet> {
    conn.query_row(
        &format!("SELECT {c} FROM hotword_sets WHERE id=?1", c = HOTWORD_SET_COLS),
        params![id],
        row_to_hotword_set,
    )
    .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))
}

/// 新建空版本。调用方先 `Uuid::new_v4().to_string()` 生成 id 传入（不再 AUTOINCREMENT）。
/// 重名由 name UNIQUE 约束拒绝（→ Err）。
pub fn insert_hotword_set(id: &str, name: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_hotword_set_at(conn, id, name))
}

pub(crate) fn insert_hotword_set_at(conn: &Connection, id: &str, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_sets (id, name) VALUES (?1, ?2)",
        params![id, name],
    )?;
    Ok(())
}

/// 改名。同时刷新 updated_at。
pub fn rename_hotword_set(id: &str, name: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| rename_hotword_set_at(conn, id, name))
}

pub(crate) fn rename_hotword_set_at(conn: &Connection, id: &str, name: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE hotword_sets SET name=?1, updated_at=datetime('now') WHERE id=?2",
        params![name, id],
    )?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 勾选/取消勾选（enabled=true 时纳入并集）。刷新 updated_at。
pub fn toggle_hotword_set(id: &str, enabled: bool) -> Result<()> {
    ensure_db()?;
    with_db(|conn| toggle_hotword_set_at(conn, id, enabled))
}

pub(crate) fn toggle_hotword_set_at(conn: &Connection, id: &str, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE hotword_sets SET enabled=?1, updated_at=datetime('now') WHERE id=?2",
        params![if enabled { 1 } else { 0 }, id],
    )?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 删除版本（连带删除其下所有词记录）。
pub fn delete_hotword_set(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_hotword_set_at(conn, id))
}

pub(crate) fn delete_hotword_set_at(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
    let n = conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// upsert 热词版本元数据——sync pull 从文件读回写 SQLite 用（v46 新增，v57 去 words_text）。
///
/// `id` 已存在时按全字段覆盖（name/enabled/created_at/updated_at/sync_md5），
/// 不存在时插入。name UNIQUE 冲突时返 Err。
pub fn upsert_hotword_set(h: &HotwordSet) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_hotword_set_at(conn, h))
}

pub(crate) fn upsert_hotword_set_at(conn: &Connection, h: &HotwordSet) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_sets (id, name, enabled, created_at, updated_at, sync_md5)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            enabled=excluded.enabled,
            created_at=excluded.created_at,
            updated_at=excluded.updated_at,
            sync_md5=excluded.sync_md5",
        params![
            h.id,
            h.name,
            if h.enabled { 1 } else { 0 },
            h.created_at,
            h.updated_at,
            h.sync_md5,
        ],
    )?;
    Ok(())
}

/// 只更新 sync_md5 字段（写命令后回填用——desktop 命令层算好 md5 调此函数）。
pub fn update_hotword_set_sync_md5(id: &str, sync_md5: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let n = conn.execute(
            "UPDATE hotword_sets SET sync_md5 = ?1 WHERE id = ?2",
            params![sync_md5, id],
        )?;
        if n == 0 {
            anyhow::bail!("热词版本不存在");
        }
        Ok(())
    })
}

// ── HotwordWord（hotword_words 表，每词一条记录，schema v57）──────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordWord {
    pub id: String,
    pub set_id: String,
    pub word: String,
    pub pinyin: String,
    pub is_deleted: bool,
    pub created_at: String,
    pub updated_at: String,
    pub sync_md5: Option<String>,
}

const HOTWORD_WORD_COLS: &str = "id, set_id, word, pinyin, is_deleted, created_at, updated_at, sync_md5";

fn row_to_hotword_word(row: &rusqlite::Row) -> rusqlite::Result<HotwordWord> {
    Ok(HotwordWord {
        id: row.get(0)?,
        set_id: row.get(1)?,
        word: row.get(2)?,
        pinyin: row.get(3)?,
        is_deleted: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        sync_md5: row.get(7)?,
    })
}

/// 列出某 set 的全部活跃词（is_deleted=0，按 word 升序）。
pub fn list_words_in_set(set_id: &str) -> Result<Vec<HotwordWord>> {
    ensure_db()?;
    with_db(|conn| list_words_in_set_at(conn, set_id))
}

pub(crate) fn list_words_in_set_at(conn: &Connection, set_id: &str) -> Result<Vec<HotwordWord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {c} FROM hotword_words WHERE set_id=?1 AND is_deleted=0 ORDER BY word ASC",
        c = HOTWORD_WORD_COLS
    ))?;
    let rows = stmt.query_map(params![set_id], row_to_hotword_word)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 纠错热路径用——取所有 enabled set 的活跃词（word + 原始拼音 + hit_count），跨 set 去重并集。
/// 返回 (word, pinyin, hit_count) 三元组——拼音随词带出（HotwordIndex 不必现算 to_pinyin），
/// hit_count 从 hotword_hits LEFT JOIN（无命中记录 = 0），用于 correct 多命中排序。
pub fn list_active_words() -> Result<Vec<(String, String, i64)>> {
    ensure_db()?;
    with_db(|conn| list_active_words_at(conn))
}

pub(crate) fn list_active_words_at(conn: &Connection) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT w.word, w.pinyin, COALESCE(h.hit_count, 0) FROM hotword_words w
         JOIN hotword_sets s ON w.set_id = s.id
         LEFT JOIN hotword_hits h ON h.word = w.word
         WHERE s.enabled = 1 AND w.is_deleted = 0",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    // 跨 set 去重（同词取第一条——同词拼音必然相同，hit_count 全局一致）
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        let (w, p, hc) = r?;
        if seen.insert(w.clone()) {
            out.push((w, p, hc));
        }
    }
    Ok(out)
}

/// 单条查询（sync / 校验用）。
pub fn get_hotword_word(set_id: &str, word: &str) -> Result<Option<HotwordWord>> {
    ensure_db()?;
    with_db(|conn| get_hotword_word_at(conn, set_id, word))
}

pub(crate) fn get_hotword_word_at(
    conn: &Connection,
    set_id: &str,
    word: &str,
) -> Result<Option<HotwordWord>> {
    let r = conn.query_row(
        &format!("SELECT {c} FROM hotword_words WHERE set_id=?1 AND word=?2", c = HOTWORD_WORD_COLS),
        params![set_id, word],
        row_to_hotword_word,
    );
    match r {
        Ok(w) => Ok(Some(w)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 追加一词到指定 set。重复词（含软删的）恢复为活跃（is_deleted=0）。
/// 复合键 (set_id, word) 幂等——重复加同词无副作用。返回是否实际新增或恢复。
pub fn add_word_to_set(set_id: &str, word: &str) -> Result<bool> {
    ensure_db()?;
    with_db(|conn| add_word_to_set_at(conn, set_id, word))
}

pub(crate) fn add_word_to_set_at(conn: &Connection, set_id: &str, word: &str) -> Result<bool> {
    ensure_within_capacity(conn, set_id, 1)?;
    let id = crate::hotword_text::hotword_word_uuid(set_id, word);
    let pinyin = crate::hotword_text::word_plain_pinyins(word).join(" ");
    // 先查是否已存在且活跃——避免容量校验后重复加浪费配额判断
    let already_active: bool = conn
        .query_row(
            "SELECT is_deleted FROM hotword_words WHERE set_id=?1 AND word=?2",
            params![set_id, word],
            |r| r.get::<_, i64>(0),
        )
        .map(|d| d == 0)
        .unwrap_or(false);
    if already_active {
        return Ok(false); // 已活跃，幂等无操作
    }
    // sync_md5：写入时填（对齐 vault cipher storage 层）——word 级 merge 据此 diff
    let sync_md5 = crate::hotword_text::hotword_word_md5_from_fields(set_id, word, &pinyin, false);
    // ON CONFLICT(set_id, word)：已存在（软删态）→ 恢复 is_deleted=0；不存在 → INSERT
    conn.execute(
        "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted, sync_md5)
         VALUES (?1, ?2, ?3, ?4, 0, ?5)
         ON CONFLICT(set_id, word) DO UPDATE SET
            is_deleted=0, pinyin=excluded.pinyin, sync_md5=excluded.sync_md5, updated_at=datetime('now')",
        params![id, set_id, word, pinyin, sync_md5],
    )?;
    Ok(true)
}

/// 批量追加多词（挖掘/导入追加用），返回实际新增/恢复条数。
pub fn add_words_to_set(set_id: &str, words: &[String]) -> Result<usize> {
    ensure_db()?;
    with_db(|conn| add_words_to_set_at(conn, set_id, words))
}

pub(crate) fn add_words_to_set_at(
    conn: &Connection,
    set_id: &str,
    words: &[String],
) -> Result<usize> {
    // 去重输入
    let unique: std::collections::HashSet<&str> = words.iter().map(|s| s.as_str()).collect();
    let unique: Vec<&str> = unique.into_iter().collect();
    ensure_within_capacity(conn, set_id, unique.len())?;
    let mut added = 0;
    for word in &unique {
        let id = crate::hotword_text::hotword_word_uuid(set_id, word);
        let pinyin = crate::hotword_text::word_plain_pinyins(word).join(" ");
        let sync_md5 = crate::hotword_text::hotword_word_md5_from_fields(set_id, word, &pinyin, false);
        let n = conn.execute(
            "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted, sync_md5)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)
             ON CONFLICT(set_id, word) DO UPDATE SET
                is_deleted=0, pinyin=excluded.pinyin, sync_md5=excluded.sync_md5, updated_at=datetime('now')",
            params![id, set_id, word, pinyin, sync_md5],
        )?;
        if n > 0 {
            added += 1;
        }
    }
    Ok(added)
}

/// 从指定 set 软删一词（is_deleted=1，记录保留供 sync 传播）。
pub fn remove_word_from_set(set_id: &str, word: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| remove_word_from_set_at(conn, set_id, word))
}

pub(crate) fn remove_word_from_set_at(conn: &Connection, set_id: &str, word: &str) -> Result<()> {
    // 软删前先读 pinyin（md5 需要它）——is_deleted=true 的 md5 与活跃态不同（参与 diff）
    let pinyin: String = conn
        .query_row(
            "SELECT pinyin FROM hotword_words WHERE set_id=?1 AND word=?2 AND is_deleted=0",
            params![set_id, word],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let sync_md5 = crate::hotword_text::hotword_word_md5_from_fields(set_id, word, &pinyin, true);
    conn.execute(
        "UPDATE hotword_words SET is_deleted=1, sync_md5=?3, updated_at=datetime('now')
         WHERE set_id=?1 AND word=?2 AND is_deleted=0",
        params![set_id, word, sync_md5],
    )?;
    Ok(())
}

/// 覆盖某 set 的词列表（导入「覆盖」模式用）。diff：新增词 INSERT，缺失词软删。
pub fn set_words_in_set(set_id: &str, words: &[String]) -> Result<()> {
    ensure_db()?;
    with_db(|conn| set_words_in_set_at(conn, set_id, words))
}

pub(crate) fn set_words_in_set_at(
    conn: &Connection,
    set_id: &str,
    words: &[String],
) -> Result<()> {
    let unique: std::collections::HashSet<&str> = words.iter().map(|s| s.as_str()).collect();
    ensure_within_capacity(conn, set_id, unique.len())?;
    // 软删不在新列表的活跃词——逐词算 is_deleted=true 的 md5（需读 pinyin）
    let to_remove: Vec<(String, String)> = {
        let json_list = serde_json::to_string(&unique.iter().collect::<Vec<_>>())?;
        let mut stmt = conn.prepare(
            "SELECT word, pinyin FROM hotword_words
             WHERE set_id=?1 AND is_deleted=0 AND word NOT IN (SELECT value FROM json_each(?2))",
        )?;
        let rows = stmt.query_map(params![set_id, json_list], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for (word, pinyin) in &to_remove {
        let sync_md5 =
            crate::hotword_text::hotword_word_md5_from_fields(set_id, word, pinyin, true);
        conn.execute(
            "UPDATE hotword_words SET is_deleted=1, sync_md5=?3, updated_at=datetime('now')
             WHERE set_id=?1 AND word=?2 AND is_deleted=0",
            params![set_id, word, sync_md5],
        )?;
    }
    // 添加/恢复新列表的词
    for word in &unique {
        let id = crate::hotword_text::hotword_word_uuid(set_id, word);
        let pinyin = crate::hotword_text::word_plain_pinyins(word).join(" ");
        let sync_md5 =
            crate::hotword_text::hotword_word_md5_from_fields(set_id, word, &pinyin, false);
        conn.execute(
            "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted, sync_md5)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)
             ON CONFLICT(set_id, word) DO UPDATE SET
                is_deleted=0, pinyin=excluded.pinyin, sync_md5=excluded.sync_md5, updated_at=datetime('now')",
            params![id, set_id, word, pinyin, sync_md5],
        )?;
    }
    Ok(())
}

/// upsert 热词词记录——sync pull 从文件读回写 SQLite 用。
pub fn upsert_hotword_word(w: &HotwordWord) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_hotword_word_at(conn, w))
}

pub(crate) fn upsert_hotword_word_at(conn: &Connection, w: &HotwordWord) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted, created_at, updated_at, sync_md5)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            set_id=excluded.set_id, word=excluded.word, pinyin=excluded.pinyin,
            is_deleted=excluded.is_deleted, created_at=excluded.created_at,
            updated_at=excluded.updated_at, sync_md5=excluded.sync_md5",
        params![
            w.id, w.set_id, w.word, w.pinyin,
            if w.is_deleted { 1 } else { 0 },
            w.created_at, w.updated_at, w.sync_md5,
        ],
    )?;
    Ok(())
}

/// 列出全部词记录（含软删，sync export 用）。
pub fn list_all_hotword_words() -> Result<Vec<HotwordWord>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {c} FROM hotword_words ORDER BY id ASC",
            c = HOTWORD_WORD_COLS
        ))?;
        let rows = stmt.query_map([], row_to_hotword_word)?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
}

/// 只更新词记录的 sync_md5（写命令后回填）。
pub fn update_hotword_word_sync_md5(id: &str, sync_md5: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE hotword_words SET sync_md5 = ?1 WHERE id = ?2",
            params![sync_md5, id],
        )?;
        Ok(())
    })
}

/// 取最近 limit 条 ASR/文本记录的 content（挖掘候选用）。
///
/// **INV-C1（热词来源不断）**：故意不过滤 `is_deleted`——软删内容仍是热词来源，
/// 这是剪贴板软删/回收站功能的核心目的。用户把文本删进回收站后，这里仍能读到它，
/// 热词挖掘继续工作。只有永久删除（`DELETE FROM`）才会让行真正消失、挖不到。
/// `ORDER BY id DESC LIMIT N` 降序取最新 N 条，软删内容 id 不变（软删只改 is_deleted），
/// 活跃和软删混在同一条时间线，不会互相挤占名额。
pub fn list_recent_text(limit: i64) -> Result<Vec<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM clipboard_history
             WHERE item_type IN ('voice','text','ocr') AND content IS NOT NULL AND content != ''
             -- 故意不过滤 is_deleted（INV-C1：软删内容仍是热词来源）
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
}

/// 取最近 limit 条 voice（ASR 识别）记录的 content——bigram 上下文打分用（仅 ASR 语料）。
/// 与 `list_recent_text` 区别：只取 item_type='voice'，语料更纯（与纠错场景一致）。
/// **故意不过滤 is_deleted**（INV-C1 对齐）：软删 voice 仍是 bigram 语料来源，
/// voice 软删回收站上限 VOICE_TRASH_MAX=500（2026-08-02 从 100 提升，丰富 bigram 语料）。
pub fn list_recent_voice_text(limit: i64) -> Result<Vec<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM clipboard_history
             WHERE item_type = 'voice' AND content IS NOT NULL AND content != ''
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
}

/// 命中计数 +1（按词文本——corrector 命中时只有文本）。写全局 `hotword_hits`（upsert）。
/// pipeline 在 correct 后批量调用（best-effort，失败由调用方忽略，不阻断纠错）。
pub fn bump_hotword_hit_by_word(word: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| bump_hotword_hit_by_word_at(conn, word))
}

pub(crate) fn bump_hotword_hit_by_word_at(conn: &Connection, word: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_hits(word, hit_count) VALUES(?1, 1) \
         ON CONFLICT(word) DO UPDATE SET hit_count = hit_count + 1",
        params![word],
    )?;
    Ok(())
}

/// 全局命中计数（前端卡片命中展示用）。返回 word → hit_count。
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>> {
    ensure_db()?;
    with_db(|conn| list_hotword_hits_at(conn))
}

pub(crate) fn list_hotword_hits_at(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT word, hit_count FROM hotword_hits")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for r in rows {
        let (w, c) = r?;
        map.insert(w, c);
    }
    Ok(map)
}

// ── FuzzyDialectRule（方言模糊规则）DB 驱动，2026-08-01 ──────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FuzzyDialectRule {
    pub token: String,
    pub label: String,
    pub from_py: String,
    pub to_py: String,
    /// 'syllable'(整音节精确) | 'initial'(声母前缀) | 'special_hu'(hu→wu+huX→wX)
    pub match_type: String,
    pub enabled: bool,
    pub sort_order: i64,
}

fn row_to_fuzzy_rule(row: &rusqlite::Row) -> rusqlite::Result<FuzzyDialectRule> {
    Ok(FuzzyDialectRule {
        token: row.get(0)?,
        label: row.get(1)?,
        from_py: row.get(2)?,
        to_py: row.get(3)?,
        match_type: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        sort_order: row.get(6)?,
    })
}

const FUZZY_RULE_COLS: &str = "token, label, from_py, to_py, match_type, enabled, sort_order";

/// 列出全部方言规则（含未启用），按 match_type + sort_order 排序。
pub fn list_fuzzy_dialect_rules() -> Result<Vec<FuzzyDialectRule>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {FUZZY_RULE_COLS} FROM fuzzy_dialect_rules ORDER BY match_type, sort_order"
        ))?;
        let rows = stmt.query_map([], row_to_fuzzy_rule)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

/// 只列出 enabled=1 的规则（normalize 用），按 match_type + sort_order 排序。
pub fn list_enabled_fuzzy_dialect_rules() -> Result<Vec<FuzzyDialectRule>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {FUZZY_RULE_COLS} FROM fuzzy_dialect_rules WHERE enabled=1 ORDER BY match_type, sort_order"
        ))?;
        let rows = stmt.query_map([], row_to_fuzzy_rule)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

/// 设置单条规则的开关（前端 toggle 用）。
pub fn set_fuzzy_dialect_rule_enabled(token: &str, enabled: bool) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let n = conn.execute(
            "UPDATE fuzzy_dialect_rules SET enabled=?1 WHERE token=?2",
            params![if enabled { 1 } else { 0 }, token],
        )?;
        if n == 0 {
            anyhow::bail!("方言规则 {} 不存在", token);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;
    use rusqlite::Connection;

    /// HotwordSet 元数据 CRUD + HotwordWord 词记录 CRUD 往返（v57 新模型）。
    #[test]
    fn hotword_set_and_word_crud_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // create set
        let id = "test-uuid-项目A-001".to_string();
        insert_hotword_set_at(&conn, &id, "项目A").unwrap();

        // list set
        let sets = list_hotword_sets_at(&conn).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "项目A");
        assert!(sets[0].enabled);
        assert!(sets[0].sync_md5.is_none());

        // 重名 → 唯一冲突
        assert!(insert_hotword_set_at(&conn, "test-uuid-项目A-002", "项目A").is_err());

        // rename + toggle
        rename_hotword_set_at(&conn, &id, "项目A2").unwrap();
        toggle_hotword_set_at(&conn, &id, false).unwrap();
        assert!(!list_hotword_sets_at(&conn).unwrap()[0].enabled);
        toggle_hotword_set_at(&conn, &id, true).unwrap();

        // add_word（v57：每词一条记录，确定性 UUID）
        assert!(add_word_to_set_at(&conn, &id, "吴大锐").unwrap());
        assert!(add_word_to_set_at(&conn, &id, "八爪鱼").unwrap());
        assert!(!add_word_to_set_at(&conn, &id, "八爪鱼").unwrap()); // 重复 → 幂等，false
        let words = list_words_in_set_at(&conn, &id).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].word, "八爪鱼"); // ORDER BY word ASC
        assert_eq!(words[1].word, "吴大锐");
        // 拼音存原始（八爪鱼 → ba zhao yu）
        assert_eq!(words[0].pinyin, "ba zhao yu");
        assert!(!words[0].is_deleted);

        // remove_word（软删——is_deleted=1，记录保留）
        remove_word_from_set_at(&conn, &id, "八爪鱼").unwrap();
        let words_after = list_words_in_set_at(&conn, &id).unwrap();
        assert_eq!(words_after.len(), 1); // 软删后 list 过滤掉
        assert_eq!(words_after[0].word, "吴大锐");
        // 软删记录仍在 DB（is_deleted=1）
        let soft = get_hotword_word_at(&conn, &id, "八爪鱼").unwrap().unwrap();
        assert!(soft.is_deleted);

        // 软删后重新加同词 → 恢复（is_deleted=0）
        assert!(add_word_to_set_at(&conn, &id, "八爪鱼").unwrap());
        let restored = get_hotword_word_at(&conn, &id, "八爪鱼").unwrap().unwrap();
        assert!(!restored.is_deleted);

        // delete set 连带删词
        delete_hotword_set_at(&conn, &id).unwrap();
        assert!(list_hotword_sets_at(&conn).unwrap().is_empty());
        assert!(list_words_in_set_at(&conn, &id).unwrap().is_empty());
    }

    /// upsert HotwordSet（元数据）——sync pull 用。
    #[test]
    fn hotword_set_upsert_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        let h1 = HotwordSet {
            id: "upsert-uuid-1".into(),
            name: "版本X".into(),
            enabled: true,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
            sync_md5: Some("md5-abc".into()),
        };
        upsert_hotword_set_at(&conn, &h1).unwrap();
        let loaded = get_hotword_set_at(&conn, "upsert-uuid-1").unwrap();
        assert_eq!(loaded.name, "版本X");
        assert_eq!(loaded.sync_md5.as_deref(), Some("md5-abc"));

        // 覆盖路径
        let h2 = HotwordSet {
            id: "upsert-uuid-1".into(),
            name: "版本X改".into(),
            enabled: false,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 11:00:00".into(),
            sync_md5: Some("md5-def".into()),
        };
        upsert_hotword_set_at(&conn, &h2).unwrap();
        let loaded2 = get_hotword_set_at(&conn, "upsert-uuid-1").unwrap();
        assert_eq!(loaded2.name, "版本X改");
        assert!(!loaded2.enabled);
        assert_eq!(loaded2.sync_md5.as_deref(), Some("md5-def"));
    }

    /// 「通用」默认版本用固定 UUID——跨设备一致。
    #[test]
    fn default_general_set_uses_fixed_uuid() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        let sets = list_hotword_sets_at(&conn).unwrap();
        let general = sets.iter().find(|s| s.name == "通用").expect("应有「通用」seed");
        assert_eq!(general.id, "00000000-0000-0000-0000-000000000001");
    }

    /// list_active_words：enabled set 的活跃词并集（含拼音），跨 set 去重。
    #[test]
    fn list_active_words_is_enabled_union() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // 「通用」(enabled=1, 固定 UUID) 加词
        let general = "00000000-0000-0000-0000-000000000001";
        add_word_to_set_at(&conn, general, "八爪鱼").unwrap();
        add_word_to_set_at(&conn, general, "吴大锐").unwrap();
        // 项目A (enabled=1)
        insert_hotword_set_at(&conn, "set-A", "项目A").unwrap();
        add_word_to_set_at(&conn, "set-A", "吴大锐").unwrap(); // 跨 set 重复
        add_word_to_set_at(&conn, "set-A", "周会").unwrap();
        // 关闭的 (enabled=0)
        insert_hotword_set_at(&conn, "set-off", "关闭的").unwrap();
        toggle_hotword_set_at(&conn, "set-off", false).unwrap();
        add_word_to_set_at(&conn, "set-off", "浮窗").unwrap();

        let words = list_active_words_at(&conn).unwrap();
        let word_set: std::collections::HashSet<&str> = words.iter().map(|(w, _, _)| w.as_str()).collect();
        assert_eq!(word_set, ["八爪鱼", "吴大锐", "周会"].into_iter().collect());
        // 拼音带出
        let bz = words.iter().find(|(w, _, _)| w == "八爪鱼").unwrap();
        assert_eq!(bz.1, "ba zhao yu");

        // 全关 → 空
        toggle_hotword_set_at(&conn, general, false).unwrap();
        toggle_hotword_set_at(&conn, "set-A", false).unwrap();
        assert!(list_active_words_at(&conn).unwrap().is_empty());
    }

    #[test]
    fn bump_hit_upserts_global_hits() {
        let conn = &mut Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();

        bump_hotword_hit_by_word_at(conn, "吴大锐").unwrap();
        bump_hotword_hit_by_word_at(conn, "吴大锐").unwrap();
        bump_hotword_hit_by_word_at(conn, "八爪鱼").unwrap();

        let wu: i64 = conn.query_row("SELECT hit_count FROM hotword_hits WHERE word='吴大锐'", [], |r| r.get(0)).unwrap();
        assert_eq!(wu, 2);
        let ba: i64 = conn.query_row("SELECT hit_count FROM hotword_hits WHERE word='八爪鱼'", [], |r| r.get(0)).unwrap();
        assert_eq!(ba, 1);

        let hits = list_hotword_hits_at(conn).unwrap();
        assert_eq!(hits.get("吴大锐"), Some(&2i64));
    }

    // ── 容量上限 HOTWORD_SET_MAX_WORDS（v57：按 hotword_words 行数校验）──

    /// 生成 n 个不重复的伪词（w0..w{n-1}）。
    fn fake_words(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("w{}", i)).collect()
    }

    /// add_word_to_set_at：满 3000 后再加被拒。
    #[test]
    fn add_single_word_rejects_when_at_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-one", "单词容量测试").unwrap();

        // 填满 3000 词（直接批量 INSERT，绕过容量校验填充）
        for w in fake_words(HOTWORD_SET_MAX_WORDS) {
            let id = crate::hotword_text::hotword_word_uuid("cap-one", &w);
            conn.execute(
                "INSERT OR IGNORE INTO hotword_words (id, set_id, word, pinyin, is_deleted) VALUES (?1, ?2, ?3, '', 0)",
                params![id, "cap-one", w],
            ).unwrap();
        }

        // 再加一个新词 → 3001，应被拒
        let err = add_word_to_set_at(&conn, "cap-one", "溢出词").unwrap_err();
        assert!(err.to_string().contains("容量已满"), "满后再加应拒：{}", err);
    }

    /// add_words_to_set_at：批量追加后超 3000 被拒。
    #[test]
    fn add_words_rejects_when_exceeding_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-add", "批量容量测试").unwrap();

        // 先填 2999 词
        for w in fake_words(2999) {
            let id = crate::hotword_text::hotword_word_uuid("cap-add", &w);
            conn.execute(
                "INSERT OR IGNORE INTO hotword_words (id, set_id, word, pinyin, is_deleted) VALUES (?1, ?2, ?3, '', 0)",
                params![id, "cap-add", w],
            ).unwrap();
        }

        // 再批量加 5 词 → 3004，超限被拒
        let extra: Vec<String> = (2999..2999 + 5).map(|i| format!("w{}", i)).collect();
        let err = add_words_to_set_at(&conn, "cap-add", &extra).unwrap_err();
        assert!(err.to_string().contains("容量已满"), "超限应拒：{}", err);
    }

    /// set_words_in_set_at：覆盖模式（新增 + 软删缺失词）。
    #[test]
    fn set_words_overrides_and_soft_deletes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "override-set", "覆盖测试").unwrap();

        // 初始加 3 词
        add_word_to_set_at(&conn, "override-set", "苹果").unwrap();
        add_word_to_set_at(&conn, "override-set", "香蕉").unwrap();
        add_word_to_set_at(&conn, "override-set", "葡萄").unwrap();

        // 覆盖为 [苹果, 西瓜] —— 香蕉/葡萄 软删，西瓜 新增，苹果 保留
        set_words_in_set_at(&conn, "override-set", &["苹果".into(), "西瓜".into()]).unwrap();
        let words = list_words_in_set_at(&conn, "override-set").unwrap();
        let word_set: std::collections::HashSet<&str> = words.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(word_set, ["苹果", "西瓜"].into_iter().collect());
        // 软删的香蕉仍在 DB
        let banana = get_hotword_word_at(&conn, "override-set", "香蕉").unwrap().unwrap();
        assert!(banana.is_deleted);
    }

    // ── word sync_md5 填充（2026-08-01 word 级 merge，对齐 vault storage 层）──

    /// add_word_to_set_at 写入后 sync_md5 非 None，且等于 hotword_word_md5_from_fields(is_deleted=false)。
    /// remove_word_from_set_at 软删后 sync_md5 变成 is_deleted=true 的指纹。
    #[test]
    fn word_sync_md5_filled_on_add_and_remove() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "md5-set", "md5测试").unwrap();

        add_word_to_set_at(&conn, "md5-set", "八爪鱼").unwrap();
        let w = get_hotword_word_at(&conn, "md5-set", "八爪鱼").unwrap().unwrap();
        let expected = crate::hotword_text::hotword_word_md5_from_fields(
            "md5-set", "八爪鱼", &w.pinyin, false,
        );
        assert_eq!(
            w.sync_md5.as_deref(),
            Some(expected.as_str()),
            "add 后 sync_md5 应等于 md5(is_deleted=false)"
        );

        // 软删 → sync_md5 变成 is_deleted=true 的指纹（参与 word 级 diff）
        remove_word_from_set_at(&conn, "md5-set", "八爪鱼").unwrap();
        let soft = get_hotword_word_at(&conn, "md5-set", "八爪鱼").unwrap().unwrap();
        let expected_soft = crate::hotword_text::hotword_word_md5_from_fields(
            "md5-set", "八爪鱼", &soft.pinyin, true,
        );
        assert_eq!(
            soft.sync_md5.as_deref(),
            Some(expected_soft.as_str()),
            "remove 后 sync_md5 应等于 md5(is_deleted=true)"
        );
        assert_ne!(w.sync_md5, soft.sync_md5, "软删前后 md5 应不同");

        // 恢复（重新 add）→ sync_md5 回到 is_deleted=false 指纹
        add_word_to_set_at(&conn, "md5-set", "八爪鱼").unwrap();
        let restored = get_hotword_word_at(&conn, "md5-set", "八爪鱼").unwrap().unwrap();
        assert_eq!(restored.sync_md5, w.sync_md5, "恢复后 md5 应回到活跃指纹");
    }

    /// add_words_to_set_at 批量 + set_words_in_set_at 覆盖（含软删）都填 sync_md5。
    #[test]
    fn word_sync_md5_filled_on_batch_and_override() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "ov-set", "覆盖测试").unwrap();

        // 批量加
        add_words_to_set_at(&conn, "ov-set", &["苹果".into(), "香蕉".into()]).unwrap();
        for word in &["苹果", "香蕉"] {
            let w = get_hotword_word_at(&conn, "ov-set", word).unwrap().unwrap();
            assert!(w.sync_md5.is_some(), "批量加的词应有 sync_md5: {}", word);
        }

        // 覆盖为 [苹果] —— 香蕉软删（md5 变 is_deleted=true），苹果保留
        set_words_in_set_at(&conn, "ov-set", &["苹果".into()]).unwrap();
        let banana = get_hotword_word_at(&conn, "ov-set", "香蕉").unwrap().unwrap();
        let expected_soft = crate::hotword_text::hotword_word_md5_from_fields(
            "ov-set", "香蕉", &banana.pinyin, true,
        );
        assert_eq!(
            banana.sync_md5.as_deref(),
            Some(expected_soft.as_str()),
            "覆盖软删的香蕉应有 is_deleted=true 的 sync_md5"
        );
    }
}
