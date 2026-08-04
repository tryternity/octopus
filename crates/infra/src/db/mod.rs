// crates/infra/src/db/mod.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。
//
// 按表域拆为子模块（每个子文件一组表的 CRUD + 对应测试）：
mod action_bar;
mod agent;
mod config;
mod hotword;
mod models;
mod prompts;
mod transcription;
mod vault;

pub use action_bar::*;
pub use agent::*;
pub use config::*;
pub use hotword::*;
pub use models::*;
pub use prompts::*;
pub use transcription::*;
pub use vault::*;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicBool, Ordering};

/// 收集 query_map 结果，遇到失败行时 log::warn 并跳过（而非静默丢弃）。
/// 替代 `.filter_map(|r| r.ok()).collect()`——后者吞掉所有错误，
/// 模型加载/历史搜索中损坏行会被静默忽略，难以排查。
pub(crate) fn collect_rows<T, E: std::fmt::Display>(
    rows: impl Iterator<Item = Result<T, E>>,
    context: &str,
) -> Vec<T> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(v) => out.push(v),
            Err(e) => log::warn!("DB row skip ({}): {}", context, e),
        }
    }
    out
}
use std::collections::HashMap;
use std::sync::OnceLock;

/// 测试模式标志：设为 true 后 [`ensure_db`] 使用 in-memory SQLite，
/// 不打开也不迁移 `~/.octopus/octopus.db`。由 [`init_test_db`] 设置。
///
/// 跨 crate 可见（不依赖 `#[cfg(test)]`——`cfg(test)` 不传递到依赖 crate），
/// 供 octopus-search / octopus-desktop 等下游 crate 的测试使用。
static TEST_MODE: AtomicBool = AtomicBool::new(false);

static DB: OnceLock<parking_lot::ReentrantMutex<Connection>> = OnceLock::new();

// 测试专用：thread-local 的 DB 连接覆盖。
//
// 与 `init_test_db()`（进程级 in-memory，OnceLock 单连接跨测试共享）互补：
// 本覆盖让**每个测试线程**装一份独立的 in-memory 连接，避免测试间数据互相污染。
//
// 不用 `#[cfg(test)]` 门控——与 `init_test_db()` 同样保持运行时可见，便于跨 crate
// （octopus-vault / octopus-desktop）的单元测试调用。未设置时 with_db 走全局
// OnceLock 路径，对生产无影响（仅多一次 thread_local 读）。
thread_local! {
    static TEST_DB_OVERRIDE: std::cell::RefCell<
        Option<std::sync::Arc<parking_lot::ReentrantMutex<Connection>>>
    > = std::cell::RefCell::new(None);
}

/// 测试专用：注入一个 in-memory 连接（建表 + 标 v40），后续 `with_db` 调用会使用它。
///
/// 调用方需自备 `rusqlite::Connection::open_in_memory()`——这样测试可控制是否 preload
/// 数据。多次调用替换前一次注入的连接（不累积）。
#[doc(hidden)]
pub fn set_test_db(conn: Connection) {
    // 与 ensure_db → open_db_conn → init_schema 的初始化路径保持一致：
    // 1. 设置 PRAGMA（WAL/busy_timeout/foreign_keys）
    // 2. 跑 INIT_SQL 建表 + seed（IF NOT EXISTS 幂等）
    // 3. 直接标 v40（跳过迁移分支）
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;",
    )
    .expect("set_test_db: set PRAGMA");
    conn.execute_batch(INIT_SQL).expect("set_test_db: INIT_SQL");
    conn.execute("PRAGMA user_version = 46", [])
        .expect("set_test_db: set user_version");
    TEST_DB_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(std::sync::Arc::new(parking_lot::ReentrantMutex::new(conn)));
    });
}

/// 测试专用：清除 thread-local DB 覆盖，恢复全局 OnceLock 路径。
#[doc(hidden)]
pub fn clear_test_db() {
    TEST_DB_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// 编译期嵌入的建表 + seed SQL（来自 crates/infra/src/db.sql）
const INIT_SQL: &str = include_str!("../db.sql");

/// DB 文件路径：~/.octopus/octopus.db
///
/// **测试/CI 可覆盖**：设置环境变量 `OCTOPUS_DB_PATH` 可重定向到任意路径；
/// 特殊值 `:memory:` 使用 in-memory SQLite（不落盘）。未设置时走默认开发库。
fn db_path() -> std::path::PathBuf {
    crate::paths::octopus_config_home().join("octopus.db")
}

/// 测试初始化：强制后续 [`ensure_db`] 使用 in-memory SQLite，彻底隔离开发库。
///
/// **必须在任何 `ensure_db` / `with_db` 调用之前调用**（通常在 test mod 顶部
/// `std::sync::Once::call_once` 中），否则若 `ensure_db` 已被并发触发并打开了
/// 文件 DB，本调用不再生效。幂等——多次调用无副作用。
///
/// 设计动机：`#[cfg(test)]` 不跨 crate 传递，下游 crate（search/desktop）的测试
/// 无法通过编译期 cfg 触发 infra 的 test 分支，改用运行时 flag。
#[doc(hidden)]
pub fn init_test_db() {
    TEST_MODE.store(true, Ordering::SeqCst);
}

/// 幂等初始化：打开/创建 DB，以 db.sql 为准建表（开发期简化，无历史迁移链）。
///
/// **三种模式**（优先级递减）：
/// 1. `TEST_MODE`（[`init_test_db`] 设置）→ in-memory，不碰文件系统
/// 2. `OCTOPUS_DB_PATH` 环境变量 → 指定路径（`:memory:` 同样 in-memory）
/// 3. 默认 → `~/.octopus/octopus.db`
///
/// **测试覆盖感知**（2026-08-01）：若当前线程已通过 [`set_test_db`] 注入 thread_local
/// 连接，直接返回 Ok——[`with_db`] 会用 thread_local 连接，不碰全局 DB。这避免了
/// `ensure_db` 打开真实 `~/.octopus/octopus.db` 破坏测试隔离（如真实 DB 版本不匹配
/// 触发 init_schema bail）。生产环境 thread_local 恒为 None，此分支不生效。
pub fn ensure_db() -> Result<()> {
    // 测试覆盖：当前线程已注入 test DB → with_db 会用它，跳过全局初始化
    let has_override = TEST_DB_OVERRIDE.with(|cell| cell.borrow().is_some());
    if has_override {
        return Ok(());
    }
    if DB.get().is_some() {
        return Ok(());
    }
    let conn = open_db_conn()?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;",
    )
    .context("set WAL + busy_timeout")?;
    init_schema(&conn)?;
    // builtin seed 幂等兜底：即使 schema 已是 v48（如迁移时代码旧未注入 builtin 行），
    // 每次启动 INSERT OR IGNORE 确保兜底引擎行存在。详见 spec 2026-07-22-builtin-models.md §3。
    ensure_builtin_seed(&conn)?;
    let _ = DB.set(parking_lot::ReentrantMutex::new(conn));
    Ok(())
}

/// 幂等注入 builtin 兜底引擎 seed 行 + 填充 manifest。
///
/// migrate_v47_to_v48 在迁移时已调，但为防止「迁移时代码不完整导致漏注入」的历史库，
/// ensure_db 每次启动都跑（INSERT OR IGNORE 幂等，UNIQUE 约束保证不重复）。
/// v48+ 库仅多一次轻量 INSERT 判定 + 可能的 manifest UPDATE，开销可忽略。
fn ensure_builtin_seed(conn: &Connection) -> Result<()> {
    let has_models = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='models'")?
        .exists([])?;
    if !has_models {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, source_type, is_available, is_streaming)
         VALUES ('asr','local','zipformer','zipformer-small','asr/zipformer-small','zh',
                 'zipformer-small 兜底引擎（27M，内置，首次启动下载）',0,0,1)",
        [],
    )?;
    // 若 builtin 行 secret_key 为空，填 manifest（首次或迁移漏填时）
    let needs_manifest: bool = conn
        .query_row(
            "SELECT secret_key = '' OR secret_key IS NULL FROM models WHERE model_name='zipformer-small'",
            [], |r| r.get::<_, i32>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false);
    if needs_manifest {
        fill_manifests(conn)?;
    }
    Ok(())
}

/// 实际打开 DB 连接——三种模式分支（见 [`ensure_db`] 文档）。
fn open_db_conn() -> Result<Connection> {
    // 1. 测试模式：in-memory
    if TEST_MODE.load(Ordering::SeqCst) {
        return Connection::open_in_memory().context("Failed to open in-memory test DB");
    }
    // 2. env var 覆盖（支持 ":memory:"）
    if let Ok(p) = std::env::var("OCTOPUS_DB_PATH") {
        if p == ":memory:" {
            return Connection::open_in_memory().context("Failed to open in-memory DB (OCTOPUS_DB_PATH)");
        }
        let path = std::path::PathBuf::from(p);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        return Connection::open(&path)
            .with_context(|| format!("Failed to open DB at {} (OCTOPUS_DB_PATH)", path.display()));
    }
    // 3. 默认：~/.octopus/octopus.db
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    Connection::open(&path)
        .with_context(|| format!("Failed to open DB at {}", path.display()))
}

/// 取 DB 锁执行闭包（未初始化时自动 ensure_db）。
///
/// 锁为 `ReentrantMutex`，支持**同线程重入**：闭包内可安全地再调 `with_db`
/// （或经多层间接调用触及，如 load_app_config / 模型 meta）。历史 `Mutex`（非递归）
/// 在此场景会永久死锁，见 memory with-db-reentrant-deadlock。
pub fn with_db<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&Connection) -> Result<R>,
{
    // 测试覆盖路径：若当前线程已注入 test DB 连接，优先用它（不走全局 OnceLock）。
    // 让每个测试线程隔离数据，避免相互污染。生产环境 thread_local 恒为 None，
    // 仅多一次 TLS 读，无实质开销。
    let override_conn: Option<std::sync::Arc<parking_lot::ReentrantMutex<Connection>>> =
        TEST_DB_OVERRIDE.with(|cell| cell.borrow().clone());
    if let Some(mutex) = override_conn {
        let guard = mutex.lock();
        return f(&guard);
    }
    if DB.get().is_none() {
        ensure_db()?;
    }
    let mutex = DB.get().context("DB not initialized")?;
    let conn = mutex.lock();
    f(&conn)
}


/// 初始化 schema：db.sql 是唯一表结构真相，全新库直接跑 db.sql 建表。
///
/// **设计**（2026-07-27 重构）：删除所有历史 migration 代码——单用户开发库，每次
/// schema 变更直接改 db.sql + 升 `user_version`，旧库一律清库重建（`rm ~/.octopus/octopus.db*`）。
///
/// 分支：
/// - `v == 0`：全新库——db.sql 建表 + 外置 seed + yaml 迁移 + manifest 填充 → v55
/// - `v == 55`：最新，no-op
/// - `v == 54`：数据迁移——asr_correct 强制翻 true（热词纠错开关，2026-08-01）
/// - `v != 0 && v < 54`：旧版本库——不支持自动迁移，bail 提示清库
///
/// schema 变更流程：改 db.sql + 升 `user_version`（init_schema 末尾 + db.sql 注释）。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v == CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    if v == 0 {
        // 全新库：db.sql 建表 + seed + manifest + 外置 seed + yaml 配置迁移
        conn.execute_batch(INIT_SQL).context("执行 db.sql 建表 + seed")?;
        migrate_yaml_to_db(conn)?; // config.yaml 存在时一次性导入（导入后重命名 .bak），否则幂等返回
        fill_manifests(conn)?;
        crate::seeds::load_external_seeds(conn)?;
        conn.execute(
            &format!("PRAGMA user_version = {}", CURRENT_SCHEMA_VERSION),
            [],
        )?;
        log::info!(
            "DB initialized (v{}): schema + external seeds + manifest fill + yaml 配置导入（无 yaml 则跳过）",
            CURRENT_SCHEMA_VERSION
        );
        return Ok(());
    }

    // 数据迁移链：v54→v55→v56... 每个 migration 升 1 版本，循环到 CURRENT。
    // 1 <= v < 54 的旧库 bail（不支持表结构迁移，但纯数据迁移 v54+ 可保留）。
    let mut cur = v;
    while cur < CURRENT_SCHEMA_VERSION {
        match cur {
            54 => {
                // v54→v55：asr_correct 强制翻 true（热词纠错开关，存量用户加了热词不生效）
                conn.execute(
                    "UPDATE app_config SET config_value = 'true' WHERE config_key = 'asr_correct'",
                    [],
                )
                .context("迁移 v54→v55：asr_correct 翻 true")?;
                log::info!("DB migrated v54→v55: asr_correct 翻 true");
            }
            55 => {
                // v55→v56：方言规则 DB 化——建 fuzzy_dialect_rules 表 + seed，
                // 并把旧 app_config.fuzzy_dialect 字符串的开关状态迁移到表的 enabled 字段。
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS fuzzy_dialect_rules (
                        token TEXT PRIMARY KEY, label TEXT NOT NULL,
                        from_py TEXT NOT NULL, to_py TEXT NOT NULL,
                        match_type TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 0,
                        sort_order INTEGER NOT NULL DEFAULT 0
                    );
                    INSERT OR IGNORE INTO fuzzy_dialect_rules (token, label, from_py, to_py, match_type, enabled, sort_order) VALUES
                        ('fei/hui', 'fei/hui（飞 / 回）', 'fei', 'hui', 'syllable', 0, 1),
                        ('yun/yong', 'yun/yong（孕 / 用）', 'yun', 'yong', 'syllable', 0, 2),
                        ('si/ci', 'si/ci（四 / 词）', 'si', 'ci', 'syllable', 0, 3),
                        ('n/l', 'n/l（刘 / 牛）', 'n', 'l', 'initial', 0, 1),
                        ('f/h', 'f/h（浮 / 护）', 'f', 'h', 'initial', 0, 2),
                        ('r/l', 'r/l（热 / 乐）', 'r', 'l', 'initial', 0, 3),
                        ('hu/wu', 'hu/wu（胡 / 吴）', 'hu', 'w', 'special_hu', 0, 1);",
                )
                .context("迁移 v55→v56：建 fuzzy_dialect_rules 表 + seed")?;
                // 旧 fuzzy_dialect 字符串 → 表 enabled 迁移
                if let Ok(old_dialect) = conn.query_row::<String, _, _>(
                    "SELECT config_value FROM app_config WHERE config_key='fuzzy_dialect'",
                    [], |r| r.get(0),
                ) {
                    for tok in old_dialect.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()) {
                        let _ = conn.execute(
                            "UPDATE fuzzy_dialect_rules SET enabled=1 WHERE token=?1",
                            params![tok],
                        );
                    }
                    log::info!("DB migrated v55→v56: fuzzy_dialect_rules 表建立，旧开关 '{}' 已迁移", old_dialect);
                } else {
                    log::info!("DB migrated v55→v56: fuzzy_dialect_rules 表建立（无旧 fuzzy_dialect 配置）");
                }
            }
            56 => {
                // v56→v57：热词拆分单记录——建 hotword_words 表，把 hotword_sets.words_text
                // 拆成每词一条记录（确定性 UUID v5 + 原始拼音），然后 DROP words_text 列。
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS hotword_words (
                        id          TEXT    PRIMARY KEY,
                        set_id      TEXT    NOT NULL,
                        word        TEXT    NOT NULL,
                        pinyin      TEXT    NOT NULL DEFAULT '',
                        is_deleted  INTEGER NOT NULL DEFAULT 0,
                        created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
                        updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
                        UNIQUE(set_id, word)
                    );
                    CREATE INDEX IF NOT EXISTS idx_hotword_words_set ON hotword_words(set_id);",
                )
                .context("迁移 v56→v57：建 hotword_words 表")?;
                // 把现有 words_text 拆成词记录（仅当列存在——全新库 v57 已无此列）
                let has_words_text: bool = {
                    let mut stmt = conn.prepare("PRAGMA table_info(hotword_sets)")?;
                    let cols: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(1))?
                        .filter_map(|r| r.ok()).collect();
                    cols.iter().any(|c| c == "words_text")
                };
                if has_words_text {
                    let sets: Vec<(String, String)> = {
                        let mut stmt = conn.prepare("SELECT id, words_text FROM hotword_sets")?;
                        let rows = stmt.query_map([], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })?;
                        let mut out = Vec::new();
                        for r in rows {
                            out.push(r?);
                        }
                        out
                    };
                    use rusqlite::params;
                    let mut total_words = 0;
                    for (set_id, words_text) in &sets {
                        for word in words_text.split_whitespace() {
                            let id = crate::hotword_text::hotword_word_uuid(set_id, word);
                            let pinyin = crate::hotword_text::word_plain_pinyins(word).join(" ");
                            let _ = conn.execute(
                                "INSERT OR IGNORE INTO hotword_words (id, set_id, word, pinyin, is_deleted)
                                 VALUES (?1, ?2, ?3, ?4, 0)",
                                params![id, set_id, word, pinyin],
                            );
                            total_words += 1;
                        }
                    }
                    conn.execute("ALTER TABLE hotword_sets DROP COLUMN words_text", [])
                        .context("迁移 v56→v57：DROP words_text 列")?;
                    log::info!(
                        "DB migrated v56→v57: hotword_words 表建立，{} 个 set 共 {} 词迁移完成，words_text 列已删",
                        sets.len(), total_words
                    );
                } else {
                    log::info!("DB migrated v56→v57: hotword_words 表建立（无 words_text 列，跳过迁移）");
                }
            }
            57 => {
                // v57→v58：hotword_sets 加 set 级软删——is_deleted 存删除时刻 epoch 秒
                // （0=活跃，>0=删除时刻）+ UNIQUE(name, is_deleted) 复合约束。
                // SQLite 不支持 ALTER TABLE 改约束（name 单列 UNIQUE → name+is_deleted 复合），
                // 用建表复制法：建新表（含 is_deleted + 复合 UNIQUE）→ 复制数据 → DROP 旧 → RENAME。
                // 现有行 is_deleted=0（活跃），复制时显式填 0。
                //
                // 第十一轮 P2：包 unchecked_transaction。execute_batch 在 autocommit 模式下逐条
                // 自动提交，DROP TABLE 与 RENAME 之间崩溃（断电/kill -9）→ 旧表已删 + _new 残留 →
                // 重启 CREATE TABLE hotword_sets_new（非 IF NOT EXISTS）报 table already exists →
                // 迁移 fail → ensure_db 持续 Err → 应用无法启动，DB 不可恢复。事务保证 4 条 DDL
                // 原子（全成功或全回滚），对齐 insert_vault_ciphers_batch（vault.rs:303）范式。
                let tx = conn.unchecked_transaction()?;
                tx.execute_batch(
                    "CREATE TABLE hotword_sets_new (
                        id          TEXT    PRIMARY KEY,
                        name        TEXT    NOT NULL,
                        enabled     INTEGER NOT NULL DEFAULT 1,
                        sync_md5    TEXT,
                        created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
                        updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
                        is_deleted  INTEGER NOT NULL DEFAULT 0,
                        UNIQUE(name, is_deleted)
                    );
                    INSERT INTO hotword_sets_new (id, name, enabled, sync_md5, created_at, updated_at, is_deleted)
                    SELECT id, name, enabled, sync_md5, created_at, updated_at, 0 FROM hotword_sets;
                    DROP TABLE hotword_sets;
                    ALTER TABLE hotword_sets_new RENAME TO hotword_sets;",
                )
                .context("迁移 v57→v58：hotword_sets 加 is_deleted + UNIQUE(name,is_deleted)")?;
                tx.commit()?;
                log::info!("DB migrated v57→v58: hotword_sets 加 set 级软删（is_deleted 时间戳 + 复合 UNIQUE）");
            }
            _ => {
                anyhow::bail!(
                    "DB schema version {} is outdated (current {}). \
                     This build no longer supports auto-migration. \
                     Run: rm ~/.octopus/octopus.db* (then restart app to rebuild from db.sql).",
                    cur, CURRENT_SCHEMA_VERSION
                );
            }
        }
        cur += 1;
        conn.execute(&format!("PRAGMA user_version = {}", cur), [])?;
    }
    if v != CURRENT_SCHEMA_VERSION {
        log::info!("DB migrated v{}→v{}", v, CURRENT_SCHEMA_VERSION);
    }
    Ok(())
}

/// 当前 schema 版本——db.sql 建出的库就是这个版本。
/// 升 schema 时：改 db.sql + 改这个常量 + 改 db.sql 顶部注释。
/// v58（2026-08-02）：hotword_sets 加 set 级软删——is_deleted 存删除时刻 epoch 秒 + UNIQUE(name,is_deleted) 复合约束（建表复制法迁移）。
/// v57（2026-08-01）：热词拆分单记录——hotword_words 表（每词一条，确定性 UUID v5 + 原始拼音 + 软删）+ hotword_sets DROP words_text 列。
/// v56（2026-08-01）：方言规则 DB 化——fuzzy_dialect_rules 表 + 旧 fuzzy_dialect 开关迁移。
/// v55（2026-08-01）：数据迁移——asr_correct 强制翻 true（让存量用户热词生效，无表结构变更）。
/// v54（2026-07-30）：image_data 表移除 blob + image_type 列（原图改文件系统存储）。
pub const CURRENT_SCHEMA_VERSION: u32 = 58;

/// v28 迁移：为所有 source_type IN (0,1)（builtin+local）且 secret_key 为空的模型填充 manifest JSON。
/// 按 domain 分发到 model_manifests 常量。
pub(crate) fn fill_manifests(conn: &Connection) -> Result<()> {
    for (domain, lookup) in [
        ("asr", crate::model_manifests::asr_manifest as fn(&str) -> Option<&str>),
        ("translate", crate::model_manifests::translate_manifest),
        ("ocr", crate::model_manifests::ocr_manifest),
    ] {
        // 填充 secret_key 为空的行（首次 init）
        let rows: Vec<String> = conn
            .prepare(
                &format!(
                    "SELECT model_name FROM models WHERE domain='{}' AND source_type IN (0,1) AND (secret_key='' OR secret_key IS NULL)",
                    domain
                ),
            )?
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        for name in &rows {
            if let Some(json) = lookup(name) {
                conn.execute(
                    "UPDATE models SET secret_key=?1 WHERE model_name=?2 AND domain=?3",
                    params![json, name, domain],
                )?;
            }
        }
        // 升级旧 bootstrap manifest（source 全空）→ 替换为预填常量（含 source URL）
        let all_rows: Vec<String> = conn
            .prepare(
                &format!(
                    "SELECT model_name FROM models WHERE domain='{}' AND source_type IN (0,1) AND secret_key != ''",
                    domain
                ),
            )?
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        for name in &all_rows {
            if let Some(preset) = lookup(name) {
                // 检查现有 manifest 是否有 source 字段（任一文件有 source 即认为已升级）
                let current: String = conn
                    .query_row(
                        &format!("SELECT secret_key FROM models WHERE model_name=?1 AND domain='{}'", domain),
                        params![name],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap_or_default();
                let has_source = serde_json::from_str::<serde_json::Value>(&current)
                    .ok()
                    .and_then(|v| v.as_object().map(|obj| {
                        obj.values().any(|v| v.get("source").and_then(|s| s.as_str()).map(|s| !s.is_empty()).unwrap_or(false))
                    }))
                    .unwrap_or(false);
                if !has_source {
                    log::info!("[fill_manifests] {} secret_key 无 source，升级为预填常量", name);
                    conn.execute(
                        "UPDATE models SET secret_key=?1 WHERE model_name=?2 AND domain=?3",
                        params![preset, name, domain],
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// 一次性 yaml → DB 迁移：config.yaml 存在时解析 → ON CONFLICT 覆盖 seed value → 重命名为 .bak。
/// 幂等：config.yaml 不存在时直接返回。
fn migrate_yaml_to_db(conn: &Connection) -> Result<()> {
    let config_path = crate::octopus_config_home().join("config.yaml");
    if !config_path.exists() {
        return Ok(());
    }

    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("读取旧 config.yaml 失败: {}", config_path.display()))?;

    // 复用字段名迁移逻辑（shortcut → asr_shortcut 等）
    let mut value: serde_yaml::Value = serde_yaml::from_str(&text)?;
    if let Some(map) = value.as_mapping_mut() {
        migrate_yaml_key(map, "shortcut", "asr_shortcut");
        migrate_yaml_key(map, "polish_interval", "polish_min_interval");
    }
    let cfg: crate::config::AppConfig = serde_yaml::from_value(value)?;

    // 覆盖 seed 默认值（INSERT OR REPLACE）
    config::save_app_config_at(conn, &cfg)?;

    // 重命名旧文件
    let bak = config_path.with_extension("yaml.bak");
    let _ = std::fs::rename(&config_path, &bak);
    log::info!(
        "config.yaml → app_config 迁移完成（备份: {}）",
        bak.display()
    );
    Ok(())
}

/// yaml 字段名迁移：旧键存在时，新键不存在则迁移、新键已存在则删旧留新。
fn migrate_yaml_key(map: &mut serde_yaml::Mapping, old: &str, new: &str) {
    let old_key = serde_yaml::Value::String(old.into());
    let new_key = serde_yaml::Value::String(new.into());
    if map.get(&old_key).is_some() {
        if map.get(&new_key).is_none() {
            let old_val = map.remove(&old_key).unwrap();
            map.insert(new_key, old_val);
        } else {
            map.remove(&old_key);
        }
    }
}

// ── 时间戳工具（避免依赖 chrono）──

/// 当前时间字符串 'YYYY-MM-DD HH:MM:SS'。
pub(crate) fn now_string() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let diy = if is_leap(year) { 366 } else { 365 };
        if days < diy {
            break;
        }
        days -= diy;
        year += 1;
    }
    let md: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0u64;
    for (i, &m) in md.iter().enumerate() {
        if days < m {
            month = i as u64 + 1;
            break;
        }
        days -= m;
    }
    if month == 0 {
        month = 12;
    }
    (year, month, days + 1)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在内存 DB 上执行 INIT_SQL，返回初始化好的连接。
    fn open_init() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    /// 全局测试 DB 初始化（进程级 Once）。
    ///
    /// 调用 [`init_test_db`] 切换到 in-memory 模式——所有经 [`with_db`] /
    /// [`ensure_db`] 的测试不再打开 `~/.octopus/octopus.db`，彻底隔离开发库。
    /// 详见架构文档「测试数据库隔离」。
    static TEST_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn setup_test_db() {
        TEST_DB_SETUP.call_once(|| {
            init_test_db();
        });
    }

    // ── action_bar shortcut 测试见 db/action_bar.rs ──

    /// 回归：`with_db` 的锁必须可重入——闭包内再调 `with_db` 不应死锁。
    /// 历史 `parking_lot::Mutex`（非递归）致同线程重入永久死锁（memory with-db-reentrant-deadlock）；
    /// 改 `ReentrantMutex` 后根治。此测试若退回 `Mutex` 会**挂起**（重入第二次 lock 永久阻塞）。
    /// 用只读 `PRAGMA` 避免污染数据；`ensure_db` 对已存在的 v18 库幂等（noop）。
    #[test]
    fn with_db_reentrant_no_deadlock() {
        setup_test_db();
        let outer = with_db(|conn| {
            // 同线程重入：闭包内再调 with_db
            let inner_v: u32 = with_db(|c2| {
                Ok(c2.query_row("PRAGMA user_version", [], |r| r.get(0))?)
            })?;
            let outer_v: u32 =
                conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            assert_eq!(inner_v, outer_v, "重入应观察到同一连接状态");
            Ok(inner_v)
        });
        assert!(outer.is_ok(), "with_db 重入不应死锁: {:?}", outer);
    }



    #[test]
    fn init_schema_fresh_db_builds_current_version() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION, "全新库 init_schema 后应到 CURRENT_SCHEMA_VERSION");
        // v48: models 表用 source_type（非旧 is_local）
        let has_source_type: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('models') WHERE name='source_type'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_source_type, "v48 models 表应有 source_type 列");
        let has_is_local: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('models') WHERE name='is_local'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!has_is_local, "v48 models 表不应再有 is_local 列");
        // 六张核心表都已建好（含 action_bar_items）
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
                 AND name IN ('models','prompts','app_config','clipboard_history','image_data','action_bar_items')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 6, "六张核心表都应建好");
        // v40 外置 seed：Agent 主菜单 + 制作 PPT 子项已注入
        let agent_cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(agent_cnt, 1, "v39→v40 升级后应注入 Agent 主菜单");
        // v43: Agent 下应有两个子菜单——PPT 大纲 + PPT 制作
        for title in ["PPT 大纲", "PPT 制作"] {
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM action_bar_items WHERE title=?1",
                    rusqlite::params![title],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 1, "v39→v40 升级后应注入「{}」子项", title);
        }
    }

    // ── action_bar 测试见 db/action_bar.rs ──

    // ── agent_adapters / agent_tasks 测试见 db/agent.rs ──

    #[test]
    fn action_bar_submenu_accepts_default_any() {
        // init_schema 完成后，db.sql seed 的 submenu 项（AI / 搜索）accepts='any'
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        // v40 后 Agent 主菜单也是 submenu（accepts='file'）——按 title 过滤，只测 db.sql
        // 中明确设 accepts='any' 的「AI」+「搜索」两项。
        let submenu_accepts: Vec<String> = conn.prepare(
            "SELECT accepts FROM action_bar_items
             WHERE action_type='submenu' AND title IN ('AI','搜索') ORDER BY id"
        ).unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(submenu_accepts.len(), 2, "应有 AI + 搜索 两个 submenu（accepts='any'）");
        for a in &submenu_accepts {
            assert_eq!(a, "any", "submenu accepts 应为 'any'，实际: {}", a);
        }
    }

    // 历史 v36 自愈 + v36→v37 语义迁移测试已删除（v40 schema 重整，迁移分支移除）：
    // - v36_self_heal_when_launcher_missing_but_version_set
    // - migration_v36_to_v37_migrates_is_enabled_semantics_and_clears_activation
    // 这些迁移只在 v17→v37 旧库升级路径上有意义；新 schema 全部由 db.sql + 外置 seed
    // 覆盖（launcher_index / models.is_available / models.is_enabled 均在 db.sql）。
    // switch_active_model 的核心不变量（每域仅 1 个 is_enabled=1）由下列测试覆盖。


    /// 已是 CURRENT_SCHEMA_VERSION 的库再次调 init_schema 应是 no-op——不重读 seed 文件、不重复插入。
    #[test]
    fn init_schema_already_v40_is_noop() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // 走完整初始化路径（含 load_external_seeds）
        init_schema(&conn).unwrap();
        // 抓基线 row counts
        let baseline_prompts: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        let baseline_agent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE title='Agent'",
                [], |r| r.get(0),
            )
            .unwrap();
        // 再次调 init_schema（应早返）
        init_schema(&conn).unwrap();
        // 验证 row counts 不变（早返 = 无 seed 加载 = 无重复插入）
        let after_prompts: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        let after_agent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE title='Agent'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(baseline_prompts, after_prompts, "v40+ 早返，prompts 不应重复插入");
        assert_eq!(baseline_agent, after_agent, "v40+ 早返，Agent 菜单不应重复插入");
    }

    // ── hotword 测试见 db/hotword.rs ──

    #[test]
    fn init_sql_is_idempotent() {
        let conn = open_init();
        conn.execute_batch(INIT_SQL).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM models WHERE domain='asr'", [], |r| r.get(0))
            .unwrap();
        // v48: 13 local ASR + 1 builtin (zipformer-small) = 14
        assert_eq!(count, 14);
    }









    // ── 模型激活语义（Task 1-2 引入）单测 ──
    // 不变量来源：specs/2026-07-17-model-activation-refactor-design.md §3.3 / §6 / §7
















    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }

    // ── transcription / FTS5 测试见 db/transcription.rs ──

    // ── app_config 表测试（见 db/config.rs）──

    #[test]
    fn prompts_table_seeded_with_default() {
        let conn = open_init();
        // prompts seed 已外置到 seeds/prompts/（v40 后 db.sql 不再内联），
        // init_schema 在生产路径会调 load_external_seeds——测试里显式调一次。
        crate::seeds::load_external_seeds(&conn).unwrap();
        // id=1 系统默认 prompt 存在
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE id=1 AND is_system=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "应有 id=1 的系统默认 prompt");
        // total 至少 1 条
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert!(total >= 1);
        // active_polish_prompt 配置项存在，默认值 '1'
        let val: String = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='active_polish_prompt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(val, "1");
    }

    // ── v54→v55 数据迁移：asr_correct 强制翻 true ──

    /// 辅助：构造一个指定 user_version 的库（跑 INIT_SQL 建表 + seed，再覆盖版本号 + asr_correct）。
    fn open_with_version(version: u32, asr_correct: &str) -> Connection {
        let conn = open_init();
        conn.execute(
            "UPDATE app_config SET config_value = ?1 WHERE config_key = 'asr_correct'",
            params![asr_correct],
        )
        .unwrap();
        conn.execute(&format!("PRAGMA user_version = {}", version), [])
            .unwrap();
        conn
    }

    /// v54 库（asr_correct='false'）→ init_schema → asr_correct 翻 true + user_version 升 55。
    #[test]
    fn migrate_v54_to_v55_flips_asr_correct_to_true() {
        let conn = open_with_version(54, "false");
        init_schema(&conn).expect("v54→v55 迁移");
        let after: String = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='asr_correct'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, "true", "迁移后 asr_correct 应翻 true");
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);
    }

    /// v55 库（已是最新）→ init_schema → no-op（不重复迁移）。
    #[test]
    fn init_schema_noop_on_current_version() {
        let conn = open_with_version(CURRENT_SCHEMA_VERSION, "true");
        init_schema(&conn).expect("已是最新版本应 no-op");
    }

    /// v < 54 的旧库 → init_schema → bail（不支持自动迁移）。
    #[test]
    fn init_schema_bails_on_very_old_version() {
        let conn = open_with_version(53, "false");
        assert!(init_schema(&conn).is_err(), "v53 旧库应 bail 提示清库");
    }

    // ── FTS5 搜索测试见 db/transcription.rs ──

    // ── action_bar items / move 测试见 db/action_bar.rs ──

    // ── hotword hits / active_words 测试见 db/hotword.rs ──

    // ── v28: manifest 填充 + 路径统一 测试 ──







    // ── TDD 防御：OCR 列表不过滤 is_enabled ──


    // ── TDD 防御：env 变量 config_key 不含 env. 前缀 ──

    // ── env_var 测试见 db/config.rs ──

    // ── search_frequency 测试见 db/action_bar.rs ──
}

#[cfg(test)]
mod vault_schema_tests {
    use super::*;

    /// 全新库（v=0）经 init_schema 后应升到 CURRENT_SCHEMA_VERSION，
    /// recordings 表 + audio_tracks 列应存在。
    #[test]
    fn fresh_db_has_recordings_and_audio_tracks() {
        let conn = Connection::open_in_memory().unwrap();
        // user_version 默认 0 → 走全新库分支
        init_schema(&conn).unwrap();

        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION, "全新库应升到 CURRENT_SCHEMA_VERSION");

        let has_recordings: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='recordings')",
                [], |r| r.get(0),
            )
            .unwrap();
        assert!(has_recordings, "全新库应有 recordings 表");

        let has_audio_tracks: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('recordings') WHERE name='audio_tracks'")
            .unwrap()
            .query_row([], |r| r.get(0))
            .unwrap_or(false);
        assert!(has_audio_tracks, "全新库 recordings 应有 audio_tracks 列");
    }

    /// 旧版本库（1 <= v < CURRENT_SCHEMA_VERSION）应 bail，不自动迁移。
    #[test]
    fn outdated_db_bails_instead_of_auto_migrating() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 50", []).unwrap();
        let result = init_schema(&conn);
        assert!(result.is_err(), "旧版本库应 bail，不应自动迁移");
    }
}
