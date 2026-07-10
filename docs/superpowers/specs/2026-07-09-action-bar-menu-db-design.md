# Action Bar 菜单数据库化设计

> **状态**：已实现（Task 1-6 全部完成）
> **日期**：2026-07-09
> **scope**：将 action bar 硬编码菜单迁移为 DB 表管理，支持两级菜单（主菜单 + 子菜单）+ 5 种动作类型 + 用户自定义扩展
> **调研依据**：[`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md)（PopClip/SnipDo/OnText/Click to Do 调研）+ [`2026-07-09-action-bar-related-tools-survey.md`](./2026-07-09-action-bar-related-tools-survey.md)（11 款相关工具综合调研，含扩展机制对比 §5/§10/§11）

---

## 1. 背景与动机

当前 action bar 菜单完全硬编码在前端 `index.tsx`（mainItems / aiItems / searchItems），动作分发逻辑也硬编码在 `executeMain` / `executeSubItem`。用户无法增删改菜单项、无法添加自定义动作。

竞品调研结论：
- **PopClip**：纯文本 Snippet（`#popclip` YAML），4 行定义一个扩展
- **OnText**：6 种动作类型（URL / Shell / AppleScript / Shortcut / Builtin / Folder）+ 正则上下文规则
- octopus 选择 **DB 表 + 设置页 UI 管理**（方案 B），与现有 prompts 表模式一致；后续可扩展导入/导出 JSON 做分享

---

## 2. DB 表结构

```sql
CREATE TABLE IF NOT EXISTS action_bar_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id   INTEGER DEFAULT NULL,         -- NULL=主菜单项；非 NULL=子菜单项
    title       TEXT NOT NULL,                 -- 显示标题，如"润色""问豆包"
    icon        TEXT NOT NULL DEFAULT '',      -- SVG 文件名（如 "polish.svg"）或内联 SVG 源码（以 "<svg" 开头）
    action_type TEXT NOT NULL,                 -- submenu | ai | url | script | copy
    action_data TEXT NOT NULL DEFAULT '',      -- 类型相关参数（见 §3）
    sort_order  INTEGER NOT NULL DEFAULT 0,    -- 同级排序，ASC
    is_system   INTEGER NOT NULL DEFAULT 1,    -- 1=内置不可删，0=用户自定义
    is_enabled  INTEGER NOT NULL DEFAULT 1,    -- 0=隐藏不显示
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
```

自引用 `parent_id` 实现两级菜单：`parent_id IS NULL` = 主菜单项，`parent_id = <某主菜单项 id>` = 其子菜单项。

---

## 3. 动作类型（action_type）

### 3.1 类型枚举

| action_type | action_data 内容 | 运行方式 | 平台 |
|-------------|-----------------|---------|------|
| `submenu` | （空） | 展开子菜单 | 全平台 |
| `ai` | system prompt 文本 | `octopus_llm::chat_text_with_prompt` | 全平台 |
| `url` | URL 模板，`{text}` 为选中文本占位符（可选） | 模板替换 → `open`（支持 `https://`、`doubao://` 等所有 scheme）；无模板时选中文本即 URL | 全平台 |
| `script` | 脚本源码，第一行 magic comment 指定语言。选中文本通过环境变量 `$OCTOPUS_TEXT` 传递 | 按注释分发运行时 | 按语言 |
| `copy` | （空） | `write_clipboard_text` | 全平台 |

### 3.2 script 语言分发

script 类型的 `action_data` 第一行必须是 magic comment：

| magic comment | 探测 | 运行时 | 平台 |
|---------------|------|--------|------|
| `#shell` | 不探测 | `sh -c "<script>"` | 全平台 |
| `#osascript` | 不探测 | `osascript -e "<script>"` | 仅 macOS |
| `#powershell` | 不探测 | `powershell -Command "<script>"` | 仅 Windows |
| `#python` | 不探测 | `python3 -c "<script>"` | 全平台 |
| `#node` | 不探测 | `node -e "<script>"` | 全平台 |
| `#deno` | 不探测 | `deno eval "<script>"` | 全平台 |
| `#bun` | 不探测 | `bun eval "<script>"` | 全平台 |
| `#javascript` | 预探测 node→bun→deno | 探测到的运行时 | 全平台 |
| `#typescript` | 预探测 npx tsx→bun→deno | 探测到的运行时 | 全平台 |

> JS/TS 运行时 + 异步模式 + 结果捕获详见[脚本增强 spec](2026-07-10-action-bar-script-enhancement-design.md)。

平台不支持时 → 前端 toast 报 `不支持的平台`，菜单项仍显示。

### 3.3 选中文本传递

**url 类型**：action_data 中 `{text}` 占位符会被运行时替换为选中文本（URL 编码后）。例：
- `url`：`https://www.google.com/search?q={text}` → `https://www.google.com/search?q=hello`
- URL scheme：`doubao://?text={text}`

**script 类型**：选中文本通过环境变量 **`$OCTOPUS_TEXT`** 传递（不使用字符串替换，避免 shell 注入）。脚本中通过 `$OCTOPUS_TEXT`（shell）、`do shell script "$OCTOPUS_TEXT"`（osascript）、`$env:OCTOPUS_TEXT`（powershell）、`os.environ["OCTOPUS_TEXT"]`（python）、`process.env.OCTOPUS_TEXT`（node/bun/tsx）、`Deno.env.get("OCTOPUS_TEXT")`（deno）读取。

⚠️ **安全**：不做 `{text}` 字符串拼接（曾有注入风险），仅用环境变量。

### 3.4 翻译特殊处理

翻译需要按 CJK 检测方向选择 prompt，不能纯静态。`ai` 类型的 `action_data` 支持 `auto_translate` 关键字——运行时检测选中文本是否含 CJK 字符，选择中译英或英译中 prompt。

### 3.5 典型用例

**问豆包（三种方式）**：
- URL scheme：`url` → `doubao://?text={text}`
- AppleScript：`script` → `#osascript\ntell application "豆包" to activate`
- Shell：`script` → `#shell\nopen -a "豆包" && sleep 1 && osascript -e 'tell application "System Events" to keystroke "v" using command down'`（选中文本在 `$OCTOPUS_TEXT` 中，需先 `echo "$OCTOPUS_TEXT" | pbcopy` 再粘贴）

---

## 4. 种子数据

预置当前所有菜单项（`is_system=1`），启动时 `INSERT OR IGNORE`：

**主菜单**（`parent_id=NULL`）：

| sort_order | title | icon | action_type | action_data |
|-----------|-------|------|-------------|-------------|
| 0 | AI | sparkles.svg | submenu | |
| 1 | 翻译 | globe.svg | ai | `auto_translate` |
| 2 | 搜索 | search.svg | submenu | |
| 3 | 网页 | link.svg | url | |

**AI 子菜单**（`parent_id=<AI 的 id>`）：

| sort_order | title | icon | action_type | action_data |
|-----------|-------|------|-------------|-------------|
| 0 | 润色 | pencil.svg | ai | `请对以下文本进行润色，使其更加流畅、专业。保持原意不变。只输出润色结果。` |
| 1 | 摘要 | file-text.svg | ai | `请用简洁的中文总结以下内容的要点，不超过 3 句话。只输出总结。` |
| 2 | 解释 | lightbulb.svg | ai | `请用简洁的中文解释以下内容的含义。只输出解释。` |

**搜索子菜单**（`parent_id=<搜索 的 id>`）：

| sort_order | title | icon | action_type | action_data |
|-----------|-------|------|-------------|-------------|
| 0 | Google | search.svg | url | `https://www.google.com/search?q={text}` |
| 1 | 百度 | search.svg | url | `https://www.baidu.com/s?wd={text}` |
| 2 | Bing | search.svg | url | `https://www.bing.com/search?q={text}` |

种子数据中的 parent_id 依赖 AI 和搜索主菜单项的 id——种子 SQL 用子查询获取 parent id（与 prompts 表 seed 的显式 id 方式不同，因为 action_bar_items 用 AUTOINCREMENT）。

---

## 5. 后端命令

### 5.1 DB 层（crates/infra/src/db.rs）

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
}

pub fn list_action_bar_items() -> Result<Vec<ActionBarItem>>   // ORDER BY parent_id ASC NULLS FIRST, sort_order ASC
pub fn insert_action_bar_item(parent_id: Option<i64>, title: &str, icon: &str, action_type: &str, action_data: &str) -> Result<i64>
pub fn update_action_bar_item(id: i64, title: &str, icon: &str, action_type: &str, action_data: &str, is_enabled: bool) -> Result<()>
pub fn delete_action_bar_item(id: i64) -> Result<()>           // is_system=1 拒绝删除
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<()>  // +1=下移, -1=上移，交换同 parent 下 sort_order
```

### 5.2 Tauri 命令层（crates/desktop/src/）

**CRUD 命令**（放 settings_commands.rs 或新建 action_bar_menu_commands.rs）：

```rust
#[tauri::command] fn list_action_bar_items() -> Result<Vec<ActionBarItem>, String>
#[tauri::command] fn create_action_bar_item(parentId, title, icon, actionType, actionData) -> Result<i64, String>
#[tauri::command] fn update_action_bar_item(id, title, icon, actionType, actionData, isEnabled) -> Result<(), String>
#[tauri::command] fn delete_action_bar_item(id) -> Result<(), String>
#[tauri::command] fn move_action_bar_item(id, direction) -> Result<(), String>
```

- `is_system=1` 的项：`delete` 拒绝，`update` 允许改 title/icon/action_data/is_enabled 但不允许改 action_type
- `move` 只在同 parent_id 下交换 sort_order

**执行命令**（替换现有 run_ai_action / action_bar_open_url 的分发逻辑）：

> ⚠️ **收口职责归后端**：前端不再直接 `getCurrentWindow().hide()`，所有窗口生命周期管理由后端统一负责。命令内层 `execute_action_bar_inner` 返回 `Result<bool>`——`Ok(true)`（ai 已自收口直通）、`Ok(false)`（url/script/copy 成功→hide+finalize）、`Err`（异常→仅 finalize，不 hide，让前端显示红色气泡提示）。所有路径都经收口，`?`/`return Err` 不会泄漏重入锁和 depth。
>
> ⚠️ **线程约束**：本 command 是 `async` → 跑在 tokio worker 线程，而 `after_floating_window_hide` 内的 `NSApplication::deactivate` 需 `MainThreadMarker`（仅主线程可获取）。故 Ok(false) 分支的 `hide_action_bar_window` 通过 `app.run_on_main_thread` 投递到主线程执行（与 `trigger_action_bar` 的 show 同模式）。`finalize_action_bar` 仅操作 `AtomicBool`，线程安全，保持即时执行。`action_bar_dismiss`（sync command）天然在主线程，无需投递。

内层 match 的分支与之前相同（ai/url/script/copy），但**不直接 `?` 返回**——异常经外层 match 兜底 finalize。

### 5.3 script 执行

> ⚠️ **已重构**：原 `run_script` 已拆为 `spawn_script`（9 种 magic comment + 预探测）+ `wait_with_timeout`（轮询 + 捕获 pipe）+ `run_script_async` / `run_script_sync_blocking`（异步/同步分流 + 落库 `script_runs`）。详见[脚本增强 spec](2026-07-10-action-bar-script-enhancement-design.md)。

---

## 6. 前端变更

### 6.1 浮窗（ActionBar/index.tsx）

- 删除硬编码的 mainItems / aiItems / searchItems / SEARCH_URLS
- mount 时 `invoke("list_action_bar_items")` 加载全部菜单项
- 按 parentId 构建两级结构（`#[serde(rename_all = "camelCase")]` 确保 JSON 字段名匹配）
- `executeMain` / `executeSubItem` 合并为统一的 `executeItem(item: ActionBarItem)`
- `ai` 类型仍走前端 loading + 超时 + timedOutRef 流程
- `url` / `script` / `copy` 也走 `try-catch`（与 ai 一致）：成功后端统一 hide+收口，失败显示红色气泡（**前端不再直接 `getCurrentWindow().hide()`**，error 视图已移除）
- 按钮布局：**水平「数字徽章+文字」一行排列**（`flex-row`），非上下两行——浮窗更矮，子菜单展开后总高 ~78px
- 视觉：`rounded-lg`（8px，与语音识别窗口一致）+ `backdrop-blur-xl` 毛玻璃 + `shadow-2xl`
- 窗口高度动态调整：主菜单 40px / 子菜单 78px / loading 48px / error 60px（前端 `setSize` 按 view 切换），避免透明区域遮挡下层点击
- 窗口宽度固定 380px

#### ⚠️ 窗口焦点策略（强需求，勿改错）

**全局快捷键不得将 settings/compact_editor 带到前台。** macOS 上 WKWebView 要求 app 进程 active 才能获得键盘焦点，而 `set_focus` 触发的 `NSApp.activate` 会把所有可见 Regular 窗口带到前台。采用**视觉焦点协调方案**（`activation::before_floating_window_show` / `after_floating_window_hide` 公共函数，`FLOAT_DEPTH` 引用计数支持多浮窗嵌套——只有最外层 depth==1 记录状态/交还焦点）：

**show 时**：
1. 记录当前前台 app（`NSWorkspace.frontmostApplication`）
2. 若 octopus app 非活跃（用户在其他 app），临时隐藏所有可见的其他窗口（settings/compact_editor/clipboard_window，`WINDOWS_TO_HIDE_ON_FLOAT`）
3. `set_focus()` 激活浮窗——此时 Regular 窗口已隐藏，只有浮窗弹出到前台并获得键盘焦点

**hide 时**：
1. `activate` 原前台 app（交还焦点给用户正在使用的应用）
2. `show` 恢复被隐藏的 Regular 窗口——此时 octopus app 已在后台，窗口温和恢复不跳前台

**剪贴板浮窗失焦恢复**（`restore_hidden_windows_only`）：
剪贴板是 toggle 模式（always-on-top 可见，点击外部不 hide）。用户切到其他 app 后剪贴板失焦（`Focused(false)` 事件）但 Regular 窗口仍隐藏 → Dock 图标点击无效。失焦 = **虚拟关闭**：扣减 `FLOAT_DEPTH` + 清 `WAS_INACTIVE`/`PREV_APP` 状态 + 恢复被隐藏窗口 + `deactivate`。不交还前台焦点（剪贴板仍可见）。**必须扣减 depth**——否则 toggle 的 else 分支（`visible=true, focused=false`）重新 `before_show` 会使 depth 累加泄漏，焦点交还机制最终瘫痪。

**多浮窗嵌套**（`FLOAT_DEPTH` 引用计数）：多个浮窗重叠唤起时（如剪贴板可见时唤出 action bar），`before_floating_window_show` 增加 depth，只有最外层（depth==1）才记录前台 app + 隐藏 Regular 窗口。`after_floating_window_hide` 减少 depth，只有回到 0 才交还焦点 + 恢复窗口。防止第二个浮窗覆盖第一个的 `WAS_INACTIVE` 状态。

**AI 结果展示时序**（`action_bar_show_result`）：不调 `hide_action_bar_window`（含 `after_floating_window_hide` → `deactivate`），直接 `win.hide()` 浮窗。因为接下来要创建/展示 CompactEditor，`deactivate` 会导致新窗口被压在后台不可见。但**必须调 `after_floating_window_hide_keep_active`**（递减 `FLOAT_DEPTH` + 恢复隐藏窗口 + 清状态，跳过 deactivate）——否则 depth 永久泄漏导致后续焦点协调彻底瘫痪（P0 已修复）。

**应用范围**：action bar + 剪贴板浮窗（需键盘操作）。语音识别窗无强键盘需求，保持现状不处理。

**上下键切换主子菜单层级，左右键在当前行移动选择。** 这是核心交互，不可混淆：

| 按键 | 行为 |
|------|------|
| **↑↓** | **切换焦点层**：焦点在主菜单→进入子菜单（focusLayer: main→sub）；焦点在子菜单→回到主菜单（sub→main）。不展开/收起子菜单。 |
| **←→** | **当前行移动**：焦点在主菜单→主菜单项之间移动（移到 submenu 项自动展开其子菜单、移到非 submenu 项自动收起子菜单）；焦点在子菜单→子菜单项之间移动。 |
| **Enter** | 执行当前焦点高亮项 |
| **1-9 + a-z** | **快捷定位**（只移动高亮，不执行）：1-9 对应第 1-9 项，a-z 对应第 10-35 项。按焦点层决定定位哪一层。超出范围无效。 |
| **Esc** | **直接关闭浮窗**（一次 Esc，不退焦点层） |

**子菜单展开/收起由左右键控制**：左右键在主菜单移动时，移到 submenu 类型的项→展开子菜单预览，移到非 submenu 项→收起子菜单。上下键只切焦点层，不碰视图展开状态。

**子菜单预览不抢焦点**：左右键展开子菜单时焦点仍在主菜单——用户必须按上下键才把焦点移入子菜单。`focusLayer` 状态（main/sub）独立于 `view` 状态（main/submenu）控制此行为。

**快捷定位语义**：1-9 数字键 + a-z 字母键定位第 1-35 项（只移动高亮，Enter 执行）。定位到 submenu 类型的主菜单项时同步展开子菜单预览。同级菜单项上限 35 个（后端 `create_action_bar_item` 校验）。

**菜单溢出滚动**：项数超过窗口宽度时容器 `overflow-x-auto scrollbar-none`（无滚动条），高亮项变化时 `scrollIntoView` 自动滚动到可见区域。左右溢出时显示 voice 色 `<`/`>` 箭头指示器（`ScrollRow` 组件，`ResizeObserver` + `scroll` 实时检测）。

### 6.2 图标渲染（浮窗已弃用，组件保留）

`ActionBarIcon` 组件（`components/ActionBarIcon.tsx`）三层渲染逻辑：

1. **文件名（`action-ai.svg`）**→ `fetch("/icons/{name}.svg")` 加载完整 SVG → 提取 inner HTML → 重组 `<svg>` 强制 `stroke/fill="currentColor"` → `<i dangerouslySetInnerHTML>`
2. **内联 SVG（`<svg>...`）**→ 直接渲染
3. **Lucide 预置名（`pencil` 等）**→ `<svg>` + 预置 path 组装

> ⚠️ **2026-07-09 变更**：浮窗和设置页均已改为**数字徽章**（`①②③`）替代图标，`ActionBarIcon` 组件当前无引用但保留。DB schema 中 `icon` 字段保留（存量数据 + 向后兼容），新增项 `icon=""`。

⚠️ **历史踩坑**：(1) React `<svg>` + `dangerouslySetInnerHTML` 注入 `<path>` 时 `currentColor` 继承不稳定 → 改为 `<i>` + 完整 SVG 字符串；(2) `ActionBarItem` struct 缺 `#[serde(rename_all = "camelCase")]` → JSON 字段 snake_case → 前端 camelCase 读不到 → 菜单完全不渲染。

### 6.3 设置页

新增 ActionBarPanel（设置窗「命令面板」tab），CRUD 界面：

- **树形控件**：两级菜单按递归树渲染。submenu 节点带 chevron 展开/收起箭头，叶节点箭头位置占位保持左侧对齐。子树用细左导引线（`border-border/50`）连接父子，支持任意深度（DB parent_id 无层级限制）。
- **注册表式序号**：每行左侧等宽序号 `01` / `1.1` / `1.2`（同级 1-based，子项 = `父序号.子序号`），编码 sort_order 信息。
- **每项行内容**：标题 / 子项计数徽标（submenu 且有子项时）/ 类型标签（色点 + 等宽大写名）/ 内置标记 / 悬浮工具栏（上移/下移/删除）。（图标已移除——浮窗用数字徽章定位，管理页无需图标区分）
- **展开状态语义**：首次加载默认展开全部 submenu 节点；后续 refresh（保存/移动/删除）**不覆盖**用户的折叠选择（`refresh` 不碰 `expanded`，仅初次 useEffect 全展开）。新增子项时显式展开其直接父节点。Header 提供「全部展开 / 全部收缩」切换按钮（ChevronsUpDown / ChevronsDownUp 图标，根据 `allExpanded` 状态自动切换）。
- **点击行 = 进入编辑**：点击节点行任意位置进入内联编辑表单（chevron / 工具栏按钮 stopPropagation 不触发改动）。
- `is_system=1`：删除按钮灰掉，类型 select 禁用（可改标题/内容/启用，不可改 action_type）。
- **编辑表单**（内联 elevated card）：标题 / 类型（select + 类型说明文案）/ 内容（textarea，submenu/copy 隐藏，其余按类型显示不同占位符）/ 启用开关。
  - **标题字符限制**：CJK 字符（中日韩）算 2、ASCII 算 1，总权重上限 6——即最多 6 个英文字母或 3 个汉字（混排如「润色A」= 2+2+1 = 5 合法）。用 for-of 逐字符累加截断，防止粘贴超长内容绕过限制。
  - **内容 textarea**：`w-full` 填满列宽 + `min-h-[120px]`（AI 提示词通常较长）。
  - **启用开关**：自定义 Toggle（细条样式）替代原生 checkbox。
- **新增（内存草稿模式）**：Header「新增主菜单项」按钮（顶层）；submenu 节点展开后底部「新增子项」按钮。**点击新增不写 DB**——只在内存创建草稿 state（`draftParentId`：undefined=非草稿 / null=顶层草稿 / number=子菜单草稿），渲染内联编辑表单。**保存时才 `create_action_bar_item` 写 DB**，取消只清内存 state。彻底消除脏数据风险（旧方案「先建 DB 行再编辑」在切 tab / 关窗口 / 连续新增时残留未保存行）。子菜单草稿创建时自动展开父节点。
- 编辑后浮窗下次打开自动加载新数据

---

## 7. 迁移策略

### 7.1 DB schema

- `action_bar_items` 表加入 `db.sql`（`CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE` 种子数据）
- `init_schema` 的 `user_version` 从 18 bump 到 19
- 已有 DB（v18）→ 重新执行 `db.sql`（IF NOT EXISTS 幂等）→ 新表创建 + 种子写入

### 7.2 代码迁移

- 保留现有 `trigger_action_bar` / `action_bar_dismiss` 不变；`action_bar_show_result` 加 `write_clipboard: bool` 参数（AI 路径 true，Script 同步路径按 `write_output_to_clipboard` 配置 opt-in）——详见[脚本增强 spec §5](2026-07-10-action-bar-script-enhancement-design.md)
- 删除 `run_ai_action`（合并进 `execute_action_bar`）
- 删除 `action_bar_open_url`（合并进 `execute_action_bar` 的 url 分支）
- 前端删除 SEARCH_URLS 常量

### 7.3 `action_bar_search_engine` 配置项

迁移后搜索引擎不再是全局配置，而是子菜单默认高亮项。`action_bar_search_engine` 配置项保留但语义改为：进入搜索子菜单时预选哪个搜索引擎（按 title 匹配，非 id）。

---

## 8. 不在本次范围

- **纯文本 Snippet 导入**（`#octopus` 格式粘贴安装）——二期
- **JSON 导入/导出**（菜单配置分享）——二期
- **正则上下文规则**（OnText 式，选中特定格式才显示对应动作）——二期
- **截图+OCR fallback**——已有能力，二期串联
- ~~**python 脚本类型**——DB schema 已支持，一期可不 seed 示例~~ **已实现**（含 node/deno/bun/javascript/typescript，详见[脚本增强 spec](2026-07-10-action-bar-script-enhancement-design.md)）
- **子菜单嵌套超过两级**——当前 parent_id 只支持两级，三级以上二期

---

## 9. 后续演进

- **纯文本 Snippet 导入**：粘贴 `#octopus\nname: xxx\n...` 自动解析进 DB
- **JSON 导入/导出**：用户导出菜单配置 JSON，其他人导入
- **正则上下文规则**：action_bar_items 加 `context_regex` 列，匹配才显示
- **三级+菜单**：parent_id 已支持多级，前端渲染递归化
- **Accessibility API 直读**：替代 Cmd+C（macOS 专属增强）
- **自动弹出**：选中文本自动触发（不做，调研验证误触多）
