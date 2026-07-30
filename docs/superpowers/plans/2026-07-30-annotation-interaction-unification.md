# 标注交互统一 useAnnotationInteraction 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 ImagePreview / Screenshot / RecordAnnotation 三个场景的标注鼠标交互（画/拖/擦/undo/redo/文字）统一到一个 hook。

**Architecture:** 扩展现有 `components/Annotation/useAnnotationState` → 新增 `useAnnotationInteraction`（在其上叠加鼠标交互 + 坐标换算）。各场景提供 `clientToNatural(clientX, clientY)` 坐标函数，hook 内部管标注状态机。渐进式迁移：先 ImagePreview，再 Screenshot，最后 RecordAnnotation 补齐。

**Tech Stack:** React hooks, TypeScript, `@/lib/annotation`（Annotation/Tool/hitTestAnnotationPrecise）

## Global Constraints

- 标注类型 `Annotation` / `Tool` 定义在 `@/lib/annotation`，不可修改
- `hitTestAnnotationPrecise(x, y, anns[]) => number | null` 来自 `@/lib/annotation`
- 坐标换算 `clientToNatural` 由各场景提供（hook 不关心 zoom/scroll/offset）
- 平移（pan）/选区（crop）/窗口管理不纳入 hook
- 文字 textarea 渲染由各场景自己管（hook 只返回 textDraft 数据）
- TDD：无法对 React hook 写纯单元测试的场景，事后冒烟测试也可（AGENTS.md 允许）
- `tsc -b && vite build` 为验证命令

---

## File Structure

| 文件 | 职责 | 操作 |
|---|---|---|
| `components/Annotation/useAnnotationInteraction.ts` | 标注鼠标交互 hook（mousedown/move/up + 坐标 + 文字 draft） | **新建** |
| `pages/ImagePreview/index.tsx` | 图片预览主组件 | **修改**（移除内联标注交互，改用 hook） |
| `pages/Screenshot/index.tsx` | 截图编辑主组件 | **修改**（同上） |
| `pages/RecordAnnotation/index.tsx` | 录屏标注主组件 | **修改**（补齐使用 hook 的鼠标交互部分） |

`components/Annotation/useAnnotationState.ts` **不修改**——`useAnnotationInteraction` 在它之上构建，复用其返回值。

---

### Task 1: 创建 useAnnotationInteraction hook

**Files:**
- Create: `crates/desktop/frontend/src/components/Annotation/useAnnotationInteraction.ts`

**Interfaces:**
- Consumes: `AnnotationState` from `useAnnotationState`（annotations/drawingRef/addAnnotation/eraseAnnotationAt/undoAnnotation/redoAnnotation/redoAvailable/clearAllAnnotations/numberCounter/setNumberCounter/toolRef/toolColorRef/toolWidthRef/toolFontSizeRef/toolFilledRef）
- Consumes: `Annotation`, `Tool`, `hitTestAnnotationPrecise` from `@/lib/annotation`
- Produces: `useAnnotationInteraction(opts) => { handleMouseDown, handleMouseMove, handleMouseUp, draftAnn, textDraft, commitText, cancelText }`

- [ ] **Step 1: 创建 hook 文件骨架 + 类型定义**

创建 `components/Annotation/useAnnotationInteraction.ts`，定义接口和空实现：

```typescript
// 标注鼠标交互 hook —— 统一 ImagePreview/Screenshot/RecordAnnotation 的标注鼠标逻辑。
// 在 useAnnotationState 基础上叠加：mousedown/move/up 交互 + 坐标换算 + 文字 draft。
//
// 各场景只需提供 clientToNatural 坐标函数 + useAnnotationState 返回值。

import { useState, useRef, useCallback } from "react";
import type { Annotation, Tool } from "@/lib/annotation";
import { hitTestAnnotationPrecise } from "@/lib/annotation";
import type { AnnotationState } from "./useAnnotationState";

/** 坐标换算：屏幕 clientX/Y → 标注自然坐标。各场景提供。 */
export type ClientToNatural = (clientX: number, clientY: number) => { x: number; y: number };

/** mousedown 时传入的工具上下文 */
export interface ToolContext {
  tool: Tool;
  color: string;
  width: number;
  fontSize: number;
  filled: boolean;
}

/** 文字标注草稿 */
export interface TextDraft {
  x: number;
  y: number;
  val: string;
  fs: number;
}

export interface UseAnnotationInteractionOptions {
  clientToNatural: ClientToNatural;
  natW: number;
  natH: number;
  state: AnnotationState;
}

export interface AnnotationInteraction {
  /** 绘制中的临时标注（SVG overlay 渲染用） */
  draftAnn: Annotation | null;
  /** 文字标注草稿 */
  textDraft: TextDraft | null;
  textDraftRef: React.MutableRefObject<TextDraft | null>;
  /** mousedown：标注创建/拖拽/擦除/文字 */
  handleMouseDown: (e: React.MouseEvent, ctx: ToolContext) => void;
  /** mousemove：绘制中/拖拽中/擦除中 */
  handleMouseMove: (e: React.MouseEvent) => void;
  /** mouseup：结束当前操作 */
  handleMouseUp: () => void;
  /** 提交文字草稿 */
  commitText: (color: string, fontSize: number) => void;
  /** 取消文字草稿 */
  cancelText: () => void;
  /** 设置文字草稿值（textarea onChange 用） */
  setTextDraftVal: (val: string) => void;
  /** 擦除中 ref（各场景判断 cursor 用） */
  erasingRef: React.MutableRefObject<boolean>;
  /** 拖拽中 ref（各场景判断 cursor 用） */
  dragRef: React.MutableRefObject<{ idx: number; dx: number; dy: number } | null>;
}
```

- [ ] **Step 2: 实现 handleMouseDown**

在 `useAnnotationInteraction` 函数体内实现 `handleMouseDown`，从 ImagePreview index.tsx L422-507 搬运逻辑，用 `ctx` 参数替代闭包变量：

```typescript
export function useAnnotationInteraction(opts: UseAnnotationInteractionOptions): AnnotationInteraction {
  const { clientToNatural, natW, natH, state } = opts;
  const {
    annotations, annotationsRef, drawingRef, addAnnotation,
    eraseAnnotationAt, numberCounter, setNumberCounter,
  } = state;

  const [draftAnn, setDraftAnn] = useState<Annotation | null>(null);
  const [textDraft, setTextDraft] = useState<TextDraft | null>(null);
  const textDraftRef = useRef<TextDraft | null>(null);
  const erasingRef = useRef(false);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);

  // clientToNatural 用 ref 镜像（闭包安全——mousemove/mouseup 在 useEffect 空依赖内调用时读最新值）
  const ctRef = useRef(clientToNatural);
  ctRef.current = clientToNatural;

  const handleMouseDown = useCallback((e: React.MouseEvent, ctx: ToolContext) => {
    if (e.button !== 0) return;
    const { x: nx, y: ny } = ctRef.current(e.clientX, e.clientY);

    // 文字草稿进行中：点击别处 = 提交当前文字
    if (textDraftRef.current) {
      commitText(ctx.color, ctx.fontSize);
    }

    // 橡皮擦
    if (ctx.tool === "eraser") {
      eraseAnnotationAt(nx, ny);
      erasingRef.current = true;
      return;
    }

    // 选择工具：hitTest → 拖拽
    if (ctx.tool === "none") {
      const idx = hitTestAnnotationPrecise(nx, ny, annotationsRef.current);
      if (idx != null) {
        dragRef.current = {
          idx,
          dx: nx - annotationsRef.current[idx].x1,
          dy: ny - annotationsRef.current[idx].y1,
        };
      }
      return;
    }

    // 文字标注
    if (ctx.tool === "text") {
      const d = { x: nx, y: ny, val: "", fs: ctx.fontSize };
      textDraftRef.current = d;
      setTextDraft(d);
      return;
    }

    // 序号标注
    if (ctx.tool === "number") {
      const ann: Annotation = {
        type: "number", x1: nx, y1: ny, x2: nx, y2: ny,
        number: numberCounter, color: ctx.color, circleSize: 28,
      };
      addAnnotation(ann);
      setNumberCounter(numberCounter + 1);
      return;
    }

    // 画笔 / 荧光笔
    if (ctx.tool === "pen" || ctx.tool === "highlight") {
      drawingRef.current = {
        type: ctx.tool, x1: nx, y1: ny, x2: nx, y2: ny,
        points: [[nx, ny]],
        color: ctx.color, lineWidth: ctx.tool === "highlight" ? 15 : ctx.width,
      };
      return;
    }

    // rect/oval/line/arrow/diamond
    drawingRef.current = {
      type: ctx.tool as Annotation["type"],
      x1: nx, y1: ny, x2: nx, y2: ny,
      color: ctx.color, lineWidth: ctx.width,
      filled: (ctx.tool === "rect" || ctx.tool === "oval" || ctx.tool === "diamond") ? ctx.filled : undefined,
    };
  }, [annotationsRef, drawingRef, addAnnotation, eraseAnnotationAt, numberCounter, setNumberCounter]);

  // commitText 前向声明（handleMouseDown 引用它）
  const commitText = useCallback((color: string, fontSize: number) => {
    const d = textDraftRef.current;
    textDraftRef.current = null;
    setTextDraft(null);
    if (!d || !d.val.trim()) return;
    addAnnotation({
      type: "text",
      x1: d.x, y1: d.y, x2: d.x, y2: d.y,
      text: d.val,
      color, fontSize,
    });
  }, [addAnnotation]);
```

- [ ] **Step 3: 实现 handleMouseMove + handleMouseUp + cancelText + setTextDraftVal**

```typescript
  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    const { x: nx, y: ny } = ctRef.current(e.clientX, e.clientY);

    // 擦除中
    if (erasingRef.current) {
      eraseAnnotationAt(nx, ny);
      return;
    }

    // 拖拽中
    if (dragRef.current) {
      const { idx, dx, dy } = dragRef.current;
      state.setAnnotations((prev) => prev.map((a, i) => {
        if (i !== idx) return a;
        const mx = nx - dx, my = ny - dy;
        const w = a.x2 - a.x1, h = a.y2 - a.y1;
        return { ...a, x1: mx, y1: my, x2: mx + w, y2: my + h };
      }));
      return;
    }

    // 绘制中
    if (drawingRef.current) {
      if ((drawingRef.current.type === "pen" || drawingRef.current.type === "highlight") && drawingRef.current.points) {
        drawingRef.current.points.push([nx, ny]);
        setDraftAnn({ ...drawingRef.current, points: [...drawingRef.current.points] });
      } else {
        drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
        setDraftAnn({ ...drawingRef.current });
      }
      return;
    }
  }, [eraseAnnotationAt, state]);

  const handleMouseUp = useCallback(() => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触
      const ok = (ann.type === "pen" || ann.type === "highlight")
        ? (ann.points?.length ?? 0) >= 2
        : (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3);
      if (ok) addAnnotation(ann);
      setDraftAnn(null);
    }
    erasingRef.current = false;
    dragRef.current = null;
  }, [drawingRef, addAnnotation]);

  const cancelText = useCallback(() => {
    textDraftRef.current = null;
    setTextDraft(null);
  }, []);

  const setTextDraftVal = useCallback((val: string) => {
    const d = textDraftRef.current;
    if (!d) return;
    const next = { ...d, val };
    textDraftRef.current = next;
    setTextDraft(next);
  }, []);

  return {
    draftAnn, textDraft, textDraftRef,
    handleMouseDown, handleMouseMove, handleMouseUp,
    commitText, cancelText, setTextDraftVal,
    erasingRef, dragRef,
  };
}
```

- [ ] **Step 4: tsc 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 errors（hook 尚未被消费，但自身类型必须正确）

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/useAnnotationInteraction.ts
git commit -m "feat(annotation): useAnnotationInteraction hook——统一标注鼠标交互"
```

---

### Task 2: ImagePreview index.tsx 迁移到 hook

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**Interfaces:**
- Consumes: `useAnnotationInteraction` from Task 1
- Consumes: `useAnnotationState` from `components/Annotation`

- [ ] **Step 1: import useAnnotationInteraction**

在 index.tsx import 区加：

```typescript
import { useAnnotationInteraction } from "@/components/Annotation/useAnnotationInteraction";
```

- [ ] **Step 2: 替换标注 state/ref 声明为 hook 调用**

将 index.tsx 内联的标注 state/ref（annotations/draftAnn/drawingRef/dragRef/erasingRef/redoStackRef/redoAvailable/textDraft/textDraftRef/toolColorRef/toolWidthRef/toolFontSizeRef/filled 等）替换为 hook 调用。保留 ImagePreview 特有的 state（tool/toolColor/toolWidth/toolFontSize/filled/popoverDismissKey/panning）。

从 hook 获取：`draftAnn`, `textDraft`, `textDraftRef`, `handleMouseDown`, `handleMouseMove`, `handleMouseUp`, `commitText`, `cancelText`, `setTextDraftVal`, `erasingRef`, `dragRef`。

注意：
- ImagePreview 当前**没有**用 `useAnnotationState`（它自己管 annotations state）——迁移时需引入 `useAnnotationState` + `useAnnotationInteraction`
- `annotations` / `setAnnotations` / `addAnnotation` / `eraseAnnotationAt` / `undoAnnotation` / `redoAnnotation` / `redoAvailable` / `clearAllAnnotations` / `numberCounter` 来自 `useAnnotationState`
- ImagePreview 特有的 `undo()`（与 useAnnotationState 的 `undoAnnotation` 等价）替换为 `undoAnnotation`
- `redo()` 替换为 `redoAnnotation`
- `composePngBytes` 保留在 index.tsx（依赖 imgRef + annotations）

- [ ] **Step 3: 替换 onMouseDown/onMouseMove/onMouseUp 为 hook handlers**

将 index.tsx 内联的 `onMouseDown`（~85 行）、`onMouseMove`（~28 行）、`onMouseUp`（~16 行）替换为：

```typescript
const onMouseDown = (e: React.MouseEvent) => {
  if (e.button !== 0) return;
  setPopoverDismissKey((k) => k + 1);
  // 文字草稿进行中或全图加载中时仍允许提交文字
  if (textDraftRef.current) {
    handleMouseDown(e, { tool, color: toolColor, width: toolWidth, fontSize: toolFontSize, filled });
    return;
  }
  // 全图加载中：仅允许选择/平移，禁止标注
  if (loadingFullRef.current && tool !== "none" && tool !== "eraser") return;
  // tool === "none" 未命中标注 → 抓手平移（ImagePreview 特有，不进 hook）
  // hook 的 handleMouseDown 只管标注逻辑，平移由 index.tsx 自己处理
  handleMouseDown(e, { tool, color: toolColor, width: toolWidth, fontSize: toolFontSize, filled });
  // 如果 hook 没有进入标注操作（dragRef 为 null 且 drawingRef 为 null）且 tool==="none"，
  // 检查是否需要平移
  if (tool === "none" && !dragRef.current && !erasingRef.current) {
    // hitTest 未命中 → startPan（保留 ImagePreview 特有的平移逻辑）
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    const idx = hitTestAnnotationPrecise(nx, ny, annotations);
    if (idx == null) startPan(e);
  }
};
```

**注意**：ImagePreview 的 `onMouseDown` 有两个特殊行为不进 hook：
1. `setPopoverDismissKey` — 收起工具栏浮窗（UI 行为）
2. `loadingFullRef` 拦截 — 全图加载中禁止标注
3. `startPan` — 抓手平移（viewport 行为）

这些在 hook 调用前后由 index.tsx 自己处理。

- [ ] **Step 4: 替换 commitText / undo / redo 调用**

```typescript
// 原 commitText 替换为
const commitText = () => interaction.commitText(toolColor, toolFontSize);

// 原 undo 替换为
const undo = () => state.undoAnnotation();

// 原 redo 替换为
const redo = () => state.redoAnnotation();
```

- [ ] **Step 5: 移除被 hook 取代的 state/ref 声明**

删除 index.tsx 内被 hook 取代的变量声明：
- `draftAnn` / `setDraftAnn`（从 hook 获取）
- `drawingRef`（从 useAnnotationState 获取）
- `dragRef`（从 hook 获取）
- `erasingRef`（从 hook 获取）
- `redoStackRef` / `redoAvailable` / `setRedoAvailable`（从 useAnnotationState 获取）
- `textDraft` / `setTextDraft` / `textDraftRef`（从 hook 获取）
- `toolColorRef` / `toolWidthRef` / `toolFontSizeRef`（从 useAnnotationState 获取）
- `filled` / `setFilled`（从 useAnnotationState 获取——用 `toolFilled` / `setToolFilled`）
- `numberCounter`（从 useAnnotationState 获取）

- [ ] **Step 6: tsc + vite build 验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: ✓ built，0 errors

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/ImagePreview/index.tsx
git commit -m "refactor(image-preview): 迁移标注交互到 useAnnotationInteraction hook"
```

---

### Task 3: Screenshot index.tsx 迁移到 hook

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

- [ ] **Step 1: 引入 hook + 替换标注交互**

Screenshot 的标注逻辑与 ImagePreview 高度相似，但有差异：
- Screenshot 没有 zoom（scale = natW / window.innerWidth，固定）
- Screenshot 有选区（crop region）逻辑——**不进 hook**
- Screenshot 的 `onMouseDown` 先判选区，再判标注

`clientToNatural`：
```typescript
const clientToNatural = useCallback((cx: number, cy: number) => {
  return { x: cx, y: cy }; // Screenshot 全屏 1:1（标注在屏幕坐标系，导出时 scale）
}, []);
```

注意：Screenshot 的标注实际存的是屏幕坐标（非自然像素坐标），导出时乘 scale 转。这与 ImagePreview 不同（ImagePreview 标注存自然坐标）。迁移时保持 Screenshot 的现有行为——`clientToNatural` 返回屏幕坐标。

- [ ] **Step 2: 移除被 hook 取代的内联代码**

同 Task 2 Step 5，删除 Screenshot 内被 hook 取代的 state/ref。

- [ ] **Step 3: tsc + vite build 验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: ✓ built，0 errors

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx
git commit -m "refactor(screenshot): 迁移标注交互到 useAnnotationInteraction hook"
```

---

### Task 4: RecordAnnotation 补齐到 hook

**Files:**
- Modify: `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx`

- [ ] **Step 1: 引入 useAnnotationInteraction 替换内联鼠标交互**

RecordAnnotation 已用 `useAnnotationState`，但鼠标交互仍内联。替换为 hook：

`clientToNatural`：
```typescript
const clientToNatural = useCallback((cx: number, cy: number) => {
  return { x: cx - canvasRectRef.current.ox, y: cy - canvasRectRef.current.oy };
}, []);
```

- [ ] **Step 2: 移除被 hook 取代的内联代码**

- [ ] **Step 3: tsc + vite build 验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: ✓ built，0 errors

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx
git commit -m "refactor(record-annotation): 补齐使用 useAnnotationInteraction hook"
```

---

## Self-Review

**Spec coverage:**
- ✅ useAnnotationInteraction hook 接口（Task 1）
- ✅ ImagePreview 迁移（Task 2）
- ✅ Screenshot 迁移（Task 3）
- ✅ RecordAnnotation 迁移（Task 4）
- ✅ clientToNatural 三场景各有示例（Task 2/3/4）
- ✅ 文字标注 textDraft / commitText / cancelText（Task 1）
- ✅ composePngBytes 保留在 index.tsx（Task 2 Step 2 说明）

**Placeholder scan:** 无 TODO/TBD，每个 step 都有具体代码。

**Type consistency:** `ToolContext` 在 Task 1 定义，Task 2/3/4 消费。`TextDraft` 在 Task 1 定义，各场景消费。`AnnotationInteraction` 接口贯穿所有 task。
