# 终端内搜索（Cmd+F + SearchAddon）

> 内嵌终端增强。终端区内浮层搜索栏，增量搜索 + 上/下一个导航。

**日期**：2026-07-31
**蓝本**：Terax `SearchInline.tsx` + `rendererPool.ts`（SearchAddon 用法）
**关联**：[功能差距对比](../../research/2026-07-30-embedded-terminal-agent-analysis.md) P1

## 目标

终端输出长（日志/编译/agent 输出）时，Cmd+F 唤出搜索栏，实时高亮匹配 + 上/下一个导航，类似 VS Code / 浏览器 Cmd+F 体验。

## 范围

- ✅ Cmd+F 触发搜索栏（浮在终端区内右上角）
- ✅ 仅搜当前活跃 tab（多 tab 切换时搜索栏跟随活跃 tab）
- ✅ 增量搜索（输入时实时高亮）
- ✅ 上一个/下一个导航（↑↓ 按钮 + Enter/Shift+Enter）
- ✅ Esc 关闭 + clearDecorations + 焦点回终端
- ✅ 匹配高亮配色（matchBackground 灰 + activeMatchBackground 琥珀）
- ❌ 跨 tab 搜索（YAGNI，仅活跃 tab）
- ❌ 正则/大小写选项（Phase 1 简单子串搜索，后续按需加）

## 架构

### 组件结构

```
TerminalPane (active tab 才渲染搜索栏)
  ├─ useTerminalSession → session.searchAddon（SearchAddon 实例）
  ├─ TerminalPane 内部状态：searchOpen + query
  └─ SearchOverlay（searchOpen 时浮在终端区内右上角）
       ├─ <input> 增量搜索
       ├─ ↑ ↓ 按钮（findPrevious / findNext）
       └─ Esc 关闭
```

### 接口变更

**useTerminalSession**：
- 创建 `SearchAddon` + `term.loadAddon(searchAddon)`
- session 返回值加 `searchAddon: SearchAddon | null`

**TerminalPane**：
- 新增内部状态 `searchOpen: boolean`
- 渲染 `<SearchOverlay>`（仅 `searchOpen && active` 时）
- `active` prop 已有（WebGL 用），复用判定活跃 tab

**SearchOverlay.tsx**（新组件）：
```typescript
type Props = {
  addon: SearchAddon;       // 来自 useTerminalSession
  onClose: () => void;      // Esc 关闭回调
};
```

### Cmd+F 拦截

在 `useTerminalSession` 的 `attachCustomKeyEventHandler` 里加判定：
- Cmd+F（macOS）/ Ctrl+F（其他）→ 返回 `false`（不交 xterm），触发 `onSearchOpen` 回调
- `onSearchOpen` 由 TerminalPane 传入，设 `searchOpen(true)`

```typescript
// useTerminalSession opts 新增：
onSearchOpen?: () => void;

// handler 内（IME 检查后，readline 序列前）：
if ((IS_MAC ? event.metaKey : event.ctrlKey) && (event.key === "f" || event.code === "KeyF")
    && !event.altKey && !event.shiftKey) {
  event.preventDefault();
  if (event.type === "keydown") onSearchOpen?.();
  return false;
}
```

### SearchOverlay 交互

```typescript
// 增量搜索（onChange）
addon.findNext(query, { incremental: true, decorations: TERM_DECORATIONS });

// 上一个/下一个（按钮 + Enter）
addon.findNext(query, { decorations: TERM_DECORATIONS });       // 下一个
addon.findPrevious(query, { decorations: TERM_DECORATIONS });   // 上一个

// 关闭（Esc）
addon.clearDecorations();
onClose();
```

高亮配色（对齐 Terax）：
```typescript
const TERM_DECORATIONS = {
  matchBackground: "#515c6a",           // 灰：所有匹配
  activeMatchBackground: "#d18616",     // 琥珀：当前匹配
  matchOverviewRuler: "#d18616",
  activeMatchColorOverviewRuler: "#d18616",
};
```

### CSS

浮层绝对定位终端区内右上角（`position: absolute; top: 8px; right: 12px`），半透明背景 + 模糊（`backdrop-filter: blur(8px)`），z-index 高于 xterm canvas。宽度自适应（min 200px），含输入框 + ↑↓ 按钮 + 关闭按钮。

## 不变量

1. 搜索栏仅活跃 tab 显示（`searchOpen && active`）
2. Cmd+F 被 xterm handler 拦截（return false），不传给终端应用
3. 关闭搜索（Esc）必须 `clearDecorations` 清除高亮
4. 切 tab（active 变化）时搜索栏隐藏（TerminalPane 按 active 渲染）；切回不自动恢复搜索（简化）

## 测试策略

- **Cmd+F 拦截判定**：keymap.ts 加 `isFindShortcut(event, isMac)` 纯函数，可 TDD（Cmd+F / Ctrl+F / 带修饰排除）
- **搜索栏 UI + SearchAddon**：依赖真实 DOM + xterm，靠 e2e 冒烟（Cmd+F 打开 → 输入高亮 → ↑↓ 导航 → Esc 关闭）

## 依赖

| 新增 | 版本 | 用途 |
|---|---|---|
| `@xterm/addon-search` | ^0.16.0（对齐 Terax + 兼容 @xterm/xterm 6） | 终端内搜索 |

## 风险

1. **SearchAddon incremental 模式**：xterm 的 `findNext(query, { incremental: true })` 要求每次输入都调，且空 query 时手动 clearDecorations——已对齐 Terax 范式。
2. **Cmd+F vs 浏览器默认**：WKWebView 可能拦截 Cmd+F 做页面搜索——xterm handler 在 keydown 阶段 preventDefault + return false 应能阻止。
3. **多 tab SearchAddon 实例**：每 tab 独立 SearchAddon（随 Terminal 创建），互不干扰。
