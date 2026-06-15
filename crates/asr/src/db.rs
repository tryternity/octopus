// crates/asr/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。
// cli/server/desktop 三端统一通过 config::load_config() 间接使用本模块。
//
// Schema 与 seed 数据统一维护于 crates/infra/src/db.sql，
// 通过 include_str! 在编译期嵌入，首次建库时执行一次。
// 开发阶段无迁移逻辑：schema 变更时删除 ~/.octopus/octopus.db 重新初始化即可。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::config::{AsrConfig, AsrSection, ModelEntry};
use octopus_infra::octopus_config_home;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// 编译期嵌入的建表 + seed SQL（来自 crates/infra/src/db.sql）
const INIT_SQL: &str = include_str!("../../infra/src/db.sql");

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    octopus_config_home().join("octopus.db")
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
    // set 失败说明另一线程已先行初始化，忽略（其 conn 会 drop）
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

// ── DB → AsrConfig（load_config 用）──

/// 从 DB models 表构造 AsrConfig（domain='asr'）。
pub fn load_models() -> Result<AsrConfig> {
    with_db(|conn| load_models_at(conn))
}

fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    let mut stmt = conn.prepare(
        "SELECT category, name, source, language, description, secret_key, is_local
         FROM models WHERE domain='asr' AND is_enabled = 1",
    )?;
    let rows: Vec<(String, String, String, String, String, String, i32)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
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
    };
    for (category, name, source, language, description, secret_key, is_local) in rows {
        let entry = ModelEntry {
            source,
            language,
            description,
            secret_key,
            is_local: is_local != 0,
        };
        let map: &mut Option<HashMap<String, ModelEntry>> = match category.as_str() {
            "whisper" => &mut asr.whisper,
            "sensevoice" => &mut asr.sensevoice,
            "paraformer" => &mut asr.paraformer,
            "qwen3-asr" => &mut asr.qwen3_asr,
            "zipformer" => &mut asr.zipformer,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(name, entry);
    }
    Ok(AsrConfig { asr })
}

/// 从 DB 加载指定名称的 LLM 配置（domain='llm'）。
pub fn load_llm_model(name: &str) -> Result<Option<octopus_llm::CompatibleLlmConfig>> {
    with_db(|conn| load_llm_model_at(conn, name))
}

fn load_llm_model_at(conn: &Connection, name: &str) -> Result<Option<octopus_llm::CompatibleLlmConfig>> {
    let mut stmt = conn.prepare(
        "SELECT category, source, secret_key, is_thinking, is_local
         FROM models WHERE domain='llm' AND name=?1 AND is_enabled = 1",
    )?;
    let mut rows = stmt.query_map(params![name], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i32>(3)?,
            row.get::<_, i32>(4)?,
        ))
    })?;
    if let Some(r) = rows.next() {
        let (category, source, secret_key, is_thinking, is_local) = r?;
        Ok(Some(octopus_llm::CompatibleLlmConfig {
            provider: category,
            model: name.to_string(),
            base_url: source,
            secret_key,
            is_thinking: is_thinking != 0,
            is_local: is_local != 0,
        }))
    } else {
        Ok(None)
    }
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
        // INSERT OR IGNORE + CREATE TABLE IF NOT EXISTS → 重复执行不报错、不翻倍
        conn.execute_batch(INIT_SQL).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM models WHERE domain='asr'", [], |r| r.get(0))
            .unwrap();
        // 应有 8 条 ASR 模型
        assert_eq!(count, 8);
    }

    #[test]
    fn seed_then_load_round_trips() {
        let conn = open_init();
        let cfg = load_models_at(&conn).unwrap();
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section");
        assert_eq!(zf.len(), 3);
        let small = zf.get("zipformer-small-ctc").unwrap();
        assert_eq!(small.source, "models/zipformer");
        assert!(small.is_local, "ASR 模型应为本地模型");
        assert_eq!(cfg.asr.whisper.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.sensevoice.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.paraformer.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_load_llm_model() {
        let conn = open_init();

        let glm = load_llm_model_at(&conn, "glm-4-flashx").unwrap().unwrap();
        assert_eq!(glm.provider, "bigmodel");
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(glm.secret_key, "");
        assert!(!glm.is_thinking, "glm-4-flashx 不是思考模型");
        assert!(!glm.is_local, "glm-4-flashx 不是本地模型");

        let ds = load_llm_model_at(&conn, "deepseek-v4-flash").unwrap().unwrap();
        assert_eq!(ds.provider, "deepseek");
        assert_eq!(ds.base_url, "https://api.deepseek.com/");
        assert!(ds.is_thinking, "deepseek-v4-flash 是思考模型");
        assert!(!ds.is_local, "deepseek-v4-flash 不是本地模型");

        let glm_think = load_llm_model_at(&conn, "glm-4.5-flash").unwrap().unwrap();
        assert!(glm_think.is_thinking, "glm-4.5-flash 是思考模型");
        assert!(!glm_think.is_local, "glm-4.5-flash 不是本地模型");

        assert!(load_llm_model_at(&conn, "nonexistent-model").unwrap().is_none());
    }

    #[test]
    fn test_is_enabled_filtering() {
        let conn = open_init();
        
        // 禁用 glm-4-flashx
        conn.execute("UPDATE models SET is_enabled = 0 WHERE name = 'glm-4-flashx'", []).unwrap();
        assert!(load_llm_model_at(&conn, "glm-4-flashx").unwrap().is_none());

        // 禁用 paraformer-streaming
        conn.execute("UPDATE models SET is_enabled = 0 WHERE name = 'paraformer-streaming'", []).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert!(cfg.asr.paraformer.is_none() || !cfg.asr.paraformer.unwrap().contains_key("paraformer-streaming"));
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
}
