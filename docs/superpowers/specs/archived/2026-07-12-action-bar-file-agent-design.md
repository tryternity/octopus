# Action Bar 文件 Agent 桥接设计

> **状态**：已实现 ✅
> **日期**：2026-07-12
> **scope**：在 Finder 内选中文件/文件夹后，通过全局热键弹出 action bar，将选中对象交给外部 CLI agent（Claude Code / pi）处理；附带复制路径内置动作
> **前置文档**：
> - [`2026-07-12-action-bar-command-shortcut-design.md`](./2026-07-12-action-bar-command-shortcut-design.md)（action bar DB 化 + 局部快捷键）
> - [`2026-07-09-action-bar-menu-db-design.md`](./2026-07-09-action-bar-menu-db-design.md)（action bar DB 表结构基础）

---

## 1. 背景与动机

### 现状

当前 action bar 仅对**选中文本**起作用：全局热键触发 → 模拟 `Cmd+C` 读剪贴板 → 获取文本 → 浮窗展示 AI/URL/Script 等动作。`ActionBarContext` 只有 `text` 字段，所有菜单项隐式假设输入是文本。

### 需求

用户希望在 **Finder 内选中文件/文件夹**后，通过快捷键弹出 action bar，将选中的文件对象交给外部 agent 处理——如「整理这个文件夹」「把这些文件做成 PPT」。octopus 定位为**桥接器**：获取 Finder 选中 → 打包文件路径 + 任务 → 交给外部 CLI agent（在终端窗口异步运行）。octopus **不碰文件系统**，不执行文件读写/移动/创建。

### 定位

| octopus 是 | octopus 不是 |
|---|---|
| Finder 选中捕获器 | 文件操作执行器 |
| Agent 检测器 + 注册表 | Agent 本身 |
| 任务 + 文件路径打包器 | 文件系统沙箱 |
| 终端启动器 | 结果回收器 |

---

## 2. 竞品调研总结

完整调研见脑暴记录，核心共识：

| 产品 | 机制 | 启示 |
|---|---|---|
| **Alfred Universal Actions** | 单热键 + 自动类型探测（text/file/url）+ 按类型过滤动作 | 类型分发是核心模型 |
| **Raycast** | `⌘K` Action Panel + Finder File Actions 扩展 | 统一入口体验 |
| **Finder Quick Actions** | 系统级，右键/Preview 面板，类型感知（视频→Trim） | 仅 Finder 内 |
| **PopClip** | 文本选中即弹气泡，靠文本路径中转文件 | 不直接处理文件对象 |

**关键共识**：单一入口热键 + 按选中类型分发动作。octopus 采用此模型，扩展 `ActionBarContext` 支持 `Files` 类型，菜单项按 `accepts` 字段过滤。

---

## 3. 设计决策汇总

| 维度 | 决策 |
|---|---|
| 触发场景 | Finder 内选中文件/文件夹；非 Finder 不触发 |
| octopus 定位 | 桥接器——拿选中 → 打包 → 丢给外部 agent，不碰文件系统 |
| Agent 形态 | 一期 T1（本地 CLI，Terminal.app 拉起终端窗口异步跑）；预留 T3（云端 agent） |
| 内置 agent | 一期预置 `claude`（Claude Code）+ `pi` 两个 adapter |
| 检测机制 | 内置系统白名单（已知 agent 列表）扫描 PATH 是否安装；用户可自定义新增 adapter |
| 任务/prompt | action 项绑 prompt 模板，含 `{{task}}` 占位符则触发时弹输入框 |
| 终端 | 一期固定 Terminal.app；启动器做成抽象 trait，后续可换内置终端 |
| 辅助动作 | 复制路径（`copy_path` 内置动作，不走 script） |

---

## 4. 架构设计

### 4.1 ActionBarContext 扩展

`crates/desktop/src/action_bar_commands.rs`：

```rust
#[derive(Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind { Text, Files }

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub kind: ContextKind,
    pub text: Option<String>,   // Text 场景
    pub files: Vec<String>,     // Files 场景（POSIX 路径）
    // 以下字段来自 main 的 app_context 合并（text 场景后台采集）：
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::app_context::SurroundingText>,
}
```

- 向后兼容：text 场景 `kind=Text, files=[]`
- 新增 files 场景：`kind=Files, text=None`

### 4.2 Finder 选中捕获

触发流程变更（`trigger_action_bar`）：

```
全局热键
  │
  ▼
检测前台 app（NSWorkspace frontmostApplication.bundleIdentifier）
  │
  ├─ Finder (com.apple.finder)
  │   ├─ AppleScript 拿 selection → POSIX 路径列表
  │   ├─ 空选中 → 静默不弹（日志）
  │   └─ 非空 → 组装 ActionBarContext{kind:Files, files} → 显示浮窗
  │
  └─ 非 Finder
      └─ 回退到现有逻辑：模拟 Cmd+C → 读剪贴板 → text 场景
```

**AppleScript 获取 Finder 选中**：

```applescript
tell application "Finder"
    set sel to selection
    if (count of sel) = 0 then return ""
    set paths to ""
    repeat with f in sel
        set paths to paths & (POSIX path of (f as alias)) & linefeed
    end repeat
    return paths
end tell
```

通过 `tauri-plugin-os` 或直接 `objc` 调 `NSWorkspace.shared.frontmostApplication.bundleIdentifier` 判断前台 app。

**坐标定位**：复用现有 `get_mouse_position`，浮窗位置计算逻辑不变（鼠标上方 + 显示器边缘碰撞检测）。

### 4.3 Agent 适配器注册表

`crates/desktop/src/agent_adapter.rs`（新文件）：

```rust
/// Agent 适配器——描述一个 CLI agent 的检测与启动方式。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapter {
    pub key: String,              // "claude"
    pub display_name: String,     // "Claude Code"
    pub detect_binary: String,    // PATH 检测的二进制名
    pub command_template: String, // 命令模板
    pub is_builtin: bool,         // 内置 vs 用户自定义
    pub is_available: bool,       // 运行时检测：是否已安装
}
```

**内置白名单（一期）**：

| key | display_name | detect_binary | command_template | 文件传法 |
|---|---|---|---|---|
| `claude` | Claude Code | `claude` | `claude --add-dir {cwd} {prompt}` | `--add-dir` 授目录访问权，文件路径在 prompt 内 |
| `pi` | Pi | `pi` | `pi {files_at} {prompt}` | `@` 前缀传文件路径 |

> **注意**：模板中**不自带引号**——`render_command` 统一负责转义：
> - `{prompt}` → `shell_escape_single`（单引号包裹，`$`/`` ` `` 全部字面）
> - `{files}`/`{files_at}`/`{cwd}` → 每个路径独立 `shell_escape_single`（单引号包裹）
> - 模板作者只需写裸占位符，引号由渲染层自动加

**命令模板占位符**：

| 占位符 | 渲染为 | 转义方式 |
|---|---|---|
| `{prompt}` | 单引号包裹的 prompt | `shell_escape_single` |
| `{files}` | 每个路径独立单引号包裹，空格分隔 | `shell_escape_single` 逐路径 |
| `{files_at}` | 每个路径 `@` 前缀 + 单引号包裹 | `shell_escape_single` 逐路径 |
| `{cwd}` | 单引号包裹的工作目录 | `shell_escape_single` |

**检测机制**：
- 应用启动时遍历内置白名单 + DB 用户自定义 adapter，对每个 `detect_binary` 跑 `which`，缓存 `is_available` 到内存
- `is_builtin_key(&str) -> bool`：零进程开销检查 key 是否为内置（create/update adapter 时拒绝与内置同名）
- 设置页 Agent 面板有「刷新检测」按钮
- 不实时监听 PATH

**用户自定义 adapter**（存 DB）：

```sql
CREATE TABLE IF NOT EXISTS agent_adapters (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    key             TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    detect_binary   TEXT NOT NULL,
    command_template TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

启动时合并内置白名单 + DB 记录 → 全量 adapter 列表 → 逐个 `which` 检测。

### 4.4 新增 actionType：agent / copy_path

`action_type` 枚举新增两个值：

#### agent

菜单项配置：

| 字段 | 值 | 说明 |
|---|---|---|
| `action_type` | `agent` | 新类型 |
| `agent` | `claude` / `pi` / 自定义 key | 绑定的 adapter（DB 新增列，原设计中叫 key，改名为 agent 更明确） |
| `action_data` | prompt 模板 | 支持 `{{files}}` `{{task}}` 占位符 |
| `accepts` | `file` | 文件场景可见 |

**prompt 模板占位符**（注意双层花括号，区别于命令模板的单层）：

| 占位符 | 渲染 | 含 `{{task}}`? | 触发行为 |
|---|---|---|---|
| `请整理这些文件：{{files}}` | `请整理这些文件：/a /b` | 否 | 直接执行 |
| `{{task}}\n\n文件列表：\n{{files}}` | `制作PPT\n\n文件列表：\n/a /b` | 是 | 先弹输入框 |

**触发流程**：
1. 点 agent 项 → 查 prompt 模板含 `{{task}}`？
   - 是 → 浮窗内联弹输入框「告诉 agent 做什么」→ 回车提交
   - 否 → 直接执行
2. 渲染 prompt 模板：`{{files}}` → 换行分隔路径，`{{task}}` → 用户输入
3. 渲染命令模板：`{prompt}` → 渲染后 prompt，`{files}` / `{files_at}` / `{cwd}` → 路径
4. **shell escape**：prompt 内容做 shell 转义防注入
5. `TerminalLauncher::spawn(command, cwd)` → Terminal.app 新窗口
6. 关闭 action bar 浮窗

#### copy_path

只读内置动作，不暴露复杂配置。复制选中文件的路径到剪贴板。

| 字段 | 值 |
|---|---|
| `action_type` | `copy_path` |
| `action_data` | 格式：`plain`（纯路径）/ `url`（`file://` URL）/ `quoted`（带引号） |
| `accepts` | `file` |

多选文件时路径用换行分隔。

### 4.5 accepts 字段：按选中类型过滤菜单

`action_bar_items` 新增 `accepts` 列：

```sql
ALTER TABLE action_bar_items ADD COLUMN accepts TEXT NOT NULL DEFAULT 'text';
```

| accepts 值 | Text 场景可见 | Files 场景可见 |
|---|:---:|:---:|
| `text` | ✅ | ❌ |
| `file` | ❌ | ✅ |
| `any` | ✅ | ✅ |

**现有类型默认值**：`ai` / `url` / `script` / `extension` / `copy` → `text`（保持行为不变）；`submenu` → `any`（容器类型，两种场景都可能承载子菜单——用户可建「文件处理」submenu 下挂多个 agent 动作）。

**新类型默认值**：`agent` / `copy_path` → `file`。

前端过滤逻辑（`ActionBar` 浮窗组件）：
```ts
const visible = items.filter(item =>
  context.kind === 'text'
    ? item.accepts === 'text' || item.accepts === 'any'
    : item.accepts === 'file' || item.accepts === 'any'
);
```

`submenu` 容器特殊处理：可见性**动态计算**——它自身的 `accepts=any` 让它通过初筛，但最终是否显示取决于「是否有任意子项在当前场景可见」。如果一个 submenu 下所有子项（递归）在当前场景都不可见，该 submenu 自身也隐藏。这样既允许用户用 submenu 组织 file 专用 agent 动作组，又保证纯 text 的 submenu（如现有「搜索」「网页」）在 file 场景不噪声。

### 4.6 终端启动器抽象

`crates/desktop/src/terminal_launcher.rs`（新文件）：

```rust
pub trait TerminalLauncher {
    /// 在新终端窗口执行命令，cwd 指定工作目录。
    fn spawn(&self, command: &str, cwd: &Path) -> Result<()>;
}

/// 一期实现：Terminal.app via AppleScript。
pub struct TerminalAppLauncher;

impl TerminalLauncher for TerminalAppLauncher {
    fn spawn(&self, command: &str, cwd: &Path) -> Result<()> {
        // osascript -e 'tell application "Terminal"
        //   do script "cd {cwd} && {command}"
        //   activate
        // end tell'
        // do script 打开新窗口；命令需 shell escape
    }
}
```

**安全**：command 由命令模板 + 用户 task 拼装，task 部分必须 shell escape（单引号包裹 + 内部单引号转义），防止命令注入。

**cwd 策略**：多文件跨目录时取首个文件的父目录。

### 4.7 DB Schema 变更汇总

```sql
-- 1. action_bar_items 新增 agent 列（绑定 adapter key）
ALTER TABLE action_bar_items ADD COLUMN agent TEXT NOT NULL DEFAULT '';

-- 2. action_bar_items 新增 accepts 列（按选中类型过滤）
ALTER TABLE action_bar_items ADD COLUMN accepts TEXT NOT NULL DEFAULT 'text';

-- 3. 新表：用户自定义 agent adapter
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

`db.sql` 的 `CREATE TABLE action_bar_items` 同步加入 `agent` 和 `accepts` 列。

**Rust 侧 `ActionBarItem` struct 扩展**：

```rust
pub struct ActionBarItem {
    // ... 现有字段 ...
    pub shortcut: String,
    pub agent: String,    // 新增：绑定的 adapter key（仅 agent 类型非空）
    pub accepts: String,  // 新增：text/file/any
}
```

---

## 5. 数据流总览

```
Finder 选中文件
    │
    ▼ 全局热键
┌─────────────────────────────────────┐
│ trigger_action_bar                  │
│  1. 检测前台 app bundleId           │
│  2. Finder → AppleScript selection  │
│  3. 组装 ActionBarContext{files}    │
│  4. 存 PENDING_CONTEXT              │
│  5. 鼠标位置 → 显示浮窗              │
│  （非 Finder → 回退现有 text 逻辑）   │
└─────────────────────────────────────┘
    │
    ▼ action-bar://show 事件
┌─────────────────────────────────────┐
│ 前端浮窗                             │
│  - get_context() 拿 {kind, files}   │
│  - 按 accepts 过滤菜单（file/any）   │
│  - 渲染（文件数量 badge?）           │
└─────────────────────────────────────┘
    │
    ▼ 用户点 agent 项
┌─────────────────────────────────────┐
│ trigger_agent_action                │
│  1. 模板含 {{task}}? → 弹输入框      │
│  2. 渲染 prompt（{{files}}{{task}}） │
│  3. 渲染命令（{prompt}{files}{cwd}） │
│  4. shell escape                     │
│  5. launcher.spawn(cmd, cwd)         │
│  6. 关浮窗                           │
└─────────────────────────────────────┘
    │
    ▼ Terminal.app 弹出
┌─────────────────────────────────────┐
│ Claude Code / pi 独立窗口异步运行     │
│  - octopus 不管结果                  │
│  - 用户在终端里交互                  │
└─────────────────────────────────────┘
```

**copy_path 数据流**（更简单）：
```
点 copy_path 项 → 格式化路径（plain/url/quoted）→ 写剪贴板 → 关浮窗
```

---

## 6. Tauri 命令清单

新增命令（注册到 `invoke_handler`）：

```rust
// Finder 选中捕获（内部用，trigger_action_bar 调用）
fn get_finder_selection() -> Result<Vec<String>, String>

// Agent adapter 管理
fn list_agent_adapters() -> Result<Vec<AgentAdapter>, String>  // 合并内置+DB+检测状态
fn create_agent_adapter(key, display_name, detect_binary, command_template) -> Result<i64, String>
fn update_agent_adapter(id, key, display_name, detect_binary, command_template) -> Result<(), String>
fn delete_agent_adapter(id) -> Result<(), String>
fn refresh_agent_detection() -> Result<Vec<AgentAdapter>, String>  // 重新跑 which

// Agent action 触发
fn trigger_agent_action(item_id: i64, task: Option<String>, app: AppHandle) -> Result<(), String>
// 内部：查 item → 查 adapter → 渲染模板 → launcher.spawn

// copy_path 触发
fn copy_selected_path(format: String) -> Result<(), String>
// format: plain/url/quoted
```

---

## 7. 前端变更

### 7.1 ActionBar 浮窗（`frontend/src/pages/ActionBar/`）

- `action_bar_get_context` 返回 `{kind, text, files}`
- 按 `accepts` 过滤菜单项
- Files 场景：顶部显示「N 个文件选中」badge
- agent 项含 `{{task}}` 时，点击后浮窗内联弹输入框（不新开窗口）

### 7.2 设置页 ActionBarPanel（`frontend/src/pages/Settings/ActionBarPanel.tsx`）

- `TYPE_META` 和 `ACTION_TYPES` 新增 `agent` / `copy_path`
- 编辑表单新增：
  - `agent` 下拉（仅 agent 类型显示，选项来自 `list_agent_adapters`，仅显示 `is_available=true` 的）
  - `accepts` 下拉（text/file/any）
- agent 类型的 action_data = prompt 模板（placeholder 提示 `{{files}}` `{{task}}`）

### 7.3 新增设置页：Agent Adapter 管理

`frontend/src/pages/Settings/AgentPanel.tsx`（新文件）：
- 列出所有 adapter（内置 + 自定义），显示检测状态（✅ 已安装 / ❌ 未找到）
- 「刷新检测」按钮
- 新增 / 编辑 / 删除自定义 adapter
- 内置 adapter 只读（不可删除/编辑命令模板）

---

## 8. 错误处理

| 场景 | 处理 |
|---|---|
| 前台非 Finder | 回退现有 text 逻辑（模拟 Cmd+C） |
| Finder 空选中 | 静默不弹（日志 `info`） |
| agent 未安装 | toast「{display_name} 未安装（未在 PATH 找到 `{binary}`）」 |
| Terminal.app 启动失败 | toast「终端启动失败：{error}」 |
| task 输入为空（模板要求 `{{task}}`） | 不执行，输入框 placeholder 提示 |
| AppleScript 执行失败 | 日志 `warn`，静默不弹 |
| 自定义 adapter key 重复 | DB UNIQUE 约束拦截，前端提示 |

---

## 9. 不做的（YAGNI）

- ❌ 多选批量 action（一个 action bar 项 → 一个 agent 实例，文件全量传入）
- ❌ agent 结果回收（异步，用户在终端里看）
- ❌ octopus 内置 PPT / 整理等具体能力（全靠外部 agent）
- ❌ 文件系统沙箱 / 撤销（octopus 不碰文件系统）
- ❌ T2（本地 HTTP/MCP agent）—— 二期
- ❌ T3（云端 agent）—— 三期
- ❌ 内置终端（二期，届时内容感知更好）
- ❌ 实时 PATH 监听（手动刷新检测即可）

---

## 10. 未来扩展

### T2：本地 HTTP / MCP agent

新增 `AgentAdapterKind::Http`，adapter 配置加 `endpoint` 字段，`spawn` 改为 HTTP POST。trait 扩展：

```rust
pub trait AgentLauncher {
    fn launch(&self, params: &LaunchParams) -> Result<()>;
}
// TerminalAppLauncher / HttpAgentLauncher / CloudAgentLauncher
```

### T3：云端 agent

adapter kind = cloud，配置加 API key / endpoint。结果可同步回 CompactEditor 展示（云端 agent 通常同步返回）。

### 内置终端

`impl TerminalLauncher for EmbeddedTerminal`——在 octopus 窗口内嵌入终端组件（xterm.js），内容感知更好（可截取 agent 输出做后续处理）。
