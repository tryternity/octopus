# Action Bar 菜单数据库化设计

> **状态**：已实现（Task 1-6 全部完成）
> **日期**：2026-07-09
> **scope**：将 action bar 硬编码菜单迁移为 DB 表管理，支持两级菜单（主菜单 + 子菜单）+ 5 种动作类型 + 用户自定义扩展
> **调研依据**：[`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md)（PopClip/SnipDo/OnText/Click to Do 调研）

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
| `script` | 脚本源码，第一行 magic comment 指定语言 + `{text}` 占位符 | 按注释分发运行时 | 按语言 |
| `copy` | （空） | `write_clipboard_text` | 全平台 |

### 3.2 script 语言分发

script 类型的 `action_data` 第一行必须是 magic comment：

| magic comment | 运行时 | 平台 |
|---------------|--------|------|
| `#shell` | `sh -c "<script>"` | 全平台 |
| `#osascript` | `osascript -e "<script>"` | 仅 macOS |
| `#powershell` | `powershell -Command "<script>"` | 仅 Windows |
| `#python` | `python3 -c "<script>"`（需 PATH 可用） | 全平台（预留，一期可选） |

平台不支持时 → 前端 toast 报 `不支持的平台`，菜单项仍显示。

### 3.3 `{text}` 占位符

`url` 和 `script` 类型的 `action_data` 中 `{text}` 会被运行时替换为选中文本（URL 编码后）。例：
- `url`：`https://www.google.com/search?q={text}` → `https://www.google.com/search?q=hello`
- `script`：`echo "{text}" | pbcopy` → `echo "hello" | pbcopy`

### 3.4 翻译特殊处理

翻译需要按 CJK 检测方向选择 prompt，不能纯静态。`ai` 类型的 `action_data` 支持 `auto_translate` 关键字——运行时检测选中文本是否含 CJK 字符，选择中译英或英译中 prompt。

### 3.5 典型用例

**问豆包（三种方式）**：
- URL scheme：`url` → `doubao://?text={text}`
- AppleScript：`script` → `#osascript\ntell application "豆包" to activate`
- Shell：`script` → `#shell\nopen -a "豆包" && sleep 1 && osascript -e 'tell application "System Events" to keystroke "v" using command down'`

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

```rust
#[tauri::command]
pub async fn execute_action_bar(item_id: i64, text: String, app: AppHandle) -> Result<(), String> {
    let item = load_action_bar_item(item_id)?;
    match item.action_type.as_str() {
        "ai" => {
            let prompt = if item.action_data == "auto_translate" {
                auto_translate_prompt(&text)
            } else {
                item.action_data.clone()
            };
            let result = octopus_llm::chat_text_with_prompt(&prompt, &text, &llm_config)?;
            action_bar_show_result(result, text, item.title, app);
        }
        "url" => {
            let url = if item.action_data.is_empty() {
                text  // 选中文本即 URL
            } else {
                item.action_data.replace("{text}", &urlencode(&text))
            };
            open_url(&url);
        }
        "script" => {
            run_script(&item.action_data, &text)?;
        }
        "copy" => {
            write_clipboard_text(&app, &text);
        }
        _ => {}
    }
    Ok(())
}
```

### 5.3 script 执行

```rust
fn run_script(source: &str, text: &str) -> Result<(), String> {
    let first_line = source.lines().next().unwrap_or("").trim();
    let body: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");
    let script = body.replace("{text}", text);

    match first_line {
        "#shell" => std::process::Command::new("sh").arg("-c").arg(&script).spawn(),
        "#osascript" => {
            #[cfg(target_os = "macos")]
            { std::process::Command::new("osascript").arg("-e").arg(&script).spawn() }
            #[cfg(not(target_os = "macos"))]
            { return Err("osascript 仅 macOS 支持".into()); }
        }
        "#powershell" => {
            #[cfg(target_os = "windows")]
            { std::process::Command::new("powershell").arg("-Command").arg(&script).spawn() }
            #[cfg(not(target_os = "windows"))]
            { return Err("powershell 仅 Windows 支持".into()); }
        }
        "#python" => std::process::Command::new("python3").arg("-c").arg(&script).spawn(),
        _ => return Err(format!("未知脚本类型: {}", first_line)),
    }.map_err(|e| e.to_string())?;
    Ok(())
}
```

---

## 6. 前端变更

### 6.1 浮窗（ActionBar/index.tsx）

- 删除硬编码的 mainItems / aiItems / searchItems / SEARCH_URLS
- mount 时 `invoke("list_action_bar_items")` 加载全部菜单项
- 按 parentId 构建两级结构（`#[serde(rename_all = "camelCase")]` 确保 JSON 字段名匹配）
- `executeMain` / `executeSubItem` 合并为统一的 `executeItem(item: ActionBarItem)`
- `ai` 类型仍走前端 loading + 超时 + timedOutRef 流程
- `url` / `script` / `copy` 直接 `invoke("execute_action_bar")`
- 按钮布局：**水平「图标+文字」一行排列**（`flex-row`），非上下两行——浮窗更矮，子菜单展开后总高 ~72px
- 视觉：`rounded-2xl` + `backdrop-blur-xl` 毛玻璃 + `shadow-2xl`
- 窗口尺寸 380×72px（水平排列需更宽）

### 6.2 图标渲染

新增 `ActionBarIcon` 组件（`components/ActionBarIcon.tsx`），三层渲染逻辑：

1. **文件名（`action-ai.svg`）**→ `fetch("/icons/{name}.svg")` 加载完整 SVG → 提取 inner HTML → 重组 `<svg>` 强制 `stroke/fill="currentColor"` → `<i dangerouslySetInnerHTML>`
2. **内联 SVG（`<svg>...`）**→ 直接渲染
3. **Lucide 预置名（`pencil` 等）**→ `<svg>` + 预置 path 组装

⚠️ **踩坑**：(1) React `<svg>` + `dangerouslySetInnerHTML` 注入 `<path>` 时 `currentColor` 继承不稳定 → 改为 `<i>` + 完整 SVG 字符串；(2) `ActionBarItem` struct 缺 `#[serde(rename_all = "camelCase")]` → JSON 字段 snake_case → 前端 camelCase 读不到 → 菜单完全不渲染。

### 6.3 设置页

新增 ActionBarPanel（或 GeneralPanel 内区块），CRUD 界面：

- 树形展示两级菜单（主菜单项 + 可展开的子菜单）
- 每项：标题 / 图标 / 类型 / 排序按钮（↑↓）/ 编辑 / 删除
- `is_system=1`：删除按钮灰掉，可编辑内容但不能改类型
- 编辑表单：标题、图标（文件名或 SVG 源码）、类型（下拉）、内容（textarea，按类型提示不同占位符）、启用开关
- 新增按钮：选 parent（主菜单 or 某子菜单组）
- 编辑后浮窗下次打开自动加载新数据

---

## 7. 迁移策略

### 7.1 DB schema

- `action_bar_items` 表加入 `db.sql`（`CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE` 种子数据）
- `init_schema` 的 `user_version` 从 18 bump 到 19
- 已有 DB（v18）→ 重新执行 `db.sql`（IF NOT EXISTS 幂等）→ 新表创建 + 种子写入

### 7.2 代码迁移

- 保留现有 `trigger_action_bar` / `action_bar_dismiss` / `action_bar_show_result` 不变
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
- **python 脚本类型**——DB schema 已支持，一期可不 seed 示例
- **子菜单嵌套超过两级**——当前 parent_id 只支持两级，三级以上二期

---

## 9. 后续演进

- **纯文本 Snippet 导入**：粘贴 `#octopus\nname: xxx\n...` 自动解析进 DB
- **JSON 导入/导出**：用户导出菜单配置 JSON，其他人导入
- **正则上下文规则**：action_bar_items 加 `context_regex` 列，匹配才显示
- **三级+菜单**：parent_id 已支持多级，前端渲染递归化
- **Accessibility API 直读**：替代 Cmd+C（macOS 专属增强）
- **自动弹出**：选中文本自动触发（不做，调研验证误触多）
