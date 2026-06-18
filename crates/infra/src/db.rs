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
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    #[serde(default)]
    pub aliyun: Option<HashMap<String, ModelEntry>>,
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
fn with_db<F, R>(f: F) -> Result<R>
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

/// 初始化 schema：user_version=0 时执行 INIT_SQL（建表 + seed），其余直接跳过。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;
    if v == 0 {
        conn.execute_batch(INIT_SQL).context("执行 db.sql 初始化失败")?;
        conn.execute("PRAGMA user_version = 1", [])?;
        log::info!("DB initialized (v1): schema + seed from db.sql");
    }
    Ok(())
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
        aliyun: None,
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
        // provider='aliyun' → asr.aliyun；其余按本地 category 映射本地族
        let map: &mut Option<HashMap<String, ModelEntry>> = match (provider.as_str(), category.as_str()) {
            ("aliyun", _) => &mut asr.aliyun,
            (_, "whisper") => &mut asr.whisper,
            (_, "sensevoice") => &mut asr.sensevoice,
            (_, "paraformer") => &mut asr.paraformer,
            (_, "qwen3-asr") => &mut asr.qwen3_asr,
            (_, "zipformer") => &mut asr.zipformer,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(model_name, entry);
    }
    Ok(AsrConfig { asr })
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

/// 分页查询历史识别记录（按 id 降序 = 最新在前）。
pub fn list_transcriptions(limit: u32, offset: u32) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| list_transcriptions_at(conn, limit, offset))
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
        // 8 local + 1 aliyun Fun-ASR
        assert_eq!(count, 9);
    }

    #[test]
    fn seed_then_load_round_trips() {
        let conn = open_init();
        // 强制启用所有模型做断言测试
        conn.execute("UPDATE models SET is_enabled = 1", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section");
        assert_eq!(zf.len(), 3);
        let small = zf.get("zipformer-small-ctc").unwrap();
        assert_eq!(small.source, "models/zipformer");
        assert!(small.is_local, "ASR 模型应为本地模型");
        assert!(small.is_enabled, "ASR 模型应为启用状态");
        assert!(small.is_streaming, "Zipformer 模型应支持流式");
        assert_eq!(cfg.asr.whisper.as_ref().unwrap().len(), 1);
        let whisper = cfg.asr.whisper.as_ref().unwrap().get("whisper-small").unwrap();
        assert!(!whisper.is_streaming, "Whisper 模型不应支持流式");
        assert_eq!(cfg.asr.sensevoice.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.paraformer.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 2);
        // aliyun Fun-ASR
        let aliyun = cfg.asr.aliyun.as_ref().expect("aliyun section");
        assert_eq!(aliyun.len(), 1);
        let funasr = aliyun.get("fun-asr-2025-11-07").unwrap();
        assert_eq!(funasr.source, "wss://dashscope.aliyuncs.com/api-ws/v1/inference");
        assert!(!funasr.is_local, "aliyun Fun-ASR 非本地");
        assert!(!funasr.is_streaming, "aliyun Fun-ASR 走 chunk 路径（is_streaming=0）");
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
}
