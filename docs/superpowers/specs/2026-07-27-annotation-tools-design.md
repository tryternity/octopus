# 标注工具扩充设计（荧光笔 / 橡皮擦 / 清空 / 删除选定）

> **日期**：2026-07-27
> **状态**：设计阶段（待实现）
> **来源**：[竞品分析报告](../../research/2026-07-27-competitive-analysis.md) §4 截屏 P1 + 用户需求

---

## 1. 需求

为三套标注系统（截图 / 录屏 / 图文编辑器 ImagePreview）扩充标注操作工具：

| 功能 | 截图 | 录屏 | 图文编辑器 |
|---|:---:|:---:|:---:|
| 荧光笔 highlight | ✅ | ❌ | ✅ |
| 橡皮擦 eraser | ✅ | ✅ | ✅ |
| 清空标注 clearAll | ✅ | ✅ | ✅ |
| 删除选定 deleteSelected | ✅ | ✅ | ✅ |

## 2. 现状

三套标注系统：

| 系统 | 工具栏 | 状态管理 | 渲染 |
|---|---|---|---|
| 截图 Screenshot | 共享 `AnnotationToolbar` | 共享 `useAnnotationState` | Canvas |
| 录屏 RecordAnnotation | 共享 `AnnotationToolbar` | 共享 `useAnnotationState` | Canvas |
| 图文编辑器 ImagePreview | 独立 `Toolbar.tsx` | 独立 state（`niions`） | SVG |

现有 9 标注工具：rect / oval / diamond / line / arrow / pen / text / number / blur。已有 undo/redo。已有 `selectedAnn` state（选中索引）但无「删除选中」action。

共享类型在 `crates/desktop/frontend/src/lib/annotation.ts`（Tool 类型 + Annotation 接口 + drawAnnotation + hitTest）。

## 3. 设计

### 3.1 荧光笔 highlight

**交互**：自由画笔（pen 变体），半透明 multiply 混合。复用 pen 的 `points` 数据结构（`number[][]` 坐标点序列）。

**Tool 类型**：`annotation.ts` 的 `Tool` union 加 `"highlight"`。

**Annotation 类型**：`type` union 加 `"highlight"`。字段同 pen（`points` + `color` + `lineWidth`），默认 lineWidth 较粗（15）。

**Canvas 绘制**（`drawAnnotation`）：
```typescript
if (ann.type === "highlight") {
  ctx.save();
  ctx.globalCompositeOperation = "multiply";
  ctx.globalAlpha = 0.35;
  ctx.lineWidth = ann.lineWidth || 15;
  // 与 pen 相同的 polyline 绘制逻辑
  ctx.restore();
}
```

**SVG 渲染**（ImagePreview `AnnotationSvg.tsx`）：
```jsx
case "highlight":
  return <polyline points={...} fill="none" stroke={color} strokeWidth={lw}
    opacity="0.35" style={{ mixBlendMode: "multiply" }} />
```

### 3.2 橡皮擦 eraser

**交互**：划过即删。tool 为 `"eraser"` 时，mousemove 对每个 annotation 做 hitTest（点位 → 标注命中），命中即删除。可连续划过多删。

**Tool 类型**：`Tool` union 加 `"eraser"`（操作模式，不是标注类型——不产生 Annotation）。

**实现**（业务侧 mousemove handler）：
- 截图/录屏（Canvas）：mousedown 进入 eraser 模式 → mousemove 时遍历 `annotationsRef.current`，调 `hitTestAnnotationPrecise(x, y, ann)` 命中则 `splice` 删除 → 触发重绘
- ImagePreview（SVG）：同逻辑，对 `niions` 数组操作

**删除时推入 redoStack**：橡皮擦删除的标注推入 `redoStackRef`，支持 undo 恢复。

**光标**：CSS `cursor: crosshair` 或自定义橡皮擦图标。

### 3.3 清空标注 clearAll

**交互**：点「清空」按钮 → 立即清空所有标注。不加确认弹窗（undo 是更好的安全网——清空前的标注全推入 redoStack，undo 可逐个或批量恢复）。

**实现**（useAnnotationState 加 action）：
```typescript
const clearAllAnnotations = () => {
  setAnnotations((prev) => {
    if (prev.length === 0) return prev;
    redoStackRef.current.push(...prev);  // 全部推入 redoStack 支持 undo
    setRedoAvailable(true);
    return [];
  });
  setSelectedAnn(null);
  setNumberCounter(1);
};
```

ImagePreview 独立实现等价逻辑。

### 3.4 删除选定 deleteSelected

**交互**：选中一个标注后（`selectedAnn != null`），点「删除选定」按钮删除它。

**实现**（useAnnotationState 加 action）：
```typescript
const deleteSelectedAnnotation = () => {
  if (selectedAnn === null) return;
  setAnnotations((prev) => {
    const removed = prev[selectedAnn!];
    if (removed) {
      redoStackRef.current.push(removed);
      setRedoAvailable(true);
    }
    return prev.filter((_, i) => i !== selectedAnn);
  });
  setSelectedAnn(null);
};
```

ImagePreview 独立实现等价逻辑。

## 4. 工具栏按钮排列

### AnnotationToolbar（截图 + 录屏共享）

现有：`select | rect oval diamond line arrow pen text number blur | undo redo | 业务按钮`

改为（截图，`showHighlight=true`）：
```
select | rect oval diamond line arrow pen highlight text number blur | divider | undo redo | divider | eraser deleteSelected clearAll | divider | 业务按钮
```

改为（录屏，`showHighlight=false`）：
```
select | rect oval diamond line arrow pen text number blur | divider | undo redo | divider | eraser deleteSelected clearAll | divider | 业务按钮
```

**新增 prop**：`showHighlight?: boolean`（默认 true，录屏传 false）。

**按钮图标**（复用现有 SVG 图标风格）：
- highlight: `icons/highlighter.svg`（需新增；或用 lucide Highlighter 图标）
- eraser: `icons/eraser.svg`（需新增；或用 lucide Eraser 图标）
- deleteSelected: `icons/trash.svg`（或 lucide Trash2）
- clearAll: `icons/clear.svg`（或 lucide Eraser 全量感）

**deleteSelected / clearAll 禁用态**：无选中 / 无标注时 opacity 0.3 + cursor default。

### ImagePreview Toolbar

现有 tools 数组末尾加 highlight（在 blur 后）。undo/redo 后面加 eraser / deleteSelected / clearAll 三个按钮。

## 5. 改动文件

| 文件 | 改动 |
|---|---|
| `lib/annotation.ts` | Tool + Annotation type 加 highlight；drawAnnotation 加 highlight 分支；hitTestAnnotationPrecise 覆盖 highlight（同 pen 逻辑） |
| `components/Annotation/useAnnotationState.ts` | 加 clearAllAnnotations + deleteSelectedAnnotation actions |
| `components/Annotation/AnnotationToolbar.tsx` | tools 数组加 highlight；加 eraser/deleteSelected/clearAll 按钮；加 showHighlight prop |
| `pages/Screenshot/index.tsx` | eraser mousemove hitTest 删除逻辑 |
| `pages/RecordAnnotation/index.tsx` | eraser mousemove hitTest 删除逻辑 + showHighlight=false |
| `pages/ImagePreview/AnnotationSvg.tsx` | highlight SVG 渲染（polyline + mixBlendMode） |
| `pages/ImagePreview/Toolbar.tsx` | tools 数组加 highlight；加 eraser/deleteSelected/clearAll 按钮 |
| `pages/ImagePreview/index.tsx` | eraser mousemove + clearAll + deleteSelected 逻辑 |
| `locales/en.yaml` + `zh-CN.yaml` | 加 highlight/eraser/clearAll/deleteSelected 文案 |

## 6. 不变量

| # | 不变量 | 保证 |
|---|---|---|
| INV-1 | 荧光笔不遮挡标注下方内容（半透明混合而非覆盖） | multiply 混合模式 + alpha 0.35 |
| INV-2 | 橡皮擦删除的标注可 undo 恢复 | 删除时推入 redoStack |
| INV-3 | 清空标注可 undo 恢复 | 清空前全部推入 redoStack |
| INV-4 | deleteSelected 无选中时 no-op | `selectedAnn === null` 早返回 |
| INV-5 | 录屏不显示荧光笔按钮 | AnnotationToolbar `showHighlight=false` prop |
| INV-6 | 橡皮擦是操作模式不产生 Annotation | Tool type 有 "eraser" 但 Annotation type 无 "eraser" |

## 7. 不做

- **橡皮擦单击删单个**（与 deleteSelected 重叠）
- **清空确认弹窗**（undo 是更好的安全网）
- **荧光笔矩形区域高亮**（用户选择自由画笔式）
