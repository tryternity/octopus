# 终端点击定位命令行光标

> **状态：❌ 探索失败，已回滚——当前用 xterm 内置 altClickMovesCursor（不精确但可用）**
> **日期**：2026-08-01（探索），2026-08-02（回滚）
> **关联**：[内嵌终端设计](2026-07-31-embedded-terminal-design.md)、[控制键设计](2026-07-31-terminal-keymap-design.md)

## 探索记录（失败教训 + 成功经验，供后续优化参考）

### 目标

命令行输入态，鼠标点击（或 Alt+Click）直接把光标移到点击位置，替代左右键移动。比 xterm 内置 altClickMovesCursor 更精确。

### xterm 内置 altClickMovesCursor 的问题

xterm 6.0 默认开启 `altClickMovesCursor: true`——Alt+Click 移动光标。**能用但不精确**：根因是 xterm 不知道 shell 真实光标位置，只能读 `buffer.active.cursorX`（渲染侧光标），多行/续行场景会偏。

### 我们的方案（已回滚）

试图自定义实现：document mousedown/mouseup 监听 Alt+Click → 坐标换算 → 门控 → 发方向键序列移动光标。

### 失败的三个根因

1. **React onClick/onMouseDown 被 xterm canvas 内部元素拦截**——xterm 的内部 canvas/textarea 不让 click/mousedown 冒泡到 `.terminal-pane-canvas`。改 document 级监听后 mousedown/mouseup 能收到，但这引出后续问题。

2. **OSC 133 inCommand 门控不稳定**——shell 启动时序导致 `inCommand` 卡在 true（precmd 的 D+A 标记被 registerPromptTracker 漏掉），`isPromptActive()` 永远返回 false，门控永远挡住。放宽门控（去掉 inCommand）后，命令执行中也响应（不安全）。

3. **方向键序列 readline 不认**——用 CUF/CUB（`\x1b[nC`）shell 字面输出 C；改用方向键重复（`\x1b[C` × n）后仍然不工作（shell 未在 readline 输入态时方向键被当字面字符）。根因可能是 shell 不在 prompt 态（OSC 133 问题导致），或 PTY write 的时序问题。

### 成功经验（保留）

1. **Maximum update depth exceeded 修复**（`1aa2abd6`）——探索过程中发现并修复的预存 bug：TerminalPane 的 onPtyId/onCwd effect 依赖内联回调（每次渲染新引用）+ setTabs 无等值判断 → 无限循环。修复：回调用 ref 持有 + setTabs 加等值短路。**这个修复独立有效，已保留。**

2. **clickCursor.ts 纯函数**（pixelToCol / shouldMoveCursor / buildCursorMoveSequence）——坐标换算 + 门控逻辑正确（13 测试通过）。文件保留备用。后续如果找到正确的光标移动方式，这些纯函数可直接复用。

3. **useTerminalSession 的 onPtyId/onCwd 回调用 ref 持有**——TerminalPane 上报 effect 的稳定化模式（对齐 AGENTS.md listener gotcha），已保留。

### 后续优化方向（待探索）

- **修 OSC 133 时序**：registerPromptTracker 注册时机晚于 shell 第一个 precmd，导致 inCommand 卡 true。修这个后 isPromptActive 门控才能可靠工作。
- **用 xterm API 而非 PTY write**：xterm 内置的 altClickMovesCursor 可能用 `term.input()` 或内部 API（而非写 PTY）。研究 xterm 源码的 `_moveToCell` 或类似方法，用 xterm 原生方式移动光标。
- **terax 参考**：Terax 用 block 模式（命令行输入是 DOM input，不是 xterm grid），所以它不需要「点击定位 xterm 光标」。octopus 走纯 xterm grid 路线，无现成参考。

---

以下为原设计文档（探索方案，已回滚，保留供参考）：


## 目标

终端命令行输入态，鼠标点击直接把光标移到点击位置（无需 Alt，精确）。类似 iTerm2 的体验，替代左右键移动光标。

## 背景：为什么不用 xterm 内置 Alt+Click

xterm 6.0 默认开启 `altClickMovesCursor`（Alt+Click 移动光标），实测能用但**不精确**——根因是 xterm 不知道 shell 真实光标位置，只能读 `buffer.active.cursorX`（渲染侧光标），多行/续行场景会偏。

我们自己做能更精确：用 OSC 133 门控确保只在命令行输入态响应 + 限制点击行 == 当前光标行（避免多行误判）。

## 范围

### 包含
- TerminalPane canvas click handler：点击换算列号 + 门控 + 发转义序列
- `useTerminalSession` 暴露 `isPromptActive()`（OSC 133 click-time reader）
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

### isPromptActive 暴露（click-time reader）

当前 `ShellIntegrationState`（osc-handlers.ts:38）只存 `inCommand: boolean`，且是 `useTerminalSession` 内部闭包变量，**未暴露给消费者**。

改造：`useTerminalSession` 返回值加 `isPromptActive(): boolean`——**click-time 闭包 reader**（非 render-time 快照）。

```typescript
// useTerminalSession.ts 返回值加：
isPromptActive: () => !shellStateRef.current.inCommand,
```

**为什么用闭包 reader 而非字段**：`inCommand` 在 OSC 133 触发时更新（`updateShellIntegration` 原地改 `shellStateRef.current.inCommand`），但**不触发 React re-render**。若 session 暴露 `inCommand: boolean` 字段（render-time 快照），click 时读到的可能是 stale 值（命令执行中误判为可输入）。用闭包 reader（`() => !shellStateRef.current.inCommand`），click 时读 live 值，确保准确。

`shellStateRef` 是外层 ref（`useRef(createShellIntegrationState())`），OSC 133 的 `registerPromptTracker` 持有同一对象引用，原地修改 inCommand，闭包 reader 读到最新。TerminalPane 的 click handler 用 `!s.isPromptActive()` 作 inCommand 门控值。

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
