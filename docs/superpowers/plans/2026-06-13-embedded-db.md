# 嵌入式 DB 存储 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 引入 rusqlite（bundled），将识别历史（原生 + AI 修正双份）与模型配置迁入 SQLite，废弃 record.txt / model.json；内存新增 `raw_text` 保证原生文本不被 polish 覆盖。

**Architecture:** 新增 `crates/desktop/src/db.rs` 封装全局 `Connection`（`OnceLock<Mutex<Connection>>`），提供 `init`（建表 + 一次性迁移）/ `insert_transcription` / `active_engine`。coordinator 的 `Stage::Streaming` / `Stage::VadSegmented` 新增 `raw_text` 字段，在识别新增时镜像全量、polish 时不触碰；最终润色后调 `db::insert_transcription`。`result_window.rs` 删除所有文件写入，`result-edited` 改发 `Command::ResultEdited`。

**Tech Stack:** rusqlite 0.31（`bundled` feature）、serde_json、`std::sync::{OnceLock, Mutex}`、tempfile（测试）

**关键不变量：** `raw_text` 始终是完整的、未经任何 LLM 润色的识别全文（含 ASR + VAD 标点）；`accumulated_text` 是展示版（可能被 polish 替换前缀）。

---

## File Structure

| 文件 | 责任 | 本次 |
|------|------|------|
| `crates/desktop/src/db.rs` | DB 访问层：连接、建表、迁移、insert、查询 | 新建 |
| `crates/desktop/Cargo.toml` | 依赖 | 加 rusqlite + tempfile(dev) |
| `crates/desktop/src/main.rs` | crate root + 启动 | 加 `mod db;` + 启动 `db::init()` |
| `crates/desktop/src/coordinator.rs` | 状态机 | Stage 加 `raw_text`、tick 同步、INSERT、`Command::ResultEdited` |
| `crates/desktop/src/result_window.rs` | 结果窗口 | 删除文件写入、`result-edited` 改发 Command |

`raw_text` 同步规则（贯穿 Task 7）：凡是「识别新增文本」的分支，都执行 `*raw_text = new_text.clone()`（与 `*accumulated_text = new_text` 并列）；`handle_polish_done` 只改 `accumulated_text`，**不碰 `raw_text`**。`StreamingSession::accept_samples` / `flush` 返回的是 ASR 全量（未经 polish），直接镜像即可。

---

## Task 1: 加依赖

**Files:**
- Modify: `crates/desktop/Cargo.toml`

- [x] **Step 1: 加 rusqlite 与 tempfile(dev)**

在 `crates/desktop/Cargo.toml` 的 `[dependencies]` 末尾（`octopus-llm` 之后）加：

```toml
# Storage
rusqlite = { version = "0.31", features = ["bundled"] }
```

在文件末尾加 dev-dependencies：

```toml
[dev-dependencies]
tempfile = "3"
```

- [x] **Step 2: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（首次会编译 bundled SQLite，耗时较长）；无 error。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/Cargo.toml
git commit -m "deps: add rusqlite (bundled) + tempfile for embedded storage"
```

---

## Task 2: db.rs 骨架（路径 / 连接 / 建表 / schema version）

**Files:**
- Create: `crates/desktop/src/db.rs`
- Modify: `crates/desktop/src/main.rs`（加 `mod db;`）

- [x] **Step 1: 写 db.rs 骨架**

创建 `crates/desktop/src/db.rs`：

```rust
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

// migrate_history / migrate_model_json / insert_transcription / active_engine
// 在后续 Task 中追加。

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
        init_schema(&conn).unwrap(); // 迁移读 ~/.octopus 文件，测试环境无则跳过
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
    }
}
```

- [x] **Step 2: main.rs 声明 db 模块**

在 `crates/desktop/src/main.rs` 的 `mod` 声明区（`mod audio;` 一带）加一行：

```rust
mod db;
```

- [x] **Step 3: 跑测试**

Run: `cargo test -p octopus-desktop --features embedded db::`
Expected: 两个测试通过（`create_tables_is_idempotent`、`init_schema_sets_user_version_1_on_fresh_db`）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/db.rs crates/desktop/src/main.rs
git commit -m "feat(db): add db.rs skeleton — connection, schema, user_version"
```

---

## Task 3: 迁移 history.txt

**Files:**
- Modify: `crates/desktop/src/db.rs`

- [x] **Step 1: 写测试**

在 `db.rs` 的 `mod tests` 内追加：

```rust
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
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop --features embedded db::parse_history db::migrate_history`
Expected: 编译失败（`parse_history_entries` / `migrate_history_at` 未定义）。

- [x] **Step 3: 写实现**

在 `db.rs`（`create_tables` 之后、`now_string` 之前）追加：

```rust
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
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop --features embedded db::`
Expected: 全部通过。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/db.rs
git commit -m "feat(db): migrate history.txt → transcriptions"
```

---

## Task 4: 迁移 model.json

**Files:**
- Modify: `crates/desktop/src/db.rs`

- [x] **Step 1: 写测试**

在 `mod tests` 内追加：

```rust
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
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop --features embedded db::migrate_model_json`
Expected: 编译失败（`migrate_model_json_at` 未定义）。

- [x] **Step 3: 写实现**

在 `db.rs` 追加（用 `serde_json::Value` 解析，feature 无关、不依赖 octopus-asr 的结构体）：

```rust
/// 迁移 model.json（默认路径）。文件不存在则跳过。
fn migrate_model_json(conn: &Connection) -> Result<()> {
    let path = octopus_asr::config::handy_home().join("model.json");
    migrate_model_json_at(conn, &path)
}

/// 迁移指定路径的 model.json（可单测注入路径）。
fn migrate_model_json_at(conn: &Connection, path: &std::path::Path) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let v: serde_json::Value = serde_json::from_str(&text).context("parse model.json")?;

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
                        insert_model(conn, "asr", category, name, entry, name == active)?;
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
                insert_model(conn, "vad", "silero", name, entry, name == active)?;
            }
        }
    }

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
    let description = entry.get("description").and_then(|s| s.as_str()).unwrap_or("");
    let quantization = entry.get("quantization").and_then(|s| s.as_str()).unwrap_or("");
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
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop --features embedded db::`
Expected: 全部通过。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/db.rs
git commit -m "feat(db): migrate model.json → models table"
```

---

## Task 5: insert_transcription + active_engine 查询

**Files:**
- Modify: `crates/desktop/src/db.rs`

- [x] **Step 1: 写测试**

在 `mod tests` 内追加：

```rust
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
             VALUES ('asr','paraformer','paraformer-streaming','src','zh','',  'int8', 1)",
            [],
        )
        .unwrap();
        let m = active_engine_at(&conn, "asr").unwrap().unwrap();
        assert_eq!(m.name, "paraformer-streaming");
        assert_eq!(m.source, "src");
    }
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop --features embedded db::insert_transcription db::active_engine`
Expected: 编译失败。

- [x] **Step 3: 写实现**

在 `db.rs` 追加：

```rust
/// 当前激活的模型（某 domain 下 is_active=1 的行）。
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
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop --features embedded db::`
Expected: 全部 7 个测试通过。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/db.rs
git commit -m "feat(db): insert_transcription + active_engine query"
```

---

## Task 6: main.rs 启动初始化 DB

**Files:**
- Modify: `crates/desktop/src/main.rs`

- [x] **Step 1: 在 setup 中调 db::init()**

找到 `main.rs` 中注册插件、`app.manage`、或 `setup` 钩子的位置（Builder 链里的 `.setup(|app| { ... })` 或 `main` 早期）。在应用启动、coordinator 创建之前插入：

```rust
    // 初始化嵌入式 DB（建表 + 首次迁移 history.txt / model.json）
    if let Err(e) = crate::db::init() {
        log::error!("DB init failed: {}, storage disabled", e);
    }
```

放在 `Coordinator::new(...)` / `app.manage(...)` **之前**（DB 必须先就绪）。

- [x] **Step 2: 验证启动生成 DB**

Run: `cargo run -p octopus-desktop --features embedded`（运行后从托盘退出）
Expected:
- 启动无 DB 相关 panic；
- `~/.octopus/octopus.db` 文件生成；
- 日志含 `DB schema initialized (v1)` 与迁移条数。

- [x] **Step 3: 用 sqlite3 客户端验证迁移结果**

Run: `sqlite3 ~/.octopus/octopus.db "SELECT count(*) FROM transcriptions; SELECT domain,name,is_active FROM models WHERE is_active=1;"`
Expected: transcriptions 行数 = 现 history.txt 条数；models 至少一行 asr active（paraformer-streaming）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "feat(db): init DB on startup (schema + migration)"
```

---

## Task 7: coordinator 内存 raw_text

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: Stage 加 raw_text 字段**

`Stage::Streaming`（约 line 40-57）在 `accumulated_text` 下方加：

```rust
        /// 原生识别全文（未经 polish，入库用）
        raw_text: String,
```

`Stage::VadSegmented`（约 line 59-88）同样在 `accumulated_text` 下方加：

```rust
        /// 原生识别全文（未经 polish，入库用）
        raw_text: String,
```

- [x] **Step 2: 初始化 raw_text**

`Stage::Streaming` 构造（约 line 274-284），在 `accumulated_text: String::new(),` 下方加：

```rust
                            raw_text: String::new(),
```

`Stage::VadSegmented` 构造（约 line 306-321），在 `accumulated_text: String::new(),` 下方加：

```rust
                                raw_text: String::new(),
```

- [x] **Step 3: tick 中同步 raw_text**

在所有「识别新增文本并赋值 accumulated_text」的分支，并列加 `*raw_text = new_text.clone();`。

`handle_streaming_tick`（约 line 860-886）的 `accept_samples` 与 `flush` 两个 `Ok(Some(new_text))` 分支，把：

```rust
                *accumulated_text = new_text;
```

改为：

```rust
                *accumulated_text = new_text.clone();
                *raw_text = new_text;
```

（accept_samples / flush 返回的是 ASR 全量，未经 polish，直接镜像给 raw_text。）

同样在 `handle_vad_segmented_tick` / `handle_transcription_done` 里 VadSegmented 的文本追加分支：凡执行 `*accumulated_text = ...`（或 `accumulated_text.push_str(...)`）的位置，对 `raw_text` 做相同操作。

> 用 grep 定位所有改动点：`grep -n "accumulated_text" crates/desktop/src/coordinator.rs`。凡是 tick / transcription-done 里的赋值或追加都同步 raw_text；**`handle_polish_done` 里的赋值不动 raw_text**。

- [x] **Step 4: 更新所有 Stage 解构**

凡是 `Stage::Streaming { ... }` / `Stage::VadSegmented { ... }` 的解构（`handle_toggle` 停止分支约 line 336、各 handler），按编译器提示补 `raw_text` 字段。`handle_polish_done` 的解构也需取出 `raw_text`（但不修改它）。

- [x] **Step 5: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过；无 error。如有 `unused variable: raw_text`，属预期（下个 Task 才使用它入库）。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(coordinator): maintain raw_text (unpolished) alongside accumulated_text"
```

---

## Task 8: 最终润色后 INSERT + Command::ResultEdited

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: Command 加 ResultEdited**

`enum Command`（约 line 16-34）末尾加：

```rust
    /// 用户在结果窗口编辑了文本
    ResultEdited { text: String },
```

- [x] **Step 2: start_pasting 扩展签名并 INSERT**

把 `fn start_pasting`（约 line 476）签名从：

```rust
fn start_pasting(
    stage: &mut Stage,
    text: &str,
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
)
```

改为：

```rust
fn start_pasting(
    stage: &mut Stage,
    text: &str,
    raw_text: &str,
    engine: &str,
    engine_mode: &str,
    config: &DesktopConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
)
```

在 `let final_text = ...;`（最终润色结果，约 line 491-508）之后、`crate::result_window::show_result(...)`（约 line 510）之前插入入库逻辑：

```rust
    // 入库：原生全文 + 修正版（仅润色成功时）+ 状态
    let (polished_for_db, polish_status) = if config.llm_config().is_some() {
        // 启用了 polish：final_text 与原 text 不同视为成功润色
        if final_text != text {
            (Some(final_text.as_str()), "done")
        } else {
            (None, "failed") // 润色未生效（空或失败 → 回退原文本）
        }
    } else {
        (None, "off")
    };
    let polish_model = if polish_status == "done" {
        Some(config.llm_model.as_str())
    } else {
        None
    };
    if let Err(e) = crate::db::insert_transcription(
        raw_text,
        polished_for_db,
        polish_status,
        polish_model,
        engine,
        Some(engine_mode),
    ) {
        log::warn!("DB insert transcription failed: {}", e);
    }
```

> `config.llm_model` 字段名以实际 `DesktopConfig` 为准（见 `crates/desktop/src/config.rs`，润色模型字段）。若字段名不同，替换为实际名。

- [x] **Step 3: 更新所有 start_pasting 调用点**

用 `grep -n "start_pasting(" crates/desktop/src/coordinator.rs` 定位调用点（`handle_toggle` 停止分支、`handle_transcription_done` WaitingCompletion 完成分支）。每处从对应 `Stage` 取出 `raw_text`，并传入 `engine` / `engine_mode`：

```rust
// Streaming 分支示例
Stage::Streaming { accumulated_text, raw_text, .. } => {
    start_pasting(
        stage,
        accumulated_text,
        raw_text,
        &config.engine_name,          // 实际引擎名字段
        "streaming",
        config,
        app_handle,
        tx,
    );
}
// VadSegmented 分支：engine_mode 传 "vad_segmented"
```

> `engine` 用 `DesktopConfig` 里实际引擎名字段（如 `config.engine_name` / `config.asr_engine`，以 config.rs 为准）。`engine_mode`：Streaming 分支 `"streaming"`，VadSegmented 分支 `"vad_segmented"`。

- [x] **Step 4: 加 handle_result_edited**

新增 handler：

```rust
/// 处理结果窗口的编辑事件：更新内存展示文本（不影响 raw_text）。
fn handle_result_edited(stage: &mut Stage, text: String) {
    match stage {
        Stage::Streaming { accumulated_text, .. } | Stage::VadSegmented { accumulated_text, .. } => {
            *accumulated_text = text;
        }
        _ => {}
    }
}
```

在 coordinator 的命令 loop（`match cmd { ... }`）加分支：

```rust
                Command::ResultEdited { text } => {
                    handle_result_edited(&mut stage, text);
                }
```

- [x] **Step 5: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(coordinator): insert transcription on paste; handle ResultEdited"
```

---

## Task 9: result_window 改造（删文件写入，result-edited 改发 Command）

**Files:**
- Modify: `crates/desktop/src/result_window.rs`
- Modify: `crates/desktop/src/main.rs`（若 create_result_window 需透传 app 句柄/state）

- [x] **Step 1: 删除文件写入相关函数**

从 `result_window.rs` 删除以下函数（record.txt / history.txt 全部废弃）：

- `save_record`
- `clear_record_file`
- `archive_to_history`
- `parse_history_entries`
- `record_file_path`
- `history_file_path`
- `chrono_now_string` / `days_to_ymd` / `is_leap`（已移至 db.rs）

删除后清理未使用的 `use`（如 `PathBuf` 若不再用）。

- [x] **Step 2: clear_result 不再归档**

`clear_result`（约 line 242）把：

```rust
pub fn clear_result(app: &tauri::AppHandle) {
    // 先归档到 history
    archive_to_history();
    ...
}
```

改为：

```rust
pub fn clear_result(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("clear-result", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = window_clone.hide();
        });
    }
}
```

- [x] **Step 3: result-edited 改发 Command**

`create_result_window` 里 `result-edited` 的 listen 闭包（约 line 212-217），从：

```rust
            let _ = window.listen("result-edited", move |event| {
                let text = event.payload();
                if !text.is_empty() {
                    save_record(text);
                }
            });
```

改为通过 app state 取 Coordinator 并发命令。先确认 `Coordinator` 已被 `app.manage(...)`（main.rs），且 Coordinator 暴露了发命令的入口。若 Coordinator 已有 `pub fn send(&self, cmd: Command)` 则直接用；否则加一个公开方法（`Command` 需 `pub`，或封装为 `pub fn report_result_edit(&self, text: String)`）。

最小改动：在 `Coordinator` 加：

```rust
impl Coordinator {
    /// 结果窗口编辑回写
    pub fn report_result_edit(&self, text: String) {
        let _ = self.tx.lock().unwrap().send(Command::ResultEdited { text });
    }
}
```

listen 闭包改为：

```rust
            let app_handle = app.clone();
            let _ = window.listen("result-edited", move |event| {
                let text = event.payload().to_string();
                if !text.is_empty() {
                    if let Some(coordinator) = app_handle.try_state::<Coordinator>() {
                        coordinator.report_result_edit(text);
                    }
                }
            });
```

> `try_state::<Coordinator>()` 返回 `Option<State<'_, Coordinator>>`，需 `use tauri::Manager;`（result_window.rs 已有）。`Coordinator` 需 `pub` 且实现 `Send + Sync`（已是：`Mutex<Sender>`，Command 含 String/Arc，Send OK）。

- [x] **Step 4: 删除 coordinator 里所有 save_record 调用**

Run: `grep -n "result_window::save_record" crates/desktop/src/coordinator.rs`
把每处 `crate::result_window::save_record(&x);` 整行删除（record.txt 已废弃，展示文本在内存，最终入库由 start_pasting 负责）。

- [x] **Step 5: 验证编译 + 运行**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过，无 `save_record` 未定义引用。

Run: `cargo run -p octopus-desktop --features embedded`
手动验证：
1. 录一段（启用 polish）→ 停止粘贴 → `sqlite3 ~/.octopus/octopus.db "SELECT raw_text, polished_text, polish_status FROM transcriptions ORDER BY id DESC LIMIT 1;"` → raw 与 polished 均有值、status=done。
2. 在结果窗口手改文本 → 停止 → 入库 polished_text 为编辑后版本、raw_text 仍为原生。
3. 关闭 polish 录一段 → status=off、polished_text 为 NULL。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/result_window.rs crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "refactor(result_window): drop record.txt/history.txt; result-edited → Command"
```

---

## Task A: model.json 运行时接入 DB

> 修复「Task 4 迁移入 DB 后，运行时模型查找仍读 model.json」的问题。提交 `efc6ef4`。

**问题**：Task 1-9 完成后，DB 已接管模型配置存储，但 `crates/asr/src/config.rs` 的 `load_config()` 仍读 `~/.octopus/model.json`——DB 与文件双份不同步，手编 DB 不生效。

**Files:**
- Modify: `crates/asr/src/config.rs`
- Modify: `crates/desktop/src/db.rs`
- Modify: `crates/desktop/src/main.rs`

- [x] **Step 1: asr config 加运行时注入**
  - `crates/asr/src/config.rs`：加 `static RUNTIME_CONFIG: OnceLock<AppConfig>` + `pub fn set_runtime_config(cfg)`；`load_config()` 优先返回注入版（`cfg.clone()`），未注入回退读 model.json。给 `AppConfig` / `VadSection` / `AsrSection` / `SimpleModelEntry` 加 `Clone` derive。

- [x] **Step 2: db.rs 加 load_app_config**
  - `crates/desktop/src/db.rs`：加 `pub fn load_app_config() -> Option<AppConfig>`（经 `load_app_config_at` 从 `models` 表构造）。关键映射：DB `category` 列存 JSON key（`"qwen3-asr"` 带 dash）→ AsrSection 字段 `qwen3_asr`（下划线）；按 dash 形式分派。空库返回 `None`。

- [x] **Step 3: main.rs 启动期注入**
  - `crates/desktop/src/main.rs`：`db::init()` 后调 `db::load_app_config()`，`Some(cfg)` → `set_runtime_config(cfg)`；`None` → `log::warn!` 回退读 model.json。

- [x] **Step 4: Commit** `efc6ef4` — "fix(db): inject runtime config from DB on desktop startup"

> 结果：desktop 运行时 `resolve_engine_category` / `find_silero_vad` / `list_engines` 等从 DB 读；cli/server 不注入，仍读 model.json。

---

## Task B: 入库时机推迟到 PasteDone + polish_status 语义修正

> 修复「原 Task 8 在 `start_pasting` 入库 + 用文本比较判 polish_status」的问题。提交 `327e1de`。

**问题**：
1. Task 8 在 `start_pasting`（`show_result` 前）入库，用户随后在结果窗口的编辑不会反映到入库的 `polished_text`。
2. Task 8 用 `final_text != text` 文本比较判 `polish_status`：润色返回与原文相同（正常情况）会被误判为 `failed`。

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: Stage::Pasting 改为结构变体**
  - 从单元变体 `Pasting` 改为 `Pasting { raw_text, polished_text, polish_status, engine, engine_mode }`，持入库所需全部数据。

- [x] **Step 2: polish_status 基于润色调用结果**
  - `start_pasting` 内 `let (final_text, polish_status) = match config.llm_config() { ... }`：`None` → `(text, "off")`；`Some` 且 `Ok(非空)` → `(润色结果, "done")`；`Some` 且 `Ok(空)` 或 `Err` → `(text, "failed")`。不再用文本比较。

- [x] **Step 3: INSERT 推迟到 PasteDone**
  - `start_pasting` 不再调 `insert_transcription`，仅构造 `Stage::Pasting`。
  - `Command::PasteDone` 分支从 `Stage::Pasting` 解构数据，调 `db::insert_transcription`；`polished_text` 仅 `done` 时传 `Some`，否则 `None`。

- [x] **Step 4: handle_result_edited 加 Pasting 分支**
  - `Stage::Pasting { polished_text, .. }` → `*polished_text = text`（更新 `polished_text`，不动 `raw_text`）。用户编辑反映到入库。

- [x] **Step 5: Commit** `327e1de` — "fix(coordinator): defer INSERT to PasteDone; polish_status by call result"

> 粘贴交互（`paste.rs`）仍用润色结果 `final_text`（编辑前），不变。

---

## Self-Review

**Spec coverage**（对照 `2026-06-13-embedded-db-design.md`）：
- §1.1 rusqlite bundled → Task 1 ✓
- §1.1 运行时模型查找接入 DB（修复 A）→ Task A ✓
- §3.1 transcriptions 表 → Task 2 ✓
- §3.2 models 表 → Task 2 ✓
- §3.3 schema user_version → Task 2（init_schema）✓
- §4 DB 文件位置 / 单连接 Mutex → Task 2（db_path / OnceLock<Mutex>）✓
- §5.1 内存 raw_text → Task 7 ✓
- §5.2 INSERT 时机（PasteDone 推迟）+ polish_status 基于润色调用结果（off/done/failed）→ Task 8（初版）+ Task B（修正）✓
- §5.3 result_window 改造（删 save_record/archive、result-edited 改 Command）→ Task 8 + Task 9 ✓
- §6 一次性迁移（history + model.json，幂等 user_version==0）→ Task 3 + Task 4 + Task 6 ✓
- §6.1 迁移后运行时由 DB 注入（set_runtime_config）→ Task A ✓
- §1.2 不做项（config.yaml 不动、duration_ms 首期 NULL、不删文件）→ 已遵守 ✓
- §7 coordinator 集成点 → Task 7 + Task 8 + Task B ✓

**Placeholder scan**：无 TBD/TODO；所有代码块完整；engine/llm_model 字段名标注「以 config.rs 为准」并给出定位方法（非占位符，是真实的不确定项 + 解决路径）。

**Type consistency**：`insert_transcription` 签名（Task 5 定义、Task 8 调用）参数顺序一致 `(raw_text, polished_text, polish_status, polish_model, engine, engine_mode)`；`Command::ResultEdited { text }`（Task 8 定义、Task 9 发送、handle_result_edited 接收）一致；`raw_text` 字段（Task 7 加、Task 8 取）一致。

**已知不确定项**（执行时以实际代码为准，plan 已给定位方法）：
- `DesktopConfig` 的引擎名字段（`engine_name` / `asr_engine`）与润色模型字段（`llm_model`）确切名 → `crates/desktop/src/config.rs` 查。
- `Coordinator` 是否已被 `app.manage` → `main.rs` 查；Task 9 Step 3 给了 `try_state` 取用方式。
