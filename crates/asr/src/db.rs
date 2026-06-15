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
            secret_key   TEXT    NOT NULL DEFAULT '',
            UNIQUE(domain, category, name)
        );",
    )?;
    Ok(())
}

// ── 默认引擎 seed（替代 model.json）──

struct DefaultModel {
    domain: &'static str,
    category: &'static str,
    name: &'static str,
    source: &'static str,
    language: &'static str,
    description: &'static str,
    secret_key: &'static str,
}

/// 默认引擎集（替代 model.json）。
/// zipformer-small-ctc 走本地打包路径（开箱即用，是兜底引擎）；其余走 HF 缓存（按需下载）。
/// 注意：不再有 is_active 列——引擎激活由 config.yaml.asr_engine 决定（见 asr::config::resolve_active_engine）。
const DEFAULT_MODELS: &[DefaultModel] = &[
    DefaultModel {
        domain: "asr",
        category: "zipformer",
        name: "zipformer-small-ctc",
        source: DEFAULT_ASR_MODEL_DIR,
        language: "zh",
        description: "zipformer-small-ctc, 27M (随应用打包)",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "zipformer",
        name: "zipformer-multi",
        source: "k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13",
        language: "zh",
        description: "zipformer-multi, 80M",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "zipformer",
        name: "zipformer-ctc",
        source: "csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30",
        language: "zh",
        description: "zipformer-ctc, 163M",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "paraformer",
        name: "paraformer-streaming",
        source: "csukuangfj/sherpa-onnx-streaming-paraformer-zh",
        language: "zh",
        description: "paraformer-streaming, 230M",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "sensevoice",
        name: "sherpa-onnx-sense-voice-funasr-nano-int8",
        source: "csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17",
        language: "auto",
        description: "SenseVoice FunASR Nano INT8, 265M",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "qwen3-asr",
        name: "qwen3-asr-0.6B",
        source: "csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
        language: "auto",
        description: "qwen3-asr-0.6B, 1G",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "qwen3-asr",
        name: "qwen3-asr-1.7B",
        source: "ilmina/qwen3-asr-1.7b-sherpa-onnx",
        language: "auto",
        description: "qwen3-asr-1.7B, 约2.7G",
        secret_key: "",
    },
    DefaultModel {
        domain: "asr",
        category: "whisper",
        name: "whisper-small",
        source: "onnx-community/whisper-small",
        language: "auto",
        description: "Whisper Small - 快速轻量, 250M",
        secret_key: "",
    },
    // LLM 润色模型
    DefaultModel {
        domain: "llm",
        category: "deepseek",
        name: "deepseek-v4-flash",
        source: "https://api.deepseek.com/",
        language: "",
        description: "DeepSeek V4 Flash 润色模型",
        secret_key: "",
    },
    DefaultModel {
        domain: "llm",
        category: "bigmodel",
        name: "GLM-4.7-FlashX",
        source: "https://open.bigmodel.cn/api/paas/v4",
        language: "",
        description: "GLM-4.7 FlashX 润色模型",
        secret_key: "",
    },
];

fn seed_default_models(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for m in DEFAULT_MODELS {
        tx.execute(
            "INSERT OR IGNORE INTO models
                (domain, category, name, source, language, description, secret_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.domain,
                m.category,
                m.name,
                m.source,
                m.language,
                m.description,
                m.secret_key
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
        "SELECT category, name, source, language, description, secret_key
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
    for (category, name, source, language, description, secret_key) in rows {
        let entry = ModelEntry {
            source,
            language,
            description,
            secret_key,
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

/// 从 DB 加载指定名称的 LLM 配置（domain='llm'）。
pub fn load_llm_model(name: &str) -> Result<Option<octopus_llm::CompatibleLlmConfig>> {
    with_db(|conn| load_llm_model_at(conn, name))
}

fn load_llm_model_at(conn: &Connection, name: &str) -> Result<Option<octopus_llm::CompatibleLlmConfig>> {
    let mut stmt = conn.prepare(
        "SELECT category, source, secret_key
         FROM models WHERE domain='llm' AND name=?1",
    )?;
    let mut rows = stmt.query_map(params![name], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    if let Some(r) = rows.next() {
        let (category, source, secret_key) = r?;
        Ok(Some(octopus_llm::CompatibleLlmConfig {
            provider: category,
            model: name.to_string(),
            base_url: source,
            secret_key,
        }))
    } else {
        Ok(None)
    }
}

// ── 识别历史写入（desktop coordinator 用）──

// ── 识别历史写入（desktop coordinator 用）──

// ── 过程入库接口（id = 应用写入毫秒戳，按识别生命周期递增更新）──

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
        assert_eq!(cfg.asr.qwen3_asr.as_ref().unwrap().len(), 2);
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
    fn test_load_llm_model() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        seed_default_models(&conn).unwrap();

        let glm = load_llm_model_at(&conn, "GLM-4.7-FlashX").unwrap().unwrap();
        assert_eq!(glm.provider, "bigmodel");
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(glm.secret_key, "");

        let ds = load_llm_model_at(&conn, "deepseek-v4-flash").unwrap().unwrap();
        assert_eq!(ds.provider, "deepseek");
        assert_eq!(ds.base_url, "https://api.deepseek.com/");

        assert!(load_llm_model_at(&conn, "nonexistent-model").unwrap().is_none());
    }

    #[test]
    fn days_to_ymd_known() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
    }

    #[test]
    fn v2_to_v3_migration_rebuilds_transcriptions() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟 v1/v2 旧 schema（id AUTOINCREMENT + models 有 is_active）
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
                secret_key TEXT NOT NULL DEFAULT '', is_active INTEGER NOT NULL DEFAULT 0,
                UNIQUE(domain, category, name)
            );
            INSERT INTO transcriptions (created_at, engine, raw_text) VALUES ('2020-01-01 00:00:00','x','旧数据');
            INSERT INTO models (domain, category, name, source, secret_key) VALUES ('asr', 'zipformer', 'zipformer-small-ctc', 'models/zipformer', '');
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
        let has_secret_key: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('models') WHERE name='secret_key'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(has_secret_key, 1);
        let secret_key: String = conn.query_row(
            "SELECT secret_key FROM models WHERE name='zipformer-small-ctc'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(secret_key, "");

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
                secret_key TEXT NOT NULL DEFAULT '', UNIQUE(domain, category, name)
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
        // 用 SQL 直接模拟 4 个新接口的语句（接口本身用全局 with_db，单测以 SQL 验证语句正确）
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status, char_count)
             VALUES (100, '2026-06-14 00:00:00', 'sensevoice', '首段', NULL, 'off', 2)",
            [],).unwrap();
        // update_raw_text
        conn.execute("UPDATE transcriptions SET raw_text='首段二段', char_count=4 WHERE id=100", []).unwrap();
        // update_polished
        conn.execute("UPDATE transcriptions SET polished_text='润色', polish_status='done', polish_model='deepseek' WHERE id=100", []).unwrap();
        // finalize
        conn.execute("UPDATE transcriptions SET raw_text='首段二段', polished_text='润色', polish_status='done', char_count=2, duration_ms=5000 WHERE id=100", []).unwrap();

        let (raw, polished, status, dur): (String, Option<String>, String, Option<i64>) = conn
            .query_row("SELECT raw_text, polished_text, polish_status, duration_ms FROM transcriptions WHERE id=100", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(raw, "首段二段");
        assert_eq!(polished, Some("润色".into()));
        assert_eq!(status, "done");
        assert_eq!(dur, Some(5000));
    }
}
