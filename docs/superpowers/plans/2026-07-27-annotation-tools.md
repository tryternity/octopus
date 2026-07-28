# 标注工具扩充实施计划

> **Status: 🔶 大部分完成**（Task 1/2/4/6/7 + Task 3/5 的 highlight/eraser/clearAll 均已实现；**deleteSelected 按钮未接线**见下）。2026-07-29 z-sync 回填 checkbox。
>
> ⚠️ **未完成项**（2 个 step 标 `[ ]`）：Task 3 Step 4 + Task 5 Step 2 的 **`deleteSelected` 按钮在 AnnotationToolbar 和 ImagePreview Toolbar 都未渲染**——底层 `deleteSelectedAnnotation` action、props、i18n 文案、`trash.svg` 图标全齐，但工具栏没接线。当前删除选中标注走键盘 Delete/Backspace（Screenshot/RecordAnnotation），且这两处直接 `setAnnotations.filter` 删除、**绕过 action、未推入 redoStack**（undo 不回来）。待办：补工具栏按钮，或把键盘删除改走 `deleteSelectedAnnotation()`。
>
> **其他偏差**：i18n 实际 key 是 `screenshot.tool.clear` / `imagePreview.clear`（非 plan 写的 `clearAll`/`deleteSelected`，功能等价）；Task 4 方案 A 的 `eraseAnnotationAt` 已实现（useAnnotationState.ts:168-181）。
>
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为截图/录屏/图文编辑器标注系统扩充 4 个工具（荧光笔/橡皮擦/清空/删除选定）。

**Architecture:** 共享层（annotation.ts 加 type/draw + useAnnotationState 加 actions + AnnotationToolbar 加按钮）+ 业务层（Screenshot/RecordAnnotation/ImagePreview 各自接 eraser mousemove + clearAll + deleteSelected）。

**Tech Stack:** TypeScript + React + Canvas（截图/录屏）+ SVG（ImagePreview）

**Spec:** `docs/superpowers/specs/2026-07-27-annotation-tools-design.md`

## Global Constraints

- 荧光笔 = pen 变体（multiply 混合 + alpha 0.35 + 粗线宽 15）
- 橡皮擦 = 划过即删（mousemove hitTest），删除推入 redoStack 支持 undo
- 清空 = 全推 redoStack，不加确认弹窗
- 录屏不显示荧光笔（AnnotationToolbar `showHighlight=false`）
- 橡皮擦是操作模式（Tool 有 "eraser"），不是 Annotation type
- 三套系统共用 annotation.ts 的类型/绘制/命中函数

---

## File Structure

| 文件 | 操作 | 职责 |
|---|---|---|
| `lib/annotation.ts` | 修改 | Tool + Annotation type 加 highlight；drawAnnotation 加 highlight canvas 分支；hitTestAnnotationPrecise 覆盖 highlight |
| `components/Annotation/useAnnotationState.ts` | 修改 | 加 clearAllAnnotations + deleteSelectedAnnotation actions |
| `components/Annotation/AnnotationToolbar.tsx` | 修改 | tools 加 highlight；加 eraser/deleteSelected/clearAll 按钮；加 showHighlight prop |
| `pages/Screenshot/index.tsx` | 修改 | eraser mousemove hitTest 删除 + showHighlight 默认 true |
| `pages/RecordAnnotation/index.tsx` | 修改 | eraser mousemove hitTest 删除 + showHighlight=false |
| `pages/ImagePreview/AnnotationSvg.tsx` | 修改 | highlight SVG 渲染 |
| `pages/ImagePreview/Toolbar.tsx` | 修改 | tools 加 highlight；加 eraser/deleteSelected/clearAll 按钮 |
| `pages/ImagePreview/index.tsx` | 修改 | eraser mousemove + clearAll + deleteSelected 逻辑 |
| `locales/en.yaml` + `zh-CN.yaml` | 修改 | 加 highlight/eraser/clearAll/deleteSelected 文案 |

---

### Task 1: 共享类型 + 绘制 + 命中（annotation.ts）

**Files:**
- Modify: `crates/desktop/frontend/src/lib/annotation.ts`

**Interfaces:**
- Produces: `Tool` union 含 `"highlight"` + `"eraser"`；`Annotation.type` 含 `"highlight"`；`drawAnnotation` 处理 highlight；`hitTestAnnotationPrecise` 覆盖 highlight

- [x] **Step 1: 加 highlight/eraser 到 Tool + Annotation type**

Read `crates/desktop/frontend/src/lib/annotation.ts`. 修改两处 type 定义：

```typescript
// Tool 加 highlight + eraser
export type Tool = "none" | "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "highlight" | "text" | "number" | "blur" | "eraser";

// Annotation.type 加 highlight（不加 eraser——eraser 是操作模式不产生标注）
export interface Annotation {
  type: "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "highlight" | "text" | "number" | "blur";
  // ... 其余字段不变
}
```

- [x] **Step 2: drawAnnotation 加 highlight canvas 分支**

在 `drawAnnotation` 函数中，`pen` 分支之后加 `highlight` 分支。highlight 复用 pen 的 points polyline 绘制，但用 multiply 混合 + 半透明 + 粗线宽：

```typescript
if (ann.type === "highlight") {
  ctx.save();
  ctx.globalCompositeOperation = "multiply";
  ctx.globalAlpha = 0.35;
  ctx.lineWidth = ann.lineWidth || 15;
  ctx.strokeStyle = color;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  if (ann.points && ann.points.length > 0) {
    ctx.beginPath();
    ctx.moveTo(ann.points[0][0], ann.points[0][1]);
    for (let i = 1; i < ann.points.length; i++) {
      ctx.lineTo(ann.points[i][0], ann.points[i][1]);
    }
    ctx.stroke();
  }
  ctx.restore();
}
```

- [x] **Step 3: hitTestAnnotationPrecise 覆盖 highlight**

highlight 的命中测试与 pen 完全一致（都是 points polyline）。找到 hitTestAnnotationPrecise 函数，在 pen 分支旁加 highlight 走相同逻辑（`ann.type === "pen" || ann.type === "highlight"`）。

- [x] **Step 4: 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/lib/annotation.ts
git commit -m "feat(annotation): highlight 类型 + canvas 绘制 + hitTest（multiply 混合）"
```

---

### Task 2: useAnnotationState 加 clearAll + deleteSelected actions

**Files:**
- Modify: `crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts`

**Interfaces:**
- Produces: `clearAllAnnotations()` + `deleteSelectedAnnotation()` 加入 AnnotationState interface
- Consumes: Task 1 的 highlight type（间接——addAnnotation 支持 highlight annotation）

- [x] **Step 1: 加 clearAllAnnotations action**

在 `useAnnotationState` 函数内，`redoAnnotation` 之后加：

```typescript
const clearAllAnnotations = () => {
  setAnnotations((prev) => {
    if (prev.length === 0) return prev;
    redoStackRef.current.push(...prev);
    setRedoAvailable(true);
    return [];
  });
  setSelectedAnn(null);
  setNumberCounter(1);
  numberCounterRef.current = 1;
};
```

- [x] **Step 2: 加 deleteSelectedAnnotation action**

紧接 `clearAllAnnotations` 后加：

```typescript
const deleteSelectedAnnotation = () => {
  setSelectedAnn((sel) => {
    if (sel === null) return null;
    setAnnotations((prev) => {
      if (sel < 0 || sel >= prev.length) return prev;
      redoStackRef.current.push(prev[sel]);
      setRedoAvailable(true);
      return prev.filter((_, i) => i !== sel);
    });
    return null;
  });
};
```

- [x] **Step 3: 在 interface + return 加这两个 action**

`AnnotationState` interface 加：
```typescript
clearAllAnnotations: () => void;
deleteSelectedAnnotation: () => void;
```

return object 加：
```typescript
clearAllAnnotations,
deleteSelectedAnnotation,
```

- [x] **Step 4: 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts
git commit -m "feat(annotation): useAnnotationState 加 clearAll + deleteSelected actions"
```

---

### Task 3: AnnotationToolbar 加按钮 + showHighlight prop

**Files:**
- Modify: `crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx`

**Interfaces:**
- Consumes: Task 1 的 Tool 含 highlight/eraser；Task 2 的 clearAllAnnotations/deleteSelectedAnnotation
- Produces: `showHighlight` prop（截图 true，录屏 false）

- [x] **Step 1: 加 showHighlight prop**

`AnnotationToolbarProps` interface 加：
```typescript
/** 是否显示荧光笔按钮（截图 true，录屏 false） */
showHighlight?: boolean;
```

解构 props 时加 `showHighlight = true`。

- [x] **Step 2: tools 数组加 highlight**

在 `tools` 数组中，`pen` 之后加 highlight（条件显示）：

```typescript
const tools: { key: Tool; src: string; label: string }[] = [
  { key: "rect", src: "icons/square.svg", label: t("screenshot.tool.rect") },
  { key: "oval", src: "icons/oval-vertical.svg", label: t("screenshot.tool.ellipse") },
  { key: "diamond", src: "icons/diamond.svg", label: t("screenshot.tool.diamond") },
  { key: "line", src: "icons/straight-line.svg", label: t("screenshot.tool.line") },
  { key: "arrow", src: "icons/arrow-line.svg", label: t("screenshot.tool.arrow") },
  { key: "pen", src: "icons/sketching.svg", label: t("screenshot.tool.pen") },
  ...(showHighlight ? [{ key: "highlight" as Tool, src: "icons/highlighter.svg", label: t("screenshot.tool.highlight") }] : []),
  { key: "text", src: "icons/text.svg", label: t("screenshot.tool.text") },
  { key: "number", src: "icons/sequence-note.svg", label: t("screenshot.tool.number") },
  { key: "blur", src: "icons/mosaic.svg", label: t("screenshot.tool.mosaic") },
];
```

- [x] **Step 3: 加 eraser 按钮到 tools 数组末尾**

eraser 也是工具按钮（切换到 eraser 模式），加在 blur 之后：

```typescript
  { key: "eraser", src: "icons/eraser.svg", label: t("screenshot.tool.eraser") },
```

注意：eraser 工具按钮的 onClick 行为和其他工具一致（切换 tool + 弹 popover），但 eraser 不需要 popover 属性。在 `onToolSelect` 里 eraser 走正常切换逻辑即可。

- [ ] **Step 4: 加 deleteSelected + clearAll 按钮** ⚠️ **部分实现**（2026-07-29 z-sync 核对）

> **偏差**：`clearAll` 按钮已加（:286-300，用 `t("screenshot.tool.clear")` + `icons/clear.svg`）；但 **`deleteSelected` 按钮从未渲染**——底层 `deleteSelectedAnnotation` action、props、i18n 文案 `deleteSelected`、`trash.svg` 图标全都齐备，工具栏却没接线。Screenshot/RecordAnnotation 改用键盘 Delete/Backspace 删除选中标注（Screenshot L527-534、RecordAnnotation L363-371），但这两处直接 `setAnnotations.filter` 删除、**绕过了 `deleteSelectedAnnotation()` action，未推入 redoStack**（删了 undo 不回来）。待办：要么补上工具栏 `deleteSelected` 按钮，要么把键盘删除路径改走 action。

在 undo/redo 之后、`{children}` 之前加两个操作按钮（非工具切换，是直接 action）：

```jsx
<Divider />

{/* 删除选定标注 */}
<ToolButton
  onClick={(e) => {
    e.stopPropagation();
    state.deleteSelectedAnnotation();
  }}
  label={t("screenshot.tool.deleteSelected")}
  icon={
    <img
      src="icons/trash.svg"
      alt={t("screenshot.tool.deleteSelected")}
      className="w-[18px] h-[18px]"
      style={{ filter: "var(--icon-filter)", opacity: state.selectedAnn !== null ? 1 : 0.3 }}
    />
  }
/>

{/* 清空所有标注 */}
<ToolButton
  onClick={(e) => {
    e.stopPropagation();
    state.clearAllAnnotations();
  }}
  label={t("screenshot.tool.clearAll")}
  icon={
    <img
      src="icons/clear.svg"
      alt={t("screenshot.tool.clearAll")}
      className="w-[18px] h-[18px]"
      style={{ filter: "var(--icon-filter)", opacity: state.annotations.length > 0 ? 1 : 0.3 }}
    />
  }
/>
```

- [x] **Step 5: 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error（图标 SVG 文件可能不存在，先不管——用占位路径，Task 6 统一加图标）

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx
git commit -m "feat(annotation): AnnotationToolbar 加 highlight/eraser/deleteSelected/clearAll 按钮"
```

---

### Task 4: 截图 + 录屏 eraser mousemove 逻辑

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx`

**Interfaces:**
- Consumes: Task 1 的 hitTestAnnotationPrecise + Task 3 的 eraser tool

- [x] **Step 1: 截图 Screenshot eraser mousemove**

Read `crates/desktop/frontend/src/pages/Screenshot/index.tsx`。找到 mousemove handler（绘制逻辑所在）。在 tool === "eraser" 时，不做绘制，改为 hitTest + 删除：

```typescript
// 在 mousemove handler 里，tool 判断分支中加 eraser：
if (toolRef.current === "eraser") {
  // 橡皮擦：hitTest 当前鼠标位置，命中标注则删除
  const { x, y } = screenToCanvas(e.clientX, e.clientY); // 用现有的坐标转换
  const anns = annotationsRef.current;
  for (let i = anns.length - 1; i >= 0; i--) {
    if (hitTestAnnotationPrecise(x, y, anns[i])) {
      // 删除命中的标注（推入 redoStack 支持 undo）
      redoStackRef.current.push(anns[i]);  // 注意：需从 useAnnotationState 暴露 redoStackRef 或加 deleteErased action
      setAnnotations((prev) => prev.filter((_, j) => j !== i));
      break; // 每次 mousemove 只删一个，避免连删太快
    }
  }
  return; // 不走后续绘制逻辑
}
```

**重要**：`redoStackRef` 目前在 useAnnotationState 内部，业务侧无法直接访问。有两个方案：
- 方案 A：在 useAnnotationState 加 `eraseAnnotationAt(x, y)` action（封装 hitTest + 删除 + redoStack 推入）—— 更干净，业务侧只调一个函数
- 方案 B：暴露 redoStackRef —— 破坏封装

**选方案 A**。回到 Task 2 补一个 `eraseAnnotationAt` action：

在 useAnnotationState 加：
```typescript
const eraseAnnotationAt = (x: number, y: number) => {
  setAnnotations((prev) => {
    for (let i = prev.length - 1; i >= 0; i--) {
      if (hitTestAnnotationPrecise(x, y, prev[i])) {
        redoStackRef.current.push(prev[i]);
        setRedoAvailable(true);
        return prev.filter((_, j) => j !== i);
      }
    }
    return prev;
  });
};
```

需要 import `hitTestAnnotationPrecise`（annotation.ts 的函数，同模块可访问）。

interface + return 加 `eraseAnnotationAt`。

然后截图 mousemove 里改为：
```typescript
if (toolRef.current === "eraser") {
  const { x, y } = screenToCanvas(e.clientX, e.clientY);
  eraseAnnotationAt(x, y);
  return;
}
```

- [x] **Step 2: 录屏 RecordAnnotation eraser mousemove**

Read `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx`。同样在 mousemove handler 加 eraser 分支（坐标转换用 RecordAnnotation 自己的 canvasRect 偏移逻辑）。

同时给 AnnotationToolbar 传 `showHighlight={false}`。

- [x] **Step 3: 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts \
  crates/desktop/frontend/src/pages/Screenshot/index.tsx \
  crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx
git commit -m "feat(annotation): 截图+录屏 eraser 划过即删 + 录屏 showHighlight=false"
```

---

### Task 5: ImagePreview highlight SVG + 按钮扩展

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/AnnotationSvg.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**Interfaces:**
- Consumes: Task 1 的 highlight type/draw

- [x] **Step 1: AnnotationSvg 加 highlight 渲染**

Read `crates/desktop/frontend/src/pages/ImagePreview/AnnotationSvg.tsx`。在 switch 的 pen 分支旁加 highlight：

```jsx
case "highlight": {
  if (!ann.points || ann.points.length === 0) return null;
  const pts = ann.points.map(p => `${p[0]},${p[1]}`).join(" ");
  return (
    <polyline
      points={pts}
      fill="none"
      stroke={color}
      strokeWidth={ann.lineWidth || 15}
      opacity={0.35}
      style={{ mixBlendMode: "multiply", strokeLinecap: "round", strokeLinejoin: "round" }}
    />
  );
}
```

- [ ] **Step 2: Toolbar 加 highlight + eraser + deleteSelected + clearAll** ⚠️ **部分实现**（2026-07-29 z-sync 核对）

> **偏差**：`highlight` 在 tools 数组（:132）、`eraser` 独立按钮（:185-195）、`clearAll` 按钮（:203-205，用 `onClearAll` + `icons/clear.svg`）均已实现；但 **`deleteSelected` 按钮未渲染**——prop 类型声明有（:70 `onDeleteSelected`/`canDeleteSelected`）、index.tsx 也传了（:796），但 Toolbar 内从未渲染对应按钮（无 `trash.svg` ToolButton）。与 Task 3 Step 4 同一问题：底层齐全，按钮缺失。

Read `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`。

tools 数组在 blur 前加 highlight（与 pen 之间）：
```typescript
{ key: "highlight", icon: <SvgIcon src="icons/highlighter.svg" ... />, title: t("imagePreview.tool.highlight") },
```

tools 数组末尾加 eraser：
```typescript
{ key: "eraser", icon: <SvgIcon src="icons/eraser.svg" ... />, title: t("imagePreview.tool.eraser") },
```

在 undo/redo 之后加 deleteSelected + clearAll 按钮（需 Toolbar 接收 onDeleteSelected + onClearAll 回调 prop）。

- [x] **Step 3: index.tsx eraser mousemove + clearAll + deleteSelected**

Read `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`。

加 eraser mousemove 逻辑（对 niions 数组 hitTest + 删除，同 Task 4 方案 A 模式——但 ImagePreview 是独立 state，直接操作 niions）。

加 clearAll + deleteSelected 函数，传给 Toolbar。

- [x] **Step 4: 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/ImagePreview/
git commit -m "feat(annotation): ImagePreview highlight SVG + eraser/deleteSelected/clearAll"
```

---

### Task 6: i18n 文案 + 图标资源

**Files:**
- Modify: `crates/desktop/frontend/src/locales/en.yaml`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`
- Create: 图标 SVG 文件（如不存在）

- [x] **Step 1: 加 i18n 文案**

`zh-CN.yaml` screenshot.tool 下加：
```yaml
highlight: 荧光笔
eraser: 橡皮擦
clearAll: 清空标注
deleteSelected: 删除选定
```

`en.yaml` 对应：
```yaml
highlight: Highlighter
eraser: Eraser
clearAll: Clear All
deleteSelected: Delete Selected
```

imagePreview.tool 下同样加（或复用 screenshot key，取决于现有 i18n 结构——读文件确认）。

- [x] **Step 2: 确认/添加图标 SVG**

检查 `crates/desktop/frontend/public/icons/` 下是否有 `highlighter.svg` / `eraser.svg` / `trash.svg` / `clear.svg`。如果缺失，从 lucide icons（lucide.dev）下载对应 SVG 放入，或用已有的 lucide React 组件替代 img 标签。

- [x] **Step 3: vite build 验证**

Run: `cd crates/desktop/frontend && npx vite build`
Expected: 0 error

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/locales/ crates/desktop/frontend/public/icons/
git commit -m "feat(annotation): i18n 文案 + 图标资源（highlight/eraser/clear/deleteSelected）"
```

---

### Task 7: 全量验证 + architecture.md

- [x] **Step 1: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vite build`
Expected: 0 error

- [x] **Step 2: desktop 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 0 error

- [x] **Step 3: 更新 architecture.md**

在标注工具描述处补 highlight/eraser/clearAll/deleteSelected。

- [x] **Step 4: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture.md 补标注工具扩充（highlight/eraser/clear/deleteSelected）"
```
