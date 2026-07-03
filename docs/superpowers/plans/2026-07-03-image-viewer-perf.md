# 图片查看器性能优化实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实施。步骤用 checkbox（`- [ ]`）跟踪。

**Goal:** 优化 ImagePreview 的画笔绘制流畅度（双 canvas 增量绘制）、缩放响应速度（createImageBitmap 异步预缩放）、大图打开体验（先 thumb 再 full 渐进加载）。

**Architecture:** 单 canvas → 双 canvas（bgCanvas 底层 + drawCanvas 顶层）分层绘制。draw 拆为 drawBg（底图+已确认标注）+ drawActive（正在绘制的笔迹预览）。zoom 变化走 createImageBitmap 异步预缩放。图片加载改为先 thumb 后 full 的渐进式。

**Tech Stack:** React 19 + TypeScript + Canvas 2D API（`createImageBitmap`）+ Tauri IPC（`get_image_thumb` / `get_image_full` 已有）。无后端变更。前端无 vitest（项目惯例：`npm run build` 类型检查 + 手动 e2e）。

## Global Constraints

- 工作目录：`<WT>` = `/Users/wudarui/workspace/agent/octopus/.worktrees/image-viewer-perf`
- 分支：`image-viewer-perf`，不往 main 同步
- dist 已 gitignore（**计划有误**：plan 原写「dist 已纳入 git」，实际 `.gitignore` 排除了 `/crates/desktop/dist/`）。前端变更只需 `npm run build` 验证类型+构建，**不提交 dist**。
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
  const containerW = window.innerWidth - FIT_PADDING;  // FIT_PADDING=96
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

---

## 实施记录（回写）

### Task 1 实施偏差

- **Review 修复**：mouseup 成功提交标注后未清空 drawCanvas → 补 `drawActive()` 调用（`drawingRef` 已 null 时 early-return 但 `canvas.width` 赋值已清空画布）。
- **dist 不提交**：`.gitignore` 排除 `/crates/desktop/dist/`，plan 原描述有误，实际只提交源文件。

### Task 2 实施偏差

- 无偏差，按 plan 逐字实施。Review Approved。

### Task 3 实施偏差

- **Review 修复 1（race condition）**：plan 原文用 `if (imgRef.current?.src === thumbDataUrl) return` 防重复，但快速切换图片时旧 promise 的 onload 仍会覆盖新图。改为 effect 内 `let cancelled = false` + cleanup `return () => { cancelled = true }`，thumb/full 的 `.then` 和 `onload` 均检查 `cancelled`。
- **Review 修复 2（drawBg 边界）**：thumb 和 full 同尺寸 + 用户未缩放时，`setNatW/H` + `setZoomSync` 无状态变化 → `drawBg` 的 useCallback 不重建 → useEffect 不触发 → canvas 保留已 close 的旧 bitmap。在 full onload 末尾补 `drawBg()` 显式重绘。
- **`loading` state 未实现**：plan Step 1 "Produces" 声明了 `loading` state 但无任何 Step 使用它，跳过（YAGNI）。

### 最终 review 后追加修复

- **thumb→full 标注坐标系错位（Important）**：thumb 的 `naturalWidth/Height` ≠ full，thumb 期间画的标注在全图加载后坐标系错位。改为 `loadingFullRef` 门控——全图加载完成前 `onMouseDown` 禁止标注（`tool !== "none"` 时 return）。
- **computeFitZoom padding 修正（Minor）**：原 plan `-24` 不匹配实际容器 `p-12`（48px×2=96px），改为 `FIT_PADDING=96`。
- **blob URL 泄漏（Minor）**：`objectUrlRef` 跟踪当前 objectURL，图片切换/卸载时 `revokeObjectURL`。unmount cleanup effect 兜底 revoke + `bitmap.close()`。
- **EXIF 条显示 thumb 尺寸（Minor）**：新增 `fullNatW/fullNatH` state，EXIF 条用 `fullNatW || natW`，thumb 期间不显示缩略图尺寸。

### 架构演进：双 canvas → SVG overlay（用户反馈驱动）

原 plan 设计为双 canvas（bgCanvas + drawCanvas）。实施后用户测试 2032×15796 超长图仍感觉标注卡顿，经历三轮迭代：

1. **RAF 节流 + 跳过无变化 canvas 尺寸重设**（commit `203be9d`）——稍好但仍慢
2. **pen 增量线段 + shape 脏区域重绘**（commit `6379614`）——再好一点，但 drawCanvas 的 GPU 合成 45M 像素本身有固定成本
3. **canvas + SVG overlay**（commit `237713a`）——最终方案，标注完全不参与 canvas 操作

**最终架构**：单 canvas（底图，zoom/imageId 变化才重绘）+ SVG overlay（标注，`AnnotationSvg.tsx`，浏览器合成器独立处理）。标注变化（增删改、实时预览）只更新 SVG DOM 属性，零 canvas 操作。

### 后续追加需求

- **自适应宽度按钮**（commit `bdfa8fa`→`ad65e02`）：默认打开模式从 fitWindow 改为 fitWidth（图片宽度=窗口宽度）。工具栏加 `MoveHorizontal`（自适应宽度）+ `Expand`（自适应窗口）两个按钮。`fitModeRef` 三态跟踪。
- **ResizeObserver**（commit `bdfa8fa`）：窗口 resize 时按 fit 模式自动重算 zoom（借鉴 RapidRAW `useImageRenderSize`）。
- **灯箱暗场重构**（commit `2dc2ccf`）：左右 padding 48→8px、zinc 冷灰背景、棋盘格 14px、EXIF 条收敛。
- **工具栏重叠修复**（commit `5bc1b4d`）：顶部 padding `pt-14`。

### 不与 Screenshot 共享渲染层的决策

`lib/annotation.ts`（类型 + 纯函数）两端共享。渲染层各自实现：
- Screenshot 用单 canvas 全量重绘——屏幕尺寸 ~2M 像素，全量重绘 <1ms，无性能问题
- Screenshot 涉及选区裁剪逻辑，坐标系为窗口显示空间（非自然像素），改造量大
- 为 DRY 承担大改动 + 回归风险换不到可感知的性能提升——当前共享边界（纯函数 DRY + 渲染各自实现）是合理的

### 视口渲染（viewport rendering）演进

用户反馈超大图（2032×15796）在 M2 Ultra 上仍迟钝。分析发现：canvas = 整张图（~45M 像素 / 174MB buffer），GPU 每帧合成 91% 不可见区域。

**演进过程**（3 次迭代）：

1. **DPR 自适应限制**（commit `5653b31`，已废弃）：`MAX_CANVAS_PIXELS=20M`，canvas 超限时降 DPR。buffer 从 174MB→80MB。后续被视口渲染取代，常量已删除。

2. **视口渲染 v1**（commit `7e69f17`，已回退）：canvas 移出 scrollContainer，用 `getBoundingClientRect` 算偏移。**失败**——flex 居中 + padding + scroll 组合导致坐标偏移不可靠，两次修复仍画不出图（commit `9f0564b`、`9fa2ad8`）。

3. **视口渲染 v2**（commit `9bca0de` + `a9faa39`，**最终方案**）：
   - 彻底放弃 flex 居中，wrapper 用 `position:absolute` + JS 手算 `left/top`
   - canvas 在 scrollContainer 外面，`absolute inset-0`，恒定窗口大小（~2M 像素）
   - drawBg 纯手算：`scrollLeft/scrollTop + imgLeft/imgTop` → 裁剪可见区域 drawImage
   - wrapper 透明背景（不遮 canvas），canvas zIndex:1 / scrollContainer zIndex:2
   - viewport state（ResizeObserver）+ scrollPos state（RAF scroll）触发 drawBg
   - canvas CSS `width:100% height:100%`（canvas 在外层 relative 容器内铺满）

**最终效果**：GPU 合成量从 174MB → ~8MB（降 20×），canvas 恒定窗口大小，不管图片多大。详见 [视口渲染 v2 spec](specs/2026-07-03-viewport-rendering-v2-design.md)。

### 代码审查修复（8 项复查）

| # | 问题 | 判定 | 处理 | commit |
|---|------|------|------|--------|
| 1 | 无标注复制失效 | ✅ 真 bug | handleCopy 补 `copy_clipboard_item` | `f9c7e9d` |
| 2 | textarea Esc 关窗 | ✅ 真 bug | `stopPropagation` | `f9c7e9d` |
| 3 | scroll 触发全组件重渲染 | ✅ 真 bug | 删除 scrollPos state，RAF 直接调 drawBg | `f9c7e9d` |
| 4 | createImageBitmap 无 debounce | ⚠️ 部分 | 有版本保护，留后续加 debounce | — |
| 5 | createImageBitmap 卸载泄漏 | ✅ 真 bug | useEffect cleanup `zoomVersionRef++` | `f9c7e9d` |
| 6 | 文本折行硬编码 200px | ✅ 低优 | `ann.textWidth \|\| Infinity`，默认不折行 | `2f9be52` |
| 7 | 保存行为不一致 | ⚠️ 设计问题 | 统一走 `save_image_dialog` 弹窗 | `0454382` |
| 8 | thumb→full 尺寸跳变 | ✅ 成立 | 等比例修正 zoom | `f9c7e9d` |

Screenshot 的 text 标注调用同一 `annotation.ts` 纯函数，#6 修复自动覆盖两端。

### 第二轮代码审查修复（V2）

| # | 问题 | 判定 | 处理 | commit |
|---|------|------|------|--------|
| 1 | createImageBitmap 无 debounce | ✅ 合理 | setTimeout 150ms 防抖，期间 drawBg 用原图拉伸占位 | `ebf9426` |
| 2 | commitText 未存 textWidth | ⚠️ 防御性 | 存 textarea clientWidth / zoom = textWidth | `ebf9426` |
| 3 | 文字框缩放时 1-2px 抖动 | ❌ 不成立 | SVG/textarea 同坐标系，亚像素抖动通性，且编辑中不会缩放 | — |

### 第三轮代码审查修复（V3）

| # | 问题 | 判定 | 处理 | commit |
|---|------|------|------|--------|
| 1 | Screenshot 文本提交遗漏 textWidth | ✅ 真 bug | 4 处文本提交补 `textWidth: 200`（与 textarea 固定宽度一致） | `83620a8` |
| 2 | composePngBytes 图片未加载时 TypeError | ✅ 真 bug | 首行防御性校验 `imgRef/natW/natH`，提前 throw | `83620a8` |

### 窗口 resize 居中漂移修复

**根因**：视口渲染中 `imgLeft`（render 时从 `viewport.w` state 算）与 drawBg 的 `vw`（从 `sc.clientWidth` DOM 取）来自不同数据源。窗口 resize 后 React state 异步更新、DOM 同步更新 → 短暂不一致 → 图片漂移/消失。

**修复**（commit `5401118`）：render 里 `imgLeft` 直接读 `sc.clientWidth`（与 drawBg 同源同帧），drawBg 内 `liveImgLeft` 也用 `sc.clientWidth`。`viewport.w` state 仅用于触发 ResizeObserver re-render，不参与坐标计算。

### 第四轮代码审查修复（V4）

| # | 问题 | 判定 | 处理 | commit |
|---|------|------|------|--------|
| 1 | 缩略图竞态降级（full 先到 thumb 后到覆盖） | ✅ 真 bug | `fullLoadedRef` 门控，全图已加载后丢弃滞后的缩略图 | `245315d` |
| 2 | 缺少快速关闭预览窗口的交互 | ⚠️ 合理 | `tool=none` 时点击暗区（content 空白）关闭预览窗 | `245315d` |

### 序号 + 马赛克工具

| 改动 | commit |
|------|--------|
| 序号工具（Hash 图标，点击放置自动递增编号） | `2fe0383` |
| 马赛克工具（Grid2x2 图标，拖拽选区域，色块拼接 + 颜色/遮挡控制） | `2fe0383` → `c1519ef` |
| ASR insert SQL 残留 img_size 子查询修复（main 合并遗留） | `604d896` |
| 滚动时 canvas/SVG 错位晃动 → canvas 移入 wrapper 同一 scroll context | `8101e60` |
| 箭头头部随线宽缩放（headLen=max(12, lw*3)） | `66f3da7` |
