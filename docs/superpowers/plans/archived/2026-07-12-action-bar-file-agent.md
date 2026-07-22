# Action Bar 文件 Agent 桥接 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 在 Finder 内选中文件/文件夹后，通过全局热键弹出 action bar，将选中对象交给外部 CLI agent（Claude Code / pi）处理；附带复制路径内置动作。

**Architecture:** 扩展 `ActionBarContext` 支持 `Files` 类型；新增 agent adapter 注册表（内置白名单 `claude` + `pi`，`which` 检测安装状态）；新增 `agent` / `copy_path` actionType；终端启动器抽象 trait，一期 Terminal.app；前端按 `accepts` 字段按选中类型过滤菜单。

**Tech Stack:** Rust + Tauri 2 + SQLite (rusqlite) + React + TypeScript + AppleScript（Finder 选中 / Terminal.app 启动）

**Spec:** [`docs/superpowers/specs/2026-07-12-action-bar-file-agent-design.md`](../specs/2026-07-12-action-bar-file-agent-design.md)

## Global Constraints

- 平台：一期仅 macOS（Finder AppleScript + Terminal.app）。非 macOS 路径编译通过但不触发，日志 warn。
- DB 迁移：遵循现有 `PRAGMA user_version` 模式，当前 v25 → v26。
- octopus 不碰文件系统——纯桥接，agent 在独立终端异步运行。
- 新增 Rust 模块在 `crates/desktop/src/main.rs` 用 `mod` 声明注册。
- 新增 Tauri 命令在 `main.rs` 的 `invoke_handler` 列表注册。
- 现有 actionType（ai/url/script/extension/copy）默认 `accepts=text`；submenu 默认 `accepts=any`。
- 注释和文档用中文。

---

## File Structure

**新建文件：**
| 文件 | 职责 |
|---|---|
| `crates/desktop/src/agent_adapter.rs` | Agent adapter 注册表：内置白名单定义、PATH 检测、DB CRUD、命令模板渲染 |
| `crates/desktop/src/terminal_launcher.rs` | 终端启动器 trait + Terminal.app 实现（AppleScript `do script`） |
| `crates/desktop/src/finder_selection.rs` | Finder 选中捕获（AppleScript + bundleId 检测） |
| `crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx` | Agent adapter 管理设置页 |

**修改文件：**
| 文件 | 变更 |
|---|---|
| `crates/infra/src/db.sql` | action_bar_items CREATE TABLE 加 agent/accepts 列；新增 agent_adapters 表 |
| `crates/infra/src/db.rs` | v25→v26 迁移；ActionBarItem struct 加 agent/accepts；CRUD 函数签名扩展；agent_adapters CRUD |
| `crates/desktop/src/action_bar_commands.rs` | ActionBarContext 扩展 kind/files；trigger 路径分流（Finder vs 文本）；execute_action_bar 加 agent/copy_path 分支 |
| `crates/desktop/src/main.rs` | mod 声明 + invoke_handler 注册新命令 |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | Context 类型扩展；accepts 过滤；agent task 输入框 |
| `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` | TYPE_META/ACTION_TYPES 加 agent/copy_path；编辑表单加 agent/accepts 字段 |

---

## Task 1: DB Schema 迁移 — action_bar_items 加列 + agent_adapters 新表

**Files:**
- Modify: `crates/infra/src/db.sql:272-288`（action_bar_items CREATE TABLE）
- Modify: `crates/infra/src/db.sql`（追加 agent_adapters CREATE TABLE）
- Modify: `crates/infra/src/db.rs:185-289`（v25→v26 迁移块）
- Test: `crates/infra/src/db.rs`（内联 mod tests）

**Interfaces:**
- Produces: `action_bar_items.agent TEXT DEFAULT ''`、`action_bar_items.accepts TEXT DEFAULT 'text'` 两列；`agent_adapters` 表；user_version = 26

- [x] **Step 1: 写失败测试 — 迁移后列存在 + agent_adapters 表存在**

在 `crates/infra/src/db.rs` 末尾 `mod tests` 中添加测试（如无 mod tests 则新建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_db() -> Connection {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        // 直接执行 INIT_SQL（含新表定义）
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 26", []).unwrap();
        conn
    }

    #[test]
    fn test_action_bar_items_has_agent_and_accepts_cols() {
        let conn = test_db();
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"agent".to_string()), "missing agent column: {:?}", cols);
        assert!(cols.contains(&"accepts".to_string()), "missing accepts column: {:?}", cols);
    }

    #[test]
    fn test_agent_adapters_table_exists() {
        let conn = test_db();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_adapters'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "agent_adapters table should exist");
    }
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra --lib tests::test_action_bar_items_has_agent_and_accepts_cols tests::test_agent_adapters_table_exists -- --nocapture`
Expected: FAIL（列/表不存在）

- [x] **Step 3: 修改 db.sql — action_bar_items CREATE TABLE 加列**

在 `crates/infra/src/db.sql` 第 284 行 `shortcut TEXT NOT NULL DEFAULT '',` 后追加两列：

```sql
    agent       TEXT NOT NULL DEFAULT '',
    accepts     TEXT NOT NULL DEFAULT 'text',
```

- [x] **Step 4: 修改 db.sql — 追加 agent_adapters 表**

在 db.sql 末尾（action_bar_items 相关 seed 之后）追加：

```sql
CREATE TABLE IF NOT EXISTS agent_adapters (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    key              TEXT NOT NULL UNIQUE,
    display_name     TEXT NOT NULL,
    detect_binary    TEXT NOT NULL,
    command_template TEXT NOT NULL,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [x] **Step 5: 修改 db.rs — v25→v26 迁移块**

在 `crates/infra/src/db.rs` 第 280 行 `return Ok(());` 之前，第 279 行 `conn.execute("PRAGMA user_version = 25", [])?;` 之后，插入迁移块：

```rust
        // v25→v26：action_bar_items 加 agent + accepts 列；新增 agent_adapters 表
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"agent".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN agent TEXT NOT NULL DEFAULT ''", [])?;
            }
            if !cols.contains(&"accepts".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN accepts TEXT NOT NULL DEFAULT 'text'", [])?;
            }
            // submenu 既有项升级为 accepts='any'（容器类型两场景通用）
            conn.execute(
                "UPDATE action_bar_items SET accepts='any' WHERE action_type='submenu' AND accepts='text'",
                [],
            )?;
            conn.execute_batch(INIT_SQL).ok(); // agent_adapters 表由 IF NOT EXISTS 自动建
            conn.execute("PRAGMA user_version = 26", [])?;
            log::info!("schema upgraded to v26 (action_bar_items.agent/accepts + agent_adapters table)");
        }
```

- [x] **Step 6: 更新 user_version 最终值**

在 `crates/infra/src/db.rs` 第 286-287 行，将全新安装的 `PRAGMA user_version = 25` 改为 `26`，日志也改：

```rust
    conn.execute("PRAGMA user_version = 26", [])?;
    log::info!("DB initialized (v26): schema + seed + yaml 配置导入（无 yaml 则跳过）");
```

- [x] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-infra --lib tests:: -- --nocapture`
Expected: PASS

- [x] **Step 8: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(db): v26 迁移——action_bar_items 加 agent/accepts 列 + agent_adapters 表"
```

---

## Task 2: ActionBarItem struct + CRUD 扩展（agent/accepts 列）

**Files:**
- Modify: `crates/infra/src/db.rs:978-1010`（ActionBarItem struct + SELECT_COLS + row mapper）
- Modify: `crates/infra/src/db.rs:1094-1176`（insert/update 函数签名）

**Interfaces:**
- Consumes: Task 1 的 agent/accepts 列
- Produces: `ActionBarItem { ..., agent: String, accepts: String }`；insert/update 带新参数

- [x] **Step 1: 写失败测试 — ActionBarItem 含 agent/accepts 字段**

在 Task 1 的 test_db 基础上，`crates/infra/src/db.rs` 的 mod tests 中添加：

```rust
    #[test]
    fn test_insert_action_bar_item_with_agent_accepts() {
        let conn = test_db();
        // 用新签名 insert
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order)
             VALUES (NULL, '测试agent', 'bot', 'agent', '{{task}}', 'claude', 'file', 0)",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.agent, "claude");
        assert_eq!(item.accepts, "file");
        assert_eq!(item.action_type, "agent");
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra --lib tests::test_insert_action_bar_item_with_agent_accepts -- --nocapture`
Expected: 编译失败（struct 无 agent/accepts 字段）

- [x] **Step 3: 扩展 ActionBarItem struct**

`crates/infra/src/db.rs:978`，在 `shortcut: String,` 后追加：

```rust
pub struct ActionBarItem {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub icon: String,
    pub action_type: String,
    pub action_data: String,
    pub sort_order: i64,
    pub is_system: bool,
    pub is_enabled: bool,
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
    pub shortcut: String,
    pub agent: String,
    pub accepts: String,
}
```

- [x] **Step 4: 更新 ACTION_BAR_SELECT_COLS + row mapper**

`crates/infra/src/db.rs:993`，SELECT_COLS 加两列：

```rust
const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts";
```

`crates/infra/src/db.rs:995-1010`，row mapper 加两列（索引 12、13）：

```rust
fn row_to_action_bar_item(row: &rusqlite::Row) -> rusqlite::Result<ActionBarItem> {
    Ok(ActionBarItem {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        icon: row.get(3)?,
        action_type: row.get(4)?,
        action_data: row.get(5)?,
        sort_order: row.get(6)?,
        is_system: row.get::<_, i32>(7)? != 0,
        is_enabled: row.get::<_, i32>(8)? != 0,
        is_async: row.get::<_, i32>(9)? != 0,
        write_output_to_clipboard: row.get::<_, i32>(10)? != 0,
        shortcut: row.get(11)?,
        agent: row.get(12)?,
        accepts: row.get(13)?,
    })
}
```

- [x] **Step 5: 扩展 insert/update 函数签名**

`crates/infra/src/db.rs:1094`，`insert_action_bar_item` 加 `agent: &str, accepts: &str` 参数（pub 和 _at 两个函数都加）：

```rust
pub fn insert_action_bar_item(
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard, shortcut, agent, accepts))
}
```

`insert_action_bar_item_at` 的 SQL 改为：

```rust
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, ?7, ?8, ?9, ?10, ?11)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts],
    )?;
```

`update_action_bar_item` 同样加 `agent: &str, accepts: &str` 参数，SQL UPDATE 加 `agent=?10, accepts=?11`：

```rust
pub fn update_action_bar_item(
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
    agent: &str,
    accepts: &str,
) -> Result<()> {
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled, is_async, write_output_to_clipboard, shortcut, agent, accepts))
}
```

`update_action_bar_item_at` 的 SQL 改为：

```rust
    conn.execute(
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, shortcut=?8, agent=?9, accepts=?10, updated_at=datetime('now') WHERE id=?11",
        params![title, icon, action_type, action_data, is_enabled as i32, is_async as i32, write_output_to_clipboard as i32, shortcut, agent, accepts, id],
    )?;
```

- [x] **Step 6: 运行测试确认通过**

Run: `cargo test -p octopus-infra --lib -- --nocapture`
Expected: PASS（可能有编译错误来自 desktop crate 调用方，先忽略，下个 task 修）

- [x] **Step 7: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(db): ActionBarItem 加 agent/accepts 字段 + CRUD 签名扩展"
```

---

## Task 3: agent_adapters 表 CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（在 ScriptRun 相关代码之后追加 agent_adapters CRUD）

**Interfaces:**
- Consumes: Task 1 的 agent_adapters 表
- Produces: `AgentAdapterRecord { id, key, display_name, detect_binary, command_template }`；`list_agent_adapter_records() / insert_agent_adapter_record() / update_agent_adapter_record() / delete_agent_adapter_record()`

- [x] **Step 1: 写失败测试 — adapter CRUD 往返**

在 `crates/infra/src/db.rs` mod tests 中添加：

```rust
    #[test]
    fn test_agent_adapter_crud_roundtrip() {
        let conn = test_db();
        let id = insert_agent_adapter_record_at(&conn, "myagent", "My Agent", "myagent-bin", "myagent {prompt}").unwrap();
        let list = list_agent_adapter_records_at(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "myagent");
        assert_eq!(list[0].command_template, "myagent {prompt}");

        update_agent_adapter_record_at(&conn, id, "myagent2", "My Agent 2", "myagent2-bin", "myagent2 {prompt} {files}").unwrap();
        let updated = list_agent_adapter_records_at(&conn).unwrap();
        assert_eq!(updated[0].key, "myagent2");
        assert_eq!(updated[0].command_template, "myagent2 {prompt} {files}");

        delete_agent_adapter_record_at(&conn, id).unwrap();
        assert_eq!(list_agent_adapter_records_at(&conn).unwrap().len(), 0);
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra --lib tests::test_agent_adapter_crud_roundtrip -- --nocapture`
Expected: 编译失败（函数不存在）

- [x] **Step 3: 实现 struct + CRUD 函数**

在 `crates/infra/src/db.rs` ScriptRun 部分之后追加：

```rust
// ── Agent Adapter（用户自定义 agent 适配器）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapterRecord {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
}

pub fn list_agent_adapter_records() -> Result<Vec<AgentAdapterRecord>> {
    with_db(list_agent_adapter_records_at)
}

fn list_agent_adapter_records_at(conn: &Connection) -> Result<Vec<AgentAdapterRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, key, display_name, detect_binary, command_template FROM agent_adapters ORDER BY id"
    )?;
    let rows = stmt.query_map([], |r| Ok(AgentAdapterRecord {
        id: r.get(0)?,
        key: r.get(1)?,
        display_name: r.get(2)?,
        detect_binary: r.get(3)?,
        command_template: r.get(4)?,
    }))?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn insert_agent_adapter_record(
    key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<i64> {
    with_db(|conn| insert_agent_adapter_record_at(conn, key, display_name, detect_binary, command_template))
}

fn insert_agent_adapter_record_at(
    conn: &Connection,
    key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES (?1, ?2, ?3, ?4)",
        params![key, display_name, detect_binary, command_template],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_agent_adapter_record(
    id: i64, key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<()> {
    with_db(|conn| update_agent_adapter_record_at(conn, id, key, display_name, detect_binary, command_template))
}

fn update_agent_adapter_record_at(
    conn: &Connection,
    id: i64, key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE agent_adapters SET key=?1, display_name=?2, detect_binary=?3, command_template=?4, updated_at=datetime('now') WHERE id=?5",
        params![key, display_name, detect_binary, command_template, id],
    )?;
    Ok(())
}

pub fn delete_agent_adapter_record(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id])?;
        Ok(())
    })
}
```

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test -p octopus-infra --lib tests::test_agent_adapter_crud_roundtrip -- --nocapture`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(db): agent_adapters 表 CRUD（用户自定义 adapter 持久化）"
```

---

## Task 4: Agent adapter 注册表 + PATH 检测 + 命令模板渲染

**Files:**
- Create: `crates/desktop/src/agent_adapter.rs`
- Modify: `crates/desktop/src/main.rs:5`（mod 声明）

**Interfaces:**
- Consumes: Task 3 的 `AgentAdapterRecord`
- Produces: `AgentAdapter { key, display_name, detect_binary, command_template, is_builtin, is_available }`；`list_adapters()`、`refresh_detection()`、`render_command(adapter_key, prompt, files, cwd)`

- [x] **Step 1: 写失败测试 — 模板渲染**

新建 `crates/desktop/src/agent_adapter.rs`，先写测试模块：

```rust
//! Agent 适配器注册表——内置白名单 + DB 用户自定义 + PATH 检测 + 命令模板渲染。

use std::path::Path;
use octopus_infra::db::AgentAdapterRecord;

/// Agent 适配器——描述一个 CLI agent 的检测与启动方式。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapter {
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
    pub is_builtin: bool,
    pub is_available: bool,
}

/// 内置白名单（一期）
fn builtin_adapters() -> Vec<AgentAdapter> {
    vec![
        AgentAdapter {
            key: "claude".into(),
            display_name: "Claude Code".into(),
            detect_binary: "claude".into(),
            command_template: "claude --add-dir \"{cwd}\" \"{prompt}\"".into(),
            is_builtin: true,
            is_available: false,
        },
        AgentAdapter {
            key: "pi".into(),
            display_name: "Pi".into(),
            detect_binary: "pi".into(),
            command_template: "pi {files_at} \"{prompt}\"".into(),
            is_builtin: true,
            is_available: false,
        },
    ]
}

/// 合并内置 + DB 用户自定义 adapter，逐个检测 PATH。
pub fn list_adapters() -> Vec<AgentAdapter> {
    let mut adapters = builtin_adapters();
    if let Ok(custom) = octopus_infra::db::list_agent_adapter_records() {
        for r in custom {
            adapters.push(AgentAdapter {
                key: r.key,
                display_name: r.display_name,
                detect_binary: r.detect_binary,
                command_template: r.command_template,
                is_builtin: false,
                is_available: false,
            });
        }
    }
    for a in adapters.iter_mut() {
        a.is_available = which(&a.detect_binary);
    }
    adapters
}

/// 重新检测所有 adapter（设置页「刷新检测」按钮用）。
pub fn refresh_detection() -> Vec<AgentAdapter> {
    list_adapters()
}

/// which <binary> —— 检测 PATH 中是否存在二进制。
fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 按模板渲染命令字符串。
/// prompt: 渲染后的 prompt（含 task）
/// files: POSIX 路径列表
/// cwd: 工作目录
pub fn render_command(template: &str, prompt: &str, files: &[String], cwd: &str) -> String {
    let files_str = files.join(" ");
    let files_at_str = files.iter().map(|f| format!("@{}", f)).collect::<Vec<_>>().join(" ");
    template
        .replace("{prompt}", &shell_escape_single(prompt))
        .replace("{files_at}", &files_at_str)
        .replace("{files}", &files_str)
        .replace("{cwd}", cwd)
}

/// shell 单引号转义：用单引号包裹，内部单引号用 '"'"' 转义。
fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_command_claude() {
        let cmd = render_command(
            "claude --add-dir \"{cwd}\" \"{prompt}\"",
            "整理这些文件",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "claude --add-dir \"/Users/x\" \"'整理这些文件'\"");
    }

    #[test]
    fn test_render_command_pi() {
        let cmd = render_command(
            "pi {files_at} \"{prompt}\"",
            "make ppt",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "pi @/a.pdf @/b.pdf \"'make ppt'\"");
    }

    #[test]
    fn test_shell_escape_single_with_quote() {
        let escaped = shell_escape_single("it's a test");
        assert_eq!(escaped, "'it'\"'\"'s a test'");
    }

    #[test]
    fn test_builtin_adapters_has_claude_and_pi() {
        let builtins = builtin_adapters();
        let keys: Vec<&str> = builtins.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&"claude"));
        assert!(keys.contains(&"pi"));
    }
}
```

- [x] **Step 2: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --lib agent_adapter::tests -- --nocapture`
Expected: PASS

注意：claude 模板测试中，`shell_escape_single("整理这些文件")` = `'整理这些文件'`（无单引号不转义），放在 `"..."` 里就是 `"'整理这些文件'"`。

- [x] **Step 3: 注册 mod**

在 `crates/desktop/src/main.rs:5` `mod action_bar_commands;` 附近添加：

```rust
mod agent_adapter;
```

- [x] **Step 4: 运行测试再次确认通过**

Run: `cargo test -p octopus-desktop --lib agent_adapter::tests -- --nocapture`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/agent_adapter.rs crates/desktop/src/main.rs
git commit -m "feat(agent): adapter 注册表——内置 claude/pi 白名单 + PATH 检测 + 命令模板渲染"
```

---

## Task 5: 终端启动器（Terminal.app）

**Files:**
- Create: `crates/desktop/src/terminal_launcher.rs`
- Modify: `crates/desktop/src/main.rs`（mod 声明）

**Interfaces:**
- Produces: `TerminalLauncher` trait + `TerminalAppLauncher` impl；`TerminalAppLauncher::spawn(command, cwd)`

- [x] **Step 1: 写失败测试 — spawn 生成有效 AppleScript**

```rust
//! 终端启动器抽象——trait + Terminal.app 实现。

use std::path::Path;

pub trait TerminalLauncher {
    /// 在新终端窗口执行命令，cwd 指定工作目录。
    fn spawn(&self, command: &str, cwd: &Path) -> Result<(), String>;
}

/// 一期实现：Terminal.app via AppleScript `do script`（打开新窗口）。
pub struct TerminalAppLauncher;

impl TerminalLauncher for TerminalAppLauncher {
    fn spawn(&self, command: &str, cwd: &Path) -> Result<(), String> {
        let cwd_str = cwd.to_string_lossy();
        // 组装完整 shell 命令：cd 到工作目录 → 执行命令
        let full_cmd = format!("cd {} && {}", shell_quote(&cwd_str), command);
        // AppleScript：tell application "Terminal" → do script（新窗口）→ activate
        let script = format!(
            r#"-e 'tell application "Terminal"' -e 'do script "{}"' -e 'activate' -e 'end tell'"#,
            escape_applescript_string(&full_cmd)
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &format!(r#"tell application "Terminal"
    do script "{}"
    activate
end tell"#, escape_applescript_string(&full_cmd))])
            .output()
            .map_err(|e| format!("启动 Terminal.app 失败: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Terminal.app 启动失败: {}", stderr));
        }
        Ok(())
    }
}

/// AppleScript 字符串转义：双引号和反斜杠。
fn escape_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// shell 引号包裹路径（处理含空格的路径）。
fn shell_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_applescript_string() {
        assert_eq!(escape_applescript_string(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_applescript_string(r#"C:\path"#), r#"C:\\path"#);
    }

    #[test]
    fn test_shell_quote() {
        assert_eq!(shell_quote("/Users/My User"), r#""/Users/My User""#);
        assert_eq!(shell_quote(r#"a"b"#), r#""a\"b""#);
    }
}
```

- [x] **Step 2: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --lib terminal_launcher::tests -- --nocapture`
Expected: PASS

- [x] **Step 3: 注册 mod**

在 `crates/desktop/src/main.rs` 添加：

```rust
mod terminal_launcher;
```

- [x] **Step 4: 运行测试再次确认通过**

Run: `cargo test -p octopus-desktop --lib terminal_launcher::tests -- --nocapture`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/terminal_launcher.rs crates/desktop/src/main.rs
git commit -m "feat(terminal): TerminalLauncher trait + Terminal.app 实现（AppleScript do script）"
```

---

## Task 6: Finder 选中捕获

**Files:**
- Create: `crates/desktop/src/finder_selection.rs`
- Modify: `crates/desktop/src/main.rs`（mod 声明）

**Interfaces:**
- Produces: `is_finder_frontmost() -> bool`；`get_finder_selection() -> Result<Vec<String>, String>`

- [x] **Step 1: 实现模块**

新建 `crates/desktop/src/finder_selection.rs`：

```rust
//! Finder 选中捕获——检测前台是否 Finder + AppleScript 拿 selection POSIX 路径。

/// 前台 app 是否为 Finder（com.apple.finder）。
pub fn is_finder_frontmost() -> bool {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Events" to get bundle identifier of first application process whose frontmost is true"#)
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let bid = String::from_utf8_lossy(&o.stdout).trim().to_string();
                bid == "com.apple.finder"
            }
            _ => false,
        }
    }
    #[cfg(not(target_os = "macos"))]
    { false }
}

/// 获取 Finder 当前选中文件的 POSIX 路径列表。空选中返回空 Vec。
pub fn get_finder_selection() -> Result<Vec<String>, String> {
    #[cfg(not(target_os = "macos"))]
    { return Err("仅 macOS 支持 Finder 选中捕获".into()); }

    #[cfg(target_os = "macos")]
    {
        let script = r#"
tell application "Finder"
    set sel to selection
    if (count of sel) = 0 then return ""
    set paths to ""
    repeat with f in sel
        set paths to paths & (POSIX path of (f as alias)) & linefeed
    end repeat
    return paths
end tell
"#;
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("osascript 执行失败: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("AppleScript 错误: {}", stderr));
        }
        let result = String::from_utf8_lossy(&output.stdout);
        let files: Vec<String> = result.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_finder_frontmost_returns_bool() {
        // 仅验证返回类型是 bool，不验证具体值（取决于运行环境）
        let _ = is_finder_frontmost();
    }
}
```

- [x] **Step 2: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --lib finder_selection::tests -- --nocapture`
Expected: PASS

- [x] **Step 3: 注册 mod**

在 `crates/desktop/src/main.rs` 添加：

```rust
mod finder_selection;
```

- [x] **Step 4: 运行测试再次确认通过**

Run: `cargo test -p octopus-desktop --lib finder_selection::tests -- --nocapture`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/finder_selection.rs crates/desktop/src/main.rs
git commit -m "feat(finder): Finder 选中捕获——bundleId 检测 + AppleScript selection"
```

---

## Task 7: ActionBarContext 扩展 + trigger 路径分流

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs:10-172`（ContextKind/ActionBarContext + trigger_action_bar）

**Interfaces:**
- Consumes: Task 6 的 `is_finder_frontmost` / `get_finder_selection`
- Produces: `ActionBarContext { kind: ContextKind, text: Option<String>, files: Vec<String> }`；trigger 分流 Finder→Files / 其他→Text

- [x] **Step 1: 扩展 ActionBarContext**

在 `crates/desktop/src/action_bar_commands.rs:10`，替换现有 ActionBarContext：

```rust
/// 选中对象类型。
#[derive(Clone, serde::Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind {
    Text,
    Files,
}

/// 暂存选中对象 + 上下文（trigger 时写入，前端 mount 时读）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub kind: ContextKind,
    pub text: Option<String>,
    pub files: Vec<String>,
}

impl ActionBarContext {
    pub fn text(text: String) -> Self {
        Self { kind: ContextKind::Text, text: Some(text), files: vec![] }
    }
    pub fn files(files: Vec<String>) -> Self {
        Self { kind: ContextKind::Files, text: None, files }
    }
}
```

- [x] **Step 2: 修改 trigger_action_bar —— Finder 分流**

在 `crates/desktop/src/action_bar_commands.rs` 的 `trigger_action_bar` 函数中（第 33 行 `std::thread::spawn` 内），在重入 guard 之后、记录剪贴板之前，插入 Finder 检测分流：

```rust
        // ── Finder 分流：前台是 Finder 时走文件选中路径，否则走文本路径 ──
        if crate::finder_selection::is_finder_frontmost() {
            let files = match crate::finder_selection::get_finder_selection() {
                Ok(f) if !f.is_empty() => f,
                Ok(_) => {
                    log::info!("[action-bar] Finder 空选中，不弹窗");
                    finalize_action_bar(&app_clone);
                    return;
                }
                Err(e) => {
                    log::warn!("[action-bar] Finder selection 获取失败: {}", e);
                    finalize_action_bar(&app_clone);
                    return;
                }
            };
            log::info!("[action-bar] Finder selection: {} files", files.len());
            *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext::files(files));

            // 鼠标位置 + 显示浮窗（复用现有坐标逻辑）
            let (mx, my) = get_mouse_position(&app_clone);
            let mut win_x = mx - 190.0;
            let win_y = my - 42.0;
            const WIN_W: f64 = 380.0;
            // 碰撞检测（复用现有 monitor 逻辑——提取为函数更佳，但此处内联保持最小改动）
            if let Some(monitor) = app_clone.available_monitors().ok().and_then(|monitors| {
                monitors.into_iter().find(|m| {
                    let scale = m.scale_factor();
                    let mon_left = m.position().x as f64 / scale;
                    let mon_top = m.position().y as f64 / scale;
                    let mon_right = (m.position().x as f64 + m.size().width as f64) / scale;
                    let mon_bottom = (m.position().y as f64 + m.size().height as f64) / scale;
                    mx >= mon_left && mx < mon_right && my >= mon_top && my < mon_bottom
                })
            }) {
                let scale = monitor.scale_factor();
                let mon_x = monitor.position().x as f64 / scale;
                let mon_w = monitor.size().width as f64 / scale;
                let mon_right = mon_x + mon_w;
                if win_x + WIN_W > mon_right { win_x = mon_right - WIN_W; }
                if win_x < mon_x { win_x = mon_x; }
            }
            let app_for_show = app_clone.clone();
            let _ = app_clone.run_on_main_thread(move || {
                show_action_bar_window(&app_for_show, win_x, win_y);
            });
            return;
        }
```

- [x] **Step 3: 修改现有 text 路径的 ActionBarContext 构造**

在同一个函数中，找到第 103 行 `*PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext { text });`，改为：

```rust
        *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext::text(text));
```

- [x] **Step 4: 修复编译错误**

`execute_action_bar` 等下游函数使用 `text` 参数的地方不变（text 场景仍传 text）。但 `execute_action_bar_inner` 现有签名 `item_id: i64, text: String, app` 需要扩展为支持 files。先暂时保持 text 签名不变，agent/copy_path 分支在下个 task 添加。确保现有代码编译通过。

Run: `cargo build -p octopus-desktop 2>&1 | head -30`
Expected: 编译通过（或仅 agent/copy_path 相关的下游警告）

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-desktop --lib action_bar_commands -- --nocapture`
Expected: PASS

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat(action-bar): ActionBarContext 扩展 kind/files + trigger Finder 分流"
```

---

## Task 8: agent / copy_path 执行分支 + Tauri 命令

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（execute_action_bar_inner 加分支 + 新命令）

**Interfaces:**
- Consumes: Task 4 `render_command` / `list_adapters`；Task 5 `TerminalAppLauncher`
- Produces: agent/copy_path 在 `execute_action_bar_inner` 中的分支；新命令 `trigger_agent_action`、`copy_selected_path`

- [x] **Step 1: 在 execute_action_bar_inner 中加 agent + copy_path 分支**

在 `crates/desktop/src/action_bar_commands.rs:787` 的 `match item.action_type.as_str()` 中，在 `"copy" =>` 分支之前追加：

```rust
        "agent" => {
            // agent 桥接：渲染命令 → Terminal.app 启动
            let adapter_key = item.agent.clone();
            let adapters = crate::agent_adapter::list_adapters();
            let adapter = adapters.into_iter().find(|a| a.key == adapter_key)
                .ok_or_else(|| format!("Agent adapter '{}' 不存在", adapter_key))?;
            if !adapter.is_available {
                return Err(format!("{} 未安装（未在 PATH 找到 `{}`）", adapter.display_name, adapter.detect_binary));
            }
            // prompt 渲染：action_data 是模板，含 {{files}} {{task}}
            // {{task}} 在前端已收集并替换（execute_action_bar 的 text 参数复用为 task）
            // 这里 text 参数 = 用户输入的 task（或空，无 {{task}} 时）
            let prompt = item.action_data
                .replace("{{task}}", &text)
                .replace("{{files}}", &app_state_files.join("\n"));
            // cwd：首个文件的父目录
            let cwd = app_state_files.first()
                .and_then(|f| std::path::Path::new(f).parent().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
            let cwd_path = std::path::Path::new(&cwd);
            let command = crate::agent_adapter::render_command(
                &adapter.command_template, &prompt, &app_state_files, &cwd,
            );
            let launcher = crate::terminal_launcher::TerminalAppLauncher;
            launcher.spawn(&command, cwd_path)?;
            Ok(false)
        }
        "copy_path" => {
            // 复制文件路径到剪贴板。format: plain / url / quoted
            let files = app_state_files.clone();
            let formatted: String = match item.action_data.as_str() {
                "url" => files.iter().map(|f| format!("file://{}", url_encode_path(f))).collect::<Vec<_>>().join("\n"),
                "quoted" => files.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join("\n"),
                _ => files.join("\n"), // plain
            };
            write_clipboard_text(app, &formatted);
            Ok(false)
        }
```

注意：`app_state_files` 是从 PENDING_CONTEXT 取的文件列表。需要在 `execute_action_bar_inner` 开头加入提取逻辑。在函数开头 `let item = ...` 之后加：

```rust
    // 从 PENDING_CONTEXT 取 files（Files 场景）
    let app_state_files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();
```

- [x] **Step 2: 添加 url_encode_path 辅助函数**

在 action_bar_commands.rs 辅助函数区域添加：

```rust
fn url_encode_path(path: &str) -> String {
    path.chars().map(|c| match c {
        ' ' => "%20".into(),
        _ => c.to_string(),
    }).collect()
}
```

- [x] **Step 3: 更新 execute_action_bar 命令签名以支持 files 上下文**

现有 `execute_action_bar(item_id, text, app)` 在 agent 场景，text 参数语义变为 task。前端调用时，text 场景传选中文本，agent 场景传用户输入的 task。保持签名不变，语义重载。

- [x] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -40`
Expected: 编译通过

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-desktop --lib -- --nocapture`
Expected: PASS

- [x] **Step 6: 注册新 Tauri 命令**

在 `crates/desktop/src/main.rs:297-320` 的 invoke_handler 列表中，现有 action_bar_commands 命令之后添加 adapter 管理命令：

```rust
            // agent adapter 管理
            action_bar_commands::list_agent_adapters,
            action_bar_commands::create_agent_adapter,
            action_bar_commands::update_agent_adapter,
            action_bar_commands::delete_agent_adapter,
            action_bar_commands::refresh_agent_detection,
```

在 `crates/desktop/src/action_bar_commands.rs` 添加这些 Tauri 命令包装：

```rust
// ── Agent Adapter 管理命令（设置页用）──

#[tauri::command]
pub fn list_agent_adapters() -> Result<Vec<crate::agent_adapter::AgentAdapter>, String> {
    Ok(crate::agent_adapter::list_adapters())
}

#[tauri::command]
pub fn create_agent_adapter(
    key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<i64, String> {
    octopus_infra::db::insert_agent_adapter_record(&key, &display_name, &detect_binary, &command_template)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_agent_adapter(
    id: i64, key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<(), String> {
    octopus_infra::db::update_agent_adapter_record(id, &key, &display_name, &detect_binary, &command_template)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_agent_adapter(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_agent_adapter_record(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn refresh_agent_detection() -> Result<Vec<crate::agent_adapter::AgentAdapter>, String> {
    Ok(crate::agent_adapter::refresh_detection())
}
```

- [x] **Step 7: 编译 + 运行**

Run: `cargo build -p octopus-desktop 2>&1 | head -40`
Expected: 编译通过

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs crates/desktop/src/main.rs
git commit -m "feat(action-bar): agent/copy_path 执行分支 + adapter 管理 Tauri 命令"
```

---

## Task 9: 前端 ActionBar 浮窗 — context 类型 + accepts 过滤

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`

**Interfaces:**
- Consumes: Task 7 的 `ActionBarContext { kind, text, files }`
- Produces: 按 accepts 过滤的菜单；Files 场景文件计数 badge

- [x] **Step 1: 扩展 Context 类型 + ActionBarItem 类型**

在 `crates/desktop/frontend/src/pages/ActionBar/index.tsx:10-27`，替换：

```tsx
type ContextKind = "text" | "files";

interface Context {
  kind: ContextKind;
  text: string;   // text 场景
  files: string[]; // files 场景
}

type View = "main" | "submenu" | "loading" | "task-input";

interface ActionBarItem {
  id: number;
  parentId: number | null;
  title: string;
  icon: string;
  actionType: string;
  actionData: string;
  sortOrder: number;
  isSystem: boolean;
  isEnabled: boolean;
  shortcut?: string;
  agent?: string;
  accepts?: string; // text/file/any
}
```

- [x] **Step 2: 修改 refresh —— 适配新 context 结构**

在 `crates/desktop/frontend/src/pages/ActionBar/index.tsx:181`，`invoke<Context | null>` 回调中：

```tsx
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        setView("main"); setSelectedIdx(0); setFocusLayer("main");
        if (ctx) { setContext(ctx); }
      });
```

无需大改——Context 结构已扩展。

- [x] **Step 3: 添加 accepts 过滤逻辑**

在 `crates/desktop/frontend/src/pages/ActionBar/index.tsx:233`，修改 `allMainItems` 过滤：

```tsx
  const isItemVisible = (item: ActionBarItem): boolean => {
    if (!context) return true;
    const accepts = item.accepts || "text";
    const kind = context.kind;
    if (accepts === "any") return true;
    if (kind === "text") return accepts === "text";
    return accepts === "file";
  };

  // submenu 特殊处理：子项全不可见则自身也隐藏（递归）
  const isSubmenuVisible = (item: ActionBarItem): boolean => {
    const subs = menuItems.filter((i) => i.parentId === item.id);
    if (subs.length === 0) return true; // 空子菜单保持可见（用户可后续加项）
    return subs.some((s) =>
      s.actionType === "submenu" ? isSubmenuVisible(s) : isItemVisible(s)
    );
  };

  const allMainItems = menuItems.filter((i) => i.parentId === null);
  const mainItems = allMainItems.filter((i) => {
    if (!isItemVisible(i)) return false;
    if (i.actionType === "submenu" && !isSubmenuVisible(i)) return false;
    if (i.actionType === "url" && i.actionData === "") {
      return context ? detectActionUrl(context.text || "").isUrl : false;
    }
    return true;
  });
```

- [x] **Step 4: 修改 executeItem —— agent 类型处理 task 输入**

在 `executeItem` 函数中（约 277 行），在 `if (item.actionType === "ai")` 之前加 agent 分支：

```tsx
    if (item.actionType === "agent") {
      // 含 {{task}} → 弹输入框；否则直接执行
      if (item.actionData.includes("{{task}}")) {
        setView("task-input");
        return;
      }
      // 无 {{task}} → 直接执行，task 传空串
      try {
        await invoke("execute_action_bar", { itemId: item.id, text: "" });
      } catch (e) {
        showQuickError(String(e).slice(0, 40));
      }
      return;
    }

    if (item.actionType === "copy_path") {
      try {
        await invoke("execute_action_bar", { itemId: item.id, text: "" });
      } catch (e) {
        showQuickError(String(e).slice(0, 40));
      }
      return;
    }
```

- [x] **Step 5: 添加 task-input 视图渲染**

在组件 JSX 中，添加 task-input 视图（一个输入框 + 回车提交）：

```tsx
  const [taskInput, setTaskInput] = useState("");
  const [taskItem, setTaskItem] = useState<ActionBarItem | null>(null);
  const taskInputRef = useRef<HTMLInputElement>(null);

  // 进入 task-input 视图时聚焦输入框
  useEffect(() => {
    if (view === "task-input") {
      setTimeout(() => taskInputRef.current?.focus(), 50);
    }
  }, [view]);

  const submitTask = async () => {
    if (!taskItem) return;
    setView("loading");
    try {
      await invoke("execute_action_bar", { itemId: taskItem.id, text: taskInput });
    } catch (e) {
      showQuickError(String(e).slice(0, 40));
      setView("main");
    }
  };
```

在动态窗口高度 useEffect 中（169 行），加 task-input 高度：

```tsx
    const height = view === "submenu" ? 78 : view === "loading" ? 48 : view === "task-input" ? 48 : 40;
```

在 render 区域（return 部分），根据 view 渲染 task-input：

```tsx
  if (view === "task-input") {
    return (
      <div data-action-bar className="flex items-center gap-2 px-3 py-2">
        <input
          ref={taskInputRef}
          value={taskInput}
          onChange={(e) => setTaskInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); submitTask(); }
            if (e.key === "Escape") { setView("main"); setTaskInput(""); }
          }}
          placeholder="告诉 agent 做什么…"
          className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/50"
        />
        <span className="text-[10px] text-muted-foreground">↵ 执行 · Esc 取消</span>
      </div>
    );
  }
```

修改 executeItem 中 agent 含 {{task}} 分支：

```tsx
    if (item.actionType === "agent") {
      if (item.actionData.includes("{{task}}")) {
        setTaskItem(item);
        setTaskInput("");
        setView("task-input");
        return;
      }
      // ...直接执行
    }
```

- [x] **Step 6: 前端编译验证**

Run: `cd crates/desktop/frontend && npm run build 2>&1 | tail -20`
Expected: 编译通过

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "feat(frontend): ActionBar 浮窗——context 扩展 kind/files + accepts 过滤 + agent task 输入框"
```

---

## Task 10: 前端设置页 — ActionBarPanel 加 agent/copy_path 类型

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`

**Interfaces:**
- Consumes: Task 8 的 `list_agent_adapters` 命令

- [x] **Step 1: 扩展 TYPE_META + ACTION_TYPES**

在 `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx:39-88`，TYPE_META 加两项：

```tsx
  agent: {
    dot: "bg-rose-500",
    label: "AGENT",
    descKey: "settings.actionBar.typeAgentDesc",
    placeholderKey: "settings.actionBar.typeAgentPlaceholder",
  },
  copy_path: {
    dot: "bg-cyan-500",
    label: "PATH",
    descKey: "settings.actionBar.typeCopyPathDesc",
    placeholderKey: "",
  },
```

ACTION_TYPES 加两项（在 copy 之前）：

```tsx
  { value: "agent", labelKey: "settings.actionBar.typeAgent" },
  { value: "copy_path", labelKey: "settings.actionBar.typeCopyPath" },
```

- [x] **Step 2: 扩展 ActionBarItem interface**

在 `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx:22-35`，ActionBarItem interface 加：

```tsx
  agent?: string;
  accepts?: string;
```

- [x] **Step 3: 编辑表单加 agent 下拉 + accepts 下拉**

在编辑表单组件（EditFormProps 相关区域）中，当 `type === "agent"` 时显示 agent adapter 下拉；当 type 为 agent/copy_path 时显示 accepts 下拉（默认 file）。

在编辑表单的 type 选择器之后添加：

```tsx
  {type === "agent" && (
    <>
      <label className="text-xs text-muted-foreground">Agent</label>
      <select
        value={form.agent || ""}
        onChange={(e) => onChange({ ...form, agent: e.target.value })}
        className="..."
      >
        <option value="">选择 agent…</option>
        {adapters.filter((a) => a.isAvailable).map((a) => (
          <option key={a.key} value={a.key}>{a.displayName}</option>
        ))}
      </select>
    </>
  )}
  {(type === "agent" || type === "copy_path") && (
    <>
      <label className="text-xs text-muted-foreground">适用场景</label>
      <select
        value={form.accepts || "file"}
        onChange={(e) => onChange({ ...form, accepts: e.target.value })}
        className="..."
      >
        <option value="file">文件</option>
        <option value="any">文件 + 文本</option>
      </select>
    </>
  )}
```

需要在组件顶部加载 adapters：

```tsx
  const [adapters, setAdapters] = useState<AgentAdapter[]>([]);
  useEffect(() => {
    invoke<AgentAdapter[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  }, []);
```

AgentAdapter interface：

```tsx
interface AgentAdapter {
  key: string;
  displayName: string;
  detectBinary: string;
  commandTemplate: string;
  isBuiltin: boolean;
  isAvailable: boolean;
}
```

- [x] **Step 4: copy_path 类型的 actionData 改为格式选择**

copy_path 的 actionData 不是脚本，是格式（plain/url/quoted）。在编辑表单中 type === copy_path 时显示格式下拉替代文本框：

```tsx
  {type === "copy_path" && (
    <>
      <label className="text-xs text-muted-foreground">路径格式</label>
      <select
        value={form.actionData || "plain"}
        onChange={(e) => onChange({ ...form, actionData: e.target.value })}
        className="..."
      >
        <option value="plain">纯路径</option>
        <option value="url">file:// URL</option>
        <option value="quoted">带引号</option>
      </select>
    </>
  )}
```

- [x] **Step 5: 前端编译验证**

Run: `cd crates/desktop/frontend && npm run build 2>&1 | tail -20`
Expected: 编译通过

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx
git commit -m "feat(frontend): ActionBarPanel 加 agent/copy_path 类型 + agent/accepts 编辑字段"
```

---

## Task 11: 前端设置页 — AgentPanel（adapter 管理）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx`
- Modify: 设置页路由注册（找到现有设置页 tab 注册处）

**Interfaces:**
- Consumes: Task 8 的 adapter CRUD 命令

- [x] **Step 1: 创建 AgentPanel 组件**

新建 `crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx`：

```tsx
import { useState, useEffect } from "react";
import { invoke } from "@/lib/tauri";
import { useT } from "@/lib/i18n";
import { cn } from "@/lib/utils";

interface AgentAdapter {
  key: string;
  displayName: string;
  detectBinary: string;
  commandTemplate: string;
  isBuiltin: boolean;
  isAvailable: boolean;
}

export default function AgentPanel() {
  const t = useT();
  const [adapters, setAdapters] = useState<AgentAdapter[]>([]);
  const [editing, setEditing] = useState<Partial<AgentAdapter> | null>(null);

  const refresh = () => {
    invoke<AgentAdapter[]>("list_agent_adapters").then(setAdapters);
  };

  useEffect(refresh, []);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">{t("settings.agent.title")}</h2>
        <button onClick={refresh} className="text-xs text-voice hover:underline">
          {t("settings.agent.refresh")}
        </button>
      </div>
      <div className="space-y-2">
        {adapters.map((a) => (
          <div key={a.key} className={cn(
            "flex items-center justify-between rounded-lg border p-3",
            a.isAvailable ? "border-voice/30" : "border-muted",
          )}>
            <div>
              <div className="flex items-center gap-2">
                <span className="font-medium">{a.displayName}</span>
                {a.isBuiltin && <span className="text-[10px] text-muted-foreground">内置</span>}
              </div>
              <div className="text-xs text-muted-foreground font-mono">{a.detectBinary}</div>
              <div className="text-xs text-muted-foreground font-mono mt-1">{a.commandTemplate}</div>
            </div>
            <div className="flex items-center gap-2">
              {a.isAvailable
                ? <span className="text-xs text-emerald-500">✅ 已安装</span>
                : <span className="text-xs text-muted-foreground">❌ 未找到</span>}
              {!a.isBuiltin && (
                <button onClick={() => setEditing(a)} className="text-xs text-voice">编辑</button>
              )}
            </div>
          </div>
        ))}
      </div>
      {/* 新增自定义 adapter 按钮 + 表单略——参照 ActionBarPanel 编辑表单模式 */}
    </div>
  );
}
```

- [x] **Step 2: 注册到设置页 tab**

找到设置页 tab 注册逻辑（搜索现有 ActionBarPanel 的引入位置），添加 AgentPanel tab。

- [x] **Step 3: 前端编译验证**

Run: `cd crates/desktop/frontend && npm run build 2>&1 | tail -20`
Expected: 编译通过

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx
git commit -m "feat(frontend): AgentPanel——adapter 管理设置页（检测状态 + CRUD）"
```

---

## Task 12: 端到端集成验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`（action bar 相关章节）

- [x] **Step 1: 全量编译**

Run: `cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -20`
Expected: 编译通过

- [x] **Step 2: 运行全量测试**

Run: `cargo test -p octopus-infra -p octopus-desktop --lib -- --nocapture 2>&1 | tail -30`
Expected: 全部 PASS

- [x] **Step 3: 手动验证 — Finder 选中文件触发**

1. 启动 octopus desktop
2. Finder 中选中一个文件
3. 按全局热键（action bar shortcut）
4. 验证浮窗弹出，仅显示 accepts=file/any 的菜单项

- [x] **Step 4: 手动验证 — agent 执行**

1. 设置页新增一个 agent 类型菜单项（adapter=claude，模板含 {{task}}）
2. Finder 选中文件 → 热键 → 点该 agent 项 → 输入 task → 回车
3. 验证 Terminal.app 弹出，claude 命令执行

- [x] **Step 5: 手动验证 — copy_path**

1. 设置页新增 copy_path 类型菜单项
2. Finder 选中文件 → 热键 → 点 copy_path → 粘贴验证路径格式

- [x] **Step 6: 更新 architecture.md**

在 action bar 相关章节补充 Files 场景 + agent adapter 注册表的架构说明。

- [x] **Step 7: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(architecture): 同步 action bar 文件 agent 桥接架构"
```

---

## Self-Review 记录

（实现完成后回填：每个 task 实际偏差、新增决策、删除/合并的子任务）
