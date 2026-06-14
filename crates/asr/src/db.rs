// crates/asr/src/db.rs
// 嵌入式 SQLite：模型配置（唯一来源）+ 识别历史。
// 全局单连接（OnceLock<Mutex<Connection>>），首次 ensure_db 时初始化。
// cli/server/desktop 三端统一通过 config::load_config() 间接使用本模块。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::config::{AsrConfig, AsrSection, ModelEntry};
use octopus_infra::{consts::DEFAULT_ASR_MODEL_DIR, octopus_config_home};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// DB 文件路径：~/.octopus/octopus.db
fn db_path() -> std::path::PathBuf {
    octopus_config_home().join("octopus.db")
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

/// 建表 + migration（按 user_version 分派）。
/// - v0：全新 DB → 建表（新 schema）+ seed → 升至 v3
/// - v1/v2：旧 DB → DROP 重建 transcriptions（id 改应用写入毫秒戳）+ 幂等补删 models.is_active → 升至 v3
/// - v3+：已就绪，no-op
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;
    match v {
        0 => {
            create_tables(conn)?;
            seed_default_models(conn)?;
            conn.execute("PRAGMA user_version = 3", [])?;
            log::info!("DB schema initialized (v3), default models seeded");
        }
        1 | 2 => {
            // v1/v2 → v3：transcriptions.id 改应用写入的毫秒戳（去 AUTOINCREMENT）。
            // SQLite 不支持 ALTER 列约束，且旧数据无所谓 → DROP + 重建。
            let tx = conn.unchecked_transaction()?;
            tx.execute("DROP TABLE IF EXISTS transcriptions", [])?;
            tx.execute_batch(
                "CREATE TABLE transcriptions (
                    id            INTEGER PRIMARY KEY,
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
                CREATE INDEX IF NOT EXISTS idx_trans_engine  ON transcriptions(engine);",
            )?;
            // v1 的 models 可能还有 is_active 列 → 幂等补 DROP（v2 已无则跳过）
            let has_is_active: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('models') WHERE name='is_active'",
                [],
                |r| r.get(0),
            )?;
            if has_is_active > 0 {
                tx.execute("ALTER TABLE models DROP COLUMN is_active", [])?;
            }
            tx.commit()?;
            conn.execute("PRAGMA user_version = 3", [])?;
            log::info!("DB schema migrated v{} → v3 (transcriptions rebuilt, id=millis)", v);
        }
        _ => {}
    }
    Ok(())
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS transcriptions (
            id            INTEGER PRIMARY KEY,
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
}

/// 默认引擎集（替代 model.json）。
/// zipformer-small-ctc 走本地打包路径（开箱即用，是兜底引擎）；其余走 HF 缓存（按需下载）。
/// 注意：不再有 is_active 列——引擎激活由 config.yaml.asr_engine 决定（见 asr::config::resolve_active_engine）。
const DEFAULT_MODELS: &[DefaultModel] = &[
    DefaultModel {
        category: "zipformer",
        name: "zipformer-small-ctc",
        source: DEFAULT_ASR_MODEL_DIR,
        language: "zh",
        description: "zipformer-small-ctc, 27M (随应用打包)",
        quantization: "int8",
    },
    DefaultModel {
        category: "zipformer",
        name: "zipformer-multi",
        source: "k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13",
        language: "zh",
        description: "zipformer-multi, 80M",
        quantization: "int8",
    },
    DefaultModel {
        category: "zipformer",
        name: "zipformer-ctc",
        source: "csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30",
        language: "zh",
        description: "zipformer-ctc, 163M",
        quantization: "int8",
    },
    DefaultModel {
        category: "paraformer",
        name: "paraformer-streaming",
        source: "csukuangfj/sherpa-onnx-streaming-paraformer-zh",
        language: "zh",
        description: "paraformer-streaming, 230M",
        quantization: "int8",
    },
    DefaultModel {
        category: "sensevoice",
        name: "sherpa-onnx-sense-voice-funasr-nano-int8",
        source: "csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17",
        language: "auto",
        description: "SenseVoice FunASR Nano INT8, 265M",
        quantization: "int8",
    },
    DefaultModel {
        category: "qwen3-asr",
        name: "qwen3-asr-0.6B",
        source: "csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
        language: "auto",
        description: "qwen3-asr-0.6B, 1G",
        quantization: "int8",
    },
    DefaultModel {
        category: "whisper",
        name: "whisper-small",
        source: "onnx-community/whisper-small",
        language: "auto",
        description: "Whisper Small - 快速轻量, 250M",
        quantization: "int8",
    },
];

fn seed_default_models(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for m in DEFAULT_MODELS {
        tx.execute(
            "INSERT OR IGNORE INTO models
                (domain, category, name, source, language, description, quantization)
             VALUES ('asr', ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                m.category,
                m.name,
                m.source,
                m.language,
                m.description,
                m.quantization
            ],
        )?;
    }
    tx.commit()?;
    log::info!("Seeded {} default models", DEFAULT_MODELS.len());
    Ok(())
}

// ── DB → AsrConfig（load_config 用）──

/// 从 DB models 表构造 AsrConfig（domain='asr'）。
pub fn load_models() -> Result<AsrConfig> {
    with_db(|conn| load_models_at(conn))
}

fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    let mut stmt = conn.prepare(
        "SELECT category, name, source, language, description, quantization
         FROM models WHERE domain='asr'",
    )?;
    let rows: Vec<(String, String, String, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
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
    for (category, name, source, language, description, quantization) in rows {
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
        map.get_or_insert_with(HashMap::new).insert(name, entry);
    }
    Ok(AsrConfig { asr })
}

// ── 识别历史写入（desktop coordinator 用）──

// ── 过程入库接口（id = 应用写入毫秒戳，按识别生命周期递增更新）──
//
// 拆分模式：
//   - 私有 `_at(conn, ...)` 接 &Connection，可单测，含业务计算（char_count 等）
//   - pub 接口仅 `with_db` 转发到 `_at`，调用方无感
// 这样单测能直接调 `_at` 覆盖业务计算，而非用裸 SQL 复刻。

/// 过程入库：首次有 ASR 文本时插入（接 Connection，可单测）。
fn insert_at_id(
    conn: &Connection,
    id: i64,
    raw_text: &str,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    let created_at = now_string();
    let char_count = raw_text.chars().count() as i64;
    conn.execute(
        "INSERT INTO transcriptions
            (id, created_at, engine, engine_mode, raw_text, polished_text, polish_status, char_count)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'off', ?6)",
        params![id, created_at, engine, engine_mode, raw_text, char_count],
    )?;
    Ok(())
}

/// 过程入库：分段后更新 raw_text（接 Connection，可单测）。
/// 0 行（记录缺失）记 warn，仍返回 Ok 不阻塞（与 spec「DB 失败不阻塞」基调一致）。
fn update_raw_at(conn: &Connection, id: i64, raw_text: &str) -> Result<()> {
    let char_count = raw_text.chars().count() as i64;
    let n = conn.execute(
        "UPDATE transcriptions SET raw_text=?1, char_count=?2 WHERE id=?3",
        params![raw_text, char_count, id],
    )?;
    if n == 0 {
        log::warn!("update_raw_text id={} affected 0 rows (record missing?)", id);
    }
    Ok(())
}

/// 过程入库：停顿润色后更新 polished_text（接 Connection，可单测）。
/// 0 行记 warn，仍返回 Ok。
fn update_polished_at(
    conn: &Connection,
    id: i64,
    polished_text: &str,
    polish_status: &str,
    polish_model: Option<&str>,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE transcriptions SET polished_text=?1, polish_status=?2, polish_model=?3 WHERE id=?4",
        params![polished_text, polish_status, polish_model, id],
    )?;
    if n == 0 {
        log::warn!("update_polished id={} affected 0 rows (record missing?)", id);
    }
    Ok(())
}

/// 过程入库：识别结束 finalize，写最终 raw/polished/status/char_count/duration_ms（接 Connection，可单测）。
/// char_count = polished_text.unwrap_or(raw_text)（有润色取润色，否则取 raw）。
/// 0 行记 warn，仍返回 Ok。
fn finalize_at(
    conn: &Connection,
    id: i64,
    raw_text: &str,
    polished_text: Option<&str>,
    polish_status: &str,
    polish_model: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    let display = polished_text.unwrap_or(raw_text);
    let char_count = display.chars().count() as i64;
    let n = conn.execute(
        "UPDATE transcriptions SET raw_text=?1, polished_text=?2, polish_status=?3, polish_model=?4, char_count=?5, duration_ms=?6 WHERE id=?7",
        params![raw_text, polished_text, polish_status, polish_model, char_count, duration_ms, id],
    )?;
    if n == 0 {
        log::warn!("finalize_transcription id={} affected 0 rows (record missing?)", id);
    }
    Ok(())
}

/// 首次有 ASR 文本时插入（应用写入毫秒戳 id，走全局连接）。
///
/// 命名约定：
/// - `insert_transcription_at_id`（pub，本函数，id 由应用写入毫秒戳，走全局连接）
/// - `insert_at_id(conn, ...)`（私有，本接口的 Connection 内部实现，供单测）
///
/// 用于过程增量入库（coordinator 在识别过程中首次有文本时 INSERT）。
pub fn insert_transcription_at_id(
    id: i64,
    raw_text: &str,
    engine: &str,
    engine_mode: Option<&str>,
) -> Result<()> {
    with_db(|conn| insert_at_id(conn, id, raw_text, engine, engine_mode))
}

/// 分段后更新 raw_text（完整 ASR = raw + increase，走全局连接）。
pub fn update_raw_text(id: i64, raw_text: &str) -> Result<()> {
    with_db(|conn| update_raw_at(conn, id, raw_text))
}

/// 停顿润色后更新 polished_text（走全局连接）。
pub fn update_polished(
    id: i64,
    polished_text: &str,
    polish_status: &str,
    polish_model: Option<&str>,
) -> Result<()> {
    with_db(|conn| update_polished_at(conn, id, polished_text, polish_status, polish_model))
}

/// 识别结束 finalize：写最终 raw/polished/status/char_count/duration_ms（走全局连接）。
pub fn finalize_transcription(
    id: i64,
    raw_text: &str,
    polished_text: Option<&str>,
    polish_status: &str,
    polish_model: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    with_db(|conn| {
        finalize_at(
            conn,
            id,
            raw_text,
            polished_text,
            polish_status,
            polish_model,
            duration_ms,
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
        let zf = cfg.asr.zipformer.as_ref().expect("zipformer section");
        assert_eq!(zf.len(), 3);
        let small = zf.get("zipformer-small-ctc").unwrap();
        assert_eq!(small.source, DEFAULT_ASR_MODEL_DIR); // 本地路径
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
    fn load_models_empty_db_returns_empty_sections() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        let cfg = load_models_at(&conn).unwrap();
        assert!(cfg.asr.whisper.is_none());
    }

    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }

    #[test]
    fn v2_to_v3_migration_rebuilds_transcriptions() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟 v1 旧 schema（id AUTOINCREMENT + models 有 is_active）
        conn.execute_batch(
            "CREATE TABLE transcriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, created_at TEXT NOT NULL,
                engine TEXT NOT NULL, engine_mode TEXT, raw_text TEXT NOT NULL,
                polished_text TEXT, polish_status TEXT NOT NULL DEFAULT 'off',
                polish_model TEXT, duration_ms INTEGER, char_count INTEGER
            );
            CREATE TABLE models (
                id INTEGER PRIMARY KEY AUTOINCREMENT, domain TEXT NOT NULL,
                category TEXT NOT NULL, name TEXT NOT NULL, source TEXT NOT NULL,
                language TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '',
                quantization TEXT NOT NULL DEFAULT '', is_active INTEGER NOT NULL DEFAULT 0,
                UNIQUE(domain, category, name)
            );
            INSERT INTO transcriptions (created_at, engine, raw_text) VALUES ('2020-01-01 00:00:00','x','旧数据');
            PRAGMA user_version = 1;",
        ).unwrap();

        // 跑 migration
        init_schema(&conn).unwrap();

        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
        // 旧数据被 DROP
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM transcriptions", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        // models.is_active 已删
        let has_is_active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('models') WHERE name='is_active'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(has_is_active, 0);
        // 能插入显式大 id（毫秒戳）
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text) VALUES (1718000000000,'2026-06-14 00:00:00','sensevoice','新数据')",
            [],).unwrap();
        let id: i64 = conn.query_row("SELECT id FROM transcriptions WHERE raw_text='新数据'", [], |r| r.get(0)).unwrap();
        assert_eq!(id, 1718000000000);
    }

    #[test]
    fn v2_already_no_is_active_migrates_cleanly() {
        // v2 现状：models 已无 is_active。验证 migration 幂等、不报错。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE transcriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, created_at TEXT NOT NULL,
                engine TEXT NOT NULL, engine_mode TEXT, raw_text TEXT NOT NULL,
                polished_text TEXT, polish_status TEXT NOT NULL DEFAULT 'off',
                polish_model TEXT, duration_ms INTEGER, char_count INTEGER
            );
            CREATE TABLE models (
                id INTEGER PRIMARY KEY AUTOINCREMENT, domain TEXT NOT NULL,
                category TEXT NOT NULL, name TEXT NOT NULL, source TEXT NOT NULL,
                language TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '',
                quantization TEXT NOT NULL DEFAULT '', UNIQUE(domain, category, name)
            );
            PRAGMA user_version = 2;",
        ).unwrap();
        init_schema(&conn).unwrap();
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn update_and_finalize_round_trip() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        // 真调 4 个内部 _at 接口（覆盖 char_count / unwrap_or 业务计算）
        // insert_at_id：char_count = raw_text("首段").chars().count() = 2
        insert_at_id(&conn, 100, "首段", "sensevoice", Some("streaming")).unwrap();
        // 验证 insert_at_id 的 char_count = raw 长度（I1 覆盖 INSERT char_count）
        let cc_after_insert: i64 = conn
            .query_row("SELECT char_count FROM transcriptions WHERE id=100", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cc_after_insert, 2); // "首段" = 2 字

        // update_raw_text：char_count 重算为 raw 长度（"首段二段" = 4）
        update_raw_at(&conn, 100, "首段二段").unwrap();
        let cc_after_raw: i64 = conn
            .query_row("SELECT char_count FROM transcriptions WHERE id=100", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cc_after_raw, 4); // "首段二段" = 4 字

        // update_polished：写 polished/status/model（不改 char_count）
        update_polished_at(&conn, 100, "润色", "done", Some("deepseek")).unwrap();

        // finalize：char_count = polished.unwrap_or(raw) = "润色"（2 字，非 raw 的 4 字）
        // —— 这条验证 unwrap_or 走 polished 分支（I1 关键覆盖点）
        finalize_at(&conn, 100, "首段二段", Some("润色"), "done", Some("deepseek"), Some(5000))
            .unwrap();

        let (raw, polished, status, dur, cc): (String, Option<String>, String, Option<i64>, i64) =
            conn.query_row(
                "SELECT raw_text, polished_text, polish_status, duration_ms, char_count
                 FROM transcriptions WHERE id=100",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(raw, "首段二段");
        assert_eq!(polished, Some("润色".into()));
        assert_eq!(status, "done");
        assert_eq!(dur, Some(5000));
        assert_eq!(cc, 2); // finalize char_count = polished("润色") = 2，验证 unwrap_or 走 polished
    }
}
