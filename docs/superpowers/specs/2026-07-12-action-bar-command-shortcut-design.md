# Action Bar 命令局部快捷键设计

> **状态**：设计完成，待实现
> **日期**：2026-07-12
> **scope**：为 action bar 菜单项新增 `Alt/⌥ + 字符` 组合快捷键，按下直接执行对应命令，跨主菜单和子菜单层级
> **前置文档**：[`2026-07-09-action-bar-menu-db-design.md`](./2026-07-09-action-bar-menu-db-design.md)（action bar DB 化设计，本特性基于其 DB 表和前端架构）

---

## 1. 背景与动机

当前 action bar 有两种执行命令的方式：
- **位置定位**：`1-9 a-z` 在当前焦点层移动高亮，再按 `Enter` 执行
- **方向键导航**：`←→↑↓` 浏览，`Enter` 执行

两者都需要两步操作（定位 → 执行）。用户希望为常用命令指定一个**局部组合快捷键**，在 action bar 打开时按下即可**直接执行**，无需先导航到该项。

### 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 与位置定位的关系 | **共存，不同按键空间**：位置定位 `1-9 a-z` 不变（单键）；新增组合键 `Alt/⌥ + 字符` |
| 修饰键 | **固定 `Alt/⌥`**，用户只需指定字符部分 |
| 作用范围 | **全局（跨主菜单和子菜单）**——一个组合键直接定位到特定命令 |
| submenu 容器 | **不可设快捷键**——只有可执行项（ai/url/script/copy，含子菜单叶项）可设 |
| 字符范围 | **`0-9 a-z`**（36 个可选） |
| 唯一性 | **全局唯一**——一个字符只能分配给一个命令，跨所有菜单层级 |
| 设置入口 | 命令管理页面（ActionBarPanel 编辑表单） |

---

## 2. DB 表结构变更

`action_bar_items` 表新增一列：

```sql
ALTER TABLE action_bar_items ADD COLUMN shortcut TEXT NOT NULL DEFAULT '';
```

- 空字符串 = 无快捷键
- 非空 = 单个小写字符（`0-9 a-z`）
- 运行时匹配不区分大小写（统一 `e.key.toLowerCase()`）

`db.sql` 的 `CREATE TABLE` 同步加入该列：

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
    is_async    INTEGER NOT NULL DEFAULT 1,
    write_output_to_clipboard INTEGER NOT NULL DEFAULT 0,
    shortcut    TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (parent_id) REFERENCES action_bar_items(id) ON DELETE CASCADE
);
```

迁移：`user_version` bump（当前版本 +1），已有 DB 执行 `ALTER TABLE`。`INSERT OR IGNORE` 种子数据不需要改动（shortcut 默认空）。

---

## 3. 后端变更

### 3.1 Rust struct（`crates/infra/src/db.rs`）

`ActionBarItem` struct 新增字段：

```rust
pub struct ActionBarItem {
    // ... 现有字段 ...
    pub shortcut: String,
}
```

`row_to_action_bar_item` 映射加入 `shortcut` 读取。

### 3.2 校验逻辑

**快捷键校验函数**（`crates/infra/src/db.rs`）：

```rust
/// 校验快捷键格式：空字符串或单个 0-9/a-z 字符
fn validate_shortcut(shortcut: &str) -> Result<()>
```

**全局唯一校验**（`crates/infra/src/db.rs`）：

```rust
/// 检查快捷键是否已被其他项占用（排除指定 id）
fn check_shortcut_conflict(shortcut: &str, exclude_id: Option<i64>) -> Result<Option<ActionBarItem>>
```

- `insert_action_bar_item` / `update_action_bar_item` 调用时校验
- 格式不合法 → 返回错误
- 冲突 → 返回错误，包含冲突项的 title

### 3.3 CRUD 函数变更

`insert_action_bar_item` 和 `update_action_bar_item` 签名加入 `shortcut: &str` 参数，在写入 DB 前调用 `validate_shortcut` + `check_shortcut_conflict`。

### 3.4 Tauri 命令变更（`crates/desktop/src/action_bar_commands.rs`）

- `create_action_bar_item`：新增 `shortcut` 参数，透传给 DB 层
- `update_action_bar_item`：新增 `shortcut` 参数，透传给 DB 层
- 校验错误（格式 / 冲突）通过 `Result<_, String>` 返回前端

---

## 4. 前端变更

### 4.1 类型定义

两个 `ActionBarItem` interface（`ActionBar/index.tsx` 和 `Settings/ActionBarPanel.tsx`）都加入 `shortcut: string` 字段。

### 4.2 浮窗快捷键处理（`ActionBar/index.tsx`）

在现有 keydown handler 中，**在位置定位分支之前**新增组合键分支：

```typescript
const handler = (e: KeyboardEvent) => {
  // Escape ...（不变）

  // 组合快捷键：Alt/⌥ + 字符 → 直接执行（最高优先级）
  if (e.altKey) {
    const ch = e.key.toLowerCase();
    if (/^[0-9a-z]$/.test(ch)) {
      const item = menuItemsRef.current.find((i) => i.shortcut === ch);
      if (item) {
        e.preventDefault();
        executeItem(item);
      }
    }
    return;  // Alt 组合键不再走后续分支
  }

  // 快捷定位：1-9 数字键 + a-z 字母键（现有逻辑，不变）
  const idx = labelToIndex(e.key.toLowerCase());
  // ...
};
```

**关键点**：
- `e.altKey` 分支在位置定位之前，确保 `Alt+t` 不会被当成位置定位的 `t`
- 查找范围是 `menuItemsRef.current`（全部菜单项），不限于当前焦点层
- 找到则 `executeItem`（走现有执行流程，包括 ai 类型的 loading 视图）
- 找不到则忽略（不 preventDefault）
- `submenu` 类型不会有 shortcut（数据约束 + 设置页禁止），组合键找到的项一定可执行

### 4.3 浮窗渲染

有快捷键的项，在 `IconBtn` 中附加显示 `⌥x` 标记：

- 数字徽章（位置序号）保持不变
- 快捷键标记紧贴标题右侧：`<span className="text-[9px] text-muted-foreground/60 font-mono">⌥{shortcut}</span>`
- 避免视觉嘈杂：标记使用低调样式（小号 + 低透明度）

### 4.4 设置页编辑表单（`ActionBarPanel.tsx`）

编辑表单新增**快捷键输入行**：

- 仅对非 `submenu` 类型显示（submenu 编辑时隐藏）
- 标签：`快捷键`
- 输入框：单字符，placeholder `Alt + 字母/数字`
- 输入处理：`onChange` 取输入文本，正则过滤 `[^0-9a-z]`，转小写，截取最后一个字符
- 禁用 `onKeyDown` 中的 Tab/Enter 拦截——纯文本输入方式（避免与表单导航冲突）
- 清除按钮：输入框右侧 `X` 图标，清空 shortcut
- 冲突提示：保存失败时，在表单内显示后端返回的错误信息（如 `Alt+t 已被「翻译」占用`）

### 4.5 设置页树形列表行

已设置快捷键的项，在类型标签旁显示 `⌥x` 徽章：

- 样式：mono 字体、`text-[10px]`、`bg-muted` 背景、`text-muted-foreground`
- 与 TypeTag 平级，在行内容区域

---

## 5. 交互流程总结

### action bar 打开时按键优先级

```
1. Esc          → 关闭浮窗（最高优先级）
2. Alt + 字符   → 直接执行对应命令（跨层级）
3. 1-9 / a-z    → 当前焦点层位置定位（仅移动高亮）
4. ←→↑↓         → 方向键导航
5. Enter/Space  → 执行当前高亮项
```

### 典型用例

| 场景 | 操作 | 效果 |
|------|------|------|
| 选中文本，快速翻译 | 唤出 action bar → `Alt+t` | 翻译立即执行 |
| 选中文本，润色 | 唤出 action bar → `Alt+p` | 润色立即执行（进入 AI loading） |
| 唤出后浏览 | `←→` 移动 | 高亮移动，submenu 自动展开 |
| 浏览后执行 | `Enter` | 执行当前高亮项 |

---

## 6. 错误处理与边界

| 情况 | 处理 |
|------|------|
| 浮窗未加载完时按组合键 | `menuItems` 为空 → `find` 返回 undefined → 忽略 |
| `Alt+字母` 无对应命令 | 静默忽略，不 preventDefault |
| 快捷键项被禁用（`is_enabled=0`） | 浮窗不显示该项（现有过滤），`find` 不到 → 行为一致 |
| 快捷键项是子菜单叶项 | 无需进入子菜单，直接执行——"不区分层级"的核心价值 |
| 保存时字符格式不合法 | 后端拒绝，前端显示错误 |
| 保存时全局冲突 | 后端拒绝，返回冲突项 title，前端显示 `Alt+x 已被「xx」占用` |

---

## 7. 不在本次范围

- 修饰键自定义（固定 `Alt/⌥`，不放开 Shift/Ctrl/Cmd 选择）
- 多字符快捷键（仅单字符 `0-9 a-z`）
- 快捷键冲突时自动迁移/提示替代键
- 快捷键导入/导出（随 JSON 配置导入导出二期一起做）
