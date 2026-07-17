# ActionBarPanel 双 TAB + 左右分栏重构

> 2026-07-17 · 命令管理页 UI 重构
>
> **状态**：已实现（含多轮视觉迭代 + code review 修复）

## 1. 背景

原 ActionBarPanel 命令管理用递归 `TreeNode` 树形控件——主菜单带 chevron 展开/收起，子菜单嵌套缩进显示，顶部有「全部展开/收缩」按钮。问题：

- 主菜单多时树形纵向过长，子菜单展开后更甚
- 用户需点 chevron 才能看子菜单，操作步骤多
- 顶部「全部展开/收缩」+「执行记录」按钮散乱

## 2. 设计

### 2.1 顶层 TAB

用项目共享组件 `UnderlineTabs`（`components/ui/tabs.tsx`）切换两个 tab：
- **命令管理**（`menu`）：左右分栏的菜单 CRUD
- **执行记录**（`runs`）：脚本执行历史（`ScriptRunsList`，含复选框批量删除）

替代原 `view: "menu" | "runs" | "edit"` 状态机。EditForm 走独立的全屏覆盖判定（`editingId !== null || draftParentId !== undefined`），不占 tab 位。

### 2.2 命令管理 TAB：左右分栏

```
┌─────────────────────────────────────────────────────────────┐
│ UnderlineTabs: [命令管理] [执行记录]                          │
├──────────┬──────────────────────────────────────────────────┤
│ 左栏 w-52 │ 右栏 flex-1                                       │
│          │                                                  │
│ [Segmented]│ ┌─ 主菜单 inline 编辑表单 ──────────────────┐  │
│ [+ 新增]  │ │ [标题 input] [保存]          [toggle]     │  │
│          │ │ 快捷键          全局快捷键                 │  │
│ 01 翻译  │ └────────────────────────────────────────────┘  │
│   ●AI·内置│                                                  │
│ 02 搜索 ◀│ ┌─ 子菜单列表（仅 submenu）──────────────────┐  │
│   ●URL   │ │ 01 翻译     上移 下移 编辑 删除             │  │
│          │ │ ...                                        │  │
└──────────┴──────────────────────────────────────────────────┘
```

### 2.3 左栏：主菜单列表

- 顶部 `Segmented` 场景过滤（全部 / 文本类 / 文件类——「文本类」「文件类」描述菜单项类型归属，不再用「选中文本」等令人困惑的操作性词汇）
- 「+ 新增主菜单」voice 主操作按钮
- 左栏宽度 `w-52`（shrink-0）
- **MenuRow 两行结构**（CSS grid 4 列）：
  - 色条（col 1，row-span-2）+ 序号（col 2，row-span-2）
  - 第一行（col 3-4）：标题（主菜单 `font-semibold` 加亮加粗）+ hover 操作栏（上移/下移/编辑/删除）
  - 第二行（col 3-4 `col-span-2`）：TypeTag（10px）+ `· 内置` / `· 已隐藏` 小字标记
- **单选高亮**：`bg-voice/12` + 色条 `h-full self-stretch`
- state：`selectedMainMenuId`，首次进 menu tab 默认选第一个，删除/过滤后 fallback `mainItems[0]`

### 2.4 右栏：主菜单详情 + 子菜单列表

**顶部 inline 编辑表单**（单卡片 `rounded-lg border bg-muted/15 p-4`）：
- **标题行**：`input` + 保存按钮（仅 `titleDraft !== null` 可点）+ 启用 Toggle（居右）
  - 标题用 `titleDraft` local state + 300ms debounce（IME 安全，防每按键 IPC 打断中文输入）
  - Enter 即时保存
- **叶子菜单（非 submenu）**：快捷键 + 全局快捷键 `grid-cols-2` 一行
  - 快捷键：`⌥ +` 单字符 inline 录制（`inlineCapturingShortcut`）
  - 全局快捷键：`ShortcutButton` + inline 录制（`inlineCapturingGlobal`）
- **类型不在此显示**——左侧 MenuRow 小字行已有，改类型走 EditForm（点编辑按钮）

**下方子菜单列表**（仅 submenu 类型）：
- 标题 + 「+ 新增子项」按钮
- `MenuRow` 复用，编辑走全屏 `EditForm`
- 叶子命令：显示「叶子命令无子项」提示

### 2.5 MenuRow 组件

CSS grid 布局 `[grid-template-columns: auto auto 1fr auto]`：

| prop | 说明 |
|---|---|
| `item` | ActionBarItem |
| `index` | 1-based 序号 |
| `selected` | 主菜单选中态（子菜单恒 false） |
| `isFirst/isLast` | 上移/下移 disabled |
| `deleteConfirmId` | 删除二次确认（同 ID 高亮 destructive） |
| `isMain?` | 主菜单 `font-semibold` 加亮加粗；子菜单不传 |
| `onSelect?` | 主菜单点击选中（子菜单不传） |
| `onMove/onEdit/onDelete` | 操作回调 |

**注意**：原 `subCount` prop 已删——子项计数徽章视觉不佳移除。

### 2.6 inline 录制范式

- `inlineCapturingShortcut`：单字符 0-9a-z，Backspace/Delete 清空，Esc 退出
- `inlineCapturingGlobal`：组合键 CmdOrCtrl/Alt/Shift + key，调 `check_shortcut` 校验
- 监听器生命周期绑定到 capturing state，useEffect cleanup 自动 removeEventListener

### 2.7 EditForm（全屏新增/编辑表单）

子菜单编辑走全屏 EditForm（点 MenuRow 编辑按钮 → `startEdit` → `editingId` set → EditForm 覆盖）。

**布局**：flex 列布局，内容 textarea 弹性填充消除双滚动条。
```
┌─ 导航栏（返回 + 类型标签 | 取消 + 保存）──────────────────┐
├────────────────────────────────────────────────────────┤
│ 标题                                                    │
│ 类型 select              启用 toggle                    │
│ 快捷键                   全局快捷键                      │
│ 执行选项（仅 script）                                   │
│ 内容 textarea ← flex-1 弹性填充                         │
│ 类型特定配置（triggerKeyword/agent/copy_path/extension） │
└────────────────────────────────────────────────────────┘
```

- ActionBarPanel 根 div：`isEditing` 时加 `h-full flex flex-col`
- EditForm 根 div：`flex min-h-0 flex-1 flex-col gap-3`
- 导航栏：`justify-between`（左=返回+类型标签，右=取消+保存），替代原底部操作栏
- 单卡片：`flex min-h-0 flex-1 flex-col gap-3`
- textarea：`min-h-[120px] flex-1`（替代原固定 `min-h-[200px]`），`FormField` 传 `className="flex min-h-0 flex-1 flex-col"`
- 执行选项（script 的异步/写剪贴板 toggle）放在内容 textarea 前面

### 2.8 执行记录 TAB（ScriptRunsList）

- 每条记录前加 checkbox（`selectedIds: Set<number>` state）
- 顶部工具栏：全选 checkbox + 「删除选中 (N)」按钮 + 「清理旧记录」按钮（右侧）
- 选中行边框 `border-voice/40` 强调
- 后端新增 `delete_script_runs(ids: Vec<i64>)` 命令 + `db::delete_script_runs`（unchecked_transaction 逐条 DELETE）

## 3. 删除项

- `TreeNode` / `NodeProps` / `TreeNodeBase` / `memo(TreeNodeBase)` ——树形渲染不再需要
- `expanded` / `toggle` / `expandAll` / `collapseAll` / `allExpanded` / `nodeCommon`
- `view` 状态机 ——TAB 替代
- 顶部工具栏的 records 按钮 / 展开收缩按钮 / 返回按钮 ——TAB 替代
- EditForm 原 6 个分散卡片 —— 单卡片替代
- EditForm 底部操作栏 —— 移到导航栏右侧

## 4. 不变量

1. **数据模型不变**——`items: ActionBarItem[]`、`parentId` 树形关系、所有 CRUD 命令原样复用
2. **选中态跨 EditForm 保留**——`selectedMainMenuId` 在父组件 state
3. **scope 过滤兼容**——左栏 mainItems 按 scope 过滤，过滤变化 fallback 第一个
4. **EditForm 复用**——子菜单编辑仍走全屏表单（字段/逻辑不变，仅布局重排）
5. **inline 编辑 saveFailed 回滚**——catch 也调 `refresh()` 重置到后端真实状态

## 5. i18n

新增 13 个 key（zh-CN + en 同步）：
- `menuManage` / `scriptRecords`（TAB 标签）
- `subItemsTitle` / `noSubItemsHint` / `leafNoSubItemsHint`（子菜单列表区）
- `selectMenuHint`（右栏未选中提示）
- `moveUp` / `moveDown` / `edit`（操作按钮 aria-label）
- `selectAll` / `deleteSelected`（执行记录批量删除）
- 场景过滤文案改写：`scopeText`「选中文本」→「文本类」，`scopeFile`「选中文件(夹)」→「文件类」

## 6. 验证

- `npx tsc --noEmit`：0 error
- `npm run build`：0 error
- `cargo build --release -p octopus-desktop`：0 error 0 warning
- code review 修复 3 Critical（IME debounce / submenu 类型约束 / extension 过滤）+ 1 Minor（accepts 重算）+ 1 Important（saveFailed 回滚）

## 7. 同批修复的 action-bar / overlay bug

本次会话还修复了以下独立 bug（非 ActionBarPanel 重构范畴，但在同批 commit 内）：

- **P1-1**：`check_and_consolidate_focus` 对已 dismiss 窗口 `set_focus`（加 `is_visible` 守卫）
- **P2-2**：`create_action_bar_window` build 失败静默吞错（加 `log::error`）
- **P2-3**：`get_mouse_position` 失败 fallback (100,100) → 返回 `Option`，detect_selection 用主屏中心占位继续检测
- **P3**：`primary_monitor_center` 注释精度 + `primary_monitor_logical_rect` DRY 提取

## 8. 不在本次范围

- 不改 ActionBar 浮窗（前端搜索/键盘导航不变）
- 不支持嵌套 submenu（原 TreeNode 递归支持任意深度，新 UI 最多一层）——如需支持需后端约束 + UI 重构
