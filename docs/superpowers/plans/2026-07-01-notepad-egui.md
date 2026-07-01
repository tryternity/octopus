# 记事本 egui 迁移实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把记事本从 Tauri webview 窗口迁到独立 egui 原生进程，降低多开内存占用（webview ~80–150MB → egui 进程基线 ~30–50MB，单进程多视图摊薄）。

**Architecture:** 新增二进制 crate `octopus-egui`（eframe，不依赖 tauri），直连共享 SQLite（先迁 WAL 支持多进程并发）。Tauri 主进程通过本地 TCP（JSON line + port 文件/pid 单实例锁）spawn 并驱动 egui 进程。notes 表重建为 `content_text + type`（去 `content_html`），egui 用 markdown 源码 + `egui_commonmark` 实时分屏预览。

**Tech Stack:** Rust + eframe/egui 0.29 + egui_commonmark（md 预览）+ rusqlite（WAL）+ std::net TCP IPC；Tauri 2（spawn + 命令薄层删减）。

**关联 spec：** `docs/superpowers/specs/2026-07-01-notepad-egui-design.md`
**分支：** `worktree-feature-notepad`。**功能完整完成前不往 main 同步。**

---

## 关键背景（给无上下文的工程师）

- **DB 全局单连接模式**：`crates/infra/src/db.rs:94` `static DB: OnceLock<Mutex<Connection>>`，每个进程一份。egui 进程链接 infra 后有自己的 OnceLock——两进程各开一个 Connection 指向同一 `~/.octopus/octopus.db` 文件，**必须先迁 WAL** 否则并发写 `database is locked`。
- **store 层是纯函数**：`crates/notepad/src/store.rs` 的 `*_at(conn, ...)` 接 `&Connection`，egui 进程可直接复用（经 `octopus_infra::db::with_db` 用本进程全局连接）。
- **notes 当前 schema**（`crates/infra/src/db.sql:292`）：含 `content_html`（TipTap 富文本）+ `content_text`（抽取纯文本）。迁移后只留 `content_text` + 新增 `type`（'text'/'markdown'）。
- **user_version 迁移机制**：`db.rs:init_schema` 按 `PRAGMA user_version` 分支。当前最新 v9。本计划 v9→v10 重建 notes 表。
- **前端窗口路由**：`crates/desktop/frontend/src/App.tsx:55` `case "notepad_window"` 渲染 `pages/Notepad`。删 webview 后这页变死代码。
- **worktree-cwd 陷阱**：所有 cargo/grep/git 命令在 worktree 内时，显式用 `--manifest-path crates/...` 或先 `cd` 到 worktree（当前 cwd 即 worktree 根）。

---

## Task 1: WAL 迁移（infra/db.rs，前置，全应用受益）

**Files:**
- Modify: `crates/infra/src/db.rs`（`ensure_db` 内，`Connection::open` 之后、`init_schema` 之前加 PRAGMA）
- Test: `crates/infra/src/db.rs`（tests 模块）

- [ ] **Step 1: 写失败测试（WAL 已生效 + 双连接并发不锁死）**

在 `crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn wal_pragmas_applied_on_file_db() {
    // WAL 不适用于内存 DB，必须用临时文件
    let dir = std::env::temp_dir().join(format!("octopus-wal-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wal.db");

    let conn = Connection::open(&path).unwrap();
    apply_wal_pragmas(&conn);

    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode 应为 WAL");
    let busy: i64 = conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0)).unwrap();
    assert_eq!(busy, 5000);

    // 第二个连接并发读（同一 DB 文件，WAL 下不阻塞）
    let conn2 = Connection::open(&path).unwrap();
    apply_wal_pragmas(&conn2);
    conn.execute_batch("CREATE TABLE IF NOT EXISTS t(x); INSERT INTO t VALUES(1);").unwrap();
    let v: i64 = conn2.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 1, "第二连接应能读到第一连接的写入（WAL 并发）");

    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --manifest-path crates/infra/Cargo.toml wal_pragmas_applied_on_file_db - --nocapture`
Expected: 编译失败（`apply_wal_pragmas` 未定义）。

- [ ] **Step 3: 实现 apply_wal_pragmas 并在 ensure_db 调用**

在 `db.rs`（`ensure_db` 上方，约 `fn db_path` 附近）加：

```rust
/// 设 WAL + busy_timeout + synchronous=NORMAL（多进程并发读写前提）。
/// - WAL：DB 级持久（设一次），产生 -wal/-shm 副文件。
/// - busy_timeout=5000：连接级，遇锁自动重试 5s。
/// - synchronous=NORMAL：WAL 下安全且更快。
fn apply_wal_pragmas(conn: &Connection) {
    // journal_mode 返回新值，execute_batch 不捕获返回行也能生效。
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL;\
         PRAGMA busy_timeout=5000;\
         PRAGMA synchronous=NORMAL;",
    );
}
```

在 `ensure_db` 内，`Connection::open` 之后、`init_schema(&conn)?` 之前插入：

```rust
    let conn = Connection::open(&path)
        .with_context(|| format!("Failed to open DB at {}", path.display()))?;
    apply_wal_pragmas(&conn);          // ← 新增：WAL 必须在建表前设
    init_schema(&conn)?;
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --manifest-path crates/infra/Cargo.toml wal_pragmas`
Expected: PASS。

- [ ] **Step 5: 回归全 infra 测试 + 提交**

Run: `cargo test --manifest-path crates/infra/Cargo.toml`
Expected: 全绿（WAL 不影响现有 in-memory 测试）。

```bash
git add crates/infra/src/db.rs
git commit -m "feat(db): WAL + busy_timeout 迁移，支持多进程并发"
```

---

## Task 2: notes 表 schema 重建（db.sql + v9→v10 迁移）

**Files:**
- Modify: `crates/infra/src/db.sql`（notes 建表段，约 289–324 行）
- Modify: `crates/infra/src/db.rs`（`init_schema`，加 v9→v10 分支 + fresh 设 v10）
- Test: `crates/infra/src/db.rs`

- [ ] **Step 1: 写失败测试（v9→v10 迁移：去 content_html、加 type）**

在 `db.rs` tests 末尾追加：

```rust
#[test]
fn migrate_v9_to_v10_rebuilds_notes_schema() {
    // 模拟旧 v9 库：建旧 notes 表（带 content_html，无 type）→ 设 user_version=9
    let dir = std::env::temp_dir().join(format!("octopus-notes-mig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mig.db");
    let conn = Connection::open(&path).unwrap();
    apply_wal_pragmas(&conn);
    conn.execute_batch(
        "CREATE TABLE notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT,
            content_html TEXT NOT NULL DEFAULT '', content_text TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'manual', source_ref_id INTEGER,
            is_pinned INTEGER NOT NULL DEFAULT 0, is_favorite INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         INSERT INTO notes (title, content_html, content_text, created_at, updated_at)
            VALUES ('旧', '<p>x</p>', 'x', '2026-01-01 00:00:00', '2026-01-01 00:00:00');
         PRAGMA user_version = 9;",
    ).unwrap();
    // 旧数据存在
    let old_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    assert_eq!(old_count, 1);

    // 执行迁移（init_schema 按 user_version 走 v9 分支）
    init_schema(&conn).unwrap();

    // 新 schema：无 content_html，有 type
    let cols: Vec<String> = conn.prepare("PRAGMA table_info(notes)").unwrap()
        .query_map([], |r| r.get::<_, String>(1)).unwrap()
        .filter_map(|r| r.ok()).collect();
    assert!(!cols.contains(&"content_html".to_string()), "content_html 应被删除");
    assert!(cols.contains(&"type".to_string()), "应有 type 列");
    // 旧数据被丢弃（drop+recreate）
    let new_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    assert_eq!(new_count, 0, "drop+recreate 应丢弃旧数据");
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 10);

    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --manifest-path crates/infra/Cargo.toml migrate_v9_to_v10`
Expected: FAIL（v9→v10 分支不存在，schema 未变）。

- [ ] **Step 3: 重写 db.sql 的 notes 建表段**

把 `crates/infra/src/db.sql` 第 289–324 行（`-- ── 记事本（notes 表）...` 到三个 trigger 结束）整体替换为：

```sql
-- ── 记事本（notes 表）─────────────────────────────────────────────────────
-- 内容收集箱：ASR/OCR/剪贴板结果一键存入 + markdown/纯文本整理。
-- type: 'text'（纯文本，ASR/OCR/剪贴板存入）| 'markdown'（egui 编辑的 md 源码）。
-- content_text = 源文本（FTS 索引 + 列表预览 + md 渲染源）；无 content_html（egui 方案去富文本）。
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',
    type          TEXT    NOT NULL DEFAULT 'text',
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_notes_source  ON notes(source);

CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    title, content_text,
    content='notes', content_rowid='id', tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS note_fts_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, title, content_text) VALUES (new.id, new.title, new.content_text);
END;
CREATE TRIGGER IF NOT EXISTS note_fts_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content_text)
    VALUES('delete', old.id, old.title, old.content_text);
END;
CREATE TRIGGER IF NOT EXISTS note_fts_au AFTER UPDATE OF title, content_text ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content_text)
    VALUES('delete', old.id, old.title, old.content_text);
    INSERT INTO notes_fts(rowid, title, content_text) VALUES (new.id, new.title, new.content_text);
END;
```

- [ ] **Step 4: db.rs init_schema 加 v9→v10 分支 + fresh 设 v10**

在 `init_schema` 内：
1. `if v < 2` 分支末尾的 `conn.execute("PRAGMA user_version = 9", [])?;` 改为 `= 10`，并把 `log::info!("DB initialized (v9)...")` 改为 v10 + 去掉 notes 描述里的 content_html 措辞。
2. 在 `else if v == 8 { ... }` 块之后追加新分支：

```rust
    } else if v == 9 {
        // v9 → v10：notes 表重建（去 content_html，加 type；egui 迁移）。
        // 旧数据不迁移（已确认接受）。先 DROP 旧 trigger/fts/table，再重跑 INIT_SQL 建新 schema。
        log::info!("DB migrating v9 → v10: rebuild notes table (drop content_html, add type)...");
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS note_fts_ai;
             DROP TRIGGER IF EXISTS note_fts_ad;
             DROP TRIGGER IF EXISTS note_fts_au;
             DROP TABLE IF EXISTS notes_fts;
             DROP TABLE IF EXISTS notes;",
        )
        .context("v9→v10: DROP 旧 notes")?;
        conn.execute_batch(INIT_SQL).context("v9→v10: 重建 notes 新 schema")?;
        conn.execute("PRAGMA user_version = 10", [])?;
        log::info!("DB migrated to v10: notes (content_text + type)");
    }
```

- [ ] **Step 5: 运行测试确认通过 + 回归**

Run: `cargo test --manifest-path crates/infra/Cargo.toml`
Expected: 全绿（含 `notes_table_and_fts_created` 仍 pass——新 schema 仍有 notes/notes_fts 空表）。

- [ ] **Step 6: 提交**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(db): notes 表重建 content_text+type（v9→v10），去 content_html"
```

---

## Task 3: notepad store/model 重构（适配新 schema）

**Files:**
- Modify: `crates/notepad/src/model.rs`（Note 去掉 content_html，加 note_type；新增 NoteType）
- Modify: `crates/notepad/src/store.rs`（create/update 签名改 content_text+type，SELECT/INSERT/UPDATE/row_to_note 改列）
- Delete: `crates/notepad/src/serialize.rs`（extract_text 仅服务于已删除的 html 抽取，无其他消费者）
- Modify: `crates/notepad/src/lib.rs`（去掉 `pub mod serialize;`）

> 注：model + store + serialize 必须同 task 改（Note 结构体变了，store 编译依赖它；改完前整个 crate 不编译，故走"改测试→改实现→编译过"一次过）。

- [ ] **Step 1: 改 model.rs（NoteType + Note 结构体）**

替换 `crates/notepad/src/model.rs` 中 `Note` 结构体（约 35–47 行），并在 `NoteSource` 之后加 `NoteType`：

```rust
/// 笔记内容类型（决定 egui 渲染形态）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Text,
    Markdown,
}

impl NoteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Text => "text",
            NoteType::Markdown => "markdown",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "markdown" => NoteType::Markdown,
            _ => NoteType::Text,
        }
    }
}

/// 一条笔记（DB notes 表一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    /// 源文本：text=纯文本；markdown=md 源码（egui 渲染源）。
    pub content_text: String,
    /// 内容类型（serde 字段名 "type"，对齐 DB 列）。
    #[serde(rename = "type")]
    pub note_type: NoteType,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

在 `model.rs` 的 `mod tests` 加 NoteType roundtrip：

```rust
    #[test]
    fn note_type_roundtrip() {
        for t in [NoteType::Text, NoteType::Markdown] {
            assert_eq!(NoteType::from_str(t.as_str()), t);
        }
        assert_eq!(NoteType::from_str("???"), NoteType::Text, "未知默认 text");
        assert_eq!(NoteType::default(), NoteType::Text);
    }
```

- [ ] **Step 2: 重写 store.rs 的 create/update 签名 + SQL + row_to_note**

在 `crates/notepad/src/store.rs`：

(a) 删掉顶部 `use crate::serialize::extract_text;`（约第 8 行）。

(b) 所有 SELECT 的列列表（`list_notes_at`、`query_with_search`、`get_note_at` 三处，约 71/95/112/205 行）从：
`id, title, content_html, content_text, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at`
改为：
`id, title, content_text, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at`

(c) `row_to_note` 改为（列顺序对齐上面新 SELECT）：

```rust
fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let type_str: String = row.get(3)?;
    let source_str: String = row.get(4)?;
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        content_text: row.get(2)?,
        note_type: NoteType::from_str(&type_str),
        source: NoteSource::from_str(&source_str),
        source_ref_id: row.get(5)?,
        is_pinned: row.get::<_, i64>(6)? != 0,
        is_favorite: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}
```

(d) `create_note` / `create_note_at` 签名改为接 `content_text` + `note_type`：

```rust
/// 新建笔记。返回新 id（AUTOINCREMENT last_insert_rowid）。title 初始 NULL。
pub fn create_note(
    source: NoteSource,
    source_ref_id: Option<i64>,
    content_text: &str,
    note_type: NoteType,
) -> Result<i64> {
    octopus_infra::db::with_db(|conn| create_note_at(conn, source, source_ref_id, content_text, note_type))
}

pub fn create_note_at(
    conn: &Connection,
    source: NoteSource,
    source_ref_id: Option<i64>,
    content_text: &str,
    note_type: NoteType,
) -> Result<i64> {
    let now = iso_now();
    conn.execute(
        "INSERT INTO notes (title, content_text, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at)
         VALUES (NULL, ?, ?, ?, ?, 0, 0, ?, ?)",
        params![content_text, note_type.as_str(), source.as_str(), source_ref_id, now, now],
    )
    .context("insert note")?;
    Ok(conn.last_insert_rowid())
}
```

(e) `update_note` / `update_note_at` 签名改为 `content_text` + `note_type`：

```rust
/// 更新正文/标题。updated_at = now。title 空串 → 存 NULL。
pub fn update_note(id: i64, title: &str, content_text: &str, note_type: NoteType) -> Result<()> {
    octopus_infra::db::with_db(|conn| update_note_at(conn, id, title, content_text, note_type))
}

pub fn update_note_at(conn: &Connection, id: i64, title: &str, content_text: &str, note_type: NoteType) -> Result<()> {
    let title_db: Option<&str> = if title.trim().is_empty() { None } else { Some(title) };
    conn.execute(
        "UPDATE notes SET title = ?, content_text = ?, type = ?, updated_at = ? WHERE id = ?",
        params![title_db, content_text, note_type.as_str(), iso_now(), id],
    )?;
    Ok(())
}
```

(f) store.rs 顶部 `use crate::model::{Note, NoteFilter, NoteSource};` 加 `NoteType`：
`use crate::model::{Note, NoteFilter, NoteSource, NoteType};`

- [ ] **Step 3: 删 serialize.rs + 去 lib.rs 导出**

```bash
rm crates/notepad/src/serialize.rs
```

`crates/notepad/src/lib.rs` 删掉 `pub mod serialize;`（保留 export/model/store）。

- [ ] **Step 4: 更新 store.rs 测试（适配新签名）**

`crates/notepad/src/store.rs` tests 模块里所有 `create_note_at(&conn, src, ref, "<p>..</p>")` 调用改为 `create_note_at(&conn, src, ref, "纯文本", NoteType::Text)`，`update_note_at(&conn, id, title, "<p>..</p>")` 改为 `update_note_at(&conn, id, title, "纯文本", NoteType::Text)`。具体（按现有测试名逐个改调用点）：

- `create_and_get_roundtrip`：`create_note_at(&conn, NoteSource::Asr, Some(123), "识别文本", NoteType::Text)`；断言 `note.content_text == "识别文本"` 且 `note.note_type == NoteType::Text`，删掉 `content_html` 断言。
- `update_rextracts_text_and_handles_title`：改 `update_note_at(&conn, id, "我的标题", "第一段", NoteType::Markdown)`；断言 `content_text == "第一段"`、`note_type == Markdown`；空标题断言不变。
- `fts_search_three_chars` / `like_fallback_short_query` / `filter_by_source_and_favorite` / `pinned_sorts_first` / `delete_batch_and_empty` / `fts_triggers_sync_on_update_and_delete`：把 `"<p>xxx</p>"` 实参换成纯文本 `"xxx"` + `NoteType::Text`（FTS 仍索引 content_text，断言不变）。

- [ ] **Step 5: 编译 + 测试**

Run: `cargo test --manifest-path crates/notepad/Cargo.toml`
Expected: 全绿。

- [ ] **Step 6: 提交**

```bash
git add crates/notepad/src/model.rs crates/notepad/src/store.rs crates/notepad/src/lib.rs
git rm crates/notepad/src/serialize.rs
git commit -m "refactor(notepad): store/model 适配 content_text+type，移除 html 抽取"
```

---

## Task 4: octopus-egui crate 骨架（验证依赖版本可编）

**Files:**
- Create: `crates/egui/Cargo.toml`
- Create: `crates/egui/src/main.rs`
- Modify: `Cargo.toml`（workspace members 加 `crates/egui`）

- [ ] **Step 1: 加 workspace member**

`Cargo.toml` 第 2 行 members 数组追加 `"crates/egui"`：

```toml
members = ["crates/infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr", "crates/capx", "crates/notepad", "crates/egui"]
```

- [ ] **Step 2: 写 crates/egui/Cargo.toml**

```toml
[package]
name = "octopus-egui"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "octopus-egui"
path = "src/main.rs"

[dependencies]
eframe = "0.29"
egui = "0.29"
egui_commonmark = "0.18"          # md 预览（与 egui 0.29 配对）
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
log = "0.4"

# 直连共享 DB（store 纯函数复用）
octopus-infra = { path = "../infra" }
octopus-notepad = { path = "../notepad" }
```

> 版本配对说明：egui 0.29 ↔ egui_commonmark 0.18。若 Step 4 编译报版本不兼容，按 egui 实际版本查 crates.io 对应的 egui_commonmark 版本对齐（这是 spike#4 的前置校验点）。

- [ ] **Step 3: 写最小 main.rs（空 eframe 窗口）**

```rust
//! octopus-egui：记事本原生进程（eframe）。单进程 + view 路由。
//! 第一阶段仅 NotepadView。Tauri 主进程经本地 TCP IPC spawn 并驱动。

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(|_cc| Ok(Box::new(NotepadApp::default()))),
    )
}

#[derive(Default)]
struct NotepadApp;

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("octopus 记事本（骨架）");
        });
    }
}
```

- [ ] **Step 4: 编译 + 试跑（验证依赖版本）**

Run: `cargo build --manifest-path crates/egui/Cargo.toml`
Expected: 编译成功。若 egui_commonmark 版本不兼容，调整 `Cargo.toml` 版本号重试。

试跑（手动，会弹出空窗口，Ctrl-C 退出）：
Run: `cargo run --manifest-path crates/egui/Cargo.toml`

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml crates/egui/Cargo.toml crates/egui/src/main.rs
git commit -m "feat(egui): octopus-egui crate 骨架（eframe 空窗口）"
```

---

## Task 5: egui IPC server（本地 TCP + JSON line + port 文件/pid 单实例锁）

**Files:**
- Create: `crates/egui/src/ipc.rs`
- Modify: `crates/egui/src/main.rs`（启动 IPC 线程 + 持有接收消息通道）
- Modify: `crates/egui/Cargo.toml`（暂无新增，std::net 即可）

> 协议（与 spec §3.4 一致）：
> - bind `127.0.0.1:0`，OS 分配端口。
> - 把 `{"pid":<pid>,"port":<port>}` 写 `~/.octopus/egui-ipc.port`（单实例锁 + 供 client 连）。
> - JSON line：每行一条 JSON。消息（Tauri→egui）：`{"type":"open","note_id":N}` / `{"type":"notes_changed"}` / `{"type":"show"}`。
> - egui 主线程经 `std::sync::mpsc` 收消息。

- [ ] **Step 1: 写 ipc.rs（server + 消息类型 + port 文件）**

```rust
//! 本地 TCP IPC server（Tauri 主进程 → egui）。
//! - bind 127.0.0.1:0，端口写 ~/.octopus/egui-ipc.port（{pid,port}，单实例锁）。
//! - JSON line：每行一条。收到消息经 mpsc 推给 egui 主线程。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::Sender;

/// port 文件路径：~/.octopus/egui-ipc.port
pub fn port_file() -> std::path::PathBuf {
    octopus_infra::paths::octopus_config_home().join("egui-ipc.port")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMsg {
    /// 打开并选中某笔记（OCR/ASR→notepad）。
    Open { note_id: i64 },
    /// Tauri 侧写笔记后通知刷新列表。
    NotesChanged,
    /// 托盘唤起：show + focus。
    Show,
}

/// 写 port 文件（{pid,port}）。单实例锁的 server 侧凭证。
fn write_port_file(port: u16) -> Result<()> {
    let path = port_file();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let body = serde_json::json!({ "pid": std::process::id(), "port": port });
    std::fs::write(&path, body.to_string())
        .with_context(|| format!("写 port 文件失败: {}", path.display()))?;
    Ok(())
}

/// 启动 IPC server（后台线程）。返回后主线程可从 rx 收消息。
/// 启动失败不阻断 UI（记事本仍可独立用，只是收不到外部 open/refresh）。
pub fn start(tx: Sender<IpcMsg>) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => {
                log::error!("IPC bind 失败: {}", e);
                return;
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        if let Err(e) = write_port_file(port) {
            log::error!("{}", e);
        }
        log::info!("egui IPC listening on 127.0.0.1:{}", port);

        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(&stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break }; // 对端断开
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    match serde_json::from_str::<IpcMsg>(line) {
                        Ok(msg) => {
                            log::info!("IPC recv: {:?}", msg);
                            let _ = tx.send(msg);
                        }
                        Err(e) => log::warn!("IPC 解析失败 ({}): {}", e, line),
                    }
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            });
        }
    });
}
```

- [ ] **Step 2: 写 IPC 单测（loopback client 发消息，server 收到）**

在 `ipc.rs` 末尾加（`#[cfg(test)]`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn server_receives_json_line_messages() {
        let (tx, rx) = mpsc::channel::<IpcMsg>();
        start(tx);
        // 轮询 port 文件直到写出
        let port = loop {
            if let Ok(text) = std::fs::read_to_string(port_file()) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(p) = v["port"].as_u64() {
                        break p as u16;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(stream, "{}", serde_json::json!({"type":"open","note_id":42})).unwrap();
        writeln!(stream, "{}", serde_json::json!({"type":"notes_changed"})).unwrap();
        stream.flush().unwrap();

        let m1 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let m2 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(m1, IpcMsg::Open { note_id: 42 }));
        assert!(matches!(m2, IpcMsg::NotesChanged));

        let _ = std::fs::remove_file(port_file());
    }

    #[test]
    fn ipc_msg_roundtrip() {
        let open = serde_json::to_string(&IpcMsg::Open { note_id: 7 }).unwrap();
        assert!(open.contains("\"type\":\"open\""));
        let parsed: IpcMsg = serde_json::from_str(&open).unwrap();
        assert!(matches!(parsed, IpcMsg::Open { note_id: 7 }));
    }
}
```

> 注：`start` 内 server 线程与测试共享全局 port 文件路径，并发跑会撞；该测试单独跑。两个测试用 `#[test]` 串行（cargo 默认多线程，可能撞 port 文件）。若 CI 撞，加 `#[serial]` 或改为 server 不写 port 文件、直接返回 port。当前接受单测本地绿。

- [ ] **Step 3: main.rs 接通道**

`crates/egui/src/main.rs` 改为：

```rust
mod ipc;

use ipc::IpcMsg;
use std::sync::mpsc;

fn main() -> eframe::Result {
    // IPC 接收通道
    let (tx, rx) = mpsc::channel::<IpcMsg>();
    ipc::start(tx);

    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(move |_cc| Ok(Box::new(NotepadApp::new(rx)))),
    )
}

struct NotepadApp {
    rx: mpsc::Receiver<IpcMsg>,
}

impl NotepadApp {
    fn new(rx: mpsc::Receiver<IpcMsg>) -> Self {
        Self { rx }
    }
}

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 排空 IPC 消息（非阻塞）
        while let Ok(msg) = self.rx.try_recv() {
            log::info!("UI 处理消息: {:?}", msg);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("octopus 记事本（骨架 + IPC）");
        });
        // 持续 request_repaint 让 IPC 消息及时被 poll（200ms）
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}
```

- [ ] **Step 4: 编译 + 单测**

Run: `cargo test --manifest-path crates/egui/Cargo.toml ipc`
Expected: 2 测试 PASS（`ipc_msg_roundtrip` 必过；`server_receives_json_line_messages` 本地应过）。

- [ ] **Step 5: 提交**

```bash
git add crates/egui/src/ipc.rs crates/egui/src/main.rs
git commit -m "feat(egui): 本地 TCP IPC server（JSON line + port 文件单实例锁）"
```

---

## Task 6: egui NotepadView（列表 + 编辑器 + md 预览 + 工具栏 + 防抖保存）

**Files:**
- Create: `crates/egui/src/notepad_view.rs`
- Modify: `crates/egui/src/main.rs`（NotepadApp 持有 NotepadView，处理 IPC 消息）
- Modify: `crates/egui/Cargo.toml`（egui_commonmark 已在 Task 4）

> UI 对齐 spec §3.3：三栏（列表 / md 源码编辑 / 预览分屏）+ 极简 5 按钮 md 工具栏 + 800ms 防抖自动保存。egui 无单测传统，本任务手动验证。

- [ ] **Step 1: 写 notepad_view.rs**

```rust
//! NotepadView：列表 + md 源码编辑 + egui_commonmark 分屏预览 + 5 按钮工具栏。
//! 直连 octopus_notepad::store（经 octopus_infra::db::with_db 用本进程全局连接，WAL）。
//! 编辑走 800ms 防抖保存（对齐原 webview 行为）。

use crate::ipc::IpcMsg;
use egui_commonmark::CommonMarkViewer;
use octopus_notepad::{Note, NoteFilter, NoteSource, NoteType};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(800);

pub struct NotepadView {
    notes: Vec<Note>,
    current_id: Option<i64>,
    title: String,
    body: String,                       // md 源码（编辑缓冲）
    body_dirty: bool,
    last_edit: Option<Instant>,
    pending_select: Option<i64>,        // IPC open 收到、待选中
    refresh_pending: bool,              // IPC notes_changed
}

impl Default for NotepadView {
    fn default() -> Self {
        let mut v = Self {
            notes: Vec::new(),
            current_id: None,
            title: String::new(),
            body: String::new(),
            body_dirty: false,
            last_edit: None,
            pending_select: None,
            refresh_pending: false,
        };
        v.reload_notes();
        // 默认选第一条
        if let Some(first) = v.notes.first().map(|n| n.id) {
            v.select(first);
        }
        v
    }
}

impl NotepadView {
    /// 处理一条 IPC 消息。
    pub fn handle_ipc(&mut self, msg: IpcMsg) {
        match msg {
            IpcMsg::Open { note_id } => {
                self.pending_select = Some(note_id);
            }
            IpcMsg::NotesChanged => {
                self.refresh_pending = true;
            }
            IpcMsg::Show => {
                // show/focus 由 main.rs 的 eframe 层处理（这里无操作）
            }
        }
    }

    fn reload_notes(&mut self) {
        self.notes = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::list_notes_at(conn, &NoteFilter::default())
        })
        .unwrap_or_default();
    }

    /// 选中某笔记：先把当前 dirty 落库，再载入选中。
    fn select(&mut self, id: i64) {
        self.flush_if_dirty();
        let note = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::get_note_at(conn, id)
        })
        .ok()
        .flatten();
        if let Some(n) = note {
            self.current_id = Some(n.id);
            self.title = n.title.unwrap_or_default();
            self.body = n.content_text;
            self.body_dirty = false;
            self.last_edit = None;
        }
    }

    fn mark_dirty(&mut self) {
        self.body_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// 防抖落库：距上次编辑 ≥ DEBOUNCE 才写。
    fn flush_if_dirty(&mut self) {
        if !self.body_dirty {
            return;
        }
        if let Some(t) = self.last_edit {
            if t.elapsed() >= DEBOUNCE {
                self.save_current();
            }
        }
    }

    fn save_current(&mut self) {
        let Some(id) = self.current_id else { return };
        let title = self.title.clone();
        let body = self.body.clone();
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_notepad::store::update_note_at(conn, id, &title, &body, NoteType::Markdown)
        });
        self.body_dirty = false;
        self.last_edit = None;
        self.reload_notes(); // 列表 updated_at 刷新
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        // 退出前 flush（ctx 即将 drop 不易感知，靠防抖 + 切换笔记 flush 兜底）
        self.flush_if_dirty();

        // 处理待选 / 刷新
        if let Some(id) = self.pending_select.take() {
            self.select(id);
            self.reload_notes();
        }
        if self.refresh_pending {
            self.refresh_pending = false;
            self.reload_notes();
        }

        egui::SidePanel::left("list").resizable(true).default_width(240.0).show(ctx, |ui| {
            ui.heading("笔记");
            let mut select_id: Option<i64> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for n in &self.notes {
                    let selected = self.current_id == Some(n.id);
                    let label = n.title.clone().unwrap_or_else(|| {
                        n.content_text.chars().take(20).collect()
                    });
                    if ui.selectable_label(selected, &label).clicked() {
                        select_id = Some(n.id);
                    }
                }
            });
            if let Some(id) = select_id {
                self.select(id);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // 标题
            ui.horizontal(|ui| {
                ui.label("标题:");
                let resp = ui.text_edit_singleline(&mut self.title);
                if resp.changed() {
                    self.mark_dirty();
                }
            });
            ui.separator();

            // 工具栏（5 按钮：选中文本→包 md 语法）
            toolbar(ui, &mut self.body, &mut self.body_dirty, &mut self.last_edit);

            // 编辑 / 预览分屏
            let available = ui.available_size();
            let half = egui::Vec2::new(available.x / 2.0, available.y);
            ui.horizontal(|ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(half);
                    ui.label("Markdown 源码");
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.body)
                            .desired_width(f32::MAX)
                            .desired_rows(20),
                    );
                    if resp.changed() {
                        self.mark_dirty();
                    }
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(half);
                    ui.label("预览");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        CommonMarkViewer::new().show(ui, &self.body);
                    });
                });
            });
        });

        // 持续 repaint 让防抖 timer 可被 poll
        if self.body_dirty {
            ctx.request_repaint_after(DEBOUNCE);
        }
    }
}

/// 5 按钮工具栏：选中文本包 md 语法。
fn toolbar(
    ui: &mut egui::Ui,
    body: &mut String,
    dirty: &mut bool,
    last_edit: &mut Option<Instant>,
) {
    ui.horizontal_wrapped(|ui| {
        let pairs: &[(&str, &str, &str)] = &[
            ("B 粗体", "**", "**"),
            ("I 斜体", "*", "*"),
            ("H 标题", "# ", ""),
            ("• 列表", "- ", ""),
            ("` 代码", "`", "`"),
        ];
        for (label, pre, post) in pairs {
            if ui.small_button(*label).clicked() {
                wrap_selection_or_append(body, pre, post);
                *dirty = true;
                *last_edit = Some(Instant::now());
            }
        }
    });
    ui.separator();
}

/// 简化版：在末尾追加 pre+post（egui TextEdit 没有选区 API，第一版用追加，
/// 后续若需选区包裹再接 egui 0.30 的 text selection）。
fn wrap_selection_or_append(body: &mut String, pre: &str, post: &str) {
    body.push_str(pre);
    body.push_str(post);
}
```

> 工具栏选区包裹说明：egui 0.29 的 `TextEdit` 暴露选区状态有限；第一版用「末尾插入语法标记」对用户可见可点。选区包覆作为后续优化（spike 范围外）。

- [ ] **Step 2: main.rs 持有 NotepadView 并分发 IPC**

```rust
mod ipc;
mod notepad_view;

use ipc::IpcMsg;
use notepad_view::NotepadView;
use std::sync::mpsc;

fn main() -> eframe::Result {
    let (tx, rx) = mpsc::channel::<IpcMsg>();
    ipc::start(tx);

    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "octopus 记事本",
        opts,
        Box::new(move |_cc| Ok(Box::new(NotepadApp::new(rx)))),
    )
}

struct NotepadApp {
    rx: mpsc::Receiver<IpcMsg>,
    view: NotepadView,
}

impl NotepadApp {
    fn new(rx: mpsc::Receiver<IpcMsg>) -> Self {
        Self { rx, view: NotepadView::default() }
    }
}

impl eframe::App for NotepadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.view.handle_ipc(msg);
        }
        self.view.show(ctx);
    }
}
```

- [ ] **Step 3: 编译**

Run: `cargo build --manifest-path crates/egui/Cargo.toml`
Expected: 编译成功。

- [ ] **Step 4: 手动 e2e（spike #2+#4 合并验证）**

预备：确保 `~/.octopus/octopus.db` 存在（desktop 跑过一次即可）。

Run: `cargo run --manifest-path crates/egui/Cargo.toml`
手动验证：
- 窗口弹出，左侧列出已有笔记（或空）。
- 点列表切换笔记，标题/正文加载。
- 编辑正文，停 1s，看到列表 updated_at 顺序变化（防抖保存生效）。
- md 源码 `# 标题` / `**粗**` 在右栏预览正确渲染（egui_commonmark 生效）。
- 工具栏 5 按钮点击在正文末尾插入语法标记。

确认无 panic、无明显卡顿。

- [ ] **Step 5: 提交**

```bash
git add crates/egui/src/notepad_view.rs crates/egui/src/main.rs
git commit -m "feat(egui): NotepadView 列表+md编辑+预览+工具栏+防抖保存"
```

---

## Task 7: egui macOS Accessory 集成（无 Dock 图标，spike #3，有兜底）

**Files:**
- Modify: `crates/egui/Cargo.toml`（macOS target 依赖 objc2 + objc2-app-kit）
- Modify: `crates/egui/src/main.rs`（启动时 setActivationPolicy(.accessory)）
- Modify: `crates/egui/src/macos.rs`（Create：accessory 设置 + show/focus）

> spec §3.5：egui 进程作 Accessory agent（无 Dock 图标），主应用独占 Dock。**搞不定则接受 2 个 Dock 图标**（功能不阻断）。

- [ ] **Step 1: Cargo.toml 加 macOS 依赖**

`crates/egui/Cargo.toml` 末尾追加：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-app-kit = { version = "0.3", features = ["NSApplication", "NSRunningApplication"] }
```

- [ ] **Step 2: 写 macos.rs（accessory + 激活）**

```rust
//! macOS 集成：egui 进程设 Accessory 激活策略（无 Dock 图标），窗口仍可 show/focus。
//! 兜底：若此处失败，egui 进程默认 Regular（2 个 Dock 图标，功能不阻断）。

#[cfg(target_os = "macos")]
pub fn set_accessory_policy() {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    let Ok(mt) = MainThreadMarker::new() else {
        log::warn!("非主线程，跳过 Accessory 设置");
        return;
    };
    let app: Retained<NSApplication> = NSApplication::sharedApplication(mt);
    unsafe {
        let _: () = msg_send![&app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory];
        let _: () = msg_send![&app, activateIgnoringOtherApps: true];
    }
    log::info!("egui 进程已设 Accessory（无 Dock 图标）");
}

#[cfg(not(target_os = "macos"))]
pub fn set_accessory_policy() {}
```

> 注：`objc2-foundation` 主线程标记需要该 crate。补依赖：在 `Cargo.toml` 的 `[target.'cfg(target_os = "macos")'.dependencies]` 加 `objc2-foundation = { version = "0.3", features = ["NSThread"] }`。`NSApplicationActivationPolicy` 常量路径依 objc2-app-kit 版本可能为枚举值或常量；若编译报路径不对，按编译器提示调整（属 spike #3 验证点，符合兜底条款）。

- [ ] **Step 3: main.rs 启动时调用**

在 `main()` 内、`eframe::run_native` 之前加：

```rust
    #[cfg(target_os = "macos")]
    {
        mod macos;
        macos::set_accessory_policy();
    }
```

（实际把 `mod macos;` 提到文件顶部 mod 区，此处仅展示调用点。）

- [ ] **Step 4: 编译（macOS）**

Run: `cargo build --manifest-path crates/egui/Cargo.toml`
Expected: 编译成功（若 objc2 API 路径不对，按编译器调整——这是 spike #3 的预期验证）。

- [ ] **Step 5: 手动验证**

Run: `cargo run --manifest-path crates/egui/Cargo.toml`
验证：egui 窗口弹出，**Dock 不出现第二个图标**（只有主应用图标，或运行 egui 时主应用没开则无图标）。窗口能正常获焦。
若 Accessory 配置失败 → egui 进程 Regular，Dock 多一图标 → **记录为兜底可接受**，继续。

- [ ] **Step 6: 提交**

```bash
git add crates/egui/Cargo.toml crates/egui/src/macos.rs crates/egui/src/main.rs
git commit -m "feat(egui): macOS Accessory 激活策略（无 Dock 图标，双图标兜底）"
```

---

## Task 8: desktop IPC client（spawn + 连接 + pid 存活检测）

**Files:**
- Create: `crates/desktop/src/egui_ipc.rs`
- Modify: `crates/desktop/src/main.rs`（`mod egui_ipc;`）

> client 职责：读 `~/.octopus/egui-ipc.port` → pid 存活（`kill(pid,0)`）→ 连 → 发 JSON line；连不上/pid 死 → 删 port 文件 → spawn `octopus-egui`（命令行带初始 note_id，或 spawn 后再发 open）。

- [ ] **Step 1: 写 egui_ipc.rs**

```rust
//! Tauri→egui IPC client：连本地 TCP 发 JSON line；连不上则 spawn octopus-egui。
//! 单实例锁 = port 文件 {pid,port} + pid 存活检测。

use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

/// port 文件路径：~/.octopus/egui-ipc.port（与 egui/src/ipc.rs::port_file 一致）
fn port_file() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("egui-ipc.port")
}

/// pid 是否存活（Unix kill(pid,0) 语义：返回 0 = 存活）。
fn pid_alive(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

// 跨平台 kill(pid,0)：macOS/Linux 走 libc；Windows 走 OpenProcess。
#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, sig) }
}
#[cfg(not(unix))]
unsafe fn libc_kill(_pid: i32, _sig: i32) -> i32 {
    0 // Windows：第一版不检 pid，靠 TCP 连接失败兜底
}

/// 读 port 文件。返回 (pid, port)。文件不存在/解析失败返回 None。
fn read_port_file() -> Option<(u32, u16)> {
    let text = std::fs::read_to_string(port_file()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let pid = v["pid"].as_u64()? as u32;
    let port = v["port"].as_u64()? as u16;
    Some((pid, port))
}

/// 解析 octopus-egui 二进制路径：与当前 exe 同目录（dev: target/debug；bundled: .app/Resources）。
fn egui_binary_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("octopus-egui"));
    p.set_file_name("octopus-egui");
    p
}

/// 连已运行的 egui 进程；连不上/pid 死 → spawn 新进程。
/// 关键：pid 活但连不上 = egui 启动中（bind 未就绪），此时**不删 port 文件、不重复 spawn**，
/// 仅返回 None 让调用方重试（避免误杀 live 进程 + 起 dup）。
fn ensure_running() -> Option<TcpStream> {
    if let Some((pid, port)) = read_port_file() {
        if pid_alive(pid) {
            return TcpStream::connect_timeout(
                &format!("127.0.0.1:{}", port).parse().ok()?,
                Duration::from_millis(500),
            )
            .ok(); // 连得上 → Some；启动中连不上 → None（不清理、不 spawn）
        }
        // pid 死 → 清理 stale port 文件
        let _ = std::fs::remove_file(port_file());
    }
    // 无 port 文件 / pid 死 → spawn
    spawn_egui();
    None
}

/// spawn octopus-egui（后台，不阻塞）。
fn spawn_egui() {
    let bin = egui_binary_path();
    match std::process::Command::new(&bin).spawn() {
        Ok(_) => log::info!("已 spawn octopus-egui: {}", bin.display()),
        Err(e) => log::error!("spawn octopus-egui 失败 ({}): {}", bin.display(), e),
    }
}

/// 发一条 JSON line；带最多 ~2s 的 spawn-后连接重试。
fn send(payload: serde_json::Value) {
    // 先尝试连已有进程
    for attempt in 0..20 {
        let stream = ensure_running()
            .or_else(|| {
                // 刚 spawn，轮询 port 文件 + 直连
                read_port_file().and_then(|(_pid, port)| {
                    TcpStream::connect_timeout(
                        &format!("127.0.0.1:{}", port).parse().ok()?,
                        Duration::from_millis(200),
                    )
                    .ok()
                })
            });
        if let Some(mut stream) = stream {
            let line = format!("{}\n", payload);
            if stream.write_all(line.as_bytes()).is_ok() {
                let _ = stream.flush();
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        let _ = attempt; // 首次循环若 None，下一轮 ensure_running 会再 spawn（幂等：已 spawn 则读 port）
    }
    log::warn!("IPC 发送失败（egui 进程未就绪）: {}", payload);
}

/// 打开并选中笔记（OCR/ASR→notepad 场景）。
pub fn open_note(note_id: i64) {
    send(json!({"type":"open","note_id":note_id}));
}

/// 通知 egui 刷新列表（Tauri 侧写笔记后）。
pub fn notes_changed() {
    send(json!({"type":"notes_changed"}));
}

/// 托盘唤起：show + focus。
pub fn show() {
    send(json!({"type":"show"}));
}
```

- [ ] **Step 2: 写 client 单测（mock port 文件 + 真 server loopback）**

在 `egui_ipc.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::io::Read;

    #[test]
    fn send_delivers_json_line_to_server() {
        // 起 mock server，写 port 文件指向它
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _ = std::fs::write(
            port_file(),
            serde_json::json!({"pid": std::process::id(), "port": port}).to_string(),
        );
        listener.set_nonblocking(true).unwrap();

        send(json!({"type":"notes_changed"}));

        // accept 一条连接读一行
        let (mut s, _) = listener.accept().unwrap();
        s.set_nonblocking(false).unwrap();
        let mut buf = [0u8; 128];
        let n = s.read(&mut buf).unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        assert!(line.contains("\"type\":\"notes_changed\""), "server 应收到消息: {}", line);

        let _ = std::fs::remove_file(port_file());
    }
}
```

> 注：`port_file()` 全局路径，测试间串行；该测试单独跑应过。

- [ ] **Step 3: main.rs 加 mod**

`crates/desktop/src/main.rs` mod 区（约第 25 行 `mod note_commands;` 附近）加：

```rust
mod egui_ipc;
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test --manifest-path crates/desktop/Cargo.toml egui_ipc`
Expected: 测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/egui_ipc.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): egui IPC client（spawn + 连接 + pid 存活检测）"
```

---

## Task 9: Tauri 侧改造（notepad_window→IPC、删 12 死命令、重写 save_ocr/transcription）

**Files:**
- Modify: `crates/desktop/src/notepad_window.rs`（open_notepad / open_notepad_with_note 改 IPC；删 webview + PENDING + on_notepad_closed）
- Modify: `crates/desktop/src/note_commands.rs`（删 12 个命令；重写 save_ocr_to_note / save_transcription_to_note 用 type='text' + IPC notes_changed）
- Modify: `crates/desktop/src/main.rs`（invoke_handler 删 12 行；删 notepad_window destroy 分支）
- Modify: `crates/desktop/src/tray.rs`（记事本菜单 → open_notepad，仍走 IPC）

- [ ] **Step 1: 重写 notepad_window.rs（纯 IPC 启动器）**

整个 `crates/desktop/src/notepad_window.rs` 替换为：

```rust
//! 记事本入口：改走 egui 独立进程（本地 TCP IPC），不再建 webview。
//! open_notepad / open_notepad_with_note 调 egui_ipc（连不上则 spawn）。

/// 打开记事本（egui 进程：已运行则 show，未运行则 spawn）。
#[tauri::command]
pub fn open_notepad(_app_handle: tauri::AppHandle) {
    crate::egui_ipc::show();
}

/// 打开记事本并选中指定笔记（OCR 识别结果存笔记后调用）。
#[tauri::command]
pub fn open_notepad_with_note(_app_handle: tauri::AppHandle, note_id: i64) {
    crate::egui_ipc::open_note(note_id);
}
```

> `get_pending_note` / `on_notepad_closed` 删除（egui 通过 IPC open 拿 note_id；无 webview destroy 事件）。

- [ ] **Step 2: 重写 note_commands.rs（仅留 save_ocr / save_transcription）**

整个 `crates/desktop/src/note_commands.rs` 替换为：

```rust
//! 记事本集成入口：识别结果 → 笔记。
//! 其余 CRUD（list/get/create/update/delete/toggle/export/import/image）已废弃——
//! egui 进程直连 octopus_notepad::store，不走 invoke。仅留这 2 个 Tauri 命令
//! 供 OCR/ASR 识别后调用：写笔记（type='text'）+ IPC 通知 egui 刷新。

use octopus_notepad::{NoteSource, NoteType};

/// 语音结果 → 新建笔记（type='text'，纯文本无 <p> 包裹）+ IPC 通知 egui。
///
/// IPC 的 send() 带最多 ~2s spawn 重试，同步调用会阻塞 async 命令线程；
/// 故写库后立即返回 id，IPC 通知 fire-and-forget 到独立线程（两条消息同线程保序）。
#[tauri::command]
pub async fn save_transcription_to_note(
    transcription_id: i64,
    text: String,
    _app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(
        NoteSource::Asr,
        Some(transcription_id),
        &text,
        NoteType::Text,
    )
    .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        crate::egui_ipc::notes_changed();
        crate::egui_ipc::open_note(id);
    });
    Ok(id)
}

/// OCR 结果 → 新建笔记（type='text'）+ IPC 通知 egui（fire-and-forget）。
#[tauri::command]
pub async fn save_ocr_to_note(text: String, _app_handle: tauri::AppHandle) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(NoteSource::Ocr, None, &text, NoteType::Text)
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        crate::egui_ipc::notes_changed();
        crate::egui_ipc::open_note(id);
    });
    Ok(id)
}
```

> 说明：原 `save_*_to_note` 调用方（`HistoryPanel.tsx`、`ImagePreview/index.tsx`）传 text 并期望返回 id + 打开笔记。这里 save 后自动 IPC `open_note(id)`（替代原 `emit("notepad://changed")` + 前端再 `open_notepad_with_note`），egui 收到 open 直接选中。ImagePreview 那段前端 `await save_ocr_to_note` 后又 `await open_notepad_with_note` 会变成双 open（幂等无害），Task 11 前端清理时精简。

- [ ] **Step 3: main.rs invoke_handler 删 12 行**

`crates/desktop/src/main.rs` 第 250–266 行的 invoke_handler 数组，删掉这 12 行：

```text
note_commands::list_notes,
note_commands::count_notes,
note_commands::get_note,
note_commands::create_note,
note_commands::update_note,
note_commands::delete_notes,
note_commands::toggle_note_pinned,
note_commands::toggle_note_favorite,
note_commands::export_note,
note_commands::import_note_from_file,
note_commands::get_note_image,
note_commands::insert_note_image,
notepad_window::get_pending_note,    // 也删（已移除）
```

**保留**这 3 行：
```text
note_commands::save_transcription_to_note,
note_commands::save_ocr_to_note,
notepad_window::open_notepad,
notepad_window::open_notepad_with_note,
```

- [ ] **Step 4: main.rs 删 notepad_window destroy 分支**

第 501–502 行 `else if label == "notepad_window" { notepad_window::on_notepad_closed(app); }` 删除（egui 进程窗口不归 Tauri 管）。

- [ ] **Step 5: tray.rs 验证记事本入口（仍调 open_notepad，现已走 IPC）**

`crates/desktop/src/tray.rs:117-119` 已是 `"notepad" => { crate::notepad_window::open_notepad(app.clone()); }`——无需改动（open_notepad 内部已改成 IPC show）。确认即可。

- [ ] **Step 6: 编译 desktop**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译成功（确认无对已删命令/字段的残留引用）。

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/src/notepad_window.rs crates/desktop/src/note_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 记事本改走 egui IPC，删 12 死命令，save_* 改 type=text+IPC"
```

---

## Task 10: 前端清理（删 Notepad 页 + notepad.ts + note 类型 + 重建 dist）

**Files:**
- Modify: `crates/desktop/frontend/src/App.tsx`（删 `case "notepad_window"` 及其 import）
- Delete: `crates/desktop/frontend/src/pages/Notepad/`（extensions.tsx / index.tsx / NoteEditor.tsx / NoteList.tsx）
- Delete: `crates/desktop/frontend/src/lib/notepad.ts`
- Delete: `crates/desktop/frontend/src/types/note.ts`（若仅被 Notepad 页/notepad.ts 引用）
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`（save_ocr 后的 `open_notepad_with_note` 调用可留可删——save_ocr 已自动 open，删之更干净）
- Build: `crates/desktop/frontend/dist/`（提交）

- [ ] **Step 1: 确认 note.ts 是否仅被 Notepad 引用**

Run: `grep -rn "types/note\|@/types/note" crates/desktop/frontend/src --include='*.ts' --include='*.tsx' | grep -v "pages/Notepad\|lib/notepad.ts"`
Expected: 若仅 Notepad 页 + notepad.ts 引用 → 可删；若有别处引用（如 Settings）→ 保留 note.ts，仅删 Notepad 页。

- [ ] **Step 2: App.tsx 删 notepad 路由**

`crates/desktop/frontend/src/App.tsx`：
- 删 `case "notepad_window": ...`（约第 55 行）及其渲染。
- 删顶部 Notepad 页的 `import`。

- [ ] **Step 3: 删 Notepad 页 + notepad.ts（+ 视情况 note.ts）**

```bash
rm -rf crates/desktop/frontend/src/pages/Notepad
rm crates/desktop/frontend/src/lib/notepad.ts
# 仅当 Step 1 确认无其他引用时：
rm crates/desktop/frontend/src/types/note.ts
```

- [ ] **Step 4: ImagePreview 精简 save_ocr 后的 open 调用（可选）**

`crates/desktop/frontend/src/pages/ImagePreview/index.tsx:287-288`：
```tsx
const noteId = await invoke<number>("save_ocr_to_note", { text });
await invoke("open_notepad_with_note", { noteId });
```
改为（save_ocr_to_note 内部已 open_note）：
```tsx
await invoke<number>("save_ocr_to_note", { text });
```
（`open_notepad_with_note` 命令仍注册可用，保留也无害；删调用更干净。）

- [ ] **Step 5: tsc 类型检查**

Run: `cd crates/desktop/frontend && npm run build`（vite build 含 tsc）
Expected: 无 TS 报错（确认无残留 import 指向已删文件）。

- [ ] **Step 6: 提交 dist + 前端**

```bash
git add crates/desktop/frontend/src crates/desktop/frontend/dist
git commit -m "chore(frontend): 移除 webview Notepad 页（迁 egui），重建 dist"
```

---

## Task 11: octopus-egui 二进制打包（bundled .app 内含）

**Files:**
- Modify: `crates/desktop/tauri.conf.json`（externalBin 或 resources）
- 可能 Modify: 构建脚本（把 target/<triple>/octopus-egui 拷到 desktop 能 resolve 的位置）

> 问题：dev（cargo run）下 `current_exe().parent()/octopus-egui` = `target/debug/octopus-egui`，存在。但 bundled `.app` 里 desktop exe 在 `MacOS/`，octopus-egui 默认不打入。需 sidecar。

- [ ] **Step 1: tauri.conf.json 加 externalBin（sidecar）**

`crates/desktop/tauri.conf.json` 的 `bundle` 加：

```json
"externalBin": ["binaries/octopus-egui"]
```

并在 `crates/desktop/binaries/` 放符号链接或构建期拷贝 `octopus-egui-<target-triple>`（Tauri sidecar 要求文件名带 `-<triple>` 后缀，如 `octopus-egui-aarch64-apple-darwin`）。

> 因 sidecar 命名 + 构建流程较繁，且「功能完整完成前不往 main」，**第一版可仅保证 dev/cargo run 路径可用**（Task 8 的 `egui_binary_path` 已覆盖 dev）。bundled 打包作为发布前收尾，本 Step 标注待发布阶段完善。

- [ ] **Step 2: 文档记录打包 TODO**

在 `docs/superpowers/plans/2026-07-01-notepad-egui.md` 本任务下注明：「bundled .app 打包 octopus-egui（Tauri externalBin sidecar）待发布阶段完善；当前 dev/cargo run 路径已验证可用」。

- [ ] **Step 3: 提交（如有配置改动）**

```bash
git add crates/desktop/tauri.conf.json docs/superpowers/plans/2026-07-01-notepad-egui.md
git commit -m "build(desktop): octopus-egui sidecar 打包配置（dev 路径已验证）"
```

---

## Task 12: 端到端手动验证（spike 收口 + 全链路）

> 全程 dev 模式：先 `cargo build`（确保 octopus-egui + desktop 都编过），再跑 desktop。

- [ ] **Step 1: 全量编译**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml && cargo build --manifest-path crates/egui/Cargo.toml`
Expected: 两个都成功，`target/debug/octopus-egui` 存在。

- [ ] **Step 2: 跑 desktop，托盘打开记事本**

Run: `cargo run --manifest-path crates/desktop/Cargo.toml`
- 托盘 → 记事本。
- 验证：octopus-egui 进程被 spawn，窗口弹出，Dock 无第二图标（Accessory 生效）或记录兜底。

- [ ] **Step 3: OCR → 记事本（端到端）**

- 截图 / 图片预览 → OCR → 存笔记。
- 验证：egui 窗口自动选中新建笔记（IPC open 生效），正文是 OCR 纯文本（type='text'）。

- [ ] **Step 4: 编辑保存 + 并发（WAL 验收）**

- egui 里编辑 md，停 1s，列表 updated_at 刷新（防抖保存）。
- 验证：同时 desktop 侧触发一次 OCR 入库（写 notes）不报 `database is locked`（WAL 生效）。

- [ ] **Step 5: 托盘唤起**

- egui 窗口最小化/失焦 → 托盘 → 记事本 → egui 窗口 show + focus（IPC show 生效）。

- [ ] **Step 6: 进程崩溃恢复**

- 手动 kill octopus-egui 进程 → 托盘 → 记事本。
- 验证：检测到 pid 死 → 删 port 文件 → spawn 新进程（egui_ipc 兜底生效）。

- [ ] **Step 7: 记录验证结果**

在本计划文件末尾「验证记录」追加每步通过/兜底结论。

---

## Task 13: 文档同步（architecture.md + 旧 spec 标注）

**Files:**
- Modify: `docs/architecture.md`（加 octopus-egui crate、进程拓扑、IPC、WAL；记事本窗口改注 egui 进程）
- Modify: `docs/superpowers/specs/2026-06-30-notepad-design.md`（顶部标注「已被 egui 方案替代」）
- Modify: `docs/superpowers/specs/2026-07-01-notepad-egui-design.md`（状态 设计中 → 已实现）

- [ ] **Step 1: architecture.md 更新**

参照 spec §3.1 拓扑图，在 architecture.md 对应 crate 清单/窗口章节：
- crate 清单加 `octopus-egui`（二进制，eframe，不依赖 tauri）。
- 窗口章节：记事本从「Tauri webview 窗口」改注为「独立 egui 进程（本地 TCP IPC 驱动）」。
- 数据层加：WAL（journal_mode=WAL + busy_timeout=5000 + synchronous=NORMAL）支持多进程并发。
- 加 IPC 协议一段（127.0.0.1 TCP + JSON line + `~/.octopus/egui-ipc.port` 单实例锁）。

- [ ] **Step 2: 旧 notepad spec 标注替代**

`docs/superpowers/specs/2026-06-30-notepad-design.md` 顶部加：

```markdown
> ⚠️ 已被 egui 方案替代（2026-07-01）。记事本迁至独立 egui 进程，见 `docs/superpowers/specs/2026-07-01-notepad-egui-design.md`。本文档保留作历史参考（webview + TipTap + content_html 方案已下线）。
```

- [ ] **Step 3: egui spec 状态置已实现**

`docs/superpowers/specs/2026-07-01-notepad-egui-design.md` 第 4 行 `状态：**设计中**` 改为 `状态：**已实现**（见 plans/2026-07-01-notepad-egui.md）`。

- [ ] **Step 4: 提交**

```bash
git add docs/architecture.md docs/superpowers/specs/2026-06-30-notepad-design.md docs/superpowers/specs/2026-07-01-notepad-egui-design.md
git commit -m "docs: 同步 egui 记事本迁移（architecture + 旧 spec 标注 + 状态已实现）"
```

---

## 验证记录

（Task 12 Step 7 在此追加每步结论：通过 / 兜底可接受 / 阻塞）

- WAL 并发：待验证
- IPC 往返：待验证
- macOS Accessory：待验证
- commonmark 预览性能：待验证
- OCR→notepad 端到端：待验证
- 托盘唤起：待验证
- 崩溃恢复：待验证

---

## Spec Coverage

| Spec section | 对应 Task |
|---|---|
| §3.2.1 WAL 迁移 | Task 1 |
| §3.2.2 notes 表重建（content_text+type） | Task 2 |
| §3.2.3 store 复用 + 连接持有 | Task 3（store 适配）+ Task 6（egui 直连） |
| §3.3 编辑器 UI（三栏+分屏+工具栏+防抖） | Task 6 |
| §3.4 IPC 协议（TCP+JSON line+port 文件） | Task 5（server）+ Task 8（client） |
| §3.5 macOS Accessory | Task 7 |
| §3.6 Tauri 侧改动（open_*→IPC、删命令、save_* 重写） | Task 9 |
| §4 数据流（OCR→notepad / 编辑保存 / 托盘唤起） | Task 6 + Task 9 + Task 12 |
| §6 测试（store/WAL/IPC 单测 + e2e） | Task 1/3/5/8/12 |
| §7 spike 4 项 | Task 1（WAL）+ Task 5/8（IPC）+ Task 7（Accessory）+ Task 6（commonmark） |
| §10 文档同步 | Task 13 |
