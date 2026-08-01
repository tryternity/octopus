# 终端点击定位命令行光标

> **日期**：2026-08-01
> **关联**：[内嵌终端设计](2026-07-31-embedded-terminal-design.md)、[控制键设计](2026-07-31-terminal-keymap-design.md)

## 目标

终端命令行输入态，鼠标点击直接把光标移到点击位置（无需 Alt，精确）。类似 iTerm2 的体验，替代左右键移动光标。

## 背景：为什么不用 xterm 内置 Alt+Click

xterm 6.0 默认开启 `altClickMovesCursor`（Alt+Click 移动光标），实测能用但**不精确**——根因是 xterm 不知道 shell 真实光标位置，只能读 `buffer.active.cursorX`（渲染侧光标），多行/续行场景会偏。

我们自己做能更精确：用 OSC 133 门控确保只在命令行输入态响应 + 限制点击行 == 当前光标行（避免多行误判）。

## 范围

### 包含
- TerminalPane canvas click handler：点击换算列号 + 门控 + 发转义序列
- `useTerminalSession` 暴露 `inCommand`（OSC 133 状态）
- 直接点击（无需 Alt）

### 不包含
- **多行/续行点击**：只响应当前光标行（cursorY）的点击，跨行不处理
- **TUI 程序内的点击**：vim/less/htop 等（alternate screen 或 mouseTracking 开启）不拦截，交给程序自己处理
- **选中文本**：点击不干扰 xterm 的选择功能（拖拽选择仍走 xterm 原生）

## 架构

### 数据流

```
用户点击终端 canvas
  → DOM click 拿 clientX/clientY
  → 坐标换算：用 .xterm-screen getBoundingClientRect + cellWidth 算 clickCol/clickRow
  → 门控（三条件全满足才响应，否则放行给 xterm 原生处理）：
      1. shellState.inCommand === false（OSC 133 命令行输入态）
      2. term.buffer.active.type === 'normal'（非 alternate screen TUI）
      3. clickRow === term.buffer.active.cursorY（当前光标行，避免多行误判）
  → 算偏移：delta = clickCol - term.buffer.active.cursorX
  → 发转义序列：
      delta > 0 → 写 \x1b[{delta}C（CUF 右移）
      delta < 0 → 写 \x1b[{-delta}D（CUB 左移）
      delta === 0 → 不动
  → session.focus()
```

### 坐标换算

```typescript
const screen = containerRef.current.querySelector('.xterm-screen');
const rect = screen.getBoundingClientRect();
const cellWidth = rect.width / sessionRef.current.cols;
const cellHeight = rect.height / sessionRef.current.rows;
const clickCol = Math.floor((e.clientX - rect.left) / cellWidth);
const clickRow = Math.floor((e.clientY - rect.top) / cellHeight);
```

`.xterm-screen`（xterm.css:105）是 xterm 的内容区容器，WebGL renderer 在其下渲染 canvas（xterm.css:109）。它覆盖整个终端文本区，`getBoundingClientRect()` 是可靠的像素基准。

### 门控详解

| 条件 | 判据 | 防止什么 |
|---|---|---|
| 命令行输入态 | `shellState.inCommand === false` | 命令执行中（输出滚动）误触发光标移动 |
| 非 TUI 全屏 | `term.buffer.active.type === 'normal'` | vim/less/htop（alternate screen）内点击干扰程序 |
| 当前光标行 | `clickRow === term.buffer.active.cursorY` | 多行输入/历史输出区点击误判 |

**TUI 鼠标模式**：`term.mouseTrackingMode`（xterm.d.ts:1932）为非 `'none'` 时，TUI 程序已接管鼠标——但 type='normal' 门控已覆盖大部分 TUI（vim/htop 用 alternate screen）。mouseTrackingMode 作为额外保险（如 tmux 在 normal screen 开鼠标）。

### inCommand 暴露

当前 `ShellIntegrationState`（osc-handlers.ts:38）只存 `inCommand: boolean`，且是 `useTerminalSession` 内部闭包变量，**未暴露给消费者**。

改造：`useTerminalSession` 返回值加 `inCommand: boolean`（从 `shellState` 读）。TerminalPane 的 click handler 用 `sessionRef.current.inCommand`。

```typescript
// useTerminalSession.ts 返回值（line 343 附近）加：
return {
  // ... 现有字段
  inCommand: shellState.inCommand,
  // ...
};
```

注意：`shellState` 是闭包内变量（line 270），返回值里读 `shellState.inCommand` 时，由于 session 对象每次渲染重新构造（字面量），会读到最新值。

### 转义序列

readline 的 CUB（Cursor Backward）/ CUF（Cursor Forward）：
- `\x1b[nC` — CUF，光标右移 n 列
- `\x1b[nD` — CUB，光标左移 n 列

这些是 ANSI 标准序列，shell 的 readline（bash/zsh）直接响应。已有先例：`keymap.ts` 的 readline 导航用 `\x1b[C`/`\x1b[D`（单步），本功能用带参数版本 `\x1b[nC`/`\x1b[nD`（多步）。

## 不变量

1. **仅命令行输入态响应**（inCommand=false + normal buffer + 当前光标行）
2. **不干扰选择**：点击不阻止 xterm 的拖拽选择（click vs drag 区分——mousedown 后移动 > 阈值是选择，不触发光标移动）
3. **不干扰 TUI**：alternate screen / mouseTracking 状态下放行给 xterm 原生
4. **转义序列走 session.write**（直写 PTY，不走 paste——光标移动不是用户输入语义）
5. **delta=0 不动**（点击位置就是光标位置）

## click vs drag 区分

xterm 的文本选择是 mousedown → mousemove（拖拽）→ mouseup。光标移动应是**纯 click**（mousedown 后无移动直接 mouseup）。

实现：在 `mousedown` 记录起始坐标，`mouseup` 时判定移动距离 < 阈值（如 4px）才算 click（触发光标移动），否则视为选择（放行）。这是常见模式（Terax `TerminalPane.tsx` 的 block 选择用同样逻辑）。

## 测试策略

| 单元 | 覆盖 | 方式 |
|---|---|---|
| 坐标换算 | 像素 → 列号的边界（rect 边缘、cellWidth 整除） | 纯函数可单测（抽 `pixelToCol(clientX, rect, cols)`） |
| 门控逻辑 | inCommand/type/row 各组合 | 纯函数可单测（抽 `shouldMoveCursor(state)`） |
| 偏移 + 转义序列 | 正/负/零 delta | 纯函数可单测（抽 `buildCursorMoveSequence(delta)`） |
| click vs drag | 移动距离阈值 | e2e 手动 |

## 风险

1. **click 与右键菜单/拖拽选择冲突**：canvas 已有 `onContextMenu`（右键菜单）+ 文件拖拽的 `mouseup` 监听。需确保 click handler 不干扰这些——右键不触发 click（click 是左键），文件拖拽用 `takeDragPath()` 区分（拖拽中不触发光标移动）。
2. **续行命令**：命令超长换行时，`cursorY` 可能不是实际输入行——但限制 `clickRow === cursorY` 已规避（续行场景点击其他行不响应）。
3. **WKWebView focus**：点击后 `session.focus()` 确保焦点在 xterm（转义序列才能到达 readline）。
4. **shellState.inCommand 时效性**：`sessionRef.current.inCommand` 必须是最新值。session 对象每次渲染重新构造，sessionRef 持有最新——但 `shellState.inCommand` 在 OSC 133 触发时更新，React 重渲染前可能读到旧值。可接受（OSC 133 在 prompt 边界触发，用户点击时通常已稳定）。
