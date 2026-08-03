# 终端 panel 可调宽度 + 记忆

> 终端窗口的 sidebar（tab 列表）和 file-tree（文件树）侧栏支持拖拽调整宽度，全局记住一份。
> **日期**：2026-07-31
> **关联**：[内嵌终端设计](2026-07-31-embedded-terminal-design.md)、[文件树设计](2026-07-31-terminal-file-tree-design.md)

## 目标

终端窗口两个固定宽度的侧栏——`.terminal-sidebar`（200px）和 `.file-tree-panel`（240px）——当前无法调整。用户需要：
1. **拖动边缘改宽度**：内容长时拉宽、占地方时收窄
2. **记住宽度**：下次打开终端窗口仍是上次调整后的值
3. **实时 refit**：拖动过程中 xterm 列数实时跟随容器宽度变化

## 范围

### 包含
- `.terminal-sidebar`（sidebar 布局模式的左侧 tab 列表）右边缘加拖拽手柄
- `.file-tree-panel`（右侧文件树，两种布局模式都有）左边缘加拖拽手柄
- 全局 localStorage 持久化（一份，所有终端窗口共享）
- 拖动时实时 `term.fit()`（复用现有 ResizeObserver）
- 启动时 clamp（窗口缩小后重开，已存宽度按当前窗口重算）

### 不包含
- tab 栏（tabs 模式顶部 `.terminal-tabbar`）高度调整——顶部 tab 栏是单行固定，无需调
- 终端窗口本身的尺寸记忆（Tauri 窗口 resize 已原生支持，本 spec 不处理窗口尺寸持久化）
- 每窗口/每 tab 独立记忆——全局一份，符合「布局偏好」语义

## 架构

### 实现方式：复用 CompactEditor 的自绘 splitter 模式

不引入 radix-ui / react-resizable-panels。工程已有成熟先例：`CompactEditor/MarkdownPane.tsx` 的 `onDividerDown/Move/Up`（pointer capture + `classList.add("dragging")` 禁选中文本 + localStorage 持久化）。本 spec 复用同一模式，差异点：

- CompactEditor 用 **ratio**（0~1 比例）存布局；本 spec 用**绝对像素**存（侧栏宽度是固定值，不是比例）
- CompactEditor 是中间 splitter（拖中间分两边）；本 spec 是**边缘 splitter**（拖侧栏边缘，只改侧栏宽，终端区 flex 自动填补剩余）

### 约束模型：只定 min，max 由终端最小宽度推导

不设固定 max。唯一硬约束是 `terminalWidth >= TERMINAL_MIN`，panel 的动态上限自动推导：

```
panelMax = containerWidth - TERMINAL_MIN - otherSidePanelWidth
panelWidth = clamp(raw, PANEL_MIN, panelMax)
```

| 常量 | 值 | 理由 |
|---|---|---|
| `PANEL_MIN` | **50px** | 统一。露出手柄（4px）+ 一条可见区域，让用户能抓住重新拉大。内容裁切由 `overflow: hidden` 处理 |
| `TERMINAL_MIN` | **320px** | 40 cols × ~7.8px/char + padding，xterm 实用下限（< 40 cols 命令行难用） |

**边界保证**：Tauri `min_inner_size(560)` 保证窗口 ≥ 560px。最差情况 sidebar(50) + 终端(320) + fileTree(50) = 420 < 560，恒满足约束，不会出现负宽。

### 组件结构

```typescript
// clampPanelWidth.ts（纯函数，TDD 入口）
function clampPanelWidth(
  raw: number,
  min: number,
  containerWidth: number,
  otherSideWidth: number,
  terminalMin: number
): number;

// usePanelWidth.ts（hook，封装「宽度状态 + 持久化 + 拖动回调」）
function usePanelWidth(storageKey: string, defaultWidth: number): {
  width: number;                                  // 渲染用值（已 clamp）
  startDrag: () => void;                          // mousedown 时调
  updateFromPointer: (                            // pointermove 时调
    clientX: number,
    containerRect: DOMRect,                       // .terminal-content 的 boundingRect
    panelEdge: "left" | "right",                  // 手柄方向
    otherSideWidth: number                        // 对侧 panel 当前宽
  ) => void;
  endDrag: () => void;                            // pointerup 时调（写 localStorage）
}
```

**hook 不监听窗口 resize**——`containerWidth` 在拖动时实时取（`containerRect.width`），不作为 hook 内部 state。这对应「窗口 resize 不主动 clamp」不变量：hook 只在拖动/启动时读容器尺寸。

### 拖拽手柄位置

- **sidebar**：手柄在 panel **右**边缘（`panelEdge="right"`）。鼠标向右拖 → 宽度增大。
- **file-tree**：手柄在 panel **左**边缘（`panelEdge="left"`）。鼠标向左拖 → 宽度增大。

### 「对侧 panel」判定

`otherSideWidth`（用于 clamp 的动态 max 计算）按当前布局实时确定，**不是固定值**：

| 当前拖动的 panel | 布局模式 | otherSideWidth |
|---|---|---|
| sidebar | sidebar | `fileTreeOpen ? fileTreeWidth : 0` |
| file-tree | sidebar | `sidebarWidth`（sidebar 模式必显示） |
| file-tree | tabs | `0`（tabs 模式无 sidebar） |

> tabs 模式下不存在「拖 sidebar」——sidebar 只在 sidebar 布局模式渲染。

### 数据流

```
用户 mousedown 手柄
  → setPointerCapture + classList.add("terminal-resizing")
  → usePanelWidth.startDrag()
onPointerMove:
  → 取 .terminal-content 的 DOMRect（实时容器尺寸）
  → sidebar 新宽 = clientX - rect.left（panelEdge=right）
    fileTree 新宽 = rect.right - clientX（panelEdge=left）
  → usePanelWidth.updateFromPointer(clientX, rect, panelEdge, otherSideWidth)
    → 内部 clampPanelWidth(raw, 50, rect.width, otherSideWidth, 320)
  → state 更新 → React 重渲染 → .terminal-content flex 重算 → 终端容器尺寸变
  → ResizeObserver 触发 fitAddon.fit() → term.onResize → pty.resize（已接好，零额外代码）
onPointerUp:
  → releasePointerCapture + classList.remove("terminal-resizing")
  → usePanelWidth.endDrag() → localStorage.setItem(key, width)
```

## 不变量

1. **全局一份**：所有终端窗口共享同一 `sidebarWidth` / `fileTreeWidth`（localStorage 单 key 各一）
2. **拖动时终端可见**：拖动过程实时 refit，终端区不会被 panel 挤没（clamp 保证 terminalWidth ≥ 320）
3. **持久化仅拖拽结束写**：拖动中不写 localStorage（频繁写影响性能），pointerup 才写一次
4. **min=50 保证可恢复**：无论 panel 多窄，至少露出 50px + 4px 手柄，用户总能抓住重新拉大
5. **启动 clamp**：读 localStorage 后立即按当前窗口宽 clamp，处理「窗口缩小后重开」场景
6. **窗口缩小时 panel 不动**：用户缩小 Tauri 窗口时，panel 宽度不自动跟随收缩（flex 终端区吸收差值）；只有终端区 < TERMINAL_MIN 时才在下次拖动/启动时 clamp

## 启动 clamp（边界情况）

```typescript
// 终端窗口打开时
const storedSidebar = localStorage.getItem(SIDEBAR_WIDTH_KEY);   // e.g. 280
const storedFileTree = localStorage.getItem(FILE_TREE_WIDTH_KEY); // e.g. 300
const containerWidth = window.innerWidth;                          // e.g. 560（用户缩小了窗口）

// 若同时展开两 panel，clamp 终端区 ≥ 320
const maxForSidebar = containerWidth - TERMINAL_MIN - (fileTreeOpen ? storedFileTree : 0);
const sidebarWidth = clamp(storedSidebar, PANEL_MIN, maxForSidebar);
// fileTree 同理对称
```

clamp 后的实际值不写回 localStorage（保留用户原始偏好，下次大窗口时恢复）——只作为本次渲染值。

## CSS

```css
/* 4px 拖拽手柄 */
.terminal-panel-resizer {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 4px;
  cursor: col-resize;
  background: transparent;
  z-index: 5;
  transition: background 0.12s;
}
.terminal-panel-resizer:hover,
.terminal-panel-resizer-dragging {
  background: var(--color-accent);
}
.terminal-panel-resizer-side-left { left: -2px; }   /* file-tree 用：手柄在左边缘 */
.terminal-panel-resizer-side-right { right: -2px; } /* sidebar 用：手柄在右边缘 */

/* 拖动中禁选中文本、禁 iframe/输入框抢焦点 */
.terminal-resizing {
  user-select: none !important;
  cursor: col-resize !important;
}
.terminal-resizing * { pointer-events: none !important; }
```

手柄绝对定位贴在 panel 边缘（`-2px` 让 4px 手柄跨边界居中），不影响 panel 内部布局。panel 本身需 `position: relative`。

## 窗口 resize 的行为（不主动 clamp）

用户拖动 Tauri 窗口边缘改变窗口大小时：
- panel 宽度（state）**不变**——由 flex 的终端区吸收窗口尺寸变化
- ResizeObserver 检测到终端容器尺寸变 → 自动 `fit()` → xterm cols/rows 更新
- 仅当终端区被挤到 < TERMINAL_MIN 时，下次拖 panel / 重开窗口才 clamp

这避免「窗口缩小时 panel 意外收缩」的困惑——用户预期 panel 是稳定的，终端区是弹性的。

## 测试策略

### 单元测试（pure logic，TDD 友好）

`usePanelWidth` 的 clamp 逻辑抽成纯函数 `clampPanelWidth(raw, min, containerWidth, otherSideWidth, terminalMin): number`，独立测：

| 场景 | 输入 | 期望 |
|---|---|---|
| 正常值 | raw=220, container=1000, otherSideWidth=240, min=50, terminalMin=320 | 220（不动） |
| 低于 min | raw=30, otherSideWidth=240 | 50 |
| 超过动态 max | raw=500, container=800, otherSideWidth=240 → max=800-320-240=240 | 240 |
| 对侧 panel 隐藏 | raw=600, container=800, otherSideWidth=0 → max=480 | 480 |
| 极小窗口 | raw=200, container=400, otherSideWidth=0 → max=400-320=80 | 80 |

### e2e 冒烟（手动）— ✅ 已合入 main，日常使用验证中

- 拖 sidebar 右边缘 → 宽度变化 + 终端实时 refit
- 拖 file-tree 左边缘 → 同上
- 松开后重开终端窗口 → 宽度恢复
- 拖到 50px → 内容裁切但手柄可见、可重新拉大
- 窗口缩小后重开 → 宽度 clamp 到合法范围

## 风险

1. **WKWebView pointer capture**：`setPointerCapture` 在 WKWebView 表现需验证。CompactEditor 已用同 API 且工作正常，风险低。fallback：用 `window` 级 `pointermove/pointerup` 监听（不依赖 capture）。
2. **拖动时 xterm 频繁 fit 的性能**：巨量输出场景下拖动可能卡。mitigation：fitAddon 内部已 debounce（xterm 6），且 ResizeObserver 本身已合批。若实测卡顿，加 rAF 节流（参考终端 rAF 节流先例）。
3. **多窗口同步**：全局一份 localStorage，多窗口同时打开时 A 窗口拖动改宽度，B 窗口不会自动同步（B 仍是旧值，直到 B 自己拖动或重开）。**可接受**——多窗口同时调宽度是极低频场景，且加跨窗口同步（storage event）复杂度不划算。
