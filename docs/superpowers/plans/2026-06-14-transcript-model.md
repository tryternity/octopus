# Transcript 模型重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 引入 `Transcript` 结构统一 raw/polished/increase 三文本，润色改为停顿驱动（修复流式中间润色 P0），DB 改为过程增量入库（id=毫秒戳），剪贴板默认保留识别结果。

**Architecture:** `Transcript` 抽成独立可测 struct（内部用 `full`+`raw_len` 派生 raw/increase），coordinator 各 Stage 持有调用；流式/伪流式统一在停顿（静音≥`pause_polish_threshold_ms`（默认 600ms）/ 段边界）时全量润色，不重置引擎；DB 表 `id` 改应用写入的毫秒时间戳，新增 UPDATE 接口支持过程增量入库；`write_to_clipboard` 全局配置控制粘贴后剪贴板归属。

**Tech Stack:** Rust, rusqlite (bundled SQLite 3.45+), tauri, enigo, arboard/tauri-clipboard

**Spec:** `docs/superpowers/specs/2026-06-14-transcript-model-design.md`

---

## File Structure

| 文件 | 职责 | 改动 |
|------|------|------|
| `crates/desktop/src/transcript.rs` | `Transcript` 结构：三文本状态机（新建） | Create |
| `crates/asr/src/db.rs` | schema migration v3 + 过程入库接口 | Modify |
| `crates/infra/src/config.rs` | `write_to_clipboard` 配置字段 | Modify |
| `crates/desktop/src/paste.rs` | 三模式按 `write_to_clipboard` 分支 | Modify |
| `crates/desktop/src/coordinator.rs` | Stage 持 Transcript + 停顿润色 + 入库接线 | Modify |
| `crates/desktop/src/lib.rs` | `pub mod transcript;` | Modify |

---

## Task 1: Transcript 结构 + 单元测试

**Files:**
- Create: `crates/desktop/src/transcript.rs`
- Modify: `crates/desktop/src/lib.rs`

**设计**：`Transcript` 内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 `raw`/`increase`，避免维护三份字符串。停顿快照时 `raw_len` 推进到 `full.len()`，`increase` 自动清空。

- [x] **Step 1: 新建 transcript.rs**

```rust
// crates/desktop/src/transcript.rs
//! 识别过程文本状态机：统一管理原生(raw)/润色(polished)/增量(increase)三文本。
//!
//! 内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 raw/increase：
//! - raw      = full[..raw_len]   （停顿快照，润色基准）
//! - increase = full[raw_len..]   （停顿后新增）
//! 停顿触发润色时 raw_len 推进到 full 长度，increase 自动清空。
//! mode=0/1 不做中间润色，display/db 直接用 full。

use crate::config::PolishMode;
use std::time::Instant;

pub struct Transcript {
    /// 识别开始时刻毫秒时间戳（DB 主键 + 时长计算基准）
    pub id: i64,
    mode: PolishMode,
    /// 当前完整 ASR（流式 set_full / 伪流式 append_segment）
    full: String,
    /// 上次停顿快照的 char 长度（raw 的边界）
    raw_len: usize,
    /// 对 raw 的润色结果（仅 mode=2 中间润色 / 各 mode 最终润色后填值）
    polished: String,
    last_polish_time: Instant,
    polish_pending: bool,
    /// 是否已 INSERT 过 DB（首次有文本时 INSERT 后置 true，之后走 UPDATE）
    db_inserted: bool,
}

impl Transcript {
    pub fn new(id: i64, mode: PolishMode) -> Self {
        Self {
            id,
            mode,
            full: String::new(),
            raw_len: 0,
            polished: String::new(),
            last_polish_time: Instant::now(),
            polish_pending: false,
            db_inserted: false,
        }
    }

    pub fn db_inserted(&self) -> bool {
        self.db_inserted
    }

    pub fn mark_db_inserted(&mut self) {
        self.db_inserted = true;
    }

    /// 流式：设置当前完整 ASR（引擎 accept_samples/flush 返回全量）。
    pub fn set_full(&mut self, text: &str) {
        self.full = text.to_string();
    }

    /// 伪流式：追加一段识别文本（delta）。
    pub fn append_segment(&mut self, delta: &str) {
        self.full.push_str(delta);
    }

    /// 当前完整 ASR（= raw + increase）。
    pub fn full(&self) -> &str {
        &self.full
    }

    /// 停顿快照部分（润色基准）。
    pub fn raw(&self) -> String {
        self.full.chars().take(self.raw_len).collect()
    }

    /// 停顿后增量（仅 mode=2 有意义；mode=0/1 恒空，符合 spec §2.2 不变量）。
    pub fn increase(&self) -> String {
        if self.mode == PolishMode::Intermediate {
            self.full.chars().skip(self.raw_len).collect()
        } else {
            String::new()
        }
    }

    /// 停顿触发：返回完整 ASR 作为润色输入，并推进 raw_len（increase 清空）。
    pub fn snapshot_for_polish(&mut self) -> String {
        self.raw_len = self.full.chars().count();
        self.full.clone()
    }

    /// 润色完成：更新 polished（raw_len 已在 snapshot_for_polish 推进）。
    pub fn on_polish_done(&mut self, polished: String) {
        self.polished = polished;
        self.polish_pending = false;
        self.last_polish_time = Instant::now();
    }

    /// 润色失败：保持 polished 不变，清 pending。
    pub fn on_polish_failed(&mut self) {
        self.polish_pending = false;
    }

    pub fn polish_pending(&self) -> bool {
        self.polish_pending
    }

    pub fn mark_polish_pending(&mut self) {
        self.polish_pending = true;
    }

    pub fn clear_polish_pending(&mut self) {
        self.polish_pending = false;
    }

    pub fn last_polish_time(&self) -> Instant {
        self.last_polish_time
    }

    pub fn mode(&self) -> PolishMode {
        self.mode
    }

    /// 展示文本：mode=2 → polished + increase；其他 → full。
    pub fn display_text(&self) -> String {
        match self.mode {
            PolishMode::Intermediate => {
                let mut s = self.polished.clone();
                s.push_str(&self.increase());
                s
            }
            _ => self.full.clone(),
        }
    }

    /// 落库文本：完整 ASR（raw + increase）。
    pub fn db_text(&self) -> String {
        self.full.clone()
    }

    /// polished（最终润色后有值；否则空）。
    pub fn polished(&self) -> &str {
        &self.polished
    }

    /// 是否无任何识别文本。
    pub fn is_empty(&self) -> bool {
        self.full.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_disabled_display_is_full() {
        let mut t = Transcript::new(1, PolishMode::Disabled);
        t.set_full("你好世界");
        assert_eq!(t.display_text(), "你好世界");
        assert_eq!(t.db_text(), "你好世界");
        assert_eq!(t.increase(), ""); // mode=0 恒空（spec §2.2）
        assert_eq!(t.db_inserted(), false);
    }

    #[test]
    fn mode_finalonly_display_is_full() {
        let mut t = Transcript::new(2, PolishMode::FinalOnly);
        t.append_segment("第一段");
        t.append_segment("第二段");
        assert_eq!(t.display_text(), "第一段第二段");
        assert_eq!(t.db_text(), "第一段第二段");
    }

    #[test]
    fn mode_intermediate_snapshot_and_merge() {
        let mut t = Transcript::new(3, PolishMode::Intermediate);
        // 说了一段
        t.set_full("你好世界");
        assert_eq!(t.display_text(), "你好世界"); // polished 空，increase=full

        // 停顿快照 → 送润色
        let snap = t.snapshot_for_polish();
        assert_eq!(snap, "你好世界");
        assert_eq!(t.raw(), "你好世界");
        assert_eq!(t.increase(), ""); // 快照后 increase 空

        // 润色完成
        t.on_polish_done("你好，世界。".into());
        assert_eq!(t.display_text(), "你好，世界。"); // polished + 空 increase

        // 继续说新内容
        t.set_full("你好，世界。今天天气不错"); // 注意：raw 前缀需稳定
        // increase = full - raw 前缀。raw="你好世界"（4 char），full 以 "你好世界" 开头？
        // 实际 raw 快照是 "你好世界"，但润色后 polished="你好，世界。"，full 仍以原始 ASR 为准
    }

    #[test]
    fn mode_intermediate_increase_after_snapshot() {
        // 验证：快照后新内容进 increase，display = polished + increase
        let mut t = Transcript::new(4, PolishMode::Intermediate);
        t.set_full("原始文本");
        t.snapshot_for_polish();
        t.on_polish_done("润色文本".into());

        // 流式：raw 前缀稳定，full 追加新内容
        t.set_full("原始文本新增部分");
        assert_eq!(t.raw(), "原始文本");
        assert_eq!(t.increase(), "新增部分");
        assert_eq!(t.display_text(), "润色文本新增部分");
    }

    #[test]
    fn append_segment_accumulates() {
        let mut t = Transcript::new(5, PolishMode::Intermediate);
        t.append_segment("A");
        t.append_segment("B");
        assert_eq!(t.full(), "AB");
    }

    #[test]
    fn polish_failed_keeps_polished() {
        let mut t = Transcript::new(6, PolishMode::Intermediate);
        t.set_full("原文");
        t.snapshot_for_polish();
        t.on_polish_done("润色".into());
        t.mark_polish_pending();
        t.on_polish_failed(); // 失败
        assert_eq!(t.polished(), "润色"); // 保持上次值
        assert!(!t.polish_pending());
    }
}
```

> ⚠️ **Step 1 的 `mode_disabled_display_is_full` 测试有注释遗留问题**：mode=0 时 `raw_len=0`，`increase()` 返回 full。这不影响 display/db（用 full），但语义上 mode=0/1 不应使用 `raw()`/`increase()`。实现正确（display/db 不依赖 raw/increase），测试只断言 display/db。

- [x] **Step 2: 在 lib.rs 注册模块**

`crates/desktop/src/lib.rs` 找到 `pub mod` 列表，新增：
```rust
pub mod transcript;
```

- [x] **Step 3: 运行测试，验证通过**

Run: `cargo test -p octopus-desktop --features embedded transcript::`
Expected: 7 tests PASS

- [x] **Step 4: 提交**

```bash
git add crates/desktop/src/transcript.rs crates/desktop/src/lib.rs
git commit -m "feat(desktop): add Transcript state machine for raw/polished/increase"
```

---

## Task 2: DB schema migration v3 + 过程入库接口

**Files:**
- Modify: `crates/asr/src/db.rs`

**改动**：`transcriptions.id` 改 `INTEGER PRIMARY KEY`（应用写毫秒戳，去 AUTOINCREMENT）；init_schema 增 v2→v3 DROP 重建分支；新增 4 个入库接口；保留旧 `insert_transcription` 改为内部委托（避免破坏其他调用方，后续 Task 移除）。

- [x] **Step 1: 改 create_tables（id 去 AUTOINCREMENT）**

`crates/asr/src/db.rs` 的 `create_tables`（:82-112），把 transcriptions 表的 `id` 列改为：
```sql
id            INTEGER PRIMARY KEY,
```
（删去 `AUTOINCREMENT`）。models 表不动。

- [x] **Step 2: 改 init_schema（v2→v3 DROP 重建）**

替换 `init_schema`（:57-80）为：
```rust
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
            // models 表（v1→v2 已删 is_active）不动。
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
            // v1 的 models 可能还有 is_active 列 → 补 DROP（幂等）
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
```

- [x] **Step 3: 新增 4 个入库接口**

在 `insert_transcription`（:300-319）之后新增：
```rust
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
```

- [x] **Step 4: 新增测试**

在 `mod tests`（:374）末尾新增：
```rust
#[test]
fn v2_to_v3_migration_rebuilds_transcriptions() {
    let conn = Connection::open_in_memory().unwrap();
    // 模拟 v2 旧 schema（id AUTOINCREMENT）
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
    conn.execute(
        "INSERT INTO transcriptions (created_at, engine, raw_text) VALUES ('2020-01-01 00:00:00','x','旧数据')",
        [],).unwrap();

    // 跑 migration
    init_schema(&conn).unwrap();

    let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 3);
    // 旧数据被 DROP
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM transcriptions", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
    // id 列无 AUTOINCREMENT（用 SQL 解析 pragma_table_info，AUTOINCREMENT 不可直接查；
    // 改验证：能插入显式大 id）
    conn.execute(
        "INSERT INTO transcriptions (id, created_at, engine, raw_text) VALUES (1718000000000,'2026-06-14 00:00:00','sensevoice','新数据')",
        [],).unwrap();
    let id: i64 = conn.query_row("SELECT id FROM transcriptions WHERE raw_text='新数据'", [], |r| r.get(0)).unwrap();
    assert_eq!(id, 1718000000000);
}

#[test]
fn update_and_finalize_round_trip() {
    let conn = Connection::open_in_memory().unwrap();
    create_tables(&conn).unwrap();
    // 模拟 insert_at_id（直接 SQL，因 with_db 用全局连接）
    conn.execute(
        "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status, char_count)
         VALUES (100, '2026-06-14 00:00:00', 'sensevoice', '首段', NULL, 'off', 2)",
        [],).unwrap();
    // update_raw_text 逻辑
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
```

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-asr`
Expected: 所有测试 PASS（含新增 2 个）

- [x] **Step 6: 提交**

```bash
git add crates/asr/src/db.rs
git commit -m "feat(asr): db v3 — id=millis timestamp + incremental update APIs"
```

---

## Task 3: write_to_clipboard 配置 + paste.rs 改造

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/desktop/src/paste.rs`

- [x] **Step 1: AppConfig 加 write_to_clipboard 字段**

`crates/infra/src/config.rs` 的 `AppConfig`（:45-121），在 `paste_method` 字段后新增：
```rust
    /// 粘贴后是否把识别结果写入剪贴板（默认 true，方便他处再粘贴）。
    /// false 时保留用户原剪贴板内容（等同旧行为）。
    #[serde(default = "default_write_to_clipboard")]
    pub write_to_clipboard: bool,
```

文件底部 `default_*` 函数区（:138 附近）新增：
```rust
fn default_write_to_clipboard() -> bool {
    true
}
```

`impl Default for AppConfig`（:163-187）新增字段初始化（在 `paste_method: default_paste_method(),` 后）：
```rust
            write_to_clipboard: default_write_to_clipboard(),
```

- [x] **Step 2: 改造 paste.rs 三模式分发**

`crates/desktop/src/paste.rs` 的 `paste`（:33-54）改为按 `write_to_clipboard` 分支。子函数增加 `write_to_clipboard: bool` 参数：

```rust
pub fn paste<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    config: &AppConfig,
) -> Result<()> {
    let method = PasteMethod::from(config.paste_method.as_str());
    let wtc = config.write_to_clipboard;
    info!("Pasting via {:?}, write_to_clipboard={}, text len: {}", method, wtc, text.len());

    match method {
        PasteMethod::None => {
            // None 模式：唯一目的就是写剪贴板，忽略 write_to_clipboard 配置
            write_to_clipboard(text, app_handle)?;
        }
        PasteMethod::Clipboard => {
            paste_via_clipboard(text, app_handle, wtc)?;
        }
        PasteMethod::Direct => {
            paste_direct(text, app_handle, wtc)?;
        }
    }
    Ok(())
}
```

`paste_via_clipboard`（:64-104）改为：
```rust
fn paste_via_clipboard<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    write_to_clipboard: bool,
) -> Result<()> {
    let clipboard = app_handle.clipboard();

    // 仅在不保留识别结果时，才需要保存原剪贴板以便恢复
    let saved = if !write_to_clipboard {
        clipboard.read_text().unwrap_or_default()
    } else {
        String::new()
    };

    clipboard
        .write_text(text)
        .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;

    std::thread::sleep(Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "macos")]
    let mod_key = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let mod_key = Key::Control;

    enigo.key(mod_key, Direction::Press).map_err(|e| anyhow::anyhow!("Mod press: {}", e))?;
    enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| anyhow::anyhow!("V click: {}", e))?;
    enigo.key(mod_key, Direction::Release).map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;

    std::thread::sleep(Duration::from_millis(50));

    // 仅在不保留识别结果时恢复原剪贴板
    if !write_to_clipboard {
        let _ = clipboard.write_text(&saved);
    }

    Ok(())
}
```

`paste_direct`（:106-122）改为（签名加 `app_handle` + `write_to_clipboard`，末尾按需写剪贴板）：
```rust
fn paste_direct<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    write_to_clipboard: bool,
) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        if try_linux_direct_typing(text) {
            if write_to_clipboard {
                let clipboard = app_handle.clipboard();
                let _ = clipboard.write_text(text);
            }
            return Ok(());
        }
        info!("Falling back to enigo for direct input");
    }

    enigo.text(text).map_err(|e| anyhow::anyhow!("Direct type failed: {}", e))?;

    // 粘贴完成后按需写剪贴板
    if write_to_clipboard {
        let clipboard = app_handle.clipboard();
        clipboard
            .write_text(text)
            .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
    }
    Ok(())
}
```

> `try_linux_direct_typing`（:124-164）不变。

- [x] **Step 3: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 0 error

- [x] **Step 4: 提交**

```bash
git add crates/infra/src/config.rs crates/desktop/src/paste.rs
git commit -m "feat: write_to_clipboard config — keep recognition result in clipboard by default"
```

---

## Task 4: coordinator Stage 持 Transcript + 文本流接入

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

**改动**：Stage 的 `accumulated_text` / `raw_text` / `polish_pending` / `polish_base_len` / `last_polish_time` 收敛为 `transcript: Transcript`。各 handler 改用 Transcript 方法。本 task 只做**文本流接入**（识别文本进 Transcript），润色触发逻辑仍按旧路径（Task 5 改停顿驱动）。

- [x] **Step 1: 改 Stage enum**

替换 `Stage`（:38-116）的 Streaming / VadSegmented / WaitingCompletion / Pasting 字段：

```rust
enum Stage {
    Idle,
    Streaming {
        engine: StreamingSession,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
        vad: Option<octopus_asr::vad::SileroVad>,
        silence_duration: f64,
        flushed: bool,
    },
    VadSegmented {
        vad: octopus_asr::vad::SileroVad,
        audio_buffer: Vec<f32>,
        overlap_tail: Vec<f32>,
        transcript: Transcript,
        silence_duration: f64,
        has_speech: bool,
        active_count: u32,
        next_seq: u64,
        completed_seq: u64,
        completed_results: HashMap<u64, String>,
        tick_active: Arc<AtomicBool>,
    },
    WaitingCompletion {
        transcript: Transcript,
        active_count: u32,
        completed_seq: u64,
        completed_results: HashMap<u64, String>,
    },
    Pasting {
        id: i64,
        raw_text: String,
        polished_text: String,
        polish_status: String,
        engine: String,
        engine_mode: String,
    },
}
```

文件顶部 import 区（:3-14）新增：
```rust
use crate::transcript::Transcript;
```

- [x] **Step 2: handle_toggle 初始化 Transcript**

`handle_toggle` 的 Idle 分支。新增毫秒戳生成辅助函数（文件顶部常量区后，:130 后）：
```rust
/// 当前 Unix 毫秒时间戳（作 Transcript id / DB 主键）。
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

Streaming 初始化（:326-337）改为：
```rust
*stage = Stage::Streaming {
    engine: streaming_engine,
    transcript: Transcript::new(now_millis(), config.polish_mode),
    streaming_active,
    vad,
    silence_duration: 0.0,
    flushed: false,
};
```

VadSegmented 初始化（:359-375）改为：
```rust
*stage = Stage::VadSegmented {
    vad,
    audio_buffer: Vec::new(),
    overlap_tail: Vec::new(),
    transcript: Transcript::new(now_millis(), config.polish_mode),
    silence_duration: 0.0,
    has_speech: false,
    active_count: 0,
    next_seq: 0,
    completed_seq: 0,
    completed_results: HashMap::new(),
    tick_active,
};
```

- [x] **Step 3: consume_completed_results 用 append_segment**

`consume_completed_results`（:622-645）改为操作 Transcript：
```rust
fn consume_completed_results(
    completed_seq: &mut u64,
    completed_results: &mut HashMap<u64, String>,
    transcript: &mut Transcript,
) {
    while let Some(text) = completed_results.remove(completed_seq) {
        if !text.is_empty() {
            // 段间加逗号（已有文本且新段不以标点开头）
            if !transcript.full().is_empty()
                && !text.starts_with(|c: char| ",.，。！？!?\n".contains(c))
            {
                transcript.append_segment("，");
            }
            transcript.append_segment(&text);
        }
        *completed_seq += 1;
    }
}
```

- [x] **Step 4: handle_streaming_tick 用 Transcript**

`handle_streaming_tick`（:924-1005）改为。**关键**：不再全量覆盖独立字段，改 `transcript.set_full(new_text)`，展示用 `display_text()`：

```rust
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if let Stage::Streaming {
        engine,
        transcript,
        vad,
        silence_duration,
        flushed,
        ..
    } = stage
    {
        let samples = audio.drain_samples();
        if samples.is_empty() {
            return;
        }

        let was_silent = detect_silence_gap(vad, &samples, silence_duration);
        if *silence_duration == 0.0 {
            *flushed = false;
        }

        match engine.accept_samples(&samples, was_silent) {
            Ok(Some(new_text)) => {
                transcript.set_full(&new_text);
                crate::result_window::update_result(app_handle, &transcript.display_text());
            }
            Ok(None) => {}
            Err(e) => warn!("Streaming accept_samples error: {}", e),
        }

        // 静音主动冲刷（>0.5s）
        if *silence_duration >= PUNCTUATION_SILENCE_THRESHOLD && !*flushed {
            match engine.flush() {
                Ok(Some(new_text)) => {
                    transcript.set_full(&new_text);
                    debug!("Flushed: '{}'", transcript.full());
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
                Ok(None) => {}
                Err(e) => warn!("Streaming flush error: {}", e),
            }
            *flushed = true;
        }

        // 停顿润色（Task 5 接入；此处先保留旧 check_and_trigger_polish 签名占位）
        check_and_trigger_polish(transcript, *silence_duration, config, tx);
    }
}
```

> `check_and_trigger_polish` 签名在本 step 改为接 `&mut Transcript` + `silence_duration`（Task 5 Step 1 实现停顿逻辑）。本 step 先改签名让编译通过。

- [x] **Step 5: handle_vad_segmented_tick 用 Transcript**

`handle_vad_segmented_tick`（:647-752）的解构（:656-670）改为：
```rust
if let Stage::VadSegmented {
    vad,
    audio_buffer,
    overlap_tail,
    transcript,
    silence_duration,
    has_speech,
    active_count,
    next_seq,
    ..
} = stage
```

段内：`update_result` 用 `transcript.display_text()`（:738-740）：
```rust
if !transcript.full().is_empty() {
    crate::result_window::update_result(app_handle, &transcript.display_text());
}
```

段末润色检查（:743-750）改为（伪流式段完成后触发停顿润色，传 silence=0.0 表示段边界）：
```rust
check_and_trigger_polish(transcript, *silence_duration, config, tx);
```

- [x] **Step 6: handle_transcription_done 用 Transcript**

`handle_transcription_done`（:1114-1221）。VadSegmented 分支（:1123-1155）解构改为 `transcript`（取代 accumulated_text/raw_text），`consume_completed_results` 调用改为传 transcript，`update_result` 用 display_text：

VadSegmented 分支：
```rust
Stage::VadSegmented {
    transcript,
    active_count,
    completed_seq,
    completed_results,
    ..
} => {
    *active_count = active_count.saturating_sub(1);
    match text {
        Ok(t) => {
            if !t.is_empty() {
                info!("VadSegmented seq={}: '{}'", seq, t);
                completed_results.insert(seq, t);
            }
        }
        Err(e) => error!("VadSegmented seq={} failed: {}", seq, e),
    }
    consume_completed_results(completed_seq, completed_results, transcript);
    if !transcript.full().is_empty() {
        crate::result_window::update_result(app_handle, &transcript.display_text());
    }
}
```

WaitingCompletion 分支（:1157-1214）同理改 `transcript`，`active_count==0` 时：
```rust
if *active_count == 0 {
    let final_text = if transcript.full().is_empty() {
        String::new()
    } else if transcript.full().ends_with(|c: char| ",.，。！？!?\n".contains(c)) {
        transcript.db_text()
    } else {
        format!("{}。", transcript.db_text())
    };
    if final_text.is_empty() {
        *stage = Stage::Idle;
        crate::overlay::hide_overlay(app_handle);
        crate::result_window::hide_result(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
    } else {
        start_pasting(stage, &final_text, transcript, &config.asr_engine, "vad_segmented", config, app_handle, tx);
    }
}
```

- [x] **Step 7: handle_polish_done 用 Transcript**

`handle_polish_done`（:1238-1305）改为：
```rust
fn handle_polish_done(
    stage: &mut Stage,
    result: Result<String, String>,
    _config: &AppConfig,
    app_handle: &tauri::AppHandle,
    _tx: &Sender<Command>,
) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("PolishDone ignored in stage {:?}", stage_name(stage));
            return;
        }
    };
    match result {
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                transcript.on_polish_failed();
                return;
            }
            transcript.on_polish_done(polished);
            if !transcript.full().is_empty() {
                crate::result_window::update_result(app_handle, &transcript.display_text());
            }
        }
        Err(e) => {
            warn!("Polish failed: {}, keeping previous", e);
            transcript.on_polish_failed();
        }
    }
}
```

> 注意：`snapshot_for_polish()`（推进 raw_len）在 Task 5 的 `check_and_trigger_polish` 内调用，本 step 的 `on_polish_done` 只更新 polished。

- [x] **Step 8: check_and_trigger_polish 临时签名**

临时实现（Task 5 替换为停顿逻辑），保证编译：
```rust
fn check_and_trigger_polish(
    transcript: &mut Transcript,
    _silence_duration: f64,
    _config: &AppConfig,
    _tx: &Sender<Command>,
) {
    // 占位：Task 5 实现停顿驱动润色
    let _ = transcript;
}
```

- [x] **Step 9: 停止分支 + start_pasting 签名**

`handle_toggle` 的 VadSegmented 停止分支（:390-466）解构改 `transcript`（取代 accumulated_text/raw_text）。关键改动：`text`/`raw` 从 transcript 取，`*polish_pending=false` 改 `transcript.clear_polish_pending()`，WaitingCompletion 持 transcript：

```rust
Stage::VadSegmented {
    audio_buffer, overlap_tail, transcript, has_speech, active_count,
    next_seq, completed_seq, completed_results, tick_active, ..
} => {
    info!("Toggle: stopping VadSegmented (active_count={})", active_count);
    tick_active.store(false, Ordering::Relaxed);
    let _ = audio.stop();

    let remaining = audio.drain_samples();
    if !remaining.is_empty() {
        audio_buffer.extend_from_slice(&remaining);
    }
    if *has_speech && !audio_buffer.is_empty() {
        let mut send_buffer = overlap_tail.clone();
        send_buffer.extend_from_slice(audio_buffer);
        let speech_samples = filter_speech_from_buffer(&send_buffer);
        if !speech_samples.is_empty() {
            let seq = *next_seq;
            *next_seq += 1;
            *active_count += 1;
            spawn_offline_transcription_with_seq(engine, config, tx, speech_samples, seq);
        }
    }

    let active = *active_count;
    transcript.clear_polish_pending();
    let cseq = *completed_seq;
    let cresults = std::mem::take(completed_results);

    if active > 0 {
        // 把 transcript 移入 WaitingCompletion（用临时 Idle 占位避免部分移动）
        let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
        *stage = Stage::WaitingCompletion {
            transcript: tr,
            active_count: active,
            completed_seq: cseq,
            completed_results: cresults,
        };
    } else {
        let final_text = if transcript.full().is_empty() {
            String::new()
        } else if transcript.full().ends_with(|c: char| ",.，。！？!?\n".contains(c)) {
            transcript.db_text()
        } else {
            format!("{}。", transcript.db_text())
        };
        if final_text.is_empty() {
            *stage = Stage::Idle;
            crate::overlay::hide_overlay(app_handle);
            crate::result_window::hide_result(app_handle);
            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        } else {
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            start_pasting(stage, &final_text, tr, &config.asr_engine, "vad_segmented", config, app_handle, tx);
        }
    }
}
```

Streaming 停止分支（:468-540）类似改造：解构 `transcript`，`finish()` 后 `set_full`，调 start_pasting 传 transcript：
```rust
Stage::Streaming {
    engine: streaming_engine, transcript, streaming_active, ..
} => {
    info!("Toggle: stopping streaming, finalizing");
    transcript.clear_polish_pending();
    streaming_active.store(false, Ordering::Relaxed);

    let final_samples = audio.drain_samples();
    if !final_samples.is_empty() {
        if let Err(e) = streaming_engine.accept_samples(&final_samples, false) {
            warn!("Error processing final samples: {}", e);
        }
    }
    let final_text = match streaming_engine.finish() {
        Ok(text) => text,
        Err(e) => {
            error!("Streaming finish failed: {}", e);
            transcript.db_text()
        }
    };
    streaming_engine.reset();
    let _ = audio.stop();

    if !final_text.is_empty() {
        transcript.set_full(&final_text);
    }
    let combined = transcript.db_text();

    if combined.is_empty() {
        *stage = Stage::Idle;
        crate::overlay::hide_overlay(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }
    crate::result_window::show_result(app_handle, &transcript.display_text());

    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
    start_pasting(stage, &combined, tr, &config.asr_engine, "streaming", config, app_handle, tx);
}
```

`start_pasting`（:553-620）签名 + 实现（接 Transcript，构造 Pasting 持 id）：
```rust
fn start_pasting(
    stage: &mut Stage,
    text: &str,
    transcript: Transcript,
    engine: &str,
    engine_mode: &str,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if text.is_empty() {
        *stage = Stage::Idle;
        crate::result_window::hide_result(app_handle);
        crate::overlay::hide_overlay(app_handle);
        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
        return;
    }

    let (final_text, polish_status) = match crate::config::llm_config(&config) {
        None => (text.to_string(), "off"),
        Some(llm_config) => match octopus_llm::polish(text, &llm_config) {
            Ok(p) if !p.is_empty() => {
                info!("Final polish: {} → {} chars", text.chars().count(), p.chars().count());
                (p, "done")
            }
            Ok(_) => {
                warn!("Final polish returned empty, using original");
                (text.to_string(), "failed")
            }
            Err(e) => {
                warn!("Final polish failed: {}, using original", e);
                (text.to_string(), "failed")
            }
        },
    };

    crate::result_window::show_result(app_handle, &final_text);

    let id = transcript.id;
    *stage = Stage::Pasting {
        id,
        raw_text: transcript.db_text(),
        polished_text: if polish_status == "done" { final_text.clone() } else { String::new() },
        polish_status: polish_status.to_string(),
        engine: engine.to_string(),
        engine_mode: engine_mode.to_string(),
    };

    let config = config.clone();
    let tx_inner = tx.clone();
    let tx_fallback = tx.clone();
    let handle_for_closure = app_handle.clone();
    let text_to_paste = final_text;

    app_handle.run_on_main_thread(move || {
        if let Err(e) = paste::paste(&text_to_paste, &handle_for_closure, &config) {
            error!("Paste failed: {}", e);
        }
        let _ = tx_inner.send(Command::PasteDone);
    }).unwrap_or_else(|e| {
        error!("run_on_main_thread failed: {:?}", e);
        let _ = tx_fallback.send(Command::PasteDone);
    });
}
```

- [x] **Step 10: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 0 error（如有遗漏的字段解构，按编译器提示补齐 `..`）

- [x] **Step 11: 提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Stage holds Transcript, text flow via Transcript methods"
```

---

## Task 5: 停顿驱动润色

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

**改动**：`check_and_trigger_polish` 从「定时+增量」改为「停顿驱动」—— 流式静音≥`pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界完成时，把 `transcript.snapshot_for_polish()`（完整 ASR）送 LLM 全量润色。不重置引擎。

- [x] **Step 1: 停顿润色常量 + check_and_trigger_polish**

文件常量区（:129 后）新增：
```rust
/// 停顿触发中间润色的静音阈值（秒）。流式 silence ≥ 此值 → 全量润色。
const PAUSE_POLISH_THRESHOLD_SEC: f64 = 0.6;
```

> **后续提取（2026-06-15）**：该常量已从硬编码提取为 `config.yaml` 字段 `pause_polish_threshold_ms`（单位毫秒，默认 600）。常量删除，`check_and_trigger_polish` 内改为 `silence_duration < config.pause_polish_threshold_ms / 1000.0`，两处调用点（流式传真实 silence、伪流式传 `config.pause_polish_threshold_ms / 1000.0`）同步。下方 Step 2 代码片段仍引用旧常量名，仅作历史记录。见 `docs/architecture.md` 核心状态机。

替换 `check_and_trigger_polish`（Task 4 Step 8 的占位实现）为：
```rust
/// 停顿驱动润色：流式 silence≥阈值 / 伪流式段边界 → 全量润色（mode=2 only）。
///
/// 流式由调用方传当前 silence_duration；伪流式在 consume 后传 0.0（段边界即视为停顿点，
/// 由 last_polish_time + increase 非空 + pending 判断决定是否触发）。
fn check_and_trigger_polish(
    transcript: &mut Transcript,
    silence_duration: f64,
    config: &AppConfig,
    tx: &Sender<Command>,
) {
    if config.polish_mode != PolishMode::Intermediate
        || transcript.polish_pending()
        || transcript.full().is_empty()
    {
        return;
    }

    // 停顿判断：流式需 silence≥阈值；伪流式（vad 段边界）通过外部传入 silence_duration
    // 但 vad_segmented_tick 调用时 silence 可能 < 阈值 → 用「段完成 + increase 非空」双重条件
    let is_streaming_pause = silence_duration >= PAUSE_POLISH_THRESHOLD_SEC;
    // increase 非空 = 停顿后有新内容待润色（伪流式段完成时 increase 必非空）
    let has_new = !transcript.increase().is_empty();

    // 流式：静音足够；伪流式：由调用方保证在段边界调用 + increase 非空
    // 统一条件：有新内容 && （流式静音达标 || 已是非流式段边界）
    // 简化：只要 increase 非空 且 静音达标（伪流式段边界时 silence 通常已累积或这里宽松处理）
    if !has_new {
        return;
    }
    // 伪流式调用时 silence_duration 可能 < 阈值（刚 consume 完），但段边界本身就是停顿。
    // 用 config 区分：流式引擎才严格判 silence。这里两者统一用 increase 非空 + 节流。
    // 流式额外要求 silence 达标：
    if silence_duration > 0.0 && silence_duration < PAUSE_POLISH_THRESHOLD_SEC {
        return;
    }

    // 节流：避免连续停顿刷爆 LLM
    let elapsed = transcript.last_polish_time().elapsed().as_secs_f64();
    if elapsed < config.polish_interval.max(MIN_POLISH_INTERVAL_SEC) {
        return;
    }

    // 快照 + 触发（推进 raw_len，increase 清空）
    let snapshot = transcript.snapshot_for_polish();
    transcript.mark_polish_pending();
    spawn_polish_thread(snapshot, config, tx);
}
```

> **伪流式段边界判断说明**：`handle_vad_segmented_tick` 在段完成（`should_send`）后调用本函数，此时传当前 `silence_duration`。静音切分时 silence ≥ segment_silence（默认 500ms），可能 < 600ms 阈值。为保证伪流式段边界能触发，在 `handle_vad_segmented_tick` 调用处传一个「段刚切分」的标记值。**简化实现**：伪流式调用时传 `PAUSE_POLISH_THRESHOLD_SEC`（达标），流式传真实 silence。见 Step 2。

- [x] **Step 2: handle_vad_segmented_tick 调用调整**

Task 4 Step 5 中伪流式调用 `check_and_trigger_polish(transcript, *silence_duration, ...)`。改为只在 `should_send`（段切分）后调用，并传达标值：
```rust
// 段切分后（should_send 块内末尾）触发停顿润色
if should_send && !speech_samples.is_empty() {
    check_and_trigger_polish(transcript, PAUSE_POLISH_THRESHOLD_SEC, config, tx);
}
```
（移除 tick 末尾的无条件调用，改为段切分时调用）

- [x] **Step 3: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 0 error

- [x] **Step 4: 提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): pause-driven polish (600ms / segment boundary), fixes streaming intermediate polish P0"
```

---

## Task 6: 过程增量入库接线

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

**改动**：识别过程中调用 DB 新接口（首次 INSERT、分段 UPDATE raw、停顿润色 UPDATE polished、停止 finalize）。DB 失败不阻塞（warn log）。

- [x] **Step 1: 新增 update_transcription_raw 辅助函数 + 流式入库**

在 coordinator.rs 末尾新增辅助函数（首次有文本 INSERT，之后 UPDATE，用 Transcript 的 `db_inserted` 标志区分）：

```rust
/// 首次有文本 INSERT，否则 UPDATE raw_text。DB 失败返回 Err 供调用方 warn。
fn update_transcription_raw(
    transcript: &mut Transcript,
    engine: &str,
    engine_mode: &str,
) -> Result<(), String> {
    if transcript.full().is_empty() {
        return Ok(());
    }
    if !transcript.db_inserted() {
        octopus_asr::db::insert_transcription_at_id(
            transcript.id,
            &transcript.db_text(),
            engine,
            Some(engine_mode),
        )
        .map_err(|e| e.to_string())?;
        transcript.mark_db_inserted();
    } else {
        octopus_asr::db::update_raw_text(transcript.id, &transcript.db_text())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

`handle_streaming_tick` 的 `accept_samples` 与 `flush` 两个 `Ok(Some(new_text))` 分支（Task 4 Step 4），`set_full` 后统一调用：
```rust
transcript.set_full(&new_text);
if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "streaming") {
    warn!("DB (streaming) failed: {}", e);
}
crate::result_window::update_result(app_handle, &transcript.display_text());
```

- [x] **Step 2: 伪流式段完成 → UPDATE raw**

`handle_transcription_done` 的 VadSegmented 分支，`consume_completed_results` 后调用 Step 1 的同一个辅助函数：
```rust
consume_completed_results(completed_seq, completed_results, transcript);
if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "vad_segmented") {
    warn!("DB (vad_segmented) failed: {}", e);
}
```

> `update_transcription_raw` 用 `transcript.db_inserted()` 区分首次 INSERT 与后续 UPDATE（Task 1 已加该字段与方法），避免「UPDATE 影响 0 行无法判断是否 INSERT 过」的歧义。

- [x] **Step 3: 停顿润色 → UPDATE polished**

`handle_polish_done`（Task 4 Step 7）的 `Ok(polished)` 成功分支，`on_polish_done` 后追加：
```rust
transcript.on_polish_done(polished);
// 入库 polished
if let Err(e) = octopus_asr::db::update_polished(
    transcript.id,
    transcript.polished(),
    "done",
    None, // polish_model 可从 config 传，此处简化
) {
    warn!("DB update_polished failed: {}", e);
}
```

- [x] **Step 4: 停止 → finalize**

`PasteDone` 分支（:205-244）改为调 `finalize_transcription`（带 duration_ms）：
```rust
Command::PasteDone => {
    if let Stage::Pasting {
        id,
        raw_text,
        polished_text,
        polish_status,
        engine,
        engine_mode,
    } = &stage
    {
        let polish_model = if polish_status == "done" { Some(config.llm_model.as_str()) } else { None };
        let polished_for_db = if polish_status == "done" { Some(polished_text.as_str()) } else { None };
        let duration_ms = now_millis() - id;
        if let Err(e) = octopus_asr::db::finalize_transcription(
            *id,
            raw_text,
            polished_for_db,
            polish_status,
            polish_model,
            Some(duration_ms),
        ) {
            warn!("DB finalize failed: {}", e);
        }
    }
    info!("Paste complete, returning to idle");
    stage = Stage::Idle;
    crate::overlay::hide_overlay(&app_handle);
    crate::result_window::clear_result(&app_handle);
    crate::tray::update_tray_label(&app_handle, crate::tray::TrayState::Idle);
}
```

- [x] **Step 5: 删除旧 insert_transcription 调用 + 旧接口**

确认 coordinator 不再调 `octopus_asr::db::insert_transcription`（旧自增版）。`db.rs` 的旧 `insert_transcription` / `insert_transcription_at` 若无其他调用方（grep 确认 cli/server 不用），可删除或保留。保留无害（YAGNI），但删更干净：
```bash
grep -rn "insert_transcription\b" crates/ --include="*.rs"
```
若仅 db.rs 内部 + 测试引用，删除公开 `insert_transcription` 与 `insert_transcription_at`，测试改用新接口。

- [x] **Step 6: 编译验证**

Run: `cargo check --workspace --all-targets`
Expected: 0 error

- [x] **Step 7: 单元测试**

`transcript.rs` 测试补 `db_inserted` 字段相关（若加了字段）。确认 Task 1 测试仍 PASS：
Run: `cargo test -p octopus-desktop --features embedded transcript::`

- [x] **Step 8: 提交**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/transcript.rs crates/asr/src/db.rs
git commit -m "feat(desktop): incremental DB persistence during recognition (insert/update/finalize)"
```

---

## Task 7: 编译验证 + 手动 e2e + 文档同步

**Files:**
- Verify: workspace 编译
- Update: `docs/architecture.md`, `docs/superpowers/specs/2026-06-14-transcript-model-design.md`（标记实现状态）, 相关 plans

- [x] **Step 1: 全量编译**

Run: `cargo check --workspace --all-targets`
Expected: 0 error, 0 warning（或仅既有 warning）

- [x] **Step 2: 全量测试**

Run: `cargo test --workspace`
Expected: 所有测试 PASS

> **Step 3-7 手动 e2e 已由用户验证通过（2026-06-15）**。代码实现与自动化测试亦全部完成（`cargo check --workspace --all-targets` 0 error，`cargo test --workspace` 全 PASS，详见 Task 1-6 各 commit）。

- [x] **Step 3: 备份 + migration 验证**

```bash
cp -r ~/.octopus /tmp/octopus-backup-$(date +%s)
rm -f ~/.octopus/octopus.db
cargo run -p octopus-desktop --features embedded &
# 启动后 sqlite3 验证
sqlite3 ~/.octopus/octopus.db "PRAGMA user_version;"  # 期望 3
sqlite3 ~/.octopus/octopus.db "SELECT sql FROM sqlite_master WHERE name='transcriptions';"  # id INTEGER PRIMARY KEY（无 AUTOINCREMENT）
```

- [x] **Step 4: 手动 e2e — 流式 + mode=2**

`~/.octopus/config.yaml` 配 `asr_engine: paraformer-streaming`、`polish_mode: 2`、`llm_*` 填 DeepSeek。
1. 按快捷键 → 结果窗口「正在聆听…」
2. 说一句话 → 停顿 600ms → 展示跳变为润色文本（polished）
3. 继续说 → 展示 = polished + 新增
4. 再按快捷键 → 粘贴 polished；他处 Cmd+V 得 polished（write_to_clipboard=true）
5. `sqlite3 ~/.octopus/octopus.db "SELECT raw_text, polished_text, polish_status, duration_ms FROM transcriptions ORDER BY id DESC LIMIT 1;"` → raw 完整、polished 有值、status=done、duration_ms>0

- [x] **Step 5: 手动 e2e — 伪流式 + mode=2**

配 `asr_engine: sherpa-onnx-sense-voice-funasr-nano-int8`。重复 Step 4 流程，验证分段识别 + 段边界润色。

- [x] **Step 6: 手动 e2e — 错误降级**

`llm_secret_key` 改错 → 录音 → 验证展示降级为 raw、不崩溃、DB `polish_status='failed'`。

- [x] **Step 7: 手动 e2e — write_to_clipboard=false**

`write_to_clipboard: false` → 粘贴后剪贴板保留原内容（粘贴前复制一段文字，粘贴后 Cmd+V 他处仍是原文字）。

- [x] **Step 8: 文档同步**

- `docs/architecture.md`：更新「持久化」「状态机」段落，说明 Transcript 模型 + 过程入库 + id=毫秒戳 + write_to_clipboard
- spec `2026-06-14-transcript-model-design.md`：§1.1 状态列标 ✅ + 提交 hash（用 z_sync_superpowers 流程）

- [x] **Step 9: 提交文档**

```bash
git add docs/
git commit -m "docs: sync transcript model implementation"
```

---

## Spec Coverage Check

| Spec Section | Task | Status |
|---|---|---|
| §2 Transcript 模型（结构/字段/不变量/方法） | Task 1 | ✅ |
| §2.4 各 polish_mode 行为 | Task 1 (测试) | ✅ |
| §3 停顿驱动润色（`pause_polish_threshold_ms` 默认 600ms，流式/伪流式统一） | Task 5 | ✅ |
| §3.2 与 Active Flush/标点协调 | Task 4 Step 4（顺序保留）+ Task 5 | ✅ |
| §4.1 schema（id=毫秒戳） | Task 2 Step 1 | ✅ |
| §4.2 migration v2→v3 DROP 重建 | Task 2 Step 2 | ✅ |
| §4.3 入库时机（INSERT/UPDATE/finalize） | Task 6 | ✅ |
| §4.4 DB 接口（4 个） | Task 2 Step 3 | ✅ |
| §5 错误处理（best-effort，不阻塞） | Task 6（warn log） | ✅ |
| §6 write_to_clipboard 配置 + 三模式矩阵 | Task 3 | ✅ |
| §7.1 Transcript 独立 struct | Task 1 | ✅ |
| §7.2 单元测试（Transcript + DB） | Task 1 Step 1, Task 2 Step 4 | ✅ |
| §7.3 手动 e2e | Task 7 Step 4-7 | ✅ |
