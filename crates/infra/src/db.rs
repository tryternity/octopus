// crates/infra/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};

/// 收集 query_map 结果，遇到失败行时 log::warn 并跳过（而非静默丢弃）。
/// 替代 `.filter_map(|r| r.ok()).collect()`——后者吞掉所有错误，
/// 模型加载/历史搜索中损坏行会被静默忽略，难以排查。
fn collect_rows<T, E: std::fmt::Display>(
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

// ── Model config schema（DB models 表）──

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct ModelEntry {
    pub source: String,
    #[serde(default)]
    pub language: String,
    /// Secret key (API key) for remote API-based ASR engines, if applicable.
    #[serde(default)]
    pub secret_key: String,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default)]
    pub is_available: bool,
    #[serde(default)]
    pub is_streaming: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
pub struct AsrSection {
    pub whisper: Option<HashMap<String, ModelEntry>>,
    /// 原版 SenseVoice-Small（FunASR 4 输入 ONNX，非 sherpa 简化版）。provider='local' + category='sensevoice-orig' 路由入此。
    #[serde(default)]
    pub sensevoice_orig: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub paraformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default, rename = "qwen3-asr")]
    pub qwen3_asr: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub zipformer: Option<HashMap<String, ModelEntry>>,
    /// Moonshine 端侧 ASR（Useful Sensors）。provider='local' + category='moonshine' 路由入此。
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,
    /// FireRedASR2-AED CTC（小红书，本地）。provider='local' + category='firered' 路由入此。
    #[serde(default)]
    pub firered: Option<HashMap<String, ModelEntry>>,
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    #[serde(default)]
    pub aliyun: Option<HashMap<String, ModelEntry>>,
    /// 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）。provider='bytedance' 路由入此。
    #[serde(default)]
    pub bytedance: Option<HashMap<String, ModelEntry>>,
    /// 腾讯云实时语音识别（WebSocket HMAC-SHA1 签名鉴权）。provider='tencent' 路由入此。
    #[serde(default)]
    pub tencent: Option<HashMap<String, ModelEntry>>,
    /// 百度智能云实时语音识别（WebSocket START 帧鉴权）。provider='baidu' 路由入此。
    #[serde(default)]
    pub baidu: Option<HashMap<String, ModelEntry>>,
}

/// DB models 表配置（domain='asr'；由 db::load_models 构造）。
#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
pub struct AsrConfig {
    pub asr: AsrSection,
}

/// 兼容 OpenAI 接口的 LLM 配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompatibleLlmConfig {
    /// 提供商标识（如 "openai", "deepseek"），仅用于日志
    pub provider: String,
    /// 模型名（如 "gpt-4o-mini", "deepseek-chat"）
    pub model: String,
    /// API base URL（如 "https://api.openai.com/v1"）
    pub base_url: String,
    /// API Key
    pub secret_key: String,
    /// 是否为思考（reasoning）模型。
    pub is_thinking: bool,
    /// 是否为本地模型。
    pub is_local: bool,
    /// 是否启用。
    pub is_enabled: bool,
}

impl CompatibleLlmConfig {
    /// 润色时是否需要显式关闭思考模式。
    pub fn needs_disable_thinking(&self) -> bool {
        self.is_thinking
    }
}

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
const INIT_SQL: &str = include_str!("db.sql");

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
pub fn ensure_db() -> Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let conn = open_db_conn()?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;",
    )
    .context("set WAL + busy_timeout")?;
    init_schema(&conn)?;
    let _ = DB.set(parking_lot::ReentrantMutex::new(conn));
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


/// 初始化 schema：以 db.sql 为唯一表结构真相，外置 seed 加载机制注入长文本。
///
/// **分支**：
/// - `v >= 39`：最新，no-op。
/// - `17 <= v < 39`：开发期历史库（唯一用户已 ≥v38）。db.sql 对这些库已 no-op
///   （所有表/列/vault 表均由 db.sql `CREATE TABLE IF NOT EXISTS` 覆盖），仅补跑外置
///   seed 升到 v40。历史 v17→v37 的 ALTER / DROP / 数据迁移分支已删除——这些表/列
///   在 db.sql 内已存在（vault_*、launcher_index、search_frequency、global_shortcut、
///   trigger_keyword、models.is_available）。`auto_paste` 列已废弃（代码不再读写），
///   不在新 schema 中出现。
/// - `v < 17`：全新库——db.sql 建表 + 外置 seed + yaml 迁移 + manifest 填充 → v40。
///
/// schema 变更流程：改 db.sql + 升下方 user_version 数值。
/// v38：vault_* 表（2026-07-18 Password Vault，db.sql 已含）。
/// v40：外置 seed 加载机制（prompts/llm_providers/agent_actions）+ Agent 菜单 + PPT。
/// v43：PPT 两阶段——「制作 PPT」改名「PPT 制作」+ 新增「PPT 大纲」子菜单。
///      纯 seed 重跑（无 schema 变更），bump user_version 触发 load_external_seeds。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v >= 47 {
        // v47+ 已最新，直接返回。
        return Ok(());
    }

    // v46→v47：clipboard_history 加 deleted_at 列（软删/回收站）。
    // 单独提前处理 v46 库——下面的 v44/v45/v46 迁移段以 v<=46 为入口，
    // v46 库不需要跑那些迁移，只需补 deleted_at 列后直接跳 v47 返回。
    if v == 46 {
        let has_clip_table = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='clipboard_history'")?
            .exists([])?;
        if has_clip_table {
            let has_deleted_at = conn
                .prepare("SELECT 1 FROM pragma_table_info('clipboard_history') WHERE name = 'deleted_at'")?
                .exists([])?;
            if !has_deleted_at {
                conn.execute("ALTER TABLE clipboard_history ADD COLUMN deleted_at TEXT", [])?;
                log::info!("schema v47: clipboard_history 补 deleted_at 列");
            }
        }
        conn.execute("PRAGMA user_version = 47", [])?;
        log::info!("schema upgraded to v47 (clipboard_history deleted_at 软删列)");
        return Ok(());
    }

    // v44→v45：vault_ciphers / vault_folders 加 sync_md5 字段（md5 内容指纹，
    // 用于增量同步 diff）。详见 vault::sync::fingerprint。
    //
    // 迁移策略：只加字段，不回填 md5（旧数据 sync_md5 = NULL）。
    // 首次 sync_now 时检测到 NULL 当作「需要写文件」处理（增量 push 重写一次），
    // 之后正常增量。避免在 infra 层引入 md5 依赖（md5 逻辑在 vault crate）。
    //
    // ALTER TABLE ADD COLUMN 需检查列是否存在（开发期中间 binary 可能跳过版本号）。
    if v == 44 {
        let has_cipher_md5 = conn
            .prepare("SELECT 1 FROM pragma_table_info('vault_ciphers') WHERE name = 'sync_md5'")?
            .exists([])?;
        if !has_cipher_md5 {
            conn.execute("ALTER TABLE vault_ciphers ADD COLUMN sync_md5 TEXT", [])?;
        }
        let has_folder_md5 = conn
            .prepare("SELECT 1 FROM pragma_table_info('vault_folders') WHERE name = 'sync_md5'")?
            .exists([])?;
        if !has_folder_md5 {
            conn.execute("ALTER TABLE vault_folders ADD COLUMN sync_md5 TEXT", [])?;
        }
        // 不设 user_version 也不 return——fall through 到 v46 段（同一次启动完成 v44→v45→v46）
    }

    // v43→v44：vault_ciphers / vault_folders 的 id 从 INTEGER AUTOINCREMENT 改
    // TEXT（UUID 字符串），支持 git 同步跨设备无冲突。
    //
    // 迁移策略（保守，不丢数据）：
    //   1. 检查 vault_ciphers 是否还是 INTEGER（旧 schema）—— SQLite 没有直接
    //      查 PRIMARY KEY 类型的 API，但 PRAGMA table_info 会返 type 列；
    //      type="TEXT" 表示已迁移过（开发期中间 binary 可能跳过），跳过；
    //      type="INTEGER" 表示旧 schema，跑迁移。
    //   2. 建新表 + 复制（每行 randomblob(16) 转 hex 当 UUID）+ 修 folder_id 引用
    //   3. DROP 旧表 + RENAME 新表
    //
    // 注意：vault 是 2026-07-18 v38 引入的；v<38 的旧库走 v<17 分支（db.sql 建
    // 新表已经是 TEXT PRIMARY KEY），不会触发此迁移——只有 v38/v43 的老 vault
    // 库需要此迁移。
    if v >= 17 && v <= 43 {
        // 先跑之前的 v40/v42 升级（需要补 need_voice / agent_adapters 列），
        // 然后跑 v44 vault UUID 迁移。
        conn.execute_batch(INIT_SQL).ok();
        // v39→v40：action_bar_items 加 need_voice 列
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"need_voice".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN need_voice INTEGER NOT NULL DEFAULT 0", [])?;
                log::info!("schema v40: action_bar_items 补 need_voice 列");
            }
        }
        // v41→v42：agent_adapters 加 is_system + is_default 列
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(agent_adapters)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"is_system".to_string()) {
                conn.execute("ALTER TABLE agent_adapters ADD COLUMN is_system INTEGER NOT NULL DEFAULT 0", [])?;
                log::info!("schema v42: agent_adapters 补 is_system 列");
            }
            if !cols.contains(&"is_default".to_string()) {
                conn.execute("ALTER TABLE agent_adapters ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0", [])?;
                log::info!("schema v42: agent_adapters 补 is_default 列");
            }
        }
        // v42 内置 agent seed
        conn.execute(
            "INSERT OR IGNORE INTO agent_adapters (key, display_name, detect_binary, command_template, is_system, is_default) VALUES
                ('claude', 'Claude Code', 'claude', 'claude --add-dir {cwd} {prompt}', 1, 0),
                ('pi',     'Pi',          'pi',     'pi {files_at} {prompt}',           1, 1)",
            [],
        ).context("seed Pi/Claude 入 agent_adapters")?;
        fill_manifests(conn)?;
        crate::seeds::load_external_seeds(conn)?;

        // **v43→v44: vault_ciphers / vault_folders id 改 UUID 字符串（git 同步）**
        // 见上方注释。检查 vault_ciphers.id 类型——INTEGER 表示旧 schema 需迁移。
        let cipher_id_type: String = conn
            .query_row(
                "SELECT type FROM pragma_table_info('vault_ciphers') WHERE name = 'id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "TEXT".to_string()); // 表不存在或查询失败时按已迁移处理
        if cipher_id_type == "INTEGER" {
            log::info!("schema v44: 迁移 vault_ciphers / vault_folders id INTEGER → TEXT (UUID)");
            // 用 hex(randomblob(16)) 作伪 UUID（足够唯一，非标准 v4 但跨设备无冲突）
            // 真正的 v4 UUID 在 vault crate 的 create_cipher 时生成；这里只是给
            // 旧 i64 行分配一个临时全局唯一 id 让迁移通过。

            // 1. folders 先迁移（cipher 引用 folder_id，要等 folder 新 id 就位）
            conn.execute_batch(
                "CREATE TABLE vault_folders_new (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL,
                    sort_order  INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );
                INSERT INTO vault_folders_new (id, name, sort_order, created_at, updated_at)
                SELECT lower(hex(randomblob(16))), name, sort_order, created_at, updated_at
                FROM vault_folders;
                -- 临时映射表：old_rowid → new_uuid
                CREATE TABLE _vault_folder_id_map (old_id INTEGER, new_id TEXT);
                INSERT INTO _vault_folder_id_map (old_id, new_id)
                SELECT rowid, id FROM vault_folders_new;",
            )?;

            // 2. cipher 迁移，folder_id 按映射翻译（NULL 保留 NULL）
            conn.execute_batch(
                "CREATE TABLE vault_ciphers_new (
                    id                  TEXT PRIMARY KEY,
                    folder_id           TEXT DEFAULT NULL,
                    favorite            INTEGER NOT NULL DEFAULT 0,
                    atype               INTEGER NOT NULL,
                    name                TEXT NOT NULL,
                    notes               TEXT DEFAULT NULL,
                    data                TEXT NOT NULL,
                    fields              TEXT DEFAULT NULL,
                    password_history    TEXT DEFAULT NULL,
                    reprompt            INTEGER NOT NULL DEFAULT 0,
                    deleted_at          TEXT DEFAULT NULL,
                    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
                );
                INSERT INTO vault_ciphers_new (id, folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt, deleted_at, created_at, updated_at)
                SELECT
                    lower(hex(randomblob(16))),
                    (SELECT new_id FROM _vault_folder_id_map WHERE old_id = vault_ciphers.folder_id),
                    favorite, atype, name, notes, data, fields, password_history, reprompt, deleted_at, created_at, updated_at
                FROM vault_ciphers;",
            )?;

            // 3. DROP 旧表 + RENAME 新表 + 重建索引（DROP TABLE 不连带索引）
            conn.execute_batch(
                "DROP TABLE vault_ciphers;
                 DROP TABLE vault_folders;
                 ALTER TABLE vault_ciphers_new RENAME TO vault_ciphers;
                 ALTER TABLE vault_folders_new RENAME TO vault_folders;
                 DROP TABLE _vault_folder_id_map;
                 CREATE INDEX IF NOT EXISTS idx_vault_ciphers_favorite
                     ON vault_ciphers(favorite) WHERE deleted_at IS NULL;
                 CREATE INDEX IF NOT EXISTS idx_vault_ciphers_deleted ON vault_ciphers(deleted_at);",
            )?;
            log::info!("schema v44: vault id INTEGER → TEXT 迁移完成");
        } else {
            log::debug!("schema v44: vault_ciphers.id 已是 TEXT，跳过 UUID 迁移");
        }

        conn.execute("PRAGMA user_version = 44", [])?;
        log::info!("schema upgraded to v44 (vault_ciphers/folders id 改 UUID 字符串)");
        // 不 return——fall through 到 v45→v46 迁移（同一次启动完成 v44+v45+v46）
    }

    // v45→v46：hotword_sets.id INTEGER→TEXT UUID + sync_md5 字段 + vault sync_md5 兜底。
    //
    // **位置在 `v >= 17 && v <= 43` 完整迁移分支之后**——覆盖所有 v>=17 的库：
    //   - v17-v43 老库：上方完整迁移后 fall through 到此
    //   - v44 库：上方 v44→v45 vault sync_md5 补丁后 fall through 到此
    //   - v45 库：直接进入此段
    // 全新库（v<17）走下方 INIT_SQL（db.sql 已含新 schema），不走这里。
    if v >= 17 {
        // hotword_sets id 迁移——检查类型决定是否建新表
        let hotword_id_type: String = conn
            .query_row(
                "SELECT type FROM pragma_table_info('hotword_sets') WHERE name = 'id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "TEXT".to_string()); // 表不存在或查询失败时按已迁移处理

        if hotword_id_type == "INTEGER" {
            log::info!("schema v46: 迁移 hotword_sets.id INTEGER → TEXT (UUID)");
            conn.execute_batch(
                "CREATE TABLE hotword_sets_new (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL UNIQUE,
                    enabled     INTEGER NOT NULL DEFAULT 1,
                    words_text  TEXT NOT NULL DEFAULT '',
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    sync_md5    TEXT
                );
                INSERT INTO hotword_sets_new (id, name, enabled, words_text, created_at, updated_at, sync_md5)
                SELECT
                    CASE WHEN name = '通用'
                         THEN '00000000-0000-0000-0000-000000000001'
                         ELSE lower(hex(randomblob(16)))
                    END,
                    name, enabled, words_text, created_at, updated_at, NULL
                FROM hotword_sets;
                DROP TABLE hotword_sets;
                ALTER TABLE hotword_sets_new RENAME TO hotword_sets;",
            )?;
            log::info!("schema v46: hotword_sets id INTEGER → TEXT 迁移完成");
        } else {
            // 已是 TEXT 但可能缺 sync_md5 列（开发期中间 binary）——检查并补
            let has_md5 = conn
                .prepare("SELECT 1 FROM pragma_table_info('hotword_sets') WHERE name = 'sync_md5'")?
                .exists([])?;
            if !has_md5 {
                conn.execute("ALTER TABLE hotword_sets ADD COLUMN sync_md5 TEXT", [])?;
                log::info!("schema v46: hotword_sets 补 sync_md5 列");
            }
        }

        // vault_ciphers/folders sync_md5 兜底（v45 补丁——确保 vault 表也有 sync_md5）。
        // 表可能不存在（如纯热词测试库只建了 hotword_sets）——先检查表存在再 ALTER。
        let vault_ciphers_exists: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('vault_ciphers')")?
            .exists([])?;
        if vault_ciphers_exists {
            let has_cipher_md5 = conn
                .prepare("SELECT 1 FROM pragma_table_info('vault_ciphers') WHERE name = 'sync_md5'")?
                .exists([])?;
            if !has_cipher_md5 {
                conn.execute("ALTER TABLE vault_ciphers ADD COLUMN sync_md5 TEXT", [])?;
            }
        }
        let vault_folders_exists: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('vault_folders')")?
            .exists([])?;
        if vault_folders_exists {
            let has_folder_md5 = conn
                .prepare("SELECT 1 FROM pragma_table_info('vault_folders') WHERE name = 'sync_md5'")?
                .exists([])?;
            if !has_folder_md5 {
                conn.execute("ALTER TABLE vault_folders ADD COLUMN sync_md5 TEXT", [])?;
            }
        }

        conn.execute("PRAGMA user_version = 46", [])?;
        log::info!("schema upgraded to v46 (hotword_sets id 改 UUID + sync_md5 字段)");
        // fall through 到 v47 段（不 return）——同一次启动完成 v44→v45→v46→v47
    }

    // v46→v47：clipboard_history 加 deleted_at 列（软删/回收站）。
    //
    // ALTER TABLE ADD COLUMN 需检查列是否存在（开发期中间 binary 可能跳过版本号）。
    // 图片不软删（image_data 引用计数约束），deleted_at 对图片始终 NULL——靠应用层保证，
    // schema 层不加 CHECK 约束（SQLite CHECK 对 item_type='image' AND deleted_at NOT NULL 的
    // 限制会影响 INSERT 性能，不值得）。
    //
    // 测试库可能只有 hotword_sets 没有 clipboard_history——先检查表存在再 ALTER。
    if v >= 17 && v <= 46 {
        let has_clip_table = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='clipboard_history'")?
            .exists([])?;
        if has_clip_table {
            let has_deleted_at = conn
                .prepare("SELECT 1 FROM pragma_table_info('clipboard_history') WHERE name = 'deleted_at'")?
                .exists([])?;
            if !has_deleted_at {
                conn.execute("ALTER TABLE clipboard_history ADD COLUMN deleted_at TEXT", [])?;
                log::info!("schema v47: clipboard_history 补 deleted_at 列");
            }
        }
        conn.execute("PRAGMA user_version = 47", [])?;
        log::info!("schema upgraded to v47 (clipboard_history deleted_at 软删列)");
        return Ok(());
    }

    // v<17 全新库：建表 + 外置 seed + manifest
    conn.execute_batch(INIT_SQL).context("执行 db.sql 建表 + seed")?;
    migrate_yaml_to_db(conn)?; // config.yaml 存在时一次性导入（导入后重命名 .bak），否则幂等返回
    // 填充 manifest（全新库 seed 中 secret_key 为空 → 从常量写入）
    fill_manifests(conn)?;
    crate::seeds::load_external_seeds(conn)?;
    // 全新库 db.sql 已含 sync_md5 字段 + hotword_sets TEXT id（v46 schema）+ deleted_at（v47），直接设 v47
    conn.execute("PRAGMA user_version = 47", [])?;
    log::info!("DB initialized (v47): schema + external seeds + manifest fill + yaml 配置导入（无 yaml 则跳过）");
    Ok(())
}

/// v28 迁移：为所有 is_local=1 且 secret_key 为空的模型填充 manifest JSON。
/// 按 domain 分发到 model_manifests 常量。
fn fill_manifests(conn: &Connection) -> Result<()> {
    for (domain, lookup) in [
        ("asr", crate::model_manifests::asr_manifest as fn(&str) -> Option<&str>),
        ("translate", crate::model_manifests::translate_manifest),
        ("ocr", crate::model_manifests::ocr_manifest),
    ] {
        // 填充 secret_key 为空的行（首次 init）
        let rows: Vec<String> = conn
            .prepare(
                &format!(
                    "SELECT model_name FROM models WHERE domain='{}' AND is_local=1 AND (secret_key='' OR secret_key IS NULL)",
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
                    "SELECT model_name FROM models WHERE domain='{}' AND is_local=1 AND secret_key != ''",
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
    save_app_config_at(conn, &cfg)?;

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

// ── Model spec 解析（统一 asr_engine / polish_llm 配置格式）──

/// 模型选择规格，统一 `asr_engine` 和 `polish_llm` 的 3-part 格式
/// `{provider}:{category}:{model_name}`。
///
/// | 配置写法 | 含义 |
/// |---------|------|
/// | `"PROVIDER:CATEGORY:NAME"` | 三段精确匹配 `provider AND category AND model_name` |
/// | `"NAME"`（无冒号） | 跨 provider/category 搜 name，优先 local（全局默认 fallback 用） |
/// | `"X:Y"`（1 个冒号，旧 2-part） | warn + 按整串作裸名兜底（NameOnly，向后兼容） |
///
/// 1 个冒号（旧 2-part 格式）按裸名兜底（NameOnly）并 warn。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSpec<'a> {
    /// `{provider}:{category}:{model_name}` 三段精确匹配
    Full { provider: &'a str, category: &'a str, model_name: &'a str },
    /// 裸 `{model_name}`：仅全局默认 fallback 用（跨 provider/category 搜 name，优先 local）
    NameOnly(&'a str),
}

/// 解析 3-part 规格字符串。
/// - 2 个冒号（3 段）→ Full
/// - 0 冒号 → NameOnly
/// - 1 冒号（旧 2-part 格式）→ warn + 按 NameOnly 兜底
pub fn parse_model_spec(spec: &str) -> ModelSpec<'_> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.len() {
        3 => ModelSpec::Full { provider: parts[0], category: parts[1], model_name: parts[2] },
        1 => ModelSpec::NameOnly(parts[0]),
        _ => {
            log::warn!(
                "模型 spec '{}' 非合法 3-part '{{provider}}:{{category}}:{{model_name}}'，按裸名兜底",
                spec
            );
            ModelSpec::NameOnly(spec)
        }
    }
}

impl<'a> ModelSpec<'a> {
    /// 返回 model_name（去掉 provider:/category: 前缀）。
    pub fn model_name(&self) -> &'a str {
        match self {
            ModelSpec::Full { model_name, .. } | ModelSpec::NameOnly(model_name) => model_name,
        }
    }
}

// ── app_config 表读写（替代 config.yaml）──

/// 从 DB app_config 表加载完整应用配置。
/// 先构造 AppConfig::default()（保底），再用 DB 行按字段类型解析覆盖。
/// 缺失行或解析失败 → 保留 default 值（防御性，正常不应触发——seed 保证 21 行齐全）。
/// 只读 category='setting' 的行（用户配置项）。
pub fn load_app_config() -> Result<crate::config::AppConfig> {
    ensure_db()?;
    with_db(load_app_config_at)
}

fn load_app_config_at(conn: &Connection) -> Result<crate::config::AppConfig> {
    // 以 AppConfig::default() 的 JSON 形态作为类型模板——每个 DB 字段按模板类型还原，
    // 不靠字符串内容猜类型（避免把值恰为数字的 String 字段误判为 Number）。
    // 字段增删自动反映，无需手动维护 match arms。parse 失败保留 default（同旧行为）。
    let mut result = serde_json::to_value(crate::config::AppConfig::default())
        .expect("AppConfig default 序列化不会失败");
    let type_hints = result
        .as_object()
        .expect("AppConfig 序列化为 JSON object")
        .clone();

    let mut stmt = conn.prepare(
        "SELECT config_key, config_value FROM app_config WHERE category = 'setting'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        // 未知 key 跳过（前向兼容，同旧 _ => {}）
        if let Some(hint) = type_hints.get(&key) {
            if let Some(slot) = result.get_mut(&key) {
                *slot = coerce_db_string(&value, hint);
            }
        }
    }
    Ok(serde_json::from_value(result).unwrap_or_default())
}

/// 按 JSON 类型模板把 DB TEXT 还原为 serde_json::Value。
/// - Bool: "true"/"false"
/// - Number: 先 i64 后 f64，parse 失败返回 hint（保留 default）
/// - String / 其他: 原样返回字符串
fn coerce_db_string(s: &str, hint: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match hint {
        Value::Bool(_) => Value::Bool(s == "true"),
        Value::Number(_) => {
            if let Ok(n) = s.parse::<i64>() {
                Value::Number(n.into())
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or_else(|| hint.clone())
            } else {
                hint.clone()
            }
        }
        _ => Value::String(s.to_string()),
    }
}

/// 全量写入应用配置（serde 自动遍历所有字段，ON CONFLICT DO UPDATE）。set_config / yaml 迁移用。
/// 仅更新 config_value，保留 description + category（不同于 INSERT OR REPLACE 会清空非指定列）。
/// 字段增删自动反映，无需手动维护字段数组。
pub fn save_app_config(cfg: &crate::config::AppConfig) -> Result<()> {
    ensure_db()?;
    with_db(|conn| save_app_config_at(conn, cfg))
}

fn save_app_config_at(conn: &Connection, cfg: &crate::config::AppConfig) -> Result<()> {
    // serde 序列化为 JSON Map 后逐字段 upsert——字段增删自动反映，无需手动维护字段数组。
    let value = serde_json::to_value(cfg).context("序列化 AppConfig")?;
    let obj = value.as_object().context("AppConfig 序列化非 object")?;

    // 包事务：所有字段写入要么全部成功要么全部回滚，避免中途崩溃导致配置半更新。
    // unchecked_transaction 可在已有事务上下文中调用（不会 panic），commit 原子提交。
    let tx = conn.unchecked_transaction()?;
    for (key, val) in obj {
        // 还原为 DB 存储的 TEXT：字符串直接取值，数字/bool to_string。
        let s = match val {
            serde_json::Value::String(s) => s.clone(),
            _ => val.to_string(),
        };
        tx.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, s],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 单键写入（persist_* 命令用，避免全量回写）。
/// 使用 ON CONFLICT DO UPDATE 仅改 config_value，保留 description + category。
pub fn save_config_key(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, value],
        )?;
        Ok(())
    })
}

/// 按 key 读取单个 config_value（不存在返回 None）。
pub fn load_config_key(key: &str) -> Result<Option<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT config_value FROM app_config WHERE config_key = ?1")?;
        let row = stmt.query_row(params![key], |r| r.get::<_, String>(0));
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
}

// ── 环境变量（category='env'）──

/// 列出所有 env 变量，返回 (key, value) 列表。
/// key 去掉 `env.` 前缀（返回裸名如 "huggingface"）。
pub fn list_env_vars() -> Result<Vec<(String, String)>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category = 'env' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            Ok((key, value))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 保存 env 变量（category='env'，config_key 不带 env. 前缀）。
pub fn save_env_var(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value, category) VALUES (?1, ?2, 'env')
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, value],
        )?;
        Ok(())
    })
}

/// 删除 env 变量。内置 3 个（huggingface/modelscope/github）不可删，返回 Ok(false)。
pub fn delete_env_var(key: &str) -> Result<bool> {
    const BUILTIN: &[&str] = &["huggingface", "modelscope", "github"];
    if BUILTIN.contains(&key) {
        return Ok(false);
    }
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "DELETE FROM app_config WHERE config_key = ?1 AND category = 'env'",
            params![key],
        )?;
        Ok(true)
    })
}

// ── DB → AsrConfig（load_config 用）──

/// 从 DB models 表构造 AsrConfig（domain='asr'）。
pub fn load_models() -> Result<AsrConfig> {
    with_db(load_models_at)
}

fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    // 新语义：is_enabled=1 表激活（每域仅 1 个），is_available=1 表可用。
    // 推理路径只需激活的那一个——LIMIT 1（虽然每域只有一个 is_enabled=1，加 LIMIT 保险）。
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1 LIMIT 1",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?;

    #[allow(clippy::type_complexity)] // DB 行映射，10 字段 tuple 最直接
    let rows: Vec<(String, String, String, String, String, String, String, i32, i32, i32)> =
        collect_rows(rows, "load_models_at");

    let mut asr = AsrSection {
        whisper: None,
        sensevoice_orig: None,
        paraformer: None,
        qwen3_asr: None,
        zipformer: None,
        moonshine: None,
        firered: None,
        aliyun: None,
        bytedance: None,
        tencent: None,
        baidu: None,
    };
    for (provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming) in rows {
        let entry = ModelEntry {
            source,
            language,
            description,
            secret_key,
            is_local: is_local != 0,
            is_enabled: is_enabled != 0,
            is_available: true, // load_models_at 只取 is_available=1 的行
            is_streaming: is_streaming != 0,
        };
        // provider='aliyun' → asr.aliyun；provider='bytedance' → asr.bytedance；
        // provider='tencent' → asr.tencent；provider='baidu' → asr.baidu；
        // 其余按本地 category 映射本地族
        let map: &mut Option<HashMap<String, ModelEntry>> = match (provider.as_str(), category.as_str()) {
            ("aliyun", _) => &mut asr.aliyun,
            ("bytedance", _) => &mut asr.bytedance,
            ("tencent", _) => &mut asr.tencent,
            ("baidu", _) => &mut asr.baidu,
            (_, "whisper") => &mut asr.whisper,
            (_, "sensevoice-orig") => &mut asr.sensevoice_orig,
            (_, "paraformer") => &mut asr.paraformer,
            (_, "qwen3-asr") => &mut asr.qwen3_asr,
            (_, "zipformer") => &mut asr.zipformer,
            (_, "moonshine") => &mut asr.moonshine,
            (_, "firered") => &mut asr.firered,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(model_name, entry);
    }
    Ok(AsrConfig { asr })
}

// ── 模型管理页：直读/写 models 表（不过滤 is_enabled）──

/// 模型管理页用的一行本地 ASR 模型（平铺，含 is_enabled）。
///
/// 与 `load_models_at`（过滤 is_enabled=1、按 category 分组、供引擎选择）区分：
/// 本结构**不过滤 is_enabled**，供模型管理页列出「所有可下载模型（含未就绪）」。
#[derive(Debug, Clone)]
pub struct LocalAsrModelRow {
    /// DB 行 id（translate_engine / asr_engine 等配置项按 id 存）。
    pub id: i64,
    pub category: String,
    pub model_name: String,
    pub source: String,
    /// local 模型重载为「文件清单 + sha256」JSON（见 model_commands）；api 模型仍是 API key。
    pub secret_key: String,
    pub description: String,
    pub is_enabled: bool,
    pub is_available: bool,
    pub is_streaming: bool,
}

/// 列出全部本地 ASR 模型（domain='asr' AND is_local=1，**不过滤 is_enabled**）。
pub fn list_all_local_asr_models() -> Result<Vec<LocalAsrModelRow>> {
    with_db(list_all_local_asr_models_at)
}

fn list_all_local_asr_models_at(conn: &Connection) -> Result<Vec<LocalAsrModelRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming
         FROM models WHERE domain='asr' AND is_local = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LocalAsrModelRow {
            id: row.get(0)?,
            category: row.get(1)?,
            model_name: row.get(2)?,
            source: row.get(3)?,
            secret_key: row.get(4)?,
            description: row.get(5)?,
            is_enabled: row.get::<_, i32>(6)? != 0,
            is_available: row.get::<_, i32>(7)? != 0,
            is_streaming: row.get::<_, i32>(8)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 按 domain 列出所有本地模型（is_local=1），通用版。
/// 用于翻译/OCR 等非 ASR domain 的模型管理。
pub fn list_local_models_by_domain(domain: &str) -> Result<Vec<LocalAsrModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming
             FROM models WHERE domain=?1 AND is_local = 1",
        )?;
        let rows = stmt.query_map(params![domain], |row| {
            Ok(LocalAsrModelRow {
                id: row.get(0)?,
                category: row.get(1)?,
                model_name: row.get(2)?,
                source: row.get(3)?,
                secret_key: row.get(4)?,
                description: row.get(5)?,
                is_enabled: row.get::<_, i32>(6)? != 0,
                is_available: row.get::<_, i32>(7)? != 0,
                is_streaming: row.get::<_, i32>(8)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 写某本地模型的 is_available（文件就绪/可用）。写 DB。
/// 原名 set_model_enabled，2026-07-17 改名为 set_model_available（语义对齐 is_available 列）。
pub fn set_model_available(model_name: &str, enabled: bool) -> Result<()> {
    with_db(|conn| set_model_available_at(conn, model_name, enabled))
}

fn set_model_available_at(conn: &Connection, model_name: &str, enabled: bool) -> Result<()> {
    if enabled {
        // 置可用——不动 is_enabled（用户需显式 switch_active_model 激活）
        conn.execute(
            "UPDATE models SET is_available = 1 WHERE model_name = ?1 AND is_local = 1 AND domain IN ('asr','translate','ocr')",
            params![model_name],
        )?;
    } else {
        // 置不可用——同步清 is_enabled（不可用模型不能保持激活，防双激活残留）
        conn.execute(
            "UPDATE models SET is_available = 0, is_enabled = 0 WHERE model_name = ?1 AND is_local = 1 AND domain IN ('asr','translate','ocr')",
            params![model_name],
        )?;
    }
    Ok(())
}

/// 写某本地模型的 secret_key（asr/translate/ocr，模型管理页存「文件清单 + sha256」JSON）。写 DB。
pub fn set_model_secret_key(model_name: &str, json: &str) -> Result<()> {
    with_db(|conn| set_model_secret_key_at(conn, model_name, json))
}

fn set_model_secret_key_at(conn: &Connection, model_name: &str, json: &str) -> Result<()> {
    conn.execute(
        "UPDATE models SET secret_key = ?1 WHERE model_name = ?2 AND is_local = 1 AND domain IN ('asr','translate','ocr')",
        params![json, model_name],
    )?;
    Ok(())
}

// ── 云端模型 CRUD（用户自建，domain='asr'|'llm' AND is_local=0）──

/// 新增云端模型。is_available=1（前端已测试通过才保存=可用）；is_enabled=0（不自动激活，
/// 用户在管理页显式激活）。返回新行 id。
pub fn insert_cloud_model(
    domain: &str, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<i64> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, secret_key, is_local, is_available, is_enabled, is_streaming, is_thinking)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, 0, ?7, ?8)",
            params![domain, provider, category, model_name, source, secret_key,
                    is_streaming as i32, is_thinking as i32],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 更新云端模型（按 id）。secret_key 为空时不覆盖原值。
pub fn update_cloud_model(
    id: i64, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<()> {
    with_db(|conn| {
        if secret_key.is_empty() {
            // 不改 secret_key
            conn.execute(
                "UPDATE models SET provider=?1, category=?2, model_name=?3, source=?4,
                 is_streaming=?5, is_thinking=?6 WHERE id=?7 AND is_local=0",
                params![provider, category, model_name, source,
                        is_streaming as i32, is_thinking as i32, id],
            )?;
        } else {
            conn.execute(
                "UPDATE models SET provider=?1, category=?2, model_name=?3, source=?4,
                 secret_key=?5, is_streaming=?6, is_thinking=?7 WHERE id=?8 AND is_local=0",
                params![provider, category, model_name, source, secret_key,
                        is_streaming as i32, is_thinking as i32, id],
            )?;
        }
        Ok(())
    })
}

/// 删除云端模型（物理删除，按 id）。
pub fn delete_cloud_model(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM models WHERE id=?1 AND is_local=0", params![id])?;
        Ok(())
    })
}

/// 按 domain + model_name + provider 查模型 id（用于前端编辑/删除）。
pub fn get_model_id(domain: &str, model_name: &str, provider: &str) -> Result<Option<i64>> {
    with_db(|conn| {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM models WHERE domain=?1 AND model_name=?2 AND provider=?3",
                params![domain, model_name, provider],
                |r| r.get(0),
            )
            .ok();
        Ok(id)
    })
}

/// 按 id 查模型的 source 和 secret_key（用于编辑时回填）。
pub fn get_model_source_key(id: i64) -> Result<(String, String)> {
    with_db(|conn| {
        conn.query_row(
            "SELECT source, secret_key FROM models WHERE id=?1",
            params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(Into::into)
    })
}

/// 按 id 查模型的 is_streaming + is_thinking（用于编辑时回填）。
pub fn get_model_flags(id: i64) -> Result<(bool, bool)> {
    with_db(|conn| {
        conn.query_row(
            "SELECT is_streaming, is_thinking FROM models WHERE id=?1",
            params![id],
            |r| Ok((r.get::<_, i32>(0)? != 0, r.get::<_, i32>(1)? != 0)),
        )
        .map_err(Into::into)
    })
}

/// 批量查 ASR 域所有模型的 id / model_name / source / secret_key / is_streaming / is_thinking。
/// 替代 N+1 的 get_model_id + get_model_source_key + get_model_flags。
/// Task 2 后补 provider/category 字段（同名不同 provider 的 ASR 模型需精确匹配）+ is_enabled。
pub struct ModelDetailRow {
    pub id: i64,
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB models.is_enabled（激活态）。供前端标 current（每域仅 1 个=1）。
    pub is_enabled: bool,
}

pub fn list_asr_model_details() -> Result<Vec<ModelDetailRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, model_name, provider, category, source, secret_key, is_streaming, is_thinking, is_enabled
             FROM models WHERE domain='asr'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ModelDetailRow {
                id: r.get(0)?,
                model_name: r.get(1)?,
                provider: r.get(2)?,
                category: r.get(3)?,
                source: r.get(4)?,
                secret_key: r.get(5)?,
                is_streaming: r.get::<_, i32>(6)? != 0,
                is_thinking: r.get::<_, i32>(7)? != 0,
                is_enabled: r.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// ASR 域全量模型行（管理列表用，不过滤 is_enabled，不分 local/cloud）。
/// 对应 `EngineInfo` 所需字段（name/provider/category/description/is_local）。
/// 与 `load_models`（过滤 is_enabled=1、按 section 分组、供推理缓存）区分：
/// 设置页/工具栏列表直查此函数，不经 RUNTIME_CONFIG 缓存，新增/编辑/删除后即时反映。
pub struct AsrEngineRow {
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub description: String,
    pub is_local: bool,
    /// Task 2 后补：DB 行 id（供前端 switch_active_model）。
    pub id: i64,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    /// DB models.is_enabled（激活态）。
    pub is_enabled: bool,
}

/// 列出 ASR 域所有模型（管理列表用）。不过滤 is_enabled，不分 local/cloud。
pub fn list_all_asr_engines() -> Result<Vec<AsrEngineRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT model_name, provider, category, description, is_local,
                    id, source, secret_key, is_streaming, is_thinking, is_enabled
             FROM models WHERE domain='asr'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AsrEngineRow {
                model_name: r.get(0)?,
                provider: r.get(1)?,
                category: r.get(2)?,
                description: r.get(3)?,
                is_local: r.get::<_, i32>(4)? != 0,
                id: r.get(5)?,
                source: r.get(6)?,
                secret_key: r.get(7)?,
                is_streaming: r.get::<_, i32>(8)? != 0,
                is_thinking: r.get::<_, i32>(9)? != 0,
                is_enabled: r.get::<_, i32>(10)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 读取 ASR 云端参考模型列表。
/// 返回 Vec<(provider, category, models_str)>，models_str 为分号分隔。
pub fn list_asr_cloud_presets() -> Result<Vec<(String, String, String)>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category='asr_cloud_model' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            // key = "provider:category"
            let parts: Vec<&str> = key.splitn(2, ':').collect();
            let provider = parts.get(0).unwrap_or(&"").to_string();
            let category = parts.get(1).unwrap_or(&"").to_string();
            Ok((provider, category, value))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 读取 LLM provider 预设 base_url。
/// LLM provider 预设（base_url + 参考模型列表）。
pub struct LlmProviderPresetRow {
    pub provider: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// 读取 LLM provider 预设。config_value 为 JSON：{"base_url":"...","models":["..."]}。
pub fn list_llm_provider_presets() -> Result<Vec<LlmProviderPresetRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category='llm_provider' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let provider: String = r.get(0)?;
            let value: String = r.get(1)?;
            // 解析 JSON {"base_url":"...","models":["..."]}
            let parsed: serde_json::Value = serde_json::from_str(&value).unwrap_or_default();
            let base_url = parsed.get("base_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let models: Vec<String> = parsed.get("models")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|m| m.as_str().map(String::from)).collect())
                .unwrap_or_default();
            Ok(LlmProviderPresetRow { provider, base_url, models })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}
///
/// `spec` 支持三种写法（见 [`parse_model_spec`]）：
/// - `"local:name"`：`is_local = true AND name`（本地 LLM，如 Ollama）
/// - `"category:name"`：`category AND name` 联合精确查询
/// - `"name"`：仅按 name 查询（向后兼容）
pub fn load_llm_model(spec: &str) -> Result<Option<CompatibleLlmConfig>> {
    with_db(|conn| load_llm_model_at(conn, spec))
}

fn load_llm_model_at(conn: &Connection, spec: &str) -> Result<Option<CompatibleLlmConfig>> {
    let parsed = parse_model_spec(spec);

    let row = match parsed {
        ModelSpec::Full { provider, category, model_name } => {
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, is_local, is_enabled
                 FROM models
                 WHERE domain='llm' AND provider=?1 AND category=?2 AND model_name=?3 AND is_available = 1",
            )?;
            let mut rows = stmt.query_map(params![provider, category, model_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })?;
            rows.next().transpose()?
        }
        ModelSpec::NameOnly(model_name) => {
            // 裸名兜底：跨 provider/category 搜 name，优先 local（ORDER BY is_local DESC）
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, is_local, is_enabled
                 FROM models
                 WHERE domain='llm' AND model_name=?1 AND is_available = 1
                 ORDER BY is_local DESC",
            )?;
            let mut rows = stmt.query_map(params![model_name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })?;
            rows.next().transpose()?
        }
    };

    let model_name = parsed.model_name();
    Ok(row.map(|(source, secret_key, is_thinking, is_local, is_enabled)| CompatibleLlmConfig {
        // Full 时取解析出的 provider；NameOnly 时为空串（仅日志用）
        provider: match parsed {
            ModelSpec::Full { provider, .. } => provider.to_string(),
            ModelSpec::NameOnly(_) => String::new(),
        },
        model: model_name.to_string(),
        base_url: source,
        secret_key,
        is_thinking: is_thinking != 0,
        is_local: is_local != 0,
        is_enabled: is_enabled != 0,
    }))
}

/// models 表的通用行（用于翻译引擎按 id 查询、激活模型查询，不限于 llm domain）。
///
/// 含全字段（含 language/description）——供 [`get_active_model`] 构造完整
/// [`ModelEntry`]（无字段缺失，4 域统一）。比 LocalAsrModelRow 更通用：不限 domain、
/// 不限 is_local。
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub id: i64,
    pub domain: String,
    pub provider: String,
    pub category: String,
    pub model_name: String,
    pub source: String,
    pub secret_key: String,
    pub language: String,
    pub description: String,
    pub is_local: bool,
    pub is_thinking: bool,
    pub is_streaming: bool,
    pub is_enabled: bool,
    pub is_available: bool,
}

/// 按 model_name 查 domain（供 model_commands 按域 reload 缓存）。
pub fn get_model_domain_by_name(model_name: &str) -> Result<Option<String>> {
    with_db(|conn| {
        let domain: Option<String> = conn.query_row(
            "SELECT domain FROM models WHERE model_name = ?1 LIMIT 1",
            params![model_name],
            |r| r.get(0),
        ).optional()?;
        Ok(domain)
    })
}

/// 按 id 查询 models 表行（不限 domain）。用于反查引擎配置。
pub fn get_model_by_id(id: i64) -> Result<Option<ModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, is_local, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], |r| model_row_mapper(r)).optional()?;
        Ok(row)
    })
}

/// 查询指定域的激活模型（is_enabled=1 且 is_available=1），每域仅一个。
/// 供 load_active_engine(domain) 使用——4 域统一激活查询。
pub fn get_active_model(domain: &str) -> Result<Option<ModelRow>> {
    with_db(|conn| get_active_model_at(conn, domain))
}

/// 查询指定域的激活模型（is_enabled=1 且 is_available=1），每域仅一个。ORDER BY id 保证确定性。

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
fn get_active_model_at(conn: &Connection, domain: &str) -> Result<Option<ModelRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain, provider, category, model_name, source, secret_key,
                language, description, is_local, is_thinking, is_streaming, is_enabled, is_available
         FROM models WHERE domain=?1 AND is_enabled=1 AND is_available=1 ORDER BY id LIMIT 1",
    )?;
    let row = stmt.query_row(params![domain], |r| model_row_mapper(r)).optional()?;
    Ok(row)
}

/// ModelRow 行映射共享闭包（get_model_by_id / get_active_model 共用，14 列顺序一致）。
fn model_row_mapper(r: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRow> {
    Ok(ModelRow {
        id: r.get(0)?,
        domain: r.get(1)?,
        provider: r.get(2)?,
        category: r.get(3)?,
        model_name: r.get(4)?,
        source: r.get(5)?,
        secret_key: r.get(6)?,
        language: r.get(7)?,
        description: r.get(8)?,
        is_local: r.get::<_, i64>(9)? != 0,
        is_thinking: r.get::<_, i64>(10)? != 0,
        is_streaming: r.get::<_, i64>(11)? != 0,
        is_enabled: r.get::<_, i64>(12)? != 0,
        is_available: r.get::<_, i64>(13)? != 0,
    })
}

/// 切换激活模型——单语句全量刷新某域的 is_enabled（仅在可用模型中切换）。
/// SQLite 用 IIF（不是 MySQL 的 IF）。每域记录不多（最多几十条），全量刷新无性能问题。
pub fn switch_active_model(domain: &str, id: i64) -> Result<()> {
    with_db(|conn| switch_active_model_at(conn, domain, id))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
///
/// SQL 语义（review fix 双激活 bug）：WHERE 覆盖两类行——
/// (a) 目标行（id=?）且 is_available=1 → 激活它
/// (b) 所有当前 is_enabled=1 的行（无论 is_available） → 清零（含残留的不可用行）
/// 这样不可用模型上残留的 is_enabled=1 也会被清理，防止「文件丢失→重新可用→双激活」。
fn switch_active_model_at(conn: &Connection, domain: &str, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE models SET is_enabled = IIF(id=?1, 1, 0) \
         WHERE domain=?2 AND ((id=?1 AND is_available=1) OR is_enabled=1)",
        params![id, domain],
    )?;
    Ok(())
}

/// 按 spec 精确查 ASR 域某可用模型（不限激活），返回完整 ModelRow。
///
/// 供 CLI `--model` 显式路径 / 多模型场景用——[`get_active_model`] 只返回激活的一个。
/// spec 支持 3-part（`provider:category:model_name`）或裸 model_name：
/// - 3-part：provider + category + model_name 精确匹配
/// - 裸名：仅按 model_name 匹配（取第一条）
pub fn get_asr_model_by_spec(provider: Option<&str>, category: Option<&str>, model_name: &str) -> Result<Option<ModelRow>> {
    with_db(|conn| get_asr_model_by_spec_at(conn, provider, category, model_name))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
fn get_asr_model_by_spec_at(conn: &Connection, provider: Option<&str>, category: Option<&str>, model_name: &str) -> Result<Option<ModelRow>> {
    const SQL_FULL: &str = "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, is_local, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain='asr' AND is_available=1 AND provider=?1 AND category=?2 AND model_name=?3 LIMIT 1";
    const SQL_NAME: &str = "SELECT id, domain, provider, category, model_name, source, secret_key,
                    language, description, is_local, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain='asr' AND is_available=1 AND model_name=?1 LIMIT 1";
    let row = match (provider, category) {
        (Some(p), Some(c)) => {
            let mut stmt = conn.prepare(SQL_FULL)?;
            stmt.query_row(params![p, c, model_name], |r| model_row_mapper(r)).optional()?
        }
        _ => {
            let mut stmt = conn.prepare(SQL_NAME)?;
            stmt.query_row(params![model_name], |r| model_row_mapper(r)).optional()?
        }
    };
    Ok(row)
}

/// LLM 模型列表项（菜单用，仅含显示与排序所需字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmModelInfo {
    pub id: i64,
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub is_local: bool,
    pub source: String,
    pub secret_key: String,
    pub is_streaming: bool,
    pub is_thinking: bool,
    pub is_enabled: bool,
}

/// 列出所有可用的 LLM 润色模型（domain='llm' AND is_available=1），按 is_local 降序、category 升序排序。
/// 管理列表用——含未激活（is_enabled=0）的可用模型。is_enabled 字段供前端标 current。
fn list_llm_models_at(conn: &Connection) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, category, model_name, is_local, source, secret_key, is_streaming, is_thinking, is_enabled FROM models
         WHERE domain='llm' AND is_available = 1
         ORDER BY is_local DESC, category",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LlmModelInfo {
            id: row.get::<_, i64>(0)?,
            provider: row.get::<_, String>(1)?,
            category: row.get::<_, String>(2)?,
            model_name: row.get::<_, String>(3)?,
            is_local: row.get::<_, i32>(4)? != 0,
            source: row.get::<_, String>(5)?,
            secret_key: row.get::<_, String>(6)?,
            is_streaming: row.get::<_, i32>(7)? != 0,
            is_thinking: row.get::<_, i32>(8)? != 0,
            is_enabled: row.get::<_, i32>(9)? != 0,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 LLM 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_llm_models() -> Result<Vec<LlmModelInfo>> {
    with_db(list_llm_models_at)
}

/// 云端模型通用列表项（不限 domain，仅 is_local=0）。供 TranslateTab 等复用 llm 风格的云端 section。
///
/// 与 [`LlmModelInfo`] 字段一致（含 id、provider、category 等），区别在于：
/// - 按 domain 参数过滤（而非写死 'llm'）
/// - 过滤 is_local=0（只列云端模型，本地走 list_local_models_by_domain）
/// - 不过滤 is_enabled（Task 1 后云端模型 insert_cloud_model 写 is_enabled=0 不自动激活；
///   此处保留 is_enabled 字段供前端标 current——用户 switch_active_model 激活后置 1）
fn list_cloud_models_by_domain_at(conn: &Connection, domain: &str) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, category, model_name, is_local, source, secret_key, is_streaming, is_thinking, is_enabled
         FROM models WHERE domain = ?1 AND is_local = 0
         ORDER BY category, model_name",
    )?;
    let rows = stmt.query_map(params![domain], |row| {
        Ok(LlmModelInfo {
            id: row.get::<_, i64>(0)?,
            provider: row.get::<_, String>(1)?,
            category: row.get::<_, String>(2)?,
            model_name: row.get::<_, String>(3)?,
            is_local: row.get::<_, i32>(4)? != 0,
            source: row.get::<_, String>(5)?,
            secret_key: row.get::<_, String>(6)?,
            is_streaming: row.get::<_, i32>(7)? != 0,
            is_thinking: row.get::<_, i32>(8)? != 0,
            is_enabled: row.get::<_, i32>(9)? != 0,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出某 domain 的云端模型（is_local=0，经 with_db）。供 Tauri 命令调用。
pub fn list_cloud_models_by_domain(domain: &str) -> Result<Vec<LlmModelInfo>> {
    with_db(|conn| list_cloud_models_by_domain_at(conn, domain))
}

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
    pub is_local: bool,
    /// Task 2 后：DB models.is_enabled（激活态，每域仅 1 个=1）。供前端标 current。
    pub is_enabled: bool,
}

/// 列出所有 OCR 模型（domain='ocr'，含未就绪的——前端列表需展示全部供下载/切换）。
fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description, is_local, is_enabled FROM models
         WHERE domain='ocr'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
            is_local: row.get::<_, i32>(2)? != 0,
            is_enabled: row.get::<_, i32>(3)? != 0,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 OCR 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_ocr_models() -> Result<Vec<OcrModelInfo>> {
    with_db(list_ocr_models_at)
}

// ── 润色提示词 CRUD（prompts 表）──

/// prompts 表记录（设置窗口 prompt 管理页用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PromptRecord {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}

const PROMPT_SELECT_COLS: &str = "id, title, content, description, is_system";

fn row_to_prompt(row: &rusqlite::Row) -> rusqlite::Result<PromptRecord> {
    Ok(PromptRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        content: row.get(2)?,
        description: row.get(3)?,
        is_system: row.get::<_, i32>(4)? != 0,
    })
}

/// 列出所有 prompt（按 is_system 降序、id 升序）。
fn list_prompts_at(conn: &Connection) -> Result<Vec<PromptRecord>> {
    let sql = format!(
        "SELECT {} FROM prompts ORDER BY is_system DESC, id ASC",
        PROMPT_SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_prompt)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn list_prompts() -> Result<Vec<PromptRecord>> {
    with_db(list_prompts_at)
}

/// 按 id 加载单条 prompt。
fn load_prompt_at(conn: &Connection, id: i64) -> Result<Option<PromptRecord>> {
    let sql = format!("SELECT {} FROM prompts WHERE id=?1", PROMPT_SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![id], row_to_prompt)?;
    Ok(rows.next().transpose()?)
}

pub fn load_prompt(id: i64) -> Result<Option<PromptRecord>> {
    with_db(|conn| load_prompt_at(conn, id))
}

/// 新建用户 prompt。返回新 id。is_system 固定 0（用户 prompt）。
fn insert_prompt_at(conn: &Connection, title: &str, content: &str, description: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO prompts (title, category, content, description, is_system)
         VALUES (?1, 'voice_text_polish', ?2, ?3, 0)",
        params![title, content, description],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_prompt(title: &str, content: &str, description: &str) -> Result<i64> {
    with_db(|conn| insert_prompt_at(conn, title, content, description))
}

/// 按 id 更新 prompt（允许 system prompt 编辑——配合「复原默认」按钮）。
/// 注意：UPDATE 语句不修改 is_system 字段，即系统/用户身份保持不变。
fn update_prompt_at(conn: &Connection, id: i64, title: &str, content: &str, description: &str) -> Result<()> {
    conn.execute(
        "UPDATE prompts SET title=?1, content=?2, description=?3, updated_at=datetime('now')
         WHERE id=?4",
        params![title, content, description, id],
    )?;
    Ok(())
}

pub fn update_prompt(id: i64, title: &str, content: &str, description: &str) -> Result<()> {
    with_db(|conn| update_prompt_at(conn, id, title, content, description))
}

/// 按 id 删除 prompt（拒绝 is_system=1）。
fn delete_prompt_at(conn: &Connection, id: i64) -> Result<()> {
    let is_system: i32 = conn
        .query_row("SELECT is_system FROM prompts WHERE id=?1", params![id], |r| r.get(0))
        .context("prompt 不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置 prompt 不可删除");
    }
    conn.execute("DELETE FROM prompts WHERE id=?1", params![id])?;
    Ok(())
}

pub fn delete_prompt(id: i64) -> Result<()> {
    with_db(|conn| delete_prompt_at(conn, id))
}

/// 读取 active_polish_prompt 配置值（字符串 id）。不存在/解析失败返回 1（fallback）。
pub fn load_active_prompt_id() -> Result<i64> {
    with_db(|conn| {
        let val: Option<String> = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='active_polish_prompt'",
                [],
                |r| r.get(0),
            )
            .ok();
        let id = val
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1);
        Ok(id)
    })
}

/// 写入 active_polish_prompt 配置值。
pub fn save_active_prompt_id(id: i64) -> Result<()> {
    save_config_key("active_polish_prompt", &id.to_string())
}

// ── Action Bar 菜单项 ──

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarItem {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub icon: String,
    pub action_type: String,
    pub action_data: String,
    pub sort_order: i64,
    pub is_system: bool,
    pub is_enabled: bool,
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
    pub shortcut: String,
    pub agent: String,
    pub accepts: String,
    pub trigger_keyword: String,
    pub global_shortcut: String,
    pub need_voice: bool,
}

const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, global_shortcut, need_voice";

fn row_to_action_bar_item(row: &rusqlite::Row) -> rusqlite::Result<ActionBarItem> {
    Ok(ActionBarItem {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        icon: row.get(3)?,
        action_type: row.get(4)?,
        action_data: row.get(5)?,
        sort_order: row.get(6)?,
        is_system: row.get::<_, i32>(7)? != 0,
        is_enabled: row.get::<_, i32>(8)? != 0,
        is_async: row.get::<_, i32>(9)? != 0,
        write_output_to_clipboard: row.get::<_, i32>(10)? != 0,
        shortcut: row.get(11)?,
        agent: row.get(12)?,
        accepts: row.get(13)?,
        trigger_keyword: row.get(14)?,
        global_shortcut: row.get(15)?,
        need_voice: row.get::<_, i32>(16)? != 0,
    })
}

/// 校验快捷键格式：空字符串或单个 0-9/a-z 字符。
pub fn validate_shortcut(shortcut: &str) -> Result<()> {
    if shortcut.is_empty() {
        return Ok(());
    }
    if shortcut.len() == 1 && shortcut.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return Ok(());
    }
    anyhow::bail!("快捷键必须为空或单个 0-9/a-z 字符");
}

/// 检查快捷键是否已被其他项占用（排除指定 id）。返回冲突项（如有）。
fn check_shortcut_conflict_at(conn: &Connection, shortcut: &str, exclude_id: Option<i64>) -> Result<Option<ActionBarItem>> {
    if shortcut.is_empty() {
        return Ok(None);
    }
    let sql = match exclude_id {
        Some(_) => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1 AND id!=?2", ACTION_BAR_SELECT_COLS),
        None => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1", ACTION_BAR_SELECT_COLS),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = match exclude_id {
        Some(eid) => stmt.query_map(params![shortcut, eid], row_to_action_bar_item)?,
        None => stmt.query_map(params![shortcut], row_to_action_bar_item)?,
    };
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 浮窗用——只返回 is_enabled=1 的项。
pub fn list_action_bar_items() -> Result<Vec<ActionBarItem>> {
    with_db(list_action_bar_items_at)
}

fn list_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items WHERE is_enabled=1 ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

/// 设置页用——返回全部项（含禁用的）。
pub fn list_all_action_bar_items() -> Result<Vec<ActionBarItem>> {
    with_db(list_all_action_bar_items_at)
}

fn list_all_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn load_action_bar_item(id: i64) -> Result<Option<ActionBarItem>> {
    with_db(|conn| load_action_bar_item_at(conn, id))
}

fn load_action_bar_item_at(conn: &Connection, id: i64) -> Result<Option<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM action_bar_items WHERE id=?1", ACTION_BAR_SELECT_COLS)
    )?;
    let mut rows = stmt.query_map(params![id], row_to_action_bar_item)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

pub fn insert_action_bar_item(
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    is_enabled: bool,
    need_voice: bool,
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, is_enabled, need_voice))
}

fn insert_action_bar_item_at(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    is_enabled: bool,
    need_voice: bool,
) -> Result<i64> {
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, None)? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    let max_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) FROM action_bar_items WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, need_voice)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?13, ?7, ?8, ?9, ?10, ?11, ?12, ?14)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, trigger_keyword, is_enabled as i32, need_voice as i32],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_action_bar_item(
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    need_voice: bool,
) -> Result<()> {
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts, trigger_keyword, need_voice))
}

fn update_action_bar_item_at(
    conn: &Connection,
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
    trigger_keyword: &str,
    need_voice: bool,
) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    if row.is_system && row.action_type != action_type {
        anyhow::bail!("系统内置菜单项不可更改动作类型");
    }
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, Some(id))? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    conn.execute(
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, shortcut=?8, agent=?9, accepts=?10, trigger_keyword=?11, need_voice=?12, updated_at=datetime('now') WHERE id=?13",
        params![title, icon, action_type, action_data, is_enabled as i32, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, trigger_keyword, need_voice as i32, id],
    )?;
    Ok(())
}

pub fn delete_action_bar_item(id: i64) -> Result<()> {
    with_db(|conn| delete_action_bar_item_at(conn, id))
}

fn delete_action_bar_item_at(conn: &Connection, id: i64) -> Result<()> {
    let is_system: i32 = conn.query_row(
        "SELECT is_system FROM action_bar_items WHERE id=?1", params![id], |r| r.get(0)
    ).context("菜单项不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置菜单项不可删除");
    }
    conn.execute("DELETE FROM action_bar_items WHERE id=?1 OR parent_id=?1", params![id])?;
    Ok(())
}

/// 设置菜单项的全局快捷键（Quick Execute silent 入口）。空串清除。
pub fn set_global_shortcut(id: i64, global_shortcut: &str) -> Result<()> {
    with_db(|conn| {
        let rows = conn.execute(
            "UPDATE action_bar_items SET global_shortcut=?1, updated_at=datetime('now') WHERE id=?2",
            params![global_shortcut, id],
        )?;
        if rows == 0 {
            anyhow::bail!("菜单项不存在: {}", id);
        }
        Ok(())
    })
}

/// 查询所有注册了全局快捷键的菜单项（is_enabled + global_shortcut 非空）。
/// 启动时和设置变更后用于注册全局快捷键。
pub fn list_action_hotkeys() -> Result<Vec<ActionBarItem>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            &format!(
                "SELECT {} FROM action_bar_items WHERE global_shortcut != '' AND is_enabled = 1",
                ACTION_BAR_SELECT_COLS
            )
        )?;
        let rows = stmt.query_map([], row_to_action_bar_item)?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

// ── Script Run（脚本执行记录）─────────────────────────────────────

// ── Launcher Index（统一启动器索引表：app + command）──────────────

/// 统一启动器索引表的一行（app + command 共用）。
///
/// - `type="app"`：应用索引缓存（来自文件系统扫描）；source 固定 `"applications"`，
///   alias 为本地化名、icon 为 base64 PNG，description/keywords 暂留空。
/// - `type="command"`：命令索引（brew/cargo/system 等）；alias/icon 留空，
///   source 为来源、description 为英文描述、keywords 为 LLM 生成的中英文关键字。
#[derive(Debug, Clone)]
pub struct LauncherRow {
    pub r#type: String,       // "app" | "command"
    pub name: String,
    pub path: String,
    pub alias: String,        // app 的本地化名，command 无
    pub icon: String,         // app 的 base64 icon，command 无
    pub source: String,       // command 的 brew/cargo/system，app 用 "applications"
    pub description: String,  // 英文描述
    pub keywords: String,     // LLM 生成的中英文关键字
}

/// 按 type 加载启动器索引行（type='app' 返回全部应用缓存，type='command' 返回命令缓存）。
pub fn load_launcher_by_type(item_type: &str) -> Result<Vec<LauncherRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT type, name, path, alias, icon, source, description, keywords
             FROM launcher_index WHERE type = ?1",
        )?;
        let rows = stmt.query_map(params![item_type], |r| Ok(LauncherRow {
            r#type: r.get(0)?,
            name: r.get(1)?,
            path: r.get(2)?,
            alias: r.get(3)?,
            icon: r.get(4)?,
            source: r.get(5)?,
            description: r.get(6)?,
            keywords: r.get(7)?,
        }))?;
        Ok(collect_rows(rows, "load_launcher_by_type"))
    })
}

/// 按 type 全量替换启动器索引（事务原子：先删该 type 再插）。
///
/// **原子性保证**：DELETE + INSERT 在同一 `unchecked_transaction` 内，
/// 中途 INSERT 失败（如磁盘满）会回滚 DELETE，避免该 type 缓存被清空
/// 导致下次启动触发全量重扫 + 期间搜索无结果。
pub fn save_launcher_batch(item_type: &str, rows: &[LauncherRow]) -> Result<()> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM launcher_index WHERE type = ?1", params![item_type])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO launcher_index
                 (type, name, path, alias, icon, source, description, keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for r in rows {
                stmt.execute(params![
                    item_type, r.name, r.path, r.alias, r.icon, r.source, r.description, r.keywords,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}

/// 更新单个启动器项的 keywords（LLM 生成关键字后调用）。
/// 按 (type, path) 定位；同时刷新 updated_at。
pub fn update_launcher_keywords(item_type: &str, path: &str, keywords: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE launcher_index SET keywords = ?3, updated_at = datetime('now')
             WHERE type = ?1 AND path = ?2",
            params![item_type, path, keywords],
        )?;
        Ok(())
    })
}

// ── App Index Cache（应用索引缓存）——launcher_index 的 app wrapper ──────────
//
// load_app_index / save_app_index 是 search crate AppIndex::scan/rescan 的契约入口。
// 保持签名不变（四元组 name/alias/path/icon），内部转 LauncherRow 读写 launcher_index
// 中 type='app' 的行——对 search crate 完全透明。

/// 从 DB 加载应用索引缓存。空表返回空 Vec（触发首次扫描）。
/// 返回 (name, alias, path, icon_base64)
pub fn load_app_index() -> Result<Vec<(String, String, String, String)>> {
    let rows = load_launcher_by_type("app")?;
    Ok(rows.into_iter().map(|r| (r.name, r.alias, r.path, r.icon)).collect())
}

/// 全量替换应用索引缓存（原子：DELETE 该 type + INSERT 在同一事务内）。
/// apps: (name, alias, path, icon_base64)
///
/// **原子性保证**：转 LauncherRow 后走 [`save_launcher_batch`]，DELETE + INSERT 同事务，
/// 中途 INSERT 失败（如磁盘满）会回滚 DELETE，避免 DB 变空表导致下次启动触发全量重扫
/// + 期间搜索无 app。
pub fn save_app_index(apps: &[(String, String, String, String)]) -> Result<()> {
    let launcher_rows: Vec<LauncherRow> = apps
        .iter()
        .map(|(name, alias, path, icon)| LauncherRow {
            r#type: "app".into(),
            name: name.clone(),
            path: path.clone(),
            alias: alias.clone(),
            icon: icon.clone(),
            source: "applications".into(),
            description: String::new(),
            keywords: String::new(),
        })
        .collect();
    save_launcher_batch("app", &launcher_rows)
}

// ── 搜索频次加权（search_frequency 表）───────────────────────────

/// 频次加权表的一行（search_frequency）。
#[derive(Debug, Clone)]
pub struct FreqRow {
    pub hit_count: i64,
    pub last_hit_ts: i64,
    pub query: String,
}

/// 记录一次搜索命中：hit_count+1，更新 query 和 last_hit_ts。
pub fn record_search_frequency(score_key: &str, query: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    with_db(|conn| {
        conn.execute(
            "INSERT INTO search_frequency (score_key, query, hit_count, last_hit_ts)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(score_key) DO UPDATE SET
                hit_count = hit_count + 1,
                query = excluded.query,
                last_hit_ts = excluded.last_hit_ts",
            params![score_key, query, now],
        )
        .with_context(|| format!("record_search_frequency key={}", score_key))?;
        Ok(())
    })
}

/// 加载所有频次记录到内存 map（key → FreqRow）。
pub fn load_search_frequency() -> Result<HashMap<String, FreqRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT score_key, hit_count, last_hit_ts, query FROM search_frequency",
        )
        .context("load_search_frequency")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FreqRow {
                    hit_count: r.get::<_, i64>(1)?,
                    last_hit_ts: r.get::<_, i64>(2)?,
                    query: r.get::<_, String>(3)?,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRun {
    pub id: i64,
    pub item_id: i64,
    pub item_title: Option<String>,
    pub script_type: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_msg: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}

/// stdout/stderr 截断上限（64KB）
const SCRIPT_OUTPUT_LIMIT: usize = 65536;

pub fn insert_script_run(
    item_id: i64,
    script_type: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    error_msg: &str,
    started_at: &str,
    finished_at: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<i64> {
    let stdout_trunc: String = stdout.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    let stderr_trunc: String = stderr.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    with_db(|conn| {
        conn.execute(
            "INSERT INTO script_runs (item_id, script_type, exit_code, stdout, stderr, error_msg, started_at, finished_at, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item_id, script_type, exit_code, stdout_trunc, stderr_trunc, error_msg, started_at, finished_at, duration_ms],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<ScriptRun>> {
    with_db(|conn| {
        let limit = limit.unwrap_or(100);
        let sql = if item_id.is_some() {
            "SELECT s.id, s.item_id, COALESCE(a.title, '已删除'), s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             WHERE s.item_id = ?2 ORDER BY s.started_at DESC LIMIT ?1"
        } else {
            "SELECT s.id, s.item_id, a.title, s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             ORDER BY s.started_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(iid) = item_id {
            stmt.query_map(params![limit, iid], row_to_script_run)?
        } else {
            stmt.query_map(params![limit], row_to_script_run)?
        };
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    })
}

fn row_to_script_run(row: &rusqlite::Row) -> rusqlite::Result<ScriptRun> {
    Ok(ScriptRun {
        id: row.get(0)?,
        item_id: row.get(1)?,
        item_title: row.get(2).ok(),
        script_type: row.get(3)?,
        exit_code: row.get(4)?,
        stdout: row.get(5)?,
        stderr: row.get(6)?,
        error_msg: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
        duration_ms: row.get(10)?,
    })
}

pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<()> {
    let keep = keep_recent.unwrap_or(100);
    with_db(|conn| {
        conn.execute(
            "DELETE FROM script_runs WHERE id NOT IN (SELECT id FROM script_runs ORDER BY started_at DESC LIMIT ?1)",
            params![keep],
        )?;
        Ok(())
    })
}

/// 按 ID 批量删除执行记录。2026-07-17 新增——执行记录 TAB 的复选框删除。
pub fn delete_script_runs(ids: &[i64]) -> Result<()> {
    if ids.is_empty() { return Ok(()); }
    with_db(|conn| {
        // 逐条 DELETE（IDs 数量有限，100 条上限不需 IN 子句优化）
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM script_runs WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    })
}

// ── Agent Adapter（agent 适配器：内置 + 用户自定义）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapterRecord {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
    pub is_system: bool,
    pub is_default: bool,
}

const AGENT_ADAPTER_SELECT_COLS: &str = "id, key, display_name, detect_binary, command_template, is_system, is_default";

pub fn list_agent_adapter_records() -> Result<Vec<AgentAdapterRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            &format!("SELECT {} FROM agent_adapters ORDER BY is_system DESC, id ASC", AGENT_ADAPTER_SELECT_COLS)
        )?;
        let rows = stmt.query_map([], |r| Ok(AgentAdapterRecord {
            id: r.get(0)?,
            key: r.get(1)?,
            display_name: r.get(2)?,
            detect_binary: r.get(3)?,
            command_template: r.get(4)?,
            is_system: r.get::<_, i32>(5)? != 0,
            is_default: r.get::<_, i32>(6)? != 0,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

/// 按 key 查单条 adapter。
pub fn load_agent_adapter_by_key(key: &str) -> Result<Option<AgentAdapterRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            &format!("SELECT {} FROM agent_adapters WHERE key=?1", AGENT_ADAPTER_SELECT_COLS)
        )?;
        let mut rows = stmt.query_map(params![key], |r| Ok(AgentAdapterRecord {
            id: r.get(0)?,
            key: r.get(1)?,
            display_name: r.get(2)?,
            detect_binary: r.get(3)?,
            command_template: r.get(4)?,
            is_system: r.get::<_, i32>(5)? != 0,
            is_default: r.get::<_, i32>(6)? != 0,
        }))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

pub fn insert_agent_adapter_record(
    key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<i64> {
    with_db(|conn| {
        // 用户自建项 is_system=0；is_default 由 set_default_agent 单独管
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template, is_system, is_default) VALUES (?1, ?2, ?3, ?4, 0, 0)",
            params![key, display_name, detect_binary, command_template],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 设为默认 agent（全局唯一）。先把全部置 0，再把目标置 1。
pub fn set_default_agent(id: i64) -> Result<()> {
    with_db(|conn| set_default_agent_at(conn, id))
}

/// 接裸连接版本（供测试用）。
fn set_default_agent_at(conn: &Connection, id: i64) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0)
    )?;
    if exists == 0 {
        anyhow::bail!("agent adapter id={} 不存在", id);
    }
    conn.execute("UPDATE agent_adapters SET is_default=0", [])?;
    conn.execute("UPDATE agent_adapters SET is_default=1 WHERE id=?1", params![id])?;
    Ok(())
}

/// 清除默认（无默认 agent；菜单 agent='' 时将走 fallback 到「第一个可用」）。
pub fn clear_default_agent() -> Result<()> {
    with_db(clear_default_agent_at)
}

/// 接裸连接版本（供测试用）。
fn clear_default_agent_at(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE agent_adapters SET is_default=0", [])?;
    Ok(())
}

pub fn update_agent_adapter_record(
    id: i64, key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_adapters SET key=?1, display_name=?2, detect_binary=?3, command_template=?4, updated_at=datetime('now') WHERE id=?5",
            params![key, display_name, detect_binary, command_template, id],
        )?;
        Ok(())
    })
}

pub fn delete_agent_adapter_record(id: i64) -> Result<()> {
    with_db(|conn| delete_agent_adapter_record_at(conn, id))
}

/// 接裸连接版本（供测试用）。
fn delete_agent_adapter_record_at(conn: &Connection, id: i64) -> Result<()> {
    // 内置不可删（与 update 对称保护）
    let is_system: i32 = conn.query_row(
        "SELECT is_system FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0)
    ).context("agent adapter 不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置 agent 不可删除");
    }
    conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id])?;
    Ok(())
}

// ── Agent Task（agent × 语音识别联动）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub status: String,
    pub agent_key: String,
    pub context: String,
    pub transcribed_text: String,
    pub error_msg: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn insert_agent_task(id: &str, agent_key: &str, context: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO agent_tasks (id, status, agent_key, context) VALUES (?1, 'pending', ?2, ?3)",
            params![id, agent_key, context],
        )?;
        Ok(())
    })
}

pub fn load_agent_task(id: &str) -> Result<Option<AgentTask>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks WHERE id=?1"
        )?;
        let mut rows = stmt.query_map(params![id], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

pub fn update_agent_task_result(id: &str, transcribed_text: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET transcribed_text=?1, status='executing', updated_at=datetime('now') WHERE id=?2",
            params![transcribed_text, id],
        )?;
        Ok(())
    })
}

pub fn update_agent_task_status(id: &str, status: &str, error_msg: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET status=?1, error_msg=?2, updated_at=datetime('now') WHERE id=?3",
            params![status, error_msg, id],
        )?;
        Ok(())
    })
}

pub fn list_agent_tasks(limit: i64) -> Result<Vec<AgentTask>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks ORDER BY created_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn delete_agent_task(id: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM agent_tasks WHERE id=?1", params![id])?;
        Ok(())
    })
}

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
    with_db(|conn| list_hotword_sets_at(conn))
}

fn list_hotword_sets_at(conn: &Connection) -> Result<Vec<HotwordSet>> {
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
    with_db(|conn| get_hotword_set_at(conn, id))
}

fn get_hotword_set_at(conn: &Connection, id: &str) -> Result<HotwordSet> {
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
    with_db(|conn| insert_hotword_set_at(conn, id, name))
}

fn insert_hotword_set_at(conn: &Connection, id: &str, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_sets (id, name) VALUES (?1, ?2)",
        params![id, name],
    )?;
    Ok(())
}

/// 改名。同时刷新 updated_at。
pub fn rename_hotword_set(id: &str, name: &str) -> Result<()> {
    with_db(|conn| rename_hotword_set_at(conn, id, name))
}

fn rename_hotword_set_at(conn: &Connection, id: &str, name: &str) -> Result<()> {
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
    with_db(|conn| toggle_hotword_set_at(conn, id, enabled))
}

fn toggle_hotword_set_at(conn: &Connection, id: &str, enabled: bool) -> Result<()> {
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
    with_db(|conn| delete_hotword_set_at(conn, id))
}

fn delete_hotword_set_at(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 覆盖写 words_text（已 normalize）。导入「覆盖」模式用。
pub fn set_hotword_set_words(id: &str, words_text: &str) -> Result<()> {
    with_db(|conn| {
        let normalized = crate::hotword_text::normalize_words_text(words_text);
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
    with_db(|conn| add_word_to_set_at(conn, id, word))
}

fn add_word_to_set_at(conn: &Connection, id: &str, word: &str) -> Result<bool> {
    let cur: String = conn
        .query_row(
            "SELECT words_text FROM hotword_sets WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
    let merged = format!("{} {}", cur, word);
    let normalized = crate::hotword_text::normalize_words_text(&merged);
    let added = normalized != cur;
    conn.execute(
        "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
        params![normalized, id],
    )?;
    Ok(added)
}

/// 批量追加多词（挖掘/导入追加用），返回实际新增条数。
pub fn add_words_to_set(id: &str, words: &[String]) -> Result<usize> {
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
    with_db(|conn| remove_word_from_set_at(conn, id, word))
}

fn remove_word_from_set_at(conn: &Connection, id: &str, word: &str) -> Result<()> {
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
    with_db(|conn| upsert_hotword_set_at(conn, h))
}

fn upsert_hotword_set_at(conn: &Connection, h: &HotwordSet) -> Result<()> {
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
    with_db(|conn| list_active_hotword_words_at(conn))
}

fn list_active_hotword_words_at(conn: &Connection) -> Result<Vec<String>> {
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
/// **INV-C1（热词来源不断）**：故意不过滤 `deleted_at`——软删内容仍是热词来源，
/// 这是剪贴板软删/回收站功能的核心目的。用户把文本删进回收站后，这里仍能读到它，
/// 热词挖掘继续工作。只有永久删除（`DELETE FROM`）才会让行真正消失、挖不到。
/// `ORDER BY id DESC LIMIT N` 降序取最新 N 条，软删内容 id 不变（软删只改 deleted_at），
/// 活跃和软删混在同一条时间线，不会互相挤占名额。
pub fn list_recent_text(limit: i64) -> Result<Vec<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM clipboard_history
             WHERE item_type IN ('voice','text','ocr') AND content IS NOT NULL AND content != ''
             -- 故意不过滤 deleted_at（INV-C1：软删内容仍是热词来源）
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
    with_db(|conn| bump_hotword_hit_by_word_at(conn, word))
}

fn bump_hotword_hit_by_word_at(conn: &Connection, word: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_hits(word, hit_count) VALUES(?1, 1) \
         ON CONFLICT(word) DO UPDATE SET hit_count = hit_count + 1",
        params![word],
    )?;
    Ok(())
}

/// 全局命中计数（前端卡片命中展示用）。返回 word → hit_count。
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>> {
    with_db(|conn| list_hotword_hits_at(conn))
}

fn list_hotword_hits_at(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT word, hit_count FROM hotword_hits")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for r in rows {
        let (w, c) = r?;
        map.insert(w, c);
    }
    Ok(map)
}

/// direction < 0 = 上移，> 0 = 下移。交换同 parent 下相邻项的 sort_order。
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<()> {
    with_db(|conn| move_action_bar_item_at(conn, id, direction))
}

fn move_action_bar_item_at(conn: &Connection, id: i64, direction: i32) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;

    let neighbor_id: Option<i64> = if direction < 0 {
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order < ?2 ORDER BY sort_order DESC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    } else {
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order > ?2 ORDER BY sort_order ASC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    };

    if let Some(nid) = neighbor_id {
        let neighbor = load_action_bar_item_at(conn, nid)?.context("相邻项不存在")?;
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![neighbor.sort_order, id])?;
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![row.sort_order, nid])?;
    }
    Ok(())
}

// ── 识别历史写入（desktop coordinator 用）──

/// 首次有 ASR 文本时插入（应用写入毫秒戳 id）。
/// `text` = finish_text 扁平（落 content 列）；
/// `segments` = transcript.segments_json()（段 JSON 真相源）。
/// 新 schema：写入 clipboard_history（item_type='voice'），meta_info JSON 存 engine/engine_mode/char_count。
pub fn insert_transcription_at_id(
    id: i64,
    text: &str,
    segments: &str,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
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
pub fn update_text_segments(id: i64, text: &str, segments: &str) -> Result<()> {
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

/// 停顿润色后更新 polish_status/polish_model + segments/text 列。
/// `text` = 润色后扁平（与 segments 段拼接一致）；`segments` = segments_json（润色后段，Polished/Edited）。
/// 新 schema：UPDATE clipboard_history content + segments + meta_info（polished/polish_model）。
pub fn update_polished(
    id: i64,
    polish_status: &str,
    polish_model: Option<&str>,
    segments: &str,
    text: &str,
) -> Result<()> {
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
pub fn update_edited_segments(id: i64, text: &str, segments: &str) -> Result<()> {
    with_db(|conn| {
        update_edited_segments_at(conn, id, text, segments)?;
        Ok(())
    })
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。返回实际更新的行数。
fn update_edited_segments_at(
    conn: &Connection,
    id: i64,
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
    id: i64,
    text: &str,
    segments: &str,
    polish_status: &str,
    polish_model: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
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
pub struct TranscriptionRecord {
    pub id: i64,
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
    with_db(|conn| list_transcriptions_search_at(conn, limit, offset, search))
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。
/// search = None / "" → 全列；>=3 字符走 FTS5 MATCH（倒排索引）；<3 字符回退 LIKE（trigram 无法生成 3-gram）。
fn list_transcriptions_search_at(
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
        let select_cols = "SELECT id, created_at,
                COALESCE(json_extract(meta_info, '$.engine'), '') as engine,
                CASE WHEN json_extract(meta_info, '$.polished') = 1 THEN 'done' ELSE 'off' END as polish_status,
                CAST(json_extract(meta_info, '$.duration_ms') AS INTEGER) as duration_ms,
                segments, content
         FROM clipboard_history";

        if q.chars().count() >= 3 {
            // FTS5 MATCH：trigram tokenizer 对 >=3 字符生成 3-gram 做倒排索引查找（子串语义）
            let escaped = escape_fts5_match(q);
            let mut stmt = conn.prepare(&format!(
                "{select_cols}
                 WHERE item_type = 'voice'
                   AND id IN (SELECT rowid FROM clipboard_history_fts
                              WHERE clipboard_history_fts MATCH ?1)
                 ORDER BY id DESC LIMIT ?2 OFFSET ?3"
            ))?;
            let rows = stmt.query_map(params![escaped, limit, offset], row_mapper)?;
            return Ok(collect_rows(rows, "fts5 search"));
        }
        // <3 字符回退 LIKE：trigram 无法生成 3-gram，MATCH 会无结果
        let pattern = format!("%{}%", q);
        let mut stmt = conn.prepare(&format!(
            "{select_cols}
             WHERE item_type = 'voice' AND content LIKE ?1
             ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![pattern, limit, offset], row_mapper)?;
        return Ok(collect_rows(rows, "like search"));
    }
    list_transcriptions_at(conn, limit, offset)
}

/// 转义 FTS5 MATCH 查询：用双引号包裹为 phrase，内部双引号双写。
/// trigram tokenizer 对 phrase 做连续 3-gram 匹配，语义等价子串匹配。
fn escape_fts5_match(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// 批量删除识别记录（按 id）。返回实际删除的行数。
/// 新 schema：DELETE FROM clipboard_history WHERE id IN (...)。
pub fn delete_transcriptions(ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    with_db(|conn| delete_transcriptions_at(conn, ids))
}

fn delete_transcriptions_at(conn: &Connection, ids: &[i64]) -> Result<usize> {
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

fn list_transcriptions_at(
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
         ORDER BY id DESC LIMIT ?1 OFFSET ?2"
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


// ── 时间戳工具（避免依赖 chrono）──

/// 当前时间字符串 'YYYY-MM-DD HH:MM:SS'。
fn now_string() -> String {
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

// ============================================================================
// Password Vault 模型（schema v38，2026-07-18）
// ============================================================================
//
// 双层 API 模式（同 ActionBarItem）：
// - 公开 `with_db` 包装函数（业务层调用，单连接线程安全）
// - 私有 `_at` 内部函数（接 `&Connection`，便于测试和复用）

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultMeta {
    pub id: i64,
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultCipher {
    pub id: String, // UUID v4 字符串（2026-07-21 v39：支持 git 同步）
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
    pub deleted_at: Option<String>,
    pub sync_md5: Option<String>, // md5 内容指纹（v45：增量同步 diff，详见 vault::sync::fingerprint）
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultFolder {
    pub id: String, // UUID v4 字符串
    pub name: String,
    pub sort_order: i64,
    pub sync_md5: Option<String>, // md5 内容指纹（v45）
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct VaultMetaInput {
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VaultCipherInput {
    pub id: String, // UUID v4 字符串——调用方生成（不再 AUTOINCREMENT）
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
    /// md5 内容指纹（v45：增量同步 diff，由调用方算好传入）。
    /// None 表示调用方未算（向后兼容旧调用方），sync 时按需重算。
    pub sync_md5: Option<String>,
}

const VAULT_META_COLS: &str = "id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism, \
                               protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp, \
                               equivalent_domains, public_key, protected_private_key, created_at, updated_at";

const VAULT_CIPHER_COLS: &str = "id, folder_id, favorite, atype, name, notes, data, fields, \
                                 password_history, reprompt, deleted_at, sync_md5, created_at, updated_at";

fn row_to_vault_meta(row: &rusqlite::Row) -> rusqlite::Result<VaultMeta> {
    Ok(VaultMeta {
        id: row.get(0)?,
        kdf_type: row.get(1)?,
        kdf_salt: row.get(2)?,
        kdf_iterations: row.get(3)?,
        kdf_memory_kib: row.get(4)?,
        kdf_parallelism: row.get(5)?,
        protected_user_vault_key: row.get(6)?,
        app_key_local_enc: row.get(7)?,
        app_key_sync_enc: row.get(8)?,
        security_stamp: row.get(9)?,
        equivalent_domains: row.get(10)?,
        public_key: row.get(11)?,
        protected_private_key: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn row_to_vault_cipher(row: &rusqlite::Row) -> rusqlite::Result<VaultCipher> {
    Ok(VaultCipher {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        favorite: row.get::<_, i32>(2)? != 0,
        atype: row.get(3)?,
        name: row.get(4)?,
        notes: row.get(5)?,
        data: row.get(6)?,
        fields: row.get(7)?,
        password_history: row.get(8)?,
        reprompt: row.get(9)?,
        deleted_at: row.get(10)?,
        sync_md5: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

// ── vault_meta CRUD ──

pub fn load_vault_meta() -> Result<Option<VaultMeta>> {
    with_db(load_vault_meta_at)
}

fn load_vault_meta_at(conn: &Connection) -> Result<Option<VaultMeta>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_meta WHERE id = 1", VAULT_META_COLS))?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_vault_meta(row)?)),
        None => Ok(None),
    }
}

pub fn upsert_vault_meta(input: &VaultMetaInput) -> Result<()> {
    with_db(|conn| upsert_vault_meta_at(conn, input))
}

fn upsert_vault_meta_at(conn: &Connection, input: &VaultMetaInput) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                  protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp,
                                  equivalent_domains, public_key, protected_private_key)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
            kdf_type = excluded.kdf_type,
            kdf_salt = excluded.kdf_salt,
            kdf_iterations = excluded.kdf_iterations,
            kdf_memory_kib = excluded.kdf_memory_kib,
            kdf_parallelism = excluded.kdf_parallelism,
            protected_user_vault_key = excluded.protected_user_vault_key,
            app_key_local_enc = excluded.app_key_local_enc,
            app_key_sync_enc = excluded.app_key_sync_enc,
            security_stamp = excluded.security_stamp,
            equivalent_domains = excluded.equivalent_domains,
            public_key = excluded.public_key,
            protected_private_key = excluded.protected_private_key,
            updated_at = datetime('now')",
        params![
            input.kdf_type,
            input.kdf_salt,
            input.kdf_iterations,
            input.kdf_memory_kib,
            input.kdf_parallelism,
            input.protected_user_vault_key,
            input.app_key_local_enc,
            input.app_key_sync_enc,
            input.security_stamp,
            input.equivalent_domains,
            input.public_key,
            input.protected_private_key,
        ],
    )?;
    Ok(())
}

pub fn update_vault_security_stamp(stamp: &str) -> Result<()> {
    with_db(|conn| update_vault_security_stamp_at(conn, stamp))
}

fn update_vault_security_stamp_at(conn: &Connection, stamp: &str) -> Result<()> {
    conn.execute(
        "UPDATE vault_meta SET security_stamp = ?1, updated_at = datetime('now') WHERE id = 1",
        params![stamp],
    )?;
    Ok(())
}

/// 删除 vault_meta 单行（id=1）。
///
/// ⚠️ **危险操作 / 卫生约束（第六轮审查）**：此函数语义是「销毁整个 vault 元数据」，
/// 正常流程**绝不**调用。唯一合法调用方是 `octopus_vault::unlock::setup_vault`
/// 在迁移失败时的显式回滚路径（A1 修复，第五轮审查）。
///
/// `#[doc(hidden)]` 是语言层软约束——vault 是独立 crate，必须 `pub` 才能跨 crate
/// 调用，无法降为 `pub(crate)`。hidden 标记让该函数不出现在 rustdoc / IDE 自动补全
/// 中，防其他模块（或误用）发现后调用清空 vault。未来若 vault 收编回 infra 或
/// 出现其他合法调用方，可重新评估可见性。
///
/// 背景语义：setup 流程先把 vault_meta INSERT 落盘（独立 commit），再触发
/// `migrate_secret_keys_to_encrypted`。若迁移失败，旧实现会让 vault_meta
/// 已初始化 + secret_key 仍全明文 + `ensure!(!is_initialized())` 阻止重跑 →
/// 不可恢复的「已初始化但部分明文」状态。此函数让 setup 失败路径能显式清掉
/// vault_meta，让 `is_initialized()` 回到 false，用户可重新走 setup。
#[doc(hidden)]
pub fn delete_vault_meta_row() -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM vault_meta WHERE id = 1", [])?;
        Ok(())
    })
}

// ── vault_ciphers CRUD ──

pub fn list_vault_ciphers() -> Result<Vec<VaultCipher>> {
    with_db(list_vault_ciphers_at)
}

fn list_vault_ciphers_at(conn: &Connection) -> Result<Vec<VaultCipher>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM vault_ciphers ORDER BY updated_at DESC",
        VAULT_CIPHER_COLS
    ))?;
    let rows = stmt.query_map([], row_to_vault_cipher)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn load_vault_cipher(id: &str) -> Result<Option<VaultCipher>> {
    with_db(|conn| load_vault_cipher_at(conn, id))
}

fn load_vault_cipher_at(conn: &Connection, id: &str) -> Result<Option<VaultCipher>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_ciphers WHERE id = ?1", VAULT_CIPHER_COLS))?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_vault_cipher(row)?)),
        None => Ok(None),
    }
}

/// 查所有软删 cipher 的 id（deleted_at IS NOT NULL），轻量查询——不解密、不读字段，
/// 仅供 `vault_empty_trash` 批量永久删除用。
pub fn list_trash_cipher_ids() -> Result<Vec<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM vault_ciphers WHERE deleted_at IS NOT NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    })
}

pub fn insert_vault_cipher(input: &VaultCipherInput) -> Result<()> {
    with_db(|conn| insert_vault_cipher_at(conn, input))
}

fn insert_vault_cipher_at(conn: &Connection, input: &VaultCipherInput) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_ciphers (id, folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt, sync_md5)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            input.id,
            input.folder_id,
            input.favorite as i32,
            input.atype,
            input.name,
            input.notes,
            input.data,
            input.fields,
            input.password_history,
            input.reprompt,
            input.sync_md5,
        ],
    )?;
    Ok(())
}

pub fn update_vault_cipher(id: &str, input: &VaultCipherInput) -> Result<()> {
    with_db(|conn| update_vault_cipher_at(conn, id, input))
}

fn update_vault_cipher_at(conn: &Connection, id: &str, input: &VaultCipherInput) -> Result<()> {
    conn.execute(
        "UPDATE vault_ciphers SET
            folder_id = ?1, favorite = ?2, atype = ?3, name = ?4, notes = ?5, data = ?6,
            fields = ?7, password_history = ?8, reprompt = ?9, sync_md5 = ?10, updated_at = datetime('now')
         WHERE id = ?11",
        params![
            input.folder_id,
            input.favorite as i32,
            input.atype,
            input.name,
            input.notes,
            input.data,
            input.fields,
            input.password_history,
            input.reprompt,
            input.sync_md5,
            id,
        ],
    )?;
    Ok(())
}

pub fn soft_delete_vault_cipher(id: &str) -> Result<()> {
    with_db(|conn| soft_delete_vault_cipher_at(conn, id))
}

fn soft_delete_vault_cipher_at(conn: &Connection, id: &str) -> Result<()> {
    // deleted_at 变 → md5 必须重算（调用方应在 soft_delete 后调 update_cipher_sync_md5）
    conn.execute(
        "UPDATE vault_ciphers SET deleted_at = datetime('now') WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

pub fn restore_vault_cipher(id: &str) -> Result<()> {
    with_db(|conn| restore_vault_cipher_at(conn, id))
}

fn restore_vault_cipher_at(conn: &Connection, id: &str) -> Result<()> {
    // deleted_at 变 → md5 必须重算（调用方应在 restore 后调 update_cipher_sync_md5）
    conn.execute(
        "UPDATE vault_ciphers SET deleted_at = NULL WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// 仅更新 sync_md5（soft_delete / restore 后 cipher 内容变了，md5 需重算）。
/// 调用方先 SELECT 拿到完整 row → 算 md5 → 调本函数写回。
pub fn update_cipher_sync_md5(id: &str, sync_md5: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_ciphers SET sync_md5 = ?1 WHERE id = ?2",
            params![sync_md5, id],
        )?;
        Ok(())
    })
}

pub fn permanent_delete_vault_cipher(id: &str) -> Result<()> {
    with_db(|conn| permanent_delete_vault_cipher_at(conn, id))
}

fn permanent_delete_vault_cipher_at(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM vault_ciphers WHERE id = ?1", params![id])?;
    Ok(())
}

// ── vault_folders CRUD ──

pub fn list_vault_folders() -> Result<Vec<VaultFolder>> {
    with_db(list_vault_folders_at)
}

fn list_vault_folders_at(conn: &Connection) -> Result<Vec<VaultFolder>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_order, sync_md5, created_at, updated_at FROM vault_folders ORDER BY sort_order ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VaultFolder {
            id: row.get(0)?,
            name: row.get(1)?,
            sort_order: row.get(2)?,
            sync_md5: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 创建 folder（调用方生成 UUID）。
/// 2026-07-21 v39：id 从 AUTOINCREMENT 改 UUID 字符串（git 同步）。
pub fn insert_vault_folder(id: &str, name: &str, sync_md5: &str) -> Result<()> {
    with_db(|conn| insert_vault_folder_at(conn, id, name, sync_md5))
}

fn insert_vault_folder_at(conn: &Connection, id: &str, name: &str, sync_md5: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_folders (id, name, sync_md5) VALUES (?1, ?2, ?3)",
        params![id, name, sync_md5],
    )?;
    Ok(())
}

/// 重命名 folder（参数应是已用 user_vault_key.encrypt 加密过的密文）。
///
/// follow-up #6：folder.name 与 cipher.name 一致存密文；调用方负责加解密。
/// sync_md5 由调用方算好传入（name 变 → md5 变）。
pub fn update_vault_folder_name(id: &str, new_name_encrypted: &str, sync_md5: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_folders SET name = ?1, sync_md5 = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![new_name_encrypted, sync_md5, id],
        )?;
        Ok(())
    })
}

/// 删除 folder。FK 配置 `ON DELETE SET NULL`——本表内的 cipher 不受影响，
/// 仅其 folder_id 被置为 NULL（条目回到根目录）。
///
/// follow-up #6。
pub fn delete_vault_folder(id: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM vault_folders WHERE id = ?1", params![id])?;
        Ok(())
    })
}

/// 返回所有需要迁移的 model：(id, 明文 secret_key)。
/// 仅 is_local=0 且不以 v1: 开头的行。
pub fn list_models_for_secret_migration() -> Result<Vec<(i64, String)>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, secret_key FROM models WHERE is_local = 0 AND secret_key != '' AND secret_key NOT LIKE 'v1:%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 更新指定 model 的 secret_key 字段。
///
/// 注意：models 表无 `updated_at` 列（与 prompts / vault_meta 等带时间戳的表不同）。
/// 早期版本误把 `updated_at = datetime('now')` 写进 UPDATE，导致整个迁移路径抛
/// "no such column: updated_at"——调用方（unlock.rs setup_vault）虽 catch 不阻塞，
/// 但用户的明文 secret_key 永远迁不掉。Wave 2 测试发现此 bug 后修复：仅更新 secret_key。
pub fn update_model_secret_key(model_id: i64, new_secret_key: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE models SET secret_key = ? WHERE id = ?",
            rusqlite::params![new_secret_key, model_id],
        )?;
        Ok(())
    })
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

    #[test]
    fn action_bar_shortcut_validate_and_conflict() {
        let conn = open_init();

        // 给 id=2（翻译）设快捷键 't'
        conn.execute("UPDATE action_bar_items SET shortcut='t' WHERE id=2", []).unwrap();

        // validate_shortcut: 合法
        assert!(validate_shortcut("").is_ok());
        assert!(validate_shortcut("t").is_ok());
        assert!(validate_shortcut("5").is_ok());
        // validate_shortcut: 非法
        assert!(validate_shortcut("T").is_err());  // 大写
        assert!(validate_shortcut("ab").is_err()); // 多字符
        assert!(validate_shortcut("-").is_err());  // 非法字符
        assert!(validate_shortcut(" ").is_err());  // 空格

        // check_shortcut_conflict: 't' 已被 id=2 占用
        let conflict = check_shortcut_conflict_at(&conn, "t", Some(5)).unwrap();
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().id, 2);

        // 排除自身——id=2 查 't' 不应冲突
        let self_ok = check_shortcut_conflict_at(&conn, "t", Some(2)).unwrap();
        assert!(self_ok.is_none());

        // 无冲突字符
        let free = check_shortcut_conflict_at(&conn, "z", None).unwrap();
        assert!(free.is_none());
    }

    #[test]
    fn action_bar_insert_with_shortcut() {
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "测试", "", "url", "", true, false, "q", "", "text", "", true, false,
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.shortcut, "q");
    }

    #[test]
    fn action_bar_update_shortcut() {
        let conn = open_init();
        update_action_bar_item_at(
            &conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "p", "", "text", "", false,
        ).unwrap();
        let item = load_action_bar_item_at(&conn, 5).unwrap().unwrap();
        assert_eq!(item.shortcut, "p");
    }

    #[test]
    fn action_bar_shortcut_conflict_rejected() {
        let conn = open_init();
        // id=2 设快捷键 't'
        update_action_bar_item_at(&conn, 2, "翻译", "globe", "ai", "auto_translate", true, true, false, "t", "", "text", "", false).unwrap();
        // id=5 也想用 't' → 应失败
        let result = update_action_bar_item_at(&conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "t", "", "text", "", false);
        assert!(result.is_err());
    }

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

    /// `get_model_by_id` 按 id 反查 models 行——translate_engine 配置链路核心。
    /// seed 后 opus-mt 是 translate domain 的本地模型，先取它的 DB id 再反查，
    /// 不假设自增起始值（避免被前面的 seed 行数变化打穿）。
    #[test]
    fn get_model_by_id_returns_translate_row() {
        setup_test_db();
        // list_local_models_by_domain 现在也返回 id（DB 行 id），直接用，无需再反查。
        let local = list_local_models_by_domain("translate").unwrap();
        let first = local
            .iter()
            .find(|r| r.model_name == "opus-mt")
            .expect("seed 应有 opus-mt 本地翻译模型");
        let id = first.id;
        let got = get_model_by_id(id).unwrap().expect("应查到 id 对应的行");
        assert_eq!(got.id, id);
        assert_eq!(got.domain, "translate");
        assert_eq!(got.model_name, "opus-mt");
        assert!(got.is_local);
        // opus-mt seed 的 provider/category 固定
        assert_eq!(got.provider, "local");
        assert_eq!(got.category, "opus-mt");
        // list_local_models_by_domain 与 get_model_by_id 的 is_enabled 取值一致（都不过滤）
        assert_eq!(got.is_enabled, first.is_enabled);
    }

    /// `get_model_by_id` 查不存在的 id 应返回 None（optional() 路径）。
    #[test]
    fn get_model_by_id_missing_returns_none() {
        setup_test_db();
        let got = get_model_by_id(9_999_999).unwrap();
        assert!(got.is_none(), "不存在的 id 应返回 None");
    }

    /// AppConfig 全字段 DB 往返：save → load 必须完整还原每个字段。
    /// 这是 serde 自动 load/save 的回归守卫——新增字段后若遗漏注册（旧手动枚举的坑），
    /// 此测试会因该字段回到 default 而失败。历史踩坑 4 次，见 archived specs 2026-06-28。
    #[test]
    fn app_config_roundtrip_all_fields() {
        use crate::config::{AppConfig, PolishMode};
        let conn = open_init();

        let mut cfg = AppConfig::default();
        // 每个字段设一个与 default 不同的哨兵值
        cfg.engine_mode = "websocket".into();
        cfg.remote_url = "http://rt:9999".into();
        cfg.grpc_endpoint = "http://grpc:50051".into();
        cfg.language = "en".into();
        cfg.asr_shortcut = "Alt+1".into();
        cfg.paste_method = "direct".into();
        cfg.write_to_clipboard = false;
        cfg.switch_input_source_on_paste = false;
        cfg.microphone = "Sentinel Mic".into();
        cfg.segment_silence = 1234.5;
        cfg.overlay_position = "bottom".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.polish_min_interval = 7.5;
        cfg.pause_polish_threshold_ms = 999.0;
        cfg.asr_hardware_accelerated = false;
        cfg.asr_correct = false;
        cfg.output_simplified = false;
        cfg.hide_toolbar = false;
        cfg.denoise_mode = 2;
        cfg.edit_shortcut = "Alt+2".into();
        cfg.edit_global_shortcut = "Alt+3".into();
        cfg.polish_global_shortcut = "Alt+4".into();
        cfg.download_mirror = "https://mirror.test".into();
        cfg.clipboard_shortcut = "Alt+5".into();
        cfg.clipboard_max_items = 42;
        cfg.clipboard_max_age_days = 7;
        cfg.clipboard_enabled = false;
        cfg.screenshot_shortcut = "Alt+6".into();

        save_app_config_at(&conn, &cfg).unwrap();
        let loaded = load_app_config_at(&conn).unwrap();

        // Debug 格式全比较——任何字段未往返都会暴露差异。
        assert_eq!(format!("{:?}", loaded), format!("{:?}", cfg));
    }

    #[test]
    fn init_schema_fresh_db_builds_v43() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 47, "全新库 init_schema 后应到 v47");
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

    #[test]
    fn action_bar_items_has_agent_and_accepts_cols() {
        let conn = open_init();
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"agent".to_string()), "missing agent column: {:?}", cols);
        assert!(cols.contains(&"accepts".to_string()), "missing accepts column: {:?}", cols);
    }

    #[test]
    fn agent_adapters_table_exists() {
        let conn = open_init();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_adapters'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "agent_adapters table should exist");
    }

    #[test]
    fn action_bar_item_has_agent_and_accepts_fields() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order)
             VALUES (NULL, '测试agent', 'bot', 'agent', '{{task}}', 'claude', 'file', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.agent, "claude");
        assert_eq!(item.accepts, "file");
        assert_eq!(item.action_type, "agent");
    }

    #[test]
    fn action_bar_item_has_trigger_keyword() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, trigger_keyword, sort_order)
             VALUES (NULL, 'Quicklink测试', 'link', 'url', 'https://example.com/?q={query}', 'ql', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.trigger_keyword, "ql");
    }

    #[test]
    fn action_bar_trigger_keyword_defaults_empty() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order)
             VALUES (NULL, '普通菜单', 'bot', 'script', 'echo hi', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.trigger_keyword, "");
    }

    #[test]
    fn agent_adapter_crud_roundtrip() {
        let conn = open_init();
        // v42 起 db.sql seed 内置 Pi + Claude（2 行），用 WHERE 过滤到测试项验证 CRUD
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('myagent', 'My Agent', 'myagent-bin', 'myagent {prompt}')",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();

        let row: (String, String, String, String) = conn.query_row(
            "SELECT key, display_name, detect_binary, command_template FROM agent_adapters WHERE id=?1",
            params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(row.0, "myagent");
        assert_eq!(row.3, "myagent {prompt}");

        conn.execute(
            "UPDATE agent_adapters SET key='myagent2', display_name='My Agent 2', detect_binary='myagent2-bin', command_template='myagent2 {prompt} {files}' WHERE id=?1",
            params![id],
        ).unwrap();
        let updated_key: String = conn.query_row(
            "SELECT key FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(updated_key, "myagent2");

        conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id]).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agent_adapters WHERE key='myagent2'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "删除后该 key 不存在");
    }

    #[test]
    fn agent_adapter_duplicate_key_rejected() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('dup', 'A', 'a-bin', 'a {prompt}')",
            [],
        ).unwrap();
        // 同 key 再插 → UNIQUE 约束拒绝
        let result = conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('dup', 'B', 'b-bin', 'b {prompt}')",
            [],
        );
        assert!(result.is_err(), "duplicate key should be rejected");
    }

    /// v42 seed：Pi + Claude 应自动入表，is_system=1。
    #[test]
    fn agent_adapter_seed_inserts_builtin_pi_claude() {
        let conn = open_init();
        let claude: (i64, String) = conn.query_row(
            "SELECT is_system, command_template FROM agent_adapters WHERE key='claude'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(claude.0, 1, "claude is_system 应为 1");
        assert_eq!(claude.1, "claude --add-dir {cwd} {prompt}");

        let pi: (i64, i64) = conn.query_row(
            "SELECT is_system, is_default FROM agent_adapters WHERE key='pi'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(pi.0, 1, "pi is_system 应为 1");
        assert_eq!(pi.1, 1, "pi 默认 is_default=1（PPT 菜单等场景的兜底）");
    }

    /// set_default_agent 必须保证全局唯一（先清零再置 1）。
    #[test]
    fn set_default_agent_is_mutually_exclusive() {
        let conn = open_init();
        // 先插一个用户自定义 agent
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('custom', 'Custom', 'custom-bin', 'custom {prompt}')",
            [],
        ).unwrap();
        let custom_id: i64 = conn.query_row(
            "SELECT id FROM agent_adapters WHERE key='custom'", [], |r| r.get(0),
        ).unwrap();

        // 初始：pi 是 default
        let pi_default: i64 = conn.query_row(
            "SELECT is_default FROM agent_adapters WHERE key='pi'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(pi_default, 1);

        // 设 custom 为 default
        set_default_agent_at(&conn, custom_id).unwrap();
        let defaults: Vec<String> = conn.prepare(
            "SELECT key FROM agent_adapters WHERE is_default=1"
        ).unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(defaults.len(), 1, "全局只能有 1 个 default");
        assert_eq!(defaults[0], "custom");
    }

    /// clear_default_agent 把所有 is_default 置 0。
    #[test]
    fn clear_default_agent_zeroes_all() {
        let conn = open_init();
        clear_default_agent_at(&conn).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agent_adapters WHERE is_default=1", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    /// 内置 agent（is_system=1）不可删除。
    #[test]
    fn delete_agent_adapter_rejects_system() {
        let conn = open_init();
        let result = delete_agent_adapter_record_at(&conn, 1);  // id=1 是 claude（首条 seed）
        assert!(result.is_err(), "内置 agent 删除应被拒绝");
    }

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

    #[test]
    fn action_bar_non_submenu_accepts_default_text() {
        // db.sql 中非 submenu 类型 seed 项的 accepts 为 'text'（列默认值）。
        // 排除 v40/v43 外置 seed 注入的 Agent 子菜单（action_type='agent', accepts='file'）——
        // 它们有独立测试覆盖。
        let conn = open_init();
        let non_submenu: Vec<(String, String)> = conn.prepare(
            "SELECT action_type, accepts FROM action_bar_items
             WHERE action_type != 'submenu'
               AND title NOT IN ('PPT 大纲', 'PPT 制作') ORDER BY id"
        ).unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert!(non_submenu.len() > 0, "seed 应有非 submenu 项");
        for (atype, accepts) in &non_submenu {
            assert_eq!(accepts, "text", "{} 类型 accepts 应为 'text'，实际: {}", atype, accepts);
        }
    }

    /// 回归 S2：save_app_index 全量替换语义 + launcher_index wrapper 正确性。
    /// v36：save_app_index 是 launcher_index 的 wrapper（转 LauncherRow 后走
    /// save_launcher_batch）。原 v34 测试断言 UNIQUE 冲突回滚，但 v36 的 save_launcher_batch
    /// 用 INSERT OR REPLACE（按 brief），同 path 不再报错而是去重覆盖——故回归目标改为：
    /// (1) wrapper 经 launcher_index 正确写入（type='app'、source='applications'）；
    /// (2) 全量替换语义——新批次完全取代旧批次（DELETE 该 type + INSERT 在同事务），
    ///     旧 App1 应消失，新批次应用就位，不残留。
    #[test]
    fn save_app_index_atomic_on_failure() {
        setup_test_db();
        // 先写入 1 个合法应用
        save_app_index(&[("App1".into(), "应用1".into(), "/Applications/App1.app".into(), "icon1".into())]).unwrap();
        let count: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(count, 1, "初始应有 1 条记录");

        // 全量替换为 2 个新应用（App1 不在新批次中 → 应被 DELETE 清掉）
        save_app_index(&[
            ("App2".into(), "应用2".into(), "/Applications/App2.app".into(), "icon2".into()),
            ("App3".into(), "应用3".into(), "/Applications/App3.app".into(), "icon3".into()),
        ]).unwrap();

        // 关键断言：全量替换——App1 应消失，新批次 2 条就位
        let count: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(count, 2, "全量替换后应有 2 条新记录，旧 App1 已清");
        let has_app1: i64 = with_db(|c| c.query_row("SELECT COUNT(*) FROM launcher_index WHERE type='app' AND name='App1'", [], |r| r.get(0)).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(has_app1, 0, "旧 App1 应被全量替换清除");

        // wrapper 字段映射正确：source='applications'、alias/icon 透传
        let (source, alias, icon): (String, String, String) = with_db(|c| c.query_row(
            "SELECT source, alias, icon FROM launcher_index WHERE type='app' AND name='App2'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).map_err(anyhow::Error::from)).unwrap();
        assert_eq!(source, "applications", "app wrapper 应填 source='applications'");
        assert_eq!(alias, "应用2");
        assert_eq!(icon, "icon2");

        // load_app_index wrapper 读回（经 load_launcher_by_type("app") → 四元组）
        let loaded = load_app_index().unwrap();
        assert_eq!(loaded.len(), 2, "load_app_index 应返回 2 条");
    }

    // 历史 v36 自愈 + v36→v37 语义迁移测试已删除（v40 schema 重整，迁移分支移除）：
    // - v36_self_heal_when_launcher_missing_but_version_set
    // - migration_v36_to_v37_migrates_is_enabled_semantics_and_clears_activation
    // 这些迁移只在 v17→v37 旧库升级路径上有意义；新 schema 全部由 db.sql + 外置 seed
    // 覆盖（launcher_index / models.is_available / models.is_enabled 均在 db.sql）。
    // switch_active_model 的核心不变量（每域仅 1 个 is_enabled=1）由下列测试覆盖。

    /// 回归 Issue #7（code review）：switch_active_model 用 id=-1（LLM「不选择模型」）
    /// 应清空该域所有 is_enabled（前端 LlmTab.tsx 传 -1 表示取消激活）。
    /// 依赖 SQLite AUTOINCREMENT 永不产生负 id（IIF(id=-1,1,0) 无匹配行，全部置 0）。
    #[test]
    fn switch_active_model_with_id_neg1_clears_domain() {
        let conn = open_init();
        // 先激活 sensevoice（is_available=1）
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", sv).unwrap();
        assert!(get_active_model_at(&conn, "asr").unwrap().is_some(),
            "测试前提：先激活一个模型");

        // 用 id=-1 调 switch_active_model（前端 LLM「不选择模型」语义）
        switch_active_model_at(&conn, "asr", -1).unwrap();

        // 验证：该域无任何 is_enabled=1 AND is_available=1
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none(),
            "id=-1 应清空该域所有激活（IIF(id=-1,1,0) 无匹配，全置 0）");

        // 原 sensevoice 的 is_enabled 应为 0（被清空）
        let sv_enabled: i64 = conn.query_row(
            "SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)
        ).unwrap();
        assert_eq!(sv_enabled, 0, "原激活模型应被清空");
    }

    /// 回归：v32-vintage 库（已有 action_bar_items 但仅缺其他表）经 init_schema 升到 v40，
    /// db.sql CREATE TABLE IF NOT EXISTS 把所有缺表补齐。验证 v40 schema 完整性。
    ///
    /// 历史 v32→v34/v35/v36 的迁移逻辑已删除（schema 由 db.sql 统一覆盖），本测试只保留
    /// 升级到 v40 的 smoke check：launcher_index / search_frequency 表存在，列齐全。
    #[test]
    fn migration_v32_db_upgrades_to_v40() {
        // 模拟 v32 库：有 action_bar_items（v32 schema）但无 app_index / launcher_index
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE action_bar_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT, parent_id INTEGER DEFAULT NULL,
                title TEXT NOT NULL, icon TEXT NOT NULL DEFAULT '', action_type TEXT NOT NULL,
                action_data TEXT NOT NULL DEFAULT '', accepts TEXT NOT NULL DEFAULT 'text',
                sort_order INTEGER NOT NULL DEFAULT 0, is_system INTEGER NOT NULL DEFAULT 1,
                is_enabled INTEGER NOT NULL DEFAULT 1, is_async INTEGER NOT NULL DEFAULT 1,
                write_output_to_clipboard INTEGER NOT NULL DEFAULT 0, shortcut TEXT NOT NULL DEFAULT '',
                agent TEXT NOT NULL DEFAULT '', trigger_keyword TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
            );"
        ).unwrap();
        conn.execute("PRAGMA user_version = 32", []).unwrap();

        // 运行迁移——v ≥ 17 分支：db.sql 全表 CREATE IF NOT EXISTS + 外置 seed
        init_schema(&conn).unwrap();

        // 验证 user_version = 43
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 47);

        // v40：launcher_index 表存在 + icon/path/alias/type 列（db.sql 提供）
        let table_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='launcher_index'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(table_count, 1, "launcher_index 表应被创建");

        let cols: Vec<String> = conn.prepare("PRAGMA table_info(launcher_index)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"icon".to_string()), "launcher_index 应有 icon 列");
        assert!(cols.contains(&"path".to_string()), "launcher_index 应有 path 列");
        assert!(cols.contains(&"alias".to_string()), "launcher_index 应有 alias 列");
        assert!(cols.contains(&"type".to_string()), "launcher_index 应有 type 列");

        // search_frequency 表存在
        let sf_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='search_frequency'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(sf_count, 1, "search_frequency 表应存在");
    }

    #[test]
    fn action_bar_insert_agent_type_default_accepts() {
        // 通过 insert 插入 agent 类型——不传 accepts 时默认 'text'
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "我的agent", "bot", "agent", "{{task}}", true, false, "", "claude", "file", "", true, false,
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.accepts, "file");
        assert_eq!(item.agent, "claude");
    }

    #[test]
    fn migration_v26_to_v27_creates_agent_tasks_table() {
        // 旧 v26→v27 迁移已删（schema 由 db.sql 统一覆盖）；本测试降级为 smoke check：
        // DROP agent_tasks 后 init_schema 应靠 db.sql CREATE IF NOT EXISTS 重建表。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DROP TABLE agent_tasks", []).unwrap();
        conn.execute("PRAGMA user_version = 26", []).unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 47);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_tasks'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn agent_task_crud_roundtrip() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES ('test-1', 'claude', '{\"kind\":\"files\"}')",
            [],
        ).unwrap();
        let row: Vec<(String, String)> = conn.prepare(
            "SELECT status, agent_key FROM agent_tasks WHERE id='test-1'"
        ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(row[0].0, "pending");
        assert_eq!(row[0].1, "claude");
        conn.execute("UPDATE agent_tasks SET transcribed_text='hello', status='executing' WHERE id='test-1'", []).unwrap();
        let text: String = conn.query_row("SELECT transcribed_text FROM agent_tasks WHERE id='test-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(text, "hello");
        conn.execute("DELETE FROM agent_tasks WHERE id='test-1'", []).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_tasks", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn agent_task_lifecycle_pending_to_done() {
        let conn = open_init();
        // 创建 task（pending）
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES ('life-1', 'claude', '{\"kind\":\"files\",\"files\":[\"/a\"]}')",
            [],
        ).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='life-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "pending");

        // 录音回调 → executing
        conn.execute("UPDATE agent_tasks SET transcribed_text='帮我整理', status='executing', updated_at=datetime('now') WHERE id='life-1'", []).unwrap();
        let (status, text): (String, String) = conn.query_row(
            "SELECT status, transcribed_text FROM agent_tasks WHERE id='life-1'", [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!(status, "executing");
        assert_eq!(text, "帮我整理");

        // 执行完成 → done
        conn.execute("UPDATE agent_tasks SET status='done', updated_at=datetime('now') WHERE id='life-1'", []).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='life-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "done");

        // 清理
        conn.execute("DELETE FROM agent_tasks WHERE id='life-1'", []).unwrap();
    }

    #[test]
    fn agent_task_lifecycle_pending_to_failed() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('life-2', 'pi')", []).unwrap();
        // 空识别 → failed
        conn.execute("UPDATE agent_tasks SET status='failed', error_msg='识别结果为空', updated_at=datetime('now') WHERE id='life-2'", []).unwrap();
        let (status, err): (String, String) = conn.query_row(
            "SELECT status, error_msg FROM agent_tasks WHERE id='life-2'", [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!(status, "failed");
        assert_eq!(err, "识别结果为空");
        conn.execute("DELETE FROM agent_tasks WHERE id='life-2'", []).unwrap();
    }

    #[test]
    fn agent_task_context_json_storage() {
        let conn = open_init();
        let complex_context = r#"{"kind":"files","files":["/a/b.pdf","/c d/e.pdf"],"cwd":"/Users/x","prompt_template":"{{task}}\n\n{{files}}"}"#;
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES (?1, ?2, ?3)",
            params!["ctx-1", "claude", complex_context],
        ).unwrap();
        let stored: String = conn.query_row("SELECT context FROM agent_tasks WHERE id='ctx-1'", [], |r| r.get(0)).unwrap();
        // JSON 往返无损
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["files"][0], "/a/b.pdf");
        assert_eq!(parsed["files"][1], "/c d/e.pdf");
        assert_eq!(parsed["cwd"], "/Users/x");
        assert_eq!(parsed["prompt_template"], "{{task}}\n\n{{files}}");
        conn.execute("DELETE FROM agent_tasks WHERE id='ctx-1'", []).unwrap();
    }

    #[test]
    fn agent_task_list_ordered_by_created_at_desc() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('old', 'claude')", []).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('new', 'pi')", []).unwrap();
        let ids: Vec<String> = conn.prepare(
            "SELECT id FROM agent_tasks ORDER BY created_at DESC"
        ).unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "new"); // 新的在前
        assert_eq!(ids[1], "old");
    }

    #[test]
    fn agent_task_default_status_is_pending() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('def-1', 'claude')", []).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='def-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn agent_task_default_context_is_empty_json() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('def-2', 'claude')", []).unwrap();
        let context: String = conn.query_row("SELECT context FROM agent_tasks WHERE id='def-2'", [], |r| r.get(0)).unwrap();
        assert_eq!(context, "{}");
    }

    #[test]
    fn init_schema_v27_db_upgrades_to_v40() {
        // v27 库再调 init_schema 应靠 v ≥ 17 分支升到 v40（不报错）
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 27", []).unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 47);
    }

    /// 用户实际升级路径：v38 → v40 应正确加载外置 seed，且保护用户已编辑的 prompt。
    #[test]
    fn migration_v38_to_v40_loads_external_seeds_and_preserves_user_edits() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // 模拟 v38 旧库：prompts 表已存在 id=1 行（v38 的 db.sql 内联种子，
        // v40 已迁出为外置 seed）。用户在此基础上编辑过。
        conn.execute(
            "INSERT INTO prompts (id, title, category, content, description, is_system)
             VALUES (1, '默认润色', 'voice_text_polish', 'v38 原始内容', '', 1)",
            [],
        ).unwrap();
        // 模拟用户在 v38 时已编辑 prompt id=1
        conn.execute(
            "UPDATE prompts SET content='用户改的 prompt 内容' WHERE id=1",
            [],
        ).unwrap();
        // 标 v38（用户当前状态）
        conn.execute("PRAGMA user_version = 38", []).unwrap();
        // 运行 init_schema（升级路径）
        init_schema(&conn).unwrap();
        // 验证升到 v43
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 47);
        // 验证 Agent 主菜单 + PPT 子菜单创建
        let agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(agent_count, 1, "v38→v40 升级时应创建 Agent 主菜单");
        // v43: Agent 下应有两个子菜单——PPT 大纲 + PPT 制作
        for title in ["PPT 大纲", "PPT 制作"] {
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM action_bar_items WHERE title=?1",
                    rusqlite::params![title],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 1, "v38→v40 升级时应创建「{}」子菜单", title);
        }
        // 验证用户编辑保留（INSERT OR IGNORE 保护）
        let prompt_content: String = conn
            .query_row("SELECT content FROM prompts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            prompt_content, "用户改的 prompt 内容",
            "用户已编辑的 prompt 应保留（INSERT OR IGNORE）"
        );
    }

    /// v42→v43 升级路径：用户 DB 已是 v42（有「制作 PPT」老菜单），重启 octopus 后
    /// 应自动：(1) bump 到 v43；(2) 改名「制作 PPT」→「PPT 制作」；(3) 新增「PPT 大纲」。
    /// row id 不变（保快捷键）。这是 v43 的核心保证——不删 ~/.octopus/octopus.db 重建也能生效。
    #[test]
    fn migration_v42_to_v43_renames_and_adds_ppt_outline() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // 模拟 v42 状态：先跑一次完整 init_schema 让它升到 v43（拿到「PPT 制作」+「PPT 大纲」），
        // 然后手工把它「倒回」v42 状态——删掉 PPT 大纲、把 PPT 制作改回老标题、user_version=42。
        init_schema(&conn).unwrap();
        let _agent_id: i64 = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        // 记下 PPT 制作 row id（应该是改名前的「制作 PPT」对应行）
        let ppt_make_id: i64 = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='PPT 制作'", [], |r| r.get(0))
            .unwrap();
        // 删 PPT 大纲（模拟 v42 时还不存在）
        conn.execute("DELETE FROM action_bar_items WHERE title='PPT 大纲'", []).unwrap();
        // 把 PPT 制作改回老标题（模拟 v42 残留）
        conn.execute(
            "UPDATE action_bar_items SET title='制作 PPT' WHERE id=?1",
            rusqlite::params![ppt_make_id],
        ).unwrap();
        // 倒回 v42
        conn.execute("PRAGMA user_version = 42", []).unwrap();

        // 验证模拟成功——v42 状态：只有「制作 PPT」，没有「PPT 大纲」
        let legacy_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(legacy_count, 1);
        let outline_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='PPT 大纲'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outline_count, 0, "测试前置：v42 状态下不应有 PPT 大纲");

        // 现在跑 init_schema（模拟用户重启 octopus）
        init_schema(&conn).unwrap();

        // 验证升到 v43
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 47);

        // 「制作 PPT」应消失
        let after_legacy: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_legacy, 0, "v43 升级后老标题应消失");

        // 「PPT 制作」存在且 row id 不变
        let (after_id,): (i64,) = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='PPT 制作'", [], |r| Ok((r.get(0)?,)))
            .unwrap();
        assert_eq!(after_id, ppt_make_id, "PPT 制作 row id 应保持不变（保快捷键）");

        // 「PPT 大纲」被新增
        let after_outline: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='PPT 大纲'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_outline, 1, "v43 升级后应新增 PPT 大纲子菜单");
    }

    /// 已是 v40 的库再次调 init_schema 应是 no-op——不重读 seed 文件、不重复插入。
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

    /// HotwordSet 全 CRUD 往返：建 → 列 → 重名冲突 → 改名 → 启停 →
    /// 单词追加（去重 + normalize 拼音首字母排序）→ 单词移除 → 删版本。
    #[test]
    fn hotword_set_crud_roundtrip() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
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
        let conn = rusqlite::Connection::open_in_memory().unwrap();
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
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        let sets = list_hotword_sets_at(&conn).unwrap();
        let general = sets.iter().find(|s| s.name == "通用").expect("应有「通用」seed");
        assert_eq!(
            general.id, "00000000-0000-0000-0000-000000000001",
            "「通用」版本必须用固定 UUID，保证跨设备 sync 时 id 一致"
        );
    }

    /// v45→v46 迁移：hotword_sets.id INTEGER→TEXT UUID + sync_md5 字段。
    /// 模拟 v45 库（INTEGER id），跑迁移后验证：id 变 TEXT + sync_md5 列存在 + 数据保留。
    #[test]
    fn migrate_v45_to_v46_hotword_id_to_uuid() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 建 v45 schema 的 hotword_sets（INTEGER id，无 sync_md5）
        conn.execute_batch(
            "CREATE TABLE hotword_sets (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL UNIQUE,
                enabled     INTEGER NOT NULL DEFAULT 1,
                words_text  TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO hotword_sets (name, enabled, words_text) VALUES ('通用', 1, '苹果');
            INSERT INTO hotword_sets (name, enabled, words_text) VALUES ('工作', 1, '会议');
            PRAGMA user_version = 45;",
        ).unwrap();

        // 跑迁移
        init_schema(&conn).unwrap();

        // 验证 user_version = 47（v46→v47 迁移加了 clipboard_history.deleted_at）
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 47);

        // id 类型应为 TEXT
        let id_type: String = conn
            .query_row(
                "SELECT type FROM pragma_table_info('hotword_sets') WHERE name = 'id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(id_type, "TEXT", "迁移后 id 应为 TEXT");

        // sync_md5 列应存在
        let has_md5: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('hotword_sets') WHERE name = 'sync_md5'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_md5, "迁移后应有 sync_md5 列");

        // 数据保留（2 条 + 迁移不丢）
        let sets = list_hotword_sets_at(&conn).unwrap();
        assert_eq!(sets.len(), 2, "迁移不应丢数据");
        // 「通用」应有固定 UUID
        let general = sets.iter().find(|s| s.name == "通用").unwrap();
        assert_eq!(general.id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(general.words_text, "苹果");
        // sync_md5 迁移后为 NULL（首次 sync 时填）
        assert!(general.sync_md5.is_none());

        // 「工作」应是随机 UUID（非固定）
        let work = sets.iter().find(|s| s.name == "工作").unwrap();
        assert_eq!(work.words_text, "会议");
        assert_ne!(work.id, "00000000-0000-0000-0000-000000000001", "非「通用」应有独立 UUID");
    }

    #[test]
    fn init_sql_is_idempotent() {
        let conn = open_init();
        conn.execute_batch(INIT_SQL).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM models WHERE domain='asr'", [], |r| r.get(0))
            .unwrap();
        // v31: 13 local ASR only (cloud models removed from seed)
        assert_eq!(count, 13);
    }

    #[test]
    fn seed_then_load_round_trips() {
        let conn = open_init();
        // 新语义：is_enabled=激活（每域仅 1），is_available=可用。
        // 激活 zipformer 模型（先置 available，再置 enabled）
        conn.execute("UPDATE models SET is_available = 1 WHERE model_name='zipformer' AND domain='asr'", []).unwrap();
        conn.execute("UPDATE models SET is_enabled = IIF(model_name='zipformer', 1, 0) WHERE domain='asr' AND is_available=1", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        // load_models_at 只返回激活的那一个（LIMIT 1）
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section（激活的）");
        assert_eq!(zf.len(), 1);
        let zp = zf.get("zipformer").unwrap();
        assert_eq!(zp.source, "asr/zipformer");
        assert!(zp.is_local, "ASR 模型应为本地模型");
        assert!(zp.is_available, "激活模型应 is_available=true");
        assert!(zp.is_streaming, "Zipformer 模型应支持流式");
        // 非激活的 section 不应出现
        assert!(cfg.asr.whisper.is_none(), "whisper 未激活不应出现");
        assert!(cfg.asr.paraformer.is_none(), "paraformer 未激活不应出现");
    }

    #[test]
    fn test_load_llm_model() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据（is_available=1 表示可用）
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_thinking, is_local, is_available)
             VALUES
             ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','GLM-4 FlashX',0,0,1),
             ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','DeepSeek V4 Flash',1,0,1),
             ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','DeepSeek via DashScope',1,0,1),
             ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Plus',0,0,1),
             ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','GLM-4.5 Flash',1,0,1)"
        ).unwrap();

        // 3-part：bigmodel:glm:glm-4-flashx
        let glm = load_llm_model_at(&conn, "bigmodel:glm:glm-4-flashx")
            .unwrap()
            .unwrap();
        assert_eq!(glm.provider, "bigmodel");
        assert_eq!(glm.model, "glm-4-flashx");
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert!(!glm.is_thinking, "glm-4-flashx 不是思考模型");

        // deepseek-v4-flash 在 deepseek 和 aliyun 两个 provider 下同名
        let ds = load_llm_model_at(&conn, "deepseek:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(ds.provider, "deepseek");
        assert!(ds.is_thinking);

        let aliyun = load_llm_model_at(&conn, "aliyun:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(aliyun.provider, "aliyun");
        assert!(aliyun.is_thinking);

        // provider 不匹配时应返回 None
        assert!(
            load_llm_model_at(&conn, "deepseek:qwen:qwen-plus")
                .unwrap()
                .is_none(),
            "deepseek 下不存在 qwen:qwen-plus"
        );

        let glm_think = load_llm_model_at(&conn, "bigmodel:glm:glm-4.5-flash")
            .unwrap()
            .unwrap();
        assert!(glm_think.is_thinking);

        // 裸名（NameOnly）
        let bare = load_llm_model_at(&conn, "glm-4-flashx").unwrap().unwrap();
        assert_eq!(bare.model, "glm-4-flashx");

        assert!(load_llm_model_at(&conn, "nonexistent-model").unwrap().is_none());

        // 插入 is_local=1 的 LLM 行，验证精确命中
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_local, is_available)
             VALUES ('llm', 'ollama', 'qwen', 'qwen3-8b', 'http://localhost:11434/v1', 'local ollama', 1, 1)",
            [],
        )
        .unwrap();
        let local_llm = load_llm_model_at(&conn, "ollama:qwen:qwen3-8b").unwrap().unwrap();
        assert_eq!(local_llm.provider, "ollama");
        assert!(local_llm.is_local);
    }

    #[test]
    fn parse_model_spec_variants() {
        // 3-part → Full
        assert_eq!(
            parse_model_spec("bigmodel:glm:glm-4-flashx"),
            ModelSpec::Full { provider: "bigmodel", category: "glm", model_name: "glm-4-flashx" }
        );
        // 裸名 → NameOnly
        assert_eq!(parse_model_spec("bare-name"), ModelSpec::NameOnly("bare-name"));
        // 2-part（旧格式）→ warn + NameOnly 兜底（用整串作为裸名）
        assert_eq!(parse_model_spec("bigmodel:glm-4-flashx"), ModelSpec::NameOnly("bigmodel:glm-4-flashx"));
    }

    #[test]
    fn model_spec_name_strips_prefix() {
        assert_eq!(
            ModelSpec::Full { provider: "p", category: "c", model_name: "foo" }.model_name(),
            "foo"
        );
        assert_eq!(ModelSpec::NameOnly("baz").model_name(), "baz");
    }

    #[test]
    fn test_is_enabled_filtering() {
        let conn = open_init();

        conn.execute("UPDATE models SET is_enabled = 0 WHERE model_name = 'glm-4-flashx'", []).unwrap();
        assert!(load_llm_model_at(&conn, "bigmodel:glm:glm-4-flashx").unwrap().is_none());
        // 裸名也应查不到（唯一匹配的那条被禁用了）
        assert!(load_llm_model_at(&conn, "glm-4-flashx").unwrap().is_none());

        conn.execute("UPDATE models SET is_enabled = 0 WHERE model_name = 'paraformer-streaming'", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert!(cfg.asr.paraformer.is_none() || !cfg.asr.paraformer.unwrap().contains_key("paraformer-streaming"));
    }

    #[test]
    fn list_all_local_asr_models_includes_disabled() {
        let conn = open_init();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        // seed 里本地 ASR 全 is_enabled=0，load_models_at 会过滤，本函数应保留
        let names: Vec<&str> = rows.iter().map(|r| r.model_name.as_str()).collect();
        assert!(names.contains(&"paraformer-streaming"), "未过滤 is_enabled=0");
        assert!(rows.iter().any(|r| !r.is_enabled), "应含未就绪模型");
        // c796cbc 后兜底 zipformer-small-ctc 移出 seed，本地模型 source 全是 HF repo id；
        // 验证列出全部 13 条本地 ASR，无 models/ 开头的随包行
        assert_eq!(rows.len(), 13, "本地 ASR 清单应含 13 条");
        assert!(rows.iter().all(|r| r.source.contains('/')), "本地 source 均为 HF repo id 形式");
    }

    #[test]
    fn set_model_available_persists() {
        let conn = open_init();
        set_model_available_at(&conn, "paraformer-streaming", true).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(p.is_available);
        // 关掉再读
        set_model_available_at(&conn, "paraformer-streaming", false).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(!p.is_available);
    }

    #[test]
    fn set_model_secret_key_persists() {
        let conn = open_init();
        let json = r#"{"files":[{"path":"a.onnx","sha256":"abc","size":10}]}"#;
        set_model_secret_key_at(&conn, "paraformer-streaming", json).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert_eq!(p.secret_key, json);
    }

    // ── 模型激活语义（Task 1-2 引入）单测 ──
    // 不变量来源：specs/2026-07-17-model-activation-refactor-design.md §3.3 / §6 / §7

    /// §7 降级路径：无激活模型（is_enabled 全 0）时 get_active_model 返回 None。
    /// seed 默认所有模型 is_enabled=0（用户激活时才设 1），故全新库应返回 None。
    #[test]
    fn get_active_model_returns_none_when_no_active() {
        let conn = open_init();
        // seed 里所有 asr 模型 is_enabled=0
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none());
        assert!(get_active_model_at(&conn, "llm").unwrap().is_none());
        assert!(get_active_model_at(&conn, "ocr").unwrap().is_none());
        assert!(get_active_model_at(&conn, "translate").unwrap().is_none());
    }

    /// §3.3 + §6.1：激活查询 WHERE domain=? AND is_enabled=1 AND is_available=1。
    /// 仅 is_enabled=1 不够——必须 is_available=1（文件未就绪的激活模型不算）。
    #[test]
    fn get_active_model_requires_both_enabled_and_available() {
        let conn = open_init();
        // 找一个 is_available=1 的 ASR 模型（sensevoice-orig-small）并激活
        let row: (i64,) = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        let sid = row.0;
        switch_active_model_at(&conn, "asr", sid).unwrap();

        // 命中：is_enabled=1 AND is_available=1
        let active = get_active_model_at(&conn, "asr").unwrap().expect("应命中激活模型");
        assert_eq!(active.id, sid);
        assert_eq!(active.model_name, "sensevoice-orig-small");
        assert_eq!(active.domain, "asr");
        // §6.1 推理正确性：完整字段（source/secret_key/model_name 与 DB 一致）
        assert!(active.source.starts_with("asr/"));
        assert_eq!(active.is_available, true);
        assert_eq!(active.is_enabled, true);

        // 反例：手动设一个 is_enabled=1 AND is_available=0 的行 → 不应命中
        conn.execute(
            "UPDATE models SET is_enabled=1, is_available=0 WHERE model_name='paraformer-streaming'",
            [],
        ).unwrap();
        // 仍应返回 sensevoice（is_available=1 的那个），不返回 paraformer-streaming
        let active2 = get_active_model_at(&conn, "asr").unwrap().expect("仍应命中 sensevoice");
        assert_eq!(active2.model_name, "sensevoice-orig-small");
    }

    /// §6.3 事务性切换：switch_active_model 单 UPDATE 原子刷新——切换后该域仅 1 个 is_enabled=1。
    /// IIF(id=?,1,0) 语义：目标行置 1，其余可用行置 0。
    #[test]
    fn switch_active_model_atomic_single_active_per_domain() {
        let conn = open_init();
        // 两个 is_available=1 的 ASR 模型：sensevoice-orig-small + firered-asr2
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();
        assert_ne!(sv, fr, "测试前提：两模型 id 不同");

        // 先激活 sensevoice
        switch_active_model_at(&conn, "asr", sv).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1, "切换后该域应仅 1 个 is_enabled=1 AND is_available=1");
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, sv);

        // 切换到 firered——sensevoice 应自动 is_enabled=0
        switch_active_model_at(&conn, "asr", fr).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM models WHERE domain='asr' AND is_enabled=1 AND is_available=1",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1, "再切换后仍仅 1 个激活");
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, fr, "激活应已切到 firered");
    }

    /// §6.3 边界：切到不可用模型时清空该域激活——
    /// SQL WHERE 覆盖 (id=? AND is_available=1) OR is_enabled=1，
    /// 不可用目标行不满足前者 → 不激活；所有 is_enabled=1 行被清零。
    #[test]
    fn switch_active_model_clears_domain_when_target_not_available() {
        let conn = open_init();
        // paraformer-streaming is_available=0（未就绪）
        let ps: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='paraformer-streaming'",
            [], |r| r.get(0)
        ).unwrap();
        // 先激活 sensevoice（is_available=1）确认初始有激活
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", sv).unwrap();
        assert!(get_active_model_at(&conn, "asr").unwrap().is_some());

        // 切到 paraformer-streaming（is_available=0）——不满足 (id=? AND is_available=1)
        switch_active_model_at(&conn, "asr", ps).unwrap();
        // sensevoice 在 is_enabled=1 范围内 → 被清零 → 无激活
        assert!(get_active_model_at(&conn, "asr").unwrap().is_none(),
            "切到未就绪模型应清空该域激活");
        // paraformer-streaming 本身 is_enabled 仍 0（不在 WHERE 范围——既不满足激活条件也非 is_enabled=1）
        let ps_enabled: i64 = conn.query_row(
            "SELECT is_enabled FROM models WHERE id=?1", params![ps], |r| r.get(0)
        ).unwrap();
        assert_eq!(ps_enabled, 0, "未就绪模型不应被设为激活");
    }

    /// 回归 review fix 双激活 bug：不可用模型上残留的 is_enabled=1 在 switch 时被清理。
    ///
    /// 触发链：X 激活 → X 文件丢失(a=0) → set_model_available("X",false) 清 e →
    /// switch 到 Y → X 不再 is_enabled=1 → X 恢复 a=1 → 不双激活。
    #[test]
    fn switch_clears_stale_enabled_on_unavailable_model() {
        let conn = open_init();
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();

        // 1. 激活 sensevoice
        switch_active_model_at(&conn, "asr", sv).unwrap();
        // 2. sensevoice 文件丢失 → set_model_available(false) 同步清 is_enabled
        set_model_available_at(&conn, "sensevoice-orig-small", false).unwrap();
        let sv_e: i64 = conn.query_row("SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)).unwrap();
        assert_eq!(sv_e, 0, "set_model_available(false) 应清 is_enabled");
        // 3. 激活 firered
        switch_active_model_at(&conn, "asr", fr).unwrap();
        // 4. sensevoice 文件恢复 → set_model_available(true)
        set_model_available_at(&conn, "sensevoice-orig-small", true).unwrap();
        // 5. 不双激活——sensevoice 的 is_enabled 仍 0
        let sv_e2: i64 = conn.query_row("SELECT is_enabled FROM models WHERE id=?1", params![sv], |r| r.get(0)).unwrap();
        assert_eq!(sv_e2, 0, "恢复可用后不应自动激活");
        // 仅 firered 激活
        let active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(active.id, fr, "仅 firered 激活");
    }

    /// §6.4 4 域统一：get_active_model / switch_active_model 对 asr/llm/ocr/translate
    /// 4 个 domain 行为一致——同一 API，按 domain 过滤，互不串扰。
    #[test]
    fn switch_active_model_isolates_domains() {
        let conn = open_init();
        // ASR 域激活 sensevoice
        let sv: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='sensevoice-orig-small'",
            [], |r| r.get(0)
        ).unwrap();
        // OCR 域激活 PP-OCRv6-small（is_available=1）
        let ocr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='ocr' AND model_name='PP-OCRv6-small'",
            [], |r| r.get(0)
        ).unwrap();

        switch_active_model_at(&conn, "asr", sv).unwrap();
        switch_active_model_at(&conn, "ocr", ocr).unwrap();

        // 4 域各查一次——asr/ocr 命中各自激活，llm/translate 仍 None
        let asr_active = get_active_model_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(asr_active.model_name, "sensevoice-orig-small");
        assert_eq!(asr_active.domain, "asr");

        let ocr_active = get_active_model_at(&conn, "ocr").unwrap().unwrap();
        assert_eq!(ocr_active.model_name, "PP-OCRv6-small");
        assert_eq!(ocr_active.domain, "ocr");

        assert!(get_active_model_at(&conn, "llm").unwrap().is_none());
        assert!(get_active_model_at(&conn, "translate").unwrap().is_none());

        // 域间不串扰：再切 ASR 不影响 OCR
        let fr: i64 = conn.query_row(
            "SELECT id FROM models WHERE domain='asr' AND model_name='firered-asr2'",
            [], |r| r.get(0)
        ).unwrap();
        switch_active_model_at(&conn, "asr", fr).unwrap();
        let ocr_still = get_active_model_at(&conn, "ocr").unwrap().unwrap();
        assert_eq!(ocr_still.id, ocr, "切 ASR 不应影响 OCR 激活态");
    }

    /// get_asr_model_by_spec：3-part spec（provider+category+name）精确匹配。
    /// 仅查 is_available=1 的（不限 is_enabled）——CLI 多模型路径专用。
    #[test]
    fn get_asr_model_by_spec_full_3part_matches_available() {
        let conn = open_init();
        // sensevoice-orig-small is_available=1（seed）
        let row = get_asr_model_by_spec_at(&conn, Some("local"), Some("sensevoice-orig"), "sensevoice-orig-small")
            .unwrap().expect("应命中可用模型");
        assert_eq!(row.model_name, "sensevoice-orig-small");
        assert_eq!(row.provider, "local");
        assert_eq!(row.category, "sensevoice-orig");
        assert_eq!(row.domain, "asr");
        assert!(row.is_available);
    }

    /// get_asr_model_by_spec：裸名（provider/category=None）跨 provider/category 匹配。
    #[test]
    fn get_asr_model_by_spec_bare_name_matches() {
        let conn = open_init();
        let row = get_asr_model_by_spec_at(&conn, None, None, "sensevoice-orig-small")
            .unwrap().expect("裸名应命中可用模型");
        assert_eq!(row.model_name, "sensevoice-orig-small");
    }

    /// get_asr_model_by_spec：is_available=0 的模型不返回（文件未就绪不可用）。
    #[test]
    fn get_asr_model_by_spec_filters_unavailable() {
        let conn = open_init();
        // paraformer-streaming is_available=0
        let result = get_asr_model_by_spec_at(&conn, Some("local"), Some("paraformer"), "paraformer-streaming")
            .unwrap();
        assert!(result.is_none(), "未就绪模型不应被查询到");
        let result2 = get_asr_model_by_spec_at(&conn, None, None, "paraformer-streaming")
            .unwrap();
        assert!(result2.is_none(), "裸名查未就绪模型也应返回 None");
    }

    /// get_asr_model_by_spec：非 ASR domain 不命中（函数硬编码 domain='asr'）。
    #[test]
    fn get_asr_model_by_spec_rejects_non_asr_domain() {
        let conn = open_init();
        // PP-OCRv6-small 是 ocr domain，is_available=1，但函数只查 asr
        let result = get_asr_model_by_spec_at(&conn, None, None, "PP-OCRv6-small")
            .unwrap();
        assert!(result.is_none(), "ocr domain 模型不应被 asr 查询命中");
    }

    /// get_asr_model_by_spec：不存在的 name 返回 None（不报错）。
    #[test]
    fn get_asr_model_by_spec_returns_none_for_unknown() {
        let conn = open_init();
        let result = get_asr_model_by_spec_at(&conn, None, None, "nonexistent-model-xxx")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_llm_models_filters_disabled_and_sorts() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据（is_available=1 表示可用）
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_local, is_available)
             VALUES
             ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','',0,1),
             ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','',0,1),
             ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','',0,1),
             ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1),
             ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1),
             ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1)"
        ).unwrap();
        // 禁用 aliyun provider 下全部 3 条（is_available=0）
        conn.execute(
            "UPDATE models SET is_available = 0 WHERE domain='llm' AND provider='aliyun'",
            [],
        ).unwrap();
        let list = list_llm_models_at(&conn).unwrap();
        // 剩余 3 条 → category 字母序: deepseek, glm, glm
        assert_eq!(list.len(), 3, "aliyun 3 条被禁用应过滤");
        assert_eq!(
            list.iter().map(|m| m.category.as_str()).collect::<Vec<_>>(),
            vec!["deepseek", "glm", "glm"],
            "按 category 字母序"
        );
    }

    #[test]
    fn list_llm_models_at_empty_when_all_disabled() {
        let conn = open_init();
        // seed 无 LLM 数据（用户自建）
        let list = list_llm_models_at(&conn).unwrap();
        assert!(list.is_empty(), "无 LLM 数据时返回空");
    }

    #[test]
    fn list_ocr_models_returns_all() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 2 条 OCR，全部返回（list_ocr_models_at 不过滤 is_available/is_enabled）
        assert_eq!(list.len(), 2, "seed 2 条 OCR，全量返回");
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv6-small"));
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv5"));
    }

    #[test]
    fn list_ocr_models_includes_all_even_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_available = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        // 即使全部 is_available=0，仍返回全部（前端需展示供切换）
        assert_eq!(list.len(), 2, "全不可用时仍返回全部 OCR 模型");
    }

    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }

    #[test]
    fn update_and_finalize_round_trip() {
        let conn = open_init();
        // 新 schema：voice 条目存 clipboard_history，content=text，segments=段 JSON，meta_info JSON 存 engine/polished/char_count/duration_ms。
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES (100, 'voice', '首段', '[{\"kind\":\"raw\",\"text\":\"首段\"}]', '{\"engine\":\"sensevoice\",\"polished\":false,\"char_count\":2}', '2026-06-14 00:00:00')",
            [],
        )
        .unwrap();
        // 流式补段 → 更新 content/segments
        conn.execute(
            "UPDATE clipboard_history SET content='首段二段', segments='[{\"kind\":\"raw\",\"text\":\"首段二段\"}]',
                meta_info=json_set(meta_info,'$.char_count',4) WHERE id=100",
            [],
        )
        .unwrap();
        // finalize → 写最终 content/segments/meta_info
        conn.execute(
            "UPDATE clipboard_history SET content='润色', segments='[{\"kind\":\"polished\",\"text\":\"润色\"}]',
                meta_info=json_set(meta_info,'$.polished',1,'$.char_count',2,'$.duration_ms',5000) WHERE id=100",
            [],
        )
        .unwrap();

        let (text, segments, polished, dur): (String, String, i64, Option<i64>) = conn
            .query_row(
                "SELECT content, segments, json_extract(meta_info,'$.polished'), json_extract(meta_info,'$.duration_ms') FROM clipboard_history WHERE id=100",
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
             VALUES (100, 'voice', '你好', '{\"engine\":\"whisper\",\"polished\":false}', '2026-06-17 10:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES (200, 'voice', '你好，世界。', '[{\"kind\":\"polished\",\"text\":\"你好，世界。\"}]', '{\"engine\":\"qwen3\",\"polished\":true}', '2026-06-17 11:00:00')",
            [],
        )
        .unwrap();
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[0].text.as_deref(), Some("你好，世界。"));
        assert_eq!(rows[0].polish_status, "done");
        let page1 = list_transcriptions_at(&conn, 1, 0).unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].id, 200);
        let page2 = list_transcriptions_at(&conn, 1, 1).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, 100);
        let page3 = list_transcriptions_at(&conn, 10, 2).unwrap();
        assert!(page3.is_empty());
    }

    #[test]
    fn delete_transcriptions_removes_specified_ids() {
        let conn = open_init();
        for &(id, eng, txt) in &[(100i64, "whisper", "你好"), (200, "qwen3", "你好世界"), (300, "sensevoice", "测试")] {
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
                params![200, 300],
            )
            .unwrap();
        assert_eq!(n, 2);
        let remaining = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 100);
    }

    #[test]
    fn delete_transcriptions_at_empty_is_noop() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES (100, 'voice', '你好', '{\"engine\":\"whisper\",\"polished\":false}', '2026-06-17 10:00:00')",
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
        for &(id, txt) in &[(100i64, "你好"), (200, "世界")] {
            conn.execute(
                "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
                 VALUES (?, 'voice', ?, '{\"engine\":\"test\",\"polished\":false}', '2026-06-17 10:00:00')",
                params![id, txt],
            )
            .unwrap();
        }
        let n = delete_transcriptions_at(&conn, &[100, 200]).unwrap();
        assert_eq!(n, 2);
        assert!(list_transcriptions_at(&conn, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn update_edited_text_persists_and_lists() {
        let conn = open_init();
        // id=100：将被编辑的记录
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, segments, meta_info, created_at)
             VALUES (100, 'voice', '润色稿', '[{\"kind\":\"polished\",\"text\":\"润色稿\"}]', '{\"engine\":\"whisper\",\"polished\":true}', '2026-06-18 10:00:00')",
            [],
        )
        .unwrap();
        // id=200：未编辑的对照记录
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, meta_info, created_at)
             VALUES (200, 'voice', '另一条', '{\"engine\":\"qwen3\",\"polished\":false}', '2026-06-18 11:00:00')",
            [],
        )
        .unwrap();

        // 走真实 update_edited_segments_at（而非裸 SQL），断言返回行数 1
        let segs = r#"[{"kind":"edited","text":"手改文本"}]"#;
        let n = update_edited_segments_at(&conn, 100, "手改文本", segs).unwrap();
        assert_eq!(n, 1);

        // 经 list_transcriptions_at 回读，同时验证 list 列序映射正确
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[1].text.as_deref(), Some("手改文本"));
        assert_eq!(rows[1].segments.as_deref(), Some(segs));
        // 未编辑记录：text 仍是原值
        assert_eq!(rows[0].text.as_deref(), Some("另一条"));

        // 不存在的 id：返回 0 行更新
        let missing = update_edited_segments_at(&conn, 9999, "无效", "[]").unwrap();
        assert_eq!(missing, 0);
    }

    // ── app_config 表测试 ──

    #[test]
    fn app_config_seed_provides_all_fields() {
        let conn = open_init();
        let cfg = load_app_config_at(&conn).unwrap();
        // seed 默认值校验（抽样关键字段）
        assert_eq!(cfg.engine_mode, "embedded");
        assert_eq!(cfg.language, "auto");
        assert!(cfg.write_to_clipboard);
        assert!(!cfg.asr_hardware_accelerated);
        assert_eq!(cfg.segment_silence, 400.0);
        assert_eq!(cfg.polish_min_interval, 5.0);
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "CmdOrCtrl+Enter");
        assert_eq!(cfg.download_mirror, "");
    }

    #[test]
    fn save_and_reload_preserves_overrides() {
        use crate::config::PolishMode;
        let conn = open_init();
        let mut cfg = load_app_config_at(&conn).unwrap();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.microphone = "My Mic".into();
        cfg.segment_silence = 350.0;
        cfg.denoise_mode = 2;
        cfg.download_mirror = "https://hf-mirror.com".to_string();
        save_app_config_at(&conn, &cfg).unwrap();

        let cfg2 = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");
        assert_eq!(cfg2.segment_silence, 350.0);
        assert_eq!(cfg2.denoise_mode, 2);
        assert_eq!(cfg2.download_mirror, "https://hf-mirror.com");
        // 未改字段保持 seed 默认
        assert_eq!(cfg2.language, "auto");
    }

    #[test]
    fn save_config_key_overrides_single_field() {
        let conn = open_init();
        conn.execute(
            "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
            params!["language", "ja"],
        ).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.language, "ja");
        assert_eq!(cfg.engine_mode, "embedded"); // 其余不变
    }

    #[test]
    fn load_with_missing_row_keeps_default() {
        let conn = open_init();
        // 删掉一行，load 应保留 default
        conn.execute("DELETE FROM app_config WHERE config_key='denoise_mode'", []).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.denoise_mode, 1); // AppConfig::default() 的值
    }

    #[test]
    fn save_preserves_description_and_category() {
        let conn = open_init();
        // 验证 seed 有 description
        let desc: String = conn
            .query_row(
                "SELECT description FROM app_config WHERE config_key='language'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!desc.is_empty(), "seed 的 description 不应为空");

        // 单键写入后 description 应保留（INSERT OR REPLACE 会清空，ON CONFLICT 不会）
        conn.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)\n             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params!["language", "zh"],
        ).unwrap();
        let (val, desc2): (String, String) = conn
            .query_row(
                "SELECT config_value, description FROM app_config WHERE config_key='language'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(val, "zh");
        assert_eq!(desc2, desc, "description 应被保留");

        // save_config_key 路径也保留
        // （save_config_key 走 with_db，需全局 DB 初始化；这里测底层 SQL 一致性即可）

        // save_app_config_at 全量写也保留
        let mut cfg = load_app_config_at(&conn).unwrap();
        cfg.language = "en".into();
        save_app_config_at(&conn, &cfg).unwrap();
        let (val3, desc3, cat3): (String, String, String) = conn
            .query_row(
                "SELECT config_value, description, category FROM app_config WHERE config_key='language'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(val3, "en");
        assert_eq!(desc3, desc, "save_app_config_at 应保留 description");
        assert_eq!(cat3, "setting", "category 应为 setting");
    }

    #[test]
    fn app_config_category_defaults_to_setting() {
        let conn = open_init();
        let categories: Vec<String> = conn
            .prepare("SELECT DISTINCT category FROM app_config")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            categories.contains(&"setting".to_string()) && categories.contains(&"env".to_string()),
            "category 应包含 'setting' 和 'env'，实际: {:?}", categories
        );
    }

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

    #[test]
    fn prompts_table_init_sql_idempotent() {
        let conn = open_init();
        // db.sql 不再内联 prompts seed——通过外置 loader 加载，二次调用幂等（OR IGNORE）。
        crate::seeds::load_external_seeds(&conn).unwrap();
        crate::seeds::load_external_seeds(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "重跑外置 seed loader 不应重复 seed");
    }

    #[test]
    fn prompt_crud_round_trip() {
        let conn = open_init();
        // prompts seed 已外置到 seeds/prompts/——通过 loader 加载初始 2 条。
        crate::seeds::load_external_seeds(&conn).unwrap();
        // list 初值：2 条系统内置（id=1 默认润色 + id=2 进阶润色（断续纠正））
        let list = list_prompts_at(&conn).unwrap();
        assert_eq!(list.len(), 2, "seed 应有 2 条系统内置 prompt");
        assert!(list[0].is_system);
        assert_eq!(list[0].title, "默认润色");
        assert!(list[1].is_system);
        assert_eq!(list[1].title, "进阶润色（断续纠正）");

        // insert 用户 prompt（id 应大于 seed 最大 id）
        let id = insert_prompt_at(&conn, "技术写作", "rule1", "desc1").unwrap();
        assert!(id > 2, "用户 prompt id 应大于 seed 最大 id(2)");

        // load
        let loaded = load_prompt_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.title, "技术写作");
        assert_eq!(loaded.content, "rule1");
        assert!(!loaded.is_system);

        // update（用户 prompt 可改）
        update_prompt_at(&conn, id, "技术写作V2", "rule2", "desc2").unwrap();
        let updated = load_prompt_at(&conn, id).unwrap().unwrap();
        assert_eq!(updated.title, "技术写作V2");
        assert_eq!(updated.content, "rule2");

        // update 系统 prompt 现在允许（配合「复原默认」按钮：编辑/复原都走 update）
        // 详见 update_prompt_at_allows_system_prompt 用例的完整断言。

        // delete 系统 prompt 被拒
        assert!(delete_prompt_at(&conn, 1).is_err());

        // delete 用户 prompt 成功
        delete_prompt_at(&conn, id).unwrap();
        assert!(load_prompt_at(&conn, id).unwrap().is_none());

        // delete 不存在的 id
        assert!(delete_prompt_at(&conn, 999).is_err());
    }

    #[test]
    fn prompt_title_allows_duplicate() {
        let conn = open_init();
        // 插入两条同名用户 prompt（title 允许重复）
        insert_prompt_at(&conn, "同名", "a", "").unwrap();
        insert_prompt_at(&conn, "同名", "b", "").unwrap();
        let list = list_prompts_at(&conn).unwrap();
        let dup_count = list.iter().filter(|p| p.title == "同名").count();
        assert_eq!(dup_count, 2, "title 允许重复");
    }

    /// update_prompt_at 允许更新 system prompt（is_system 字段保持不变）。
    /// 历史：曾因「不可编辑」bail，移除拒绝以支持「复原默认」按钮（先编辑再保存）。
    #[test]
    fn update_prompt_at_allows_system_prompt() {
        let conn = open_init();
        // open_init 只建表，不 seed——需手动加载外部 seed（id=1/2 系统 prompt）
        crate::seeds::load_external_seeds(&conn).unwrap();
        // seed 后 id=1 是系统内置（默认润色）
        let before = load_prompt_at(&conn, 1).unwrap().unwrap();
        assert!(before.is_system, "seed id=1 应是 is_system=true");

        // 更新系统 prompt 成功
        update_prompt_at(&conn, 1, "改过的标题", "改过的内容", "改过的描述").unwrap();
        let updated = load_prompt_at(&conn, 1).unwrap().unwrap();
        assert_eq!(updated.title, "改过的标题");
        assert_eq!(updated.content, "改过的内容");
        assert_eq!(updated.description, "改过的描述");
        assert!(updated.is_system, "is_system 字段应保持 true（不被翻转）");
    }

    // ── FTS5 搜索（trigram MATCH >=3 char，LIKE 回退 <3 char）──

    /// 辅助：插入 voice 行，返回连接
    fn open_with_voice(rows: &[(i64, &str)]) -> Connection {
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
            (100, "今天的会议纪要很详细"),
            (200, "明天去爬山"),
        ]);
        // 4 字符 → FTS5 MATCH 路径
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("会议纪要")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 100);
        assert_eq!(rows[0].text.as_deref(), Some("今天的会议纪要很详细"));
    }

    #[test]
    fn fts5_search_short_query_falls_back_to_like() {
        let conn = open_with_voice(&[
            (100, "你好世界"),
            (200, "再见"),
        ]);
        // 2 字符 → LIKE 回退（trigram 无法生成 3-gram）
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("你好")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 100);
    }

    #[test]
    fn fts5_search_special_chars_no_panic() {
        let conn = open_with_voice(&[(100, "test*result"), (200, "a\"quoted\"b")]);
        // 含 FTS5 特殊字符的查询不应 panic 或 SQL 错误
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("test*resu")).unwrap();
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("AND")).unwrap();
        let _ = list_transcriptions_search_at(&conn, 10, 0, Some("quoted")).unwrap();
    }

    #[test]
    fn fts5_search_empty_content_not_indexed() {
        let conn = open_with_voice(&[(100, ""), (200, "有内容的记录")]);
        // 空 content 不索引，但搜索应正常返回有内容的行
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("有内容的")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 200);
    }

    #[test]
    fn fts5_backfill_sql_is_idempotent() {
        // 验证 backfill SQL 本身的正确性与幂等性（实际触发器行为由 FTS5 外部内容表保证）
        let conn = open_with_voice(&[(100, "历史遗留的会议记录"), (200, "另一条记录")]);
        // backfill SQL（与 init_schema v17→v18 相同）
        let backfill = "INSERT INTO clipboard_history_fts(rowid, content)
             SELECT id, content FROM clipboard_history
             WHERE content != ''
               AND id NOT IN (SELECT rowid FROM clipboard_history_fts)";
        // 触发器已索引这些行（NOT IN 排除）→ backfill 不插入（幂等）
        conn.execute_batch(backfill).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clipboard_history_fts WHERE rowid IN (100,200)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "行已在索引中，backfill 幂等不重复");
        // backfill 后搜索仍正常
        let rows = list_transcriptions_search_at(&conn, 10, 0, Some("会议记录")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 100);
    }

    #[test]
    fn fts5_escape_wraps_in_phrase() {
        assert_eq!(escape_fts5_match("会议纪要"), "\"会议纪要\"");
        assert_eq!(escape_fts5_match("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_fts5_match("AND"), "\"AND\"");
    }

    #[test]
    fn action_bar_items_seed_has_10_items() {
        let conn = open_init();
        let items = list_all_action_bar_items_at(&conn).unwrap();
        assert!(items.len() >= 10, "expected >=10 seed items, got {}", items.len());
    }

    #[test]
    fn action_bar_items_list_enabled_filters_disabled() {
        let conn = open_init();
        let id = insert_action_bar_item_at(&conn, None, "测试禁用", "test", "url", "", true, false, "", "", "text", "", true, false).unwrap();
        update_action_bar_item_at(&conn, id, "测试禁用", "test", "url", "", false, true, false, "", "", "text", "", false).unwrap();
        let enabled = list_action_bar_items_at(&conn).unwrap();
        assert!(!enabled.iter().any(|i| i.id == id));
        let all = list_all_action_bar_items_at(&conn).unwrap();
        assert!(all.iter().any(|i| i.id == id));
        delete_action_bar_item_at(&conn, id).unwrap();
    }

    #[test]
    fn action_bar_items_system_item_cannot_delete() {
        let conn = open_init();
        let result = delete_action_bar_item_at(&conn, 1);
        assert!(result.is_err());
    }

    #[test]
    fn action_bar_items_move_swaps_order() {
        let conn = open_init();
        let id_a = insert_action_bar_item_at(&conn, None, "AAA", "test", "url", "", true, false, "", "", "text", "", true, false).unwrap();
        let id_b = insert_action_bar_item_at(&conn, None, "BBB", "test", "url", "", true, false, "", "", "text", "", true, false).unwrap();
        let a_before = load_action_bar_item_at(&conn, id_a).unwrap().unwrap();
        let b_before = load_action_bar_item_at(&conn, id_b).unwrap().unwrap();
        assert!(a_before.sort_order < b_before.sort_order);
        move_action_bar_item_at(&conn, id_a, 1).unwrap();
        let a_after = load_action_bar_item_at(&conn, id_a).unwrap().unwrap();
        assert_eq!(a_after.sort_order, b_before.sort_order);
        delete_action_bar_item_at(&conn, id_a).unwrap();
        delete_action_bar_item_at(&conn, id_b).unwrap();
    }

    #[test]
    fn list_active_words_is_enabled_union() {
        let conn = &mut rusqlite::Connection::open_in_memory().unwrap();
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
        let conn = &mut rusqlite::Connection::open_in_memory().unwrap();
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

    // ── v28: manifest 填充 + 路径统一 测试 ──

    /// fill_manifests 应为 secret_key 为空的 is_local=1 模型填充 manifest。
    #[test]
    fn fill_manifests_populates_empty_secret_key() {
        let conn = open_init();
        // INIT_SQL 后 secret_key 全空（seed 不预填 manifest）
        let empty_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE is_local=1 AND (secret_key='' OR secret_key IS NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(empty_count > 0, "seed 后应有 is_local=1 且 secret_key 空的行");

        fill_manifests(&conn).unwrap();

        // 验证 ASR 模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "whisper-small secret_key 应被填充");
        let parsed: serde_json::Value = serde_json::from_str(&sk).unwrap();
        assert!(parsed.as_object().unwrap().contains_key("onnx/encoder_model_int8.onnx"));

        // 验证翻译模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='opus-mt' AND domain='translate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "opus-mt secret_key 应被填充");

        // 验证 OCR 模型有了 manifest
        let sk: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='PP-OCRv6-small' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sk.is_empty(), "PP-OCRv6-small secret_key 应被填充");
    }

    /// fill_manifests 幂等：已填充的 manifest 不应被覆盖。
    #[test]
    fn fill_manifests_is_idempotent() {
        let conn = open_init();
        fill_manifests(&conn).unwrap();
        let sk1: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // 再次调用——不会重写（secret_key 非空，WHERE 条件不匹配）
        fill_manifests(&conn).unwrap();
        let sk2: String = conn
            .query_row(
                "SELECT secret_key FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sk1, sk2, "二次调用不应改变已有 manifest");
    }

    /// list_local_models_by_domain 按 domain 正确过滤。
    #[test]
    fn list_local_models_by_domain_filters_correctly() {
        let conn = open_init();
        fill_manifests(&conn).unwrap();

        let asr_rows = list_all_local_asr_models_at(&conn).unwrap();
        assert!(asr_rows.iter().all(|r| r.source.starts_with("asr/")),
            "ASR models source 应以 asr/ 开头");

        // 用新函数查 translate
        let translate_rows: Vec<LocalAsrModelRow> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, category, model_name, source, secret_key, description, is_enabled, is_available, is_streaming
                     FROM models WHERE domain='translate' AND is_local = 1",
                )
                .unwrap();
            let rows = stmt.query_map([], |row| {
                Ok(LocalAsrModelRow {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    model_name: row.get(2)?,
                    source: row.get(3)?,
                    secret_key: row.get(4)?,
                    description: row.get(5)?,
                    is_enabled: row.get::<_, i32>(6)? != 0,
                    is_available: row.get::<_, i32>(7)? != 0,
                    is_streaming: row.get::<_, i32>(8)? != 0,
                })
            }).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert_eq!(translate_rows.len(), 2, "应有 2 个翻译模型");
        assert!(
            translate_rows.iter().any(|r| r.model_name == "opus-mt"),
            "应包含 opus-mt"
        );
        assert!(
            translate_rows.iter().any(|r| r.model_name == "m2m100-418M"),
            "应包含 m2m100-418M"
        );
    }

    /// ASR source 应从旧 HF repo 格式更新为 asr/{name} 路径标识。
    #[test]
    fn asr_source_is_path_identifier() {
        let conn = open_init();
        // INIT_SQL 已用新 seed（asr/{name}），验证
        let source: String = conn
            .query_row(
                "SELECT source FROM models WHERE model_name='whisper-small' AND domain='asr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "asr/whisper-small");
    }

    /// OCR seed 不再含旧 GitHub MNN URL。
    #[test]
    fn ocr_source_is_path_identifier_not_mnn() {
        let conn = open_init();
        let source: String = conn
            .query_row(
                "SELECT source FROM models WHERE model_name='PP-OCRv6-small' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "ocr/PP-OCRv6-small");
        assert!(!source.contains("github.com"), "不应再含 GitHub URL");
    }

    /// PP-OCRv5 应在 seed 中。
    #[test]
    fn ocr_v5_in_seed() {
        let conn = open_init();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE model_name='PP-OCRv5' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "PP-OCRv5 应在 seed 中");
    }

    // ── TDD 防御：OCR 列表不过滤 is_enabled ──

    /// list_ocr_models 返回全部 OCR 模型（含 is_enabled=0 的未就绪模型）。
    #[test]
    fn list_ocr_models_includes_disabled() {
        let conn = open_init();
        // PP-OCRv5 默认 is_enabled=0
        let pp5_enabled: i32 = conn
            .query_row(
                "SELECT is_enabled FROM models WHERE model_name='PP-OCRv5' AND domain='ocr'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pp5_enabled, 0, "PP-OCRv5 默认未就绪");

        // list_ocr_models_at 不过滤 is_enabled → 应包含 PP-OCRv5
        let ocrs = list_ocr_models_at(&conn).unwrap();
        assert!(
            ocrs.iter().any(|m| m.model_name == "PP-OCRv5"),
            "list_ocr_models 应包含未就绪的 PP-OCRv5"
        );
        assert!(
            ocrs.iter().any(|m| m.model_name == "PP-OCRv6-small"),
            "list_ocr_models 应包含已就绪的 PP-OCRv6-small"
        );
    }

    // ── TDD 防御：env 变量 config_key 不含 env. 前缀 ──

    /// DB seed 中 env 变量 config_key 不含 env. 前缀。
    #[test]
    fn env_var_keys_have_no_env_prefix() {
        let conn = open_init();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_config WHERE category='env' AND config_key LIKE 'env.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "env 变量 config_key 不应含 env. 前缀");

        // 验证 bare key 存在
        let hf: String = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='huggingface' AND category='env'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!hf.is_empty(), "huggingface 环境变量应有值");
    }

    // ── Task 1: search_frequency 表 + record/load fns ──

    /// record_search_frequency 写一行 → load_search_frequency 读回，验证字段。
    /// 再 record 同一 key → hit_count +1，query/last_hit_ts 更新。
    #[test]
    fn search_frequency_record_and_load_roundtrip() {
        setup_test_db();
        // 清理可能的旧数据（测试隔离）
        let _ = with_db(|conn| {
            conn.execute(
                "DELETE FROM search_frequency WHERE score_key LIKE 'test_%'",
                [],
            )?;
            Ok(())
        });
        record_search_frequency("test_key_1", "test_query").unwrap();
        let map = load_search_frequency().unwrap();
        let row = map.get("test_key_1").expect("应能读到刚写的记录");
        assert_eq!(row.hit_count, 1);
        assert_eq!(row.query, "test_query");
        assert!(row.last_hit_ts > 0);
        // 再 record 一次，hit_count 应 +1
        record_search_frequency("test_key_1", "test_query2").unwrap();
        let map = load_search_frequency().unwrap();
        assert_eq!(map.get("test_key_1").unwrap().hit_count, 2);
        assert_eq!(map.get("test_key_1").unwrap().query, "test_query2");
    }

    /// schema v35 迁移后 search_frequency 表应存在于 sqlite_master。
    #[test]
    fn search_frequency_table_exists_after_init() {
        setup_test_db();
        let exists: bool = with_db(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='search_frequency'",
            )?;
            let mut found = false;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                if r?.contains("search_frequency") {
                    found = true;
                }
            }
            Ok(found)
        })
        .unwrap_or(false);
        assert!(exists, "search_frequency 表应在 schema v35 后存在");
    }
}

#[cfg(test)]
mod vault_schema_tests {
    use super::*;

    /// 在内存 DB 上执行 db.sql，得到含全部 schema（含 vault v38 表）的连接。
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("db.sql")).unwrap();
        conn.execute("PRAGMA user_version = 43", []).unwrap();
        conn
    }

    #[test]
    fn test_vault_meta_upsert_and_load() {
        let conn = test_db();
        // 全新库无 meta 行
        assert!(load_vault_meta_at(&conn).unwrap().is_none());

        let input = VaultMetaInput {
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:aaa".into(),
            app_key_local_enc: "v1:bbb".into(),
            app_key_sync_enc: "v1:ccc".into(),
            security_stamp: "stamp-1".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
        };
        upsert_vault_meta_at(&conn, &input).unwrap();

        let loaded = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded.id, 1);
        assert_eq!(loaded.kdf_type, 0);
        assert_eq!(loaded.kdf_salt, vec![1u8; 32]);
        assert_eq!(loaded.kdf_iterations, 3);
        assert_eq!(loaded.kdf_memory_kib, 65_536);
        assert_eq!(loaded.kdf_parallelism, 4);
        assert_eq!(loaded.protected_user_vault_key, "v1:aaa");
        assert_eq!(loaded.app_key_local_enc, "v1:bbb");
        assert_eq!(loaded.app_key_sync_enc, "v1:ccc");
        assert_eq!(loaded.security_stamp, "stamp-1");
        assert_eq!(loaded.equivalent_domains, "[]");
        assert!(loaded.public_key.is_none());
        assert!(loaded.protected_private_key.is_none());

        // Upsert（覆盖）—— security_stamp 变更，其他字段保持。
        let mut input2 = input.clone();
        input2.security_stamp = "stamp-2".into();
        upsert_vault_meta_at(&conn, &input2).unwrap();
        let loaded2 = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded2.security_stamp, "stamp-2");
        // 仍是单行
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vault_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_vault_cipher_crud() {
        let conn = test_db();
        let id = "crud-test-uuid-1";
        let input = VaultCipherInput {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:enc-name".into(),
            notes: None,
            data: "v1:enc-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            sync_md5: None,
        };
        insert_vault_cipher_at(&conn, &input).unwrap();

        let loaded = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.name, "v1:enc-name");
        assert_eq!(loaded.atype, 1);
        assert!(!loaded.favorite);
        assert!(loaded.deleted_at.is_none());

        // 更新
        let mut input2 = input.clone();
        input2.name = "v1:enc-name-2".into();
        input2.favorite = true;
        update_vault_cipher_at(&conn, id, &input2).unwrap();
        let loaded2 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded2.name, "v1:enc-name-2");
        assert!(loaded2.favorite);

        // 软删除
        soft_delete_vault_cipher_at(&conn, id).unwrap();
        let loaded3 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert!(loaded3.deleted_at.is_some(), "软删除后 deleted_at 应非空");

        // 恢复
        restore_vault_cipher_at(&conn, id).unwrap();
        let loaded4 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert!(loaded4.deleted_at.is_none(), "恢复后 deleted_at 应为空");

        // 物理删除
        permanent_delete_vault_cipher_at(&conn, id).unwrap();
        assert!(load_vault_cipher_at(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_vault_meta_check_constraint() {
        let conn = test_db();
        // 尝试插入 id=2 应失败（CHECK id=1）
        let result = conn.execute(
            "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                      protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp)
             VALUES (2, 0, X'00', 0, 0, 0, '', '', '', '')",
            [],
        );
        assert!(result.is_err(), "CHECK(id=1) 应阻止 id=2 的插入");
    }
}
