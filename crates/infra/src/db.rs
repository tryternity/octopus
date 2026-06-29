// crates/infra/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
    pub sensevoice: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub paraformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default, rename = "qwen3-asr")]
    pub qwen3_asr: Option<HashMap<String, ModelEntry>>,
    #[serde(default)]
    pub zipformer: Option<HashMap<String, ModelEntry>>,
    /// Moonshine 端侧 ASR（Useful Sensors）。provider='local' + category='moonshine' 路由入此。
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,
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

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// 编译期嵌入的建表 + seed SQL（来自 crates/infra/src/db.sql）
const INIT_SQL: &str = include_str!("db.sql");

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    crate::paths::octopus_config_home().join("octopus.db")
}

/// 幂等初始化：打开/创建 DB，user_version=0 时执行 INIT_SQL 建表+seed。
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
    init_schema(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// 取 DB 锁执行闭包（未初始化时自动 ensure_db）。
pub fn with_db<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&Connection) -> Result<R>,
{
    if DB.get().is_none() {
        ensure_db()?;
    }
    let mutex = DB.get().context("DB not initialized")?;
    let conn = mutex.lock().unwrap();
    f(&conn)
}

/// 初始化 schema + 迁移：
/// - v0（全新安装）: 执行 INIT_SQL → yaml 迁移 → v4
/// - v1（旧版升级）: 重跑 INIT_SQL（幂等，补建 app_config + prompts + seed）→ yaml 迁移 → v4
/// - v2（v2 升级）: ALTER TABLE app_config ADD COLUMN category → 重跑 INIT_SQL → v5
/// - v3（v3 升级）: 重跑 INIT_SQL（幂等，补建 prompts 表 + seed）→ v5
/// - v4（v4 升级）: 重跑 INIT_SQL（幂等，补建 clipboard_history + FTS5）→ v5
/// - v5+: 跳过
///
/// INIT_SQL 全部为 CREATE TABLE IF NOT EXISTS + INSERT OR IGNORE，幂等安全重跑。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v < 2 {
        // v0: 首次建表 + seed；v1: 幂等重跑（旧表跳过，app_config 新建 + seed）
        conn.execute_batch(INIT_SQL).context("执行 db.sql 初始化失败")?;
        // 一次性 yaml → DB 迁移
        migrate_yaml_to_db(conn)?;
        // v0/v1 跳过 v2-v5，直接到 v6（INIT_SQL 建全部表，category 默认 'setting'）
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB initialized (v7): schema + app_config(setting) + prompts + clipboard_history + image_data + yaml migration");
    } else if v == 2 {
        // v2 → v4：app_config 补 category 列；prompts 表 + app_config seed 由 INIT_SQL 幂等补建
        log::info!("DB migrating v2 → v4: adding app_config.category column + prompts table...");
        conn.execute(
            "ALTER TABLE app_config ADD COLUMN category TEXT NOT NULL DEFAULT 'setting'",
            [],
        )?;
        conn.execute_batch(INIT_SQL).context("v2→v7: 重跑 db.sql 幂等补建 prompts + clipboard_history + image_data")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated to v7: app_config.category + prompts + clipboard_history + image_data");
    } else if v == 3 {
        // v3 → v4：prompts 表 + app_config.active_polish_prompt seed（INIT_SQL 幂等补建）
        log::info!("DB migrating v3 → v4: adding prompts table + active_polish_prompt seed...");
        conn.execute_batch(INIT_SQL).context("v3→v7: 重跑 db.sql 幂等补建 clipboard_history + image_data")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated to v7: prompts + clipboard_history + image_data");
    } else if v == 4 {
        // v4 → v5：clipboard_history 表 + FTS5 + 触发器 + app_config seed
        log::info!("DB migrating v4 → v5: adding clipboard_history table...");
        conn.execute_batch(INIT_SQL).context("v4→v5: 建 clipboard_history 表 + FTS5")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated v4→v7 (skip v5/v6): clipboard_history + FTS5 + image_data");
    } else if v == 5 {
        // v5 → v6：app_config category 'default' → 'setting'（语义化分组）
        log::info!("DB migrating v5 → v6: app_config category 'default' → 'setting'...");
        conn.execute(
            "UPDATE app_config SET category = 'setting' WHERE category = 'default'",
            [],
        )?;
        conn.execute_batch(INIT_SQL).context("v5→v7: 补建 image_data 表")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated v5→v7: app_config category renamed + image_data");
    } else if v == 6 {
        // v6 → v7：image_data 表
        log::info!("DB migrating v6 → v7: adding image_data table...");
        conn.execute_batch(INIT_SQL).context("v6→v7: 建 image_data 表")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated to v7: image_data");
    } else if v == 7 {
        // v7 → v8：FTS5 UPDATE 触发器收窄到 UPDATE OF search_text。
        // 旧 clip_fts_au 是 AFTER UPDATE（任意列），touch_created_at（更新 created_at）
        // 与 toggle_favorite（更新 is_favorite）等非搜索字段更新也会无谓 delete+insert
        // FTS 索引项。db.sql 里已改 OF search_text，但 CREATE TRIGGER IF NOT EXISTS 对
        // 已存在的旧库会跳过，故此处先 DROP 再重建，使现存库生效。
        log::info!("DB migrating v7 → v8: 收窄 clip_fts_au 到 UPDATE OF search_text");
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS clip_fts_au;
             CREATE TRIGGER clip_fts_au AFTER UPDATE OF search_text ON clipboard_history BEGIN
                 INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, search_text)
                 VALUES('delete', old.id, old.search_text);
                 INSERT INTO clipboard_history_fts(rowid, search_text) VALUES (new.id, new.search_text);
             END;",
        )
        .context("v7→v8: 重建 clip_fts_au 触发器")?;
        conn.execute("PRAGMA user_version = 8", [])?;
        log::info!("DB migrated to v8: clip_fts_au 限定 UPDATE OF search_text");
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
    with_db(|conn| load_app_config_at(conn))
}

fn load_app_config_at(conn: &Connection) -> Result<crate::config::AppConfig> {
    use crate::config::{AppConfig, PolishMode};
    let mut cfg = AppConfig::default();
    let mut stmt = conn.prepare(
        "SELECT config_key, config_value FROM app_config WHERE category = 'setting'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            // 字符串字段：直接赋值
            "engine_mode" => cfg.engine_mode = value,
            "remote_url" => cfg.remote_url = value,
            "grpc_endpoint" => cfg.grpc_endpoint = value,
            "asr_engine" => cfg.asr_engine = value,
            "language" => cfg.language = value,
            "asr_shortcut" => cfg.asr_shortcut = value,
            "edit_shortcut" => cfg.edit_shortcut = value,
            "paste_method" => cfg.paste_method = value,
            "microphone" => cfg.microphone = value,
            "overlay_position" => cfg.overlay_position = value,
            "polish_llm" => cfg.polish_llm = value,
            "ocr_model" => cfg.ocr_model = value,
            "download_mirror" => cfg.download_mirror = value,
            "clipboard_shortcut" => cfg.clipboard_shortcut = value,
            "edit_global_shortcut" => cfg.edit_global_shortcut = value,
            "polish_global_shortcut" => cfg.polish_global_shortcut = value,
            // i64 字段
            "clipboard_max_items" => { if let Ok(v) = value.parse() { cfg.clipboard_max_items = v; } }
            "clipboard_max_age_days" => { if let Ok(v) = value.parse() { cfg.clipboard_max_age_days = v; } }
            "screenshot_shortcut" => cfg.screenshot_shortcut = value,
            // bool 字段：parse 失败保留 default
            "write_to_clipboard" => { if let Ok(v) = value.parse() { cfg.write_to_clipboard = v; } }
            "asr_hardware_accelerated" => { if let Ok(v) = value.parse() { cfg.asr_hardware_accelerated = v; } }
            "asr_correct" => { if let Ok(v) = value.parse() { cfg.asr_correct = v; } }
            "output_simplified" => { if let Ok(v) = value.parse() { cfg.output_simplified = v; } }
            "hide_toolbar" => { if let Ok(v) = value.parse() { cfg.hide_toolbar = v; } }
            // f64 字段
            "segment_silence" => { if let Ok(v) = value.parse() { cfg.segment_silence = v; } }
            "polish_min_interval" => { if let Ok(v) = value.parse() { cfg.polish_min_interval = v; } }
            "pause_polish_threshold_ms" => { if let Ok(v) = value.parse() { cfg.pause_polish_threshold_ms = v; } }
            // u8 枚举字段
            "polish_mode" => {
                if let Ok(n) = value.parse::<u8>() {
                    cfg.polish_mode = match n {
                        1 => PolishMode::FinalOnly,
                        2 => PolishMode::Intermediate,
                        _ => PolishMode::Disabled,
                    };
                }
            }
            "denoise_mode" => { if let Ok(v) = value.parse() { cfg.denoise_mode = v; } }
            _ => {} // 忽略未知 key（前向兼容）
        }
    }
    Ok(cfg)
}

/// 全量写入应用配置（29 字段 ON CONFLICT DO UPDATE）。set_config / yaml 迁移用。
/// 仅更新 config_value，保留 description + category（不同于 INSERT OR REPLACE 会清空非指定列）。
pub fn save_app_config(cfg: &crate::config::AppConfig) -> Result<()> {
    ensure_db()?;
    with_db(|conn| save_app_config_at(conn, cfg))
}

fn save_app_config_at(conn: &Connection, cfg: &crate::config::AppConfig) -> Result<()> {
    use crate::config::PolishMode;
    let polish_mode_u8 = match cfg.polish_mode {
        PolishMode::Disabled => 0u8,
        PolishMode::FinalOnly => 1,
        PolishMode::Intermediate => 2,
    };
    let fields: [(&str, String); 29] = [
        ("engine_mode", cfg.engine_mode.clone()),
        ("remote_url", cfg.remote_url.clone()),
        ("grpc_endpoint", cfg.grpc_endpoint.clone()),
        ("asr_engine", cfg.asr_engine.clone()),
        ("language", cfg.language.clone()),
        ("asr_shortcut", cfg.asr_shortcut.clone()),
        ("edit_shortcut", cfg.edit_shortcut.clone()),
        ("paste_method", cfg.paste_method.clone()),
        ("write_to_clipboard", cfg.write_to_clipboard.to_string()),
        ("microphone", cfg.microphone.clone()),
        ("overlay_position", cfg.overlay_position.clone()),
        ("segment_silence", cfg.segment_silence.to_string()),
        ("polish_mode", polish_mode_u8.to_string()),
        ("polish_min_interval", cfg.polish_min_interval.to_string()),
        ("pause_polish_threshold_ms", cfg.pause_polish_threshold_ms.to_string()),
        ("polish_llm", cfg.polish_llm.clone()),
        ("ocr_model", cfg.ocr_model.clone()),
        ("asr_hardware_accelerated", cfg.asr_hardware_accelerated.to_string()),
        ("asr_correct", cfg.asr_correct.to_string()),
        ("output_simplified", cfg.output_simplified.to_string()),
        ("hide_toolbar", cfg.hide_toolbar.to_string()),
        ("denoise_mode", cfg.denoise_mode.to_string()),
        ("download_mirror", cfg.download_mirror.clone()),
        ("clipboard_shortcut", cfg.clipboard_shortcut.clone()),
        ("edit_global_shortcut", cfg.edit_global_shortcut.clone()),
        ("polish_global_shortcut", cfg.polish_global_shortcut.clone()),
        ("clipboard_max_items", cfg.clipboard_max_items.to_string()),
        ("clipboard_max_age_days", cfg.clipboard_max_age_days.to_string()),
        ("screenshot_shortcut", cfg.screenshot_shortcut.clone()),
    ];
    for (key, value) in &fields {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, value],
        )?;
    }
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

// ── DB → AsrConfig（load_config 用）──

/// 从 DB models 表构造 AsrConfig（domain='asr'）。
pub fn load_models() -> Result<AsrConfig> {
    with_db(|conn| load_models_at(conn))
}

fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_enabled = 1",
    )?;
    let rows: Vec<(String, String, String, String, String, String, String, i32, i32, i32)> = stmt
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
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut asr = AsrSection {
        whisper: None,
        sensevoice: None,
        paraformer: None,
        qwen3_asr: None,
        zipformer: None,
        moonshine: None,
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
            (_, "sensevoice") => &mut asr.sensevoice,
            (_, "paraformer") => &mut asr.paraformer,
            (_, "qwen3-asr") => &mut asr.qwen3_asr,
            (_, "zipformer") => &mut asr.zipformer,
            (_, "moonshine") => &mut asr.moonshine,
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

/// 从 DB 加载 LLM 配置（domain='llm'）。
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
    with_db(|conn| list_llm_models_at(conn))
}

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
}

/// 列出所有启用的 OCR 模型（domain='ocr' AND is_enabled=1）。
fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description FROM models
         WHERE domain='ocr' AND is_enabled = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
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
    with_db(|conn| list_ocr_models_at(conn))
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

// ── 识别历史写入（desktop coordinator 用）──

/// 首次有 ASR 文本时插入（应用写入毫秒戳 id）。
pub fn insert_transcription_at_id(
    id: i64,
    raw_text: &str,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    with_db(|conn| {
        let created_at = now_string();
        let char_count = raw_text.chars().count() as i64;
        conn.execute(
            "INSERT INTO transcriptions
                (id, created_at, engine, engine_mode, raw_text, polished_text, polish_status, char_count)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'off', ?6)",
            params![id, created_at, engine, engine_mode, raw_text, char_count],
        )?;
        Ok(())
    })
}

/// 分段后更新 raw_text（完整 ASR = raw + increase）。
pub fn update_raw_text(id: i64, raw_text: &str) -> Result<()> {
    with_db(|conn| {
        let char_count = raw_text.chars().count() as i64;
        conn.execute(
            "UPDATE transcriptions SET raw_text=?1, char_count=?2 WHERE id=?3",
            params![raw_text, char_count, id],
        )?;
        Ok(())
    })
}

/// 停顿润色后更新 polished_text。
pub fn update_polished(
    id: i64,
    polished_text: &str,
    polish_status: &str,
    polish_model: Option<&str>,
) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE transcriptions SET polished_text=?1, polish_status=?2, polish_model=?3 WHERE id=?4",
            params![polished_text, polish_status, polish_model, id],
        )?;
        Ok(())
    })
}

/// 用户提交编辑 / 中间润色折回后更新 edited_text。
pub fn update_edited_text(id: i64, edited_text: &str) -> Result<()> {
    with_db(|conn| {
        update_edited_text_at(conn, id, edited_text)?;
        Ok(())
    })
}

/// 接裸连接版本（供测试用 `open_init()` 内存 conn 走真实代码）。返回实际更新的行数。
fn update_edited_text_at(conn: &Connection, id: i64, edited_text: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
        params![edited_text, id],
    )?)
}

/// 识别结束 finalize：写最终 raw/polished/status/char_count/duration_ms。
pub fn finalize_transcription(
    id: i64,
    raw_text: &str,
    polished_text: Option<&str>,
    polish_status: &str,
    polish_model: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    with_db(|conn| {
        let display = polished_text.unwrap_or(raw_text);
        let char_count = display.chars().count() as i64;
        conn.execute(
            "UPDATE transcriptions SET raw_text=?1, polished_text=?2, polish_status=?3, polish_model=?4, char_count=?5, duration_ms=?6 WHERE id=?7",
            params![raw_text, polished_text, polish_status, polish_model, char_count, duration_ms, id],
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
    pub raw_text: String,
    pub polished_text: Option<String>,
    /// 用户编辑后的最终文本（None=未编辑，回退用 polished_text/raw_text）。
    pub edited_text: Option<String>,
    pub polish_status: String,
    pub duration_ms: Option<i64>,
}

/// 分页查询历史识别记录（按 id 降序 = 最新在前）。可选搜索关键词。
pub fn list_transcriptions(limit: u32, offset: u32, search: Option<&str>) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| {
        if let Some(q) = search {
            if !q.is_empty() {
                let pattern = format!("%{}%", q);
                let mut stmt = conn.prepare(
                    "SELECT id, created_at, engine, raw_text, polished_text, edited_text, polish_status, duration_ms
                     FROM transcriptions
                     WHERE raw_text LIKE ?1 OR polished_text LIKE ?1 OR edited_text LIKE ?1
                     ORDER BY id DESC LIMIT ?2 OFFSET ?3"
                )?;
                let rows = stmt.query_map(params![pattern, limit, offset], |row| {
                    Ok(TranscriptionRecord {
                        id: row.get(0)?, created_at: row.get(1)?, engine: row.get(2)?,
                        raw_text: row.get(3)?, polished_text: row.get(4)?, edited_text: row.get(5)?,
                        polish_status: row.get(6)?, duration_ms: row.get(7)?,
                    })
                })?;
                return Ok(rows.filter_map(|r| r.ok()).collect());
            }
        }
        list_transcriptions_at(conn, limit, offset)
    })
}

/// 批量删除识别记录（按 id）。返回实际删除的行数。
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
    let sql = format!("DELETE FROM transcriptions WHERE id IN ({})", placeholders);
    let n = conn.execute(&sql, params.as_slice())?;
    Ok(n)
}

fn list_transcriptions_at(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TranscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, engine, raw_text, polished_text, edited_text, polish_status, duration_ms
         FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
    )?;
    let rows = stmt.query_map(params![limit, offset], |row| {
        Ok(TranscriptionRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            engine: row.get(2)?,
            raw_text: row.get(3)?,
            polished_text: row.get(4)?,
            edited_text: row.get(5)?,
            polish_status: row.get(6)?,
            duration_ms: row.get(7)?,
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
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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
    fn init_sql_is_idempotent() {
        let conn = open_init();
        conn.execute_batch(INIT_SQL).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM models WHERE domain='asr'", [], |r| r.get(0))
            .unwrap();
        // 12 local + 2 bytedance + 2 tencent + 1 baidu + 3 aliyun (Fun-ASR + Paraformer + Qwen-ASR)
        assert_eq!(count, 20);
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
        assert_eq!(zp.source, "csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30");
        assert!(zp.is_local, "ASR 模型应为本地模型");
        assert!(zp.is_enabled, "测试强制 is_enabled=1，此处应为 true");
        assert!(zp.is_streaming, "Zipformer 模型应支持流式");
        assert_eq!(cfg.asr.whisper.as_ref().unwrap().len(), 1);
        let whisper = cfg.asr.whisper.as_ref().unwrap().get("whisper-small").unwrap();
        assert!(!whisper.is_streaming, "Whisper 模型不应支持流式");
        assert_eq!(cfg.asr.sensevoice.as_ref().unwrap().len(), 1);
        // c796cbc 后本地 paraformer 4 条：bilingual / multi-zh / streaming / zh
        assert_eq!(cfg.asr.paraformer.as_ref().unwrap().len(), 4);
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 2);
        // moonshine ASR（base + tiny）
        assert_eq!(cfg.asr.moonshine.as_ref().unwrap().len(), 2);
        // aliyun ASR（Fun-ASR / Paraformer / Qwen-ASR）
        let aliyun = cfg.asr.aliyun.as_ref().expect("aliyun section");
        assert_eq!(aliyun.len(), 3);
        // bytedance ASR（Doubao-ASR 1.0 + 2.0）
        let bytedance = cfg.asr.bytedance.as_ref().expect("bytedance section");
        assert_eq!(bytedance.len(), 2);
        let doubao = bytedance.get("doubao-asr-1.0-streaming").unwrap();
        assert_eq!(doubao.source, "volc.bigasr.sauc.duration");
        assert!(!doubao.is_local, "bytedance 模型非本地");
        assert!(doubao.is_enabled, "测试强制 is_enabled=1，此处应为 true");
        assert!(doubao.is_streaming, "Doubao-ASR 应支持流式");
        // tencent ASR（16k_zh + 16k_zh_en）
        let tencent = cfg.asr.tencent.as_ref().expect("tencent section");
        assert_eq!(tencent.len(), 2);
        let tc_zh = tencent.get("16k_zh").unwrap();
        assert!(!tc_zh.is_local, "tencent 模型非本地");
        assert!(tc_zh.is_streaming, "tencent ASR 应支持流式");
        // baidu ASR（15372 中文加强标点）
        let baidu = cfg.asr.baidu.as_ref().expect("baidu section");
        assert_eq!(baidu.len(), 1);
        let bd = baidu.get("15372").unwrap();
        assert!(!bd.is_local, "baidu 模型非本地");
        assert!(bd.is_streaming, "baidu ASR 应支持流式");
        let funasr = aliyun.get("fun-asr-realtime").unwrap();
        assert_eq!(funasr.source, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
        assert!(!funasr.is_local, "aliyun Fun-ASR 非本地");
        assert!(!funasr.is_streaming, "aliyun Fun-ASR 走 chunk 路径（is_streaming=0）");
        // Paraformer Realtime（共用 inference 端点）
        let paraformer = aliyun.get("paraformer-realtime-v2").unwrap();
        assert_eq!(paraformer.source, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
        // Qwen-ASR Realtime（realtime 端点 + OpenAI Realtime 协议）
        let qwen = aliyun.get("qwen3-asr-flash-realtime").unwrap();
        assert_eq!(qwen.source, "wss://dashscope.aliyuncs.com/api-ws/v1/realtime");
        assert!(qwen.is_streaming, "aliyun Qwen-ASR 走 CloudStreaming 路径（is_streaming=1）");
    }

    #[test]
    fn test_load_llm_model() {
        let conn = open_init();
        // 强制启用所有模型做断言测试
        conn.execute("UPDATE models SET is_enabled = 1", []).unwrap();

        // 3-part：bigmodel:glm:glm-4-flashx
        let glm = load_llm_model_at(&conn, "bigmodel:glm:glm-4-flashx")
            .unwrap()
            .unwrap();
        assert_eq!(glm.provider, "bigmodel");
        assert_eq!(glm.model, "glm-4-flashx");
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(glm.secret_key, "");
        assert!(!glm.is_thinking, "glm-4-flashx 不是思考模型");
        assert!(!glm.is_local, "glm-4-flashx 不是本地模型");
        assert!(glm.is_enabled, "glm-4-flashx 应为启用状态");

        // deepseek-v4-flash 在 deepseek 和 aliyun 两个 provider 下同名同系列，
        // 必须用 3-part "provider:category:model_name" 才能唯一定位。
        let ds = load_llm_model_at(&conn, "deepseek:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(ds.provider, "deepseek");
        assert_eq!(ds.model, "deepseek-v4-flash");
        assert_eq!(ds.base_url, "https://api.deepseek.com/");
        assert!(ds.is_thinking, "deepseek-v4-flash 是思考模型");
        assert!(!ds.is_local, "deepseek-v4-flash 不是本地模型");
        assert!(ds.is_enabled, "deepseek-v4-flash 应为启用状态");

        let aliyun = load_llm_model_at(&conn, "aliyun:deepseek:deepseek-v4-flash")
            .unwrap()
            .unwrap();
        assert_eq!(aliyun.provider, "aliyun");
        assert_eq!(aliyun.model, "deepseek-v4-flash");
        assert_eq!(
            aliyun.base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert!(aliyun.is_thinking, "aliyun 下的 deepseek-v4-flash 也是思考模型");
        assert!(aliyun.is_enabled);

        // Feature 1：aliyun:qwen 原生（同名 model_name 跨 provider）
        let qwen = load_llm_model_at(&conn, "aliyun:qwen:qwen-plus")
            .unwrap()
            .unwrap();
        assert_eq!(qwen.provider, "aliyun");
        assert_eq!(qwen.model, "qwen-plus");
        assert_eq!(
            qwen.base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert!(!qwen.is_thinking, "qwen-plus 非思考模型");

        // provider 不匹配时应返回 None（同 model_name 但不同 provider）
        assert!(
            load_llm_model_at(&conn, "deepseek:qwen:qwen-plus")
                .unwrap()
                .is_none(),
            "deepseek 下不存在 qwen:qwen-plus"
        );
        // category 不匹配也应返回 None
        assert!(
            load_llm_model_at(&conn, "bigmodel:deepseek:deepseek-v4-flash")
                .unwrap()
                .is_none(),
            "bigmodel 下不存在 deepseek 系列"
        );

        let glm_think = load_llm_model_at(&conn, "bigmodel:glm:glm-4.5-flash")
            .unwrap()
            .unwrap();
        assert!(glm_think.is_thinking, "glm-4.5-flash 是思考模型");
        assert!(!glm_think.is_local, "glm-4.5-flash 不是本地模型");
        assert!(glm_think.is_enabled, "glm-4.5-flash 应为启用状态");

        // 裸名（NameOnly）：跨 provider/category 搜 model_name，优先 local。
        // seed 中所有 LLM 均 is_local=0，但仍可查到（ORDER BY is_local DESC 兜底）。
        let bare = load_llm_model_at(&conn, "glm-4-flashx").unwrap().unwrap();
        assert_eq!(bare.model, "glm-4-flashx");
        assert!(!bare.is_local);
        assert!(bare.provider.is_empty(), "NameOnly 时 provider 字段为空串（仅日志用）");

        assert!(load_llm_model_at(&conn, "nonexistent-model").unwrap().is_none());

        // 插入一个 is_local=1 的 LLM 行，验证 3-part 能精确命中本地模型
        //（注：model_name 必须不含冒号，3-part spec 不支持冒号嵌入 name）
        conn.execute(
            "INSERT INTO models (domain, provider, category, model_name, source, description, is_local, is_enabled)
             VALUES ('llm', 'ollama', 'qwen', 'qwen3-8b', 'http://localhost:11434/v1', 'local ollama', 1, 1)",
            [],
        )
        .unwrap();
        let local_llm = load_llm_model_at(&conn, "ollama:qwen:qwen3-8b").unwrap().unwrap();
        assert_eq!(local_llm.provider, "ollama");
        assert_eq!(local_llm.model, "qwen3-8b");
        assert!(local_llm.is_local, "ollama 本地模型应命中");
        // 裸名也应能命中（NameOnly 跨 provider/category 搜，优先 local）
        let bare_local = load_llm_model_at(&conn, "qwen3-8b").unwrap().unwrap();
        assert_eq!(bare_local.model, "qwen3-8b");
        assert!(bare_local.is_local);
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
        // 验证列出全部 12 条本地 ASR，无 models/ 开头的随包行
        assert_eq!(rows.len(), 12, "本地 ASR 清单应含 12 条");
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
        // seed 默认 6 条 LLM 全 is_enabled=0；全部启用
        conn.execute("UPDATE models SET is_enabled = 1 WHERE domain='llm'", []).unwrap();
        // 再禁用 aliyun provider 下全部 3 条（deepseek-v4-flash + qwen-plus + qwen-turbo）
        conn.execute(
            "UPDATE models SET is_enabled = 0 WHERE domain='llm' AND provider='aliyun'",
            [],
        ).unwrap();
        let list = list_llm_models_at(&conn).unwrap();
        // 剩余 3 条（全 is_local=0）→ is_local desc 无影响 → category 字母序
        // categories: deepseek(deepseek-v4-flash), glm(glm-4-flashx), glm(glm-4.5-flash)
        assert_eq!(list.len(), 3, "aliyun 3 条被禁用应过滤");
        assert_eq!(
            list.iter().map(|m| m.category.as_str()).collect::<Vec<_>>(),
            vec!["deepseek", "glm", "glm"],
            "按 category 字母序"
        );
        assert!(list.iter().all(|m| !m.is_local), "seed LLM 全远程");
        // 同 category 内未显式二级排序（仅 is_local + category），但当前两条 glm 顺序
        // 不依赖 name——验证集合而非顺序
        let glm_names: Vec<&str> = list.iter()
            .filter(|m| m.category == "glm")
            .map(|m| m.model_name.as_str())
            .collect();
        let mut sorted = glm_names.clone();
        sorted.sort();
        assert_eq!(glm_names, sorted, "glm 两条均存在");
        assert!(sorted.contains(&"glm-4-flashx") && sorted.contains(&"glm-4.5-flash"));
    }

    #[test]
    fn list_llm_models_at_empty_when_all_disabled() {
        let conn = open_init();
        // seed 全 is_enabled=0（默认）
        let list = list_llm_models_at(&conn).unwrap();
        assert!(list.is_empty(), "全禁用时返回空");
    }

    #[test]
    fn list_ocr_models_returns_enabled() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 默认 1 条 OCR（PP-OCRv6-small, is_enabled=1）
        assert_eq!(list.len(), 1, "seed 1 条启用 OCR");
        assert_eq!(list[0].model_name, "PP-OCRv6-small");
        assert!(!list[0].description.is_empty(), "description 非空");
    }

    #[test]
    fn list_ocr_models_filters_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_enabled = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        assert!(list.is_empty(), "全禁用时返回空");
    }

    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }

    #[test]
    fn update_and_finalize_round_trip() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status, char_count)
             VALUES (100, '2026-06-14 00:00:00', 'sensevoice', '首段', NULL, 'off', 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE transcriptions SET raw_text='首段二段', char_count=4 WHERE id=100",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE transcriptions SET polished_text='润色', polish_status='done', polish_model='deepseek' WHERE id=100",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE transcriptions SET raw_text='首段二段', polished_text='润色', polish_status='done', char_count=2, duration_ms=5000 WHERE id=100",
            [],
        )
        .unwrap();

        let (raw, polished, status, dur): (String, Option<String>, String, Option<i64>) = conn
            .query_row(
                "SELECT raw_text, polished_text, polish_status, duration_ms FROM transcriptions WHERE id=100",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(raw, "首段二段");
        assert_eq!(polished, Some("润色".into()));
        assert_eq!(status, "done");
        assert_eq!(dur, Some(5000));
    }

    #[test]
    fn list_transcriptions_returns_records_descending() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status)
             VALUES (200, '2026-06-17 11:00:00', 'qwen3', '你好世界', '你好，世界。', 'done')",
            [],
        )
        .unwrap();
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[0].raw_text, "你好世界");
        assert_eq!(rows[0].polished_text.as_deref(), Some("你好，世界。"));
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
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (200, '2026-06-17 11:00:00', 'qwen3', '你好世界', 'off')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (300, '2026-06-17 12:00:00', 'sensevoice', '测试', 'off')",
            [],
        )
        .unwrap();
        let n = conn
            .execute("DELETE FROM transcriptions WHERE id IN (?,?)", params![200, 300])
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
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
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
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (200, '2026-06-17 11:00:00', 'qwen3', '世界', 'off')",
            [],
        )
        .unwrap();
        let n = delete_transcriptions_at(&conn, &[100, 200]).unwrap();
        assert_eq!(n, 2);
        assert!(list_transcriptions_at(&conn, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn update_edited_text_persists_and_lists() {
        let conn = open_init();
        // id=100：将被编辑的记录
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status)
             VALUES (100, '2026-06-18 10:00:00', 'whisper', 'raw原文', '润色稿', 'done')",
            [],
        )
        .unwrap();
        // id=200：未编辑的对照记录（验证 NULL → None 映射）
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (200, '2026-06-18 11:00:00', 'qwen3', '另一条', 'off')",
            [],
        )
        .unwrap();

        // 走真实 update_edited_text_at（而非裸 SQL），断言返回行数 1
        let n = update_edited_text_at(&conn, 100, "手改文本").unwrap();
        assert_eq!(n, 1);

        // 经 list_transcriptions_at 回读，同时验证 list 列序映射正确
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[1].edited_text.as_deref(), Some("手改文本"));
        // 未编辑记录：edited_text 为 NULL → Option None
        assert_eq!(rows[0].edited_text, None);

        // 不存在的 id：返回 0 行更新
        let missing = update_edited_text_at(&conn, 9999, "无效").unwrap();
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
        assert_eq!(cfg.edit_shortcut, "Cmd+Enter");
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
        assert_eq!(categories, vec!["setting"], "所有行 category 应为 'setting'");
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
}
