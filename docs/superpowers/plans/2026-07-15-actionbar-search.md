# ActionBar 搜索功能 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 ActionBar 从纯菜单条升级为搜索驱动的命令面板：搜索输入框 + Tab 分组结果（应用/文件/Shell/书签）+ Silent Hotkey + Run And Paste。

**Architecture:** 前端 ActionBar 组件重构（输入框 + Tab 栏 + 结果列表 + 展开/收起逻辑），后端新增搜索引擎（应用索引 + mdfind 文件搜索 + 书签解析 + nucleo-matcher 模糊匹配），DB 加 `trigger_keyword` + `auto_paste` 字段。

**Tech Stack:** Rust, Tauri 2, React/TypeScript, nucleo-matcher

**Spec:** `docs/superpowers/specs/2026-07-15-actionbar-search-design.md`

## Global Constraints

- 输入框为空时 → 菜单条显示（现有行为不变）
- 输入框有内容时 → Tab + 搜索结果替代菜单条
- 搜索结果最多 10 行可见，无结果时透明+穿透
- 展开方向：屏幕下半部分向下展开，上半部分向上展开
- Tab 键循环：搜索框 → Tab 页 → 搜索框
- 即时搜索（应用+菜单+Quicklinks）<16ms，延迟搜索（文件+书签）150ms 防抖
- 匹配优先级：精确 > 前缀 > 模糊 > 拼音；同级别：应用 > 文件 > Shell > 其他
- Silent Hotkey 不弹面板（除非 auto_paste=false 首次确认）

---

### Task 1: DB v32 — trigger_keyword + auto_paste 字段 ✅

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`（v31→v32 迁移）

- [ ] **Step 1: db.sql action_bar_items 加字段**

在 `action_bar_items` 表 CREATE 语句的列定义末尾加：
```sql
trigger_keyword TEXT NOT NULL DEFAULT '',
auto_paste INTEGER NOT NULL DEFAULT 0,
```

- [ ] **Step 2: db.rs v31→v32 迁移**

```rust
// v31→v32：action_bar_items 加 trigger_keyword + auto_paste
{
    let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if !cols.contains(&"trigger_keyword".to_string()) {
        conn.execute("ALTER TABLE action_bar_items ADD COLUMN trigger_keyword TEXT NOT NULL DEFAULT ''", [])?;
    }
    if !cols.contains(&"auto_paste".to_string()) {
        conn.execute("ALTER TABLE action_bar_items ADD COLUMN auto_paste INTEGER NOT NULL DEFAULT 0", [])?;
    }
    conn.execute("PRAGMA user_version = 32", [])?;
    log::info!("schema upgraded to v32 (action_bar_items: trigger_keyword + auto_paste)");
}
```

更新 `if v >= 31` → `if v >= 32`，全新库 `PRAGMA user_version = 32`。更新所有测试断言 v31→v32。

- [ ] **Step 3: 编译 + 测试**

Run: `cargo test -p octopus-infra`
Expected: PASS

- [ ] **Step 4: Commit**

---

### Task 2: nucleo-matcher 集成 + 搜索核心模块 ✅

**Files:**
- Modify: `crates/desktop/Cargo.toml`（加 nucleo-matcher 依赖）
- Create: `crates/desktop/src/search/mod.rs`（搜索模块入口）
- Create: `crates/desktop/src/search/matcher.rs`（模糊匹配 + 拼音）
- Create: `crates/desktop/src/search/app_index.rs`（应用索引）
- Create: `crates/desktop/src/search/bookmark.rs`（书签解析）
- Create: `crates/desktop/src/search/file_search.rs`（mdfind 文件搜索）

**Interfaces:**
- Produces: `search::SearchEngine` 结构体，持有应用/书签索引
- Produces: `search::SearchResult` 统一结果结构
- Produces: `search::search_all(query, tab) -> Vec<SearchResult>`

- [ ] **Step 1: 加 nucleo-matcher 依赖**

```toml
[dependencies]
nucleo-matcher = "0.3"
```

- [ ] **Step 2: SearchResult 统一结构**

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub source: String,       // "app" | "file" | "menu" | "bookmark" | "quicklink" | "shell"
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>, // base64 或空
    pub action_type: String,  // "launch_app" | "open_file" | "menu" | "url" | "shell"
    pub action_data: String,  // JSON: { path/url/command/... }
    pub score: i32,           // 匹配得分（排序用）
}
```

- [ ] **Step 3: matcher.rs — 模糊匹配 + 拼音首字母**

```rust
pub fn fuzzy_match(query: &str, targets: &[String]) -> Vec<(usize, i32)> {
    // nucleo-matcher 对 targets 做模糊匹配，返回 (index, score)
}

pub fn pinyin_initials(text: &str) -> String {
    // 中文 → 拼音首字母（如 "翻译" → "fy"）
    // 简单实现：硬编码常用菜单项首字母表，或用 pinyin crate
}
```

拼音处理：初期可硬编码 `action_bar_items` 常用菜单名（翻译=fy, 搜索=ss, 润色=rs），或引入 `pinyin` crate。

- [ ] **Step 4: app_index.rs — 应用索引**

```rust
pub struct AppEntry {
    pub name: String,
    pub bundle_name: String,
    pub path: String,
}

pub struct AppIndex {
    apps: Vec<AppEntry>,
}

impl AppIndex {
    pub fn scan() -> Self {
        // 扫描 /Applications/, ~/Applications/, /System/Applications/
        // 读 Info.plist 取 CFBundleName
    }

    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        // 模糊匹配 name + bundle_name
    }
}
```

- [ ] **Step 5: bookmark.rs — 书签解析**

```rust
pub struct BookmarkEntry {
    pub title: String,
    pub url: String,
    pub browser: String,
}

pub fn load_bookmarks() -> Vec<BookmarkEntry> {
    // Safari: ~/Library/Safari/Bookmarks.plist
    // Chrome: ~/Library/Application Support/Google/Chrome/Default/Bookmarks
    // Edge: ~/Library/Application Support/Microsoft Edge/Default/Bookmarks
    // Firefox: places.sqlite（可能跳过，太复杂）
}
```

- [ ] **Step 6: file_search.rs — mdfind 文件搜索**

```rust
pub async fn search_files(query: &str) -> Vec<SearchResult> {
    // std::process::Command::new("mdfind")
    //     .args(["-name", query, "-onlyin", home_dir])
    //     .output()
    // 限制 10 条结果
}
```

- [ ] **Step 7: search/mod.rs — 统一搜索引擎**

```rust
pub struct SearchEngine {
    app_index: AppIndex,
    bookmarks: Vec<BookmarkEntry>,
}

impl SearchEngine {
    pub fn new() -> Self {
        // 启动时扫描应用 + 书签
    }

    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        // 根据 tab 过滤来源
        // 即时搜索：apps + menus + quicklinks
        // 延迟搜索：files + bookmarks（150ms 防抖在调用方处理）
        // 混合排序
    }
}
```

- [ ] **Step 8: 编译 + 单测**

Run: `cargo build -p octopus-desktop --features embedded`
Run: `cargo test -p octopus-desktop --features embedded --bin octopus-desktop search`
Expected: PASS

- [ ] **Step 9: Commit**

---

### Task 3: Tauri 命令 — 搜索 + 执行 ✅

**Files:**
- Modify: `crates/desktop/src/main.rs`（注册命令）
- Create: `crates/desktop/src/search_commands.rs`（Tauri 命令）

- [ ] **Step 1: 搜索命令**

```rust
static SEARCH_ENGINE: OnceLock<SearchEngine> = OnceLock::new();

#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<Vec<SearchResult>, String> {
    let engine = SEARCH_ENGINE.get_or_init(|| SearchEngine::new());
    Ok(engine.search(&query, &tab).await)
}

#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn execute_shell(command: String) -> Result<String, String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c").arg(&command)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
```

- [ ] **Step 2: 注册命令**

invoke_handler 加 `search_all`、`launch_app`、`open_file`、`execute_shell`。

- [ ] **Step 3: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 4: Commit**

---

### Task 4: 前端 — 搜索输入框 + Tab 栏 + 结果列表 ✅

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`
- Create: `crates/desktop/frontend/src/pages/ActionBar/SearchPanel.tsx`
- Create: `crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts`
- Create: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts`
- Create: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.test.ts`
- Modify: `crates/search/src/engine.rs`（加 "quick" + "files_bookmarks" tab 支持即时/延迟分离）
- Modify: `crates/search/src/app_index.rs` + `bookmark.rs`（清理未用导入）

- [x] **Step 1: 搜索输入框组件** — 始终显示在 ActionBar 顶部（向下展开）或底部（向上展开），
  无选中文本时自动聚焦。输入框有内容时隐藏菜单条，显示搜索结果。

- [x] **Step 2: Tab 栏 + 结果列表组件** — SearchPanel 组件含 5 个 Tab
  （`? 全部` `a 应用` `f 文件` `> Shell` `b 书签`）+ 最多 10 行结果列表。

- [x] **Step 3: 展开方向判定** — show 时通过 `window.outerPosition()` + `window.screen.height`
  计算展开方向（向下/向上），一次 show 中固定。

- [x] **Step 4: 搜索请求（即时 / 延迟）** — 即时搜索调用 `search_all(query, "quick")`
  （应用+菜单+Quicklinks，无防抖），延迟搜索调用 `search_all(query, "files_bookmarks")`
  （文件+书签，150ms 防抖，query ≥ 2 字符）。后端新增 "quick" + "files_bookmarks" tab
  支持即时/延迟搜索分离。

- [x] **Step 5: 键盘导航** — 完整实现 spec 中的键盘导航表：
  - 输入框：Tab/↑↓ → 结果区，Enter → 执行首个，Escape → 清空
  - 结果区：↑↓ 导航，Enter 执行，Tab 循环 Tab 页，?/a/f/>/b 跳转，i 回输入框
  - 输入框聚焦时菜单快捷键不干扰（防止字符进入输入框时同时触发菜单导航）

- [x] **Step 6: 结果执行** — 按 actionType 分发：launch_app / open_file / menu / url / shell

- [x] **Step 7: 前端编译** — `npm run build` PASS

- [x] **Step 8: 测试** — searchLogic.test.ts 55 个单元测试 + 前端全量 213 个测试 PASS

**实际偏差/新增决策：**
- 搜索逻辑提取为纯函数模块 `searchLogic.ts`（15 个函数），配套 55 个单元测试
- 类型定义独立为 `searchTypes.ts`（SearchResult / TabId / TABS / ExpandDirection 等）
- 后端引擎加 "quick" + "files_bookmarks" tab（原计划未提及），用于即时/延迟搜索分离
- 引擎新增 4 个测试覆盖新 tab 行为（quick_tab_excludes / files_bookmarks_tab_excludes / quick_tab_shell / all_tab_combined）

- Tab 栏在搜索结果上方（向下展开）或下方（向上展开）
- 结果列表最多 10 行，每行：图标 + 标题 + 副标题
- 无结果时透明 + pointer-events: none

- [ ] **Step 3: 展开方向判定**

```typescript
const rect = inputRef.current?.getBoundingClientRect();
if (rect) {
    const spaceBelow = window.screen.height - rect.bottom;
    setExpandDown(spaceBelow > 400);
}
```

- [ ] **Step 4: 搜索请求（防抖 50ms 即时 / 150ms 延迟）**

```typescript
// 即时搜索（应用+菜单+Quicklinks）
useEffect(() => {
    if (!query) return;
    invoke<SearchResult[]>("search_all", { query, tab: activeTab })
        .then(setResults);
}, [query]); // 无防抖

// 延迟搜索（文件+书签，150ms 防抖，query ≥ 2 字符）
useEffect(() => {
    if (query.length < 2) return;
    const timer = setTimeout(() => {
        invoke<SearchResult[]>("search_all", { query, tab: "files_bookmarks" })
            .then(setDelayedResults);
    }, 150);
    return () => clearTimeout(timer);
}, [query]);
```

- [ ] **Step 5: 键盘导航**

| 焦点 | 按键 | 行为 |
|------|------|------|
| 搜索框 | Tab | → `[? 全部]` 第一个结果 |
| 搜索框 | ↑↓ | → 结果列表项 |
| Tab 页 | Tab | 循环：Tab 页 → ... → 搜索框 |
| Tab 页 | ?/a/f/>/b | 跳转对应 Tab |
| Tab 页 | i | 回搜索框 |
| 结果项 | ↑↓ | 导航 |
| 结果项 | Enter | 执行 |

- [ ] **Step 6: 结果执行**

根据 `action_type` 分发：
- `launch_app` → `invoke("launch_app", { path })`
- `open_file` → `invoke("open_file", { path })`
- `menu` → 现有 action bar 菜单执行逻辑
- `url` → `invoke("open_url", { url })` 或 window.open
- `shell` → `invoke("execute_shell", { command })` + 显示输出

- [ ] **Step 7: 前端编译**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 8: Commit**

---

### Task 5: Quicklinks 关键词触发 ✅

**Files:**
- Modify: `crates/desktop/src/search_commands.rs`
- Modify: `crates/desktop/frontend/src/pages/ActionBar/SearchPanel.tsx`

- [ ] **Step 1: 后端 Quicklink 搜索**

搜索引擎中检测 `<keyword> <query>` 模式：
```rust
// 在 search() 中：如果 query 以某个 trigger_keyword + 空格开头
// 匹配 action_bar_items WHERE trigger_keyword = first_word
// 替换 action_data URL 中的 {query} 为剩余部分
```

- [ ] **Step 2: 前端 Quicklink 结果执行**

Quicklink 结果点击 → 浏览器打开替换后的 URL。

- [ ] **Step 3: 设置页加 trigger_keyword 编辑**

`ActionBarPanel.tsx` 编辑表单加 `trigger_keyword` 输入框（仅 `url` 类型显示）。

- [ ] **Step 4: 编译**

Run: `cargo build -p octopus-desktop --features embedded && cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 6: Silent Query Hotkey + Run And Paste ✅

**实际实现决策：** Silent Hotkey 全局热键注册未实现（需确定 modifier 方案），
重点实现了 Run And Paste 核心逻辑：auto_paste 模式下执行后直接粘贴结果到光标，
不弹 CompactEditor。autoPaste 开关在设置页 AI/Script 类型编辑表单中。

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（全局热键注册 + silent handler）
- Modify: `crates/desktop/src/coordinator.rs`（Run And Paste 逻辑）
- Modify: `crates/desktop/frontend/src/pages/ActionBar/SearchPanel.tsx`（确认 UI）

- [ ] **Step 1: 全局热键注册**

启动时扫描 `action_bar_items` 中有 `shortcut` 字段的项，注册全局热键。handler 中：
```rust
if item.auto_paste == 1 {
    // Run And Paste: 执行 → 结果写剪贴板 → CGEvent ⌘V
} else {
    // 弹面板显示结果 + 「下次直接粘贴」按钮
}
```

- [ ] **Step 2: Run And Paste 实现**

复用现有 paste 逻辑：
```rust
// 1. 执行菜单项（AI/翻译/脚本等）
// 2. 结果写入剪贴板（已有 set_clipboard）
// 3. 模拟 ⌘V（已有 CGEvent post paste）
```

- [ ] **Step 3: 前端确认 UI**

面板底部加提示条：「下次直接粘贴，不再确认？」+ `[记住]` 按钮。
点击记住 → `invoke("set_auto_paste", { id, value: true })`。

- [ ] **Step 4: 编译 + 前端**

Run: `cargo build -p octopus-desktop --features embedded && cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 7: 全量编译 + 测试 + 文档同步 ✅

- [ ] **Step 1: 全量编译**

Run: `cargo build --release -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 2: 全量测试**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: 前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 4: 更新 architecture.md**

- [ ] **Step 5: Commit**
