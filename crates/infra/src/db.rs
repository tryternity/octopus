// crates/infra/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

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
    pub is_streaming: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
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
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
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

static DB: OnceLock<parking_lot::ReentrantMutex<Connection>> = OnceLock::new();

/// 编译期嵌入的建表 + seed SQL（来自 crates/infra/src/db.sql）
const INIT_SQL: &str = include_str!("db.sql");

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    crate::paths::octopus_config_home().join("octopus.db")
}

/// 幂等初始化：打开/创建 DB，以 db.sql 为准建表（开发期简化，无历史迁移链）。
pub fn ensure_db() -> Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("Failed to open DB at {}", path.display()))?;
    // WAL 模式（读写并发友好，server 多任务访问不 SQLITE_BUSY）+ busy_timeout（锁竞争时
    // 等待 5s 而非立即报错）。journal_mode 持久化在 db 头（设一次即生效），重复设置幂等。
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
        .context("set WAL + busy_timeout")?;
    init_schema(&conn)?;
    let _ = DB.set(parking_lot::ReentrantMutex::new(conn));
    Ok(())
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
    if DB.get().is_none() {
        ensure_db()?;
    }
    let mutex = DB.get().context("DB not initialized")?;
    let conn = mutex.lock();
    f(&conn)
}


/// 初始化 schema（开发期简化版）：以 db.sql 为唯一表结构真相，无历史迁移链。
///
/// - v17（已最新）: 跳过
/// - 其他（v0 全新库）: 跑 INIT_SQL 建表 + seed → 一次性 yaml 配置导入 → v17
///
/// INIT_SQL 全部为 CREATE TABLE IF NOT EXISTS + INSERT OR IGNORE，幂等。
/// schema 变更流程：改 db.sql + 升下方 user_version 数值，勿新增 ALTER 迁移分支。
/// 开发期无历史库需兼容（用户确认），故不保留 v1-v16 迁移/DROP 兜底。
/// v18：FTS5 backfill（历史行补入索引），搜索走 MATCH。
/// v19：新增 action_bar_items 表（db.sql IF NOT EXISTS 自动创建）。
/// v20：paste_input_source_switch。
/// v21：action_bar_items 加 is_async + write_output_to_clipboard 列；新建 script_runs 表。
/// v20：新增 hotwords 表（db.sql IF NOT EXISTS 自动创建）。
/// v23：新增 hotword_sets + hotword_hits 表；现有 active 热词迁「通用」版本。
/// v24：action_bar_items 加 shortcut 列。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v >= 31 {
        return Ok(()); // 已最新
    }
    if v >= 17 {
        // v17→v18：backfill FTS5 索引——触发器（clip_fts_ai）仅维护建表后的新行，
        // 历史 voice 行（建表前已有或从旧 schema 迁移来的）不在索引中，需一次性回填。
        // 幂等：NOT IN 排除已索引行；空文本不索引（与触发器行为一致）。
        if v < 18 {
            conn.execute_batch(
                "INSERT INTO clipboard_history_fts(rowid, content)
                 SELECT id, content FROM clipboard_history
                 WHERE content != ''
                   AND id NOT IN (SELECT rowid FROM clipboard_history_fts)"
            ).context("FTS5 backfill")?;
        }
        // v18→v19：action_bar_items 表由 db.sql 的 IF NOT EXISTS 自动创建，重跑 INIT_SQL 幂等
        // v19→v20：hotwords 表由 db.sql 的 IF NOT EXISTS 自动创建，重跑 INIT_SQL 幂等
        conn.execute_batch(INIT_SQL).ok();
        // v20→v21：action_bar_items 加 is_async + write_output_to_clipboard 列。
        // CREATE TABLE IF NOT EXISTS 对已有表无效，必须 ALTER TABLE 补列。
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !cols.contains(&"is_async".to_string()) {
            conn.execute("ALTER TABLE action_bar_items ADD COLUMN is_async INTEGER NOT NULL DEFAULT 1", [])?;
        }
        if !cols.contains(&"write_output_to_clipboard".to_string()) {
            conn.execute("ALTER TABLE action_bar_items ADD COLUMN write_output_to_clipboard INTEGER NOT NULL DEFAULT 0", [])?;
        }
        conn.execute("PRAGMA user_version = 21", [])?;
        log::info!("schema upgraded to v21 (action_bar_items + script_runs)");
        // v21→v22：env 变量 seed（huggingface/modelscope/github）
        conn.execute_batch(
            "INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES\n             ('huggingface', 'https://hf-mirror.com', 'HuggingFace 下载镜像地址', 'env'),\n             ('modelscope',  'https://modelscope.cn',  '魔搭社区下载镜像地址',   'env'),\n             ('github',      'https://github.com',     'GitHub 下载地址',         'env')"
        )?;
        conn.execute("PRAGMA user_version = 22", [])?;
        log::info!("schema upgraded to v22 (env vars seed)");
        // v22→v23：热词多版本——hotword_sets/hotword_hits 表由 db.sql IF NOT EXISTS 自动创建。
        // 一次性迁移：现有 active 热词 → 「通用」版本 words_text（normalize 排序去重）；
        // 命中计数 → hotword_hits。pending 词丢弃（废弃 pending 确认流）。
        // hotwords 表保留但停用（不 DROP，留待后续清理）。
        if v < 23 {
            conn.execute_batch(INIT_SQL).ok(); // 确保 hotword_sets/hotword_hits 已建
            let words_text: String = {
                let mut stmt = conn.prepare(
                    "SELECT word FROM hotwords WHERE status='active' ORDER BY created_at",
                )?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut words: Vec<String> = Vec::new();
                for r in rows { words.push(r?); }
                crate::hotword_text::normalize_words_text(&words.join(" "))
            };
            // 「通用」版本：无则新建；用户若已手建同名，则把 active 词并入既有版本（spec 要求）。
            let existing: Option<String> = conn
                .query_row("SELECT words_text FROM hotword_sets WHERE name='通用'", [], |r| r.get(0))
                .ok();
            let merged = match existing {
                Some(cur) => crate::hotword_text::normalize_words_text(&format!("{} {}", cur, words_text)),
                None => words_text, // words_text 已 normalize
            };
            conn.execute(
                "INSERT INTO hotword_sets(name, enabled, words_text) VALUES('通用', 1, ?1) \
                 ON CONFLICT(name) DO UPDATE SET words_text = excluded.words_text, updated_at = datetime('now')",
                params![merged],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO hotword_hits(word, hit_count) \
                 SELECT word, hit_count FROM hotwords WHERE status='active'",
                [],
            )?;
            conn.execute("PRAGMA user_version = 23", [])?;
            log::info!("schema upgraded to v23 (hotword_sets + hotword_hits)");
        }
        // v23→v24：action_bar_items 加 shortcut 列
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"shortcut".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN shortcut TEXT NOT NULL DEFAULT ''", [])?;
            }
            conn.execute("PRAGMA user_version = 24", [])?;
            log::info!("schema upgraded to v24 (action_bar_items.shortcut)");
        }
        // v24→v25：seed 新增「问豆包」菜单项（用 title 去重避免 id 冲突）
        if v < 25 {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE title='问豆包' AND parent_id IS NULL",
                [], |r| r.get(0),
            )?;
            if exists == 0 {
                conn.execute(
                    "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
                        (NULL, '问豆包', 'sparkles', 'script', '#osascript\nset the clipboard to (do shell script (\"printf %s \" & quoted form of (system attribute \"OCTOPUS_TEXT\")))\ndo shell script \"open -a Doubao\"\ndelay 2\ntell application \"System Events\"\n    tell process \"Doubao\"\n        keystroke \"v\" using command down\n        delay 0.3\n        key code 36\n    end tell\nend tell', 4, 1)",
                    [],
                )?;
                log::info!("schema upgraded to v25 (seed: 问豆包 menu item)");
            }
            conn.execute("PRAGMA user_version = 25", [])?;
        }
        // v25→v26：action_bar_items 加 agent + accepts 列；新增 agent_adapters 表
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"agent".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN agent TEXT NOT NULL DEFAULT ''", [])?;
            }
            if !cols.contains(&"accepts".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN accepts TEXT NOT NULL DEFAULT 'text'", [])?;
            }
            // submenu 既有项升级为 accepts='any'（容器类型两场景通用）
            conn.execute(
                "UPDATE action_bar_items SET accepts='any' WHERE action_type='submenu' AND accepts='text'",
                [],
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS agent_adapters (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL, detect_binary TEXT NOT NULL, command_template TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
                [],
            )?;
            conn.execute("PRAGMA user_version = 26", [])?;
            log::info!("schema upgraded to v26 (action_bar_items.agent/accepts + agent_adapters table)");
        }
        // v26→v27：agent_tasks 表（action bar agent × 语音识别联动）
        {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS agent_tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL DEFAULT 'pending', agent_key TEXT NOT NULL, context TEXT NOT NULL DEFAULT '{}', transcribed_text TEXT NOT NULL DEFAULT '', error_msg TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
                [],
            )?;
            conn.execute("PRAGMA user_version = 27", [])?;
            log::info!("schema upgraded to v27 (agent_tasks table)");
        }
        // v27→v28：模型路径统一 + manifest 填充 + 翻译模型入 DB
        //
        // 1. ASR source: HF repo → asr/{model_name}（路径标识）
        // 2. OCR source/secret_key: GitHub MNN URL → ocr/{name}（路径标识，清旧 secret_key）
        // 3. translate seed: db.sql IF NOT EXISTS 自动创建
        // 4. secret_key: 从 model_manifests 常量写入所有 is_local=1 模型
        {
            // 重跑 INIT_SQL 确保 translate/OCR seed 已建（幂等）
            conn.execute_batch(INIT_SQL).ok();

            // ASR source 改路径标识（幂等：只改 source 仍为 HF repo 格式的行）
            conn.execute(
                "UPDATE models SET source = 'asr/' || model_name
                 WHERE domain = 'asr' AND is_local = 1
                   AND source NOT LIKE 'asr/%'",
                [],
            )?;

            // OCR source 改路径标识 + 清除旧 MNN URL secret_key
            conn.execute(
                "UPDATE models SET source = 'ocr/' || model_name, secret_key = ''
                 WHERE domain = 'ocr' AND is_local = 1
                   AND source NOT LIKE 'ocr/%'",
                [],
            )?;

            // 填充 manifest（secret_key 空 → 从常量写入）
            fill_manifests(conn)?;

            conn.execute("PRAGMA user_version = 28", [])?;
            log::info!("schema upgraded to v28 (model path unification + manifest fill + translate seed)");
        }
        // v28→v29：env 变量 config_key 去掉 env. 前缀（category='env' 已是命名空间）
        {
            // INIT_SQL 已用新 key (huggingface/modelscope/github) 重新 seed。
            // 旧 env.* 行与新 key 共存会导致唯一约束冲突——直接删除旧 env.* 行。
            conn.execute(
                "DELETE FROM app_config WHERE config_key IN ('env.huggingface','env.modelscope','env.github')",
                [],
            )?;
            conn.execute("PRAGMA user_version = 29", [])?;
            log::info!("schema upgraded to v29 (env var config_key: remove env. prefix)");
        }
        // v29→v30：OCR provider/category 修正（provider=local, category=paddleocr）
        // 旧行 ('ocr','paddleocr','ocr',...) 与 INIT_SQL 新行 ('ocr','local','paddleocr',...) 冲突——删除旧行
        {
            conn.execute(
                "DELETE FROM models WHERE domain='ocr' AND provider='paddleocr' AND category='ocr'",
                [],
            )?;
            conn.execute("PRAGMA user_version = 30", [])?;
            log::info!("schema upgraded to v30 (OCR provider/category fix)");
        }
        // v30→v31：云端模型改为用户自建——删除 seed，新增参考列表
        {
            conn.execute("DELETE FROM models WHERE domain='asr' AND is_local=0", [])?;
            conn.execute("DELETE FROM models WHERE domain='llm'", [])?;
            // 重跑 INIT_SQL 确保参考列表 seed 已建（幂等）
            conn.execute_batch(INIT_SQL).ok();
            conn.execute("PRAGMA user_version = 31", [])?;
            log::info!("schema upgraded to v31 (cloud models: remove seed, add presets)");
        }
        return Ok(());
    }

    conn.execute_batch(INIT_SQL).context("执行 db.sql 建表 + seed")?;
    migrate_yaml_to_db(conn)?; // config.yaml 存在时一次性导入（导入后重命名 .bak），否则幂等返回
    // 填充 manifest（全新库 seed 中 secret_key 为空 → 从常量写入）
    fill_manifests(conn)?;
    conn.execute("PRAGMA user_version = 31", [])?;
    log::info!("DB initialized (v31): schema + seed + manifest fill + yaml 配置导入（无 yaml 则跳过）");
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
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_enabled = 1",
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
    pub category: String,
    pub model_name: String,
    pub source: String,
    /// local 模型重载为「文件清单 + sha256」JSON（见 model_commands）；api 模型仍是 API key。
    pub secret_key: String,
    pub description: String,
    pub is_enabled: bool,
    pub is_streaming: bool,
}

/// 列出全部本地 ASR 模型（domain='asr' AND is_local=1，**不过滤 is_enabled**）。
pub fn list_all_local_asr_models() -> Result<Vec<LocalAsrModelRow>> {
    with_db(list_all_local_asr_models_at)
}

fn list_all_local_asr_models_at(conn: &Connection) -> Result<Vec<LocalAsrModelRow>> {
    let mut stmt = conn.prepare(
        "SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_local = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LocalAsrModelRow {
            category: row.get(0)?,
            model_name: row.get(1)?,
            source: row.get(2)?,
            secret_key: row.get(3)?,
            description: row.get(4)?,
            is_enabled: row.get::<_, i32>(5)? != 0,
            is_streaming: row.get::<_, i32>(6)? != 0,
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
            "SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming
             FROM models WHERE domain=?1 AND is_local = 1",
        )?;
        let rows = stmt.query_map(params![domain], |row| {
            Ok(LocalAsrModelRow {
                category: row.get(0)?,
                model_name: row.get(1)?,
                source: row.get(2)?,
                secret_key: row.get(3)?,
                description: row.get(4)?,
                is_enabled: row.get::<_, i32>(5)? != 0,
                is_streaming: row.get::<_, i32>(6)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 设置某本地 ASR 模型就绪状态（is_enabled）。写 DB；调方需随后 reload 运行时缓存。
pub fn set_model_enabled(model_name: &str, enabled: bool) -> Result<()> {
    with_db(|conn| set_model_enabled_at(conn, model_name, enabled))
}

fn set_model_enabled_at(conn: &Connection, model_name: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE models SET is_enabled = ?1 WHERE model_name = ?2 AND domain='asr' AND is_local = 1",
        params![if enabled { 1 } else { 0 }, model_name],
    )?;
    Ok(())
}

/// 写某本地 ASR 模型的 secret_key（模型管理页存「文件清单 + sha256」JSON）。写 DB。
pub fn set_model_secret_key(model_name: &str, json: &str) -> Result<()> {
    with_db(|conn| set_model_secret_key_at(conn, model_name, json))
}

fn set_model_secret_key_at(conn: &Connection, model_name: &str, json: &str) -> Result<()> {
    conn.execute(
        "UPDATE models SET secret_key = ?1 WHERE model_name = ?2 AND domain='asr' AND is_local = 1",
        params![json, model_name],
    )?;
    Ok(())
}

// ── 云端模型 CRUD（用户自建，domain='asr'|'llm' AND is_local=0）──

/// 新增云端模型。返回新行 id。
pub fn insert_cloud_model(
    domain: &str, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<i64> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, secret_key, is_local, is_enabled, is_streaming, is_thinking)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, ?7, ?8)",
            params![domain, provider, category, model_name, source, secret_key,
                    is_streaming as i32, is_thinking as i32],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 更新云端模型（按 id）。
pub fn update_cloud_model(
    id: i64, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE models SET provider=?1, category=?2, model_name=?3, source=?4,
             secret_key=?5, is_streaming=?6, is_thinking=?7 WHERE id=?8",
            params![provider, category, model_name, source, secret_key,
                    is_streaming as i32, is_thinking as i32, id],
        )?;
        Ok(())
    })
}

/// 删除云端模型（物理删除，按 id）。
pub fn delete_cloud_model(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM models WHERE id=?1", params![id])?;
        Ok(())
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
/// 返回 Vec<(provider, base_url)>
pub fn list_llm_provider_presets() -> Result<Vec<(String, String)>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category='llm_provider' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
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
                 WHERE domain='llm' AND provider=?1 AND category=?2 AND model_name=?3 AND is_enabled = 1",
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
                 WHERE domain='llm' AND model_name=?1 AND is_enabled = 1
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

/// LLM 模型列表项（菜单用，仅含显示与排序所需字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmModelInfo {
    pub model_name: String,
    pub provider: String,
    pub category: String,
    pub is_local: bool,
}

/// 列出所有启用的 LLM 润色模型（domain='llm' AND is_enabled=1），按 is_local 降序、category 升序排序。
fn list_llm_models_at(conn: &Connection) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, is_local FROM models
         WHERE domain='llm' AND is_enabled = 1
         ORDER BY is_local DESC, category",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LlmModelInfo {
            provider: row.get::<_, String>(0)?,
            category: row.get::<_, String>(1)?,
            model_name: row.get::<_, String>(2)?,
            is_local: row.get::<_, i32>(3)? != 0,
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

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
    pub is_local: bool,
}

/// 列出所有 OCR 模型（domain='ocr'，含未就绪的——前端列表需展示全部供下载/切换）。
fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description, is_local FROM models
         WHERE domain='ocr'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
            is_local: row.get::<_, i32>(2)? != 0,
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

/// 按 id 更新 prompt（拒绝 is_system=1）。
fn update_prompt_at(conn: &Connection, id: i64, title: &str, content: &str, description: &str) -> Result<()> {
    let is_system: i32 = conn
        .query_row("SELECT is_system FROM prompts WHERE id=?1", params![id], |r| r.get(0))
        .context("prompt 不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置 prompt 不可编辑");
    }
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
}

const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts";

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
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard, shortcut, agent, accepts))
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
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, ?7, ?8, ?9, ?10, ?11)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts],
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
) -> Result<()> {
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts))
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
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, shortcut=?8, agent=?9, accepts=?10, updated_at=datetime('now') WHERE id=?11",
        params![title, icon, action_type, action_data, is_enabled as i32, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, id],
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

// ── Script Run（脚本执行记录）─────────────────────────────────────
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

// ── Agent Adapter（用户自定义 agent 适配器）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapterRecord {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
}

pub fn list_agent_adapter_records() -> Result<Vec<AgentAdapterRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, key, display_name, detect_binary, command_template FROM agent_adapters ORDER BY id"
        )?;
        let rows = stmt.query_map([], |r| Ok(AgentAdapterRecord {
            id: r.get(0)?,
            key: r.get(1)?,
            display_name: r.get(2)?,
            detect_binary: r.get(3)?,
            command_template: r.get(4)?,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn insert_agent_adapter_record(
    key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<i64> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES (?1, ?2, ?3, ?4)",
            params![key, display_name, detect_binary, command_template],
        )?;
        Ok(conn.last_insert_rowid())
    })
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
    with_db(|conn| {
        conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id])?;
        Ok(())
    })
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
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub words_text: String,
    pub created_at: String,
    pub updated_at: String,
}

const HOTWORD_SET_COLS: &str = "id, name, enabled, words_text, created_at, updated_at";

fn row_to_hotword_set(row: &rusqlite::Row) -> rusqlite::Result<HotwordSet> {
    Ok(HotwordSet {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        words_text: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// 列出全部版本（按 id 升序）。设置页渲染用。
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>> {
    with_db(|conn| list_hotword_sets_at(conn))
}

fn list_hotword_sets_at(conn: &Connection) -> Result<Vec<HotwordSet>> {
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
}

/// 单条查询（rename/toggle 后回读、命令层透传用）。
pub fn get_hotword_set(id: i64) -> Result<HotwordSet> {
    with_db(|conn| {
        conn.query_row(
            &format!("SELECT {c} FROM hotword_sets WHERE id=?1", c = HOTWORD_SET_COLS),
            params![id],
            row_to_hotword_set,
        )
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))
    })
}

/// 新建空版本。重名由 name UNIQUE 约束拒绝（→ Err）。
pub fn insert_hotword_set(name: &str) -> Result<i64> {
    with_db(|conn| insert_hotword_set_at(conn, name))
}

fn insert_hotword_set_at(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO hotword_sets (name) VALUES (?1)",
        params![name],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 改名。同时刷新 updated_at。
pub fn rename_hotword_set(id: i64, name: &str) -> Result<()> {
    with_db(|conn| rename_hotword_set_at(conn, id, name))
}

fn rename_hotword_set_at(conn: &Connection, id: i64, name: &str) -> Result<()> {
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
pub fn toggle_hotword_set(id: i64, enabled: bool) -> Result<()> {
    with_db(|conn| toggle_hotword_set_at(conn, id, enabled))
}

fn toggle_hotword_set_at(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
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
pub fn delete_hotword_set(id: i64) -> Result<()> {
    with_db(|conn| delete_hotword_set_at(conn, id))
}

fn delete_hotword_set_at(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
    if n == 0 {
        anyhow::bail!("热词版本不存在");
    }
    Ok(())
}

/// 覆盖写 words_text（已 normalize）。导入「覆盖」模式用。
pub fn set_hotword_set_words(id: i64, words_text: &str) -> Result<()> {
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
pub fn add_word_to_set(id: i64, word: &str) -> Result<bool> {
    with_db(|conn| add_word_to_set_at(conn, id, word))
}

fn add_word_to_set_at(conn: &Connection, id: i64, word: &str) -> Result<bool> {
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
pub fn add_words_to_set(id: i64, words: &[String]) -> Result<usize> {
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
pub fn remove_word_from_set(id: i64, word: &str) -> Result<()> {
    with_db(|conn| remove_word_from_set_at(conn, id, word))
}

fn remove_word_from_set_at(conn: &Connection, id: i64, word: &str) -> Result<()> {
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
pub fn list_recent_text(limit: i64) -> Result<Vec<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM clipboard_history
             WHERE item_type IN ('voice','text','ocr') AND content IS NOT NULL AND content != ''
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 在内存 DB 上执行 INIT_SQL，返回初始化好的连接。
    fn open_init() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
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
            &conn, None, "测试", "", "copy", "", true, false, "q", "", "text",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.shortcut, "q");
    }

    #[test]
    fn action_bar_update_shortcut() {
        let conn = open_init();
        update_action_bar_item_at(
            &conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "p", "", "text",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, 5).unwrap().unwrap();
        assert_eq!(item.shortcut, "p");
    }

    #[test]
    fn action_bar_shortcut_conflict_rejected() {
        let conn = open_init();
        // id=2 设快捷键 't'
        update_action_bar_item_at(&conn, 2, "翻译", "globe", "ai", "auto_translate", true, true, false, "t", "", "text").unwrap();
        // id=5 也想用 't' → 应失败
        let result = update_action_bar_item_at(&conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "t", "", "text");
        assert!(result.is_err());
    }

    /// 回归：`with_db` 的锁必须可重入——闭包内再调 `with_db` 不应死锁。
    /// 历史 `parking_lot::Mutex`（非递归）致同线程重入永久死锁（memory with-db-reentrant-deadlock）；
    /// 改 `ReentrantMutex` 后根治。此测试若退回 `Mutex` 会**挂起**（重入第二次 lock 永久阻塞）。
    /// 用只读 `PRAGMA` 避免污染数据；`ensure_db` 对已存在的 v18 库幂等（noop）。
    #[test]
    fn with_db_reentrant_no_deadlock() {
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
        cfg.asr_engine = "local:zipformer:sentinel".into();
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
        cfg.polish_llm = "local:sentinel-llm".into();
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
        cfg.clipboard_tab_modifier = "alt".into();
        cfg.screenshot_shortcut = "Alt+6".into();
        cfg.ocr_model = "sentinel-ocr".into();

        save_app_config_at(&conn, &cfg).unwrap();
        let loaded = load_app_config_at(&conn).unwrap();

        // Debug 格式全比较——任何字段未往返都会暴露差异。
        assert_eq!(format!("{:?}", loaded), format!("{:?}", cfg));
    }

    #[test]
    fn init_schema_fresh_db_builds_v25() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 31, "全新库 init_schema 后应到 v31");
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
    fn agent_adapter_crud_roundtrip() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('myagent', 'My Agent', 'myagent-bin', 'myagent {prompt}')",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();

        let rows: Vec<(i64, String, String, String, String)> = conn.prepare(
            "SELECT id, key, display_name, detect_binary, command_template FROM agent_adapters ORDER BY id"
        ).unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "myagent");
        assert_eq!(rows[0].4, "myagent {prompt}");

        conn.execute(
            "UPDATE agent_adapters SET key='myagent2', display_name='My Agent 2', detect_binary='myagent2-bin', command_template='myagent2 {prompt} {files}' WHERE id=?1",
            params![id],
        ).unwrap();
        let updated_key: String = conn.query_row(
            "SELECT key FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(updated_key, "myagent2");

        conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id]).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_adapters", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
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

    #[test]
    fn action_bar_submenu_accepts_default_any() {
        // init_schema 运行 v25→v26 迁移，submenu 项的 accepts 升级为 'any'
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let submenu_accepts: Vec<String> = conn.prepare(
            "SELECT accepts FROM action_bar_items WHERE action_type='submenu' ORDER BY id"
        ).unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        // 至少有 seed 的 AI(id=1) 和 搜索(id=3) 两个 submenu
        assert!(submenu_accepts.len() >= 2, "seed 应有 submenu 项");
        for a in &submenu_accepts {
            assert_eq!(a, "any", "submenu accepts 应为 'any'，实际: {}", a);
        }
    }

    #[test]
    fn migration_v25_to_v26_upgrades_submenu_accepts() {
        // 模拟老库：v25 schema（无 agent/accepts 列），手动插入 submenu 行
        let conn = Connection::open_in_memory().unwrap();
        // 先建一张 v25 风格的 action_bar_items（无 agent/accepts 列）
        conn.execute_batch(
            "CREATE TABLE action_bar_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id INTEGER DEFAULT NULL,
                title TEXT NOT NULL,
                icon TEXT NOT NULL DEFAULT '',
                action_type TEXT NOT NULL,
                action_data TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                is_system INTEGER NOT NULL DEFAULT 1,
                is_enabled INTEGER NOT NULL DEFAULT 1,
                is_async INTEGER NOT NULL DEFAULT 1,
                write_output_to_clipboard INTEGER NOT NULL DEFAULT 0,
                shortcut TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
            );
            INSERT INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system)
            VALUES (1, NULL, '测试菜单', '', 'submenu', '', 0, 1),
                   (2, NULL, '测试脚本', '', 'script', '', 1, 0);"
        ).unwrap();
        conn.execute("PRAGMA user_version = 25", []).unwrap();

        // 运行迁移
        init_schema(&conn).unwrap();

        // 验证：迁移后 user_version = 31
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 31);

        // 验证：submenu 行 accepts 升级为 'any'
        let accepts: String = conn.query_row(
            "SELECT accepts FROM action_bar_items WHERE id=1", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(accepts, "any", "老 submenu 应升级为 accepts='any'");

        // 验证：非 submenu 行保持 'text'
        let accepts2: String = conn.query_row(
            "SELECT accepts FROM action_bar_items WHERE id=2", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(accepts2, "text", "非 submenu 应保持 accepts='text'");

        // 验证：agent_adapters 表已建
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_adapters'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn action_bar_non_submenu_accepts_default_text() {
        // 非 submenu 类型 seed 的 accepts 保持 'text'（列默认值）
        let conn = open_init();
        let non_submenu: Vec<(String, String)> = conn.prepare(
            "SELECT action_type, accepts FROM action_bar_items WHERE action_type != 'submenu' ORDER BY id"
        ).unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert!(non_submenu.len() > 0, "seed 应有非 submenu 项");
        for (atype, accepts) in &non_submenu {
            assert_eq!(accepts, "text", "{} 类型 accepts 应为 'text'，实际: {}", atype, accepts);
        }
    }

    #[test]
    fn action_bar_insert_agent_type_default_accepts() {
        // 通过 insert 插入 agent 类型——不传 accepts 时默认 'text'
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "我的agent", "bot", "agent", "{{task}}", true, false, "", "claude", "file",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.accepts, "file");
        assert_eq!(item.agent, "claude");
    }

    #[test]
    fn migration_v26_to_v27_creates_agent_tasks_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("DROP TABLE agent_tasks", []).unwrap();
        conn.execute("PRAGMA user_version = 26", []).unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 31);
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
    fn init_schema_v25_is_noop() {
        // 已是 v27 的库再调 init_schema 应早退（不重跑、不报错）
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 27", []).unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 31);
    }

    /// HotwordSet 全 CRUD 往返：建 → 列 → 重名冲突 → 改名 → 启停 →
    /// 单词追加（去重 + normalize 拼音首字母排序）→ 单词移除 → 删版本。
    #[test]
    fn hotword_set_crud_roundtrip() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        // db.sql 现含默认「通用」版本 seed；本测试聚焦 CRUD 逻辑，清掉种子避免干扰 [0]/len 断言。
        conn.execute("DELETE FROM hotword_sets WHERE name='通用'", []).unwrap();

        // create
        let id = insert_hotword_set_at(&conn, "项目A").unwrap();
        assert!(id > 0);

        // list
        let sets = list_hotword_sets_at(&conn).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "项目A");
        assert!(sets[0].enabled);
        assert_eq!(sets[0].words_text, "");

        // 重名 → 唯一冲突
        assert!(insert_hotword_set_at(&conn, "项目A").is_err());

        // rename
        rename_hotword_set_at(&conn, id, "项目A2").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].name, "项目A2");

        // toggle enabled
        toggle_hotword_set_at(&conn, id, false).unwrap();
        assert!(!list_hotword_sets_at(&conn).unwrap()[0].enabled);
        toggle_hotword_set_at(&conn, id, true).unwrap();

        // add_word（normalize：序 + 去重）
        add_word_to_set_at(&conn, id, "吴大锐").unwrap();
        add_word_to_set_at(&conn, id, "八爪鱼").unwrap();
        add_word_to_set_at(&conn, id, "八爪鱼").unwrap(); // 重复 → 去重
        let s = list_hotword_sets_at(&conn).unwrap()[0].clone();
        assert_eq!(s.words_text, "八爪鱼 吴大锐"); // BZY < WDR

        // remove_word
        remove_word_from_set_at(&conn, id, "八爪鱼").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].words_text, "吴大锐");

        // delete set
        delete_hotword_set_at(&conn, id).unwrap();
        assert!(list_hotword_sets_at(&conn).unwrap().is_empty());
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
        // 强制启用所有模型做断言测试
        conn.execute("UPDATE models SET is_enabled = 1", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        // c796cbc 后本地 zipformer 2 条（zipformer / zipformer-large）；
        // 兜底 zipformer-small-ctc 移出 seed，由代码（asr/config.rs FALLBACK_ASR_ENGINE_NAME）写死
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section");
        assert_eq!(zf.len(), 2);
        let zp = zf.get("zipformer").unwrap();
        assert_eq!(zp.source, "asr/zipformer");
        assert!(zp.is_local, "ASR 模型应为本地模型");
        assert!(zp.is_enabled, "测试强制 is_enabled=1，此处应为 true");
        assert!(zp.is_streaming, "Zipformer 模型应支持流式");
        assert_eq!(cfg.asr.whisper.as_ref().unwrap().len(), 1);
        let whisper = cfg.asr.whisper.as_ref().unwrap().get("whisper-small").unwrap();
        assert!(!whisper.is_streaming, "Whisper 模型不应支持流式");
        // c796cbc 后本地 paraformer 4 条：bilingual / multi-zh / streaming / zh
        assert_eq!(cfg.asr.paraformer.as_ref().unwrap().len(), 4);
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 2);
        // moonshine ASR（base + tiny）
        assert_eq!(cfg.asr.moonshine.as_ref().unwrap().len(), 2);
        // 云端 ASR 已移除 seed（v31），用户自建
        assert!(cfg.asr.aliyun.is_none(), "aliyun 云端 ASR 不再 seed");
        assert!(cfg.asr.bytedance.is_none(), "bytedance 云端 ASR 不再 seed");
        assert!(cfg.asr.tencent.is_none(), "tencent 云端 ASR 不再 seed");
        assert!(cfg.asr.baidu.is_none(), "baidu 云端 ASR 不再 seed");
    }

    #[test]
    fn test_load_llm_model() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_thinking, is_local, is_enabled)
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
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_local, is_enabled)
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
    fn set_model_enabled_persists() {
        let conn = open_init();
        set_model_enabled_at(&conn, "paraformer-streaming", true).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(p.is_enabled);
        // 关掉再读
        set_model_enabled_at(&conn, "paraformer-streaming", false).unwrap();
        let rows = list_all_local_asr_models_at(&conn).unwrap();
        let p = rows.iter().find(|r| r.model_name == "paraformer-streaming").unwrap();
        assert!(!p.is_enabled);
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

    #[test]
    fn list_llm_models_filters_disabled_and_sorts() {
        let conn = open_init();
        // LLM 不再 seed（v31），插入测试数据
        conn.execute_batch(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_local, is_enabled)
             VALUES
             ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','',0,1),
             ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','',0,1),
             ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','',0,1),
             ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1),
             ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1),
             ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','',0,1)"
        ).unwrap();
        // 禁用 aliyun provider 下全部 3 条
        conn.execute(
            "UPDATE models SET is_enabled = 0 WHERE domain='llm' AND provider='aliyun'",
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
        // seed 全 is_enabled=0（默认）
        let list = list_llm_models_at(&conn).unwrap();
        assert!(list.is_empty(), "全禁用时返回空");
    }

    #[test]
    fn list_ocr_models_returns_all() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 2 条 OCR（PP-OCRv6-small is_enabled=1 + PP-OCRv5 is_enabled=0），全部返回
        assert_eq!(list.len(), 2, "seed 2 条 OCR，不过滤 is_enabled");
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv6-small"));
        assert!(list.iter().any(|m| m.model_name == "PP-OCRv5"));
    }

    #[test]
    fn list_ocr_models_includes_all_even_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_enabled = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        // 即使全部 is_enabled=0，仍返回全部（前端需展示供切换）
        assert_eq!(list.len(), 2, "全禁用时仍返回全部 OCR 模型");
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
        cfg.asr_engine = "whisper-small".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.microphone = "My Mic".into();
        cfg.segment_silence = 350.0;
        cfg.denoise_mode = 2;
        cfg.download_mirror = "https://hf-mirror.com".to_string();
        save_app_config_at(&conn, &cfg).unwrap();

        let cfg2 = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg2.asr_engine, "whisper-small");
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
            params!["asr_engine", "sensevoice-test"],
        ).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.asr_engine, "sensevoice-test");
        assert_eq!(cfg.language, "auto"); // 其余不变
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
        conn.execute_batch(INIT_SQL).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "重跑 INIT_SQL 不应重复 seed");
    }

    #[test]
    fn prompt_crud_round_trip() {
        let conn = open_init();
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

        // update 系统 prompt 被拒
        assert!(update_prompt_at(&conn, 1, "x", "y", "z").is_err());

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
        let id = insert_action_bar_item_at(&conn, None, "测试禁用", "test", "copy", "", true, false, "", "", "text").unwrap();
        update_action_bar_item_at(&conn, id, "测试禁用", "test", "copy", "", false, true, false, "", "", "text").unwrap();
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
        let id_a = insert_action_bar_item_at(&conn, None, "AAA", "test", "copy", "", true, false, "", "", "text").unwrap();
        let id_b = insert_action_bar_item_at(&conn, None, "BBB", "test", "copy", "", true, false, "", "", "text").unwrap();
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
    fn migrate_v22_hotwords_to_general_set() {
        // 构造 v22 库：hotwords 表 2 个 active（带 hit_count）+ 1 个 pending
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 22", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('八爪鱼','active','manual',3)", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('吴大锐','active','manual',1)", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('候选词','pending','mined',0)", []).unwrap();

        init_schema(&conn).unwrap();

        // v24
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 31);
        let (name, words_text): (String, String) = conn
            .query_row("SELECT name, words_text FROM hotword_sets WHERE name='通用'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "通用");
        assert_eq!(words_text, "八爪鱼 吴大锐"); // BZY, WDR 升序（B<W）

        // hit_count 迁入 hotword_hits
        let wu: i64 = conn
            .query_row("SELECT hit_count FROM hotword_hits WHERE word='吴大锐'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wu, 1);
        // pending 词不进 hits
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM hotword_hits", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn migrate_v22_merges_into_preexisting_general_set() {
        // 用户已手建同名「通用」版本（含「浮窗」），迁移应把 active 词并入，而非丢词。
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 22", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('八爪鱼','active','manual',3)", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('吴大锐','active','manual',1)", []).unwrap();
        // db.sql 已 seed 空「通用」；模拟用户已往里填「浮窗」
        conn.execute("UPDATE hotword_sets SET words_text='浮窗' WHERE name='通用'", []).unwrap();

        init_schema(&conn).unwrap();

        // 合并并集：八爪鱼(BZY) 浮窗(FC) 吴大锐(WDR) → B<F<W
        let words_text: String = conn
            .query_row("SELECT words_text FROM hotword_sets WHERE name='通用'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(words_text, "八爪鱼 浮窗 吴大锐");

        // 「通用」仍只有一行（未重复建）
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM hotword_sets WHERE name='通用'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
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
                    "SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming
                     FROM models WHERE domain='translate' AND is_local = 1",
                )
                .unwrap();
            let rows = stmt.query_map([], |row| {
                Ok(LocalAsrModelRow {
                    category: row.get(0)?,
                    model_name: row.get(1)?,
                    source: row.get(2)?,
                    secret_key: row.get(3)?,
                    description: row.get(4)?,
                    is_enabled: row.get::<_, i32>(5)? != 0,
                    is_streaming: row.get::<_, i32>(6)? != 0,
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
}
