# 终端点击定位命令行光标 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 终端命令行输入态，鼠标点击直接把光标移到点击位置（无需 Alt，精确）。

**Architecture:** canvas click handler（坐标换算 + 门控 + 转义序列）+ `useTerminalSession` 暴露 `inCommand`。三个纯函数（坐标换算/门控/转义序列）可 TDD，接线靠 e2e。

**Tech Stack:** TypeScript + React + vitest，零新依赖。

**Spec:** `docs/superpowers/specs/2026-08-01-terminal-click-cursor-design.md`

## Global Constraints

- **门控三条件全满足才响应**：`inCommand=false` + `buffer.type='normal'` + 点击行==`cursorY`
- **仅当前光标行响应**（clickRow === cursorY），跨行/多行不处理
- **click vs drag 区分**：mousedown 后移动 <4px 算 click，否则是文本选择放行
- **转义序列走 session.write**（非 paste——光标移动不是用户输入语义）
- **delta=0 不动**
- **不干扰 TUI**：alternate screen / mouseTracking 状态放行 xterm 原生
- **坐标换算基准元素**：`.xterm-screen`（xterm.css:105，WebGL renderer 在其下渲染 canvas）

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/frontend/src/pages/Terminal/clickCursor.ts` | 纯函数：pixelToCol / shouldMoveCursor / buildCursorMoveSequence | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/clickCursor.test.ts` | 三个纯函数测试 | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts` | 暴露 `inCommand`（shellState 提到外层 ref） | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx` | canvas click handler（坐标换算 + 门控 + 转义序列） | 修改 |

---

### Task 1: clickCursor 纯函数（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/clickCursor.ts`
- Test: `crates/desktop/frontend/src/pages/Terminal/clickCursor.test.ts`

**Interfaces:**
- Produces: `pixelToCol` / `shouldMoveCursor` / `buildCursorMoveSequence` —— Task 3 的 click handler 调用。

- [ ] **Step 1: Write the failing test**

Create `crates/desktop/frontend/src/pages/Terminal/clickCursor.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { pixelToCol, shouldMoveCursor, buildCursorMoveSequence } from "./clickCursor";

describe("pixelToCol", () => {
  it("像素坐标 → 列号（整除）", () => {
    // rect.left=100, width=800, cols=80 → cellWidth=10
    // clientX=145 → (145-100)/10 = 4.5 → floor = 4
    expect(pixelToCol(145, 100, 800, 80)).toBe(4);
  });

  it("左边缘 → 第 0 列", () => {
    expect(pixelToCol(100, 100, 800, 80)).toBe(0);
  });

  it("右边缘 → 最后一列", () => {
    // clientX=899 → (899-100)/10 = 79.9 → floor = 79
    expect(pixelToCol(899, 100, 800, 80)).toBe(79);
  });

  it("超出右边缘 → clamp 到最后一列", () => {
    expect(pixelToCol(1000, 100, 800, 80)).toBe(79);
  });

  it("超出左边缘 → clamp 到 0", () => {
    expect(pixelToCol(50, 100, 800, 80)).toBe(0);
  });
});

describe("shouldMoveCursor", () => {
  const base = { inCommand: false, bufferType: "normal" as const, clickRow: 5, cursorY: 5 };
  it("全满足 → true", () => {
    expect(shouldMoveCursor(base)).toBe(true);
  });
  it("inCommand=true → false（命令执行中）", () => {
    expect(shouldMoveCursor({ ...base, inCommand: true })).toBe(false);
  });
  it("bufferType=alternate → false（TUI 全屏）", () => {
    expect(shouldMoveCursor({ ...base, bufferType: "alternate" })).toBe(false);
  });
  it("clickRow != cursorY → false（非当前行）", () => {
    expect(shouldMoveCursor({ ...base, clickRow: 3, cursorY: 5 })).toBe(false);
  });
});

describe("buildCursorMoveSequence", () => {
  it("delta>0 → CUF 右移", () => {
    expect(buildCursorMoveSequence(5)).toBe("\x1b[5C");
  });
  it("delta<0 → CUB 左移", () => {
    expect(buildCursorMoveSequence(-3)).toBe("\x1b[3D");
  });
  it("delta=0 → 空字符串（不动）", () => {
    expect(buildCursorMoveSequence(0)).toBe("");
  });
  it("delta=1 → 单步右移", () => {
    expect(buildCursorMoveSequence(1)).toBe("\x1b[1C");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/clickCursor.test.ts`
Expected: FAIL，`Failed to resolve import "./clickCursor"`。

- [ ] **Step 3: Write minimal implementation**

Create `crates/desktop/frontend/src/pages/Terminal/clickCursor.ts`:

```typescript
/**
 * 终端点击定位光标的纯函数（spec 2026-08-01-terminal-click-cursor）。
 *
 * 像素坐标 → 列号 → 门控 → 偏移转义序列。三个函数严格分工，可独立测。
 */

/** 像素 clientX → xterm 列号（clamp 到 [0, cols-1]）。 */
export function pixelToCol(
  clientX: number,
  rectLeft: number,
  rectWidth: number,
  cols: number,
): number {
  const cellWidth = rectWidth / cols;
  const col = Math.floor((clientX - rectLeft) / cellWidth);
  return Math.max(0, Math.min(cols - 1, col));
}

/** 门控：是否应该响应点击移动光标（三条件全满足）。 */
export function shouldMoveCursor(state: {
  inCommand: boolean;
  bufferType: "normal" | "alternate";
  clickRow: number;
  cursorY: number;
}): boolean {
  return (
    !state.inCommand &&
    state.bufferType === "normal" &&
    state.clickRow === state.cursorY
  );
}

/** 偏移量 → ANSI 转义序列（CUF 右移 / CUB 左移）。delta=0 返回空字符串。 */
export function buildCursorMoveSequence(delta: number): string {
  if (delta === 0) return "";
  if (delta > 0) return `\x1b[${delta}C`;
  return `\x1b[${-delta}D`;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/clickCursor.test.ts`
Expected: PASS（12 个 it 全过）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/clickCursor.ts crates/desktop/frontend/src/pages/Terminal/clickCursor.test.ts
git commit -m "feat(terminal): clickCursor 纯函数——坐标换算/门控/转义序列（点击定位 TDD 入口）"
```

---

### Task 2: useTerminalSession 暴露 inCommand

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts`

**Interfaces:**
- Produces: session 返回值加 `inCommand: boolean`（Task 3 的 click handler 读 `sessionRef.current.inCommand`）。

**Context:**
- `shellState` 当前定义在 `term.open().then()` 回调内（line 270），是闭包局部变量，return（line 343）访问不到。
- 改造：用 ref 持有 shellState（`shellStateRef`），在 OSC 133 回调里更新 ref.current，return 里读 ref.current.inCommand。

- [ ] **Step 1: 把 shellState 提到外层 ref**

修改 `useTerminalSession.ts`。在 `const [trackedCwd, setTrackedCwd] = useState(...)`（line 134）附近加：

```typescript
  // OSC 133 shell 集成状态——持有 inCommand（命令行输入态），供点击定位光标门控读
  const shellStateRef = useRef(createShellIntegrationState());
```

在 `term.open().then()` 回调内（原 `const shellState = createShellIntegrationState();` line 270），改为用 ref：

```typescript
        // OSC 7 cwd 追踪 + OSC 133 prompt tracker（安全过滤）
        const shellState = shellStateRef.current;
        registerPromptTracker(term, shellState);
        registerCwdHandler(term, (c) => setTrackedCwd(c), shellState);
```

- [ ] **Step 2: session 返回值加 inCommand**

在 return 对象（line 343）加：

```typescript
  return {
    write: (data: string) => { ... },
    focus: () => { ... },
    ptyId,
    searchAddon: searchAddonRef.current,
    cwd: trackedCwd,
    inCommand: shellStateRef.current.inCommand,  // 新增：OSC 133 命令行输入态
    hasSelection: () => ...,
    // ... 其余不变
  };
```

同时 `PtySession` 类型（如 line 48 附近的 type/interface）加 `inCommand: boolean`。先 grep 确认类型定义位置：

```bash
rg -n "type PtySession|interface PtySession|cwd: string" crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts | head
```

- [ ] **Step 3: tsc + vitest**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vitest run`
Expected: tsc 0 error；vitest 全过（不回归）。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts
git commit -m "feat(terminal): useTerminalSession 暴露 inCommand——OSC 133 命令行输入态

shellState 提到外层 ref（原在 term.open 闭包内），session 返回值加 inCommand
供点击定位光标门控读取。"
```

---

### Task 3: TerminalPane canvas click handler

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx`

**Interfaces:**
- Consumes: `pixelToCol` / `shouldMoveCursor` / `buildCursorMoveSequence` from Task 1，`session.inCommand` from Task 2。

**Context:**
- canvas div（`.terminal-pane-canvas`，containerRef）已有 `onContextMenu`。
- 文件拖拽的 `document mouseup`（line 160）用 `takeDragPath()` 区分——拖拽中不触发光标移动（takeDragPath 返回非 null）。
- click vs drag 区分：mousedown 记录起点，mouseup 判移动 <4px。

- [ ] **Step 1: import clickCursor 纯函数**

TerminalPane 顶部 import 加：

```typescript
import { pixelToCol, shouldMoveCursor, buildCursorMoveSequence } from "./clickCursor";
```

- [ ] **Step 2: 加 mousedown 起点 ref + canvas click handler**

在组件内（sessionRef 定义附近）加 mousedown 起点 ref：

```typescript
  // click vs drag 区分：mousedown 记录起点，移动 <4px 算 click（触发光标移动）
  const mouseDownPos = useRef<{ x: number; y: number } | null>(null);
```

给 `.terminal-pane-canvas` div 加 `onMouseDown` + `onClick`：

```typescript
      <div
        ref={containerRef}
        className="terminal-pane-canvas"
        onContextMenu={openContextMenu}
        onMouseDown={(e) => {
          if (e.button === 0) mouseDownPos.current = { x: e.clientX, y: e.clientY };
        }}
        onClick={(e) => {
          // click vs drag 区分：移动 <4px 算 click
          const down = mouseDownPos.current;
          mouseDownPos.current = null;
          if (!down) return;
          const moved = Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y);
          if (moved > 4) return; // 拖拽选择，放行
          // 文件拖拽中不触发光标移动
          // （document mouseup 的 takeDragPath 已处理——这里不冲突，takeDragPath 在 document 级先触发）

          const s = sessionRef.current;
          const term = termRef（需确认怎么拿 term 实例——见下文）;
          if (!term) return;

          // 坐标换算：.xterm-screen getBoundingClientRect
          const screen = containerRef.current?.querySelector(".xterm-screen");
          if (!screen) return;
          const rect = (screen as HTMLElement).getBoundingClientRect();
          const clickCol = pixelToCol(e.clientX, rect.left, rect.width, term.cols);
          const clickRow = Math.floor((e.clientY - rect.top) / (rect.height / term.rows));

          // 门控
          if (!shouldMoveCursor({
            inCommand: s.inCommand,
            bufferType: term.buffer.active.type,
            clickRow,
            cursorY: term.buffer.active.cursorY,
          })) return;

          // 偏移 + 转义序列
          const delta = clickCol - term.buffer.active.cursorX;
          const seq = buildCursorMoveSequence(delta);
          if (seq) {
            s.write(seq);
            s.focus();
          }
        }}
      />
```

**⚠️ term 实例访问**：click handler 需要 `term.cols` / `term.rows` / `term.buffer.active`。但 `useTerminalSession` 当前不暴露 term 实例（只暴露封装方法）。两个选择：
- (a) `useTerminalSession` 暴露 term 实例（或需要的属性 cols/rows/buffer）
- (b) click handler 用 `containerRef.current?.querySelector(".xterm-screen")` 拿尺寸 + session 已有的 cols/rows/buffer 访问器

实现时核实：session 是否已暴露 cols/rows？若无，加到返回值（或暴露 term）。**Task 3 Step 2b 专门处理这个**。

- [ ] **Step 2b: 给 TerminalSession 加 cols/cursorX/cursorY/bufferType getter（已确认需要）**

已确认 `TerminalSession`（useTerminalSession.ts:48-64）**未暴露** cols/cursorX/cursorY/bufferType。采用方案 A（只暴露需要的 getter，不暴露整个 term）：

1. `TerminalSession` 类型加字段：
```typescript
export type TerminalSession = {
  // ... 现有字段
  /** 终端列数（点击定位坐标换算用）。 */
  cols: number;
  /** 终端行数（点击行换算用）。 */
  rows: number;
  /** 当前光标列（点击偏移计算用）。 */
  cursorX: number;
  /** 当前光标行（门控：只当前行响应）。 */
  cursorY: number;
  /** buffer 类型（门控：非 alternate 才响应）。 */
  bufferType: "normal" | "alternate";
};
```

2. return 对象加（读 termRef.current）：
```typescript
  return {
    // ... 现有
    cols: termRef.current?.cols ?? 80,
    rows: termRef.current?.rows ?? 24,
    cursorX: termRef.current?.buffer.active.cursorX ?? 0,
    cursorY: termRef.current?.buffer.active.cursorY ?? 0,
    bufferType: termRef.current?.buffer.active.type ?? "normal",
  };
```

3. Step 2 的 click handler 用 `s.cols` / `s.cursorX` / `s.cursorY` / `s.bufferType`（不需直接访问 term 实例）。

- [ ] **Step 3: tsc + vitest**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vitest run`
Expected: tsc 0 error；vitest 全过（原 + Task 1 的 12 = 新总数）。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts
git commit -m "feat(terminal): 点击定位命令行光标——canvas click handler

命令行输入态鼠标点击直接移动光标（OSC 133 门控 + 偏移转义序列）。
坐标换算用 .xterm-screen getBoundingClientRect，click vs drag 用 <4px 阈值。"
```

---

### Task 4: e2e 验证 + 文档同步

**Files:**
- 无代码改动，验证 + 文档

- [ ] **Step 1: 构建 + 手动 e2e**

Run:
```bash
./scripts/build-macos-dmg.sh --no-lto --open
```

手动测试清单：
1. ✅ 输入命令 `echo hello world test`，点击 `world` 中间 → 光标移到 world 处
2. ✅ 点击命令行左侧 → 光标左移；右侧 → 右移
3. ✅ 点击当前位置 → 不动
4. ✅ 拖拽选择文本（mousedown 移动 >4px）→ 不触发光标移动，正常选中文本
5. ✅ vim 内点击 → 不触发光标移动（alternate screen 门控）
6. ✅ 命令执行中（如 `sleep 5`）点击 → 不触发（inCommand 门控）
7. ✅ 点击非当前行（历史输出）→ 不触发（cursorY 门控）

- [ ] **Step 2: 更新 architecture.md**

Modify `docs/architecture.md` 终端章节，补「点击定位命令行光标（OSC 133 门控）」。

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(sync): 点击定位命令行光标——architecture 同步"
```

- [ ] **Step 4: Review plan（强制——回看偏差）**

实现完成后回到本 plan，把实际偏差（如 term 实例访问方案、click vs drag 阈值实测）回写。

---

## Self-Review 记录

**Spec 覆盖检查**：
- ✅ 门控三条件 → Task 1 shouldMoveCursor + Task 3 click handler
- ✅ 坐标换算 → Task 1 pixelToCol + Task 3 .xterm-screen
- ✅ 偏移转义序列 → Task 1 buildCursorMoveSequence + Task 3 write
- ✅ click vs drag → Task 3 mouseDownPos ref + 移动阈值
- ✅ inCommand 暴露 → Task 2 shellStateRef
- ✅ write 非 paste → Task 3 s.write(seq)

**类型一致性**：
- `pixelToCol(clientX, rectLeft, rectWidth, cols)` —— Task 1 定义、Task 3 调用一致
- `shouldMoveCursor({ inCommand, bufferType, clickRow, cursorY })` —— Task 1/3 一致
- `buildCursorMoveSequence(delta)` —— Task 1/3 一致

**已知实现注意**（非占位符）：
- ~~Task 3 Step 2b 的 term 实例访问：session 可能不暴露 cols/rows/buffer~~ —— 已确认 `TerminalSession`（line 48-64）未暴露，采用方案 A 加 cols/rows/cursorX/cursorY/bufferType getter（Task 3 Step 2b 已定稿）。
- Task 2 的 shellStateRef：`createShellIntegrationState()` 在外层 ref 初始化，`.then()` 回调内用 `shellStateRef.current`。注意 `registerPromptTracker` 接收的是 ref.current（对象引用），OSC 133 更新 inCommand 时直接改 ref.current.inCommand——return 读 `shellStateRef.current.inCommand` 拿到最新。
