// crates/desktop/src/db.rs
// 嵌入式 SQLite 存储层：识别历史 + 模型配置。
// 全局单连接（OnceLock<Mutex<Connection>>），启动时 init()。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Mutex, OnceLock};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    octopus_asr::config::handy_home().join("octopus.db")
}

/// 启动时初始化：打开/创建 DB，建表 + 一次性迁移。
/// 仅在全新建库（user_version == 0）时跑迁移；已初始化的 DB 重启不重跑。
pub fn init() -> Result<()> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("Failed to open DB at {}", path.display()))?;
    init_schema(&conn)?;
    // set 失败说明重复 init，忽略
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// 取 DB 锁执行闭包。
fn with_db<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&Connection) -> Result<R>,
{
    let mutex = DB.get().context("DB not initialized")?;
    let conn = mutex.lock().unwrap();
    f(&conn)
}

/// 建表 + 迁移（仅在 user_version==0 时）。可单测：传入临时连接。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;
    if v == 0 {
        create_tables(conn)?;
        migrate_history(conn)?;
        migrate_model_json(conn)?;
        conn.execute("PRAGMA user_version = 1", [])?;
        log::info!("DB schema initialized (v1), migration done");
    }
    Ok(())
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS transcriptions (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at    TEXT    NOT NULL,
            engine        TEXT    NOT NULL,
            engine_mode   TEXT,
            raw_text      TEXT    NOT NULL,
            polished_text TEXT,
            polish_status TEXT    NOT NULL DEFAULT 'off',
            polish_model  TEXT,
            duration_ms   INTEGER,
            char_count    INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_trans_created ON transcriptions(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_trans_engine  ON transcriptions(engine);

        CREATE TABLE IF NOT EXISTS models (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            domain       TEXT    NOT NULL,
            category     TEXT    NOT NULL,
            name         TEXT    NOT NULL,
            source       TEXT    NOT NULL,
            language     TEXT    NOT NULL DEFAULT '',
            description  TEXT    NOT NULL DEFAULT '',
            quantization TEXT    NOT NULL DEFAULT '',
            is_active    INTEGER NOT NULL DEFAULT 0,
            UNIQUE(domain, category, name)
        );",
    )?;
    Ok(())
}

struct HistoryEntry {
    timestamp: String,
    body: String,
}

/// 解析 history.txt 内容（`--- timestamp ---\nbody` 分隔）。
fn parse_history_entries(content: &str) -> Vec<HistoryEntry> {
    let mut entries = Vec::new();
    let mut ts: Option<String> = None;
    let mut body = String::new();
    for line in content.lines() {
        if line.starts_with("--- ") && line.ends_with(" ---") {
            if let Some(t) = ts.take() {
                if !body.trim().is_empty() {
                    entries.push(HistoryEntry {
                        timestamp: t,
                        body: body.trim().to_string(),
                    });
                }
            }
            ts = Some(
                line.trim_start_matches("--- ")
                    .trim_end_matches(" ---")
                    .to_string(),
            );
            body.clear();
        } else if ts.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(t) = ts {
        if !body.trim().is_empty() {
            entries.push(HistoryEntry {
                timestamp: t,
                body: body.trim().to_string(),
            });
        }
    }
    entries
}

/// 校验 timestamp 是否为标准格式 'YYYY-MM-DD HH:MM:SS'。
fn is_standard_timestamp(ts: &str) -> bool {
    let b = ts.as_bytes();
    ts.len() == 19
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && b[16] == b':'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[17..19].iter().all(u8::is_ascii_digit)
}

/// 迁移 history.txt（默认路径）。文件不存在/为空则跳过。
fn migrate_history(conn: &Connection) -> Result<()> {
    let path = octopus_asr::config::handy_home().join("history.txt");
    migrate_history_at(conn, &path)
}

/// 迁移指定路径的 history.txt（可单测注入路径）。
///
/// 用 `unchecked_transaction()`（&self 事务）包裹整个循环：
/// 单连接由全局 `Mutex` 串行保护，此处独占持有，事务安全。
/// 中途任一 `tx.execute` 抛错 → `?` 早返回 → `tx` drop 自动回滚（RAII），
/// 保证原子性：要么全部插入，要么一条不留；避免半截迁移导致重复插入。
fn migrate_history_at(conn: &Connection, path: &std::path::Path) -> Result<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return Ok(()),
    };
    let entries = parse_history_entries(&content);
    if entries.is_empty() {
        return Ok(());
    }
    let count = entries.len();
    let tx = conn.unchecked_transaction()?;
    for e in &entries {
        if !is_standard_timestamp(&e.timestamp) {
            log::warn!(
                "history.txt entry has non-standard timestamp: {}",
                e.timestamp
            );
        }
        tx.execute(
            "INSERT INTO transcriptions
                (created_at, engine, engine_mode, raw_text, polished_text, polish_status, char_count)
             VALUES (?1, '', NULL, ?2, ?2, 'done', ?3)",
            params![e.timestamp, e.body, e.body.chars().count() as i64],
        )?;
    }
    tx.commit()?;
    log::info!("Migrated {} entries from history.txt", count);
    Ok(())
}

/// 迁移 model.json（默认路径）。文件不存在则跳过。
fn migrate_model_json(conn: &Connection) -> Result<()> {
    let path = octopus_asr::config::handy_home().join("model.json");
    migrate_model_json_at(conn, &path)
}

/// 迁移指定路径的 model.json（可单测注入路径）。
/// 用事务包裹 insert 循环，保证原子性（与 migrate_history_at 一致）。
fn migrate_model_json_at(conn: &Connection, path: &std::path::Path) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let v: serde_json::Value = serde_json::from_str(&text).context("parse model.json")?;

    let tx = conn.unchecked_transaction()?;

    // ASR 域：active + 各 category 的 {name → entry}
    if let Some(asr) = v.get("asr") {
        let active = asr.get("active").and_then(|a| a.as_str()).unwrap_or("");
        if let Some(map) = asr.as_object() {
            for (category, entries) in map {
                if category == "active" {
                    continue;
                }
                if let Some(em) = entries.as_object() {
                    for (name, entry) in em {
                        insert_model(&tx, "asr", category, name, entry, name == active)?;
                    }
                }
            }
        }
    }

    // VAD 域：active + silero {name → entry}
    if let Some(vad) = v.get("vad") {
        let active = vad.get("active").and_then(|a| a.as_str()).unwrap_or("");
        if let Some(silero) = vad.get("silero").and_then(|s| s.as_object()) {
            for (name, entry) in silero {
                insert_model(&tx, "vad", "silero", name, entry, name == active)?;
            }
        }
    }

    tx.commit()?;
    log::info!("Migrated model.json → models table");
    Ok(())
}

fn insert_model(
    conn: &Connection,
    domain: &str,
    category: &str,
    name: &str,
    entry: &serde_json::Value,
    is_active: bool,
) -> Result<()> {
    let source = entry.get("source").and_then(|s| s.as_str()).unwrap_or("");
    let language = entry.get("language").and_then(|s| s.as_str()).unwrap_or("");
    let description = entry
        .get("description")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let quantization = entry
        .get("quantization")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    conn.execute(
        "INSERT OR IGNORE INTO models
            (domain, category, name, source, language, description, quantization, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            domain,
            category,
            name,
            source,
            language,
            description,
            quantization,
            is_active as i64
        ],
    )?;
    Ok(())
}

/// 当前激活的模型（某 domain 下 is_active=1 的行）。
/// 只读投影，不含 id/is_active/description；切换引擎（UPDATE is_active）需另查 id。
pub struct ActiveModel {
    pub category: String,
    pub name: String,
    pub source: String,
    pub language: String,
    pub quantization: String,
}

/// 插入一条识别记录（指定连接，可单测）。
fn insert_transcription_at(
    conn: &Connection,
    raw_text: &str,
    polished_text: Option<&str>,
    polish_status: &str,
    polish_model: Option<&str>,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    let created_at = now_string();
    let display = polished_text.unwrap_or(raw_text);
    let char_count = display.chars().count() as i64;
    conn.execute(
        "INSERT INTO transcriptions
            (created_at, engine, engine_mode, raw_text, polished_text, polish_status, polish_model, char_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            created_at,
            engine,
            engine_mode,
            raw_text,
            polished_text,
            polish_status,
            polish_model,
            char_count
        ],
    )?;
    Ok(())
}

/// 对外：用全局连接插入一条识别记录。
/// - raw_text：原生识别全文（必有）
/// - polished_text：仅 polish_status='done' 时传 Some，否则 None
/// - polish_status：'off' | 'done' | 'failed'
pub fn insert_transcription(
    raw_text: &str,
    polished_text: Option<&str>,
    polish_status: &str,
    polish_model: Option<&str>,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    with_db(|conn| {
        insert_transcription_at(
            conn,
            raw_text,
            polished_text,
            polish_status,
            polish_model,
            engine,
            engine_mode,
        )
    })
}

fn active_engine_at(conn: &Connection, domain: &str) -> Result<Option<ActiveModel>> {
    let row = conn
        .query_row(
            "SELECT category, name, source, language, quantization
             FROM models WHERE domain=?1 AND is_active=1",
            params![domain],
            |r| {
                Ok(ActiveModel {
                    category: r.get(0)?,
                    name: r.get(1)?,
                    source: r.get(2)?,
                    language: r.get(3)?,
                    quantization: r.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// 对外：查询某 domain 的当前激活模型。
pub fn active_engine(domain: &str) -> Result<Option<ActiveModel>> {
    with_db(|conn| active_engine_at(conn, domain))
}

/// 当前时间字符串 'YYYY-MM-DD HH:MM:SS'（从 result_window 移植，避免依赖 chrono）。
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

    #[test]
    fn create_tables_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        create_tables(&conn).unwrap(); // 幂等，不报错
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('transcriptions','models')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn init_schema_sets_user_version_1_on_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap(); // stub 迁移不读文件，测试环境干净
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
    }

    /// days_to_ymd 边界单测。
    /// 每个输入 days 值与期望 (y,m,d) 均经 python3 独立验证：
    ///   python3 -c "from datetime import date; print((date(Y,M,D) - date(1970,1,1)).days)"
    #[test]
    fn days_to_ymd_boundary_cases() {
        // epoch：起点 0 天
        assert_eq!(days_to_ymd(0), (1970, 1, 1));

        // 平年月末跨月：1 月 31 日（days=30，从 0 计起）
        assert_eq!(days_to_ymd(30), (1970, 1, 31));
        // 下一天 2 月 1 日：跨月
        assert_eq!(days_to_ymd(31), (1970, 2, 1));

        // 闰年 Feb29 存在：2024 是闰年
        assert_eq!(days_to_ymd(19782), (2024, 2, 29));

        // 平年无 Feb29：2025 非闰，2 月最后一天是 28
        assert_eq!(days_to_ymd(20147), (2025, 2, 28));
        // 下一天直接 3 月 1 日（不存在 2 月 29）
        assert_eq!(days_to_ymd(20148), (2025, 3, 1));

        // 世纪平年：2100 能被 100 整除但不被 400 → 非闰
        assert_eq!(days_to_ymd(47540), (2100, 2, 28));
        assert_eq!(days_to_ymd(47541), (2100, 3, 1));

        // 能被 400 整除的闰年：2000 → 闰，2 月 29 存在
        assert_eq!(days_to_ymd(11016), (2000, 2, 29));

        // 跨年边界：2023-12-31 → 下一天 2024-01-01
        assert_eq!(days_to_ymd(19722), (2023, 12, 31));
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
    }

    #[test]
    fn parse_history_entries_extracts_timestamp_and_body() {
        let content = "--- 2026-06-13 10:00:00 ---\n第一句\n--- 2026-06-13 11:00:00 ---\n第二句\n";
        let entries = parse_history_entries(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp, "2026-06-13 10:00:00");
        assert_eq!(entries[0].body, "第一句");
        assert_eq!(entries[1].body, "第二句");
    }

    #[test]
    fn migrate_history_at_imports_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.txt");
        std::fs::write(&path, "--- 2026-06-13 10:00:00 ---\n你好世界\n").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        migrate_history_at(&conn, &path).unwrap();
        let (raw, status): (String, String) = conn
            .query_row(
                "SELECT raw_text, polish_status FROM transcriptions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(raw, "你好世界");
        assert_eq!(status, "done"); // 历史数据视为已润色
    }

    #[test]
    fn parse_history_entries_skips_empty_body() {
        let content = "--- 2026-06-13 10:00:00 ---\n\n--- 2026-06-13 11:00:00 ---\n有内容\n";
        let entries = parse_history_entries(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, "有内容");
    }

    #[test]
    fn parse_history_entries_handles_multiline_body() {
        let content = "--- 2026-06-13 10:00:00 ---\n第一行\n第二行\n";
        let entries = parse_history_entries(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, "第一行\n第二行");
    }

    #[test]
    fn migrate_model_json_at_imports_asr_and_vad() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.json");
        std::fs::write(
            &path,
            r#"{
              "vad": { "active": "", "silero": { "silero-vad": { "source": "onnx-community/silero-vad" } } },
              "asr": {
                "active": "paraformer-streaming",
                "paraformer": {
                  "paraformer-streaming": { "source": "csukuangfj/x", "language": "zh", "quantization": "int8" }
                }
              }
            }"#,
        )
        .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        migrate_model_json_at(&conn, &path).unwrap();

        // asr active 行
        let (name, is_active): (String, i64) = conn
            .query_row(
                "SELECT name, is_active FROM models WHERE domain='asr' AND is_active=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "paraformer-streaming");

        // vad silero（无 active）
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE domain='vad' AND category='silero'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn insert_transcription_then_query() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        insert_transcription_at(
            &conn,
            "raw text",
            Some("polished text"),
            "done",
            Some("deepseek-v4-flash"),
            "paraformer-streaming",
            Some("streaming"),
        )
        .unwrap();
        let (raw, polished, status, model): (String, Option<String>, String, Option<String>) = conn
            .query_row(
                "SELECT raw_text, polished_text, polish_status, polish_model FROM transcriptions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(raw, "raw text");
        assert_eq!(polished.as_deref(), Some("polished text"));
        assert_eq!(status, "done");
        assert_eq!(model.as_deref(), Some("deepseek-v4-flash"));
    }

    #[test]
    fn active_engine_returns_active_row() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO models (domain, category, name, source, language, description, quantization, is_active)
             VALUES ('asr','paraformer','paraformer-streaming','src','zh','','int8',1)",
            [],
        )
        .unwrap();
        let m = active_engine_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(m.name, "paraformer-streaming");
        assert_eq!(m.source, "src");
    }

    /// insert_transcription 的 None→NULL 路径：
    /// polished_text=None / polish_model=None / engine_mode=None 时写 NULL，
    /// 且 char_count 用 display = polished.unwrap_or(raw) = raw_text 的字符数。
    #[test]
    fn insert_transcription_with_none_fields() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        insert_transcription_at(
            &conn,
            "原生文本abc",
            None,
            "off",
            None,
            "paraformer-streaming",
            None,
        )
        .unwrap();
        let (raw, polished, status, model, mode, count): (
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            i64,
        ) = conn
            .query_row(
                "SELECT raw_text, polished_text, polish_status, polish_model, engine_mode, char_count FROM transcriptions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(raw, "原生文本abc");
        assert_eq!(polished, None);
        assert_eq!(status, "off");
        assert_eq!(model, None);
        assert_eq!(mode, None);
        // char_count 用 display = polished.unwrap_or(raw) = "原生文本abc" 的字符数
        assert_eq!(count, "原生文本abc".chars().count() as i64);
    }

    /// active_engine 无 active 行（is_active=0）时返回 None。
    #[test]
    fn active_engine_returns_none_when_no_active() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        // 插入一行 is_active=0（非激活）
        conn.execute(
            "INSERT INTO models (domain, category, name, source, language, description, quantization, is_active)
             VALUES ('asr','paraformer','paraformer-streaming','src','zh','','int8',0)",
            [],
        )
        .unwrap();
        let m = active_engine_at(&conn, "asr").unwrap();
        assert!(m.is_none());
    }
}
