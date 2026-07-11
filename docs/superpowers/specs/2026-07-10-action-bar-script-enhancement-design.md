# Action Bar 脚本增强——JS/TS 运行时 + 执行结果捕获 + 异步模式

> **日期**：2026-07-10
> **状态**：✅ 已实现（plan `plans/2026-07-10-action-bar-script-enhancement.md` 43/43 Task 完成）
> **关联**：[action-bar-menu-db spec](2026-07-09-action-bar-menu-db-design.md) §5.3 script 执行、[调研报告](2026-07-09-action-bar-related-tools-survey.md) §11 PopClip

---

## 1. 背景与目标

当前 `run_script` 支持 4 种 magic comment（`#shell` / `#osascript` / `#powershell` / `#python`），存在三个不足：

1. **JS/TS 缺失**——PopClip 7 种 Action 类型之一，用户量大、运行时易得
2. **fire-and-forget**——spawn 后不捕获 stdout/stderr，脚本结果无法回传用户
3. **强制同步等待**——慢脚本阻塞浮窗 UI

本次增强目标：
- 新增 `#node` / `#deno` / `#bun` / `#javascript` / `#typescript` 五种 magic comment
- 所有 script 类型的执行结果捕获 stdout/stderr，落库 `script_runs` 表
- 菜单项新增 `is_async`（异步执行）+ `write_output_to_clipboard`（结果写剪贴板）两个配置项

---

## 2. Magic Comment 体系

### 2.1 完整分发表（更新后）

| magic comment | 探测 | 命令 | 平台 |
|---|---|---|---|
| `#shell` | 不探测 | `sh -c "<code>"` | 全平台 |
| `#osascript` | 不探测 | `osascript -e "<code>"` | 仅 macOS |
| `#powershell` | 不探测 | `powershell -Command "<code>"` | 仅 Windows |

| `#node` | 不探测 | `node -e "<code>"` | 全平台 |
| `#deno` | 不探测 | `deno eval "<code>"` | 全平台 |
| `#bun` | 不探测 | `bun eval "<code>"` | 全平台 |
| `#javascript` | 预探测 | 探测到的运行时对应命令 | 全平台 |


### 2.2 运行时探测（`#javascript` / `#typescript`）

**设计决策**：预探测法（`--version` 检测 PATH 可用性），选定后只 spawn 一次，错误信息更精准。

**`#javascript` 优先级**：node → bun → deno

```text
node --version 成功 → 用 node -e "<code>"
    ↓ 失败
bun --version 成功 → 用 bun eval "<code>"
    ↓ 失败
deno --version 成功 → 用 deno eval "<code>"
    ↓ 失败
报错："未检测到 JS 运行时，请安装 Node.js / Bun / Deno 之一"
```

**`#typescript` 优先级**：npx tsx → bun → deno

```text
npx --yes tsx --version 成功 → 用 npx --yes tsx -e "<code>"
    ↓ 失败
bun --version 成功 → 用 bun eval "<code>"（Bun 原生 TS）
    ↓ 失败
deno --version 成功 → 用 deno eval "<code>"（Deno 原生 TS）
    ↓ 失败
报错："未检测到 TS 运行时，请安装 tsx（npm i -g tsx）/ Bun / Deno 之一"
```

**显式 magic comment（`#node`/`#deno`/`#bun`）跳过探测**，直接 spawn，与 `#python` 一致——失败报 `脚本执行失败: <系统错误>`。

**选中文本传递**：所有运行时统一通过环境变量 `OCTOPUS_TEXT`，与现有 shell/osascript/python 一致。

| 运行时 | 读取方式 |
|---|---|
| node | `process.env.OCTOPUS_TEXT` |
| deno | `Deno.env.get("OCTOPUS_TEXT")` |
| bun | `process.env.OCTOPUS_TEXT` |
| tsx | `process.env.OCTOPUS_TEXT` |

---

## 3. DB Schema 变更

### 3.1 `action_bar_items` 表新增字段

```sql
ALTER TABLE action_bar_items ADD COLUMN is_async INTEGER NOT NULL DEFAULT 1;
ALTER TABLE action_bar_items ADD COLUMN write_output_to_clipboard INTEGER NOT NULL DEFAULT 0;
```

- `is_async`：0=同步等待结果，1=异步 fire-and-forget（默认异步）
- `write_output_to_clipboard`：仅 `is_async=0` 时有意义；1=成功+stdout 非空时写剪贴板

**DB schema 变更流程**：改 `db.sql`（加列到 CREATE TABLE）+ 升 `user_version`。**已有 DB 需 ALTER TABLE 迁移**——`CREATE TABLE IF NOT EXISTS` 对已有表无效，升级路径用 `PRAGMA table_info` 检测列是否存在 + `ALTER TABLE ADD COLUMN` 补列（v20→v21 迁移已实现，e2e 验证通过）。

### 3.2 新表 `script_runs`

```sql
CREATE TABLE IF NOT EXISTS script_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     INTEGER NOT NULL,
    script_type TEXT NOT NULL,           -- #node / #javascript / #shell ...
    exit_code   INTEGER,                 -- null = 超时强杀
    stdout      TEXT NOT NULL DEFAULT '',-- 截断上限 64KB
    stderr      TEXT NOT NULL DEFAULT '',-- 截断上限 64KB
    error_msg   TEXT NOT NULL DEFAULT '',-- spawn 失败等系统级错误
    started_at  TEXT NOT NULL,           -- ISO 8601
    finished_at TEXT,                    -- null = 超时
    duration_ms INTEGER,
    FOREIGN KEY (item_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_script_runs_started_at ON script_runs(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_script_runs_item_id ON script_runs(item_id);
```

**stdout/stderr 截断**：在 Rust 端捕获后 truncate 到 64KB（`chars().take(65536)`），防超大输出撑爆 DB。

### 3.3 ActionBarItem struct 扩展

```rust
pub struct ActionBarItem {
    // ... 现有字段 ...
    pub is_async: bool,                  // 新增
    pub write_output_to_clipboard: bool, // 新增
}
```

`ActionBarItem` 的 `#[serde(rename_all = "camelCase")]` 确保 JSON 自动转 `isAsync` / `writeOutputToClipboard`。

---

## 4. 执行模式

### 4.1 异步模式（`is_async = true`，默认）

```text
execute_action_bar_inner
  → run_script_async(source, text, item_id)
    → std::thread::spawn 后台线程:
        → 预探测（如需）+ spawn 子进程（不捕获 stdout pipe）
        → try_wait 轮询 60 秒超时强杀
        → 收割 stdout/stderr/exit_code → 写 script_runs
  → 立即返回 Ok(false) → 后端 hide + finalize（正常关闭浮窗）
```

**特点**：
- spawn 后立即关闭浮窗，用户无感
- 不展示结果，不写剪贴板
- 后台线程负责落库 `script_runs`
- 适用于纯副作用脚本（调 API、启动服务、操作文件）

### 4.2 同步模式（`is_async = false`）

```text
execute_action_bar_inner（已是 async command）
  → tokio::task::spawn_blocking(run_script_sync(...))
    → 预探测（如需）+ spawn 子进程（捕获 stdout + stderr pipe）
    → 同步等待 exit（try_wait 轮询 60 秒）
    → 写 script_runs
    → 返回 ScriptResult { exit_code, stdout, stderr, timed_out }
  → 根据 ScriptResult:
    → 成功 + stdout 非空 → action_bar_show_result(stdout...) → Ok(true)
    → 成功 + stdout 空 → Ok(false) → hide
    → 失败/超时 → Err(stderr/error_msg) → 前端红色气泡提示（2 秒消失）
```

**特点**：
- 复用现有 AI loading 视图（前端 timeout 机制不变）
- 成功结果经 `action_bar_show_result` → CompactEditor 展示
- `write_output_to_clipboard=true` 时在 `show_result` 内部额外 `write_text`
- 失败经 Err → 前端**红色气泡**（半透明覆盖浮窗顶部，限制 40 字符，2 秒后自动消失回到菜单，不切 error 视图）
- **error 视图已移除**（AI 超时 / url / script / copy 错误统一走气泡）

### 4.3 `run_script` 重构

现有 `run_script` 拆为两个函数：

```rust
struct ScriptResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// 异步执行——spawn 后立即返回，后台线程收割并落库
fn run_script_async(source: &str, text: &str, item_id: i64, script_type: &str) -> Result<(), String>;

/// 同步执行——spawn_blocking 等待完成，返回结果
fn run_script_sync(source: &str, text: &str, item_id: i64, script_type: &str) -> Result<ScriptResult, String>;
```

共享逻辑提取到：
```rust
/// 按 magic comment 分发，返回子进程 Child（预探测 + spawn）
fn spawn_script(source: &str, text: &str) -> Result<(std::process::Child, &'static str), String>;

/// 轮询等待 + 超时强杀（复用现有 try_wait × 120 逻辑）
fn wait_with_timeout(child: &mut Child) -> ScriptResult;
```

---

## 5. 菜单项配置变更

### 5.1 前端联动规则

**仅 `action_type === "script"` 时显示两个选项**，其他类型不显示。

| is_async | write_output_to_clipboard | UI 行为 |
|---|---|---|
| true（异步） | — | checkbox 禁用 + 强制 false |
| false（同步） | 可选 | checkbox 可勾选 |

### 5.2 前端 TYPE_META 更新

```typescript
script: {
    dot: "bg-emerald-500",
    label: "SCRIPT",
    desc: "首行 #shell / #osascript / #powershell / #python / #node / #deno / #bun / #javascript / #typescript 决定运行时；选中文本经 $OCTOPUS_TEXT 传入",
    placeholder:
      "#shell / #osascript / #powershell / #python\n#node / #deno / #bun\n#javascript / #typescript\n选中文本在 $OCTOPUS_TEXT 环境变量中",
},
```

### 5.3 DB 命令签名变更

```rust
// insert 新增两参数
pub fn insert_action_bar_item(
    parent_id: Option<i64>, title: &str, icon: &str,
    action_type: &str, action_data: &str,
    is_async: bool, write_output_to_clipboard: bool,  // 新增
) -> Result<i64>;

// update 新增两参数
pub fn update_action_bar_item(
    id: i64, title: &str, icon: &str,
    action_type: &str, action_data: &str, is_enabled: bool,
    is_async: bool, write_output_to_clipboard: bool,  // 新增
) -> Result<()>;
```

Tauri command `create_action_bar_item` / `update_action_bar_item` 对应新增两参数。

---

## 6. 脚本执行结果管理界面

### 6.1 入口

设置页「命令面板」tab 新增子页/抽屉：「脚本执行记录」。

### 6.2 列表

| 列 | 说明 |
|---|---|
| 时间 | `started_at`（相对时间 + tooltip 绝对时间） |
| 菜单项 | 关联 `action_bar_items.title`（CASCADE 删除后显示「已删除」） |
| 类型 | `#javascript` / `#shell` 等 |
| 状态 | 成功（绿）/ 失败（红）/ 超时（橙） |
| 耗时 | `duration_ms` |
| 输出 | stdout 前 200 字预览，点击展开全文（只读 textarea） |

### 6.3 交互

- 单行展开看 stdout/stderr 全文
- 批量清理（保留最近 N 条，默认 100 条）
- `ON DELETE CASCADE`：菜单项删除时级联删其执行记录

### 6.4 命令

```rust
#[tauri::command]
fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<ScriptRun>, String>;

#[tauri::command]
fn clear_script_runs(keep_recent: Option<i64>) -> Result<(), String>;
```

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRun {
    pub id: i64,
    pub item_id: i64,
    pub item_title: Option<String>,   // JOIN action_bar_items，删除后 None
    pub script_type: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_msg: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}
```

---

## 7. 不在本次范围

- **PopClip 其他 Action 类型**（Key Press / Service / Shortcut）——非脚本类，后续独立 spec
- **Snippet YAML 导入**（`#octopus` 格式粘贴安装）——二期
- **代码编辑器语法高亮**——当前 textarea 保持纯文本，Monaco/CodeMirror 后续考虑
- **运行时自动安装**——不自动安装 Node/Bun/Deno，仅报错提示

---

## 8. 不变量

1. **magic comment 第一行**——`source.lines().next()` 解析，空行/空源报 `未知脚本类型`
2. **stdout/stderr 截断 64KB**——防 DB 膨胀
3. **超时策略**——同步 60 秒强杀（`wait_with_timeout` 轮询），异步不超时（`wait_forever` 阻塞 `child.wait()`，CPU 0%）
4. **`write_output_to_clipboard` 仅同步模式可生效**——异步模式 UI 禁用 + 强制 false
5. **所有执行都落库**——不论异步/同步/成功/失败/超时，`script_runs` 都有记录
6. **`OCTOPUS_TEXT` 传递**——≤200KB 环境变量直传；超出写临时文件 + marker `_____ULTRA_LONG_TEXT_____:/path`，脚本结束后清理；写入失败回退按字节截断（`is_char_boundary`）
7. **CASCADE 删除**——`PRAGMA foreign_keys = ON`，菜单项删除时级联删 `script_runs`
8. **error_msg 统一**——`script_error_msg()` 覆盖超时/异常退出/非零退出码
9. **运行时探测缓存**——`detect_js/ts_runtime` 用 `OnceLock` 仅首次探测（TS 优先级 bun→deno→npx tsx）
10. **跨平台**——`#python` Windows 用 `python`；`action_data` 绝对路径判断用 `Path::is_absolute()`；`delete_extension` 路径匹配用 `Path::starts_with()`
11. **pipe 并发读取**——stdout/stderr 各独立线程，防 >64KB 管道死锁
