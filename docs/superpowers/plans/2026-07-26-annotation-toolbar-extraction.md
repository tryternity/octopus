# 标注工具栏抽取 — 实施计划（plan）

> **配套 spec**：[`docs/superpowers/specs/2026-07-26-annotation-toolbar-extraction.md`](../specs/2026-07-26-annotation-toolbar-extraction.md)
>
> **目标**：把 `Screenshot/index.tsx`（1021 行）与 `RecordAnnotation/index.tsx`（595 行）共有的标注 state + 工具栏 UI + 位置算法抽取到 `components/Annotation/`，消除 ~250 行重复，根除「位置/行为不一致」反馈。
>
> **范围**：本轮只迁 Screenshot + RecordAnnotation；ImagePreview 留 phase 2（详见 spec §0.2）。

## 执行节奏总览

| Task | 内容 | 验证命令 | 预计 LOC |
|---|---|---|---|
| 1 | 建抽取层（新文件，不动现有代码）| `npm test`（新单测）+ `npm run build` | +400 |
| 2 | 迁 RecordAnnotation（小，先验证）| 手动 e2e + `npm run build` | -150 / +30 |
| 3 | 迁 Screenshot（大，重点回归）| 手动 e2e + `npm run build` | -250 / +40 |
| 4 | review plan + 同步 architecture.md | — | +50（文档）|

每个 Task 独立 commit；Task 1-3 中任一 step 失败可立即停下不污染下一 Task。

---

## Task 1：建抽取层（新文件）

**目标**：在 `components/Annotation/` 下新建文件，**完全不动 Screenshot / RecordAnnotation**。本 Task 结束后现有功能零变化。

### 1.1 `src/components/Annotation/position.ts`

纯函数，复制 Screenshot L744-781 的算法：

```typescript
export type ToolbarPlacement = "below" | "above" | "inside";
export interface Rect { x: number; y: number; w: number; h: number; }
export interface ToolbarPosition { y: number; placement: ToolbarPlacement; belowOrAbove: boolean; }

export const TOOLBAR_H = 44;
export const DOCK_MARGIN = 80;

export function computeToolbarPosition(sel: Rect, viewportH: number): ToolbarPosition {
  const belowSpace = viewportH - (sel.y + sel.h + 8);
  const aboveSpace = sel.y;
  const toolbarBelow = belowSpace >= TOOLBAR_H;
  const toolbarAbove = !toolbarBelow && aboveSpace >= TOOLBAR_H;
  const y = toolbarBelow
    ? Math.min(sel.y + sel.h + 8, viewportH - TOOLBAR_H)
    : toolbarAbove
      ? sel.y - TOOLBAR_H - 4
      : Math.max(sel.y, sel.y + sel.h - TOOLBAR_H - 8);  // inside 兜底
  return { y, placement: toolbarBelow ? "below" : toolbarAbove ? "above" : "inside", belowOrAbove: toolbarBelow || toolbarAbove };
}

export function computeToolbarCenterX(sel: Rect, viewportW: number, toolbarW: number): number {
  const halfW = toolbarW / 2;
  return Math.max(DOCK_MARGIN + halfW, Math.min(sel.x + sel.w / 2, viewportW - DOCK_MARGIN - halfW));
}
```

**注意**：录屏调用时 `canvasRect` 转 `Rect` 用 `{ x: canvasRect.ox, y: canvasRect.oy, w: canvasRect.w, h: canvasRect.h }`，业务侧做这层转换，纯函数不感知 canvasRect。

### 1.2 `src/components/Annotation/position.test.ts`

完整单元测试（参照 `src/lib/i18n.test.ts` 写法）：

- `computeToolbarPosition` 三选：below（默认）/ above（下方不够）/ inside（上下都不够，全屏截图场景）/ below 时 y clamp
- `computeToolbarCenterX`：选区居中 / 靠左 clamp / 靠右 clamp / toolbarW=0 边界

**验证命令**：`cd crates/desktop/frontend && npm test -- position.test.ts`

### 1.3 `src/components/Annotation/useAnnotationState.ts`

抽 hook，统一两边 state（重点修复 `numberCounter` 不一致：用 ref + state 镜像）。**直接合并 RecordAnnotation L48-69 + L134-159** 的实现作为基线（它已经是 ref 模式）：

- tool / toolColor / toolWidth / toolFontSize / toolFilled / toolCircleSize：全部 state + ref 镜像
- annotations + annotationsRef
- drawingRef
- redoStackRef + redoAvailable
- numberCounter + numberCounterRef（ref 为主，state 触发 render）
- selectedAnn + setSelectedAnn
- showPopover + popoverX
- addAnnotation / undoAnnotation / redoAnnotation（合并两边，用 ref 读最新值）

**hook 不写单元测试**：理由——`@testing-library/react` 未装，引入新依赖不值；hook 行为通过 Task 2/3 的 e2e 验证。`computeToolbarPosition` 这种纯算法才是单元测试的核心目标。

### 1.4 `src/components/Annotation/AnnotationToolbar.tsx`

组件，把 Screenshot L886-998 / RecordAnnotation L501-592 的工具栏 JSX 抽出来：

```typescript
export interface AnnotationToolbarProps {
  state: AnnotationState;
  onToolChange?: (t: Tool) => void;
  top: number;
  left: number;
  toolbarRef?: React.Ref<HTMLDivElement>;
  children?: React.ReactNode;
  popoverY?: number;
}
```

**职责边界**：
- 工具按钮 onClick 触发 `state.setTool` + `state.setShowPopover` + `state.setPopoverX` + 调用 `onToolChange(t)` 让业务侧做透传（passthrough / 取消选区 move 等）
- `children` 渲染在 divider 后（业务按钮 OCR/save/stop/...）
- popover 用现有 `ToolPropsPopover`（从 `@/pages/Screenshot/ToolPropsPopover` import，本轮不重写）
- popover 由本组件基于 `state.showPopover && state.tool !== "none"` 渲染，但 `popoverY` 由业务侧传入

### 1.5 `src/components/Annotation/index.ts`

re-export 上述 4 个文件的公共 API。

### Task 1 验证

```bash
cd crates/desktop/frontend
npm test -- position        # 新纯函数单测全过
npm run build               # 0 error 0 warning（新文件被编译但不被引用，tree-shake 掉）
cargo check -p octopus-desktop  # 不影响 Rust 编译
```

**commit message**：`refactor(annotation): 抽 useAnnotationState + AnnotationToolbar + position 纯函数（Task 1，未接入）`

---

## Task 2：迁 RecordAnnotation（小，先验证）

**目标**：用 Task 1 的抽取层替换 `RecordAnnotation/index.tsx` 里重复的 state/logic/JSX。预计从 595 行降到 ~450 行。

### 2.1 替换 state 块（L48-89）

删约 20 行重复 state，改 `const annotation = useAnnotationState()`。保留 RecordAnnotation 独有：textDraft/textDraftRef/textInputRef、annMoveStartRef、canvasRect/canvasRectRef/toolbarPos、toolbarW/toolbarRef。

### 2.2 替换 add/undo/redo（L134-159）

直接删除——用 hook 返回的 actions。

### 2.3 替换工具栏 JSX（L501-571）

```tsx
<AnnotationToolbar
  state={annotation}
  onToolChange={(t) => invoke("set_annotation_passthrough", { passthrough: t === "none" }).catch(() => {})}
  top={toolbarTop}
  left={toolbarCenterX}
  toolbarRef={toolbarRef}
  popoverY={popoverY}
>
  <button onClick={onStopClick} title={t("tray.recordStop")} style={{ /* 红色圆点 */ }}>...</button>
</AnnotationToolbar>
```

### 2.4 替换位置算法（L412-435）

```typescript
const canvasAsRect = { x: canvasRect.ox, y: canvasRect.oy, w: canvasRect.w, h: canvasRect.h };
const tbPos = computeToolbarPosition(canvasAsRect, window.innerHeight);
const toolbarTop = tbPos.y;
const popoverY = tbPos.belowOrAbove ? toolbarTop + TOOLBAR_H : Math.max(0, toolbarTop - 200);
const toolbarCenterX = computeToolbarCenterX(canvasAsRect, window.innerWidth, toolbarW || 300);
```

**注意**：录屏的 `toolbarPos` URL 参数原本是后端三选注入的，现在改用 `computeToolbarPosition` 实时算。验证后端 URL 参数是否还有用——如果只是 tooltip 性质的提示可以忽略；如果后端依赖前端 toolbarPos 反馈，需检查 `record_annotation_window.rs`。

### 2.5 mousedown/move/up 保留

录屏的 mouse handler（L226-352）**保留不动**——它们处理 canvasRect 偏移 + 业务特定分支。仅替换其中对 `tool`/`toolColorRef` 等变量的访问——改用 `annotation.toolRef.current` / `annotation.toolColorRef.current`。

### Task 2 验证

```bash
cd crates/desktop/frontend
npm run build               # 0 error 0 warning
cargo check -p octopus-desktop
# 手动 e2e（必做）：
# 1. Cmd+Shift+R → 配置浮窗 → 切 area → 拖选区
# 2. 开始录制 → overlay 出现 → 默认 passthrough
# 3. 选 rect/arrow/pen/text/number 各画一个 → 都被录进视频
# 4. cmd+z undo 3 次 → cmd+shift+z redo 2 次
# 5. 按 A 切换 passthrough → 操作下层应用 → 切回
# 6. 点工具栏 stop 按钮 → 停止 + 入库
# 7. 播放视频 → 检查标注可见 + 选区正确
```

**commit message**：`refactor(record): RecordAnnotation 改用 AnnotationToolbar + useAnnotationState（Task 2）`

---

## Task 3：迁 Screenshot（大，重点回归）

**目标**：同样替换 `Screenshot/index.tsx` 重复部分。预计从 1021 行降到 ~770 行。

### 3.1 替换 state 块（L40-79）

同 Task 2.1，保留 Screenshot 独有：sel/setSel/mode/modeRef/resizeHandle + 选区拖动相关 refs、bgImgRef/bgBitmapRef/ready、scrollPreview/scrollHeight/scrollFrameRef/scrollSaveAfterStopRef、editTextColorRef/editTextFontSizeRef/editTextOrigRef、ocrWarn。

### 3.2 替换 add/undo/redo（L161-185）

删除，用 hook。**重点验证 numberCounter**：截图原是 `useState` 模式，迁移后改 ref 模式——连续 number → undo → redo → 继续 number 必须正常。

### 3.3 替换工具栏 JSX（L886-959）

业务按钮 OCR/scroll/save/pin/confirm/cancel 作为 children 注入。

### 3.4 替换位置算法（L744-781）

```typescript
const tbPos = sel ? computeToolbarPosition(sel, window.innerHeight) : null;
const toolbarY = tbPos ? tbPos.y : 0;
const toolbarBelow = tbPos?.placement === "below";
const toolbarAbove = tbPos?.placement === "above";
const popoverY = tbPos?.belowOrAbove ? toolbarY + TOOLBAR_H : Math.max(0, toolbarY - 200);
const toolbarCenterX = sel ? computeToolbarCenterX(sel, window.innerWidth, toolbarW) : 0;
```

**注意**：截图的 `toolbarBelow` 还被 `pin 按钮位置`（L966）和 `选区尺寸 label 位置`（L281-283）依赖，必须保留这个 bool。

### 3.5 保留 mousedown/move/up（L322-528）

同 Task 2.5，截图 mouse handler 有大量选区逻辑（hitTest 手柄 / move / resize / 滚动），抽进 hook 得不偿失。仅替换对 state 变量的访问。

### Task 3 验证

```bash
cd crates/desktop/frontend
npm run build               # 0 error 0 warning
cargo check -p octopus-desktop
# 手动 e2e（重点回归，必做）：
# 1. 截图快捷键 → 拖选区 → 9 工具各画一个 → cmd+z/shift+z
# 2. 选区靠屏幕顶/底/左/右/全屏 → 工具栏不超出屏幕（验证 computeToolbarPosition 三选）
# 3. 选区 move/resize 手柄正常
# 4. 双击文字标注 → 编辑 → ESC 恢复
# 5. OCR → 复制到剪贴板
# 6. 滚动截图（startScroll）→ 预览正常
# 7. save/pin/confirm/cancel 按钮
# 8. 选区尺寸 label 位置正确（toolbarBelow 时在上、above 时在下）
```

**commit message**：`refactor(screenshot): Screenshot 改用 AnnotationToolbar + useAnnotationState（Task 3）`

---

## Task 4：review plan + 同步文档

### 4.1 review plan（强制）

按 AGENTS.md「review plan（强制）」原则，Task 3 完成后回看本 plan，把以下偏差回写：
- 实际 LOC 增减（预计 vs 实际）
- `numberCounter` 迁移是否发现新 bug
- 录屏 `toolbarPos` URL 参数迁移后的处理（保留 / 删除）
- AnnotationToolbar 的 props 是否需要调整（如新增 `onToolChange` 时机）
- Task 1 的 hook 是否真的没装 `@testing-library/react`（如果迁移中发现需要，回写到 plan）

### 4.2 同步 architecture.md

更新 architecture.md 的「屏幕录制」和「截图」section：
- 新增「标注工具栏抽取」subsection（指向本 spec/plan）
- 在「模块依赖关系」补充 `components/Annotation/` 的位置

### 4.3 触发 z-sync-superpowers

调用 `z-sync-superpowers` skill 确保 spec/plan/architecture 全同步。

---

## 风险与回滚

| 风险 | 应对 |
|---|---|
| Task 2 后 hook 设计有问题，Task 3 受阻 | Task 2 是独立 commit，可 revert 仅回滚到 Task 1 后状态 |
| Task 3 后截图回归严重（选区/滚动/OCR）| Task 3 是独立 commit，可 revert 回 Task 2 后状态（截图回到原版，录屏保持新版本）|
| `numberCounter` ref 模式引入新 bug | Task 3 验证步骤覆盖连续 number → undo → redo 路径 |
| 录屏的 `toolbarPos` URL 参数被后端依赖 | Task 2.4 验证步骤里检查 `record_annotation_window.rs`，如有依赖保留参数解析但忽略结果 |

## 不做的事

- ❌ 不抽 mouse handler（mousedown/move/up 两边控制流不同）
- ❌ 不重写 ToolPropsPopover（126 行已被两边共用，复用即可）
- ❌ 不动 ImagePreview（phase 2）
- ❌ 不动 `lib/annotation.ts`（纯函数层稳定）
- ❌ 不装 `@testing-library/react`（hook 行为靠 e2e 验证，纯函数靠单测）
