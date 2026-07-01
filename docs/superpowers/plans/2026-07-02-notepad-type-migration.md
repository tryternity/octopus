# 记事本 type 迁移 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 webview（TipTap）现役记事本上采纳 egui 分支的 `content_text + type` 表结构（保留 `content_html`），把 `type` 放开到 text/markdown/html 三态，提供安全迁移与三类型编辑器。

**Architecture:** DB 层加 `type` 列（v9→v10 幂等 ALTER，不丢数据）；后端 `NoteType` enum + `Note.note_type`，store 按 type 分发抽取（仅 html 抽取纯文本）；IPC create/update 透传 type；前端按 `note_type` 分发 TipTap/textarea/markdown 编辑器，新建时选 type、已建锁定。

**Tech Stack:** Rust（rusqlite, serde）、React + TypeScript + TipTap、`marked`（md 预览）、Tauri IPC、SQLite FTS5。

**Spec:** `docs/superpowers/specs/2026-07-02-notepad-type-migration-design.md`

**关键约束（来自 CLAUDE.md / 记忆）：**
- worktree 内 cargo/git 必须显式指 worktree 路径（`--manifest-path` / `-C` / 绝对路径）—— worktree cwd 陷阱。
- 前端改完必须 `npm run build` 并提交 `crates/desktop/dist/*`（dist 已跟踪）。
- `config/` 用绝对路径 `~/.octopus/`（本任务不涉及 config）。

---

## Task 1: `NoteType` enum + `Note.note_type` 字段

**Files:**
- Modify: `crates/notepad/src/model.rs`
- Modify: `crates/notepad/src/lib.rs`

- [ ] **Step 1: 写失败测试** — 在 `model.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn note_type_roundtrip() {
        for t in [NoteType::Html, NoteType::Text, NoteType::Markdown] {
            assert_eq!(NoteType::from_str(t.as_str()), t);
        }
    }

    #[test]
    fn note_type_from_unknown_defaults_html() {
        // 未知值 → Html（保守：历史/异常值保持富文本不丢格式）
        assert_eq!(NoteType::from_str("???"), NoteType::Html);
        assert_eq!(NoteType::from_str(""), NoteType::Html);
    }
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad note_type` → 编译失败（`NoteType` 未定义）。

- [ ] **Step 3: 实现 NoteType** — 在 `model.rs` 的 `NoteSource` impl 之后、`Note` struct 之前插入：

```rust
/// 笔记内容格式（DB `notes.type` 列）。
/// - `Html`：TipTap 富文本（content_html 存原始，content_text 存抽取纯文本）。
/// - `Text`：纯文本（content_text 存原文，content_html 空）。
/// - `Markdown`：md 源码（content_text 存源码，content_html 空，预览端渲染）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Html,
    Text,
    Markdown,
}

impl NoteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Html => "html",
            NoteType::Text => "text",
            NoteType::Markdown => "markdown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => NoteType::Text,
            "markdown" => NoteType::Markdown,
            // "html" 及未知值 → Html（历史数据 DEFAULT 'html'，容错偏富文本）
            _ => NoteType::Html,
        }
    }
}
```

- [ ] **Step 4: Note struct 加字段** — 把 `Note` struct 改为（在 `content_text` 后加 `note_type`）：

```rust
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub note_type: NoteType,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

- [ ] **Step 5: lib.rs 导出 NoteType** — `pub use model::{Note, NoteFilter, NoteSource};` 改为：

```rust
pub use model::{Note, NoteFilter, NoteSource, NoteType};
```

- [ ] **Step 6: 跑测试确认通过** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad` → NoteType 测试 PASS（store 测试此时可能编译失败，Task 4 修，本步只看 model 测试通过即可；若 store 编译错阻碍，临时 `cargo test -p octopus-notepad --lib model::tests`）。

- [ ] **Step 7: 提交** — `git add crates/notepad/src/model.rs crates/notepad/src/lib.rs && git commit -m "feat(notepad): NoteType enum (html/text/markdown) + Note.note_type 字段"`

---

## Task 2: schema `db.sql` notes 加 `type` 列

**Files:**
- Modify: `crates/infra/src/db.sql`

- [ ] **Step 1: 改 notes 建表** — 把 `db.sql` 中 notes 建表改为（加 `type` 列 + 更新注释）：

```sql
-- ── 记事本（notes 表）─────────────────────────────────────────────────────
-- 内容收集箱：ASR/OCR/剪贴板结果一键存入 + 富文本/markdown/纯文本整理。
-- type: 'html'(TipTap 富文本，默认) | 'text'(纯文本) | 'markdown'(md 源码)。
-- content_html = 富文本原始（仅 type=html）；content_text = 纯文本/md源码/html抽取（FTS + 列表预览）。
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',
    content_html  TEXT    NOT NULL DEFAULT '',
    type          TEXT    NOT NULL DEFAULT 'html',
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);
```

> FTS5 表 + 触发器不变（仍索引 `content_text`，`type` 不进 FTS）。

- [ ] **Step 2: 提交** — `git add crates/infra/src/db.sql && git commit -m "feat(infra): notes 表加 type 列 (html/text/markdown)"`

---

## Task 3: v9→v10 迁移（幂等 ALTER ADD type）

**Files:**
- Modify: `crates/infra/src/db.rs`
- Test: `crates/infra/src/db.rs`（同文件 `#[cfg(test)]`）

- [ ] **Step 1: 写失败测试** — 在 db.rs 测试模块追加（参考 egui 分支 `migrate_v9_to_v10_rebuilds_notes_schema` 结构，但断言**保留数据**）：

```rust
    #[test]
    fn migrate_v9_to_v10_adds_type_column_keeps_data() {
        let dir = std::env::temp_dir().join(format!("octopus-type-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mig.db");
        let conn = Connection::open(&path).unwrap();
        apply_wal_pragmas(&conn);
        // 模拟旧 v9 库：notes 有 content_html/content_text，无 type
        conn.execute_batch(
            "CREATE TABLE notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT,
                content_html TEXT NOT NULL DEFAULT '', content_text TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT 'manual', source_ref_id INTEGER,
                is_pinned INTEGER NOT NULL DEFAULT 0, is_favorite INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
             INSERT INTO notes (title, content_html, content_text, source, created_at, updated_at)
                VALUES ('旧富文本', '<p>你好</p>', '你好', 'manual', '2026-01-01 00:00:00', '2026-01-01 00:00:00');
             PRAGMA user_version = 9;",
        ).unwrap();

        init_schema(&conn).unwrap();

        // type 列存在
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(notes)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"type".to_string()), "应有 type 列");
        assert!(cols.contains(&"content_html".to_string()), "content_html 应保留");

        // 旧数据保留，type 默认 html
        let row: (String, String, String) = conn.query_row(
            "SELECT content_html, content_text, type FROM notes WHERE title='旧富文本'", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(row.0, "<p>你好</p>");
        assert_eq!(row.1, "你好");
        assert_eq!(row.2, "html", "历史笔记默认 type=html");

        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 10);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_v9_to_v10_is_idempotent() {
        // 重复 init_schema 不应崩溃（type 列已存在时跳过 ALTER）
        let dir = std::env::temp_dir().join(format!("octopus-type-mig-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("mig.db")).unwrap();
        apply_wal_pragmas(&conn);
        conn.execute_batch(
            "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT, content_html TEXT DEFAULT '', content_text TEXT DEFAULT '', type TEXT DEFAULT 'html', source TEXT DEFAULT 'manual', source_ref_id INTEGER, is_pinned INTEGER DEFAULT 0, is_favorite INTEGER DEFAULT 0, created_at TEXT, updated_at TEXT);
             PRAGMA user_version = 9;",
        ).unwrap();
        // type 列已存在 → init_schema 应跳过 ALTER，不报 duplicate column
        init_schema(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra migrate_v9_to_v10` → 失败（无 v9→v10 分支，user_version 停在 9）。

- [ ] **Step 3: 实现迁移** — 在 db.rs `init_schema` 的 `} else if v == 8 { ... }` 分支之后、函数结束前追加：

```rust
    } else if v == 9 {
        // v9 → v10：notes 加 type 列（html/text/markdown）。
        // 幂等：v8→v9 重跑 INIT_SQL 建 notes 时已含 type（db.sql 已改），此处先查列存在再 ALTER。
        let has_type: bool = conn
            .prepare("PRAGMA table_info(notes)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|c| c == "type");
        if !has_type {
            conn.execute(
                "ALTER TABLE notes ADD COLUMN type TEXT NOT NULL DEFAULT 'html'",
                [],
            )
            .context("v9→v10: ALTER notes ADD type")?;
            log::info!("DB migrated to v10: notes.type 列已加（历史笔记默认 html）");
        }
        conn.execute("PRAGMA user_version = 10", [])?;
    }
```

- [ ] **Step 4: v0/v1 新库直跳 v10** — 把 v0/v1 分支（`if v < 2 { ... conn.execute("PRAGMA user_version = 9", [])?; }`）的 `= 9` 改为 `= 10`（INIT_SQL 建的 notes 已带 type，新库直接 v10）。

- [ ] **Step 5: 更新顶部 version 流转注释** — 在 init_schema 文档注释的版本流转说明里补一行 `/// - v9 → v10: notes 加 type 列（ALTER ADD，幂等）`，并把 v0/v1 注释里的 → v9 改 → v10。

- [ ] **Step 6: 跑测试确认通过** — `cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra` → migrate_v9_to_v10_adds_type / _is_idempotent 均 PASS，且不破坏现有 db 测试。

- [ ] **Step 7: 提交** — `git add crates/infra/src/db.rs && git commit -m "feat(infra): v9→v10 迁移 notes 加 type 列（幂等 ALTER，保留历史数据）"`

---

## Task 4: `store.rs` 适配 type（create/update/row/SELECT + 分发抽取）

**Files:**
- Modify: `crates/notepad/src/store.rs`

- [ ] **Step 1: 写失败测试** — 在 store.rs 测试模块追加（覆盖三类型 create + 抽取分发 + update）：

```rust
    #[test]
    fn create_note_html_extracts_text() {
        // html 类型：content_text 由 html 抽取
        let (conn, _dir) = test_db();  // 见下：测试 helper
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>你好</p><p>世界</p>", NoteType::Html).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Html);
        assert_eq!(n.content_html, "<p>你好</p><p>世界</p>");
        assert_eq!(n.content_text, "你好\n世界");  // extract_text 抽取
    }

    #[test]
    fn create_note_text_stores_raw_no_extract() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "纯文本 <不抽取>", NoteType::Text).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Text);
        assert_eq!(n.content_text, "纯文本 <不抽取>");  // 原文，不经抽取
        assert_eq!(n.content_html, "");                 // text 无 html
    }

    #[test]
    fn create_note_markdown_stores_source() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "# 标题\n正文", NoteType::Markdown).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Markdown);
        assert_eq!(n.content_text, "# 标题\n正文");
        assert_eq!(n.content_html, "");
    }

    #[test]
    fn update_note_by_type() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "x", NoteType::Text).unwrap();
        update_note_at(&conn, id, "标题", "<p>新</p>", NoteType::Html).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.title.as_deref(), Some("标题"));
        assert_eq!(n.note_type, NoteType::Html);
        assert_eq!(n.content_text, "新");
    }
```

> 若 store.rs 已有 `test_db()` helper 则复用；若无，在测试模块加：
> ```rust
> fn test_db() -> (rusqlite::Connection, std::path::PathBuf) {
>     let dir = std::env::temp_dir().join(format!("octopus-store-test-{}", std::process::id()));
>     let _ = std::fs::remove_dir_all(&dir);
>     std::fs::create_dir_all(&dir).unwrap();
>     let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
>     octopus_infra::db::init_for_test(&conn);  // 见 Step 4 备注
>     (conn, dir)
> }
> ```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad create_note` → 编译失败（签名不匹配）。

- [ ] **Step 3: 改 create 签名 + 分发抽取** — 替换 `create_note` / `create_note_at`：

```rust
/// 新建笔记。type=Html 时 content_text 由 body(html) 抽取；text/markdown 时 content_text=body 原文。
pub fn create_note(
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    octopus_infra::db::with_db(|conn| create_note_at(conn, source, source_ref_id, body, note_type))
}

pub fn create_note_at(
    conn: &Connection,
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    let (content_html, content_text) = split_body(body, note_type);
    let now = iso_now();
    conn.execute(
        "INSERT INTO notes (title, content_text, content_html, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at)
         VALUES (NULL, ?, ?, ?, ?, ?, 0, 0, ?, ?)",
        params![content_text, content_html, note_type.as_str(), source.as_str(), source_ref_id, now, now],
    )
    .context("insert note")?;
    Ok(conn.last_insert_rowid())
}

/// 按 type 拆 body → (content_html, content_text)。
/// Html：html 存原始，text 存抽取纯文本。Text/Markdown：text 存原文/源码，html 空。
fn split_body(body: &str, note_type: NoteType) -> (String, String) {
    match note_type {
        NoteType::Html => (body.to_string(), extract_text(body)),
        NoteType::Text | NoteType::Markdown => (String::new(), body.to_string()),
    }
}
```

- [ ] **Step 4: 改 update 签名** — 替换 `update_note` / `update_note_at`：

```rust
pub fn update_note(id: i64, title: &str, body: &str, note_type: NoteType) -> Result<()> {
    octopus_infra::db::with_db(|conn| update_note_at(conn, id, title, body, note_type))
}

pub fn update_note_at(conn: &Connection, id: i64, title: &str, body: &str, note_type: NoteType) -> Result<()> {
    let (content_html, content_text) = split_body(body, note_type);
    let title_db: Option<&str> = if title.trim().is_empty() { None } else { Some(title) };
    conn.execute(
        "UPDATE notes SET title = ?, content_text = ?, content_html = ?, type = ?, updated_at = ? WHERE id = ?",
        params![title_db, content_text, content_html, note_type.as_str(), iso_now(), id],
    )?;
    Ok(())
}
```

- [ ] **Step 5: row_to_note + 3 处 SELECT 加 type** — `row_to_note` 改为（SELECT 多一列 `type`，索引顺移）：

```rust
fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let source_str: String = row.get(4)?;
    let type_str: String = row.get(10)?;
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        content_html: row.get(2)?,
        content_text: row.get(3)?,
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

把 3 处 SELECT 列表（`list_notes_at` 的 sql、`query_with_search` 的两个 sql、`get_note_at` 的 prepare）统一在 `updated_at` 后加 `, type`：
- `list_notes_at`: `... is_pinned, is_favorite, created_at, updated_at, type FROM notes ...`
- `query_with_search` LIKE 分支: `... updated_at, type FROM notes ...`
- `query_with_search` FTS 分支: `... n.updated_at, n.type FROM notes_fts ...`
- `get_note_at`: `... updated_at, type FROM notes WHERE id = ?`

- [ ] **Step 6: import NoteType** — store.rs 顶部 `use` 加 `NoteType`（如 `use crate::model::{Note, NoteFilter, NoteSource, NoteType};` 或现有 import 风格）。

- [ ] **Step 7: 跑测试确认通过** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad` → 全 PASS。

> **备注（test_db helper）**：若 `infra::db` 无 `init_for_test` 公开入口，测试 helper 改为直接执行建表 SQL：在 `test_db()` 内 `conn.execute_batch(include_str!("../../infra/src/db.sql 的 notes 部分"))`。实现时按 store.rs 现有测试模式对齐（读现有 store 测试怎么建临时库，照搬）。

- [ ] **Step 8: 提交** — `git add crates/notepad/src/store.rs && git commit -m "feat(notepad): store 按 type 分发 create/update/读取（html 抽取，text/md 直存）"`

---

## Task 5: clipboard 剪贴板存入改调 notepad（type=text）

**Files:**
- Modify: `crates/clipboard/src/store.rs`（约 :976）
- Modify: `crates/clipboard/Cargo.toml`（加 octopus-notepad 依赖，若未有）

- [ ] **Step 1: 读现状** — 读 `clipboard/src/store.rs:960-990` 确认当前 INSERT notes 的完整上下文（标题来源、source 值、是否事务内）。

- [ ] **Step 2: 改调 notepad 统一入口** — 把直写 SQL 的 INSERT 替换为：

```rust
use octopus_notepad::{NoteSource, NoteType};
// ...
let id = octopus_notepad::store::create_note_at(
    conn,
    NoteSource::Clipboard,
    None,
    &text,            // 剪贴板纯文本
    NoteType::Text,   // 剪贴板来源固定纯文本
)?;
```

> 保留原函数的 conn 传递（若原代码在 `with_db` 闭包内，调 `create_note_at(conn, ...)` 即可）。删除原 `extract_text` 调用（notepad 内部按 type=text 直存，无需抽取）。

- [ ] **Step 3: Cargo.toml 加依赖** — 若 `crates/clipboard/Cargo.toml` 未列 `octopus-notepad`，加 `octopus-notepad = { path = "../notepad" }`。

- [ ] **Step 4: 编译 + 测试** — `cargo build --manifest-path crates/clipboard/Cargo.toml -p octopus-clipboard` 通过；跑 clipboard 现有测试不破坏。

- [ ] **Step 5: 提交** — `git add crates/clipboard/src/store.rs crates/clipboard/Cargo.toml && git commit -m "refactor(clipboard): 剪贴板存笔记改调 notepad create_note_at (type=text 统一入口)"`

---

## Task 6: IPC `note_commands.rs` 透传 type

**Files:**
- Modify: `crates/desktop/src/note_commands.rs`

- [ ] **Step 1: create_note / update_note 命令加参数** —

```rust
use octopus_notepad::{Note, NoteFilter, NoteSource, NoteType};

#[tauri::command]
pub async fn create_note(
    source: String,
    source_ref_id: Option<i64>,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::create_note_at(
            conn,
            NoteSource::from_str(&source),
            source_ref_id,
            &body,
            NoteType::from_str(&note_type),
        )
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

#[tauri::command]
pub async fn update_note(
    id: i64,
    title: String,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::update_note_at(conn, id, &title, &body, NoteType::from_str(&note_type))
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}
```

- [ ] **Step 2: save_transcription_to_note / save_ocr_to_note 固定 type=text** — 找到这两个命令内调 `create_note_at` / `<p>` 包裹处，改为传 `NoteType::Text`（ASR/OCR 是纯文本来源，不再 `<p>` 包裹成 html）：

```rust
// 原：let html = format!("<p>{}</p>", text); create_note_at(conn, Asr, id, &html)
// 改：
octopus_notepad::store::create_note_at(conn, NoteSource::Asr, transcription_id, &text, NoteType::Text)?;
// OCR 同理：NoteSource::Ocr, NoteType::Text
```

- [ ] **Step 3: 检查 invoke_handler 注册** — create_note/update_note 签名变了，但命令名不变，`invoke_handler!` 注册处无需改（参数由前端按名传）。

- [ ] **Step 4: 编译** — `cargo build --manifest-path crates/desktop/Cargo.toml -p octopus-desktop` 通过。

- [ ] **Step 5: 提交** — `git add crates/desktop/src/note_commands.rs && git commit -m "feat(desktop): create/update_note IPC 透传 type；ASR/OCR 存入固定 type=text"`

---

## Task 7: 后端整体编译 + 测试收口

- [ ] **Step 1: workspace 编译** — `cargo build --manifest-path Cargo.toml` 全 workspace 通过。

- [ ] **Step 2: workspace 测试** — `cargo test --manifest-path Cargo.toml -p octopus-notepad -p octopus-infra -p octopus-clipboard` 全 PASS。

- [ ] **Step 3: 修复回归** — 若其他 crate 因 Note 字段/签名变更编译失败（如 capx/server 引用 Note），按编译错误逐一适配（grep `create_note`/`update_note`/`Note {` 调用点）。

- [ ] **Step 4: 提交（若有修复）** — `git add -A && git commit -m "fix: 适配 NoteType 透传的下游调用点"`

---

## Task 8: 前端类型 + IPC 封装

**Files:**
- Modify: `crates/desktop/frontend/src/types/note.ts`
- Modify: `crates/desktop/frontend/src/lib/notepad.ts`

- [ ] **Step 1: types/note.ts 加 NoteType** —

```ts
export type NoteSource = "asr" | "ocr" | "clipboard" | "manual";
export type NoteType = "html" | "text" | "markdown";

export interface Note {
  id: number;
  title: string | null;
  content_text: string;
  content_html: string;
  note_type: NoteType;
  source: NoteSource;
  source_ref_id: number | null;
  is_pinned: boolean;
  is_favorite: boolean;
  created_at: string;
  updated_at: string;
}
// NoteListParams 不变
```

- [ ] **Step 2: lib/notepad.ts create/update 加 noteType** —

```ts
import type { Note, NoteListParams, NoteSource, NoteType } from "@/types/note";

export const createNote = (
  source: NoteSource,
  sourceRefId: number | null,
  body: string,
  noteType: NoteType,
) => invoke<number>("create_note", { source, sourceRefId, body, noteType });

export const updateNote = (
  id: number,
  title: string,
  body: string,
  noteType: NoteType,
) => invoke<void>("update_note", { id, title, body, noteType });
```

> 其余导出（list/count/get/delete/pin/favorite/export/import/image）不变。

- [ ] **Step 3: 提交** — `git add crates/desktop/frontend/src/types/note.ts crates/desktop/frontend/src/lib/notepad.ts && git commit -m "feat(frontend): NoteType 类型 + createNote/updateNote 透传 noteType"`

---

## Task 9: `MarkdownEditor` 组件 + marked 依赖

**Files:**
- Create: `crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx`
- Modify: `crates/desktop/frontend/package.json`

- [ ] **Step 1: 加 marked 依赖** — `cd crates/desktop/frontend && npm install marked`，确认 `package.json` 出现 `"marked"`。

- [ ] **Step 2: 创建 MarkdownEditor.tsx** —

```tsx
import { useState, useMemo } from "react";
import { marked } from "marked";
import {
  Bold, Italic, Heading1, List, Code, Link as LinkIcon, Quote,
} from "lucide-react";

interface Props {
  value: string;
  onChange: (md: string) => void;
}

/** markdown 编辑器：左源码 textarea + 工具栏，右可折叠预览（marked 渲染）。 */
export default function MarkdownEditor({ value, onChange }: Props) {
  const [showPreview, setShowPreview] = useState(true);

  const html = useMemo(() => marked.parse(value || "", { async: false }) as string, [value]);

  // 在 textarea 选区/光标处插入语法
  const wrap = (before: string, after: string = before) => {
    const ta = document.getElementById("md-textarea") as HTMLTextAreaElement | null;
    if (!ta) return;
    const { selectionStart: s, selectionEnd: e } = ta;
    const sel = value.slice(s, e);
    const next = value.slice(0, s) + before + sel + after + value.slice(e);
    onChange(next);
    requestAnimationFrame(() => {
      ta.focus();
      ta.selectionStart = s + before.length;
      ta.selectionEnd = e + before.length;
    });
  };

  const linePrefix = (prefix: string) => {
    const ta = document.getElementById("md-textarea") as HTMLTextAreaElement | null;
    if (!ta) return;
    const s = ta.selectionStart;
    const lineStart = value.lastIndexOf("\n", s - 1) + 1;
    const next = value.slice(0, lineStart) + prefix + value.slice(lineStart);
    onChange(next);
  };

  const tools = [
    { icon: Heading1, title: "标题", onClick: () => linePrefix("# ") },
    { icon: Bold, title: "粗体", onClick: () => wrap("**") },
    { icon: Italic, title: "斜体", onClick: () => wrap("*") },
    { icon: List, title: "列表", onClick: () => linePrefix("- ") },
    { icon: Quote, title: "引用", onClick: () => linePrefix("> ") },
    { icon: Code, title: "代码", onClick: () => wrap("`") },
    { icon: LinkIcon, title: "链接", onClick: () => {
        const url = prompt("链接 URL"); if (url) wrap("[", `](${url})`); } },
  ];

  return (
    <div className="flex-1 flex flex-col">
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border">
        {tools.map(({ icon: Icon, title, onClick }, i) => (
          <button key={i} title={title} onClick={onClick}
            className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
            <Icon className="w-4 h-4" />
          </button>
        ))}
        <button onClick={() => setShowPreview((v) => !v)}
          className="ml-auto px-2 py-1 text-xs rounded hover:bg-accent text-muted-foreground">
          {showPreview ? "隐藏预览" : "显示预览"}
        </button>
      </div>
      <div className={`flex-1 flex ${showPreview ? "flex-row" : "flex-col"} overflow-hidden`}>
        <textarea
          id="md-textarea"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className={`flex-1 p-4 font-mono text-sm bg-transparent resize-none focus:outline-none border-0 ${showPreview ? "border-r border-border" : ""}`}
          placeholder="输入 markdown..."
        />
        {showPreview && (
          <div className="flex-1 overflow-y-auto px-4 py-2 prose prose-sm max-w-none"
               dangerouslySetInnerHTML={{ __html: html }} />
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx crates/desktop/frontend/package.json crates/desktop/frontend/package-lock.json && git commit -m "feat(frontend): MarkdownEditor 组件（源码+工具栏+marked 可折叠预览）"`

---

## Task 10: `NoteEditor` 按 type 分发编辑器

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx`

- [ ] **Step 1: doSave 透传 type** — 把 `doSave` 改为按当前 note 的 type 调用：

```tsx
  const doSave = useCallback(
    (body: string) => {
      const id = currentId.current;
      if (id == null || !note) return;
      updateNote(id, title, body, note.note_type).catch(console.error);
    },
    [title, note],
  );
```

标题 debounce 保存同理：`updateNote(noteId, title, editor.getHTML(), note.note_type)`。

- [ ] **Step 2: 编辑区分发** — 在 return 的编辑区（`{/* 编辑器 */}` 处）按 `note.note_type` 分发。保留现有 TipTap 工具栏仅在 html 时显示；text/markdown 用各自编辑器：

```tsx
      {/* 编辑区：按 type 分发 */}
      <div className="flex-1 overflow-hidden flex flex-col">
        {note.note_type === "html" && (
          <>
            {/* 现有 TipTap 工具栏 + EditorContent 保持原样 */}
            <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border flex-wrap">
              {tools.map(({ icon: Icon, title, onClick }, i) => ( /* ... 原样 ... */ ))}
              <div className="ml-auto flex items-center gap-0.5">{/* 导入/导出/收藏/置顶 原样 */}</div>
            </div>
            <input /* 标题 input 原样 */ />
            <div className="flex-1 overflow-y-auto px-4 pb-4">
              <div className="prose prose-sm max-w-none [&_img]:max-w-full">
                <EditorContent editor={editor} />
              </div>
            </div>
          </>
        )}
        {note.note_type === "text" && (
          <TextEditor note={note} title={title} onTitle={setTitle} onSave={doSave} />
        )}
        {note.note_type === "markdown" && (
          <MarkdownEditorOuter note={note} title={title} onTitle={setTitle} onSave={doSave} />
        )}
      </div>
```

> 收藏/置顶/导出按钮在 text/markdown 也需要：抽出公共 Header 组件或在每个分支重复。为控制 scope，建议把标题 + 收藏/置顶/导出抽成 `NoteHeader` 子组件（接收 note + setters），三种编辑器共用。实现时按现有结构重构。

- [ ] **Step 3: 内联 TextEditor（纯 textarea）** — 在 NoteEditor.tsx 内或新建 `TextEditor.tsx`：

```tsx
function TextEditor({ note, title, onTitle, onSave }: EditorProps) {
  const [text, setText] = useState(note.content_text);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => setText(note.content_text), [note.id]);  // 切换笔记重置
  const onChange = (v: string) => {
    setText(v);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => onSave(v), 800);
  };
  return (
    <>
      <NoteHeader note={note} title={title} onTitle={onTitle} />
      <textarea
        value={text}
        onChange={(e) => onChange(e.target.value)}
        className="flex-1 p-4 font-mono text-sm bg-transparent resize-none focus:outline-none border-0"
        placeholder="输入纯文本..."
      />
    </>
  );
}
```

- [ ] **Step 4: MarkdownEditor 接入（外层包 debounce + header）** —

```tsx
function MarkdownEditorOuter({ note, title, onTitle, onSave }: EditorProps) {
  const [md, setMd] = useState(note.content_text);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => setMd(note.content_text), [note.id]);
  const onChange = (v: string) => {
    setMd(v);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => onSave(v), 800);
  };
  return (
    <>
      <NoteHeader note={note} title={title} onTitle={onTitle} />
      <MarkdownEditor value={md} onChange={onChange} />
    </>
  );
}
```

> `EditorProps = { note: Note; title: string; onTitle: (t: string) => void; onSave: (body: string) => void }`。`NoteHeader` 抽出标题 input + 收藏/置顶/导出按钮（从现有 html 分支搬出，三种复用）。

- [ ] **Step 5: 类型检查 + 构建** — `cd crates/desktop/frontend && npm run build` → 通过（dist 产出）。修复 TS 报错（如 doSave 依赖、未用 import）。

- [ ] **Step 6: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/ && git commit -m "feat(frontend): NoteEditor 按 note_type 分发 html/text/markdown 编辑器"`

---

## Task 11: 新建笔记 type 选择 UX

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`（新建按钮处）

- [ ] **Step 1: 读 NoteList 新建逻辑** — 确认新建按钮当前如何调 `createNote`（默认 source=manual）。

- [ ] **Step 2: 新建按钮加 type 选择** — 新建时默认 `html`（与现状一致），并提供 type 切换。实现为：新建按钮旁加一个 type 下拉（`select`），或新建按钮改为弹出三个选项（富文本/纯文本/Markdown）。推荐下拉：

```tsx
const [newType, setNewType] = useState<NoteType>("html");

const handleCreate = async () => {
  const id = await createNote("manual", null, "", newType);
  onSelect(id);
};

// UI：新建按钮 + type 下拉并排
<div className="flex gap-1">
  <select value={newType} onChange={(e) => setNewType(e.target.value as NoteType)}
          className="text-xs border border-border rounded px-1">
    <option value="html">富文本</option>
    <option value="text">纯文本</option>
    <option value="markdown">Markdown</option>
  </select>
  <button onClick={handleCreate}>新建</button>
</div>
```

> 已建笔记 type 锁定：编辑器内不提供 type 切换（NoteEditor 只读 `note.note_type`）。新建空笔记 body="" 按 type 存（html 空、text 空、md 空）。

- [ ] **Step 3: 构建检查** — `npm run build` 通过。

- [ ] **Step 4: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/NoteList.tsx && git commit -m "feat(frontend): 新建笔记可选 type（富文本/纯文本/Markdown），已建锁定"`

---

## Task 12: 列表 type 标记

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`

- [ ] **Step 1: 列表项加 type 角标** — 在每条笔记标题旁，非 html 类型显示小标记：

```tsx
{note.note_type === "markdown" && <span className="text-[10px] text-blue-500">MD</span>}
{note.note_type === "text" && <span className="text-[10px] text-muted-foreground">TXT</span>}
{/* html 不标（默认） */}
```

- [ ] **Step 2: 构建检查** — `npm run build` 通过。

- [ ] **Step 3: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/NoteList.tsx && git commit -m "feat(frontend): 列表项显示 md/txt 类型标记"`

---

## Task 13: dist rebuild + 提交

- [ ] **Step 1: 完整 rebuild** — `cd crates/desktop/frontend && npm run build`，确认 `crates/desktop/dist/assets/*` 产出新 hash 文件。

- [ ] **Step 2: 提交 dist** — `git add crates/desktop/dist/ && git commit -m "chore: rebuild dist（notepad type 三类型编辑器）"`

---

## Task 14: e2e 验证

- [ ] **Step 1: 迁移验证** — 用现有真实库（`~/.octopus/` 下）启动应用 → 记事本能打开 → 历史笔记（html）正常显示编辑 → 查 DB `SELECT type FROM notes` 全为 'html'。

- [ ] **Step 2: 三类型新建** — 新建富文本/纯文本/Markdown 笔记各一条 → 各自编辑器正确渲染 → 输入内容 → 等 800ms 自动保存 → 重开内容正确。

- [ ] **Step 3: type 锁定** — 已建笔记编辑器内无 type 切换入口 → 确认锁定生效。

- [ ] **Step 4: 搜索** — text/markdown 笔记内容可被搜索命中（FTS 索引 content_text）。

- [ ] **Step 5: 剪贴板/OCR/ASR 存入** — 剪贴板一键存笔记 → 列表显示 TXT 标记 → 内容为纯文本（无 html 包裹）→ type='text'。

- [ ] **Step 6: markdown 预览** — markdown 笔记输入 `# 标题\n**粗体**` → 预览面板正确渲染标题+粗体 → 折叠/展开预览正常。

- [ ] **Step 7: 记录结果** — e2e 通过后通知用户；若有问题回到对应 task 修复。

---

## Spec Coverage

| Spec section | Task |
|--------------|------|
| §4 Schema（content_text+content_html+type） | Task 2 |
| §4 迁移 v9→v10 幂等 ALTER | Task 3 |
| §5.1 NoteType enum（+Html） | Task 1 |
| §5.2 Note.note_type 字段 | Task 1 |
| §5.3 store 分发抽取 | Task 4 |
| §5.4 clipboard 改调 notepad（type=text） | Task 5 |
| §5.5 IPC 透传 type | Task 6 |
| §6.1-6.2 前端类型 + IPC 封装 | Task 8 |
| §6.3 三编辑器分发（TipTap/textarea/md） | Task 9, 10 |
| §6.4 type 选择 UX（方案①锁定） | Task 11 |
| §6.5 列表 type 标记 | Task 12 |
| §7 数据兼容（历史 html / 来源默认 type） | Task 3, 5, 6, 14 |
| §9 测试（NoteType/迁移/store） | Task 1, 3, 4 |
| §11 风险（幂等迁移 / dist 提交） | Task 3, 13 |
