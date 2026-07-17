# ActionBarPanel 双 TAB + 左右分栏重构

> 2026-07-17 · 命令管理页 UI 重构
>
> **状态**：已实现

## 1. 背景

原 ActionBarPanel 命令管理用递归 `TreeNode` 树形控件——主菜单带 chevron 展开/收起，子菜单嵌套缩进显示，顶部有「全部展开/收缩」按钮。问题：

- 主菜单多时树形纵向过长，子菜单展开后更甚
- 用户需点 chevron 才能看子菜单，操作步骤多
- 顶部「全部展开/收缩」+「执行记录」按钮散乱

## 2. 设计

### 2.1 顶层 TAB

用项目共享组件 `UnderlineTabs`（`components/ui/tabs.tsx`）切换两个 tab：
- **命令管理**（`menu`）：左右分栏的菜单 CRUD
- **执行记录**（`runs`）：脚本执行历史（复用现有 `ScriptRunsList`）

替代原 `view: "menu" | "runs" | "edit"` 状态机。EditForm 走独立的全屏覆盖判定（`editingId !== null || draftParentId !== undefined`），不占 tab 位。

### 2.2 命令管理 TAB：左右分栏

```
┌─────────────────────────────────────────────────────────────┐
│ UnderlineTabs: [命令管理] [执行记录]                          │
├──────────────┬──────────────────────────────────────────────┤
│ 左栏 w-64     │ 右栏 flex-1                                   │
│              │                                              │
│ [Segmented]   │ ┌─ 主菜单 inline 编辑表单 ────────────────┐ │
│ [+ 新增主菜单] │ │ 标题 / 类型 / 快捷键 / 全局快捷键 / 启用 │ │
│              │ └──────────────────────────────────────────┘ │
│ ▸ 主菜单 1    │                                              │
│ ▸ 主菜单 2 ◀ │ ┌─ 子菜单列表（仅 submenu）────────────────┐ │
│   主菜单 3    │ │ 子菜单 1   上移 下移 编辑 删除            │ │
│              │ │ ...                                      │ │
└──────────────┴──────────────────────────────────────────────┘
```

### 2.3 左栏：主菜单列表

- 顶部 `Segmented` 场景过滤（全部/选中文本/选中文件）
- 「+ 新增主菜单」voice 主操作按钮
- `MenuRow` 行渲染：序号 + 标题 + 类型徽章 + 子项计数（submenu）+ 4 操作按钮（hover 显示）
- **单选高亮**：`bg-voice/12` + 左侧色条加粗（`h-7` vs 默认 `h-5`）
- state：`selectedMainMenuId: number | null`，首次进 menu tab 默认选第一个，删除/过滤后自动 fallback 相邻项

### 2.4 右栏：主菜单详情 + 子菜单列表

**顶部 inline 编辑表单**（主菜单字段实时保存，无需点保存按钮）：
- 标题（input，CJK 权重 2 / ASCII 权重 1，上限 12）
- 类型（select，submenu 不可改；系统菜单 disabled）
- 快捷键（⌥ + 单字符，inline 录制 `inlineCapturingShortcut`）
- 全局快捷键（`ShortcutButton`，inline 录制 `inlineCapturingGlobal`）
- 启用（Toggle）

`updateMainInline(patch)` 复用 `update_action_bar_item` + `set_global_shortcut`，参数从原 item 派生。

**下方子菜单列表**（仅 submenu 类型）：
- 标题 + 「+ 新增子项」按钮
- `MenuRow` 复用，编辑走全屏 `EditForm`（`startEdit` → `editingId` set）
- 空列表提示「该菜单暂无子项」
- 叶子命令（非 submenu）：显示「叶子命令无子项」提示，隐藏新增子项按钮

### 2.5 MenuRow 组件（主菜单/子菜单共用）

替代原 `TreeNode`：

| 字段 | 说明 |
|---|---|
| `item` | ActionBarItem |
| `index` | 1-based 序号 |
| `selected` | 主菜单选中态（子菜单恒 false） |
| `isFirst/isLast` | 上移/下移 disabled 判定 |
| `deleteConfirmId` | 删除二次确认（同 ID 高亮） |
| `subCount?` | submenu 主菜单的子项数徽章 |
| `onSelect?` | 主菜单点击选中（子菜单不传） |
| `onMove/onEdit/onDelete` | 操作回调 |

行结构：色条 + 序号 + 标题 + 子项计数 + TypeTag + 内置标记 + hover 操作栏。

### 2.6 inline 录制范式

主菜单的快捷键/全局快捷键 inline 录制复用 EditForm 的 keydown 监听范式：
- `inlineCapturingShortcut`：单字符 0-9a-z，Backspace/Delete 清空，Esc 退出
- `inlineCapturingGlobal`：组合键 CmdOrCtrl/Alt/Shift + key，调 `check_shortcut` 校验，Backspace/Delete 清空
- 监听器生命周期绑定到 capturing state，useEffect cleanup 自动 removeEventListener

## 3. 删除项

- `TreeNode` / `NodeProps` / `TreeNodeBase` / `memo(TreeNodeBase)` ——树形渲染不再需要
- `expanded: Set<number>` / `toggle` / `expandAll` / `collapseAll` / `allExpanded` ——展开/收缩语义被左右分栏替代
- `view` 状态机 ——TAB 替代
- `nodeCommon` ——不再传递树形 props
- 顶部工具栏的 records 按钮 / 展开收缩按钮 / 返回按钮 ——TAB 替代

## 4. 不变量

1. **数据模型不变**——`items: ActionBarItem[]`、`parentId` 树形关系、所有 CRUD 命令（create/update/delete/move/set_global_shortcut）原样复用
2. **EditForm 不变**——子菜单编辑仍走全屏表单（只是触发入口从树形改成右栏按钮）
3. **ScriptRunsList 不变**——执行记录 TAB 直接复用现有组件
4. **选中态跨 EditForm 保留**——`selectedMainMenuId` 在父组件 state，EditForm 退出后仍指向原主菜单
5. **scope 过滤兼容**——左栏 mainItems 仍按 scope 过滤，过滤变化时若选中项不在结果集则 fallback 第一个

## 5. i18n

新增 9 个 key（zh-CN + en 同步）：
- `menuManage` / `scriptRecords`（TAB 标签）
- `subItemsTitle` / `noSubItemsHint` / `leafNoSubItemsHint`（子菜单列表区）
- `selectMenuHint`（右栏未选中提示）
- `moveUp` / `moveDown` / `edit`（操作按钮 aria-label）

## 6. 验证

- `npx tsc --noEmit`：0 error
- `npm run build`：0 error
- `cargo build --release -p octopus-desktop`：0 error 0 warning
- 待手测：① TAB 切换 ② 左栏选中→右栏同步 ③ 主菜单 inline 编辑实时生效 ④ 子菜单增删改 + 上移下移 ⑤ 主菜单上移下移/删除 ⑥ 删除选中主菜单后自动选相邻

## 7. 不在本次范围

- 不改后端命令/数据模型
- 不改 EditForm 内部（子菜单编辑的表单字段/逻辑不变）
- 不改 ScriptRunsList（执行记录组件不变）
- 不改 ActionBar 浮窗（前端搜索/键盘导航不变）
