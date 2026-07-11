# Action Bar 命令局部快捷键 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 action bar 菜单项新增 `Alt/⌥ + 字符` 组合快捷键，按下直接执行对应命令，跨主菜单和子菜单层级。

**Architecture:** DB 层加 `shortcut` 列存储快捷键字符；后端 CRUD 函数加参数 + 校验（格式 + 全局唯一）；前端浮窗 keydown handler 在位置定位之前检查 `e.altKey` 分支；设置页编辑表单加快捷键输入行。

**Tech Stack:** Rust（rusqlite + Tauri commands）、React + TypeScript + Tailwind（前端）

## Global Constraints

- 修饰键固定 `Alt/⌥`，用户只指定单个字符
- 字符范围 `0-9 a-z`（小写），存储和匹配均统一小写
- 全局唯一：一个字符只能分配给一个命令，跨所有菜单层级
- 只有非 `submenu` 类型可设快捷键
- 现有 `1-9 a-z` 位置定位行为不变（单键，仅移动高亮）
- 现有方向键 / Enter / Esc 行为不变

---

## 文件结构

| 文件 | 职责 | 操作 |
|------|------|------|
| `crates/infra/src/db.sql` | DB schema 真相源 | 修改：CREATE TABLE 加 `shortcut` 列 |
| `crates/infra/src/db.rs` | DB 操作层 | 修改：struct + row mapper + CRUD + 校验 + 迁移 + 测试 |
| `crates/desktop/src/action_bar_commands.rs` | Tauri 命令层 | 修改：create/update 命令加 `shortcut` 参数 |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | 浮窗 | 修改：keydown handler + 渲染快捷键标记 |
| `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` | 设置页 | 修改：编辑表单 + 树行显示 |

---

### Task 1: DB schema + struct + 迁移

**Files:**
- Modify: `crates/infra/src/db.sql:272-287`（CREATE TABLE）
- Modify: `crates/infra/src/db.rs:947-963`（struct + ACTION_BAR_SELECT_COLS）
- Modify: `crates/infra/src/db.rs:965-979`（row_to_action_bar_item）
- Modify: `crates/infra/src/db.rs:159-260`（init_schema 迁移）

**Interfaces:**
- Produces: `ActionBarItem.shortcut: String` 字段，供后续任务使用

- [ ] **Step 1: db.sql CREATE TABLE 加 shortcut 列**

在 `crates/infra/src/db.sql` 的 `action_bar_items` CREATE TABLE 中，在 `write_output_to_clipboard` 行之后、`created_at` 之前加入：

```sql
    shortcut    TEXT NOT NULL DEFAULT '',
```

完整修改后的 CREATE TABLE：

```sql
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
    is_async   INTEGER NOT NULL DEFAULT 1,
    write_output_to_clipboard INTEGER NOT NULL DEFAULT 0,
    shortcut    TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
```

- [ ] **Step 2: Rust struct 加 shortcut 字段**

在 `crates/infra/src/db.rs` 的 `ActionBarItem` struct（L947-961）末尾加字段：

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
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
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
    pub shortcut: String,
}
```

- [ ] **Step 3: ACTION_BAR_SELECT_COLS 加 shortcut**

在 `crates/infra/src/db.rs:963` 的常量末尾加 `shortcut`：

```rust
const ACTION_BAR_SELECT_COLS: &str = "id, parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut";
```

- [ ] **Step 4: row_to_action_bar_item 加 shortcut 读取**

在 `crates/infra/src/db.rs:965-979` 的 `row_to_action_bar_item` 末尾加：

```rust
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
        is_async: row.get::<_, i32>(9)? != 0,
        write_output_to_clipboard: row.get::<_, i32>(10)? != 0,
        shortcut: row.get(11)?,
    })
}
```

- [ ] **Step 5: init_schema 加迁移分支**

在 `crates/infra/src/db.rs` 的 `init_schema` 函数中：

1. 将 `if v >= 23`（L178）改为 `if v >= 24`
2. 在 v22→v23 迁移块之后（L249 `PRAGMA user_version = 23` 之后）、`return Ok(())` 之前，加入 v23→v24 迁移：

```rust
        // v23→v24：action_bar_items 加 shortcut 列
        {
            let cols: Vec<String> = conn.prepare("PRAGMA table_info(action_bar_items)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            if !cols.contains(&"shortcut".to_string()) {
                conn.execute("ALTER TABLE action_bar_items ADD COLUMN shortcut TEXT NOT NULL DEFAULT ''", [])?;
            }
            conn.execute("PRAGMA user_version = 24", [])?;
            log::info!("schema upgraded to v24 (action_bar_items.shortcut)");
        }
```

3. 更新注释：在 `v23` 注释行下方加 `/// v24：action_bar_items 加 shortcut 列。`

4. 新建 DB 路径（L255-258）的 `PRAGMA user_version = 23` 改为 `= 24`

- [ ] **Step 6: 编译验证**

Run: `cargo build -p octopus-infra`
Expected: 编译通过，无错误

- [ ] **Step 7: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): add shortcut column to action_bar_items + schema v24 migration"
```

---

### Task 2: 后端校验 + CRUD 变更

**Files:**
- Modify: `crates/infra/src/db.rs:1032-1100`（insert/update 函数）

**Interfaces:**
- Consumes: Task 1 的 `shortcut` 列
- Produces: `insert_action_bar_item(... shortcut: &str)` 和 `update_action_bar_item(... shortcut: &str)` 新签名

- [ ] **Step 1: 写校验+CRUD 的失败测试**

在 `crates/infra/src/db.rs` 的 `mod tests`（L1815+）中，在 `open_init` helper 之后加入测试：

```rust
    #[test]
    fn action_bar_shortcut_validate_and_conflict() {
        let conn = open_init();

        // 给 id=2（翻译）设快捷键 't'
        conn.execute(
            "UPDATE action_bar_items SET shortcut='t' WHERE id=2",
            [],
        )
        .unwrap();

        // validate_shortcut: 合法
        assert!(validate_shortcut("").is_ok());
        assert!(validate_shortcut("t").is_ok());
        assert!(validate_shortcut("5").is_ok());
        // validate_shortcut: 非法
        assert!(validate_shortcut("T").is_err());  // 大写
        assert!(validate_shortcut("ab").is_err()); // 多字符
        assert!(validate_shortcut("-").is_err());  // 非法字符
        assert!(validate_shortcut(" ").is_err());  // 空格

        // check_shortcut_conflict: 't' 已被 id=2 占用
        let conflict = check_shortcut_conflict_at(&conn, "t", Some(5)).unwrap();
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().id, 2);

        // 排除自身——id=2 查 't' 不应冲突
        let self_ok = check_shortcut_conflict_at(&conn, "t", Some(2)).unwrap();
        assert!(self_ok.is_none());

        // 无冲突字符
        let free = check_shortcut_conflict_at(&conn, "z", None).unwrap();
        assert!(free.is_none());
    }

    #[test]
    fn action_bar_insert_with_shortcut() {
        let conn = open_init();
        let id = insert_action_bar_item_at(
            &conn, None, "测试", "", "copy", "", true, false, "q",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, id).unwrap().unwrap();
        assert_eq!(item.shortcut, "q");
    }

    #[test]
    fn action_bar_update_shortcut() {
        let conn = open_init();
        // id=5（润色）原本无快捷键
        update_action_bar_item_at(
            &conn, 5, "润色", "pencil", "ai", "prompt", true, true, false, "p",
        ).unwrap();
        let item = load_action_bar_item_at(&conn, 5).unwrap().unwrap();
        assert_eq!(item.shortcut, "p");
    }
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p octopus-infra action_bar_shortcut`
Expected: FAIL — `validate_shortcut` / `check_shortcut_conflict_at` 未定义

- [ ] **Step 3: 实现校验函数**

在 `crates/infra/src/db.rs` 中，在 `row_to_action_bar_item` 之后（L979 之后）加入校验函数：

```rust
/// 校验快捷键格式：空字符串或单个 0-9/a-z 字符。
pub fn validate_shortcut(shortcut: &str) -> Result<()> {
    if shortcut.is_empty() {
        return Ok(());
    }
    if shortcut.len() == 1 && shortcut.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return Ok(());
    }
    anyhow::bail!("快捷键必须为空或单个 0-9/a-z 字符");
}

/// 检查快捷键是否已被其他项占用（排除指定 id）。返回冲突项（如有）。
fn check_shortcut_conflict_at(conn: &Connection, shortcut: &str, exclude_id: Option<i64>) -> Result<Option<ActionBarItem>> {
    if shortcut.is_empty() {
        return Ok(None);
    }
    let sql = match exclude_id {
        Some(_) => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1 AND id!=?2", ACTION_BAR_SELECT_COLS),
        None => format!("SELECT {} FROM action_bar_items WHERE shortcut=?1", ACTION_BAR_SELECT_COLS),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = match exclude_id {
        Some(eid) => stmt.query_map(params![shortcut, eid], row_to_action_bar_item)?,
        None => stmt.query_map(params![shortcut], row_to_action_bar_item)?,
    };
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 4: insert/update 函数加 shortcut 参数 + 校验**

修改 `crates/infra/src/db.rs` 的 insert/update 公开函数和内部 `_at` 函数签名。

`insert_action_bar_item`（L1032-1042）改为：

```rust
pub fn insert_action_bar_item(
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
) -> Result<i64> {
    with_db(|conn| insert_action_bar_item_at(conn, parent_id, title, icon, action_type, action_data, is_async, write_output_to_clipboard, shortcut))
}
```

`insert_action_bar_item_at`（L1044-1065）改为（加校验 + INSERT 加 shortcut 列）：

```rust
fn insert_action_bar_item_at(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
) -> Result<i64> {
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, None)? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    let max_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) FROM action_bar_items WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, is_enabled, is_async, write_output_to_clipboard, shortcut)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1, ?7, ?8, ?9)",
        params![parent_id, title, icon, action_type, action_data, max_order + 1, is_async as i32, write_output_to_clipboard as i32, shortcut],
    )?;
    Ok(conn.last_insert_rowid())
}
```

`update_action_bar_item`（L1067-1078）改为：

```rust
pub fn update_action_bar_item(
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
) -> Result<()> {
    with_db(|conn| update_action_bar_item_at(conn, id, title, icon, action_type, action_data, is_enabled, is_async, write_output_to_clipboard, shortcut))
}
```

`update_action_bar_item_at`（L1080-1100）改为（加校验 + UPDATE 加 shortcut）：

```rust
fn update_action_bar_item_at(
    conn: &Connection,
    id: i64,
    title: &str,
    icon: &str,
    action_type: &str,
    action_data: &str,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: &str,
) -> Result<()> {
    let row = load_action_bar_item_at(conn, id)?.context("菜单项不存在")?;
    if row.is_system && row.action_type != action_type {
        anyhow::bail!("系统内置菜单项不可更改动作类型");
    }
    let shortcut = shortcut.to_lowercase();
    validate_shortcut(&shortcut)?;
    if let Some(conflict) = check_shortcut_conflict_at(conn, &shortcut, Some(id))? {
        anyhow::bail!("快捷键 Alt+{} 已被「{}」占用", shortcut, conflict.title);
    }
    conn.execute(
        "UPDATE action_bar_items SET title=?1, icon=?2, action_type=?3, action_data=?4, is_enabled=?5, is_async=?6, write_output_to_clipboard=?7, shortcut=?8, updated_at=datetime('now') WHERE id=?9",
        params![title, icon, action_type, action_data, is_enabled as i32, is_async as i32, write_output_to_clipboard as i32, shortcut, id],
    )?;
    Ok(())
}
```

- [ ] **Step 5: 运行测试验证通过**

Run: `cargo test -p octopus-infra action_bar_shortcut && cargo test -p octopus-infra action_bar_insert_with_shortcut && cargo test -p octopus-infra action_bar_update_shortcut`
Expected: 全部 PASS

- [ ] **Step 6: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): shortcut validation + conflict check + CRUD with shortcut param"
```

---

### Task 3: Tauri 命令层加 shortcut 参数

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs:257-290`

**Interfaces:**
- Consumes: Task 2 的 `insert/update_action_bar_item(... shortcut)` 新签名
- Produces: Tauri 命令 `create_action_bar_item` / `update_action_bar_item` 接受 `shortcut` 参数

- [ ] **Step 1: 修改 create_action_bar_item 命令**

在 `crates/desktop/src/action_bar_commands.rs:257-275`，加入 `shortcut` 参数并透传：

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
    shortcut: String,
) -> Result<i64, String> {
    // 同级菜单项最多 35 个（9 数字 + 26 字母快捷键上限）
    let all = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    let sibling_count = all.iter().filter(|i| i.parent_id == parent_id).count();
    if sibling_count >= 35 {
        return Err("同级菜单项已达上限 35 个（快捷键 1-9 + a-z）".into());
    }
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data, is_async, write_output_to_clipboard, &shortcut)
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 2: 修改 update_action_bar_item 命令**

在 `crates/desktop/src/action_bar_commands.rs:277-290`，加入 `shortcut` 参数并透传：

```rust
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
    shortcut: String,
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled, is_async, write_output_to_clipboard, &shortcut)
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 3: 检查 install_extension 命令**

搜索 `install_extension` 命令，如果它内部调用 `insert_action_bar_item`，也需要加 `shortcut` 参数。用 `rg "insert_action_bar_item" crates/desktop/src/` 检查所有调用点。

对于 `install_extension`：传入 `""` 空快捷键（扩展包不支持快捷键设置，用户安装后在编辑界面设置）。

- [ ] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat(desktop): pass shortcut param through create/update action bar commands"
```

---

### Task 4: 前端类型 + 浮窗快捷键处理

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`

**Interfaces:**
- Consumes: Task 3 的 Tauri 命令返回含 `shortcut` 字段的 `ActionBarItem`
- Produces: 浮窗响应 `Alt+字符` 直接执行命令

- [ ] **Step 1: ActionBarItem interface 加 shortcut 字段**

在 `crates/desktop/frontend/src/pages/ActionBar/index.tsx:15-25` 的 `ActionBarItem` interface 加字段：

```typescript
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
  shortcut?: string;
}
```

- [ ] **Step 2: keydown handler 加 Alt 组合键分支**

在 `crates/desktop/frontend/src/pages/ActionBar/index.tsx` 的 keydown handler（L298-412）中，在 `viewRef.current === "loading"` 检查之后、位置定位 `labelToIndex` 分支之前，加入 Alt 分支：

```typescript
      if (viewRef.current === "loading") return;

      // 组合快捷键：Alt/⌥ + 字符 → 直接执行（最高优先级，跨层级）
      if (e.altKey) {
        const ch = e.key.toLowerCase();
        if (/^[0-9a-z]$/.test(ch)) {
          const item = menuItemsRef.current.find((i: ActionBarItem) => i.shortcut === ch);
          if (item) {
            e.preventDefault();
            executeItem(item);
          }
        }
        return; // Alt 组合键不再走后续位置定位分支
      }

      // 快捷定位：1-9 数字键 + a-z 字母键（支持最多 35 项）
      const idx = labelToIndex(e.key.toLowerCase());
```

- [ ] **Step 3: IconBtn 渲染快捷键标记**

修改 `IconBtn` 组件（L43-71），加 `shortcut` 可选 prop。在标题右侧显示 `⌥x`：

```typescript
const IconBtn = ({ index, label, active, onClick, btnRef, shortcut }: {
  index: number; label: string; active: boolean; onClick: () => void;
  btnRef?: (el: HTMLButtonElement | null) => void;
  shortcut?: string;
}) => (
  <button
    ref={btnRef}
    className={cn(
      "flex items-center gap-1.5 px-2 py-1.5 rounded-lg transition-all duration-150 shrink-0",
      active
        ? "bg-voice/12 text-voice"
        : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
    )}
    onMouseDown={(e) => e.stopPropagation()}
    onClick={onClick}
    title={label}
  >
    <span
      className={cn(
        "inline-flex h-[18px] w-[18px] items-center justify-center rounded-md font-mono text-[11px] font-semibold tabular-nums leading-none",
        active
          ? "bg-voice text-white"
          : "bg-muted text-muted-foreground",
      )}
    >
      {indexLabel(index)}
    </span>
    <span className="text-[10px] font-medium leading-none whitespace-nowrap">{label}</span>
    {shortcut && (
      <span className="text-[9px] text-muted-foreground/50 font-mono leading-none">⌥{shortcut}</span>
    )}
  </button>
);
```

- [ ] **Step 4: 主菜单和子菜单渲染传 shortcut**

在主菜单渲染处（L448-459），`IconBtn` 加 `shortcut={item.shortcut}`：

```tsx
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            index={i + 1}
            label={item.title}
            active={selectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { mainBtnRefs.current[i] = el; }}
            shortcut={item.shortcut}
          />
        ))}
```

在子菜单渲染处（L467-476），同样加 `shortcut={item.shortcut}`：

```tsx
        {subItems.map((item, i) => (
          <IconBtn
            key={item.id}
            index={i + 1}
            label={item.title}
            active={focusLayer === "sub" && subSelectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { subBtnRefs.current[i] = el; }}
            shortcut={item.shortcut}
          />
        ))}
```

- [ ] **Step 5: 前端编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "feat(frontend): action bar Alt+shortcut direct execution + shortcut badge rendering"
```

---

### Task 5: 设置页编辑表单 + 树行显示

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`

**Interfaces:**
- Consumes: Task 3 的 `create/update_action_bar_item` 命令（含 `shortcut` 参数）

- [ ] **Step 1: ActionBarItem interface 加 shortcut 字段**

在 `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx:20-32` 的 `ActionBarItem` interface 加字段：

```typescript
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
  isAsync?: boolean;
  writeOutputToClipboard?: boolean;
  shortcut?: string;
}
```

- [ ] **Step 2: EditForm 加快捷键输入行**

在 `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` 的 `EditForm` 组件中，在 `showContent` 定义（L295）之后加一个变量控制快捷键输入行显示：

```typescript
  const showShortcut = type !== "submenu";
```

在"执行选项"Field（L370-403）之后、"启用"Field（L405）之前，加入快捷键 Field：

```tsx
        {showShortcut && (
          <Field label="快捷键">
            <div className="flex items-center gap-2">
              <div className="flex items-center gap-1">
                <span className="text-xs text-muted-foreground/60 font-mono">⌥ +</span>
                <input
                  className="w-10 text-center bg-background border border-border rounded-md px-2 py-1.5 text-sm font-mono outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all"
                  placeholder="—"
                  maxLength={1}
                  value={form.shortcut || ""}
                  onChange={(e) => {
                    const raw = e.target.value.toLowerCase();
                    const filtered = raw.replace(/[^0-9a-z]/g, "").slice(-1);
                    onChange({ ...form, shortcut: filtered });
                  }}
                />
              </div>
              {form.shortcut && (
                <button
                  onClick={() => onChange({ ...form, shortcut: "" })}
                  className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-red-500/10 hover:text-red-500"
                  aria-label="清除快捷键"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              )}
              <span className="text-[11px] text-muted-foreground/60">
                action bar 打开时按 Alt+此键直接执行
              </span>
            </div>
          </Field>
        )}
```

- [ ] **Step 3: saveEdit 传递 shortcut 参数**

在 `saveEdit` 函数中（L851-930），所有 `invoke("create_action_bar_item", ...)` 和 `invoke("update_action_bar_item", ...)` 调用加 `shortcut` 参数。

新建草稿分支（L899-909）：

```typescript
        await invoke("create_action_bar_item", {
          parentId: draftParentId,
          title: editingForm.title || "新菜单项",
          icon: "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
        });
```

编辑已有项分支（L911-922）：

```typescript
        await invoke("update_action_bar_item", {
          id: editingId,
          title: editingForm.title || "",
          icon: editingForm.icon || "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isEnabled: editingForm.isEnabled ?? true,
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
        });
```

同时，编辑已有扩展项分支（L886-895）的 `update_action_bar_item` 也需要加 `shortcut: editingForm.shortcut || ""`。

- [ ] **Step 4: 树行显示快捷键徽章**

在 `TreeNodeBase` 组件中（L547-548 的 `TypeTag` 之后），加快捷键徽章：

```tsx
        {/* 类型标签 */}
        <TypeTag type={item.actionType} />

        {/* 快捷键徽章 */}
        {item.shortcut && (
          <span className="shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[10px] text-muted-foreground">
            ⌥{item.shortcut}
          </span>
        )}
```

- [ ] **Step 5: 前端编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx
git commit -m "feat(frontend): shortcut input in edit form + shortcut badge in tree view"
```

---

### Task 6: 文档同步 + 整体验证

**Files:**
- Modify: `docs/architecture.md`（action_bar_items 表结构描述）

- [ ] **Step 1: 全量编译**

Run: `cargo build --release -p octopus-server -p octopus-cli`
Expected: 编译通过

- [ ] **Step 2: 全量测试**

Run: `cargo test -p octopus-infra`
Expected: 全部 PASS（含新增的 shortcut 测试）

- [ ] **Step 3: 前端全量编译**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 无错误

- [ ] **Step 4: 桌面应用编译**

Run: `cargo build --release -p octopus-desktop --features embedded`
Expected: 编译通过

- [ ] **Step 5: 更新 architecture.md**

在 `docs/architecture.md` 中找到 action_bar_items 表描述，加入 `shortcut` 列说明。搜索 `action_bar_items` 定位相关段落。

- [ ] **Step 6: 更新 spec 状态**

在 `docs/superpowers/specs/2026-07-12-action-bar-command-shortcut-design.md` 的状态行改为"已实现"。

- [ ] **Step 7: Commit**

```bash
git add docs/
git commit -m "docs: sync architecture + spec status for shortcut feature"
```
