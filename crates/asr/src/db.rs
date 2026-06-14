// crates/asr/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。
// cli/server/desktop 三端统一通过 config::load_config() 间接使用本模块。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::config::{handy_home, AppConfig, AsrSection, ModelEntry};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    handy_home().join("octopus.db")
}

/// 幂等初始化：打开/创建 DB，建表 + 首次 seed 默认引擎。
/// user_version==0 时建表+seed；已初始化的 DB 重启不重跑。
/// init_schema 幂等，多线程首次竞争也安全（INSERT OR IGNORE / CREATE IF NOT EXISTS）。
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

/// 建表 + 首次 seed（仅在 user_version==0 时）。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;
    if v == 0 {
        create_tables(conn)?;
        seed_default_models(conn)?;
        conn.execute("PRAGMA user_version = 1", [])?;
        log::info!("DB schema initialized (v1), default models seeded");
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

// ── 默认引擎 seed（替代 model.json）──

struct DefaultModel {
    category: &'static str,
    name: &'static str,
    source: &'static str,
    language: &'static str,
    description: &'static str,
    quantization: &'static str,
    is_active: bool,
}

/// 默认引擎集（替代 model.json）。
/// zipformer-small-ctc 走本地打包路径（开箱即用，active）；其余走 HF 缓存（按需下载）。
const DEFAULT_MODELS: &[DefaultModel] = &[
    DefaultModel {
        category: "zipformer",
        name: "zipformer-small-ctc",
        source: "models/zipformer",
        language: "zh",
        description: "zipformer-small-ctc, 27M (随应用打包)",
        quantization: "int8",
        is_active: true,
    },
    DefaultModel {
        category: "zipformer",
        name: "zipformer-multi",
        source: "k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13",
        language: "zh",
        description: "zipformer-multi, 80M",
        quantization: "int8",
        is_active: false,
    },
    DefaultModel {
        category: "zipformer",
        name: "zipformer-ctc",
        source: "csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30",
        language: "zh",
        description: "zipformer-ctc, 163M",
        quantization: "int8",
        is_active: false,
    },
    DefaultModel {
        category: "paraformer",
        name: "paraformer-streaming",
        source: "csukuangfj/sherpa-onnx-streaming-paraformer-zh",
        language: "zh",
        description: "paraformer-streaming, 230M",
        quantization: "int8",
        is_active: false,
    },
    DefaultModel {
        category: "sensevoice",
        name: "sherpa-onnx-sense-voice-funasr-nano-int8",
        source: "csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17",
        language: "auto",
        description: "SenseVoice FunASR Nano INT8, 265M",
        quantization: "int8",
        is_active: false,
    },
    DefaultModel {
        category: "qwen3-asr",
        name: "qwen3-asr-0.6B",
        source: "csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
        language: "auto",
        description: "qwen3-asr-0.6B, 1G",
        quantization: "int8",
        is_active: false,
    },
    DefaultModel {
        category: "whisper",
        name: "whisper-small",
        source: "onnx-community/whisper-small",
        language: "auto",
        description: "Whisper Small - 快速轻量, 250M",
        quantization: "int8",
        is_active: false,
    },
];

fn seed_default_models(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for m in DEFAULT_MODELS {
        tx.execute(
            "INSERT OR IGNORE INTO models
                (domain, category, name, source, language, description, quantization, is_active)
             VALUES ('asr', ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.category,
                m.name,
                m.source,
                m.language,
                m.description,
                m.quantization,
                m.is_active as i64
            ],
        )?;
    }
    tx.commit()?;
    log::info!("Seeded {} default models", DEFAULT_MODELS.len());
    Ok(())
}

// ── DB → AppConfig（load_config 用）──

/// 从 DB models 表构造 AppConfig（domain='asr'）。
pub fn load_models() -> Result<AppConfig> {
    with_db(|conn| load_models_at(conn))
}

fn load_models_at(conn: &Connection) -> Result<AppConfig> {
    let mut stmt = conn.prepare(
        "SELECT category, name, source, language, description, quantization, is_active
         FROM models WHERE domain='asr'",
    )?;
    let rows: Vec<(String, String, String, String, String, String, i64)> = stmt
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
        active: String::new(),
        whisper: None,
        sensevoice: None,
        paraformer: None,
        qwen3_asr: None,
        zipformer: None,
    };
    for (category, name, source, language, description, quantization, is_active) in rows {
        let entry = ModelEntry {
            source,
            language,
            description,
            quantization,
        };
        // category 存 JSON key（带 dash，如 "qwen3-asr"），按 dash 形式分派
        let map: &mut Option<HashMap<String, ModelEntry>> = match category.as_str() {
            "whisper" => &mut asr.whisper,
            "sensevoice" => &mut asr.sensevoice,
            "paraformer" => &mut asr.paraformer,
            "qwen3-asr" => &mut asr.qwen3_asr,
            "zipformer" => &mut asr.zipformer,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(name.clone(), entry);
        if is_active == 1 {
            asr.active = name;
        }
    }
    Ok(AppConfig { asr })
}

// ── 识别历史写入（desktop coordinator 用）──

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

    #[test]
    fn create_tables_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        create_tables(&conn).unwrap(); // 不 panic
    }

    #[test]
    fn seed_then_load_round_trips() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        seed_default_models(&conn).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert_eq!(cfg.asr.active, "zipformer-small-ctc");
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section");
        assert_eq!(zf.len(), 3);
        let small = zf.get("zipformer-small-ctc").unwrap();
        assert_eq!(small.source, "models/zipformer"); // 本地路径
        assert_eq!(cfg.asr.whisper.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.sensevoice.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.paraformer.as_ref().unwrap().len(), 1);
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn seed_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        seed_default_models(&conn).unwrap();
        seed_default_models(&conn).unwrap(); // INSERT OR IGNORE
        let cfg = load_models_at(&conn).unwrap();
        assert_eq!(cfg.asr.zipformer.as_ref().unwrap().len(), 3); // 未翻倍
    }

    #[test]
    fn insert_transcription_writes_row() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        insert_transcription_at(
            &conn,
            "原文",
            Some("润色"),
            "done",
            Some("deepseek"),
            "sensevoice",
            Some("offline"),
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transcriptions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn load_models_empty_db_returns_empty_sections() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert_eq!(cfg.asr.active, "");
        assert!(cfg.asr.whisper.is_none());
    }

    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }
}
