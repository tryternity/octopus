# 标注交互统一：useAnnotationInteraction hook

**日期**：2026-07-30
**类型**：架构重构
**分支**：`daily_feature_0729`

## 背景

ImagePreview（876 行）、Screenshot（1108 行）、RecordAnnotation（581 行）三个场景各自内联了一套标注鼠标交互逻辑（画/拖/擦/undo/redo/文字提交），代码高度重复但坐标换算各不相同。RecordAnnotation 已部分迁移到 `components/Annotation/useAnnotationState`，但 ImagePreview 和 Screenshot 仍是内联。

## 目标

- 统一三个场景的标注交互到一个 hook（`useAnnotationInteraction`）
- 各场景只提供坐标换算函数（`clientToNatural`），不碰鼠标交互细节
- ImagePreview index.tsx 从 ~876 行降至 ~600 行（移除 ~300 行标注交互代码）
- Screenshot index.tsx 从 ~1108 行降至 ~850 行
- RecordAnnotation 补齐到完整使用 hook

## 设计

### 核心接口

```typescript
// components/Annotation/useAnnotationInteraction.ts

/** 坐标换算：屏幕 clientX/Y → 标注自然坐标。各场景提供。 */
type ClientToNatural = (clientX: number, clientY: number) => { x: number; y: number };

interface UseAnnotationInteractionOptions {
  /** 坐标换算函数（必须随 zoom/scroll 变化时返回最新闭包） */
  clientToNatural: ClientToNatural;
  /** 图片自然尺寸（hitTest 边界 + 文字标注定位） */
  natW: number;
  natH: number;
}

function useAnnotationInteraction(opts: UseAnnotationInteractionOptions) {
  // ── 标注数据（受控）──
  const annotations: Annotation[];
  const draftAnn: Annotation | null;
  const setAnnotations: Dispatch<SetStateAction<Annotation[]>>;

  // ── 鼠标交互（各场景绑定到自己的 canvas/svg 元素）──
  /** mousedown：根据 tool 创建/拖拽/擦除/平移/文字提交。需传入当前工具状态。 */
  const handleMouseDown: (e: React.MouseEvent, ctx: ToolContext) => void;
  /** mousemove：拖拽中/绘制中/擦除中更新。 */
  const handleMouseMove: (e: React.MouseEvent) => void;
  /** mouseup：结束当前操作（绘制完成/拖拽落定/擦除完成）。 */
  const handleMouseUp: () => void;

  // ── undo/redo ──
  const undo: () => void;
  const redo: () => void;
  const redoAvailable: boolean;

  // ── 文字标注 ──
  const textDraft: { x: number; y: number; val: string; fs: number } | null;
  const commitText: () => void;
  const cancelText: () => void;

  // ── 清空 ──
  const clearAll: () => void;
}

/** mousedown 时传入的工具上下文（各场景从自己的 toolbar state 提供） */
interface ToolContext {
  tool: Tool;
  color: string;
  width: number;
  fontSize: number;
  filled: boolean;
}
```

### 各场景坐标换算实现

hook 不关心坐标怎么算——各场景提供 `clientToNatural`：

**ImagePreview**（有 zoom + scroll container）：
```typescript
const clientToNatural = useCallback((cx: number, cy: number) => {
  const sc = scrollContainerRef.current!;
  const scRect = sc.getBoundingClientRect();
  const imgScreenX = scRect.left + imgLeft - sc.scrollLeft;
  const imgScreenY = scRect.top + imgTop - sc.scrollTop;
  return { x: (cx - imgScreenX) / zoomRef.current, y: (cy - imgScreenY) / zoomRef.current };
}, [imgLeft, imgTop]);
```

**Screenshot**（全屏，scale = natW / windowW）：
```typescript
const clientToNatural = useCallback((cx: number, cy: number) => {
  const scale = natW / window.innerWidth;
  return { x: cx * scale, y: cy * scale };
}, [natW]);
```

**RecordAnnotation**（选区固定位置，1:1）：
```typescript
const clientToNatural = useCallback((cx: number, cy: number) => {
  return { x: cx - canvasRectRef.current.ox, y: cy - canvasRectRef.current.oy };
}, []);
```

### onMouseDown 语义统一

当前三个场景的 onMouseDown 都承担"标注创建 + 拖拽 + 擦除 + 平移 + 文字提交"多种语义。统一后的 `handleMouseDown(e, ctx)` 内部分支：

```
tool === 'text' → 提交已有文字 draft 或创建新文字 draft
tool === 'eraser' → 进入擦除模式
tool === 'select' → hitTest：命中=拖拽，未命中=无操作
其他（pen/rect/arrow/...）→ 开始创建新标注
```

平移（手型工具）不纳入 hook——各场景自己处理（ImagePreview 的 scroll pan、Screenshot 的选区拖拽）。

### composePngBytes

`composePngBytes`（合成标注到 canvas 导出 PNG）也移到 hook 或独立工具函数，签名：
```typescript
async function composeAnnotatedImage(
  img: HTMLImageElement,
  natW: number, natH: number,
  annotations: Annotation[],
): Promise<ArrayBuffer>
```

三个场景共用。

### 与现有 useAnnotationState 的关系

**扩展而非新建**——当前 `components/Annotation/useAnnotationState.ts` 已有：
- `annotations` state + add/update/delete
- `clearAll`
- `redoStack`

**新增到 useAnnotationInteraction**（在 useAnnotationState 基础上）：
- `handleMouseDown/Move/Up` 鼠标交互
- `clientToNatural` 坐标换算
- `undo`（当前只有 redo）
- `textDraft` / `commitText` / `cancelText`
- `draftAnn`（绘制中的临时标注）

### 迁移路径

1. **扩展 useAnnotationState → useAnnotationInteraction**（加鼠标交互 + 坐标换算 + undo + 文字）
2. **ImagePreview index.tsx 迁移**（~300 行 → ~30 行 hook 调用）
3. **Screenshot index.tsx 迁移**（~320 行 → ~30 行 hook 调用）
4. **RecordAnnotation 补齐**（已有 useAnnotationState，补加鼠标交互）

每步独立可验证（tsc + 手动冒烟）。

## 不在本次范围

- Screenshot 的选区逻辑（crop region）——不纳入 hook，各场景自己管
- canvas 底图绘制 + zoom/fit/pan——不纳入 hook（各场景 viewport 策略不同）
- 窗口管理（透明全屏 / sticky canvas）——不纳入 hook

## 风险

- **坐标闭包陷阱**：`clientToNatural` 必须在每次 zoom/scroll 变化时返回最新闭包。hook 内部用 ref 镜像（同 index.tsx 现有模式）。
- **mousedown 多语义分支**：三个场景的 mousedown 分支细节有差异（ImagePreview 有 loadingFull 拦截，Screenshot 有 popoverDismissKey），迁移时需逐一核对。
- **文字标注的 textarea 定位**：ImagePreview 用 CSS 绝对定位 + fontSize/zoom 计算，Screenshot/RecordAnnotation 可能不同——hook 返回 textDraft 数据，各场景自己渲染 textarea。

## 实施结果（2026-07-30）

### 已完成
- ✅ Task 1：useAnnotationInteraction hook 创建
- ✅ Task 2：ImagePreview 迁移（876→751，-125 行）

### 暂不迁移
- ⏸️ Task 3：Screenshot —— 标注交互与选区状态机深度耦合（同一 onMouseDown 内先判 handle、再判选区内/外、再判标注工具），硬拆破坏选区+标注交互时序
- ⏸️ Task 4：RecordAnnotation —— 标注拖拽用 annMoveStartRef（数组快照模式）而非 hook 的 dragRef（偏移量模式），eraser 用 e.buttons 检测而非 erasingRef，接口不兼容

### 根因分析
三个场景的标注交互虽然"看起来相似"，但在关键实现细节上有本质差异：
1. **拖拽模式**：ImagePreview 用 `{idx, dx, dy}` 偏移量（hook 兼容）；Screenshot/RecordAnnotation 用 `{idx, mx, my, anns}` 数组快照
2. **eraser 检测**：ImagePreview 用 erasingRef（hook 兼容）；Screenshot/RecordAnnotation 用 `e.buttons & 1` 运行时检测
3. **文字标注**：ImagePreview 的 textDraft 有 textWidth（从 DOM 读）；Screenshot/RecordAnnotation 硬编码 textWidth: 200
4. **选区耦合**：Screenshot 的标注操作被 `inSelection(mx, my)` 门控（只在选区内画标注）；RecordAnnotation 用 canvasRect 偏移

hook 的价值在于为 ImagePreview 这个最大场景（876 行）减负了 125 行。Screenshot/RecordAnnotation 的迁移需要先统一拖拽模式 + eraser 检测方式——这是行为变更，不是纯重构，需要单独评估。

### hook 本身的价值
useAnnotationInteraction 已创建并验证，可以作为**新场景**（如未来新增的图片编辑器）的标注交互基础设施。现有三个场景的渐进迁移留待后续。
