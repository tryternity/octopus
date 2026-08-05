# 粘贴队列（Paste Stack）设计

- 日期：2026-08-05
- 分支：`feat/paste-stack`
- 类型：新功能（剪贴板增强）
- 依赖：`paste_clipboard_item` 命令（已有）、`clipboard_shortcut` 全局热键体系（已有）

## 1. 背景与动机

### 1.1 场景

批量填表 / 批量录入——用户有多段文本要按顺序粘贴到不同位置（表单字段、文档段落、终端命令序列）。当前只能一条条从剪贴板窗口双击粘贴，每次都要切回窗口选下一条。

### 1.2 竞品参考

- **Ditto**（5 buffers）：固定 5 个缓冲槽，热键粘贴
- **ortu**（paste stack）：选中多条入栈，热键逐条出栈
- **VloamClip**（FIFO/LIFO 队列）：队列可视化 + 方向切换

### 1.3 设计约束

| 决策 | 结论 | 理由 |
|---|---|---|
| 交互模型 | **Stack 模式**（入栈 → 热键逐条弹出粘贴） | 最符合批量填表心智；不需要额外 UI 面板 |
| 出栈方向 | **FIFO only**（先入先出 = 列表顺序） | 最直觉；LIFO 后续可加 |
| 持久化 | **不持久化**（内存 Mutex<VecDeque>，重启清空） | 粘贴队列是临时操作流，重启后无意义 |
| 多选交互 | **Cmd+点击**多选 + 绿色高亮 + 序号标号 | macOS 原生多选范式 |
| 入栈触发 | 剪贴板窗口「入栈」按钮 | 显式操作，避免误触 |
| 出栈触发 | **全局热键 Cmd+Shift+V** | 用户已切到目标应用，需要全局热键 |

## 2. 数据结构

```rust
// crates/desktop/src/clipboard/paste_stack.rs
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

struct PasteStack {
    items: VecDeque<String>,  // history_id 队列（FIFO：front 先出）
}

static PASTE_STACK: OnceLock<Mutex<PasteStack>> = OnceLock::new();
```

**不进 DB**——重启清空是合理的（粘贴队列是临时操作流）。

## 3. 后端 API

### 3.1 Tauri 命令

| 命令 | 签名 | 说明 |
|---|---|---|
| `push_to_paste_stack` | `(ids: Vec<String>) -> Result<usize>` | 入栈（按传入顺序），返回栈大小 |
| `pop_and_paste` | `(app, handle, focus) -> Result<bool>` | 弹出栈底 + 写剪贴板 + 模拟 Cmd+V；返回 false=栈空 |
| `clear_paste_stack` | `() -> ()` | 清空栈 |
| `paste_stack_status` | `() -> PasteStackStatus` | 查询栈状态（剩余数量 + 当前栈底预览） |

### 3.2 PasteStackStatus

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteStackStatus {
    pub remaining: usize,             // 剩余条数
    pub next_preview: Option<String>, // 下一条内容预览（前 30 字符）
}
```

### 3.3 全局热键

`Cmd+Shift+V` → 调 `pop_and_paste`——与 `clipboard_shortcut`（Alt+C 打开窗口）同体系注册。

在 `clipboard_window.rs::register_clipboard_shortcut` 旁加 `register_paste_stack_shortcut`：
- 默认 `Cmd+Shift+V`
- 可配置（`paste_stack_shortcut` config 字段）
- 热重载（与 clipboard_shortcut 同模式）

### 3.4 pop_and_paste 流程

```
1. lock PASTE_STACK
2. items.pop_front() → 拿到 history_id（栈空返 false）
3. 读 DB clipboard_history 行 by history_id
4. handle.write_text(content) → 写剪贴板（设 suppress flag 防 watcher 回环）
5. emit paste_stack://updated { remaining } → 前端更新计数
6. sleep 100ms 等剪贴板稳定
7. focus.restore_focus() → 焦点还给目标应用
8. simulate_paste() → osascript 发 Cmd+V
9. return true
```

与 `paste_clipboard_item`（行 197-229）几乎同流程，区别只是 id 来源从「前端传」变成「从栈弹」。

## 4. 前端交互

### 4.1 多选（Cmd+点击）

- 剪贴板历史列表项支持 Cmd+点击多选（macOS 原生范式）
- 选中项绿色高亮 + 左上角序号标号（①②③...）
- Shift+点击范围选（可选，后续加）
- 再次 Cmd+点击取消选中

### 4.2 入栈按钮

- 选中 ≥1 条时显示「入栈」按钮（浮动在列表底部或工具栏）
- 点击 → `invoke("push_to_paste_stack", { ids: selectedIds })` → toast `已入栈 N 条`
- 入栈后清空多选
- **入栈后自动切到队列 tab**（`setFilter("queue")`）——2026-08-05 用户反馈，入栈后立即聚焦到队列 tab 看入栈结果

### 4.3 栈状态指示

- 剪贴板窗口角落显示栈计数（如「📋 3/5」——剩余 3 条，共入栈 5 条）
- 栈空时隐藏
- `listen("paste_stack://updated")` 实时更新

### 4.4 清空栈

- 栈计数旁加「×」按钮 → `invoke("clear_paste_stack")`

## 5. 不变量

1. **FIFO 顺序**——入栈顺序 = 粘贴顺序（列表从上到下 = 弹出从先到后）
2. **栈空安全**——`pop_and_paste` 栈空时返 false，不发 Cmd+V（避免覆盖用户当前剪贴板内容）
3. **suppress flag**——写剪贴板时设 suppress 防 watcher 回环（与 `paste_clipboard_item` 同款）
4. **全局热键不依赖窗口可见**——`Cmd+Shift+V` 在任何应用 focus 时都能触发（不需要剪贴板窗口可见）
5. **栈不持久化**——重启清空（设计约束，非 bug）

## 6. crate 架构

| 位置 | 职责 |
|---|---|
| `crates/desktop/src/clipboard/paste_stack.rs` | PasteStack struct + push/pop/clear/status |
| `crates/desktop/src/clipboard/clipboard_commands.rs` | Tauri 命令（push_to_paste_stack / pop_and_paste / clear_paste_stack / paste_stack_status） |
| `crates/desktop/src/clipboard/clipboard_window.rs` | 全局热键注册（register_paste_stack_shortcut） |
| `crates/infra/src/config.rs` | `paste_stack_shortcut` 配置字段（默认 `Cmd+Shift+V`） |
| 前端 `ClipboardPanel.tsx` | Cmd+点击多选 + 序号高亮 + 入栈按钮 + 栈计数 |

## 7. 测试

- `push` 后 `status` 返回正确 remaining + next_preview
- `pop` 按入栈顺序出（FIFO）
- `pop` 栈空返 false
- `clear` 清空
- `pop_and_paste` 的 suppress flag 防回环（已有 paste_clipboard_item 测试模式可借鉴）
