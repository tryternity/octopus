# Action Bar Agent × 语音识别联动 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** agent 项含 `{{task}}` 时联动语音录音（复用 ASR 流程），识别结果作为 task 注入 agent 命令执行。

**Architecture:** DB agent_tasks 表存完整上下文；录音只带 `RecordType::AgentBridge { task_id }`；finalize 按 record_type match 分流；execute_agent_task 独立函数从 DB 取上下文执行。

**Tech Stack:** Rust + Tauri 2 + SQLite (rusqlite) + React + TypeScript

**Spec:** [`docs/superpowers/specs/2026-07-13-action-bar-agent-voice-design.md`](../specs/2026-07-13-action-bar-agent-voice-design.md)

## Global Constraints

- DB 迁移：当前 v26 → v27。
- coordinator 改动最小侵入——Transcript 加字段 + start_recording 加参数 + finalize 分流。
- RecordType 枚举定义在 coordinator.rs（与 Stage/Command 同文件）。
- 新增 Tauri 命令在 `main.rs` 的 `invoke_handler` 注册。
- 注释和文档用中文。

---

## Task 1: DB agent_tasks 表 + 迁移

**Files:**
- Modify: `crates/infra/src/db.sql`（追加 agent_tasks DDL）
- Modify: `crates/infra/src/db.rs`（v26→v27 迁移）

- [x] **Step 1: db.sql 追加 agent_tasks 表**

在 db.sql 末尾追加：

```sql
CREATE TABLE IF NOT EXISTS agent_tasks (
    id               TEXT PRIMARY KEY,
    status           TEXT NOT NULL DEFAULT 'pending',
    agent_key        TEXT NOT NULL,
    context          TEXT NOT NULL DEFAULT '{}',
    transcribed_text TEXT NOT NULL DEFAULT '',
    error_msg        TEXT NOT NULL DEFAULT '',
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [x] **Step 2: db.rs v26→v27 迁移**

在 v25→v26 迁移块之后、`return Ok(())` 之前，插入：

```rust
        // v26→v27：agent_tasks 表（action bar agent × 语音识别联动）
        {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS agent_tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL DEFAULT 'pending', agent_key TEXT NOT NULL, context TEXT NOT NULL DEFAULT '{}', transcribed_text TEXT NOT NULL DEFAULT '', error_msg TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
                [],
            )?;
            conn.execute("PRAGMA user_version = 27", [])?;
            log::info!("schema upgraded to v27 (agent_tasks table)");
        }
```

同步更新 `if v >= 26` → `if v >= 27`，以及全新安装 `PRAGMA user_version = 27`。

- [x] **Step 3: 更新既有测试中的 user_version 断言**

`init_schema_fresh_db_builds_v25` 中 `assert_eq!(v, 26...)` → `27`。
`init_schema_v25_is_noop` 中 `PRAGMA user_version = 27` + `assert_eq!(v, 27)`。
`migrate_v22_hotwords_to_general_set` 中 `assert_eq!(v, 26)` → `27`。

- [x] **Step 4: 补迁移测试**

```rust
#[test]
fn migration_v26_to_v27_creates_agent_tasks_table() {
    let conn = Connection::open_in_memory().unwrap();
    // 建 v26 库（有 action_bar_items + agent_adapters，无 agent_tasks）
    conn.execute_batch(INIT_SQL).unwrap();
    // INIT_SQL 已含 agent_tasks（db.sql 已更新），删除它模拟 v26
    conn.execute("DROP TABLE agent_tasks").unwrap();
    conn.execute("PRAGMA user_version = 26", []).unwrap();
    // 运行迁移
    init_schema(&conn).unwrap();
    let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 27);
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_tasks'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 1);
}
```

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-infra --lib -- --nocapture`
Expected: PASS

- [x] **Step 6: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(db): v27 迁移——agent_tasks 表"
```

---

## Task 2: agent_tasks CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（在 agent_adapters CRUD 之后追加）

- [x] **Step 1: 定义 struct + CRUD 函数**

```rust
// ── Agent Task（agent × 语音识别联动）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub status: String,
    pub agent_key: String,
    pub context: String,
    pub transcribed_text: String,
    pub error_msg: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn insert_agent_task(id: &str, agent_key: &str, context: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO agent_tasks (id, status, agent_key, context) VALUES (?1, 'pending', ?2, ?3)",
            params![id, agent_key, context],
        )?;
        Ok(())
    })
}

pub fn load_agent_task(id: &str) -> Result<Option<AgentTask>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks WHERE id=?1"
        )?;
        let mut rows = stmt.query_map(params![id], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

pub fn update_agent_task_result(id: &str, transcribed_text: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET transcribed_text=?1, status='executing', updated_at=datetime('now') WHERE id=?2",
            params![transcribed_text, id],
        )?;
        Ok(())
    })
}

pub fn update_agent_task_status(id: &str, status: &str, error_msg: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET status=?1, error_msg=?2, updated_at=datetime('now') WHERE id=?3",
            params![status, error_msg, id],
        )?;
        Ok(())
    })
}

pub fn list_agent_tasks(limit: i64) -> Result<Vec<AgentTask>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks ORDER BY created_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn delete_agent_task(id: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM agent_tasks WHERE id=?1", params![id])?;
        Ok(())
    })
}
```

- [x] **Step 2: 补 CRUD 往返测试**

```rust
#[test]
fn agent_task_crud_roundtrip() {
    let conn = open_init();
    conn.execute(
        "INSERT INTO agent_tasks (id, agent_key, context) VALUES ('test-1', 'claude', '{}')",
        [],
    ).unwrap();
    // load
    let row: Vec<(String, String, String)> = conn.prepare(
        "SELECT id, status, agent_key FROM agent_tasks WHERE id='test-1'"
    ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
    .filter_map(|r| r.ok()).collect();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].0, "test-1");
    assert_eq!(row[0].1, "pending");
    assert_eq!(row[0].2, "claude");
    // update result
    conn.execute("UPDATE agent_tasks SET transcribed_text='hello', status='executing' WHERE id='test-1'", []).unwrap();
    let text: String = conn.query_row("SELECT transcribed_text FROM agent_tasks WHERE id='test-1'", [], |r| r.get(0)).unwrap();
    assert_eq!(text, "hello");
    // delete
    conn.execute("DELETE FROM agent_tasks WHERE id='test-1'", []).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_tasks", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
}
```

- [x] **Step 3: 运行测试 + Commit**

```bash
cargo test -p octopus-infra --lib -- agent_task -- --nocapture
git add crates/infra/src/db.rs
git commit -m "feat(db): agent_tasks 表 CRUD"
```

---

## Task 3: RecordType 枚举 + Transcript 加字段

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（枚举定义 + Transcript 引用）
- Modify: `crates/desktop/src/transcript.rs`（加字段 + new() 默认值）

- [x] **Step 1: 在 coordinator.rs 定义 RecordType**

在 `Command` 枚举之前定义：

```rust
/// 录音类型——决定录音结束后 finalize 的回调路径。
#[derive(Clone, Debug)]
pub enum RecordType {
    /// 普通语音输入 → paste/剪贴板
    Input,
    /// agent 桥接 → 录音结果作为 task 注入 agent 命令
    AgentBridge { task_id: String },
    // 未来扩展：Translate { task_id: String } 等
}

impl Default for RecordType {
    fn default() -> Self { RecordType::Input }
}
```

- [x] **Step 2: Transcript 加 record_type 字段**

`crates/desktop/src/transcript.rs`，在 `pub id: i64,` 之后加：

```rust
    /// 录音类型——finalize 时按 type 分流回调。
    pub record_type: crate::coordinator::RecordType,
```

注意：Transcript 在 transcript.rs，RecordType 在 coordinator.rs。为避免循环依赖，RecordType 加 `pub` 且 transcript.rs 引用 `crate::coordinator::RecordType`。coordinator.rs 已 `use crate::transcript::Transcript`，不反向引用，无循环。

`Transcript::new()` 加 `record_type` 参数：

```rust
pub fn new(id: i64, mode: PolishMode, record_type: crate::coordinator::RecordType) -> Self {
    Self {
        id, record_type, mode, segments: Vec::new(), caret_gap: 0,
        // ... 其余不变 ...
    }
}
```

- [x] **Step 3: 修复所有 Transcript::new 调用点**

在 coordinator.rs 中搜索 `Transcript::new(` 调用，全部补 `RecordType::Input` 参数（现有录音都是 Input）。关键位置：

- `begin_recording` 函数内的 `Transcript::new(id, mode)` → `Transcript::new(id, mode, record_type)`
- begin_recording 需要接受 `record_type: RecordType` 参数并传递
- 其他位置（如 `commit_edit_apply` 中的 `Transcript::new`）传 `RecordType::Input`（编辑态不是录音）

- [x] **Step 4: begin_recording 加 record_type 参数**

```rust
#[allow(clippy::too_many_arguments)]
fn begin_recording(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    use_streaming: bool,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,  // 新增
    #[cfg(feature = "cloud")] use_cloud_streaming: bool,
) {
```

`Transcript::new(id, mode)` → `Transcript::new(id, mode, record_type)`。

所有调用 `begin_recording` 的位置补 `RecordType::Input`（StartRecording、FallbackStart 分支）。后续 Task 5 会从 start_recording 命令传 AgentBridge。

- [x] **Step 5: 编译验证**

Run: `cargo check -p octopus-desktop 2>&1 | grep "^error" | head -10`
Expected: 无 error（warnings 可接受）

- [x] **Step 6: 运行现有测试确认无回归**

Run: `cargo test -p octopus-desktop --bin octopus-desktop 2>&1 | grep "test result"`
Expected: PASS

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/transcript.rs
git commit -m "feat(coordinator): RecordType 枚举 + Transcript 加 record_type 字段"
```

---

## Task 4: start_recording 命令加 record_type 参数

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

**Interfaces:**
- Consumes: Task 3 的 RecordType
- Produces: start_recording 带 record_type，begin_recording 透传

- [x] **Step 1: Command::StartRecording 加 record_type 字段**

```rust
    StartRecording {
        prepare_id: i64,
        selection: Option<(String, usize, usize)>,
        record_type: RecordType,  // 新增
    },
```

同步更新 `Command::FallbackStart`——FallbackStart 不从外部来，加默认 `RecordType::Input`？不——FallbackStart 需要知道是什么类型的录音。方案：FallbackStart 也带 record_type。

```rust
    FallbackStart { prepare_id: i64, record_type: RecordType },
```

- [x] **Step 2: Coordinator::start_recording 加参数**

```rust
pub fn start_recording(
    &self,
    prepare_id: i64,
    selection: Option<(String, usize, usize)>,
    record_type: RecordType,
) {
    let tx = self.tx.lock();
    if tx.send(Command::StartRecording { prepare_id, selection, record_type }).is_err() {
        error!("Coordinator channel closed");
    }
}
```

- [x] **Step 3: 主循环 match 分支更新**

`Command::StartRecording` 分支：从 cmd 取出 record_type 传给 begin_recording。
`Command::FallbackStart` 分支：同上。

看门狗 spawn 的 `FallbackStart` 也需带 record_type：

```rust
let _ = tx_clone.send(Command::FallbackStart { prepare_id, record_type: record_type_clone });
```

这要求 Toggle 处理 Idle 分支时知道 record_type。但 Toggle 来自 ASR 热键，固定 Input。action bar 触发的录音不走 Toggle——走 start_recording 命令。所以 Toggle 路径的 record_type 固定 Input，start_recording 命令路径可携带 AgentBridge。

在主循环 Toggle → Idle 分支中，record_type 固定 `RecordType::Input`：

```rust
// Toggle Idle 分支的看门狗
let record_type = RecordType::Input;
let _ = tx_clone.send(Command::FallbackStart { prepare_id, record_type: record_type.clone() });
```

- [x] **Step 4: Tauri 命令 start_recording 加参数**

```rust
#[tauri::command]
pub fn start_recording(
    coordinator: tauri::State<'_, Coordinator>,
    prepare_id: i64,
    selection: Option<(String, usize, usize)>,
    record_type: Option<String>,  // 前端传 "input" / "agent-bridge" + task_id
) {
    // 解析 record_type——前端传 JSON 字符串
    let rt = match record_type.as_deref() {
        Some(s) if s.starts_with("agent-bridge:") => {
            RecordType::AgentBridge { task_id: s["agent-bridge:".len()..].to_string() }
        }
        _ => RecordType::Input,
    };
    coordinator.start_recording(prepare_id, selection, rt);
}
```

等一下——前端通过 Tauri invoke 传参，复杂枚举不好序列化。更简洁方案：前端不传 record_type，后端 `trigger_agent_voice` 直接调 `coordinator.start_recording()`（Rust 内部调用，不走 Tauri command 层），传 `RecordType::AgentBridge { task_id }`。

所以 Tauri command `start_recording`（前端 ASR 热键响应用）仍传 `RecordType::Input`：

```rust
#[tauri::command]
pub fn start_recording(
    coordinator: tauri::State<'_, Coordinator>,
    prepare_id: i64,
    selection: Option<(String, usize, usize)>,
) {
    coordinator.start_recording(prepare_id, selection, RecordType::Input);
}
```

`trigger_agent_voice` 内部直接调 `coordinator.start_recording(prepare_id, None, RecordType::AgentBridge { task_id })`——但这需要 coordinator 的 AppHandle 可访问。coordinator 在 `app.manage()` 后可从 `app.state::<Coordinator>()` 获取。

- [x] **Step 5: 编译 + 测试 + Commit**

```bash
cargo check -p octopus-desktop
cargo test -p octopus-desktop --bin octopus-desktop
git add crates/desktop/src/coordinator.rs
git commit -m "feat(coordinator): start_recording 加 RecordType 参数"
```

---

## Task 5: finalize_after_stop 按 record_type 分流 + execute_agent_task

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: execute_agent_task 函数**

在 `finalize_after_stop` 之后定义：

```rust
/// agent task 执行器：从 DB 取上下文 + 识别文本 → 渲染命令 → Terminal.app
fn execute_agent_task(app_handle: &tauri::AppHandle, task_id: &str, transcribed_text: &str) {
    // 1. 写入识别结果 + 状态 → executing
    if let Err(e) = octopus_infra::db::update_agent_task_result(task_id, transcribed_text) {
        log::error!("[agent-task] 更新 task 失败: {}", e);
        return;
    }

    // 2. 读回完整 task
    let task = match octopus_infra::db::load_agent_task(task_id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            log::warn!("[agent-task] task {} 不存在", task_id);
            return;
        }
        Err(e) => {
            log::error!("[agent-task] 加载 task 失败: {}", e);
            return;
        }
    };

    // 3. 解析 context JSON
    let context: serde_json::Value = serde_json::from_str(&task.context).unwrap_or(serde_json::json!({}));
    let files: Vec<String> = context["files"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let cwd = context["cwd"].as_str().unwrap_or("/tmp").to_string();
    let prompt_template = context["prompt_template"].as_str().unwrap_or("").to_string();

    // 4. 渲染 prompt
    let prompt = crate::action_bar_commands::render_agent_prompt(
        &prompt_template, transcribed_text, &files,
    );

    // 5. 查 adapter
    let adapters = crate::agent_adapter::list_adapters();
    let adapter = match adapters.into_iter().find(|a| a.key == task.agent_key) {
        Some(a) => a,
        None => {
            let msg = format!("Agent adapter '{}' 不存在", task.agent_key);
            let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", &msg);
            crate::result_window::show_result(app_handle, &format!("❌ {}", msg));
            return;
        }
    };
    if !adapter.is_available {
        let msg = format!("{} 未安装", adapter.display_name);
        let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", &msg);
        crate::result_window::show_result(app_handle, &format!("❌ {}", msg));
        return;
    }

    // 6. 渲染命令 + Terminal.app 启动
    let command = crate::agent_adapter::render_command(
        &adapter.command_template, &prompt, &files, &cwd,
    );
    let launcher = crate::terminal_launcher::TerminalAppLauncher;
    use crate::terminal_launcher::TerminalLauncher;
    match launcher.spawn(&command, std::path::Path::new(&cwd)) {
        Ok(()) => {
            let _ = octopus_infra::db::update_agent_task_status(task_id, "done", "");
        }
        Err(e) => {
            let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", &e);
            crate::result_window::show_result(app_handle, &format!("❌ Terminal 启动失败: {}", e));
        }
    }

    // 7. 隐藏 Result 窗口
    crate::result_window::hide_result(app_handle);
    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
}
```

- [x] **Step 2: finalize_after_stop 按 record_type 分流**

在 `finalize_after_stop` 函数中，找到现有的 paste/show_result 逻辑。在润色完成 + 句末标点补全之后、paste 之前，插入 record_type 分流。

现有代码大致结构（简化）：
```rust
// ... combined 文本 ...
crate::result_window::show_result(app_handle, &transcript.display_text());
// ... paste ...
```

改为：

```rust
    match &transcript.record_type {
        RecordType::Input => {
            // 现有逻辑：show_result + paste + DB 落库
            crate::result_window::show_result(app_handle, &transcript.display_text());
            // ... 现有 paste 逻辑 ...
        }
        RecordType::AgentBridge { task_id } => {
            // 空文本 → failed
            if combined.is_empty() {
                let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", "识别结果为空");
                crate::result_window::hide_result(app_handle);
                crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                return;
            }
            execute_agent_task(app_handle, task_id, &combined);
        }
    }
    *stage = Stage::Idle;
    return;
```

注意：finalize_after_stop 有多个 return 点（空文本、润色 pending 等）。record_type 分流只在最终 combined 文本确定后的那个路径。需要仔细阅读现有代码，在正确位置插入 match。

- [x] **Step 3: 编译 + 测试 + Commit**

```bash
cargo check -p octopus-desktop
cargo test -p octopus-desktop --bin octopus-desktop
git add crates/desktop/src/coordinator.rs
git commit -m "feat(coordinator): finalize 按 record_type 分流 + execute_agent_task"
```

---

## Task 6: trigger_agent_voice 命令

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [x] **Step 1: trigger_agent_voice 命令**

在 action_bar_commands.rs 追加：

```rust
/// agent 项含 {{task}} 时：创建 agent_task → 触发音录。
#[tauri::command]
pub fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, crate::coordinator::Coordinator>,
) -> Result<(), String> {
    // 1. 读菜单项
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    // 2. 从 PENDING_CONTEXT 取 files
    let files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();

    // 3. 组装 context JSON
    let cwd = crate::action_bar_commands::derive_cwd(&files);
    let context = serde_json::json!({
        "kind": "files",
        "files": files,
        "cwd": cwd,
        "prompt_template": item.action_data,
    }).to_string();

    // 4. 生成 task_id + 写 DB
    let task_id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::insert_agent_task(&task_id, &item.agent, &context)
        .map_err(|e| e.to_string())?;

    // 5. 隐藏 action bar 浮窗
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        let _ = win.hide();
    }
    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide(&app); }
    finalize_action_bar(&app);

    // 6. 触发录音（prepare-record 两阶段流程）
    let prepare_id = chrono::Utc::now().timestamp_millis();
    // emit prepare-record 让前端响应（与 Toggle Idle 分支一致的流程）
    let _ = app.emit("prepare-record", prepare_id);
    // 看门狗：200ms 后 FallbackStart
    let tx Coordinator 的 channel... 
```

等一下——trigger_agent_voice 需要走和 Toggle Idle 一样的两阶段流程（emit prepare-record → 前端响应 → start_recording），但 start_recording 要传 `RecordType::AgentBridge`。问题是前端的 `start_recording` Tauri 命令传的是 `RecordType::Input`。

解决方案：trigger_agent_voice 不走前端两阶段，直接调 coordinator 的内部方法 start_recording（跳过 prepare-record 前端交互）。agent 录音不需要 selection（没有跨会话选区需求），可以直接开始：

```rust
    // 6. 直接触发录音（跳过 prepare-record 两阶段——agent 录音无 selection 需求）
    coordinator.start_recording(now_millis(), None, RecordType::AgentBridge { task_id });
```

但 start_recording 发的是 Command::StartRecording，主循环校验 pending_prepare。trigger_agent_voice 没设 pending_prepare → 会被丢弃。

正确方案：trigger_agent_voice 直接发 Command，绕过 prepare_record 机制。加一个新 Command 或修改流程。

最简方案：trigger_agent_voice 直接调 `begin_recording` 的入口——但 begin_recording 是 coordinator 内部函数。

实际最简方案：trigger_agent_voice 设 pending_prepare + emit prepare-record + 看门狗（同 Toggle Idle），但在 coordinator 内部调用时传 AgentBridge record_type。前端响应 prepare-record 后调 start_recording（Input）→ 校验 prepare_id 通过 → begin_recording，但此时 record_type 信息丢了。

更好的方案：**pending_prepare 存 RecordType**。改 `pending_prepare: Option<i64>` 为 `pending_prepare: Option<(i64, RecordType)>`。

trigger_agent_voice 内部：
```rust
// 直接在主线程投递 Toggle——不行，Toggle 固定 Input。
```

最干净方案：**新增 Command::StartAgentRecording { task_id }**，主循环直接处理，不走 prepare-record：

```rust
    StartAgentRecording { task_id: String },
```

主循环处理：
```rust
    Command::StartAgentRecording { task_id } => {
        // 同 begin_recording，但 record_type = AgentBridge
        // sync runtime config（同 Toggle Idle）
        // begin_recording(..., RecordType::AgentBridge { task_id })
    }
```

trigger_agent_voice：
```rust
    coordinator.send(Command::StartAgentRecording { task_id });
```

Coordinator 加一个 pub 方法 `start_agent_recording(&self, task_id: String)`。

- [x] **Step 2: Coordinator::start_agent_recording 方法**

```rust
pub fn start_agent_recording(&self, task_id: String) {
    let tx = self.tx.lock();
    if tx.send(Command::StartAgentRecording { task_id }).is_err() {
        error!("Coordinator channel closed");
    }
}
```

- [x] **Step 3: 主循环处理 StartAgentRecording**

在主循环 match 中，复制 Toggle Idle 分支的 runtime sync 逻辑，但 record_type 传 AgentBridge：

```rust
    Command::StartAgentRecording { task_id } => {
        if !matches!(stage, Stage::Idle) {
            warn!("StartAgentRecording ignored: not Idle");
            continue;
        }
        let rc = runtime_config.read();
        config.asr_engine = match octopus_asr_local::config::resolve_active_engine(&rc.asr_engine) {
            Ok(_) => rc.asr_engine.clone(),
            Err(_) => "local:zipformer:zipformer-small-ctc".to_string(),
        };
        config.microphone = rc.microphone.clone();
        config.engine_mode = rc.engine_mode.clone();
        sync_runtime_fields(&mut config, &rc);
        drop(rc);
        use_streaming = config.engine_mode == "embedded"
            && crate::config::is_streaming_engine(&config);
        #[cfg(feature = "cloud")]
        {
            use_cloud_streaming = is_cloud_engine(&config);
            if use_cloud_streaming { use_streaming = false; }
        }
        begin_recording(
            &mut stage, &audio, &engine, &config, &app_handle, &tx,
            use_streaming, None,
            RecordType::AgentBridge { task_id },
            #[cfg(feature = "cloud")] use_cloud_streaming,
        );
    }
```

- [x] **Step 4: trigger_agent_voice 完整实现**

```rust
#[tauri::command]
pub fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, crate::coordinator::Coordinator>,
) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    let files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();

    let cwd = derive_cwd(&files);
    let context = serde_json::json!({
        "kind": "files",
        "files": files,
        "cwd": cwd,
        "prompt_template": item.action_data,
    }).to_string();

    let task_id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::insert_agent_task(&task_id, &item.agent, &context)
        .map_err(|e| e.to_string())?;

    // 隐藏 action bar 浮窗
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        let _ = win.hide();
    }
    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide(&app); }
    finalize_action_bar(&app);

    // 触发 agent 录音
    coordinator.start_agent_recording(task_id);
    Ok(())
}
```

- [x] **Step 5: main.rs 注册命令**

invoke_handler 加：

```rust
            action_bar_commands::trigger_agent_voice,
```

- [x] **Step 6: 添加 uuid 依赖（如未有）**

检查 Cargo.toml 是否有 uuid。如无，加：

```toml
uuid = { version = "1", features = ["v4"] }
```

- [x] **Step 7: list/delete/retry agent_tasks 命令**

在 action_bar_commands.rs 追加：

```rust
#[tauri::command]
pub fn list_agent_tasks(limit: Option<i64>) -> Result<Vec<octopus_infra::db::AgentTask>, String> {
    octopus_infra::db::list_agent_tasks(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_agent_task(id: String) -> Result<(), String> {
    octopus_infra::db::delete_agent_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn retry_agent_task(id: String, app: AppHandle) -> Result<(), String> {
    let task = octopus_infra::db::load_agent_task(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task 不存在")?;
    if task.status != "failed" && task.status != "done" {
        return Err("仅 failed/done 状态可重试".into());
    }
    // 用已有的 transcribed_text 重新执行
    crate::coordinator::retry_agent_task(&app, &id);
    Ok(())
}
```

retry 在 coordinator 里实现（复用 execute_agent_task）：

```rust
pub fn retry_agent_task(app_handle: &tauri::AppHandle, task_id: &str) {
    let task = match octopus_infra::db::load_agent_task(task_id) {
        Ok(Some(t)) => t,
        _ => return,
    };
    execute_agent_task(app_handle, task_id, &task.transcribed_text);
}
```

main.rs 注册：

```rust
            action_bar_commands::list_agent_tasks,
            action_bar_commands::delete_agent_task,
            action_bar_commands::retry_agent_task,
```

- [x] **Step 8: 编译 + 测试 + Commit**

```bash
cargo check -p octopus-desktop
cargo test -p octopus-desktop --bin octopus-desktop
git add -A
git commit -m "feat(agent-voice): trigger_agent_voice + task 管理命令"
```

---

## Task 7: 前端 ActionBar 联动

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`

- [x] **Step 1: agent 含 {{task}} 时调 trigger_agent_voice**

找到前端 `executeItem` 中 agent 含 `{{task}}` 分支：

```ts
    // 旧：setView("task-input")
    // 新：
    if (item.actionData.includes("{{task}}")) {
      setView("loading");
      try {
        await invoke("trigger_agent_voice", { itemId: item.id });
      } catch (e) {
        showQuickError(String(e).slice(0, 40));
        setView("main");
      }
      return;
    }
```

- [x] **Step 2: 移除 task-input 视图（不再需要文本输入框）**

移除 `taskInput`/`taskItem`/`submitTask` 相关 state 和 JSX。注意保留 `View` 类型中的 `"task-input"`（或移除）。

等一下——文本输入框完全移除？spec 说「不含 {{task}} 的 agent 项仍走现有 execute_action_bar」。含 {{task}} 的全走语音。但用户可能不想每次都说——如果环境安静怎么办？

保留文本输入作为 fallback：按某个键从 loading 切回文本输入。但 spec 说不做。

**决策**：一期含 {{task}} 全走语音，移除 task-input。如需 fallback 后续再加。

- [x] **Step 3: 编译 + Commit**

```bash
cd crates/desktop/frontend && node_modules/.bin/tsc --noEmit
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "feat(frontend): agent 含 {{task}} 时联动语音录音替代文本输入框"
```

---

## Task 8: AgentPanel 任务列表区 + i18n

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`
- Modify: `crates/desktop/frontend/src/locales/en.yaml`

- [x] **Step 1: AgentPanel 加任务列表区**

在 adapter 列表之后加一个任务列表区，查 `list_agent_tasks`，支持删除和重试。

```tsx
// AgentPanel 底部
<div className="space-y-2">
  <h3 className="text-sm font-medium">{t("agentPanel.tasksTitle")}</h3>
  {tasks.map((task) => (
    <div key={task.id} className="flex items-center gap-3 rounded-lg border border-border p-3">
      <span className={cn("h-1.5 w-1.5 rounded-full",
        task.status === "done" ? "bg-emerald-500"
        : task.status === "failed" ? "bg-red-500"
        : task.status === "executing" ? "bg-sky-500"
        : "bg-muted-foreground")} />
      <span className="font-mono text-[10px] text-muted-foreground">{task.id.slice(0, 8)}</span>
      <span className="text-xs">{task.agentKey}</span>
      <span className="flex-1 truncate text-xs text-muted-foreground">{task.transcribedText || "—"}</span>
      <span className="text-[10px] text-muted-foreground">{task.status}</span>
      {task.status === "failed" && (
        <button onClick={() => retryTask(task.id)} className="text-[10px] text-voice hover:underline">
          {t("agentPanel.retry")}
        </button>
      )}
      <button onClick={() => deleteTask(task.id)} className="text-[10px] text-muted-foreground hover:text-red-500">
        ✕
      </button>
    </div>
  ))}
</div>
```

- [x] **Step 2: i18n 键**

zh-CN:
```yaml
    tasksTitle: 任务列表
    retry: 重试
    taskStatusPending: 等待中
    taskStatusExecuting: 执行中
    taskStatusDone: 已完成
    taskStatusFailed: 失败
```

en:
```yaml
    tasksTitle: Tasks
    retry: Retry
    taskStatusPending: Pending
    taskStatusExecuting: Executing
    taskStatusDone: Done
    taskStatusFailed: Failed
```

- [x] **Step 3: 编译 + Commit**

```bash
cd crates/desktop/frontend && node_modules/.bin/tsc --noEmit
git add -A
git commit -m "feat(frontend): AgentPanel 任务列表区 + i18n"
```

---

## Task 9: 端到端验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 全量编译**

Run: `cargo check -p octopus-desktop 2>&1 | grep "^error"`
Expected: 无 error

- [x] **Step 2: 全量测试**

Run: `cargo test -p octopus-infra --lib && cargo test -p octopus-desktop --bin octopus-desktop`
Expected: 全 PASS

- [x] **Step 3: 更新 architecture.md**

在文件 agent 桥接章节后追加语音联动说明。

- [x] **Step 4: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(architecture): 同步 agent × 语音识别联动架构"
```

---

## Self-Review 记录

（实现完成后回填）
