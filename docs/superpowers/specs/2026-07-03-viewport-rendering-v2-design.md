# 图片查看器视口渲染 v2 设计

> 日期：2026-07-03
> 状态：📋 设计中
> 前置：`2026-07-03-image-viewer-perf-design.md`（SVG overlay + DPR 自适应已合入 main）

## 1. 背景

当前方案（canvas = 整张图 + DPR 自适应限制）在 2032×15796 超长图上：
- canvas ~20M 像素（DPR 降级后），buffer ~80MB
- GPU 每帧合成整个 canvas（91% 不可见）
- 标注已用 SVG overlay 解决（零 canvas 操作）

v1 视口渲染尝试失败（坐标偏移），回退到 DPR 降级方案。

## 2. v1 失败根因

```
v1 结构：
  <canvas absolute inset-0>          ← 在 scrollContainer 外面（兄弟节点）
  <scrollContainer absolute inset-0>  ← overflow-auto
    <flex justify-center pt-14>
      <wrapper>  ← 图片占位 + SVG overlay
```

问题：
1. canvas 和 scrollContainer 是兄弟节点，都 `absolute inset-0`，但 scrollContainer 有滚动条 → `clientWidth` ≠ canvas 实际渲染宽度
2. canvas `width:100%` = 父容器宽度（含滚动条区域），与 `sc.clientWidth`（不含滚动条）不一致
3. flex 居中 + `pt-14` padding → wrapper 的 `getBoundingClientRect().left` 随窗口/图片宽度比变化，无法简单推导
4. `drawImage` 的源/目标坐标基于错误的偏移 → 只画出窄条

## 3. v2 方案：sticky canvas + 纯 scroll offset

**核心思路**：canvas 放在 scrollContainer **里面**，用 `position: sticky` 钉在可视区顶部。wrapper 撑起 `dispW × dispH` 滚动条。滚动时 `sticky` 自动保持 canvas 在可视区，`drawBg` 用 `scrollLeft/scrollTop` 算可见区域。

```
v2 结构：
  <scrollContainer overflow-auto absolute inset-0>
    <wrapper style="width:dispW; height:dispH; position:relative">
      ← 棋盘格底背景（永远显示，即使 canvas 没画到的地方）
      <canvas style="position:sticky; top:0; left:0; width:100vw; height:100vh">
        ← sticky 钉在可视区，尺寸=窗口大小
      <svg overlay style="absolute inset-0; width:dispW; height:dispH">
        ← 标注，随滚动移动（自然坐标 viewBox）
```

**坐标计算**（纯 scroll offset，不依赖 getBoundingClientRect）：

```ts
const drawBg = () => {
  const sc = scrollContainerRef.current;
  const { scrollLeft: sl, scrollTop: st, clientWidth: vw, clientHeight: vh } = sc;
  canvas.width = vw * dpr; canvas.height = vh * dpr;

  // 图片显示尺寸
  const dw = natW * zoom, dh = natH * zoom;

  // 图片在 wrapper 内的偏移（wrapper 从 padding flex 居中，但 sticky canvas
  // 的 (0,0) 对齐 scrollContainer 可视区左上角 = wrapper 的 (sl, st) 点）
  // → canvas 上应画图片的 (sl, st) 到 (sl+vw, st+vh) 区域
  const visLeft = Math.max(0, sl);
  const visTop = Math.max(0, st - paddingTop);
  const visRight = Math.min(dw, sl + vw);
  const visBottom = Math.min(dh, st + vh - paddingTop);

  // 但 wrapper 有 flex 居中 → 图片可能不在 wrapper 的 (0,0)
  // 解决：wrapper 去掉 flex 居中，改 absolute 定位
};
```

**去掉 flex 居中**是关键——用 JS 算 `left/top` 绝对定位 wrapper（当图片小于窗口时居中），这样所有坐标都是确定的数值，不依赖布局引擎。

## 4. 详细设计

### 4.1 布局

```tsx
<div ref={scrollContainerRef}
  className="absolute inset-0 overflow-auto thin-scrollbar">
  {/* 内容层：撑起滚动条，尺寸 = max(dispW, viewport) × max(dispH, viewport) */}
  <div style={{
    position: "relative",
    width: Math.max(dispW + 16, scWidth),   // +padding
    height: Math.max(dispH + 72, scHeight), // +padding(top56+bottom)
  }}>
    {/* wrapper：图片占位，absolute 居中定位 */}
    <div ref={wrapperRef}
      style={{
        position: "absolute",
        left: imgLeft,   // JS 算：图片小于窗口时居中，否则顶左
        top: 56,         // pt-14 = 56px
        width: dispW, height: dispH,
        ...棋盘格底, ...cursor, ...onMouse
      }}>
      {/* canvas：sticky 钉在 scrollContainer 可视区 */}
      <canvas ref={bgCanvasRef}
        style={{
          position: "sticky",
          top: 0, left: 0,
          width: viewportW, height: viewportH,
          pointerEvents: "none",
        }} />
      <svg ...overlay />
    </div>
  </div>
</div>
```

**问题**：`sticky` 相对于最近的可滚动祖先（scrollContainer）。但 canvas 在 wrapper 内，wrapper 在 content div 内——sticky 行为可能不对。

### 4.2 更简单方案：canvas 在 scrollContainer 直接子级

```
<scrollContainer overflow-auto absolute inset-0>
  <!-- canvas：sticky 钉可视区，pointer-events:none -->
  <canvas style="position:sticky; top:56px; left:0; z-index:1; pointer-events:none" />
  <!-- content：撑滚动条 + wrapper（棋盘格 + SVG + 鼠标） -->
  <div style="position:relative; padding-top:56px; min-height:100%; display:flex; justify-content:center;">
    <wrapper style="width:dispW; height:dispH; ...">
      <svg overlay />
    </wrapper>
  </div>
</scrollContainer>
```

canvas 作为 scrollContainer 的**直接第一个子元素**，`sticky top:56px` 让它钉在可视区（留出工具栏空间）。content div 撑滚动条。

**drawBg 坐标**（canvas 的 `(0,0)` = scrollContainer 可视区 `(0, 56)` 处）：

```ts
const drawBg = () => {
  const sc = scrollContainerRef.current;
  const sl = sc.scrollLeft, st = sc.scrollTop;
  const vw = sc.clientWidth, vh = sc.clientHeight;
  canvas.width = vw * dpr; canvas.height = (vh - 56) * dpr;

  // wrapper 的位置：flex 居中 + paddingTop 56
  // 图片左上角在 content 空间的坐标：
  const imgX = Math.max(0, (vw - dispW) / 2);  // flex 居中
  const imgY = 0;  // 相对 padding 后

  // 图片左上角在 viewport 空间的坐标：
  const imgVpX = imgX - sl;
  const imgVpY = imgY - (st - 56);  // sticky top:56 偏移

  // 但 sticky canvas top:56 → canvas (0,0) = viewport (0, 56)
  // → 图片在 canvas 空间的坐标：
  const imgCx = imgVpX;
  const imgCy = imgY - st + 56;  // = 56 - st + 56? 不对...
};
```

**flex 居中又来了**——只要用 flex 就有这个推导问题。

### 4.3 最终方案：彻底放弃 flex，全部 absolute

**原则**：所有定位用 `absolute` + JS 算的数值，不依赖 CSS 布局引擎推导。

```tsx
<scrollContainer overflow-auto absolute inset-0>
  {/* spacer：纯撑滚动条，不可见 */}
  <div style={{ width: scrollW, height: scrollH }} />
  {/* canvas：sticky 钉可视区 */}
  <canvas sticky style="top:0; left:0; width:100%; height:100%; pointer-events:none" />
  {/* wrapper：absolute 定位，随滚动移动（用 transform 偏移） */}
  <div wrapper absolute style="transform: translate(imgLeft - sl, imgTop - st); width:dispW; height:dispH">
    <svg overlay />
  </div>
</scrollContainer>
```

不行——`sticky` + `absolute` 兄弟节点在同一 scrollContainer 内行为复杂。

### 4.4 最可靠方案：canvas fixed + scroll listener + 手算坐标

回到 canvas 在 scrollContainer **外面**的方案，但**彻底手算所有坐标**：

1. scrollContainer `absolute inset-0 overflow-auto`
2. 内部只有 wrapper（撑滚动条 + SVG + 棋盘格 + 鼠标事件），**无 canvas**
3. canvas 在 scrollContainer 外面，`absolute inset-0 pointer-events:none`
4. drawBg 用 `sc.scrollLeft/scrollTop + sc.clientWidth/Height` + **手算图片位置**

**手算图片位置**（不用 getBoundingClientRect）：
- 图片在 scrollContainer 内的位置 = flex 居中 + paddingTop
- `imgLeftInContent = max(0, (clientWidth - dispW) / 2)`（flex 居中）
- `imgTopInContent = 56`（paddingTop，但 content 用 padding 不用 flex 的 align）

等等——flex `justify-center` 的居中值 = `max(0, (container - child) / 2)`。这是确定的数学公式，可以手算。问题是 container 宽度 = scrollContainer 的 `clientWidth`（不含滚动条）。

**验证公式**：
- scrollContainer `clientWidth = 1200`（假设窗口 1200px 宽，无竖滚动条时）
- `dispW = 1185`（fit-to-width 后）
- `imgLeftInContent = max(0, (1200 - 1185) / 2) = 7.5`
- 图片在 viewport 中的 x = `imgLeftInContent - scrollLeft = 7.5 - 0 = 7.5`
- canvas 应该在 x=7.5 处开始画图片

**但如果 scrollContainer 有竖滚动条**：
- `clientWidth = 1200 - 15 = 1185`（滚动条 15px）
- flex 容器宽度 = `clientWidth = 1185`
- 但 flex 容器实际在 `padding-right` 内...

**关键简化**：去掉 flex 居中和 padding，用 content div 的 `position: relative` + wrapper `position: absolute` + JS 算 `left/top`。

```tsx
<scrollContainer overflow-auto absolute inset-0>
  <content style="position:relative; width:contentW; height:contentH;">
    <wrapper absolute style={{
      left: imgLeft,   // JS: 图片小于可视区时居中
      top: imgTop,     // JS: 56px padding
      width: dispW, height: dispH,
      ...棋盘格, ...cursor, ...onMouse
    }}>
      <svg overlay />
    </wrapper>
  </content>
</scrollContainer>
<canvas absolute inset-0 pointer-events:none />
```

**drawBg 坐标**（完全手算，不依赖 DOM 查询）：

```ts
const drawBg = () => {
  const sc = scrollContainerRef.current;
  const sl = sc.scrollLeft, st = sc.scrollTop;
  const vw = sc.clientWidth, vh = sc.clientHeight;

  // 图片位置（content 空间坐标，= wrapper 的 left/top）
  const imgLeft = Math.max(0, (vw - dispW) / 2);  // 居中（无 padding 影响——content 无 padding）
  const imgTop = 56;  // 固定 top padding

  // canvas 和 scrollContainer 同为 absolute inset-0 → 同坐标系
  // 图片可见区在 canvas 坐标系中的位置：
  // 图片在 viewport 中的左上角 = (imgLeft - sl, imgTop - st)
  const imgVpX = imgLeft - sl;
  const imgVpY = imgTop - st;

  // 图片可见部分（相对图片左上角，显示坐标）
  const visL = Math.max(0, -imgVpX);
  const visT = Math.max(0, -imgVpY);
  const visR = Math.min(dispW, vw - imgVpX);
  const visB = Math.min(dispH, vh - imgVpY);

  // → 源图裁剪 + 画到 canvas 对应位置
  const sx = (visL / dispW) * srcW;
  ...
  const dx = visL + imgVpX;
  const dy = visT + imgVpY;
  ctx.drawImage(bitmap || img, sx, sy, sw, sh, dx, dy, visR - visL, visB - visT);
};
```

**wrapper 的 left/top**（React 渲染时同步设）：

```tsx
const imgLeft = Math.max(0, (scWidth - dispW) / 2);
const imgTop = 56;
```

**这里 scWidth = scrollContainer.clientWidth**，需要在 render 时知道。用 state 跟踪：

```ts
const [viewport, setViewport] = useState({ w: 0, h: 0 });
useEffect(() => {
  const sc = scrollContainerRef.current;
  if (!sc) return;
  const update = () => setViewport({ w: sc.clientWidth, h: sc.clientHeight });
  update();
  const ro = new ResizeObserver(update);
  ro.observe(sc);
  return () => ro.disconnect();
}, []);
```

## 5. canvasCoords

鼠标 → wrapper 的 `left/top` + scroll → 图片坐标：

```ts
const canvasCoords = (e: React.MouseEvent) => {
  const sc = scrollContainerRef.current!;
  const imgLeft = Math.max(0, (viewport.w - dispW) / 2);
  const imgTop = 56;
  // wrapper 的 getBoundingClientRect 是可靠的（它随滚动移动）
  // 但我们不查 DOM——直接手算
  const scRect = sc.getBoundingClientRect();
  // 图片左上角在屏幕上的位置
  const imgScreenX = scRect.left + imgLeft - sc.scrollLeft;
  const imgScreenY = scRect.top + imgTop - sc.scrollTop;
  return {
    cssX: e.clientX - imgScreenX,
    cssY: e.clientY - imgScreenY,
  };
};
```

## 6. 棋盘格底

wrapper 有棋盘格背景。但 canvas 只画可见区域的图片——**不可见区域（canvas 没画到的地方）只显示棋盘格底**。这是正确的：用户滚动时，新区域先看到棋盘格底（瞬态），RAF drawBg 后画上图片。

## 7. SVG overlay

SVG 保持在 wrapper 内，`width: dispW, height: dispH, viewBox: 0 0 natW natH`。随 wrapper 一起滚动（wrapper absolute 定位随 content 滚动）。标注坐标系统不变。

## 8. 不变量

1. 所有定位用 absolute + JS 手算，不依赖 CSS flex 居中推导
2. canvas 和 scrollContainer 同为 `absolute inset-0`（同一父容器，同坐标系）
3. drawBg 只用 `scrollLeft/scrollTop + clientWidth/clientHeight` + 已知的 `imgLeft/imgTop`
4. wrapper 的 left/top 在 React render 和 drawBg 中用同一公式
5. `getBoundingClientRect` 不用于 drawBg（只用于 canvasCoords 的 scRect 左上角基准）
