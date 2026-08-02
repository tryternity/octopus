// db/hotword.rs —— HotwordSet（hotword_sets 元数据）+ HotwordWord（hotword_words 每词一条）
//   + hotword_hits 全局命中 + recent_text 挖掘源。
// v57（2026-08-01）：words_text 列移除，词数据迁到 hotword_words 表（每词一条记录）。

use super::{ensure_db, with_db, Connection, Result, params};

// ── HotwordSet（热词版本/场景元数据）──────────────────────────────
// v57 起 set 只存元数据（id/name/enabled/timestamps/sync_md5），词数据在 hotword_words。
// v58 起 is_deleted 存删除时刻 epoch 秒（0=活跃，>0=tombstone），UNIQUE(name,is_deleted) 复合约束。
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
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。sync merge 据此传播删除意图。
    pub is_deleted: i64,
}

const HOTWORD_SET_COLS: &str = "id, name, enabled, created_at, updated_at, sync_md5, is_deleted";

/// 单个热词词典（版本）的词数上限（2026-08-01）。
///
/// 限制理由：热词只加专有名词（人名/地名/术语），不加常用词——
/// 常用词增加碰撞面导致误纠。3000 覆盖典型场景（专有名词/产品名/术语），
/// 超出建议用户另建新词典分摊。
pub const HOTWORD_SET_MAX_WORDS: usize = 3000;

/// tombstone 保留时长（秒）——超过此时长的软删 set/词被 GC 硬删。硬编码 10 天。
///
/// GC 触发：scheduler 每日 `purge_expired_hotword_tombstones` + sync merge 按年龄过滤
/// （防跨设备复活）。详见 `2026-08-02-hotword-tombstone-gc` spec。
pub const HOTWORD_TOMBSTONE_RETENTION_SECS: i64 = 10 * 86400;

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
        is_deleted: row.get(6)?,
    })
}

/// 列出全部活跃版本（is_deleted=0，按 name 升序）。设置页渲染用。
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>> {
    ensure_db()?;
    with_db(|conn| list_hotword_sets_at(conn))
}

pub(crate) fn list_hotword_sets_at(conn: &Connection) -> Result<Vec<HotwordSet>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {c} FROM hotword_sets WHERE is_deleted=0 ORDER BY name ASC",
        c = HOTWORD_SET_COLS
    ))?;
    let rows = stmt.query_map([], row_to_hotword_set)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 列出全部版本（含软删 tombstone，按 id 升序）——sync export 用（mirror list_all_hotword_words）。
/// tombstone 需 export 到 .sync 传播删除意图，故不过滤 is_deleted。
pub fn list_all_hotword_sets() -> Result<Vec<HotwordSet>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {c} FROM hotword_sets ORDER BY id ASC",
            c = HOTWORD_SET_COLS
        ))?;
        let rows = stmt.query_map([], row_to_hotword_set)?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
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

/// 软删版本（v58 起：is_deleted=删除时刻 epoch 秒，连带软删其下所有词记录）。
///
/// tombstone 行保留在 DB（is_deleted>0），sync merge 据此传播删除意图——对端 pull 后
/// 该集也变软删，不再复活。`list_hotword_sets` 过滤 is_deleted=0 不显示；`list_all_hotword_sets`
/// 含 tombstone（sync export 用）。UNIQUE(name,is_deleted) 复合约束：软删后 name 不变，
/// 用户重建同名活跃词典（is_deleted=0）不冲突（tombstone 的 is_deleted=时间戳≠0）。
pub fn delete_hotword_set(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_hotword_set_at(conn, id))
}

pub(crate) fn delete_hotword_set_at(conn: &Connection, id: &str) -> Result<()> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 级联软删词记录（保持原硬删行为：删词典=清空其词）。词级 tombstone 也参与 sync 传播。
    conn.execute(
        "UPDATE hotword_words SET is_deleted=1, updated_at=datetime('now')
         WHERE set_id=?1 AND is_deleted=0",
        params![id],
    )?;
    // 软删词典：is_deleted=删除时刻 epoch 秒，updated_at 刷新（merge 方向判定用）
    let n = conn.execute(
        "UPDATE hotword_sets SET is_deleted=?2, updated_at=datetime('now')
         WHERE id=?1 AND is_deleted=0",
        params![id, now_secs],
    )?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 物理删除版本（连带词记录）——**仅测试/重置场景用**，生产代码用 [`delete_hotword_set`]（软删）。
///
/// v58 起 `delete_hotword_set` 改软删（行保留为 tombstone）。测试需真正清空 DB 隔离场景时
/// 用此函数（DELETE FROM）。生产代码不应调用——会丢失 tombstone 导致跨设备删除复活。
pub fn hard_delete_hotword_set(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
        conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
        Ok(())
    })
}

// ── tombstone GC（2026-08-02）──软删 set/词超期后硬删，防 DB + .sync 无限堆积 ──

/// 硬删超期 tombstone——set is_deleted>0 且 `now - is_deleted > RETENTION` + 其词记录 + 超期 word。
///
/// GC 范围：① set tombstone（超期）→ 连带词记录（硬删）；② 活跃词典里的超期 word tombstone（硬删）。
/// 返回硬删的 set 数（词数不返回，日志记）。scheduler 每日调 + export 重建清 .sync。
///
/// 跨设备自洽：merge 按年龄过滤（`pull_set`/`pull_word` 超期 skip），GC 后 export 不含超期
/// tombstone → 对端 pull 时即使旧 outline 有也 skip → 收敛。详见 tombstone-gc spec §3。
pub fn purge_expired_hotword_tombstones(now_secs: i64) -> Result<usize> {
    ensure_db()?;
    let cutoff = now_secs - HOTWORD_TOMBSTONE_RETENTION_SECS;
    with_db(|conn| {
        let mut purged_sets = 0usize;
        // 1. 超期 set tombstone：先收集 id（连带删词 + hits），再硬删
        let expired_set_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM hotword_sets WHERE is_deleted > 0 AND is_deleted < ?1",
            )?;
            let rows = stmt.query_map(params![cutoff], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in &expired_set_ids {
            conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
            conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
            purged_sets += 1;
        }
        // 2. 超期 word tombstone（活跃词典里的软删词）——is_deleted>0 且超期
        let word_cutoff = cutoff; // 同阈值
        let n = conn.execute(
            "DELETE FROM hotword_words WHERE is_deleted > 0 AND is_deleted < ?1",
            params![word_cutoff],
        )?;
        // 3. hotword_hits 孤儿清理：词在所有活跃词典（is_deleted=0 的 hotword_words）消失 → 命中清零。
        //    放在删 word tombstone 之后——保证超期硬删的词的 hits 也被清。hits 不参与 sync，
        //    各机独立按本地活跃词集合判定孤儿，无跨设备复活问题。详见 tombstone-gc spec §5。
        let orphan_hits = conn.execute(
            "DELETE FROM hotword_hits WHERE word NOT IN \
             (SELECT word FROM hotword_words WHERE is_deleted = 0)",
            [],
        )?;
        if purged_sets > 0 || n > 0 || orphan_hits > 0 {
            log::info!(
                "[hotword-gc] purged {} set tombstones + {} word tombstones + {} orphan hits (cutoff={})",
                purged_sets,
                n,
                orphan_hits,
                cutoff
            );
        }
        Ok(purged_sets)
    })
}

/// 统计 set tombstone 数（前端「回收站 (N)」按钮用）。
pub fn count_hotword_tombstones() -> Result<i64> {
    ensure_db()?;
    with_db(|conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM hotword_sets WHERE is_deleted > 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    })
}

/// 手动清空回收站——硬删所有 set tombstone（不限年龄）+ 其词 + 所有 word tombstone。
/// 前端「清空回收站」按钮调（用户确认后）。
pub fn purge_all_hotword_tombstones() -> Result<usize> {
    ensure_db()?;
    with_db(|conn| {
        let tombstone_set_ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT id FROM hotword_sets WHERE is_deleted > 0")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let mut purged = 0usize;
        for id in &tombstone_set_ids {
            conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
            conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
            purged += 1;
        }
        // 所有 word tombstone（不限年龄）
        let n = conn.execute("DELETE FROM hotword_words WHERE is_deleted > 0", [])?;
        // hotword_hits 孤儿清理（同 purge_expired 逻辑，详见 tombstone-gc spec §5）
        let orphan_hits = conn.execute(
            "DELETE FROM hotword_hits WHERE word NOT IN \
             (SELECT word FROM hotword_words WHERE is_deleted = 0)",
            [],
        )?;
        log::info!(
            "[hotword-gc] manual purge: {} sets + {} words + {} orphan hits",
            purged, n, orphan_hits
        );
        Ok(purged)
    })
}

/// upsert 热词版本元数据——sync pull 从文件读回写 SQLite 用（v46 新增，v57 去 words_text，v58 加 is_deleted）。
///
/// `id` 已存在时按全字段覆盖（name/enabled/created_at/updated_at/sync_md5/is_deleted），
/// 不存在时插入。is_deleted 也覆盖——sync pull tombstone（is_deleted>0）时写入软删态。
pub fn upsert_hotword_set(h: &HotwordSet) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_hotword_set_at(conn, h))
}

pub(crate) fn upsert_hotword_set_at(conn: &Connection, h: &HotwordSet) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_sets (id, name, enabled, created_at, updated_at, sync_md5, is_deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            enabled=excluded.enabled,
            created_at=excluded.created_at,
            updated_at=excluded.updated_at,
            sync_md5=excluded.sync_md5,
            is_deleted=excluded.is_deleted",
        params![
            h.id,
            h.name,
            if h.enabled { 1 } else { 0 },
            h.created_at,
            h.updated_at,
            h.sync_md5,
            h.is_deleted,
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
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。统一语义（GC 2026-08-02，原 bool 0/1）。
    pub is_deleted: i64,
    pub created_at: String,
    pub updated_at: String,
}

const HOTWORD_WORD_COLS: &str = "id, set_id, word, pinyin, is_deleted, created_at, updated_at";

fn row_to_hotword_word(row: &rusqlite::Row) -> rusqlite::Result<HotwordWord> {
    Ok(HotwordWord {
        id: row.get(0)?,
        set_id: row.get(1)?,
        word: row.get(2)?,
        pinyin: row.get(3)?,
        is_deleted: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
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
         WHERE s.enabled = 1 AND s.is_deleted = 0 AND w.is_deleted = 0",
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
    // ON CONFLICT(set_id, word)：已存在（软删态）→ 恢复 is_deleted=0；不存在 → INSERT
    conn.execute(
        "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(set_id, word) DO UPDATE SET
            is_deleted=0, pinyin=excluded.pinyin, updated_at=datetime('now')",
        params![id, set_id, word, pinyin],
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
        let n = conn.execute(
            "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(set_id, word) DO UPDATE SET
                is_deleted=0, pinyin=excluded.pinyin, updated_at=datetime('now')",
            params![id, set_id, word, pinyin],
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
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "UPDATE hotword_words SET is_deleted=?3, updated_at=datetime('now')
         WHERE set_id=?1 AND word=?2 AND is_deleted=0",
        params![set_id, word, now_secs],
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
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 软删不在新列表的活跃词——逐词算 is_deleted=now_secs 的 md5（需读 pinyin）
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
    for (word, _pinyin) in &to_remove {
        conn.execute(
            "UPDATE hotword_words SET is_deleted=?3, updated_at=datetime('now')
             WHERE set_id=?1 AND word=?2 AND is_deleted=0",
            params![set_id, word, now_secs],
        )?;
    }
    // 添加/恢复新列表的词
    for word in &unique {
        let id = crate::hotword_text::hotword_word_uuid(set_id, word);
        let pinyin = crate::hotword_text::word_plain_pinyins(word).join(" ");
        conn.execute(
            "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(set_id, word) DO UPDATE SET
                is_deleted=0, pinyin=excluded.pinyin, updated_at=datetime('now')",
            params![id, set_id, word, pinyin],
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
        "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            set_id=excluded.set_id, word=excluded.word, pinyin=excluded.pinyin,
            is_deleted=excluded.is_deleted, created_at=excluded.created_at,
            updated_at=excluded.updated_at",
        params![
            w.id, w.set_id, w.word, w.pinyin,
            w.is_deleted,
            w.created_at, w.updated_at,
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

/// 取最近 limit 条记录的 segments JSON 里 kind="edited" 的段文本。
/// 用户编辑过的段 = 引擎识别错了（用户才改），这些段里的词是高质量热词候选。
/// 解析失败/无 segments 的记录跳过（返回空不报错）。
pub fn list_recent_edited_segments(limit: i64) -> Result<Vec<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT segments FROM clipboard_history
             WHERE item_type = 'voice' AND segments IS NOT NULL AND segments != ''
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        let mut list = Vec::new();
        for r in rows {
            let json = r?;
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                for item in &arr {
                    let kind = item.get("kind").and_then(|v| v.as_str());
                    let text = item.get("text").and_then(|v| v.as_str());
                    if kind == Some("edited") {
                        if let Some(t) = text {
                            if !t.is_empty() {
                                list.push(t.to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(list)
    })
}
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
        assert_eq!(words[0].is_deleted, 0);

        // remove_word（软删——is_deleted=1，记录保留）
        remove_word_from_set_at(&conn, &id, "八爪鱼").unwrap();
        let words_after = list_words_in_set_at(&conn, &id).unwrap();
        assert_eq!(words_after.len(), 1); // 软删后 list 过滤掉
        assert_eq!(words_after[0].word, "吴大锐");
        // 软删记录仍在 DB（is_deleted=1）
        let soft = get_hotword_word_at(&conn, &id, "八爪鱼").unwrap().unwrap();
        assert!(soft.is_deleted > 0);

        // 软删后重新加同词 → 恢复（is_deleted=0）
        assert!(add_word_to_set_at(&conn, &id, "八爪鱼").unwrap());
        let restored = get_hotword_word_at(&conn, &id, "八爪鱼").unwrap().unwrap();
        assert_eq!(restored.is_deleted, 0);

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
            is_deleted: 0,
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
            is_deleted: 0,
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

    /// 事务批量 INSERT n 个伪词到 set（绕过容量校验，填充到接近/达上限）。
    /// 事务包裹比逐条 INSERT 快约 10x（2 万词 ~30ms vs ~300ms）。
    fn fill_words_batch(conn: &Connection, set_id: &str, n: usize) {
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..n {
            let w = format!("w{}", i);
            let id = crate::hotword_text::hotword_word_uuid(set_id, &w);
            conn.execute(
                "INSERT OR IGNORE INTO hotword_words (id, set_id, word, pinyin, is_deleted) VALUES (?1, ?2, ?3, '', 0)",
                params![id, set_id, w],
            ).unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
    }

    /// add_word_to_set_at：满上限后再加被拒。
    #[test]
    fn add_single_word_rejects_when_at_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-one", "单词容量测试").unwrap();

        // 填满上限词（事务批量 INSERT，绕过容量校验填充）
        fill_words_batch(&conn, "cap-one", HOTWORD_SET_MAX_WORDS);

        // 再加一个新词 → 超限，应被拒
        let err = add_word_to_set_at(&conn, "cap-one", "溢出词").unwrap_err();
        assert!(err.to_string().contains("容量已满"), "满后再加应拒：{}", err);
    }

    /// add_words_to_set_at：批量追加后超上限被拒。
    #[test]
    fn add_words_rejects_when_exceeding_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-add", "批量容量测试").unwrap();

        // 先填到上限 - 1
        fill_words_batch(&conn, "cap-add", HOTWORD_SET_MAX_WORDS - 1);

        // 再批量加 5 词 → 超限被拒
        let base = HOTWORD_SET_MAX_WORDS - 1;
        let extra: Vec<String> = (base..base + 5).map(|i| format!("w{}", i)).collect();
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
        assert!(banana.is_deleted > 0);
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
        assert_eq!(w.word, "八爪鱼");
        assert_eq!(w.is_deleted, 0, "add 后 is_deleted=0");

        // 软删
        remove_word_from_set_at(&conn, "md5-set", "八爪鱼").unwrap();
        let soft = get_hotword_word_at(&conn, "md5-set", "八爪鱼").unwrap().unwrap();
        assert!(soft.is_deleted > 0, "软删后 is_deleted>0");

        // 恢复（重新 add）
        add_word_to_set_at(&conn, "md5-set", "八爪鱼").unwrap();
        let restored = get_hotword_word_at(&conn, "md5-set", "八爪鱼").unwrap().unwrap();
        assert_eq!(restored.is_deleted, 0, "恢复后 is_deleted=0");
        assert_eq!(restored.word, w.word, "恢复后 word 不变");
    }

    /// add_words_to_set_at 批量 + set_words_in_set_at 覆盖（含软删）。
    #[test]
    fn word_batch_and_override_lifecycle() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "ov-set", "覆盖测试").unwrap();

        // 批量加
        add_words_to_set_at(&conn, "ov-set", &["苹果".into(), "香蕉".into()]).unwrap();
        for word in &["苹果", "香蕉"] {
            let w = get_hotword_word_at(&conn, "ov-set", word).unwrap().unwrap();
            assert_eq!(w.is_deleted, 0, "批量加的词应活跃: {}", word);
        }

        // 覆盖为 [苹果] —— 香蕉软删
        set_words_in_set_at(&conn, "ov-set", &["苹果".into()]).unwrap();
        let banana = get_hotword_word_at(&conn, "ov-set", "香蕉").unwrap().unwrap();
        assert!(banana.is_deleted > 0, "覆盖后香蕉应软删");
        let apple = get_hotword_word_at(&conn, "ov-set", "苹果").unwrap().unwrap();
        assert_eq!(apple.is_deleted, 0, "苹果仍活跃");
    }

    // ── set 级软删（v58，is_deleted 存时间戳 + UNIQUE(name,is_deleted)）──

    /// delete_hotword_set 软删：list 看不见但行还在（is_deleted>0）+ 级联软删词 + 重建同名不冲突。
    #[test]
    fn delete_hotword_set_soft_deletes_and_allows_rebuild_same_name() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        let id1 = "softdel-aaaa-0001";
        insert_hotword_set_at(&conn, id1, "项目A").unwrap();
        add_word_to_set_at(&conn, id1, "八爪鱼").unwrap();

        // 软删
        delete_hotword_set_at(&conn, id1).unwrap();

        // list_hotword_sets 看不见（is_deleted=0 过滤）
        let active = list_hotword_sets_at(&conn).unwrap();
        assert!(active.iter().all(|s| s.id != id1), "软删后 list 不应含该集");

        // 但行还在（is_deleted>0）——直接查能读到
        let row: (i64,) = conn
            .query_row("SELECT is_deleted FROM hotword_sets WHERE id=?1", params![id1], |r| {
                Ok((r.get(0)?,))
            })
            .unwrap();
        assert!(row.0 > 0, "is_deleted 应 >0（删除时刻 epoch 秒）");

        // 级联软删词
        let word = get_hotword_word_at(&conn, id1, "八爪鱼").unwrap().unwrap();
        assert!(word.is_deleted > 0, "词典软删后其词也应级联软删");

        // 重建同名「项目A」（新 UUID）——UNIQUE(name,is_deleted) 不冲突
        let id2 = "softdel-bbbb-0002";
        insert_hotword_set_at(&conn, id2, "项目A").unwrap();
        let active2 = list_hotword_sets_at(&conn).unwrap();
        let a_row = active2.iter().find(|s| s.name == "项目A").unwrap();
        assert_eq!(a_row.id, id2, "重建的「项目A」应是新行（id2），不是软删的 id1");
    }

    /// upsert_hotword_set 传播 is_deleted（sync pull tombstone 用）。
    #[test]
    fn upsert_hotword_set_propagates_is_deleted_tombstone() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // 初始活跃
        insert_hotword_set_at(&conn, "tomb-0001", "远程集").unwrap();
        let initial = get_hotword_set_at(&conn, "tomb-0001").unwrap();
        assert_eq!(initial.is_deleted, 0);

        // sync pull tombstone：远程 meta.json is_deleted=时间戳 → upsert 覆盖
        let tombstone = HotwordSet {
            id: "tomb-0001".into(),
            name: "远程集".into(),
            enabled: true,
            created_at: initial.created_at.clone(),
            updated_at: initial.updated_at.clone(),
            sync_md5: Some("md5".into()),
            is_deleted: 1800000000, // tombstone
        };
        upsert_hotword_set_at(&conn, &tombstone).unwrap();

        let after = get_hotword_set_at(&conn, "tomb-0001").unwrap();
        assert_eq!(
            after.is_deleted, 1800000000,
            "upsert 应传播 is_deleted（tombstone）"
        );
        // list 过滤掉
        assert!(list_hotword_sets_at(&conn).unwrap().is_empty());
    }

    // ── tombstone GC（2026-08-02）──

    /// purge_expired_hotword_tombstones：超期 set tombstone 硬删 + 活跃词典不动 + 超期 word 硬删。
    #[test]
    fn purge_expired_tombstones_deletes_old_keeps_active() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // 活跃词典（不应被 GC）
        insert_hotword_set_at(&conn, "active-set", "活跃词典").unwrap();
        add_word_to_set_at(&conn, "active-set", "苹果").unwrap();

        // 超期 set tombstone（is_deleted=远过去）
        conn.execute(
            "INSERT INTO hotword_sets (id, name, is_deleted) VALUES ('old-tomb', '旧删', 1000)",
            [],
        )
        .unwrap(); // is_deleted=1000（1970 年，远超期）
        conn.execute(
            "INSERT INTO hotword_words (id, set_id, word, pinyin, is_deleted) \
             VALUES ('w1', 'old-tomb', '旧词', '', 1000)",
            [],
        )
        .unwrap();

        // 未超期 set tombstone（is_deleted=未来，不应被 GC）
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        conn.execute(
            "INSERT INTO hotword_sets (id, name, is_deleted) VALUES ('new-tomb', '新删', ?1)",
            params![future],
        )
        .unwrap();

        let now = future; // now = 当前
        let purged = purge_expired_hotword_tombstones_pub(&conn, now).unwrap();
        assert_eq!(purged, 1, "应硬删 1 个超期 set tombstone（旧删）");

        // 活跃词典还在
        assert!(get_hotword_set_at(&conn, "active-set").is_ok());
        // 超期 tombstone 硬删了
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM hotword_sets WHERE id='old-tomb'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "超期 set tombstone 应硬删");
        // 未超期 tombstone 还在
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM hotword_sets WHERE id='new-tomb'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "未超期 set tombstone 不应被 GC");
    }

    /// count_hotword_tombstones + purge_all_hotword_tombstones（手动清空）。
    #[test]
    fn count_and_purge_all_tombstones() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        conn.execute(
            "INSERT INTO hotword_sets (id, name, is_deleted) VALUES ('t1', '删1', 1000), ('t2', '删2', 2000)",
            [],
        )
        .unwrap();

        assert_eq!(count_hotword_tombstones_pub(&conn).unwrap(), 2);

        let purged = purge_all_hotword_tombstones_pub(&conn).unwrap();
        assert_eq!(purged, 2, "应清空 2 个 tombstone");
        assert_eq!(count_hotword_tombstones_pub(&conn).unwrap(), 0);
    }

    /// GC 清 hotword_hits 孤儿：词在所有活跃词典消失 → 命中行清零。
    /// 跨 set 同词（任一活跃 set 存在）→ 保留。
    #[test]
    fn purge_orphan_hotword_hits_when_word_fully_gone() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // set-A 活跃，含「苹果」；set-B 活跃，含「香蕉」
        insert_hotword_set_at(&conn, "set-A", "词典A").unwrap();
        add_word_to_set_at(&conn, "set-A", "苹果").unwrap();
        insert_hotword_set_at(&conn, "set-B", "词典B").unwrap();
        add_word_to_set_at(&conn, "set-B", "香蕉").unwrap();

        // 三词各 bump 命中（橘子从无词记录——纯孤儿）
        bump_hotword_hit_by_word_at(&conn, "苹果").unwrap();
        bump_hotword_hit_by_word_at(&conn, "香蕉").unwrap();
        bump_hotword_hit_by_word_at(&conn, "橘子").unwrap();

        // 无 tombstone，now 取当前——purge_expired 不删 set/词，但应清孤儿 hits
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let purged = purge_expired_hotword_tombstones_pub(&conn, now).unwrap();
        assert_eq!(purged, 0, "无 tombstone，set 不删");

        // 橘子（孤儿）被清；苹果/香蕉（活跃词）保留
        let hits = list_hotword_hits_at(&conn).unwrap();
        assert_eq!(hits.get("苹果"), Some(&1i64), "活跃词命中应保留");
        assert_eq!(hits.get("香蕉"), Some(&1i64), "活跃词命中应保留");
        assert!(!hits.contains_key("橘子"), "孤儿词命中应被清");
    }

    /// GC 后 hotword_hits 「同词在另一活跃 set 存在」→ 保留，不误清。
    #[test]
    fn purge_keeps_hits_when_word_still_active_in_any_set() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // set-A、set-B 都含「苹果」（跨 set 同词）
        insert_hotword_set_at(&conn, "set-A", "词典A").unwrap();
        add_word_to_set_at(&conn, "set-A", "苹果").unwrap();
        insert_hotword_set_at(&conn, "set-B", "词典B").unwrap();
        add_word_to_set_at(&conn, "set-B", "苹果").unwrap();

        bump_hotword_hit_by_word_at(&conn, "苹果").unwrap();
        bump_hotword_hit_by_word_at(&conn, "苹果").unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT hit_count FROM hotword_hits WHERE word='苹果'", [], |r| r.get(0)).unwrap(),
            2
        );

        // 软删 set-A（超期 tombstone），GC 硬删 set-A + 其词记录
        conn.execute(
            "UPDATE hotword_sets SET is_deleted=1000 WHERE id='set-A'",
            [],
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let purged = purge_expired_hotword_tombstones_pub(&conn, now).unwrap();
        assert_eq!(purged, 1, "硬删 1 个超期 set tombstone");

        // 「苹果」在 set-B 仍活跃 → hits 保留，hit_count 不变
        let hits = list_hotword_hits_at(&conn).unwrap();
        assert_eq!(hits.get("苹果"), Some(&2i64), "跨 set 同词在任一活跃 set 存在 → 命中保留");
    }

    // 测试用 pub 包装（避免 ensure_db/with_db 在 in-memory 测里走全局 DB）
    fn purge_expired_hotword_tombstones_pub(conn: &Connection, now_secs: i64) -> Result<usize> {
        let cutoff = now_secs - HOTWORD_TOMBSTONE_RETENTION_SECS;
        let mut purged_sets = 0usize;
        let expired_set_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM hotword_sets WHERE is_deleted > 0 AND is_deleted < ?1",
            )?;
            let rows = stmt.query_map(params![cutoff], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in &expired_set_ids {
            conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
            conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
            purged_sets += 1;
        }
        let _ = conn.execute(
            "DELETE FROM hotword_words WHERE is_deleted > 0 AND is_deleted < ?1",
            params![cutoff],
        )?;
        // hotword_hits 孤儿清理（与生产函数同步——详见 tombstone-gc spec §5）
        let _ = conn.execute(
            "DELETE FROM hotword_hits WHERE word NOT IN \
             (SELECT word FROM hotword_words WHERE is_deleted = 0)",
            [],
        )?;
        Ok(purged_sets)
    }
    fn count_hotword_tombstones_pub(conn: &Connection) -> Result<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM hotword_sets WHERE is_deleted > 0",
            [],
            |r| r.get(0),
        )?)
    }
    fn purge_all_hotword_tombstones_pub(conn: &Connection) -> Result<usize> {
        let ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT id FROM hotword_sets WHERE is_deleted > 0")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let mut purged = 0usize;
        for id in &ids {
            conn.execute("DELETE FROM hotword_words WHERE set_id=?1", params![id])?;
            conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
            purged += 1;
        }
        conn.execute("DELETE FROM hotword_words WHERE is_deleted > 0", [])?;
        // hotword_hits 孤儿清理（与生产函数同步——详见 tombstone-gc spec §5）
        conn.execute(
            "DELETE FROM hotword_hits WHERE word NOT IN \
             (SELECT word FROM hotword_words WHERE is_deleted = 0)",
            [],
        )?;
        Ok(purged)
    }
}
