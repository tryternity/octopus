# 图片查看器性能优化实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实施。步骤用 checkbox（`- [ ]`）跟踪。

**Goal:** 优化 ImagePreview 的画笔绘制流畅度（双 canvas 增量绘制）、缩放响应速度（createImageBitmap 异步预缩放）、大图打开体验（先 thumb 再 full 渐进加载）。

**Architecture:** 单 canvas → 双 canvas（bgCanvas 底层 + drawCanvas 顶层）分层绘制。draw 拆为 drawBg（底图+已确认标注）+ drawActive（正在绘制的笔迹预览）。zoom 变化走 createImageBitmap 异步预缩放。图片加载改为先 thumb 后 full 的渐进式。

**Tech Stack:** React 19 + TypeScript + Canvas 2D API（`createImageBitmap`）+ Tauri IPC（`get_image_thumb` / `get_image_full` 已有）。无后端变更。前端无 vitest（项目惯例：`npm run build` 类型检查 + 手动 e2e）。

## Global Constraints

- 工作目录：`<WT>` = `/Users/wudarui/workspace/agent/octopus/.worktrees/image-viewer-perf`
- 分支：`image-viewer-perf`，不往 main 同步
- dist 已纳入 git：前端变更必须 `npm run build` 并提交 `crates/desktop/dist`
- 标注坐标始终在自然像素空间（与 zoom 解耦）
- composePngBytes（保存/复制）不受双 canvas 影响，仍在 offscreen canvas 自然尺寸 1:1 合成
- 不新增后端命令（`get_image_thumb` 和 `get_image_full` 已有）
- Spec 路径：`docs/superpowers/specs/2026-07-03-image-viewer-perf-design.md`

## 文件结构

| 文件 | 责任 | 动作 |
|------|------|------|
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 预览主组件：双 canvas、draw 拆分、渐进加载、缩放逻辑 | 改（主要文件） |

仅改一个文件。Toolbar.tsx 不变（zoom 按钮行为不变，只是底层数据来源从直接 drawImage 改为 createImageBitmap）。后端不变。

---

## Task 1: 双 canvas 分层 + draw 拆分

**Files:**
- Modify: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**Interfaces:**
- Consumes: 现有 `annotation.ts`（`drawAnnotation`、`hitTestAnnotationPrecise`）
- Produces: 新内部函数 `drawBg`、`drawActive`、`clearDrawCanvas`，新 ref `bgCanvasRef`、`drawCanvasRef`、`scaledBitmapRef`

本任务将单个 canvas 拆为 bgCanvas（底图+已确认标注）+ drawCanvas（正在绘制的笔迹），所有标注工具的实时预览统一走 drawCanvas 增量绘制。这是三个优化点的骨架，后续 Task 2/3 在此基础上叠加。

- [ ] **Step 1：替换 canvas refs + HTML 结构**

删除 `canvasRef`，新增 `bgCanvasRef` + `drawCanvasRef`：

```ts
// 删除:
// const canvasRef = useRef<HTMLCanvasElement>(null);

// 新增:
const bgCanvasRef = useRef<HTMLCanvasElement>(null);
const drawCanvasRef = useRef<HTMLCanvasElement>(null);
```

HTML 中 canvas 区域改为两个叠放 canvas + 棋盘格底背景移到容器 div（两个 canvas 共享）：

```tsx
{/* canvas wrapper：relative 让 textarea 相对 canvas 定位、随滚动移动 */}
<div className="relative" style={{
  width: dispW || undefined, height: dispH || undefined,
  // 棋盘格底从 canvas 移到容器，两个 canvas 都能看到
  backgroundColor: "#292524",
  backgroundImage:
    "linear-gradient(45deg, #1c1917 25%, transparent 25%)," +
    "linear-gradient(-45deg, #1c1917 25%, transparent 25%)," +
    "linear-gradient(45deg, transparent 75%, #1c1917 75%)," +
    "linear-gradient(-45deg, transparent 75%, #1c1917 75%)",
  backgroundSize: "20px 20px",
  backgroundPosition: "0 0, 0 10px, 10px -10px, -10px 0px",
}}>
  {/* 底层：底图 + 已确认标注 */}
  <canvas
    ref={bgCanvasRef}
    className="absolute inset-0 block"
    style={{ width: dispW, height: dispH }}
  />
  {/* 顶层：正在绘制的笔迹/形状预览；pointer 事件绑此层 */}
  <canvas
    ref={drawCanvasRef}
    className="absolute inset-0 block"
    style={{
      width: dispW, height: dispH,
      cursor: tool === "none" ? (panning ? "grabbing" : "grab") : "crosshair",
    }}
    onMouseDown={onMouseDown}
    onMouseMove={onMouseMove}
    onMouseUp={onMouseUp}
  />
  {/* img 解码源 + textarea 不变 */}
  {dataUrl && (
    <img
      ref={imgRef}
      src={dataUrl}
      alt=""
      crossOrigin="anonymous"
      style={{ display: "none" }}
      onLoad={(e) => {
        setNatW(e.currentTarget.naturalWidth);
        setNatH(e.currentTarget.naturalHeight);
      }}
    />
  )}
  {draftBox && (
    <textarea ... />  {/* 不变 */}
  )}
</div>
```

注意：原来 `<canvas>` 上的 `onMouseDown/onMouseMove/onMouseUp`、`cursor`、`backgroundColor`/`backgroundImage`/`backgroundSize`/`backgroundPosition` 全部移到 drawCanvas 或容器 div 上。

- [ ] **Step 2：拆分 draw → drawBg + drawActive**

删除现有 `draw` 函数和 `useEffect(() => { draw(); }, [draw]);`。新增：

```ts
const scaledBitmapRef = useRef<ImageBitmap | null>(null);

// 底层重绘：底图 + 已确认标注（imageId/zoom/annotations 变化时调用）
const drawBg = useCallback(() => {
  const canvas = bgCanvasRef.current;
  const img = imgRef.current;
  if (!canvas || !img || !natW || !natH) return;
  const dw = natW * zoom;
  const dh = natH * zoom;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(dw * dpr);
  canvas.height = Math.round(dh * dpr);
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, dw, dh);
  // 优先用预缩放位图（Task 2 会异步生成），fallback 原图
  const bitmap = scaledBitmapRef.current;
  ctx.drawImage(bitmap || img, 0, 0, dw, dh);
  // 标注：自然坐标 → ×zoom 缩放到显示空间
  ctx.save();
  ctx.scale(zoom, zoom);
  for (const ann of annotations) drawAnnotation(ctx, ann);
  ctx.restore();
}, [natW, natH, zoom, annotations]);

// 顶层重绘：仅正在绘制的笔迹/形状预览
const drawActive = useCallback(() => {
  const canvas = drawCanvasRef.current;
  if (!canvas || !natW || !natH) return;
  const dw = natW * zoom;
  const dh = natH * zoom;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(dw * dpr);   // 赋值即清空
  canvas.height = Math.round(dh * dpr);
  if (!drawingRef.current) return;
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.save();
  ctx.scale(zoom, zoom);
  drawAnnotation(ctx, drawingRef.current);
  ctx.restore();
}, [natW, natH, zoom]);

// bgCanvas 同步触发：imageId/zoom/annotations 任一变化 → 完整重绘底层
useEffect(() => { drawBg(); }, [drawBg]);
```

- [ ] **Step 3：改 onMouseMove / onMouseUp 用 drawActive**

`onMouseMove` 中 `draw()` 调用改为 `drawActive()`：

```ts
// 原代码 229-237 行区间:
if (drawingRef.current) {
  if (drawingRef.current.type === "pen" && drawingRef.current.points) {
    drawingRef.current.points.push([nx, ny]);
  } else {
    drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
  }
  drawActive();  // 原来是 draw()
}
```

`onMouseUp` 中无效绘制清除也用 `drawActive()`：

```ts
// 原代码 251 行:
} else {
  drawActive();  // 原来是 draw()
}
```

- [ ] **Step 4：改 canvasCoords 使用 drawCanvas ref**

`canvasCoords` 函数中 `canvasRef.current` 改为 `drawCanvasRef.current`：

```ts
const canvasCoords = (e: React.MouseEvent) => {
  const rect = drawCanvasRef.current!.getBoundingClientRect();
  return { cssX: e.clientX - rect.left, cssY: e.clientY - rect.top };
};
```

- [ ] **Step 5：改 composePngBytes 使用 imgRef**

`composePngBytes` 不变（它创建 offscreen canvas + drawImage(imgRef) + drawAnnotation，与 canvas 分层无关）。但确认其内部仍然引用 `imgRef.current`（已确认是正确的，不动）。

- [ ] **Step 6：类型检查 + 构建验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 构建成功，无 type error。

- [ ] **Step 7：提交**

```bash
cd <WT>
git add crates/desktop/frontend/src/pages/ImagePreview/index.tsx crates/desktop/dist
git commit -m "refactor(ImagePreview): 双 canvas 分层 — bgCanvas(底图+标注) + drawCanvas(绘制预览)"
```

---

## Task 2: createImageBitmap 异步预缩放（缩放优化）

**Files:**
- Modify: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**Interfaces:**
- Consumes: Task 1 的 `scaledBitmapRef`、`drawBg`
- Produces: 新 ref `zoomVersionRef`（防止过时 zoom 值覆盖最新帧）

- [ ] **Step 1：新增缩放预缩放逻辑**

在 `drawBg` 的 `useEffect` 之后，新增 zoom 变化时的异步预缩放 effect：

```ts
const zoomVersionRef = useRef(0);

// zoom 变化 → 异步生成预缩放位图 → drawBg（不阻塞主线程）
useEffect(() => {
  const img = imgRef.current;
  if (!img || !natW || !natH) return;
  const version = ++zoomVersionRef.current;
  const dw = natW * zoom;
  const dh = natH * zoom;
  const dpr = window.devicePixelRatio || 1;
  const pw = Math.round(dw * dpr);
  const ph = Math.round(dh * dpr);
  // 极小尺寸（zoom 接近 0）跳过
  if (pw < 1 || ph < 1) return;

  createImageBitmap(img, {
    resizeWidth: pw,
    resizeHeight: ph,
    resizeQuality: "high",
  }).then((bitmap) => {
    // 版本不匹配 → 用户已切换到另一个 zoom，丢弃
    if (version !== zoomVersionRef.current) {
      bitmap.close();
      return;
    }
    const old = scaledBitmapRef.current;
    scaledBitmapRef.current = bitmap;
    if (old) old.close();
    drawBg();
  }).catch(() => {});
}, [zoom, natW, natH]); // eslint-disable-line react-hooks/exhaustive-deps
```

注意：首次加载（img onload 后）也需要生成位图。这由 Task 3 的加载流程处理（img onload 后设 zoom → zoom effect 自动触发 createImageBitmap）。zoom=1 时 `createImageBitmap` 的 resizeWidth/Height 等于 canvas 像素尺寸，仍有意义——浏览器可 GPU 缩放替代 CPU drawImage 降采样。

- [ ] **Step 2：imageId 变化时清理旧位图**

在 imageId 变化的 useEffect（原代码 86-98 行）中，清理 scaledBitmapRef：

```ts
useEffect(() => {
  if (imageId == null) return;
  // 清理旧位图
  const old = scaledBitmapRef.current;
  scaledBitmapRef.current = null;
  if (old) old.close();
  zoomVersionRef.current++;
  // ... 后续 Task 3 会替换为 thumb+full 并行加载，此处暂保留原逻辑
  invoke<ArrayBuffer>("get_image_full", { id: imageId })
    .then((buf) => {
      const blob = new Blob([buf], { type: "image/webp" });
      const url = URL.createObjectURL(blob);
      setDataUrl(url);
      setAnnotations([]);
      setZoomSync(1);
    })
    .catch((e) => console.error(e));
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [imageId]);
```

- [ ] **Step 3：类型检查 + 构建验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 构建成功。

- [ ] **Step 4：提交**

```bash
cd <WT>
git add crates/desktop/frontend/src/pages/ImagePreview/index.tsx crates/desktop/dist
git commit -m "perf(ImagePreview): zoom 缩放走 createImageBitmap 异步预缩放，不阻塞主线程"
```

---

## Task 3: 先 thumb 再 full 渐进加载

**Files:**
- Modify: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**Interfaces:**
- Consumes: Task 1 的 `drawBg`、`imgRef`，Task 2 的 `zoomVersionRef`、`scaledBitmapRef`
- Produces: 新 ref `userZoomedRef`（标记用户是否手动缩放过）、新 state `loading`（加载中指示）

- [ ] **Step 1：新增 fit-to-window 计算函数 + userZoomedRef**

在组件顶部（常量区后）新增：

```ts
// fit-to-window：图片完整显示在窗口内，最大不超过 1:1
const computeFitZoom = (w: number, h: number): number => {
  const containerW = window.innerWidth - 24;  // 减去两侧 padding
  const containerH = window.innerHeight - 24;
  return Math.min(1, containerW / w, containerH / h);
};
```

新增 ref（标记用户是否手动改过 zoom）：

```ts
const userZoomedRef = useRef(false);
```

修改 `setZoomSync`，标记用户手动操作：

```ts
const setZoomSync = (z: number, userInitiated = false) => {
  const clamped = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, z));
  zoomRef.current = clamped;
  if (userInitiated) userZoomedRef.current = true;
  setZoom(clamped);
};
```

修改 `zoomIn`/`zoomOut`/`zoomReset` 传 `userInitiated=true`：

```ts
const zoomIn = () => setZoomSync(zoomRef.current * ZOOM_STEP, true);
const zoomOut = () => setZoomSync(zoomRef.current / ZOOM_STEP, true);
const zoomReset = () => setZoomSync(1, true);
```

- [ ] **Step 2：替换 imageId 加载逻辑为并行 thumb + full**

将现有 imageId useEffect（原代码 86-98 行）替换为：

```ts
// —— imageId 变 → 并行拉缩略图（秒开）+ 全图（异步替换） ——
useEffect(() => {
  if (imageId == null) return;
  // 清理旧资源
  const old = scaledBitmapRef.current;
  scaledBitmapRef.current = null;
  if (old) old.close();
  zoomVersionRef.current++;
  userZoomedRef.current = false;  // 新图重置用户缩放标记
  drawingRef.current = null;
  setAnnotations([]);
  setNatW(0);
  setNatH(0);

  // 拉缩略图（秒开）和全图（异步）
  const thumbPromise = invoke<string>("get_image_thumb", { id: imageId });
  const fullPromise = invoke<ArrayBuffer>("get_image_full", { id: imageId });

  // 缩略图先到 → 立即显示
  thumbPromise.then((thumbDataUrl) => {
    // 用户可能已关闭或切换到另一张图
    if (imgRef.current?.src === thumbDataUrl) return;
    const thumbImg = new Image();
    thumbImg.crossOrigin = "anonymous";
    thumbImg.onload = () => {
      imgRef.current = thumbImg;
      setDataUrl(thumbDataUrl);
      const fitZoom = computeFitZoom(thumbImg.naturalWidth, thumbImg.naturalHeight);
      setNatW(thumbImg.naturalWidth);
      setNatH(thumbImg.naturalHeight);
      setZoomSync(fitZoom);
    };
    thumbImg.src = thumbDataUrl;
  }).catch((e) => console.error("thumb failed:", e));

  // 全图后到 → 无缝替换
  fullPromise.then((buf) => {
    const blob = new Blob([buf], { type: "image/webp" });
    const url = URL.createObjectURL(blob);
    const fullImg = new Image();
    fullImg.crossOrigin = "anonymous";
    fullImg.onload = () => {
      imgRef.current = fullImg;
      setDataUrl(url);
      // 全图尺寸可能与缩略图不同 → 重算（仅在用户未手动缩放时）
      if (!userZoomedRef.current) {
        const fitZoom = computeFitZoom(fullImg.naturalWidth, fullImg.naturalHeight);
        setNatW(fullImg.naturalWidth);
        setNatH(fullImg.naturalHeight);
        setZoomSync(fitZoom);
      } else {
        setNatW(fullImg.naturalWidth);
        setNatH(fullImg.naturalHeight);
      }
      // 强制重新生成位图（全图替换缩略图后 zoom 可能不变，不触发 zoom effect）
      const oldBitmap = scaledBitmapRef.current;
      scaledBitmapRef.current = null;
      if (oldBitmap) oldBitmap.close();
      zoomVersionRef.current++;
    };
    fullImg.src = url;
  }).catch((e) => console.error("full failed:", e));

  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [imageId]);
```

注意关键设计：
- 缩略图和全图各自创建独立 `new Image()`，不共享 `<img>` DOM 元素
- `imgRef.current` 指向当前最新的 Image 对象（drawBg/drawActive 都通过 imgRef 获取底图）
- `dataUrl` 更新触发 React 中的 `<img>` 渲染（仍需 DOM img 用于 `crossOrigin` 等属性），但 `imgRef` 是实际绘制源
- 全图替换时如果 zoom 未变（computeFitZoom 返回值与缩略图时相同），zoom effect 不触发 createImageBitmap——所以手动 `zoomVersionRef.current++` 强制触发

- [ ] **Step 3：修改 dataUrl 的 img DOM 渲染**

现有 JSX 中 `<img ref={imgRef} src={dataUrl} ...>` 的 `onLoad` 回调需要适配——现在 natW/natH 由 useEffect 内直接设置，`onLoad` 不再需要设置它们。但保留 `onLoad` 用于安全兜底（React 渲染的 img 与 imgRef 不是同一个时）：

```tsx
{dataUrl && (
  <img
    ref={imgRef}
    src={dataUrl}
    alt=""
    crossOrigin="anonymous"
    style={{ display: "none" }}
    onLoad={(e) => {
      // natW/natH 已在 useEffect 加载流程中设置
      // 此处仅兜底：确保 imgRef.current 指向 React 渲染的最新 img
      if (!imgRef.current || imgRef.current !== e.currentTarget) {
        imgRef.current = e.currentTarget;
      }
    }}
  />
)}
```

- [ ] **Step 4：更新 load 事件处理器**

`listen("image-preview://load")` 回调中，重置 `userZoomedRef`：

```ts
const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
  setImageId(e.payload.imageId);
  // setAnnotations 和 setZoomSync 已在 imageId useEffect 中处理
});
```

无需额外清理——imageId useEffect 的清理逻辑已覆盖。

- [ ] **Step 5：类型检查 + 构建验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 构建成功。

- [ ] **Step 6：提交**

```bash
cd <WT>
git add crates/desktop/frontend/src/pages/ImagePreview/index.tsx crates/desktop/dist
git commit -m "perf(ImagePreview): 先 thumb 再 full 渐进加载 + fit-to-window 自适应缩放"
```

---

## Task 4: 文档同步 + 自查

**Files:**
- Modify: `<WT>/docs/architecture.md`（ImagePreview 性能优化说明）

- [ ] **Step 1：更新 architecture.md**

在 ImagePreview 相关章节补充双 canvas 架构 + 性能优化说明。

- [ ] **Step 2：更新实施计划（本文件）**

回写实际偏差到计划文档——plan 是「实施记录」而非「一次性待办」。

- [ ] **Step 3：最终构建验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 构建成功，无 error。

- [ ] **Step 4：最终提交**

```bash
cd <WT>
git add docs/
git commit -m "docs: 同步图片查看器性能优化到 architecture.md"
```
