# Action Bar Agent × 语音识别联动设计

> **状态**：已实现 ✅
> **日期**：2026-07-13
> **scope**：action bar agent 项需要用户输入任务时，联动语音识别（复用现有 ASR 流程），识别结果作为 task 注入 agent 命令执行
> **前置文档**：
> - [`2026-07-12-action-bar-file-agent-design.md`](./2026-07-12-action-bar-file-agent-design.md)（文件 agent 桥接设计）
> - [`2026-07-12-actionbar-app-context-design.md`](./2026-07-12-actionbar-app-context-design.md)（应用上下文采集）

---

## 1. 背景与动机

### 现状

action bar agent 项含 `{{task}}` 占位符时，前端弹一个文本输入框让用户打字输入任务。这不够自然——用户在 Finder 选了文件，想说的是「帮我整理成 PPT」，而不是切到键盘打字。

### 需求

agent 项需要用户输入任务时，**联动语音识别**：
1. 用户选 agent 项 → 自动启动录音（复用现有 ASR 完整流程）
2. Result 浮窗弹出，实时展示识别
3. 用户按 ASR 热键停止 → 识别文本作为 task
4. task + files 组合 → 渲染命令 → Terminal.app 启动 agent

### 设计原则

- **录音流程不被 agent 逻辑污染**——只多带一个 `RecordType` 枚举标签，结束时按 type 回调分流
- **agent 上下文通过 DB task 解耦**——录音只携带 task_id（轻量字符串），回调时从 DB 取回完整上下文
- **不悬空、可并行、死任务可查可清**——DB 存 task，管理界面处理

---

## 2. 设计决策汇总

| 维度 | 决策 |
|---|---|
| 录音触发 | action bar 选 agent 项后，复用现有 ASR 两阶段录音流程（prepare-record → StartRecording） |
| 录音停止 | 用户按现有 ASR 热键停止 |
| type 传递 | Rust 枚举 `RecordType`，变体携带数据：`AgentBridge { task_id }` |
| type 存放位置 | `Transcript.record_type` 字段，贯穿录音生命周期 |
| 上下文解耦 | DB agent_tasks 表存完整上下文（context JSON + prompt_template）；录音只带 task_id |
| 回调方式 | finalize_after_stop 按 record_type match 分流 |
| task 状态 | 精简 4 态：pending / executing / done / failed |
| 管理界面 | AgentPanel 底部加任务列表区 |

---

## 3. 架构设计

### 3.1 数据流总览

```
用户在 Finder 选中文件
    │
    ▼ action bar 热键 → 浮窗 → 选 agent 项（含 {{task}}）
    │
    ▼ 创建 agent_task（DB: pending + context）
    │
    ▼ 触发录音（record_type=AgentBridge { task_id }）
    │   action bar 浮窗隐藏
    │   Result 浮窗弹出，实时识别展示
    │
    ▼ 用户按 ASR 热键停止录音
    │
    ▼ finalize_after_stop
    │   match transcript.record_type {
    │     Input → 现有 paste 流程不变
    │     AgentBridge { task_id } → execute_agent_task(task_id, 识别文本)
    │   }
    │
    ▼ execute_agent_task
    │   UPDATE agent_tasks SET transcribed_text, status='executing'
    │   读回 context → 渲染 prompt → render_command → Terminal.app
    │   UPDATE agent_tasks SET status='done'
    │   隐藏 Result 窗口
    │
    ▼ Terminal.app 弹出 agent 独立窗口异步运行
```

### 3.2 RecordType 枚举

```rust
#[derive(Clone, Debug)]
pub enum RecordType {
    Input,
    AgentBridge { task_id: String },
    // 未来扩展：Translate { task_id: String, source_lang: String } 等
}
```

Rust 枚举变体携带数据，type 标签和关联 ID 不分离，match 穷尽且类型安全。

### 3.3 DB agent_tasks 表

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

**context JSON 结构**（逐步填充）：

```json
// 创建时（pending）
{
  "kind": "files",
  "files": ["/a.pdf", "/b.pdf"],
  "cwd": "/Users/x",
  "prompt_template": "{{task}}\n\n文件列表：\n{{files}}"
}

// 录音回调后（executing）
// context 不变，transcribed_text 单独写入
```

**context 字段说明**：

| 字段 | 说明 |
|---|---|
| `kind` | 选中对象类型（当前 `files`，未来可扩展） |
| `files` | POSIX 路径列表 |
| `cwd` | 首个文件父目录 |
| `prompt_template` | 从 `action_bar_items.action_data` 拷贝的模板 |

### 3.4 coordinator 改动

五个触点：

**RecordType 枚举定义**（`coordinator.rs`）：

```rust
#[derive(Clone, Debug)]
pub enum RecordType {
    Input,
    AgentBridge { task_id: String },
}
impl Default for RecordType {
    fn default() -> Self { RecordType::Input }
}
```

**Transcript 加 record_type 字段**（`transcript.rs`）：

```rust
pub struct Transcript {
    pub record_type: crate::coordinator::RecordType,  // 默认 Input
}
```

`Transcript::new()` 加 `record_type` 参数。贯穿录音全生命周期。

**start_recording / begin_recording / prepare_*_session 全链路透传 record_type**：

`Coordinator::start_recording()` 发 `Command::StartRecording { record_type: RecordType::Input }`（ASR 热键路径固定 Input）。`begin_recording()` + `prepare_streaming_session` / `prepare_vad_segmented_session` / `prepare_cloud_streaming_session` 全部加 `record_type: RecordType` 参数，`Transcript::new()` 时设置。

**StartAgentRecording Command**（跳过 prepare-record 两阶段）：

```rust
Command::StartAgentRecording { task_id: String }
```

`Coordinator::start_agent_recording(task_id)` 发送此命令。主循环处理：sync runtime config → `begin_recording(..., RecordType::AgentBridge { task_id })`。不走 prepare-record，agent 录音无 selection 需求。

**finalize_after_stop 按 record_type 分流**：

```rust
match &transcript.record_type {
    RecordType::AgentBridge { task_id } => {
        execute_agent_task(app_handle, task_id, &combined);
        *stage = Stage::Idle;
        return;
    }
    RecordType::Input => {} // 走现有 paste 流程
}
```

### 3.5 action bar → 录音联动

**新增 Tauri 命令 `trigger_agent_voice`**：

```rust
#[tauri::command]
pub fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, Coordinator>,
) -> Result<(), String> {
    // 1. 从 action_bar_items 读菜单项（agent_key, action_data）
    // 2. 从 PENDING_CONTEXT 取 files + cwd
    // 3. 组装 context JSON（kind, files, cwd, prompt_template）
    // 4. 生成 UUID task_id + INSERT agent_tasks
    // 5. 隐藏 action bar 浮窗
    // 6. coordinator.start_agent_recording(task_id)
    //    → 发 Command::StartAgentRecording → 直接 begin_recording
    //    （跳过 prepare-record 两阶段，agent 录音无 selection 需求）
}
```

> **实现偏差**：设计阶段考虑走 prepare-record 两阶段流程，实现时改为独立 `StartAgentRecording` Command——更简洁，避免前端 start_recording 命令需要感知 record_type。

**前端 ActionBar 修改**：

agent 项含 `{{task}}` 时：
```ts
// 旧：setView("task-input") 弹文本输入框
// 新：invoke("trigger_agent_voice", { itemId: item.id })
```

不含 `{{task}}` 的 agent 项：不变，直接走现有 `execute_action_bar`（无录音）。

**窗口焦点协调**：

action bar 浮窗隐藏 → Result 浮窗弹出录音，沿用现有 `FLOAT_DEPTH` 引用计数机制。

### 3.6 execute_agent_task + parse_agent_context

`execute_agent_task` 调用 `parse_agent_context`（提取为 pub 纯函数供测试）：

```rust
pub struct AgentContext {
    pub files: Vec<String>,
    pub cwd: String,
    pub prompt_template: String,
}

pub fn parse_agent_context(context_json: &str) -> AgentContext {
    // 解析 JSON，缺字段/非法 JSON 降级到默认值
}

fn execute_agent_task(app: &AppHandle, task_id: &str, transcribed_text: &str) {
    // 1. UPDATE agent_tasks SET transcribed_text, status='executing'
    // 2. load_agent_task → parse_agent_context
    // 3. render_agent_prompt → render_command → TerminalAppLauncher.spawn
    // 4. UPDATE status='done' / 'failed'
    // 5. hide_result + tray Idle
}
```

### 3.7 管理界面

在现有 AgentPanel（`AgentPanel.tsx`）底部加**任务列表区**：

| task_id（前 8 位） | 状态 | agent | 识别文本（前 20 字） | 创建时间 | 操作 |
|---|---|---|---|---|---|
| a1b2c3d4 | ✅ done | Claude Code | 帮我整理成PPT | 5 分钟前 | 删除 |
| e5f6g7h8 | ❌ failed | Pi | 制作摘要 | 10 分钟前 | 重试 · 删除 |
| i9j0k1l2 | ⏳ pending | Claude Code | — | 1 小时前 | 删除 |

- 仅查最近 50 条
- failed/done 可重试（重新组装命令 + Terminal.app 执行）

**新增 Tauri 命令**：

```rust
#[tauri::command]
fn list_agent_tasks(limit: Option<i64>) -> Vec<AgentTask>

#[tauri::command]
fn delete_agent_task(id: String)

#[tauri::command]
fn retry_agent_task(id: String)  // 重新执行 failed 的 task
```

---

## 4. 错误处理

| 场景 | 处理 |
|---|---|
| 录音后识别为空 | UPDATE status='failed', error_msg='识别结果为空'；Result 窗口 toast 提示后隐藏 |
| agent adapter 未安装 | UPDATE status='failed', error_msg='agent 未安装'；toast 提示 |
| Terminal.app 启动失败 | UPDATE status='failed', error_msg |
| task_id 在 DB 中不存在（异常） | 日志 warn，静默降级为 Input 路径（走 paste） |

---

## 5. 不做的（YAGNI）

- ❌ task 超时自动清理（一期手动删除）
- ❌ task 执行进度追踪（agent 在终端异步运行，octopus 不管结果）
- ❌ 并发 task 数量限制
- ❌ 文本输入框完全移除（不含 `{{task}}` 的 agent 项仍走现有 execute_action_bar；未来可考虑保留文本输入作为 fallback）

---

## 6. 文件变更清单

**新建文件：**
无（功能嵌入现有模块）

**修改文件：**

| 文件 | 变更 |
|---|---|
| `crates/infra/src/db.sql` | 新增 agent_tasks 表 DDL |
| `crates/infra/src/db.rs` | v26→v27 迁移；agent_tasks CRUD |
| `crates/desktop/src/coordinator.rs` | Transcript 加 record_type；start_recording 加参数；finalize 按 record_type 分流；execute_agent_task 函数 |
| `crates/desktop/src/action_bar_commands.rs` | trigger_agent_voice 命令；list/delete/retry_agent_task 命令 |
| `crates/desktop/src/main.rs` | invoke_handler 注册新命令 |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | agent 含 {{task}} 时调 trigger_agent_voice 替代弹输入框 |
| `crates/desktop/frontend/src/pages/Settings/AgentPanel.tsx` | 底部加任务列表区 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | 新增 i18n 键 |
| `crates/desktop/frontend/src/locales/en.yaml` | 新增 i18n 键 |

---

## 7. 未来扩展

- **RecordType 新增变体**：如 `Translate { task_id }`，录音结束后自动翻译
- **context.kind 扩展**：如 `text`（选中文本场景）
- **task 超时自动清理**：后台定时扫描 pending 超时任务
- **task 执行进度感知**：内置终端替代 Terminal.app，可截取 agent 输出
