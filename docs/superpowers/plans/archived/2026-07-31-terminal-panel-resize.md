# 终端 Panel 可调宽度 + 记忆 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 终端窗口的 sidebar（tab 列表）和 file-tree（文件树）支持拖拽边缘改宽度，全局 localStorage 记住一份。

**Architecture:** 复用 CompactEditor 的自绘 splitter 模式（pointer capture + classList dragging + localStorage 持久化），差异点是用绝对像素存而非 ratio，且只定 min=50（max 由终端最小宽度 320 推导）。三个新单元：`clampPanelWidth` 纯函数（TDD 入口）+ `usePanelWidth` hook（状态/持久化/拖动回调）+ `PanelResizer` 组件（4px 手柄）。

**Tech Stack:** React 19 + TypeScript + vitest，零新依赖。

**Spec:** `docs/superpowers/specs/2026-07-31-terminal-panel-resize-design.md`

## Global Constraints

- `PANEL_MIN = 50`（px，统一两 panel，保证手柄可见可恢复）
- `TERMINAL_MIN = 320`（px，xterm 实用下限 40 cols）
- `SIDEBAR_DEFAULT = 200`、`FILE_TREE_DEFAULT = 240`（与现状一致，不改默认体验）
- localStorage key：`octopus-terminal-sidebar-width`（存数字字符串）、`octopus-terminal-file-tree-width`（存数字字符串）
- 命名先例：`LAYOUT_KEY = "octopus-terminal-layout"`（`index.tsx:51`），本 plan 沿用 `octopus-terminal-` 前缀
- 拖动用 Pointer Events（`onPointerDown/Move/Up/Cancel`）+ `setPointerCapture`（CompactEditor `MarkdownPane.tsx:97-116` 已验证 WKWebView 可用）
- 纯函数测试不依赖 DOM，沿用 `agent-activity.test.ts` 范式（`describe/it/expect`，无 `beforeEach`）
- 不引入 `react-resizable-panels` / radix-ui（spec 已否决）

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.ts` | 纯函数：`clampPanelWidth(raw, min, containerWidth, otherSideWidth, terminalMin)` | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.test.ts` | clamp 纯函数测试（5 场景） | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/usePanelWidth.ts` | hook：`usePanelWidth(storageKey, defaultWidth)` → `{ width, startDrag, updateFromPointer, endDrag }` | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/PanelResizer.tsx` | 4px 拖拽手柄组件 | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx` | 加 `width?: number` prop，根 div 用 `style={{ width }}` 覆盖 CSS 默认 | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/index.tsx` | 引入两 hook + 渲染两 PanelResizer + 传 width 给 FileTreePanel + sidebar 加 style | 修改 |
| `crates/desktop/frontend/src/index.css` | `.terminal-panel-resizer` 样式 + sidebar/file-tree-panel 加 `position: relative` | 修改 |

**Decomposition 理由**：clampPanelWidth 纯函数无 React 依赖，独立可测；usePanelWidth 封装状态+持久化（单一职责）；PanelResizer 只管手柄渲染和 pointer 事件转发（不持有宽度状态，由父用 hook 控制）；index.tsx 是唯一知道两侧布局关系的「协调者」，故「对侧宽度」计算放这里。

---

### Task 1: clampPanelWidth 纯函数（TDD 入口）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.ts`
- Test: `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.test.ts`

**Interfaces:**
- Produces: `clampPanelWidth(raw: number, min: number, containerWidth: number, otherSideWidth: number, terminalMin: number): number` —— Task 2 的 `usePanelWidth.updateFromPointer` 调用此函数。

- [x] **Step 1: Write the failing test**

Create `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { clampPanelWidth } from "./clampPanelWidth";

describe("clampPanelWidth", () => {
  const MIN = 50;
  const TERMINAL_MIN = 320;

  it("正常值不动（在 min 与动态 max 之间）", () => {
    // container=1000, otherSide=240 → max = 1000-320-240 = 440；raw=220 在 [50,440]
    expect(clampPanelWidth(220, MIN, 1000, 240, TERMINAL_MIN)).toBe(220);
  });

  it("低于 min 收到 min", () => {
    expect(clampPanelWidth(30, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
    expect(clampPanelWidth(0, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
    expect(clampPanelWidth(-10, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
  });

  it("超过动态 max 收到 max", () => {
    // container=800, otherSide=240 → max = 800-320-240 = 240；raw=500 超过
    expect(clampPanelWidth(500, MIN, 800, 240, TERMINAL_MIN)).toBe(240);
  });

  it("对侧 panel 隐藏（otherSide=0）时 max 更大", () => {
    // container=800, otherSide=0 → max = 800-320 = 480；raw=600 超过 → 480
    expect(clampPanelWidth(600, MIN, 800, 0, TERMINAL_MIN)).toBe(480);
  });

  it("极小窗口：动态 max < min 时，min 优先（保证手柄可见）", () => {
    // container=400, otherSide=0 → max = 400-320 = 80；raw=200 超过 80
    expect(clampPanelWidth(200, MIN, 400, 0, TERMINAL_MIN)).toBe(80);
    // container=300, otherSide=0 → max = -20 < 50；min 优先
    expect(clampPanelWidth(100, MIN, 300, 0, TERMINAL_MIN)).toBe(50);
  });

  it("NaN raw 回退到 min", () => {
    expect(clampPanelWidth(NaN, MIN, 1000, 0, TERMINAL_MIN)).toBe(50);
  });
});
```

- [x] **Step 2: Run test to verify it fails**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/clampPanelWidth.test.ts`
Expected: FAIL，报 `Failed to resolve import "./clampPanelWidth"` 或 `clampPanelWidth is not defined`。

- [x] **Step 3: Write minimal implementation**

Create `crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.ts`:

```typescript
/**
 * 纯函数：panel 宽度 clamp。
 *
 * 约束模型：只定 min（保证手柄可见可恢复），max 由 terminalMin 推导——
 * 终端区至少要留 terminalMin，所以 panelMax = containerWidth - terminalMin - otherSideWidth。
 *
 * 当 panelMax < min 时（极小窗口），min 优先——保证用户总能抓住手柄重新拉大，
 * 此时终端区会被挤到 < terminalMin，但 Tauri min_inner_size(560) 兜底，实际不会到这步。
 */
export function clampPanelWidth(
  raw: number,
  min: number,
  containerWidth: number,
  otherSideWidth: number,
  terminalMin: number,
): number {
  const safeRaw = Number.isFinite(raw) ? raw : min;
  const dynamicMax = containerWidth - terminalMin - otherSideWidth;
  // dynamicMax < min 时（极小窗口），min 优先：max(min, ...) 保证下界
  const effectiveMax = Math.max(min, dynamicMax);
  return Math.min(effectiveMax, Math.max(min, safeRaw));
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/clampPanelWidth.test.ts`
Expected: PASS（6 个 it 全过）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.ts crates/desktop/frontend/src/pages/Terminal/clampPanelWidth.test.ts
git commit -m "feat(terminal): clampPanelWidth 纯函数 + 测试（panel resize TDD 入口）"
```

---

### Task 2: usePanelWidth hook

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/usePanelWidth.ts`

**Interfaces:**
- Consumes: `clampPanelWidth` from Task 1（签名见上）。
- Produces:
  ```typescript
  type PanelEdge = "left" | "right";
  function usePanelWidth(storageKey: string, defaultWidth: number): {
    width: number;                                          // 渲染用值
    startDrag: () => void;                                  // mousedown 调
    updateFromPointer: (                                    // pointermove 调
      clientX: number,
      containerRect: DOMRect,
      panelEdge: PanelEdge,
      otherSideWidth: number,
    ) => void;
    endDrag: () => void;                                    // pointerup 调（落 localStorage）
    clampTo: (containerWidth: number, otherSideWidth: number) => void;  // 启动时按容器 clamp
  }
  ```
- Task 3、Task 4 调用 `usePanelWidth` 的返回值。

- [x] **Step 1: Write the hook implementation**

Create `crates/desktop/frontend/src/pages/Terminal/usePanelWidth.ts`:

```typescript
/**
 * panel 宽度 hook：状态 + localStorage 持久化 + 拖动回调。
 *
 * - 初始值：localStorage 读取，缺失用 defaultWidth。不在此处 clamp（clamp 在
 *   updateFromPointer 拖动时 + index.tsx 启动时按容器尺寸做）。
 * - 拖动中只更新 state（不写 localStorage，避免逐帧 IO），pointerup 时 endDrag 落盘。
 * - 复用 CompactEditor MarkdownPane.tsx 的 ref + persist 模式（line 58-62, 110-116）。
 */
import { useCallback, useRef, useState } from "react";
import { clampPanelWidth } from "./clampPanelWidth";

export type PanelEdge = "left" | "right";

export const PANEL_MIN = 50;
export const TERMINAL_MIN = 320;

export function usePanelWidth(storageKey: string, defaultWidth: number) {
  const [width, setWidth] = useState<number>(() => {
    const saved = Number(localStorage.getItem(storageKey));
    return Number.isFinite(saved) && saved > 0 ? saved : defaultWidth;
  });
  const widthRef = useRef(width);
  const draggingRef = useRef(false);

  // widthRef 同步——endDrag 时读 ref 落盘，避免闭包陷阱（依赖数组遗漏 width）
  widthRef.current = width;

  const startDrag = useCallback(() => {
    draggingRef.current = true;
    document.documentElement.classList.add("terminal-resizing");
  }, []);

  const updateFromPointer = useCallback(
    (
      clientX: number,
      containerRect: DOMRect,
      panelEdge: PanelEdge,
      otherSideWidth: number,
    ) => {
      if (!draggingRef.current) return;
      // panelEdge="right"（sidebar，手柄在右边缘）：宽度 = clientX - 容器左
      // panelEdge="left"（file-tree，手柄在左边缘）：宽度 = 容器右 - clientX
      const raw =
        panelEdge === "right"
          ? clientX - containerRect.left
          : containerRect.right - clientX;
      const next = clampPanelWidth(
        raw,
        PANEL_MIN,
        containerRect.width,
        otherSideWidth,
        TERMINAL_MIN,
      );
      setWidth(next);
    },
    [],
  );

  const endDrag = useCallback(() => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    document.documentElement.classList.remove("terminal-resizing");
    localStorage.setItem(storageKey, String(widthRef.current));
  }, [storageKey]);

  /** 启动时按容器尺寸 clamp 已存宽度（不写 localStorage，只改本次渲染值）。
   *  场景：用户拖大 sidebar 后缩小窗口、重开——已存宽度可能让终端区 < TERMINAL_MIN。 */
  const clampTo = useCallback(
    (containerWidth: number, otherSideWidth: number) => {
      setWidth((prev) =>
        clampPanelWidth(prev, PANEL_MIN, containerWidth, otherSideWidth, TERMINAL_MIN),
      );
    },
    [],
  );

  return { width, startDrag, updateFromPointer, endDrag, clampTo };
}
```

**注意**：`widthRef.current = width` 直接赋值（非 useEffect）——render 期同步，确保 endDrag 闭包读到的总是最新 width。CompactEditor 用 `useEffect` 同步 ref 是因为它在 render 中读 ref 做布局计算；这里只在 event handler 读，render 期赋值更直接（React 允许 render 期写自己的 ref，但不允许写别人的）。

- [x] **Step 2: Type check**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error（hook 无消费者时不会报未使用，因 export 了）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/usePanelWidth.ts
git commit -m "feat(terminal): usePanelWidth hook——宽度状态 + 持久化 + 拖动回调"
```

---

### Task 3: PanelResizer 组件 + CSS

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/PanelResizer.tsx`
- Modify: `crates/desktop/frontend/src/index.css`（在终端布局区块，`.file-tree-panel` 定义附近）

**Interfaces:**
- Consumes: 无（纯展示 + pointer 事件转发）
- Produces: `PanelResizer` 组件，Task 4 在 index.tsx 渲染。

- [x] **Step 1: Write PanelResizer component**

Create `crates/desktop/frontend/src/pages/Terminal/PanelResizer.tsx`:

```typescript
/**
 * panel 拖拽手柄——4px 宽，绝对定位贴在 panel 边缘。
 *
 * 只负责 pointer 事件转发（不持有宽度状态），宽度逻辑由父用 usePanelWidth hook 控制。
 * 侧边条手柄（side="right" 贴右边缘，sidebar 用）；side="left" 贴左边缘（file-tree 用）。
 *
 * 参考实现：CompactEditor MarkdownPane.tsx:205-216（pointer capture + dragging class）。
 */
import { useRef } from "react";

type Props = {
  side: "left" | "right";
  onStart: () => void;
  onMove: (clientX: number) => void;
  onEnd: () => void;
};

export function PanelResizer({ side, onStart, onMove, onEnd }: Props) {
  const draggingRef = useRef(false);

  const handleDown = (e: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    onStart();
  };

  const handleMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    onMove(e.clientX);
  };

  const handleUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      /* 已 release */
    }
    onEnd();
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      className={`terminal-panel-resizer terminal-panel-resizer-side-${side}`}
      onPointerDown={handleDown}
      onPointerMove={handleMove}
      onPointerUp={handleUp}
      onPointerCancel={handleUp}
    />
  );
}
```

- [x] **Step 2: Add CSS**

在 `crates/desktop/frontend/src/index.css` 找到 `.file-tree-panel {` 定义（约 840 行），在其**前面**插入 resizer 样式：

```css
/* ── panel 拖拽手柄（4px，贴在 panel 边缘）── */
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
.terminal-resizing .terminal-panel-resizer {
  background: var(--color-accent);
}
.terminal-panel-resizer-side-left { left: -2px; }   /* file-tree 用：手柄在左边缘 */
.terminal-panel-resizer-side-right { right: -2px; } /* sidebar 用：手柄在右边缘 */

/* 拖动中：禁选中文本、统一光标 */
.terminal-resizing {
  user-select: none !important;
  cursor: col-resize !important;
}
```

然后在 `.terminal-sidebar {` 和 `.file-tree-panel {` 两个规则里各加 `position: relative;`（让手柄 absolute 相对 panel 定位）：

`.terminal-sidebar {`（约 715 行）改成：
```css
.terminal-sidebar {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 200px;
  flex-shrink: 0;
  background: var(--color-background);
  border-right: 1px solid var(--color-border);
  overflow: hidden;
}
```

`.file-tree-panel {`（约 840 行）改成：
```css
.file-tree-panel {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 240px;
  flex-shrink: 0;
  background: var(--color-background);
  border-left: 1px solid var(--color-border);
  overflow: hidden;
}
```

- [x] **Step 3: Type check + build**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/PanelResizer.tsx crates/desktop/frontend/src/index.css
git commit -m "feat(terminal): PanelResizer 组件 + 手柄 CSS（4px col-resize）"
```

---

### Task 4: index.tsx 接线 + 启动 clamp

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx`（加 width prop）

**Interfaces:**
- Consumes: `usePanelWidth`、`PanelResizer`、`PANEL_MIN`、`TERMINAL_MIN` from Task 2/3。
- Produces: 可拖拽的 sidebar + file-tree，宽度持久化。

- [x] **Step 1: FileTreePanel 加 width prop + 内嵌 resizer**

Modify `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx`。

Props 类型加 `width` + resizer 回调（resizer 放 FileTreePanel 内部，因为只有展开态需要）：

```typescript
type Props = {
  cwd: string | null;
  expanded: boolean;
  onToggle: () => void;
  width?: number;  // 可选：拖拽改宽度，不传用 CSS 默认 240px
  // resizer 回调（展开态才渲染手柄）
  onResizerStart?: () => void;
  onResizerMove?: (clientX: number) => void;
  onResizerEnd?: () => void;
};

export function FileTreePanel({ cwd, expanded, onToggle, width, onResizerStart, onResizerMove, onResizerEnd }: Props) {
```

展开态根 div（约 201 行，`return ( <div className="file-tree-panel">`）改成带 width style + 内嵌 PanelResizer：

```typescript
  return (
    <div
      className="file-tree-panel"
      style={width !== undefined ? { width: `${width}px` } : undefined}
    >
      {/* 拖拽手柄（左边缘）——仅展开态渲染 */}
      {onResizerStart && onResizerMove && onResizerEnd && (
        <PanelResizer
          side="left"
          onStart={onResizerStart}
          onMove={onResizerMove}
          onEnd={onResizerEnd}
        />
      )}
      <div className="file-tree-toolbar">
```

文件顶部 import 加：`import { PanelResizer } from "./PanelResizer";`

> **设计决策**：resizer 放 FileTreePanel 展开态根 div 内部，而非外层 wrapper——因为收缩态（`.file-tree-collapsed`，24px 小条）不需要 resizer，FileTreePanel 自己控制何时展开。`.file-tree-panel` 在 Task 3 已加 `position: relative`，PanelResizer 的 `position: absolute` 相对它定位。

- [x] **Step 2: index.tsx 引入 hooks + 容器 ref**

Modify `crates/desktop/frontend/src/pages/Terminal/index.tsx`。

现有 import 行（约 20 行 `import { useEffect, useState, useCallback } from "react";`）改为加 `useRef`：
```typescript
import { useEffect, useState, useCallback, useRef } from "react";
```

import 区加（TerminalPane import 附近）：
```typescript
import { usePanelWidth } from "./usePanelWidth";
```

在 `const [fileTreeOpen, setFileTreeOpen] = useState(false);`（约 67 行）后面加：
```typescript
  // ── panel 宽度（拖拽 + localStorage 持久化，全局一份）──
  const sidebarWidthCtrl = usePanelWidth("octopus-terminal-sidebar-width", 200);
  const fileTreeWidthCtrl = usePanelWidth("octopus-terminal-file-tree-width", 240);
  // .terminal-content 容器 ref——拖动时实时取 boundingRect 算 clamp 边界
  const contentRef = useRef<HTMLDivElement>(null);
```

- [x] **Step 3: 启动 clamp**

index.tsx 加一个 useEffect（放在上面 hook 声明之后）：

```typescript
  // 启动 clamp：窗口缩小后重开，已存宽度按当前容器重算。
  // 不写回 localStorage（保留用户偏好，下次大窗口恢复），只改本次渲染值。
  // 依赖：空数组（仅启动时跑一次，用闭包内的初始 width 值）。
  useEffect(() => {
    if (!contentRef.current) return;
    const rect = contentRef.current.getBoundingClientRect();
    const isSidebarLayout = layout === "sidebar";
    const otherForFileTree = isSidebarLayout ? sidebarWidthCtrl.width : 0;
    const otherForSidebar = fileTreeOpen ? fileTreeWidthCtrl.width : 0;
    fileTreeWidthCtrl.clampTo(rect.width, otherForFileTree);
    sidebarWidthCtrl.clampTo(rect.width, otherForSidebar);
    // 二次收敛：sidebar clamp 后宽度可能变小，fileTree 的 otherSide 应重算
    if (isSidebarLayout && fileTreeOpen) {
      fileTreeWidthCtrl.clampTo(rect.width, sidebarWidthCtrl.width);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
```

> **注意**：`contentRef.current` 在首次渲染后才有值，所以 useEffect 在 mount 后跑（React 保证）。`.terminal-content` div 要加 `ref={contentRef}`（Step 5 会改两处渲染分支）。

- [x] **Step 4: 给 .terminal-content 加 ref + sidebar 加 width style + resizer**

sidebar 模式渲染（约 254 行 `<div className="terminal-window terminal-sidebar-layout">` 块内）：

sidebar aside（约 255 行）改为带 width style + 内嵌 resizer：
```typescript
        <aside
          className="terminal-sidebar"
          style={{ width: `${sidebarWidthCtrl.width}px` }}
        >
          <div className="terminal-sidebar-header">
            {/* ... 原内容不变 ... */}
          </div>
          <div className="terminal-sidebar-list" role="tablist">
            {/* ... 原 SidebarItem 列表不变 ... */}
          </div>
          {/* 拖拽手柄（右边缘）——sidebar 没有收缩态，始终渲染 */}
          <PanelResizer
            side="right"
            onStart={sidebarWidthCtrl.startDrag}
            onMove={(clientX) => {
              if (!contentRef.current) return;
              sidebarWidthCtrl.updateFromPointer(
                clientX,
                contentRef.current.getBoundingClientRect(),
                "right",
                fileTreeOpen ? fileTreeWidthCtrl.width : 0,
              );
            }}
            onEnd={sidebarWidthCtrl.endDrag}
          />
        </aside>
        <div className="terminal-content" ref={contentRef}>
          {panes}
          {fileTree}
        </div>
```

> `.terminal-sidebar` 在 Task 3 已加 `position: relative`，PanelResizer 相对它定位。

- [x] **Step 5: tabs 模式 .terminal-content 加 ref + fileTree 传 width/resizer**

tabs 模式渲染（约 333 行 `<div className="terminal-window">` 块内），把 `<div className="terminal-content">` 改为加 ref：
```typescript
      <div className="terminal-content" ref={contentRef}>
        {panes}
        {fileTree}
      </div>
```

fileTree 变量定义（约 234 行）改成传 width + resizer 回调：
```typescript
  const fileTree = (
    <FileTreePanel
      cwd={activeTabCwd}
      expanded={fileTreeOpen}
      onToggle={() => setFileTreeOpen(!fileTreeOpen)}
      width={fileTreeWidthCtrl.width}
      onResizerStart={fileTreeWidthCtrl.startDrag}
      onResizerMove={(clientX) => {
        if (!contentRef.current) return;
        fileTreeWidthCtrl.updateFromPointer(
          clientX,
          contentRef.current.getBoundingClientRect(),
          "left",
          layout === "sidebar" ? sidebarWidthCtrl.width : 0,
        );
      }}
      onResizerEnd={fileTreeWidthCtrl.endDrag}
    />
  );
```

> FileTreePanel 收缩态（`expanded=false`）返回 `.file-tree-collapsed`（24px 小条），不渲染 resizer（Step 1 的 `onResizerStart && ...` 条件保证）。展开态才有 resizer。

- [x] **Step 6: Type check + test + build**

Run:
```bash
cd crates/desktop/frontend
npx tsc --noEmit
npx vitest run
npm run build
```
Expected: tsc 0 error，vitest 全过（原 68 测试 + Task 1 的 6 个 = 74），build 成功。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/index.tsx crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx
git commit -m "feat(terminal): sidebar + file-tree 可拖拽改宽度，localStorage 记忆"
```

---

### Task 5: e2e 冒烟 + 文档同步

**Files:**
- 无代码改动，验证 + 文档

- [x] **Step 1: 构建桌面应用并手动验证**

Run（在 worktree 根）:
```bash
./scripts/build-macos-dmg.sh --no-lto --open
```
或在开发模式：
```bash
cd crates/desktop && cargo run -p octopus-desktop --features embedded,custom-protocol
```

手动测试清单（对照 spec 测试策略）：
1. ✅ sidebar 模式：拖 sidebar 右边缘 → 宽度变化，终端实时 refit（cols 变化）
2. ✅ sidebar 模式：拖 file-tree 左边缘 → 同上
3. ✅ tabs 模式：拖 file-tree 左边缘 → 同上（无 sidebar 手柄，符合预期）
4. ✅ 拖到 50px → 内容裁切但手柄可见（accent 色 hover），能重新拉大
5. ✅ 松开后关闭终端窗口重开 → 宽度恢复
6. ✅ 缩小 Tauri 窗口到最小 560px，重开 → 宽度 clamp 到合法范围
7. ✅ 多窗口：A 窗口拖动改宽度，B 窗口仍是旧值（可接受，spec 已声明）

- [x] **Step 2: 更新 architecture.md**

Modify `docs/architecture.md`，在终端相关章节补充 panel resize 能力（找到文件树/侧栏描述处，加一句「panel 宽度可拖拽调整，全局 localStorage 记忆」）。

- [x] **Step 3: 更新 research 功能差距表**

Modify `docs/research/2026-07-30-embedded-terminal-agent-analysis.md`，「octopus 独有功能」表加一行：
```
| **panel 可调宽度** | sidebar + file-tree 拖拽边缘改宽度，全局 localStorage 记忆（min=50，max 由终端最小宽推导）。Terax 固定宽度 |
```

- [x] **Step 4: Commit**

```bash
git add docs/architecture.md docs/research/2026-07-30-embedded-terminal-agent-analysis.md
git commit -m "docs(sync): panel resize 同步 architecture + research"
```

- [x] **Step 5: Review plan（强制——回看偏差）**

实现完成后回到本 plan，把实际偏差（如 clampTo 是否真需要二次收敛、contentRef 在两布局分支是否都正确取到 rect）回写到对应 Task 的注释里。**plan 是「实施记录」而非「一次性待办」**。

---

## Self-Review 记录

**Spec 覆盖检查**：
- ✅ 两 panel 拖拽（sidebar + file-tree）→ Task 4 Step 4/5
- ✅ 全局 localStorage 一份 → Task 2（key 定义 `"octopus-terminal-sidebar-width"` / `"octopus-terminal-file-tree-width"`）
- ✅ min=50 统一 → Global Constraints + Task 1 测试（`PANEL_MIN`）
- ✅ max 由 terminalMin=320 推导 → Task 1 clampPanelWidth（`TERMINAL_MIN`）
- ✅ 实时 refit → 复用现有 ResizeObserver（spec 声明零额外代码，Task 4 无需手动接 fit）
- ✅ 启动 clamp → Task 4 Step 3（`clampTo` 在 Task 2 定义）
- ✅ 窗口 resize 不主动 clamp → Task 2 hook 不监听 resize（`clampTo` 仅启动 useEffect 调）
- ✅ 持久化仅 pointerup 写 → Task 2 `endDrag`
- ✅ clampPanelWidth 纯函数 5+1 场景 → Task 1（加了 NaN 场景，共 6 个 it）
- ✅ 对侧 panel 判定（sidebar 模式才计入 sidebar 宽）→ Task 4 Step 3/5（`layout === "sidebar" ? sidebarWidth : 0`）

**类型一致性**：
- `clampPanelWidth(raw, min, containerWidth, otherSideWidth, terminalMin)` —— Task 1/2 全部一致
- `usePanelWidth(storageKey, defaultWidth)` → `{ width, startDrag, updateFromPointer, endDrag, clampTo }` —— Task 2 定义、Task 4 调用一致（含 clampTo）
- `PanelResizer({ side, onStart, onMove, onEnd })` —— Task 3 定义、Task 4 通过 FileTreePanel/aside 渲染一致
- `FileTreePanel({ cwd, expanded, onToggle, width?, onResizerStart?, onResizerMove?, onResizerEnd? })` —— Task 4 Step 1 定义、Step 5 调用一致

**关键设计决策**（非占位符）：
- **resizer 放 FileTreePanel 内部**（而非外层 wrapper）——因为收缩态（`.file-tree-collapsed` 24px 小条）不需要 resizer，FileTreePanel 自己用 `expanded` 控制何时渲染。sidebar 无收缩态，resizer 直接放 aside 内。
- **clampTo 用 setWidth 函数式更新**（`setWidth((prev) => clamp(...))`）——避免闭包读到 stale width，且无需把 width 加进 useCallback 依赖。
- **启动 clamp 二次收敛**——sidebar clamp 后宽度变小会影响 fileTree 的 otherSide，再 clamp 一次 fileTree。仅在 sidebar 模式 + fileTree 展开时需要。
- **contentRef 两布局分支都要绑**——sidebar 模式（Step 4）和 tabs 模式（Step 5）的 `.terminal-content` 都加 `ref={contentRef}`，否则切布局后 ref 失效。

## 实施记录（Review plan 回写，2026-07-31）

实际实现与 plan 的偏差，已在 subagent-driven-development 流程中修复：

1. **启动 clamp stale-state（Task 4，commit `f8e0dd4b`）**
   - plan 原写的「二次收敛」`fileTreeWidthCtrl.clampTo(rect.width, sidebarWidthCtrl.width)` 读的是 **pre-clamp 的 stale closure 值**（React setState 异步），二次收敛实际是 no-op。
   - 修复：用纯函数 `clampPanelWidth` 同步算出 `sidebarAfter`，传给 fileTree 的 clampTo。

2. **几何 reference 元素错误（Task 4，commit `2b944caa`，最终全分支 review 发现）**
   - plan 原写 `contentRef` 绑在 `.terminal-content`。但 sidebar 布局下 `.terminal-content` 在 sidebar **右侧**，`containerRect.left` 是 sidebar 的移动右边缘 → sidebar 拖拽 raw 自我参照、宽度抖动；fileTree 的 panelMax 把 sidebar 减两次。
   - 修复：`contentRef` 上移到 `.terminal-window`（包含 sidebar 的行容器）。sidebar/fileTree 的几何 + panelMax 全部正确。

3. **PanelResizer 卸载清理（Task 3，commit `2b944caa`）**
   - plan 原写的 PanelResizer 无 unmount cleanup。若组件在拖拽中卸载（切布局/收 fileTree/HMR），`terminal-resizing` class 泄漏到 `document.documentElement` → 全局 `cursor: col-resize` + 不可选中。
   - 修复：加 `useEffect(() => () => classList.remove("terminal-resizing"), [])`，对齐 CompactEditor 先例。

**最终验证**：tsc 0 error · vitest 428/428 · desktop rust test 488 passed · e2e 通过（sidebar/fileTree 拖拽实时 refit + 持久化 + 启动 clamp）。

