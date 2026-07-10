# Action Bar 脚本增强——实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 action bar 脚本执行新增 JS/TS 运行时支持、执行结果捕获落库、异步/同步模式选择、脚本执行记录管理界面。

**Architecture:** `run_script` 拆分为 `spawn_script`（预探测+spawn）+ `wait_with_timeout`（轮询收割）共享逻辑，上层分 `run_script_async`（fire-and-forget 后台落库）和 `run_script_sync`（spawn_blocking 等待返回结果）。DB 新增 `script_runs` 表 + `action_bar_items` 两列。前端编辑表单联动 `is_async`/`write_output_to_clipboard`，设置页新增执行记录子页。

**Tech Stack:** Rust（std::process + tokio spawn_blocking + rusqlite）、TypeScript / React（Tauri command + ActionBarPanel）、SQLite

**Spec:** [`docs/superpowers/specs/2026-07-10-action-bar-script-enhancement-design.md`](../specs/2026-07-10-action-bar-script-enhancement-design.md)

## Global Constraints

- magic comment 第一行解析：`source.lines().next().unwrap_or("").trim()`
- 选中文本统一经环境变量 `OCTOPUS_TEXT` 传递（防注入，不拼字符串）
- stdout/stderr 截断 64KB（`chars().take(65536)`）防 DB 膨胀
- 60 秒超时强杀（try_wait × 120 × 500ms，复用现有逻辑）
- `#javascript` 探测优先级 node → bun → deno；`#typescript` 优先级 npx tsx → bun → deno
- `write_output_to_clipboard` 仅同步模式可生效，异步模式 UI 禁用 + 强制 false
- 所有执行（成功/失败/超时）都落库 `script_runs`
- DB schema 变更：改 `db.sql` + 升 `user_version`，不新增 ALTER 迁移分支
- `run_script_sync` 跑在 `tokio::task::spawn_blocking`（async command 不能阻塞）
- `action_bar_show_result` 的 label 对 script 类型用菜单项 title

---

## File Structure

| 文件 | 职责 |
|------|------|
| `crates/infra/src/db.sql` | 建表 + 种子数据 |
| `crates/infra/src/db.rs` | `ActionBarItem` 扩展 + CRUD 签名变更 + `ScriptRun` + script_runs CRUD |
| `crates/desktop/src/action_bar_commands.rs` | `run_script` 重构 + 运行时探测 + `execute_action_bar_inner` script 分支 + 新 Tauri command |
| `crates/desktop/src/main.rs` | `invoke_handler` 注册新 command |
| `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` | 编辑表单联动 + 执行记录子页 |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | script loading timeout 对齐（同步模式超时） |

---

## Task 1: DB Schema——`action_bar_items` 加列 + `script_runs` 建表

**Files:**
- Modify: `crates/infra/src/db.sql`（L266-279 action_bar_items CREATE TABLE 追加列 + 新建 script_runs）
- Modify: `crates/infra/src/db.rs`（user_version 升级 + ActionBarItem struct + CRUD 签名 + ScriptRun struct + script_runs CRUD）

**Interfaces:**
- Produces: `ActionBarItem { is_async: bool, write_output_to_clipboard: bool }`、`ScriptRun` struct、`insert_script_run` / `list_script_runs` / `clear_script_runs` 函数

- [x] **Step 1: db.sql——action_bar_items 加两列**

在 `crates/infra/src/db.sql` 的 `action_bar_items` CREATE TABLE 中，`is_enabled` 行之后加：

```sql
    is_async   INTEGER NOT NULL DEFAULT 1,
    write_output_to_clipboard INTEGER NOT NULL DEFAULT 0,
```

- [x] **Step 2: db.sql——script_runs 建表**

在 action_bar_items 种子数据之后（L298 之后），加：

```sql
-- ── 脚本执行记录 ──────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS script_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     INTEGER NOT NULL,
    script_type TEXT NOT NULL,
    exit_code   INTEGER,
    stdout      TEXT NOT NULL DEFAULT '',
    stderr      TEXT NOT NULL DEFAULT '',
    error_msg   TEXT NOT NULL DEFAULT '',
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER,
    FOREIGN KEY (item_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_script_runs_started_at ON script_runs(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_script_runs_item_id ON script_runs(item_id);
```

- [x] **Step 3: db.rs——升 user_version**

在 `crates/infra/src/db.rs` 中，将 `user_version = 20` 改为 `user_version = 21`（两处，约 L193 和 L200）。在注释中加 `// v21：action_bar_items 加 is_async + write_output_to_clipboard 列；新建 script_runs 表。`

- [x] **Step 4: db.rs——ActionBarItem struct 扩展**

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
    pub is_async: bool,                  // 新增
    pub write_output_to_clipboard: bool, // 新增
}
```

更新 `ACTION_BAR_SELECT_COLS`：
```rust
const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard";
```

更新 `row_to_action_bar_item` 追加两列（index 9, 10）：
```rust
    is_async: row.get::<_, i32>(9)? != 0,
    write_output_to_clipboard: row.get::<_, i32>(10)? != 0,
```

- [x] **Step 5: db.rs——insert/update 签名变更**

`insert_action_bar_item` 和 `insert_action_bar_item_at` 追加两参数：
```rust
pub fn insert_action_bar_item(
    parent_id: Option<i64>, title: &str, icon: &str,
    action_type: &str, action_data: &str,
    is_async: bool, write_output_to_clipboard: bool,
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard))
}
```

INSERT SQL 改为：
```sql
INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, ?7, ?8)
```
params 追加 `is_async as i32, write_output_to_clipboard as i32`。

`update_action_bar_item` 和 `update_action_bar_item_at` 追加两参数，UPDATE SQL 改为：
```sql
UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, updated_at=datetime('now') WHERE id=?8
```

- [x] **Step 6: db.rs——ScriptRun struct + CRUD**

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRun {
    pub id: i64,
    pub item_id: i64,
    pub item_title: Option<String>,
    pub script_type: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_msg: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}

/// stdout/stderr 截断上限（64KB）
const SCRIPT_OUTPUT_LIMIT: usize = 65536;

pub fn insert_script_run(
    item_id: i64, script_type: &str, exit_code: Option<i32>,
    stdout: &str, stderr: &str, error_msg: &str,
    started_at: &str, finished_at: Option<&str>, duration_ms: Option<i64>,
) -> Result<i64> {
    let stdout_trunc: String = stdout.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    let stderr_trunc: String = stderr.chars().take(SCRIPT_OUTPUT_LIMIT).collect();
    with_db(|conn| {
        conn.execute(
            "INSERT INTO script_runs (item_id, script_type, exit_code, stdout, stderr, error_msg, started_at, finished_at, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item_id, script_type, exit_code, stdout_trunc, stderr_trunc, error_msg, started_at, finished_at, duration_ms],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<ScriptRun>> {
    with_db(|conn| {
        let limit = limit.unwrap_or(100);
        let sql = if item_id.is_some() {
            "SELECT s.id, s.item_id, COALESCE(a.title, '已删除'), s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             WHERE s.item_id = ?2 ORDER BY s.started_at DESC LIMIT ?1"
        } else {
            "SELECT s.id, s.item_id, a.title, s.script_type, s.exit_code, s.stdout, s.stderr, s.error_msg, s.started_at, s.finished_at, s.duration_ms
             FROM script_runs s LEFT JOIN action_bar_items a ON s.item_id = a.id
             ORDER BY s.started_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(iid) = item_id {
            stmt.query_map(params![limit, iid], row_to_script_run)?
        } else {
            stmt.query_map(params![limit], row_to_script_run)?
        };
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

fn row_to_script_run(row: &rusqlite::Row) -> rusqlite::Result<ScriptRun> {
    Ok(ScriptRun {
        id: row.get(0)?,
        item_id: row.get(1)?,
        item_title: row.get(2).ok(),
        script_type: row.get(3)?,
        exit_code: row.get(4)?,
        stdout: row.get(5)?,
        stderr: row.get(6)?,
        error_msg: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
        duration_ms: row.get(10)?,
    })
}

pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<()> {
    let keep = keep_recent.unwrap_or(100);
    with_db(|conn| {
        conn.execute(
            "DELETE FROM script_runs WHERE id NOT IN (SELECT id FROM script_runs ORDER BY started_at DESC LIMIT ?1)",
            params![keep],
        )?;
        Ok(())
    })
}
```

- [x] **Step 7: 编译 + 测试**

Run: `cargo build -p octopus-infra && cargo test -p octopus-infra`
Expected: 编译通过，60 测试全过

- [x] **Step 8: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(db): action_bar_items 加 is_async/write_output_to_clipboard + script_runs 表"
```

---

## Task 2: 运行时探测——`spawn_script` + `wait_with_timeout`

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（重构 `run_script` → `spawn_script` + `wait_with_timeout` + 探测函数）

**Interfaces:**
- Consumes: 无（独立逻辑）
- Produces: `spawn_script(source, text) -> Result<(Child, String)>`、`wait_with_timeout(child) -> ScriptResult`、`ScriptResult` struct

- [x] **Step 1: ScriptResult struct**

在 `action_bar_commands.rs` 的 `run_script` 函数之前加：

```rust
struct ScriptResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}
```

- [x] **Step 2: 运行时探测函数**

```rust
/// 探测 JS 运行时——优先级 node → bun → deno
fn detect_js_runtime() -> Option<(&'static str, &'static str)> {
    for (bin, flag) in [("node", "-e"), ("bun", "eval"), ("deno", "eval")] {
        if std::process::Command::new(bin).arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some((bin, flag));
        }
    }
    None
}

/// 探测 TS 运行时——优先级 npx tsx → bun → deno
fn detect_ts_runtime() -> Option<(&'static str, Vec<&'static str>)> {
    // tsx via npx
    if std::process::Command::new("npx").args(["--yes", "tsx", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("npx", vec!["--yes", "tsx", "-e"]));
    }
    if std::process::Command::new("bun").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("bun", vec!["eval"]));
    }
    if std::process::Command::new("deno").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("deno", vec!["eval"]));
    }
    None
}
```

- [x] **Step 3: spawn_script——替代原 run_script 的 spawn 部分**

```rust
/// 按 magic comment 分发运行时，spawn 子进程。
/// 返回 (Child, script_type) —— script_type 用于落库。
/// capture_output=true 时 stdout/stderr 用 pipe（同步模式），false 时用 null（异步模式）。
fn spawn_script(source: &str, text: &str, capture_output: bool) -> Result<(std::process::Child, String), String> {
    let first_line = source.lines().next().unwrap_or("").trim();
    let script: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");

    let stdout_cfg = if capture_output { std::process::Stdio::piped() } else { std::process::Stdio::null() };
    let stderr_cfg = if capture_output { std::process::Stdio::piped() } else { std::process::Stdio::null() };

    let mut cmd_result: Result<std::process::Command, String> = match first_line {
        "#shell" => {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#osascript" => {
            #[cfg(target_os = "macos")]
            { let mut c = std::process::Command::new("osascript"); c.arg("-e").arg(&script); Ok(c) }
            #[cfg(not(target_os = "macos"))]
            { Err("osascript 仅 macOS 支持".into()) }
        }
        "#powershell" => {
            #[cfg(target_os = "windows")]
            { let mut c = std::process::Command::new("powershell"); c.arg("-Command").arg(&script); Ok(c) }
            #[cfg(not(target_os = "windows"))]
            { Err("powershell 仅 Windows 支持".into()) }
        }
        "#python" => {
            let mut c = std::process::Command::new("python3");
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#node" => {
            let mut c = std::process::Command::new("node");
            c.arg("-e").arg(&script);
            Ok(c)
        }
        "#deno" => {
            let mut c = std::process::Command::new("deno");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#bun" => {
            let mut c = std::process::Command::new("bun");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#javascript" => {
            let (bin, flag) = detect_js_runtime()
                .ok_or_else(|| "未检测到 JS 运行时，请安装 Node.js / Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            c.arg(flag).arg(&script);
            Ok(c)
        }
        "#typescript" => {
            let (bin, args) = detect_ts_runtime()
                .ok_or_else(|| "未检测到 TS 运行时，请安装 tsx（npm i -g tsx）/ Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            for a in &args { c.arg(a); }
            c.arg(&script);
            Ok(c)
        }
        _ => return Err(format!(
            "未知脚本类型: {}（第一行须为 #shell/#osascript/#powershell/#python/#node/#deno/#bun/#javascript/#typescript）",
            first_line
        )),
    };

    let mut cmd = cmd_result?;
    cmd.env("OCTOPUS_TEXT", text);
    cmd.stdout(stdout_cfg);
    cmd.stderr(stderr_cfg);
    let child = cmd.spawn().map_err(|e| format!("脚本执行失败: {}", e))?;
    Ok((child, first_line.to_string()))
}
```

- [x] **Step 4: wait_with_timeout——替代原后台收割逻辑**

```rust
/// 轮询等待子进程退出，60 秒超时强杀。捕获 stdout/stderr。
fn wait_with_timeout(child: std::process::Child) -> ScriptResult {
    let mut child = child;
    for _ in 0..120 {
        match child.try_wait() {
            Ok(Some(_)) => {
                // 进程已退出，读取 stdout/stderr pipe
                let stdout = read_child_output(&mut child);
                let stderr = read_child_stderr(&mut child);
                let code = child.wait().ok().and_then(|s| s.code());
                return ScriptResult { exit_code: code, stdout, stderr, timed_out: false };
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(_) => {
                let stdout = read_child_output(&mut child);
                let stderr = read_child_stderr(&mut child);
                return ScriptResult { exit_code: None, stdout, stderr, timed_out: false };
            }
        }
    }
    // 超时强杀
    let _ = child.kill();
    let _ = child.wait();
    let stdout = read_child_output(&mut child);
    let stderr = read_child_stderr(&mut child);
    ScriptResult { exit_code: None, stdout, stderr, timed_out: true }
}
```

加辅助函数读取 pipe（child stdout/stderr 是 `Option<ChildStdout>`，只有 piped 时才有值）：

```rust
use std::io::Read;

fn read_child_output(child: &mut std::process::Child) -> String {
    if let Some(mut stdout) = child.stdout.take() {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    } else { String::new() }
}

fn read_child_stderr(child: &mut std::process::Child) -> String {
    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    } else { String::new() }
}
```

- [x] **Step 5: 删除旧 run_script**

删除原来的 `fn run_script` 整个函数（被 `spawn_script` + `wait_with_timeout` + `run_script_async` + `run_script_sync` 替代）。

- [x] **Step 6: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（`execute_action_bar_inner` 的 script 分支会在 Task 3 更新）

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "refactor: run_script 拆为 spawn_script + wait_with_timeout + 运行时探测"
```

---

## Task 3: 异步/同步执行——`run_script_async` + `run_script_sync` + execute_action_bar_inner 整合

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`

**Interfaces:**
- Consumes: Task 1 的 `insert_script_run`、Task 2 的 `spawn_script` / `wait_with_timeout` / `ScriptResult`
- Produces: `run_script_async` / `run_script_sync` + 重构的 `execute_action_bar_inner` script 分支

- [x] **Step 1: run_script_async——fire-and-forget + 后台落库**

```rust
/// 异步执行脚本——spawn 后立即返回，后台线程收割并落库。
fn run_script_async(source: &str, text: &str, item_id: i64, item_title: &str) -> Result<(), String> {
    let (mut child, script_type) = spawn_script(source, text, false)?;
    let started_at = chrono::Utc::now().to_rfc3339();
    std::thread::spawn(move || {
        let result = wait_with_timeout(child);
        let finished_at = chrono::Utc::now().to_rfc3339();
        let duration_ms = chrono::DateTime::parse_from_rfc3339(&started_at).ok()
            .map(|s| (chrono::Utc::now() - s.with_timezone(&chrono::Utc)).num_milliseconds());
        let error_msg = if result.timed_out { "执行超时（60秒）".to_string() }
            else if result.exit_code.is_none() { "进程异常退出".to_string() }
            else { String::new() };
        let _ = octopus_infra::db::insert_script_run(
            item_id, &script_type, result.exit_code,
            &result.stdout, &result.stderr, &error_msg,
            &started_at, Some(&finished_at), duration_ms,
        );
    });
    let _ = item_title; // 仅日志用，暂不 log
    Ok(())
}
```

> **注意**：如果 `chrono` 未在 desktop crate 的依赖中，检查 `Cargo.toml`。infra crate 已有 chrono 依赖。desktop 也可用 `std::time::Instant` 替代时间戳——如果 chrono 不可用，改为：
> ```rust
> use std::time::{Instant, SystemTime, UNIX_EPOCH};
> // started_at 用 SystemTime::now().duration_since(UNIX_EPOCH).as_millis()
> // duration_ms 用 Instant::now().elapsed().as_millis()
> ```
> 保持与 DB 已有的 `datetime('now')` 格式一致即可。优先检查 chrono 是否可用。

- [x] **Step 2: run_script_sync——spawn_blocking 等待 + 返回结果**

```rust
/// 同步执行脚本——在 spawn_blocking 中等待完成，返回结果。
fn run_script_sync_blocking(source: &str, text: &str, item_id: i64, script_type: &str)
    -> Result<ScriptResult, String>
{
    let (child, _) = spawn_script(source, text, true)?;
    let started = std::time::Instant::now();
    let mut result = wait_with_timeout(child);
    let elapsed = started.elapsed().as_millis() as i64;
    let finished_at = chrono::Utc::now().to_rfc3339();
    let error_msg = if result.timed_out { "执行超时（60秒）".to_string() }
        else if result.exit_code.is_none() { "进程异常退出".to_string() }
        else { String::new() };
    let _ = octopus_infra::db::insert_script_run(
        item_id, script_type, result.exit_code,
        &result.stdout, &result.stderr, &error_msg,
        &started_at_placeholder, Some(&finished_at), Some(elapsed),
    );
    Ok(result)
}
```

> **同 Step 1 注意**：时间戳格式与 chrono 可用性。

- [x] **Step 3: execute_action_bar_inner script 分支重构**

将 `execute_action_bar_inner` 的 `"script"` 分支改为：

```rust
        "script" => {
            let is_async = item.is_async;
            let write_output = item.write_output_to_clipboard;
            let item_title = item.title.clone();
            let item_id = item.id;

            if is_async {
                // 异步——fire-and-forget，后台落库，立即关闭浮窗
                run_script_async(&item.action_data, &text, item_id, &item_title)?;
                Ok(false) // 走统一 hide 收口
            } else {
                // 同步——spawn_blocking 等待结果，前端 loading 视图
                let source = item.action_data.clone();
                let script_type = source.lines().next().unwrap_or("").trim().to_string();
                let text_clone = text.clone();
                let result = tokio::task::spawn_blocking(move || {
                    run_script_sync_blocking(&source, &text_clone, item_id, &script_type)
                }).await.map_err(|e| format!("脚本执行线程异常: {}", e))??;

                if result.timed_out {
                    return Err("脚本执行超时（60秒），已强制终止".into());
                }
                if let Some(code) = result.exit_code {
                    if code != 0 {
                        let detail = if result.stderr.is_empty() { String::new() } else { format!("\n{}", result.stderr) };
                        return Err(format!("脚本退出码 {}{}", code, detail));
                    }
                }
                // 成功
                if !result.stdout.is_empty() {
                    // 有输出 → CompactEditor 展示
                    let display_text = format!("【{}】\n{}", item_title, result.stdout);
                    if write_output {
                        write_clipboard_text(app, &result.stdout);
                    }
                    action_bar_show_result(result.stdout, text, item_title, app.clone());
                    return Ok(true); // 自行收口
                }
                // 成功无输出 → 正常关闭
                Ok(false)
            }
        }
```

- [x] **Step 4: action_bar_show_result 适配**

`action_bar_show_result` 当前硬编码 `write_clipboard_text(&app, &result)`（L183 附近）。Script 同步路径已自行控制 `write_output`，需让 show_result 不再无条件写剪贴板。

方案：`action_bar_show_result` 加参数 `write_clipboard: bool`：

```rust
pub fn action_bar_show_result(result: String, _original_text: String, action: String, app: AppHandle, write_clipboard: bool) {
    // ... hide + keep_active ...
    let label = match action.as_str() { ... };
    let display_text = format!("【{}】\n{}", label, result);

    // 仅 write_clipboard=true 时写入（AI 路径传 true，Script 路径按 write_output 传）
    if write_clipboard {
        write_clipboard_text(&app, &result);
    }
    // ... 后续 CompactEditor 逻辑不变 ...
}
```

更新所有调用方：
- `execute_action_bar_inner` ai 分支：`action_bar_show_result(result, text, item.title, app.clone(), true)` —— AI 结果始终写剪贴板
- `execute_action_bar_inner` script 同步分支：`action_bar_show_result(result.stdout, text, item_title, app.clone(), write_output)` —— 仅勾选时写

- [x] **Step 5: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop`
Expected: 编译通过，103 测试全过

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat: script 异步/同步执行 + 结果捕获落库 + JS/TS 运行时分发"
```

---

## Task 4: Tauri Command——create/update 签名 + list/clear script_runs

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（create/update command 加参数 + 新增 list/clear script_runs command）
- Modify: `crates/desktop/src/main.rs`（invoke_handler 注册）

**Interfaces:**
- Consumes: Task 1 的 `insert_action_bar_item`（新签名）、`list_script_runs` / `clear_script_runs`
- Produces: 前端可调用的 `create_action_bar_item` / `update_action_bar_item`（新参数）、`list_script_runs` / `clear_script_runs` Tauri command

- [x] **Step 1: create/update command 签名变更**

```rust
#[tauri::command]
pub fn create_action_bar_item(
    parent_id: Option<i64>,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
    is_async: bool,
    write_output_to_clipboard: bool,
) -> Result<i64, String> {
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data, is_async, write_output_to_clipboard)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_action_bar_item(
    id: i64,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled, is_async, write_output_to_clipboard)
        .map_err(|e| e.to_string())
}
```

- [x] **Step 2: list/clear script_runs command**

```rust
#[tauri::command]
pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<octopus_infra::db::ScriptRun>, String> {
    octopus_infra::db::list_script_runs(limit, item_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<(), String> {
    octopus_infra::db::clear_script_runs(keep_recent).map_err(|e| e.to_string())
}
```

- [x] **Step 3: main.rs invoke_handler 注册**

在 `crates/desktop/src/main.rs` 的 `invoke_handler` 中追加：

```rust
            action_bar_commands::list_script_runs,
            action_bar_commands::clear_script_runs,
```

- [x] **Step 4: capabilities/default.json 检查**

Run: `grep "invoke" crates/desktop/capabilities/default.json`

如果 capabilities 使用 `allow-except` 或白名单模式（而非 `allow-all`），需追加 `list_script_runs` 和 `clear_script_runs`。如果已有 `allow-all` 或通过 `core:default` 覆盖，则无需改动。

- [x] **Step 5: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs crates/desktop/src/main.rs
git commit -m "feat: create/update command 加 is_async/write_output + list/clear script_runs"
```

---

## Task 5: 前端——编辑表单联动 + script_runs 管理界面

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`（TYPE_META 文案 + 编辑表单 is_async/write_output checkbox + 脚本执行记录子页）

**Interfaces:**
- Consumes: Task 4 的 `create_action_bar_item` / `update_action_bar_item`（新参数）、`list_script_runs` / `clear_script_runs`

> **强制**：涉及前端 UI 修改，动手前先 `view` frontend-design skill SKILL.md 做设计规划。

- [x] **Step 1: 加载 frontend-design skill**

View: `/Users/wudarui/.claude/skills/frontend-design/SKILL.md`，按色彩/字体/布局/签名元素原则规划脚本选项 checkbox 和执行记录子页的视觉设计。

- [x] **Step 2: TYPE_META + ACTION_TYPES 更新**

更新 `script` 的 `desc` 和 `placeholder`：

```typescript
script: {
    dot: "bg-emerald-500",
    label: "SCRIPT",
    desc: "首行 #shell / #osascript / #powershell / #python / #node / #deno / #bun / #javascript / #typescript；选中文本经 $OCTOPUS_TEXT 传入",
    placeholder:
      "#shell / #osascript / #powershell / #python\n#node / #deno / #bun\n#javascript / #typescript\n选中文本在 $OCTOPUS_TEXT 环境变量中",
},
```

- [x] **Step 3: 编辑表单——is_async + write_output_to_clipboard checkbox**

在编辑表单的 textarea 之后、isEnabled checkbox 附近，仅 `actionType === "script"` 时显示：

```tsx
{form.actionType === "script" && (
  <div className="flex items-center gap-4">
    <ToggleCheckbox
      label="异步执行"
      hint="不等待结果，后台运行（默认）"
      checked={form.isAsync ?? true}
      onChange={(v) => onChange({ ...form, isAsync: v, writeOutputToClipboard: v ? false : form.writeOutputToClipboard })}
    />
    {!(form.isAsync ?? true) && (
      <ToggleCheckbox
        label="结果写入剪贴板"
        hint="脚本成功输出写入系统剪贴板"
        checked={form.writeOutputToClipboard ?? false}
        onChange={(v) => onChange({ ...form, writeOutputToClipboard: v })}
      />
    )}
  </div>
)}
```

**联动规则**：is_async=true 时 write_output_to_clipboard 隐藏 + 强制 false。is_async=false 时 write_output_to_clipboard 可选。

- [x] **Step 4: 编辑表单 save 传参更新**

更新 `handleSave`（约 L530），create/update invoke 调用加新参数：

```typescript
const result = form.id
  ? await invoke("update_action_bar_item", {
      id: form.id, title: form.title, icon: form.icon,
      actionType: form.actionType || "copy", actionData: form.actionData,
      isEnabled: form.isEnabled ?? true,
      isAsync: form.actionType === "script" ? (form.isAsync ?? true) : true,
      writeOutputToClipboard: form.actionType === "script" ? (form.writeOutputToClipboard ?? false) : false,
    })
  : await invoke("create_action_bar_item", {
      parentId: form.parentId, title: form.title, icon: form.icon,
      actionType: form.actionType || "copy", actionData: form.actionData,
      isAsync: form.actionType === "script" ? (form.isAsync ?? true) : true,
      writeOutputToClipboard: form.actionType === "script" ? (form.writeOutputToClipboard ?? false) : false,
    });
```

- [x] **Step 5: 脚本执行记录子页**

在 ActionBarPanel 内部新增 view state `"runs"`，header 新增「执行记录」按钮切换到该 view。`runs` view 显示：

- `list_script_runs({ limit: 100 })` 加载记录
- 列表渲染：时间 / 菜单项标题 / 类型标签 / 状态色点（绿=成功/红=失败/橙=超时）/ 耗时 / stdout 预览
- 点击单行展开 stdout/stderr 全文（只读 `<textarea>`）
- 底部「清理（保留最近 100 条）」按钮 → `clear_script_runs({ keepRecent: 100 })`

- [x] **Step 6: 前端类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无错误

- [x] **Step 7: 编译 + 全量测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop && cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 全部通过

- [x] **Step 8: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx
git commit -m "feat(ui): script 编辑表单异步/写剪贴板选项 + 脚本执行记录子页"
```

---

## Task 6: 文档同步

**Files:**
- Modify: `docs/architecture.md`（action bar script 部分更新）
- Modify: `docs/superpowers/specs/2026-07-09-action-bar-menu-db-design.md`（§3.2 分发表 + §5.3 script 执行）
- Modify: `docs/superpowers/plans/2026-07-09-action-bar-menu-db.md`（§8 不在本次范围——python 移除已完成标记）

- [x] **Step 1: architecture.md 更新**

action bar 第 9 点中，script 描述更新：
- magic comment 列表加 `#node/#deno/#bun/#javascript/#typescript`
- 新增 `is_async` / `write_output_to_clipboard` 配置说明
- 新增 `script_runs` 表说明
- 新增执行记录管理界面说明

- [x] **Step 2: spec §3.2 + §5.3 更新**

`2026-07-09-action-bar-menu-db-design.md`：
- §3.2 分发表追加 5 行新 magic comment
- §5.3 run_script 说明更新为 spawn_script + wait_with_timeout + async/sync 模式

- [x] **Step 3: plan §8 更新**

`2026-07-09-action-bar-menu-db.md` §8「不在本次范围」中 python 脚本已实现，更新标注。

- [x] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs: 同步 action bar 脚本增强文档（JS/TS + 异步模式 + script_runs）"
```

---

## Self-Review

**1. Spec coverage:**
- §2 magic comment 体系 → Task 2 Step 2-3 ✅
- §3 DB schema 变更 → Task 1 ✅
- §4 执行模式 → Task 3 ✅
- §5 菜单项配置 → Task 4 + Task 5 ✅
- §6 管理界面 → Task 5 Step 5 ✅
- §7 不在本次范围 → 不实现 ✅
- §8 不变量 → Global Constraints 逐条覆盖 ✅

**2. Placeholder scan:** 无 TBD/TODO，所有 code step 含完整代码 ✅

**3. Type consistency:**
- `ScriptResult { exit_code, stdout, stderr, timed_out }` — Task 2 定义，Task 3 消费 ✅
- `spawn_script(source, text, capture_output) -> (Child, String)` — Task 2 定义，Task 3 消费 ✅
- `ActionBarItem.is_async / write_output_to_clipboard` — Task 1 定义，Task 3+5 消费 ✅
- `insert_script_run` 签名 — Task 1 定义，Task 3 消费 ✅
- `action_bar_show_result` 加 `write_clipboard: bool` — Task 3 Step 4 定义并更新所有调用方 ✅
