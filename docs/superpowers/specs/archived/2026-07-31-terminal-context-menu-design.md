# 终端右键菜单（三个区域）

> 内嵌终端增强。三个区域各自右键菜单。

**日期**：2026-07-31
**关联**：[功能差距对比](../../research/2026-07-30-embedded-terminal-agent-analysis.md)

## 目标

终端窗口三个区域各支持右键菜单：
1. **终端内容区**（xterm）：复制/粘贴/全选/清屏
2. **tab 标签/sidebar item**：改名/关闭/新建
3. **文件树**：复制路径（Phase 1 仅此一项）

## 范围

### 1. 终端内容区右键（核心）
- **复制**：有选中时 `term.getSelection()` → 系统剪贴板。无选中时禁用。**WKWebView 的 `navigator.clipboard` 不可靠**（e2e 验证），实际用「临时 textarea + `document.execCommand('copy')`」兜底。
- **粘贴**：`document.execCommand('paste')` → xterm 内置 textarea 聚焦后触发，xterm 监听 `paste` 事件写入 PTY。`navigator.clipboard.readText()` 在 WKWebView 受限。
- **全选**：`term.select(0, 0, term.cols, term.rows)`（选中当前可见 buffer）。
- **清屏**：`term.clear()`（清空 scrollback，保留当前行）。
- 右键时如果有选中，不丢失选中（先弹菜单，不先取消选区）。

### 2. tab 标签/sidebar item 右键
- **改名**：触发双击改名的同一逻辑（`setEditing(true)`）。
- **关闭**：`closeTab(id)`。
- **新建 tab**：`addTab()`。
- tabs 模式和 sidebar 模式都支持。

### 3. 文件树右键
- **复制路径**：复制文件/目录的完整路径到系统剪贴板。
- **复制名称**：复制文件/目录的名称到系统剪贴板。

## 架构

### 实现方式：原生 contextmenu 事件 + 自定义浮层

不用 radix-ui（octopus 前端没装）。用原生 `onContextMenu` + 一个简单的浮层菜单组件：

```typescript
// ContextMenu.tsx（新组件，通用浮层菜单）
type MenuItem = {
  label: string;
  action: () => void;
  disabled?: boolean;
  separator?: boolean; // 分隔线后的项
};

type Props = {
  items: MenuItem[];
  // 由调用方管理 open 状态 + 坐标
};
```

组件渲染为 `position: fixed` 浮层，点击外部关闭（`window click` 监听）。

### 终端内容区

`TerminalPane` 的容器 div 加 `onContextMenu`：
```typescript
onContextMenu={(e) => {
  e.preventDefault();
  openMenu(e.clientX, e.clientY, terminalMenuItems);
}}
```

菜单项调 xterm Terminal 实例的方法（通过 `useTerminalSession` 暴露的 ref 或回调）。

需要 `useTerminalSession` 额外暴露：
- `hasSelection()` → `term.hasSelection()`
- `getSelection()` → `term.getSelection()`
- `paste(text)` → `term.paste(text)`
- `selectAll()` → `term.select(...)`
- `clear()` → `term.clear()`

### tab 标签 / sidebar item

`TabButton` 和 `SidebarItem` 的 div 加 `onContextMenu`，菜单项调已有的 `onRename`/`onClose` + 父组件的 `addTab`。

### 文件树

`FileTreePanel` 的树节点加 `onContextMenu`，菜单项复制 `fullPath`。

## 不变量

1. 终端右键不取消已有选区（先弹菜单）
2. 无选中时「复制」禁用（disabled）
3. 菜单点击外部关闭
4. **剪贴板一律走 `document.execCommand`**（WKWebView `navigator.clipboard` 不可靠，e2e 验证失败）
5. tab 改名复用 `forceEditing` prop（不依赖 `window.prompt`，后者在 WKWebView 不工作）

## 测试策略

- **ContextMenu 组件**：纯 UI，靠 e2e 冒烟
- **菜单项逻辑**：复制/粘贴/清屏调 xterm API，靠 e2e（右键 → 点复制 → 系统剪贴板验证）

## 风险（已 e2e 验证，2026-07-31）

1. **WKWebView clipboard 权限**：~~`navigator.clipboard.readText()` 可能需要 HTTPS 或 user gesture~~ → **实测不可靠**，复制/粘贴一律改走 `document.execCommand('copy'/'paste')`（e2e 通过）。复制需临时 textarea 聚焦，粘贴依赖 xterm 内置 textarea 处于聚焦态。
2. **xterm selectAll**：`term.select(0, 0, term.cols, term.rows)` 已验证可用（xterm 6）。
3. **菜单点击外部不关闭**：contextmenu 事件不触发 mousedown/click，需在 `useEffect` 内立即注册监听（不延迟），且同时监听 mousedown + click + contextmenu + wheel + Escape（capture 阶段）。已修复（commit `3cfaddf6`）。
