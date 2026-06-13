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

/// 迁移 history.txt（默认路径）。文件不存在/为空则跳过。
fn migrate_history(conn: &Connection) -> Result<()> {
    let path = octopus_asr::config::handy_home().join("history.txt");
    migrate_history_at(conn, &path)
}

/// 迁移指定路径的 history.txt（可单测注入路径）。
fn migrate_history_at(conn: &Connection, path: &std::path::Path) -> Result<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return Ok(()),
    };
    let entries = parse_history_entries(&content);
    let count = entries.len();
    for e in entries {
        conn.execute(
            "INSERT INTO transcriptions
                (created_at, engine, engine_mode, raw_text, polished_text, polish_status, char_count)
             VALUES (?1, '', NULL, ?2, ?2, 'done', ?3)",
            params![e.timestamp, e.body, e.body.chars().count() as i64],
        )?;
    }
    if count > 0 {
        log::info!("Migrated {} entries from history.txt", count);
    }
    Ok(())
}

// 占位 stub：真实迁移逻辑在 Task 4（model.json）填充。
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
}
