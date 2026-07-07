# 图片查看器性能优化设计

> 日期：2026-07-03
> 状态：📋 设计中
> 前置：`docs/superpowers/specs/2026-07-01-image-preview-design.md`（图片预览初版，已完成，已删除，git history 可追溯）
> 分支：`image-viewer-perf`

## 1. 背景与目标

图片预览窗（ImagePreview）已具备完整功能（画布、标注、缩放、保存/复制/OCR）。实际使用中发现三个性能痛点，按优先级排列：

| 优先级 | 问题 | 症状 | 根因 |
|--------|------|------|------|
| **C** | 画笔拖动掉帧 | pen tool mousemove 时画面卡顿、笔迹不连贯 | 每帧全量重绘（底图 + 全部标注 + 正在画的笔迹） |
| **B** | 缩放操作卡顿 | zoom 按钮点击或快速缩放时白屏闪烁 | 每次 zoom 变更重设 `canvas.width/height`（清屏）+ 原尺寸 `drawImage` 光栅化 |
| **A** | 大图打开白屏 | 4K 图片打开后 200-500ms 空白等待 | 单步加载链：IPC 拉全图 → 解码 → 首次重绘，无中间态 |

## 2. 范围

**做：**
- 视口渲染 canvas（固定窗口大小 ~2M 像素，只画可见区域）+ SVG overlay（标注）
- 缩放时 `createImageBitmap` 异步预缩放底图（150ms debounce）
- 大图先 thumb 再 full 渐进加载（`fullLoadedRef` 防竞态降级）
- fit-to-width 默认 + fitModeRef 三态（fitWidth/fitWindow/manual）+ ResizeObserver
- 标注工具扩展：序号/马赛克/菱形 + 实心填充 + redo
- 属性浮窗交互：点击工具弹出，画布操作时自动收起，再点按钮重新弹出
- 图标统一为截图 SVG 风格（icons/*.svg）

**不做（YAGNI）：**
- 多级缩放位图缓存（单级异步 + debounce 足够）
- WebP 渐进解码（浏览器行为不可控）
- 后端变更（`get_image_thumb`、`get_image_full` 已有）
- 滚轮缩放 / 触控手势（当前只有按钮缩放）
- OCR 文本块可视化（独立需求）

## 3. 架构

### 3.1 单 canvas（底图）+ SVG overlay（标注）= 标注零 canvas 开销

**演进**：单 canvas（整张图）→ SVG overlay（标注脱离 canvas）→ 视口渲染（canvas 在 wrapper 内只画可见区域，与 SVG 同一 scroll context 零晃动）。

```tsx
<div className="relative" style={{ width: dispW, height: dispH, ...棋盘格 }}>
  <canvas ref={bgCanvasRef} className="absolute inset-0 block"         // 只画底图
    style={{ width: dispW, height: dispH, cursor: ... }}
    onMouseDown={onMouseDown} onMouseMove={onMouseMove} onMouseUp={onMouseUp} />
  <svg className="absolute inset-0 block"                                  // 标注 overlay
    viewBox={`0 0 ${natW} ${natH}`} preserveAspectRatio="none"
    style={{ width: dispW, height: dispH, pointerEvents: "none" }}>
    {annotations.map((ann, i) => <AnnotationSvg key={i} ann={ann} />)}
    {draftAnn && <AnnotationSvg ann={draftAnn} />}
  </svg>
</div>
```

**SVG overlay 原理**：SVG 元素（`<rect>`/`<ellipse>`/`<line>`/`<polyline>`/`<text>`）由浏览器合成器独立处理，不参与 canvas GPU 合成。标注变化（增删改、实时预览）只更新 SVG DOM 属性，零 canvas 操作。

**坐标系统**：SVG `viewBox="0 0 natW natH"` + `preserveAspectRatio="none"` → SVG 内部坐标 = 自然像素空间，CSS 尺寸 = `dispW×dispH`。标注坐标定义不变（自然像素），SVG 自动缩放。

**bgCanvas（视口渲染）**：canvas 固定窗口大小，只画可见区域的图片部分。滚动时 RAF 触发 drawBg 裁剪可见区域。所有定位用 absolute + JS 手算（不依赖 flex 居中）。

```ts
const drawBg = useCallback(() => {
  const sl = sc.scrollLeft, st = sc.scrollTop;
  const vw = sc.clientWidth, vh = sc.clientHeight;
  canvas.width = vw * dpr; canvas.height = vh * dpr;  // canvas = 窗口大小
  // 图片在 viewport 中的位置（手算）
  const imgVpX = imgLeft - sl;   // imgLeft = JS 算的居中位置
  const imgVpY = imgTop - st;    // imgTop = 56px（工具栏空间）
  // 裁剪可见区域 → drawImage
  const visL = Math.max(0, -imgVpX), visT = Math.max(0, -imgVpY);
  const visR = Math.min(dispW, vw - imgVpX), visB = Math.min(dispH, vh - imgVpY);
  ctx.drawImage(bitmap || img,
    (visL/dispW)*srcW, (visT/dispH)*srcH,     // 源裁剪
    ((visR-visL)/dispW)*srcW, ((visB-visT)/dispH)*srcH,
    visL+imgVpX, visT+imgVpY,                   // 目标位置
    visR-visL, visB-visT);
}, [natW, natH, zoom, viewport, imgLeft, imgTop, dispW, dispH]);
```

**布局**（彻底放弃 flex，所有定位 absolute + JS 手算）：
- canvas 在 scrollContainer **外面**（兄弟节点），`absolute inset-0 pointer-events:none zIndex:1`
- scrollContainer 内有 content div（relative，撑滚动条）+ wrapper（absolute，手算 left/top 居中）
- wrapper 透明背景（不遮 canvas），含 SVG overlay + 鼠标事件
- `viewport` state（ResizeObserver）触发 drawBg；滚动 RAF 直接调 `drawBg()`（不走 React state，避免全组件重渲染）

**实时预览**：正在绘制的标注存为 `draftAnn` state（React），mousemove 时 `setDraftAnn(...)` → React 只渲染一个 SVG 元素的属性 diff。mouseup 后 `draftAnn` 入 `annotations` 或清空。

### 3.2 createImageBitmap 异步预缩放（解决 B）

**核心思路**：zoom 变化时不再在主线程对原图做 `drawImage` 到超大画布，而是先异步生成预缩放 `ImageBitmap`，再画到 bgCanvas。

**流程**：

```
zoom 变化
  → useEffect([zoom])
    → setIsScaling(true)  // 可选：显示缩放中态
    → createImageBitmap(img, { resizeWidth, resizeHeight })  // 异步，GPU 加速
      .then(bitmap => {
        scaledBitmapRef.current = bitmap;
        drawBg(bitmap);  // 用预缩放位图重绘 bgCanvas
        setIsScaling(false);
      });
    → 期间 bgCanvas 保持上一帧（不闪白）
```

**关键细节**：
- `createImageBitmap` 接受 `{ resizeWidth, resizeHeight }` 选项，浏览器用 GPU 做缩放，不占主线程。
- 返回的 `ImageBitmap` 已经是目标像素尺寸，`bgCtx.drawImage(bitmap, 0, 0, dispW, dispH)` 只做像素拷贝，无额外缩放开销。
- 旧 `ImageBitmap` 必须 `.close()` 释放 GPU 内存（`drawBg` 末尾调用）。
- 首次加载（imageId 变化）也走同一条路径：img onload → createImageBitmap → drawBg。
- 缩放期间 bgCanvas 保持上一帧内容（不清空），用户看到的是"稍糊但完整"的画面瞬间切换到清晰——比白屏好几个量级。

### 3.3 先 thumb 再 full 渐进加载（解决 A）

**核心思路**：mount 时并行拉缩略图和全图，缩略图秒开作为占位，全图就绪后无缝替换。

**流程**：

```
imageId 变化
  → 并行发起两个 invoke:
    1. get_image_thumb → 缩略图 data URL（小，~10ms）
    2. get_image_full → 全图 blob（大，100-500ms）

  → 缩略图先到：
    → img.src = thumbUrl
    → img.onload → setNatW/H（缩略图尺寸）
    → computeFitZoom() → setZoom(fitZoom)
    → createImageBitmap(img) → drawBg()  // 模糊但可见

  → 全图后到：
    → revokeObjectURL(oldThumbUrl)
    → img.src = fullBlobUrl
    → img.onload → setNatW/H（全图尺寸）
    → 全图尺寸与缩略图不同 → 重算 fitZoom（如果用户未手动缩放）
    → createImageBitmap(img) → drawBg()  // 清晰全图替换
```

**fit-to-window 计算**：

```ts
const computeFitZoom = (w: number, h: number): number => {
  const containerW = window.innerWidth - FIT_PADDING;  // FIT_PADDING=16（px-2 左右各 8px）
  const containerH = window.innerHeight - FIT_PADDING;
  return Math.min(1, containerW / w, containerH / h);
};
```

**默认行为变化**：
- 原设计：默认 zoom=1（1:1 自然分辨率），超出窗口出滚动条
- 新设计：首次打开时 fit-to-window（zoom < 1 时缩放显示，无滚动条），用户手动缩放后尊重用户选择
- 用 `userZoomedRef` 标记用户是否手动改过 zoom：thumb→full 替换时只在用户未手动缩放时重算 fitZoom
- **fit 模式跟踪**（`fitModeRef`）：`'fitWindow'` | `'fitWidth'` | `'manual'`。打开图片默认 `fitWidth`，点自适应窗口按钮切 `fitWindow`，手动缩放切 `manual`。
- **自适应宽度**（fit-to-width）：图片宽度 = 窗口宽度（`containerW / natW`），高度可超出窗口 → 垂直滚动。工具栏 `MoveHorizontal` 按钮触发。自适应窗口 = `Expand` 按钮。
- **ResizeObserver 自适应**：窗口 resize 时，若 `fitModeRef` 非 `manual`，按当前 fit 模式自动重算 zoom（fitWindow 重算 fitZoom，fitWidth 重算 fitToWidthZoom）。手动缩放后不再自动调整。

## 4. 拖动标注（drag）与抓手平移（pan）的处理

**拖动已确认标注**（tool=none + hitTest 命中）：
- mousemove 更新 annotation 坐标 → `setAnnotations` → React 重新渲染被拖动的 SVG 元素
- canvas 不参与（标注由 SVG overlay 渲染）

**抓手平移**（tool=none + 未命中标注 + 按住拖拽）：
- 只操作 `scrollContainerRef.scrollLeft/Top`，不触发任何 canvas 重绘
- 已有实现无性能问题，无需改动

## 5. 边界情况

- **zoom 期间快速连续点击**：每个 zoom 值都触发 createImageBitmap，中间结果被最新的一次覆盖（旧 bitmap 在 drawBg 中 close）。最终只有最新 zoom 值的位图画上 bgCanvas。
- **thumb→full 替换期间用户正在绘制标注**：thumb 和 full 的 `naturalWidth/Height` 不同（thumb 是缩略图尺寸），如果允许在 thumb 期间画标注，坐标会存在 thumb 坐标系，full 加载后被重新诠释为 full 坐标系 → 标注错位。**解决方案**：`loadingFullRef` 门控——全图加载完成前禁止标注（`onMouseDown` 中 `loadingFullRef.current && tool !== "none"` 时 return）。用户在此期间只能选择/平移，不能新建标注。
- **SVG overlay 尺寸同步**：dispW/dispH 变化时（zoom 变化）wrapper 的 CSS 尺寸同步更新。SVG 的 viewBox 不变（自然空间），浏览器自动按 CSS 尺寸缩放内容。canvas 保持窗口大小，只裁剪可见区域。
- **撤退路径**：如果 `createImageBitmap` 不支持（极老浏览器），fallback 到当前直接 `drawImage` 方式（原逻辑不变）。
- **blob URL 泄漏**：`objectUrlRef` 跟踪当前全图的 objectURL，图片切换时 `revokeObjectURL` 旧的、unmount cleanup effect 兜底 revoke + `bitmap.close()`。
- **EXIF 条显示 thumb 尺寸**：`fullNatW/fullNatH` state 仅在全图 onload 后赋值，EXIF 条用 `fullNatW || natW`——thumb 期间不显示缩略图尺寸。

## 6. 不变量

1. 标注坐标始终在**自然像素空间**（与 zoom 解耦），zoom 变化/底图替换不影响标注正确性
2. 标注是纯数据（`Annotation[]`），渲染由 SVG overlay 处理，不碰底图像素
3. canvas 固定窗口大小（视口渲染），只画可见区域——不管图多大，canvas 恒定 ~2M 像素 / ~8MB buffer
4. 所有定位用 absolute + JS 手算，不依赖 CSS flex 居中推导
5. composePngBytes（保存/复制）不受影响，仍在独立 offscreen canvas 上自然尺寸 1:1 合成（`drawImage(原图) + drawAnnotation(ann)`，仅保存时执行）

## 7. 与 Screenshot 标注的关系

**共享层**：`lib/annotation.ts`（`Annotation`/`Tool` 类型 + `drawAnnotation`/`drawAnnotationScaled`/`hitTestAnnotationPrecise`/`annBounds` 纯函数）——两端的标注数据模型、绘制逻辑、命中检测完全统一。

**渲染层不共享**（各自实现）：

| | ImagePreview | Screenshot |
|---|---|---|
| 渲染方式 | 视口渲染 canvas（窗口大小）+ **SVG overlay**（标注） | 单 canvas（底图+标注全量重绘） |
| 坐标空间 | **自然像素**（图像本征分辨率） | **窗口显示空间**（`window.innerWidth` 系） |
| canvas 尺寸 | 恒定窗口大小 ~2M 像素 | 屏幕尺寸 ~2M 像素 |

**不统一的原因**：Screenshot 的 canvas 是屏幕尺寸（~2M 像素），全量重绘 <1ms，无性能问题；且截图涉及选区裁剪逻辑，改造量大。为 DRY 承担大改动 + 回归风险换不到可感知的性能提升。当前的共享边界（纯函数 DRY + 渲染各自实现）是合理的。
