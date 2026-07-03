# 图片查看器性能优化设计

> 日期：2026-07-03
> 状态：📋 设计中
> 前置：`docs/superpowers/specs/2026-07-01-image-preview-design.md`（图片预览初版，已完成）
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
- 双 canvas 分层（bgCanvas + drawCanvas），绘制操作增量追加到顶层
- 缩放时 `createImageBitmap` 异步预缩放底图
- 大图先 thumb 再 full 渐进加载
- 缩略图显示期间 fit-to-window 自适应缩放
- 相关重构：`draw()` 拆分为 `drawBg()` + `drawActive()`，缩放逻辑调优

**不做（YAGNI）：**
- 多级缩放位图缓存（用户不会连续缩放十几个级别，单级异步足够）
- WebP 渐进解码（浏览器行为不可控）
- 后端变更（`get_image_thumb`、`get_image_full` 已有，无需新增命令）
- 滚轮缩放 / 触控手势（当前只有按钮缩放，不做新交互）

## 3. 架构

### 3.1 双 canvas 分层（解决 C）

**核心思路**：将「已确认内容」和「正在绘制的内容」分离到两个叠放的 canvas 上，mousemove 只操作顶层（增量追加），不触发底层重绘。

```
┌─ drawCanvas（顶层，透明底）────────────────┐  ← 正在绘制的笔迹/形状预览
│  mousedown→mousemove: 增量追加线段（pen）     │     mouseup 时提交→清空
│  mousedown→mousemove: 清空→重画当前形状（其他）  │
└──────────────────────────────────────────────┘
┌─ bgCanvas（底层，底图+已确认标注）────────────┐  ← imageId/zoom/annotations 变化时重绘
│  drawImage(预缩放位图) + drawAnnotation(anns)  │
└──────────────────────────────────────────────┘
```

**HTML 结构**（两个 canvas 叠放在同一个 relative 容器内）：

```tsx
<div className="relative" style={{ width: dispW, height: dispH }}>
  <canvas ref={bgCanvasRef} className="absolute inset-0 block" style={{ width: dispW, height: dispH }} />
  <canvas ref={drawCanvasRef} className="absolute inset-0 block" style={{ width: dispW, height: dispH, cursor: ... }}
    onMouseDown={onMouseDown} onMouseMove={onMouseMove} onMouseUp={onMouseUp} />
  <img ref={imgRef} ... /> {/* display:none，解码源 */}
</div>
```

**事件绑定**：pointer 事件只绑 drawCanvas（顶层），穿透无需处理（drawCanvas 透明区自然穿透到 bgCanvas 视觉上，事件已由顶层捕获）。

**draw 拆分**：

```ts
// drawBg：imageId / zoom / annotations 变化时调用（含 createImageBitmap 预缩放）
const drawBg = useCallback(async () => {
  const bitmap = await createImageBitmap(img, {
    resizeWidth: natW * zoom * dpr,
    resizeHeight: natH * zoom * dpr,
  });
  bgCanvas.width = bitmap.width;
  bgCanvas.height = bitmap.height;
  bgCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
  bgCtx.drawImage(bitmap, 0, 0, dispW, dispH);
  bgCtx.save();
  bgCtx.scale(zoom, zoom);
  for (const ann of annotations) drawAnnotation(bgCtx, ann);
  bgCtx.restore();
  bitmap.close();
}, [natW, natH, zoom, annotations]);

// drawActive：mousemove 调用，只操作 drawCanvas
const drawActive = useCallback(() => {
  drawCanvas.width = dispW * dpr;  // 清空
  drawCanvas.height = dispH * dpr;
  drawCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
  if (drawingRef.current) {
    drawCtx.save();
    drawCtx.scale(zoom, zoom);
    drawAnnotation(drawCtx, drawingRef.current);
    drawCtx.restore();
  }
}, [dispW, dispH, zoom]);
```

**mouseup 提交流程**：

```ts
const onMouseUp = () => {
  if (drawingRef.current) {
    const ann = drawingRef.current;
    drawingRef.current = null;
    // 过滤误触后入 annotations
    if (ok) setAnnotations(prev => [...prev, ann]);
    else drawActive(); // 清掉不合法的绘制预览
  }
  dragRef.current = null;
};
```

annotations state 更新 → useEffect 触发 drawBg → 底层重绘含新标注，drawCanvas 无需额外清空（下次 drawActive 会清）。

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
  const containerW = window.innerWidth - FIT_PADDING;  // FIT_PADDING=96（p-12 = 48px×2）
  const containerH = window.innerHeight - FIT_PADDING;
  return Math.min(1, containerW / w, containerH / h);
};
```

**默认行为变化**：
- 原设计：默认 zoom=1（1:1 自然分辨率），超出窗口出滚动条
- 新设计：首次打开时 fit-to-window（zoom < 1 时缩放显示，无滚动条），用户手动缩放后尊重用户选择
- 用 `userZoomedRef` 标记用户是否手动改过 zoom：thumb→full 替换时只在用户未手动缩放时重算 fitZoom

## 4. 拖动标注（drag）与抓手平移（pan）的处理

**拖动已确认标注**（tool=none + hitTest 命中）：
- mousemove 更新 annotation 坐标 → `setAnnotations` → useEffect 触发 drawBg 重绘底层
- 顶层 drawCanvas 不参与（无正在绘制的内容）
- 此场景无增量优化，但标注数量通常很少（<20），全量重绘成本低

**抓手平移**（tool=none + 未命中标注 + 按住拖拽）：
- 只操作 `scrollContainerRef.scrollLeft/Top`，不触发任何 canvas 重绘
- 已有实现无性能问题，无需改动

## 5. 边界情况

- **zoom 期间快速连续点击**：每个 zoom 值都触发 createImageBitmap，中间结果被最新的一次覆盖（旧 bitmap 在 drawBg 中 close）。最终只有最新 zoom 值的位图画上 bgCanvas。
- **thumb→full 替换期间用户正在绘制标注**：thumb 和 full 的 `naturalWidth/Height` 不同（thumb 是缩略图尺寸），如果允许在 thumb 期间画标注，坐标会存在 thumb 坐标系，full 加载后被重新诠释为 full 坐标系 → 标注错位。**解决方案**：`loadingFullRef` 门控——全图加载完成前禁止标注（`onMouseDown` 中 `loadingFullRef.current && tool !== "none"` 时 return）。用户在此期间只能选择/平移，不能新建标注。
- **drawCanvas 尺寸同步**：dispW/dispH 变化时（zoom 变化）两个 canvas 的 CSS 尺寸同步更新，bgCanvas 的像素尺寸由 drawBg 内部设，drawCanvas 的像素尺寸由 drawActive 设。
- **撤退路径**：如果 `createImageBitmap` 不支持（极老浏览器），fallback 到当前直接 `drawImage` 方式（原逻辑不变）。
- **blob URL 泄漏**：`objectUrlRef` 跟踪当前全图的 objectURL，图片切换时 `revokeObjectURL` 旧的、unmount cleanup effect 兜底 revoke + `bitmap.close()`。
- **EXIF 条显示 thumb 尺寸**：`fullNatW/fullNatH` state 仅在全图 onload 后赋值，EXIF 条用 `fullNatW || natW`——thumb 期间不显示缩略图尺寸。

## 6. 不变量

1. 标注坐标始终在**自然像素空间**（与 zoom 解耦），zoom 变化/底图替换不影响标注正确性
2. drawCanvas 始终是"正在绘制中"的临时层，mouseup 后内容要么入 annotations 要么被清空
3. bgCanvas 是"已确认状态"的权威渲染，imageId/zoom/annotations 任一变化都触发完整重绘
4. composePngBytes（保存/复制）不受双 canvas 影响，仍在独立 offscreen canvas 上自然尺寸 1:1 合成
