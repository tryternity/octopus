# Action Bar 菜单数据库化 — 实施记录

> plan 是实施记录，非一次性待办。以下为实际执行过程中的全部提交和偏差回写。

**Goal:** 将 action bar 硬编码菜单迁移为 DB 表管理，支持两级菜单 + 5 种动作类型（submenu/ai/url/script/copy）+ 设置页 CRUD。

**Architecture:** 新建 `action_bar_items` DB 表（自引用 parent_id 两级菜单）+ DB 层 CRUD（infra/db.rs）+ Tauri 命令层 + 统一执行入口 `execute_action_bar` + 前端动态加载菜单 + 设置页管理 UI。

**Tech Stack:** Rust + SQLite + Tauri 2 + React + TypeScript

## Global Constraints

- **DB schema**：`CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE` 幂等种子。`user_version` 从 18 bump 到 19。现有 v18 DB 重新执行 db.sql 自动建新表 + seed。
- **is_system 保护**：内置项（`is_system=1`）不可删除；可编辑内容但不可改 action_type。
- **script magic comment**：第一行 `#shell` / `#osascript` / `#powershell` / `#python` 决定运行时；平台不支持返回错误而非隐藏菜单项。
- **图标三种格式**：(1) `.svg` 文件名 → `fetch("/icons/{name}.svg")` → 提取 inner HTML → 重组 SVG 强制 `currentColor`；(2) `<svg>` 开头 → 内联渲染；(3) Lucide 预置名 → 组装。⚠️ 必须用 `<i dangerouslySetInnerHTML>` 注入完整 SVG 字符串（React `<svg>` + innerHTML 注入 `<path>` 的 `currentColor` 继承不稳定）。
- **`#[serde(rename_all = "camelCase")]`**：`ActionBarItem` struct 必须加此 attribute，否则 JSON 字段 `parent_id` → 前端 `parentId` 读不到 → 菜单完全不渲染（已踩坑）。
- **选中文本传递**：url 类型用 `{text}` 占位符替换（URL 编码）；script 类型通过环境变量 `$OCTOPUS_TEXT` 传递（不做字符串替换，防 shell 注入）。
- **翻译特殊处理**：ai 类型 action_data 为 `auto_translate` 时按 CJK 检测方向。
- **已有基础设施复用**：`chat_text_with_prompt`（LLM 调用）、`finalize_action_bar`（出口收口）、`timedOutRef`（前端超时）。
- **Tauri 命令注册**：新命令必须加入 `main.rs` 的 `invoke_handler` 列表，否则前端 invoke 被拒。
- **按钮布局**：水平「图标+文字」一行排列（`flex-row`），窗口宽度 380px，高度按 view 动态调整（主菜单 40px / 子菜单 76px）。浮窗在用户内容上方，必须矮。
- **窗口焦点策略（⚠️ 强需求，勿改错）**：全局快捷键不得将 settings/compact_editor 带到前台。macOS WKWebView 需 app active 才有键盘焦点，但 `set_focus` 的 `activate` 会带出 Regular 窗口。方案：show 前记录前台 app + 隐藏 Regular → set_focus → hide 时先交还前台焦点再恢复 Regular（`activation::before_floating_window_show` / `after_floating_window_hide`）。`FLOAT_DEPTH` 引用计数支持多浮窗嵌套。`action_bar_show_result` 不调 deactivate（避免 CompactEditor 被压后台）。action bar + 剪贴板共用，语音识别窗无强键盘需求不处理。
- **script 超时**：`run_script` 后台 `try_wait` 轮询 60 秒后 `kill`，防止僵尸进程 + 线程泄漏。
- **键盘导航（⚠️ 强需求，勿改错）**：**上下键切换主子菜单层级（focusLayer main↔sub），左右键在当前行移动选择。** 子菜单展开/收起由左右键控制（移到 submenu 项展开、移到非 submenu 项收起），上下键只切焦点不碰视图。焦点层（`focusLayer`）独立于视图层（`view`）——左右键展开子菜单时不抢焦点，必须上下键才进入。**Esc 直接关闭浮窗**（一次 Esc，不退焦点层，不做两次 Esc）。

---

### Task 1: DB 表 + 种子数据

**Files:**
- Modify: `crates/infra/src/db.sql`（追加 `action_bar_items` 表定义 + 种子）
- Modify: `crates/infra/src/db.rs:168-194`（user_version 18→19）

**Interfaces:**
- Produces: `action_bar_items` 表（DB 层，供 Task 2 查询）

- [ ] **Step 1: 在 db.sql 末尾追加 action_bar_items 表定义 + 种子**

在 `crates/infra/src/db.sql` 末尾（line 259 `action_bar_search_engine` 那行之后）追加：

```sql

-- Action Bar 菜单项（两级菜单，自引用 parent_id）
CREATE TABLE IF NOT EXISTS action_bar_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id   INTEGER DEFAULT NULL,
    title       TEXT NOT NULL,
    icon        TEXT NOT NULL DEFAULT '',
    action_type TEXT NOT NULL,
    action_data TEXT NOT NULL DEFAULT '',
    sort_order  INTEGER NOT NULL DEFAULT 0,
    is_system   INTEGER NOT NULL DEFAULT 1,
    is_enabled  INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);

-- 种子数据：主菜单项（parent_id=NULL）
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (1, NULL, 'AI',    'sparkles', 'submenu', '', 0, 1),
    (2, NULL, '翻译',  'globe',    'ai', 'auto_translate', 1, 1),
    (3, NULL, '搜索',  'search',   'submenu', '', 2, 1),
    (4, NULL, '网页',  'link',     'url', '', 3, 1);

-- 种子数据：AI 子菜单（parent_id=1）
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (5, 1, '润色', 'pencil',     'ai', '请对以下文本进行润色，使其更加流畅、专业。保持原意不变。只输出润色结果。', 0, 1),
    (6, 1, '摘要', 'file-text',  'ai', '请用简洁的中文总结以下内容的要点，不超过 3 句话。只输出总结。', 1, 1),
    (7, 1, '解释', 'lightbulb',  'ai', '请用简洁的中文解释以下内容的含义。只输出解释。', 2, 1);

-- 种子数据：搜索子菜单（parent_id=3）
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (8, 3, 'Google', 'search', 'url', 'https://www.google.com/search?q={text}', 0, 1),
    (9, 3, '百度',   'search', 'url', 'https://www.baidu.com/s?wd={text}', 1, 1),
    (10, 3, 'Bing',  'search', 'url', 'https://www.bing.com/search?q={text}', 2, 1);
```

注意：种子用显式 id（1-10），与 prompts 表 seed 方式一致。parent_id 直接写死数字，不依赖 AUTOINCREMENT。

- [ ] **Step 2: bump user_version 18→19**

在 `crates/infra/src/db.rs` 的 `init_schema` 函数中，将三处 `18` 改为 `19`：

```rust
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v >= 19 { return Ok(()); }              // 已最新
    if v >= 17 {
        // v17→v18 FTS5 backfill（保持不变）
        conn.execute_batch(
            "INSERT INTO clipboard_history_fts(rowid, content)
             SELECT id, content FROM clipboard_history
             WHERE content != ''
               AND id NOT IN (SELECT rowid FROM clipboard_history_fts)"
        )?;
        // v18→v19: action_bar_items 表由 db.sql 的 IF NOT EXISTS 自动创建，
        // 无需额外迁移——但需重跑 db.sql 以建新表
        conn.execute_batch(INIT_SQL).ok(); // 幂等，已存在的表跳过
        conn.execute("PRAGMA user_version = 19", [])?;
        return Ok(();
    }

    conn.execute_batch(INIT_SQL).context("执行 db.sql 建表 + seed")?;
    migrate_yaml_to_db(conn)?;
    conn.execute("PRAGMA user_version = 19", [])?;
    Ok(())
}
```

- [ ] **Step 3: 验证编译 + 运行测试**

Run: `cargo build -p octopus-infra && cargo test -p octopus-infra`
Expected: 编译通过，测试全过

- [ ] **Step 4: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat: action_bar_items DB 表 + 种子数据（user_version 19）"
```

---

### Task 2: DB 层 CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（追加 ActionBarItem struct + CRUD 函数，放在 prompts CRUD 之后）
- Test: `crates/infra/src/db.rs`（内联 `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: `action_bar_items` 表（Task 1）
- Produces: `ActionBarItem` struct + `list_action_bar_items()` / `load_action_bar_item(id)` / `insert_action_bar_item(...)` / `update_action_bar_item(...)` / `delete_action_bar_item(id)` / `move_action_bar_item(id, direction)`

- [ ] **Step 1: 写 ActionBarItem struct + row mapper**

在 `crates/infra/src/db.rs` 的 prompts CRUD 之后（约 line 830）追加：

```rust
// ── Action Bar 菜单项 ──

#[derive(Debug, Clone, serde::Serialize)]
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

const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled";

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
    })
}
```

- [ ] **Step 2: 写 list + load 函数**

```rust
fn list_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items WHERE is_enabled=1 ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn list_action_bar_items() -> Result<Vec<ActionBarItem>> {
    with_db(list_action_bar_items_at)
}

fn load_action_bar_item_at(conn: &Connection, id: i64) -> Result<Option<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM action_bar_items WHERE id=?1", ACTION_BAR_SELECT_COLS)
    )?;
    let mut rows = stmt.query_map(params![id], row_to_action_bar_item)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

pub fn load_action_bar_item(id: i64) -> Result<Option<ActionBarItem>> {
    with_db(|conn| load_action_bar_item_at(conn, id))
}
```

注意：`list_action_bar_items` 只返回 `is_enabled=1` 的项（浮窗只显示启用的）。设置页需要看全部——另写一个 `list_all_action_bar_items` 不带 is_enabled 过滤。

```rust
fn list_all_action_bar_items_at(conn: &Connection) -> Result<Vec<ActionBarItem>> {
    let mut stmt = conn.prepare(
        &format!(
            "SELECT {} FROM action_bar_items ORDER BY parent_id IS NOT NULL, parent_id ASC, sort_order ASC",
            ACTION_BAR_SELECT_COLS
        )
    )?;
    let rows = stmt.query_map([], row_to_action_bar_item)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn list_all_action_bar_items() -> Result<Vec<ActionBarItem>> {
    with_db(list_all_action_bar_items_at)
}
```

- [ ] **Step 3: 写 insert + update + delete 函数**

```rust
fn insert_action_bar_item_at(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
) -> Result<i64> {
    // 新项 sort_order = 同 parent 下最大值 + 1
    let max_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) FROM action_bar_items WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_action_bar_item(
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data))
}

fn update_action_bar_item_at(
    conn: &Connection,
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    // is_system=1 不允许改 action_type
    if row.is_system && row.action_type != action_type {
        anyhow::bail!("系统内置菜单项不可更改动作类型");
    }
    conn.execute(
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, updated_at=datetime('now') WHERE id=?6",
        params![title, icon, action_type, action_data, is_enabled as i32, id],
    )?;
    Ok(())
}

pub fn update_action_bar_item(
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
) -> Result<()> {
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled))
}

fn delete_action_bar_item_at(conn: &Connection, id: i64) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    if row.is_system {
        anyhow::bail!("系统内置菜单项不可删除");
    }
    // 删除自身 + 子项（CASCADE）
    conn.execute("DELETE FROM action_bar_items WHERE id=?1 OR parent_id=?1", params![id])?;
    Ok(())
}

pub fn delete_action_bar_item(id: i64) -> Result<()> {
    with_db(|conn| delete_action_bar_item_at(conn, id))
}
```

- [ ] **Step 4: 写 move 函数（上移/下移交换 sort_order）**

```rust
fn move_action_bar_item_at(conn: &Connection, id: i64, direction: i32) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    // 找同 parent 下相邻项
    let neighbor_id: Option<i64> = if direction < 0 {
        // 上移：找 sort_order 小于当前的最大项
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order < ?2 ORDER BY sort_order DESC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    } else {
        // 下移：找 sort_order 大于当前的最小项
        conn.query_row(
            "SELECT id FROM action_bar_items WHERE parent_id IS ?1 AND sort_order > ?2 ORDER BY sort_order ASC LIMIT 1",
            params![row.parent_id, row.sort_order],
            |r| r.get(0),
        ).ok()
    };
    if let Some(nid) = neighbor_id {
        let neighbor = load_action_bar_item_at(conn, nid)?.context("相邻项不存在")?;
        // 交换 sort_order
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![neighbor.sort_order, id])?;
        conn.execute("UPDATE action_bar_items SET sort_order=?1 WHERE id=?2", params![row.sort_order, nid])?;
    }
    Ok(())
}

pub fn move_action_bar_item(id: i64, direction: i32) -> Result<()> {
    with_db(|conn| move_action_bar_item_at(conn, id, direction))
}
```

- [ ] **Step 5: 写测试**

在 `crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 中追加（如果不存在则新建）：

```rust
#[test]
fn action_bar_items_seed_has_10_items() {
    // 确保测试前 DB 已初始化
    let _ = crate::db::ensure_db();
    let items = crate::db::list_all_action_bar_items().unwrap();
    assert!(items.len() >= 10, "expected >=10 seed items, got {}", items.len());
}

#[test]
fn action_bar_items_list_enabled_filters_disabled() {
    let _ = crate::db::ensure_db();
    // 插入一个禁用项
    let id = crate::db::insert_action_bar_item(None, "测试禁用", "test", "copy", "").unwrap();
    crate::db::update_action_bar_item(id, "测试禁用", "test", "copy", "", false).unwrap();
    // list_action_bar_items 只返回 enabled
    let enabled = crate::db::list_action_bar_items().unwrap();
    assert!(!enabled.iter().any(|i| i.id == id));
    // list_all 包含
    let all = crate::db::list_all_action_bar_items().unwrap();
    assert!(all.iter().any(|i| i.id == id));
    // 清理
    crate::db::delete_action_bar_item(id).unwrap();
}

#[test]
fn action_bar_items_system_item_cannot_delete() {
    let _ = crate::db::ensure_db();
    let result = crate::db::delete_action_bar_item(1); // id=1 是 AI（system）
    assert!(result.is_err());
}

#[test]
fn action_bar_items_move_swaps_order() {
    let _ = crate::db::ensure_db();
    // 插入两个用户项
    let id_a = crate::db::insert_action_bar_item(None, "AAA", "test", "copy", "").unwrap();
    let id_b = crate::db::insert_action_bar_item(None, "BBB", "test", "copy", "").unwrap();
    let a_before = crate::db::load_action_bar_item(id_a).unwrap().unwrap();
    let b_before = crate::db::load_action_bar_item(id_b).unwrap().unwrap();
    assert!(a_before.sort_order < b_before.sort_order);
    // 下移 A
    crate::db::move_action_bar_item(id_a, 1).unwrap();
    let a_after = crate::db::load_action_bar_item(id_a).unwrap().unwrap();
    assert_eq!(a_after.sort_order, b_before.sort_order);
    // 清理
    crate::db::delete_action_bar_item(id_a).unwrap();
    crate::db::delete_action_bar_item(id_b).unwrap();
}
```

- [ ] **Step 6: 运行测试**

Run: `cargo test -p octopus-infra -- action_bar_items`
Expected: 4 tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat: action_bar_items DB CRUD（list/insert/update/delete/move）"
```

---

### Task 3: Tauri 命令层 + 统一执行入口

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（新增 CRUD 命令 + `execute_action_bar` + `run_script` + `auto_translate_prompt`）
- Modify: `crates/desktop/src/main.rs:272-278`（注册新命令）

**Interfaces:**
- Consumes: `ActionBarItem` CRUD（Task 2）
- Produces: Tauri 命令供前端调用

- [ ] **Step 1: 新增 CRUD Tauri 命令**

在 `crates/desktop/src/action_bar_commands.rs` 末尾追加：

```rust
// ── 菜单管理命令（设置页用）──

use octopus_infra::db::ActionBarItem;

#[tauri::command]
pub fn list_action_bar_items() -> Result<Vec<ActionBarItem>, String> {
    octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_action_bar_item(
    parent_id: Option<i64>,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
) -> Result<i64, String> {
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data)
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
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_action_bar_item(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_action_bar_item(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<(), String> {
    octopus_infra::db::move_action_bar_item(id, direction).map_err(|e| e.to_string())
}
```

- [ ] **Step 2: 写 auto_translate_prompt + run_script 辅助函数**

```rust
/// 按 CJK 检测方向，返回翻译 system prompt。
fn auto_translate_prompt(text: &str) -> &'static str {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        "Please translate the following text into English. Only output the translation."
    } else {
        "请将以下文本翻译成中文。只输出翻译结果。"
    }
}

/// 执行脚本：按第一行 magic comment 分发运行时。
fn run_script(source: &str, text: &str) -> Result<(), String> {
    let first_line = source.lines().next().unwrap_or("").trim();
    let body: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");
    let script = body.replace("{text}", text);

    let result: std::io::Result<std::process::Child> = match first_line {
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
        _ => return Err(format!("未知脚本类型: {}（第一行须为 #shell/#osascript/#powershell/#python）", first_line)),
    };

    result.map_err(|e| format!("脚本执行失败: {}", e))?;
    Ok(())
}
```

- [ ] **Step 3: 写统一执行入口 execute_action_bar**

```rust
/// 统一执行菜单项动作。
#[tauri::command]
pub async fn execute_action_bar(item_id: i64, text: String, app: AppHandle) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    match item.action_type.as_str() {
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let prompt = if item.action_data == "auto_translate" {
                auto_translate_prompt(&text)
            } else {
                &item.action_data
            };
            let result = octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)
                .map_err(|e| e.to_string())?;
            // 复用现有 show_result（写剪贴板 + CompactEditor 展示）
            action_bar_show_result(result, text, item.title, app);
        }
        "url" => {
            let url = if item.action_data.is_empty() {
                text.clone()
            } else {
                item.action_data.replace("{text}", &urlencoding::encode(&text))
            };
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("open").arg(&url).spawn(); }
            #[cfg(target_os = "windows")]
            { let _ = std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn(); }
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }
        }
        "script" => {
            run_script(&item.action_data, &text)?;
        }
        "copy" => {
            write_clipboard_text(&app, &text);
        }
        _ => {
            return Err(format!("未知动作类型: {}", item.action_type));
        }
    }

    Ok(())
}
```

注意：`urlencoding` crate 需确认是否已在 Cargo.toml。如果没有，用 `text.replace(...)` + 简单编码替代，或 `percent-encoding` crate（检查现有依赖）。

- [ ] **Step 4: 在 main.rs 注册新命令**

在 `crates/desktop/src/main.rs` 的 `invoke_handler` 中（line 272-278 附近），追加：

```rust
            action_bar_commands::list_action_bar_items,
            action_bar_commands::create_action_bar_item,
            action_bar_commands::update_action_bar_item,
            action_bar_commands::delete_action_bar_item,
            action_bar_commands::move_action_bar_item,
            action_bar_commands::execute_action_bar,
```

- [ ] **Step 5: 检查 urlencoding 依赖**

Run: `grep -r "urlencoding\|percent-encoding" crates/desktop/Cargo.toml crates/infra/Cargo.toml`

如果都没有，检查是否可用已有方式替代。前端目前用 `encodeURIComponent`，后端也可以用简单方式：

```rust
fn simple_url_encode(s: &str) -> String {
    s.chars().map(|c| {
        if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            c.to_string()
        } else {
            format!("%{:02X}", c as u8)
        }
    }).collect()
}
```

- [ ] **Step 6: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop`
Expected: 编译通过，98 测试全过

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs crates/desktop/src/main.rs
git commit -m "feat: execute_action_bar 统一执行入口 + CRUD 命令"
```

---

### Task 4: 前端浮窗动态加载菜单

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`（删除硬编码菜单，改为 DB 加载）
- Create: `crates/desktop/frontend/src/components/ActionBarIcon.tsx`（图标渲染组件）

**Interfaces:**
- Consumes: `list_action_bar_items` + `execute_action_bar`（Task 3）
- Produces: 动态菜单渲染 + 统一执行

- [ ] **Step 1: 创建 ActionBarIcon 组件**

```tsx
// crates/desktop/frontend/src/components/ActionBarIcon.tsx
import { SvgIcon } from "@/components/SvgIcon";

export function ActionBarIcon({ icon, className }: { icon: string; className?: string }) {
  if (icon.startsWith("<svg")) {
    return (
      <span
        className={className}
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "center" }}
        dangerouslySetInnerHTML={{ __html: icon }}
      />
    );
  }
  // 文件名 → SvgIcon mask 方案
  return <SvgIcon name={icon.replace(".svg", "")} className={className} />;
}
```

- [ ] **Step 2: 重构 ActionBar index.tsx — 删除硬编码 + DB 加载**

删除 `SEARCH_URLS` 常量、`mainItems` / `aiItems` / `searchItems` 硬编码数组。

新增类型 + 加载逻辑：

```tsx
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
}

// 组件内
const [menuItems, setMenuItems] = useState<ActionBarItem[]>([]);

// mount 时加载
useEffect(() => {
  invoke<ActionBarItem[]>("list_action_bar_items").then((items) => {
    setMenuItems(items);
  });
}, []);

// 派生
const mainItems = menuItems.filter((i) => i.parentId === null);
const getSubItems = (parentId: number) => menuItems.filter((i) => i.parentId === parentId);
```

- [ ] **Step 3: 统一 executeItem 替换 executeMain/executeSubItem**

```tsx
const executeItem = async (item: ActionBarItem) => {
  const ctx = contextRef.current;
  if (!ctx) return;

  if (item.actionType === "submenu") {
    setSubmenuParentId(item.id);
    // 搜索子菜单默认高亮配置引擎
    const subs = getSubItems(item.id);
    const engineIdx = subs.findIndex((s) => s.title.toLowerCase() === searchEngineRef.current);
    setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
    setView("submenu");
    return;
  }

  if (item.actionType === "ai") {
    executeAiItem(item);
    return;
  }

  // url / script / copy → 直接 invoke
  await invoke("execute_action_bar", { itemId: item.id, text: ctx.text });
  getCurrentWindow().hide();
};

const executeAiItem = async (item: ActionBarItem) => {
  const ctx = contextRef.current;
  if (!ctx) return;
  setView("loading");
  timedOutRef.current = false;

  const timeoutMs = item.actionData === "auto_translate" ? AI_TRANSLATE_TIMEOUT_MS : AI_TIMEOUT_MS;
  const timeoutId = setTimeout(() => {
    timedOutRef.current = true;
    setErrorMsg(`请求超时（${timeoutMs / 1000} 秒），请检查网络或 LLM 配置`);
    setView("error");
  }, timeoutMs);

  try {
    const result = await invoke<string>("execute_action_bar", { itemId: item.id, text: ctx.text });
    clearTimeout(timeoutId);
    if (timedOutRef.current) {
      console.warn("[action-bar] AI result arrived after timeout, discarding");
      return;
    }
    getCurrentWindow().hide();
  } catch (e) {
    clearTimeout(timeoutId);
    if (timedOutRef.current) return;
    setErrorMsg(String(e));
    setView("error");
  }
};
```

注意：`execute_action_bar` 对 ai 类型内部已调 `action_bar_show_result`（写剪贴板 + CompactEditor），前端不再单独调 show_result。

- [ ] **Step 4: 更新键盘导航用 menuItems**

键盘导航的 `mainItemsRef` / `aiItemsRef` / `searchItemsRef` 改为基于 `menuItems` + `submenuParentId`：

```tsx
const submenuParentId = useRef<number | null>(null);
// ...
useEffect(() => {
  const subs = submenuParentId.current !== null
    ? getSubItems(submenuParentId.current)
    : [];
  aiItemsRef.current = subs;
}, [menuItems, submenuParentId.current]);
```

Enter 执行从 `executeMain(id)` / `executeSubItem(id)` 改为 `executeItem(item)`，item 从 ref 数组按 index 取。

- [ ] **Step 5: 更新渲染——IconBtn 改用 ActionBarIcon**

```tsx
const IconBtn = ({ item, active, onClick }: {
  item: ActionBarItem; active: boolean; onClick: () => void;
}) => (
  <button
    className={cn(
      "flex flex-col items-center justify-center gap-0.5 px-3 py-1.5 rounded-md transition-all",
      active
        ? "bg-voice/15 text-voice ring-1 ring-voice/30"
        : "text-muted-foreground hover:bg-muted hover:text-foreground",
    )}
    onMouseDown={(e) => e.stopPropagation()}
    onClick={onClick}
    title={item.title}
  >
    <ActionBarIcon icon={item.icon} className="w-4 h-4" />
    <span className="text-[9px]">{item.title}</span>
  </button>
);
```

- [ ] **Step 6: TypeScript 检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无错误

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx crates/desktop/frontend/src/components/ActionBarIcon.tsx
git commit -m "feat: 前端浮窗动态加载菜单 + 统一 executeItem"
```

---

### Task 5: 设置页菜单管理 UI

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`（挂载 ActionBarPanel）

**Interfaces:**
- Consumes: CRUD 命令（Task 3）

- [ ] **Step 1: 创建 ActionBarPanel 组件**

树形展示两级菜单 + 编辑表单 + 增删改 + 排序。核心结构：

```tsx
export function ActionBarPanel() {
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [editingId, setEditingId] = useState<number | null>(null);

  const refresh = () => {
    invoke<ActionBarItem[]>("list_action_bar_items").then(setItems);
  };

  useEffect(() => { refresh(); }, []);

  const mainItems = items.filter((i) => i.parentId === null);
  const getSubs = (parentId: number) => items.filter((i) => i.parentId === parentId);

  // 渲染：每项一行（标题/类型/启用/排序按钮/编辑/删除）
  // 点击编辑展开表单（标题/icon/类型/内容/启用）
  // 新增按钮
  // ...
}
```

编辑表单字段：
- 标题：text input
- 图标：text input + 实时预览（ActionBarIcon）
- 类型：select（submenu / ai / url / script / copy）
- 内容：textarea，按类型显示不同 placeholder 提示
- 启用：checkbox
- is_system=1：类型 select 禁用（不可改类型）、删除按钮灰掉

- [ ] **Step 2: 在 Settings/index.tsx 挂载 ActionBarPanel**

在设置页 tab 列表中新增 "命令面板" tab，渲染 ActionBarPanel。

- [ ] **Step 3: TypeScript 检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无错误

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx crates/desktop/frontend/src/pages/Settings/index.tsx
git commit -m "feat: 设置页 action bar 菜单管理 UI"
```

---

### Task 6: 清理旧代码 + 文档同步

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（删除 `run_ai_action`）
- Modify: `crates/desktop/src/main.rs`（从 invoke_handler 删除 `run_ai_action`）
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`（清理残留 import）
- Modify: `docs/architecture.md`、`docs/superpowers/specs/2026-07-08-action-bar-design.md`、`docs/superpowers/plans/2026-07-08-action-bar.md`、`docs/superpowers/specs/2026-07-09-action-bar-menu-db-design.md`

- [ ] **Step 1: 删除 run_ai_action**

`run_ai_action` 的功能已完全被 `execute_action_bar` 的 ai 分支替代。删除 `action_bar_commands.rs` 中的 `run_ai_action` 函数，从 `main.rs` invoke_handler 删除 `action_bar_commands::run_ai_action`。

- [ ] **Step 2: 删除 action_bar_open_url**

`action_bar_open_url` 已被 `execute_action_bar` 的 url 分支替代。检查前端是否还有直接调 `action_bar_open_url` 的地方——如果都走 `executeItem`，删除后端函数 + main.rs 注册。

- [ ] **Step 3: 清理前端残留**

删除不再使用的 import（`Sparkles, Globe, Search, Link as LinkIcon, FileText, Lightbulb, Pencil`）。`Loader2` 仍用于 loading 状态。

- [ ] **Step 4: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test && cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 全过

- [ ] **Step 5: 更新文档**

更新以下文档反映菜单数据库化：
- `docs/architecture.md`：action bar 描述更新（菜单 DB 驱动）
- `docs/superpowers/specs/2026-07-08-action-bar-design.md`：§5 命令表更新
- `docs/superpowers/plans/2026-07-08-action-bar.md`：追加偏差
- `docs/superpowers/specs/2026-07-09-action-bar-menu-db-design.md`：标记已实现

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: 清理旧 action bar 硬编码命令 + 文档同步"
```
