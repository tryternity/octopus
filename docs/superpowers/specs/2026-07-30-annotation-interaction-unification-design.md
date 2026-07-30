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
