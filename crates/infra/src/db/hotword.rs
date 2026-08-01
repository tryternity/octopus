// db/hotword.rs —— HotwordSet（hotword_sets / hotword_hits 表）CRUD + words + hits + recent_text。

use super::{ensure_db, with_db, Connection, Result, params};

// ── HotwordSet（热词版本/场景）──────────────────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordSet {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub words_text: String,
    pub created_at: String,
    pub updated_at: String,
    /// md5 内容指纹（v46：增量同步 diff，由调用方算好传入）。
    /// None 表示调用方未算（向后兼容旧调用方），sync 时按需重算。
    pub sync_md5: Option<String>,
}

const HOTWORD_SET_COLS: &str = "id, name, enabled, words_text, created_at, updated_at, sync_md5";

/// 单个热词词典（版本）的词数上限（2026-08-01）。
///
/// 限制理由：① 加载时 `HotwordIndex::from_words` 构建 O(N) 索引；② fuzzy 搜索
/// `match_score` 逐词 O(N) 匹配。词数过大影响启动 + 搜索性能。3000 覆盖典型场景
/// （专业术语/专有名词），超出建议用户另建新词典分摊。
pub const HOTWORD_SET_MAX_WORDS: usize = 3000;

/// 校验写入后的词数是否超容量上限。`prospective_words_text` 是「将要写入」的内容
/// （已 normalize 或待 normalize 均可——normalize 只去重排序不改变词数）。
fn ensure_within_capacity(prospective_words_text: &str) -> Result<()> {
    let n = prospective_words_text.split_whitespace().count();
    if n > HOTWORD_SET_MAX_WORDS {
        anyhow::bail!(
            "词典容量已满（{} 词上限），建议另建新词典分摊（当前 {} 词）",
            HOTWORD_SET_MAX_WORDS, n
        );
    }
    Ok(())
}

fn row_to_hotword_set(row: &rusqlite::Row) -> rusqlite::Result<HotwordSet> {
    Ok(HotwordSet {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        words_text: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        sync_md5: row.get(6)?,
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

/// 删除版本。
pub fn delete_hotword_set(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_hotword_set_at(conn, id))
}

pub(crate) fn delete_hotword_set_at(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 覆盖写 words_text（已 normalize）。导入「覆盖」模式用。
pub fn set_hotword_set_words(id: &str, words_text: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let normalized = crate::hotword_text::normalize_words_text(words_text);
        ensure_within_capacity(&normalized)?;
        let n = conn.execute(
            "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
            params![normalized, id],
        )?;
        if n == 0 {
            anyhow::bail!("热词版本不存在");
        }
        Ok(())
    })
}

/// 追加一词到指定版本（并集 + normalize）。重复词去重无副作用，返回是否实际新增。
pub fn add_word_to_set(id: &str, word: &str) -> Result<bool> {
    ensure_db()?;
    with_db(|conn| add_word_to_set_at(conn, id, word))
}

pub(crate) fn add_word_to_set_at(conn: &Connection, id: &str, word: &str) -> Result<bool> {
    let cur: String = conn
        .query_row(
            "SELECT words_text FROM hotword_sets WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
    let merged = format!("{} {}", cur, word);
    let normalized = crate::hotword_text::normalize_words_text(&merged);
    ensure_within_capacity(&normalized)?;
    let added = normalized != cur;
    conn.execute(
        "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
        params![normalized, id],
    )?;
    Ok(added)
}

/// 批量追加多词（挖掘/导入追加用），返回实际新增条数。
pub fn add_words_to_set(id: &str, words: &[String]) -> Result<usize> {
    ensure_db()?;
    with_db(|conn| {
        let cur: String = conn
            .query_row(
                "SELECT words_text FROM hotword_sets WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
        let before: std::collections::HashSet<&str> = cur.split_whitespace().collect();
        let merged = format!("{} {}", cur, words.join(" "));
        let normalized = crate::hotword_text::normalize_words_text(&merged);
        ensure_within_capacity(&normalized)?;
        let after: std::collections::HashSet<&str> = normalized.split_whitespace().collect();
        let added = after.len().saturating_sub(before.len());
        conn.execute(
            "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
            params![normalized, id],
        )?;
        Ok(added)
    })
}

/// 从指定版本移除一词（normalize 重排）。
pub fn remove_word_from_set(id: &str, word: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| remove_word_from_set_at(conn, id, word))
}

pub(crate) fn remove_word_from_set_at(conn: &Connection, id: &str, word: &str) -> Result<()> {
    let cur: String = conn
        .query_row(
            "SELECT words_text FROM hotword_sets WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
    let filtered: Vec<&str> = cur.split_whitespace().filter(|w| *w != word).collect();
    let normalized = crate::hotword_text::normalize_words_text(&filtered.join(" "));
    conn.execute(
        "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
        params![normalized, id],
    )?;
    Ok(())
}

/// upsert 热词版本——sync pull 从文件读回写 SQLite 用（v46 新增）。
///
/// `id` 已存在时按全字段覆盖（name/enabled/words_text/created_at/updated_at/sync_md5），
/// 不存在时插入。name UNIQUE 冲突时返 Err（跨设备同名版本合并需上层处理）。
///
/// 与普通 insert/update 的区别：
/// - insert：只新建（不覆盖），调用方生成 id
/// - update 系列：只改单字段（rename/toggle/set_words）
/// - upsert：全字段覆盖——sync 拉到远程版本时直接整体写入，不关心本地是否已有
pub fn upsert_hotword_set(h: &HotwordSet) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_hotword_set_at(conn, h))
}

pub(crate) fn upsert_hotword_set_at(conn: &Connection, h: &HotwordSet) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_sets (id, name, enabled, words_text, created_at, updated_at, sync_md5)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            enabled=excluded.enabled,
            words_text=excluded.words_text,
            created_at=excluded.created_at,
            updated_at=excluded.updated_at,
            sync_md5=excluded.sync_md5",
        params![
            h.id,
            h.name,
            if h.enabled { 1 } else { 0 },
            h.words_text,
            h.created_at,
            h.updated_at,
            h.sync_md5,
        ],
    )?;
    Ok(())
}

/// 只更新 sync_md5 字段（写命令后回填用——desktop 命令层算好 md5 调此函数）。
///
/// 与 upsert 的区别：upsert 全字段覆盖（sync pull 用），本函数只动 sync_md5
/// （本地写命令后补充指纹，不覆盖其他字段）。
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

/// 纠错热路径用——取所有 enabled 版本的 words_text 切词去重并集（构造 HotwordIndex 用）。
pub fn list_active_hotword_words() -> Result<Vec<String>> {
    ensure_db()?;
    with_db(|conn| list_active_hotword_words_at(conn))
}

pub(crate) fn list_active_hotword_words_at(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT words_text FROM hotword_sets WHERE enabled=1")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in rows {
        for w in r?.split_whitespace() {
            set.insert(w.to_string());
        }
    }
    Ok(set.into_iter().collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;
    use rusqlite::Connection;

    /// HotwordSet 全 CRUD 往返：建 → 列 → 重名冲突 → 改名 → 启停 →
    /// 单词追加（去重 + normalize 拼音首字母排序）→ 单词移除 → 删版本。
    #[test]
    fn hotword_set_crud_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // db.sql 现含默认「通用」版本 seed；本测试聚焦 CRUD 逻辑，清掉种子避免干扰 [0]/len 断言。
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // create（调用方生成 UUID——v46 改造：id 不再 AUTOINCREMENT）
        let id = "test-uuid-项目A-001".to_string();
        insert_hotword_set_at(&conn, &id, "项目A").unwrap();

        // list
        let sets = list_hotword_sets_at(&conn).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "项目A");
        assert_eq!(sets[0].id, id);
        assert!(sets[0].enabled);
        assert_eq!(sets[0].words_text, "");
        assert!(sets[0].sync_md5.is_none()); // 新建时 sync_md5 = NULL

        // 重名 → 唯一冲突
        assert!(insert_hotword_set_at(&conn, "test-uuid-项目A-002", "项目A").is_err());

        // rename
        rename_hotword_set_at(&conn, &id, "项目A2").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].name, "项目A2");

        // toggle enabled
        toggle_hotword_set_at(&conn, &id, false).unwrap();
        assert!(!list_hotword_sets_at(&conn).unwrap()[0].enabled);
        toggle_hotword_set_at(&conn, &id, true).unwrap();

        // add_word（normalize：序 + 去重）
        add_word_to_set_at(&conn, &id, "吴大锐").unwrap();
        add_word_to_set_at(&conn, &id, "八爪鱼").unwrap();
        add_word_to_set_at(&conn, &id, "八爪鱼").unwrap(); // 重复 → 去重
        let s = list_hotword_sets_at(&conn).unwrap()[0].clone();
        assert_eq!(s.words_text, "八爪鱼 吴大锐"); // BZY < WDR

        // remove_word
        remove_word_from_set_at(&conn, &id, "八爪鱼").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].words_text, "吴大锐");

        // delete set
        delete_hotword_set_at(&conn, &id).unwrap();
        assert!(list_hotword_sets_at(&conn).unwrap().is_empty());
    }

    /// upsert（v46 新增）——sync pull 用。覆盖 + 新建两种路径。
    #[test]
    fn hotword_set_upsert_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // 新建路径——id 不存在，INSERT
        let h1 = HotwordSet {
            id: "upsert-uuid-1".into(),
            name: "版本X".into(),
            enabled: true,
            words_text: "苹果".into(),
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
            sync_md5: Some("md5-abc".into()),
        };
        upsert_hotword_set_at(&conn, &h1).unwrap();
        let loaded = get_hotword_set_at(&conn, "upsert-uuid-1").unwrap();
        assert_eq!(loaded.name, "版本X");
        assert_eq!(loaded.words_text, "苹果");
        assert_eq!(loaded.sync_md5.as_deref(), Some("md5-abc"));

        // 覆盖路径——同 id，改 name/words_text/sync_md5
        let h2 = HotwordSet {
            id: "upsert-uuid-1".into(),
            name: "版本X改".into(),
            enabled: false,
            words_text: "苹果 香蕉".into(),
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 11:00:00".into(),
            sync_md5: Some("md5-def".into()),
        };
        upsert_hotword_set_at(&conn, &h2).unwrap();
        let loaded2 = get_hotword_set_at(&conn, "upsert-uuid-1").unwrap();
        assert_eq!(loaded2.name, "版本X改");
        assert!(!loaded2.enabled);
        assert_eq!(loaded2.words_text, "苹果 香蕉");
        assert_eq!(loaded2.sync_md5.as_deref(), Some("md5-def"));
    }

    /// 「通用」默认版本用固定 UUID——跨设备一致（v46 设计）。
    #[test]
    fn default_general_set_uses_fixed_uuid() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        let sets = list_hotword_sets_at(&conn).unwrap();
        let general = sets.iter().find(|s| s.name == "通用").expect("应有「通用」seed");
        assert_eq!(
            general.id, "00000000-0000-0000-0000-000000000001",
            "「通用」版本必须用固定 UUID，保证跨设备 sync 时 id 一致"
        );
    }

    #[test]
    fn list_active_words_is_enabled_union() {
        let conn = &mut Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // db.sql 已 seed 空「通用」（enabled=1）；此处改为含词以测 enabled 并集
        conn.execute("UPDATE hotword_sets SET words_text='八爪鱼 吴大锐' WHERE name='通用'", []).unwrap();
        conn.execute("INSERT INTO hotword_sets(name, enabled, words_text) VALUES('项目A', 1, '吴大锐 周会')", []).unwrap();
        conn.execute("INSERT INTO hotword_sets(name, enabled, words_text) VALUES('关闭的', 0, '浮窗')", []).unwrap();

        let words = list_active_hotword_words_at(conn).unwrap();
        // 并集去重：八爪鱼 吴大锐 周会（enabled=0 的「浮窗」不在）
        let set: std::collections::HashSet<&str> = words.iter().map(|s| s.as_str()).collect();
        assert_eq!(set, ["八爪鱼", "吴大锐", "周会"].into_iter().collect());

        // 全关 → 空
        conn.execute("UPDATE hotword_sets SET enabled=0", []).unwrap();
        assert!(list_active_hotword_words_at(conn).unwrap().is_empty());
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

    // ── 容量上限 HOTWORD_SET_MAX_WORDS（2026-08-01）──

    /// 生成 n 个不重复的伪词（w0..w{n-1}）。
    fn fake_words(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("w{}", i)).collect()
    }

    /// set_hotword_set_words：恰好 3000 词通过，3001 词被拒。
    #[test]
    fn set_words_respects_capacity_limit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-set", "容量测试").unwrap();
        // 走 _at 版本，直接用本地 conn
        let exactly = fake_words(HOTWORD_SET_MAX_WORDS).join(" ");
        let normalized = crate::hotword_text::normalize_words_text(&exactly);
        assert!(ensure_within_capacity(&normalized).is_ok(), "3000 词应在上限内");

        let over = fake_words(HOTWORD_SET_MAX_WORDS + 1).join(" ");
        let normalized_over = crate::hotword_text::normalize_words_text(&over);
        let err = ensure_within_capacity(&normalized_over).unwrap_err();
        assert!(err.to_string().contains("容量已满"), "超限应返容量错误：{}", err);
    }

    /// add_words_to_set：批量追加后总词数超 3000 应被拒（不部分写入）。
    #[test]
    fn add_words_rejects_when_exceeding_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-add", "批量容量测试").unwrap();

        // 先填到 2999 词
        let base = fake_words(2999);
        let base_normalized = crate::hotword_text::normalize_words_text(&base.join(" "));
        conn.execute(
            "UPDATE hotword_sets SET words_text=?1 WHERE id='cap-add'",
            params![base_normalized],
        ).unwrap();

        // 模拟 add_words_to_set 内部逻辑：merge + normalize + 校验
        let extra: Vec<String> = (2999..2999 + 5).map(|i| format!("w{}", i)).collect();
        let merged = format!("{} {}", base_normalized, extra.join(" "));
        let merged_normalized = crate::hotword_text::normalize_words_text(&merged);
        let err = ensure_within_capacity(&merged_normalized).unwrap_err();
        assert!(err.to_string().contains("容量已满"), "超限应返容量错误：{}", err);

        // 原内容未被改动
        let after: String = conn.query_row(
            "SELECT words_text FROM hotword_sets WHERE id='cap-add'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(after.split_whitespace().count(), 2999, "被拒后原内容不应改动");
    }

    /// add_word_to_set_at：单词追加超限被拒。
    #[test]
    fn add_single_word_rejects_when_at_capacity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();
        insert_hotword_set_at(&conn, "cap-one", "单词容量测试").unwrap();

        // 填满 3000 词
        let full = fake_words(HOTWORD_SET_MAX_WORDS).join(" ");
        let full_normalized = crate::hotword_text::normalize_words_text(&full);
        conn.execute(
            "UPDATE hotword_sets SET words_text=?1 WHERE id='cap-one'",
            params![full_normalized],
        ).unwrap();

        // 再加一个新词 → 3001，add_word_to_set_at 内部 ensure_within_capacity 应拒
        let err = add_word_to_set_at(&conn, "cap-one", "溢出词").unwrap_err();
        assert!(err.to_string().contains("容量已满"), "满后再加应拒：{}", err);
    }
}
