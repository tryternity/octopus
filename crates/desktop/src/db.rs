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

// 占位 stub：真实迁移逻辑在后续 Task 3（history.txt）/ Task 4（model.json）填充。
// 此处返回 Ok(()) 以保证 init_schema 结构完整、Task 2 可独立编译通过。
fn migrate_history(_conn: &Connection) -> Result<()> {
    Ok(())
}
fn migrate_model_json(_conn: &Connection) -> Result<()> {
    Ok(())
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
}
