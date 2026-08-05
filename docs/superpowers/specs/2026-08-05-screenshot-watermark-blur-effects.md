# 截图水印 + 多种模糊效果

- 日期：2026-08-05
- 类型：功能增强（截图标注）
- 优先级：P2
- 依赖：现有 `lib/annotation.ts` drawMosaic + AnnotationToolbar + useAnnotationState

## 1. 背景与动机

### 1.1 模糊效果现状

octopus 截图标注的 `blur` 工具目前只有一种效果——**像素化马赛克**（`drawMosaic`，shrink-enlarge 算法，`lib/annotation.ts:323-353`）。竞品对比（调研报告 §2）：

| 效果 | octopus | Snapzy | CleanShot | PixPin |
|---|:---:|:---:|:---:|:---:|
| Pixelate（像素化） | ✅ | ✅ | ✅ | ✅ |
| Gaussian（高斯模糊） | ❌ | ✅ | ✅ | ✅ |
| Redact（纯黑遮挡） | ❌ | ✅ | ✅ | ❌ |

**缺口**：无高斯模糊（人脸/背景自然遮挡）、无 Redact（正式文档完全遮挡）。

### 1.2 水印现状

**水印功能完全不存在**（全仓 grep 零命中）。竞品 Snapzy / CleanShot / PixPin 普遍支持文字水印（截图导出时自动叠加，用于版权标记 / 时间戳 / 作者署名）。

### 1.3 本 spec 范围

补齐实用优先的模糊变体（不追求 Snapzy 报告里不可复现的「8 种」）+ 文字水印。**全前端 canvas 实现**，后端零改动（复用现有 drawMosaic 的纯前端渲染模式）。

## 2. 模糊效果（3 种）

### 2.1 工具交互

现有 `blur` 工具按钮（`AnnotationToolbar.tsx`）点击后弹 **popover 子菜单**，切换 3 种效果：

```
点 blur 按钮 → popover:
  ○ Pixelate（现有马赛克，默认）
  ○ Gaussian（高斯模糊）
  ○ Redact（纯黑遮挡）
选中后 popover 关闭，blur 工具保持选中
```

切换不影响已绘制的 blur 标注（每个 blur 标注独立记录自己的 blurMode）。

### 2.2 数据结构

`Annotation` 加 `blurMode` 字段（`lib/annotation.ts:10-22`）：

```ts
export interface Annotation {
  type: "rect" | "oval" | ... | "blur";
  // ... 现有字段
  blurMode?: "pixelate" | "gaussian" | "redact";  // 仅 type="blur" 时有意义，默认 "pixelate"
}
```

- `blurMode` 是 optional，老 annotations（无此字段）反序列化时 `undefined` → 代码侧 `ann.blurMode ?? "pixelate"` 默认 pixelate（向后兼容）
- 新建 blur 标注时，`useAnnotationState` 把当前 `blurMode` state 写入 `ann.blurMode`

### 2.3 渲染实现

`lib/annotation.ts` 新增 2 个绘制函数 + 1 个分发器：

```ts
// 高斯模糊：ctx.filter + 裁切选区重绘
export function drawGaussian(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number) {
  const bx = Math.min(ann.x1, ann.x2) * scale;
  const by = Math.min(ann.y1, ann.y2) * scale;
  const bw = Math.abs(ann.x2 - ann.x1) * scale;
  const bh = Math.abs(ann.y2 - ann.y1) * scale;
  // blur 半径由 lineWidth 控制（默认 3 → ~10px blur）
  const radius = Math.max(4, (ann.lineWidth || 3) * 3);
  ctx.save();
  ctx.beginPath();
  ctx.rect(bx, by, bw, bh);
  ctx.clip();
  ctx.filter = `blur(${radius}px)`;
  // 把当前 canvas 的该区域重画一遍（带 filter）——WKWebView 的 ctx.filter 支持
  ctx.drawImage(ctx.canvas, bx, by, bw, bh, bx, by, bw, bh);
  ctx.restore();
}

// 纯黑遮挡
export function drawRedact(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number) {
  const bx = Math.min(ann.x1, ann.x2) * scale;
  const by = Math.min(ann.y1, ann.y2) * scale;
  const bw = Math.abs(ann.x2 - ann.x1) * scale;
  const bh = Math.abs(ann.y2 - ann.y1) * scale;
  ctx.fillStyle = "#000";
  ctx.fillRect(bx, by, bw, bh);
}

// 分发器：根据 blurMode 调对应函数（导出 + 预览统一入口）
export function drawBlur(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number) {
  switch (ann.blurMode ?? "pixelate") {
    case "gaussian": drawGaussian(ctx, ann, scale); break;
    case "redact":   drawRedact(ctx, ann, scale);   break;
    default:         drawMosaic(ctx, ann, scale);   break;  // pixelate + 兼容老数据
  }
}
```

**替换点**：现有 3 处调用 `drawMosaic` 的地方（`pages/Screenshot/index.tsx:657-663` / `pages/ImagePreview/index.tsx:500-503` / `pages/RecordAnnotation/index.tsx:340`）改为调 `drawBlur` 分发器。

### 2.4 Gaussian 的 WKWebView 风险 + fallback

`ctx.filter = 'blur(Npx)'` 在 WKWebView 上支持但性能可能不如原生。**e2e 验证策略**：

1. 先用 `ctx.filter` 实现（最简单，~10 行）
2. e2e 测试截图时画 gaussian blur 区域，确认：
   - 视觉上真的模糊了（不是空白/原图）
   - 性能可接受（拖拽绘制不卡，大区域 < 100ms）
3. **若 `ctx.filter` 不可用或性能差**：fallback 手写 **stackblur 算法**（~50 行，纯 JS，不依赖任何库）——stackblur 是业界经典快速高斯近似算法，PixPin/CleanShot 前端也用类似方案

### 2.5 预览态（拖拽绘制时）

现有 `drawAnnotation` / `drawAnnotationScaled` 的 `blur` 分支画「半透明色块网格」做预览（annotation.ts:132-153）。新增 gaussian/redact 的预览：

- **gaussian 预览**：画半透明灰色矩形 + 边框（标注「将模糊」意图，不实时算 blur 避免拖拽卡顿）
- **redact 预览**：画半透明黑色矩形（接近最终效果）

## 3. 水印

### 3.1 交互流程

```
工具栏点「水印」按钮 → 弹输入框（文本内容）
  ↓ 输入 "© 2026 wudarui" + 确认
  ↓ 写入 config.screenshot_watermark_text + 触发 canvas 重绘
  ↓ 导出时 drawWatermark 自动叠加到配置的位置
```

- 输入框为空时确认 = 清除水印（`screenshot_watermark_text = ""`）
- 输入框用 `<input>` + autoCapitalize="off"（对齐 AGENTS.md 禁自动大写规范）

### 3.2 配置项（新增到 AppConfig）

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `screenshot_watermark_text` | String | `""` | 水印文字，空=不加水印 |
| `screenshot_watermark_position` | String | `"bottom-right"` | 9 格位置：`top-left`/`top-center`/`top-right`/`middle-left`/`middle-center`/`middle-right`/`bottom-left`/`bottom-center`/`bottom-right` |
| `screenshot_watermark_opacity` | f32 | `0.3` | 透明度 0.0-1.0 |
| `screenshot_watermark_font_size` | u32 | `24` | 字号（逻辑像素） |

### 3.3 渲染

`lib/annotation.ts` 新增 `drawWatermark`：

```ts
export interface WatermarkOpts {
  text: string;
  position: string;      // 9 格
  opacity: number;       // 0-1
  fontSize: number;
  color?: string;        // 默认 "#ffffff"
}

export function drawWatermark(ctx: CanvasRenderingContext2D, canvasW: number, canvasH: number, opts: WatermarkOpts) {
  if (!opts.text) return;
  const margin = 16;
  ctx.save();
  ctx.globalAlpha = opts.opacity;
  ctx.fillStyle = opts.color ?? "#ffffff";
  ctx.font = `${opts.fontSize}px -apple-system, system-ui, sans-serif`;
  // 测量文字宽高
  const metrics = ctx.measureText(opts.text);
  const tw = metrics.width;
  const th = opts.fontSize;
  // 9 格定位
  let x = margin, y = margin;
  if (opts.position.includes("right"))  x = canvasW - tw - margin;
  if (opts.position.includes("center") && !opts.position.includes("top") && !opts.position.includes("bottom")) x = (canvasW - tw) / 2;
  if (opts.position.includes("bottom")) y = canvasH - th - margin;
  if (opts.position.includes("middle") && !opts.position.includes("left") && !opts.position.includes("right")) y = (canvasH - th) / 2;
  ctx.fillText(opts.text, x, y + th);  // y+th 因为 fillText 基线在文字底部
  ctx.restore();
}
```

**调用点**：`composeAndCropBytes`（`pages/Screenshot/index.tsx:635-677`）在所有标注绘制完、裁切前调用：

```ts
// 现有：先 drawMosaic 循环，再 drawAnnotationScaled 循环
// 新增：最后 drawWatermark（叠加到全 canvas，裁切会自动带上对应区域）
if (watermarkText) {
  drawWatermark(tmpCtx, bgW, bgH, { text: watermarkText, position, opacity, fontSize });
}
```

### 3.4 水印不进 annotations 数组

**关键设计**：水印是全局叠加层，**不作为 Annotation 存进 `annotations` 数组**。原因：
1. 避免 undo/redo 误删水印（undo 应该只撤销标注，不撤销水印）
2. 水印配置在 config，所有截图统一（不是单张截图的属性）
3. 导出时从 config 读配置 + 当前 canvas 尺寸算位置，简单直接

水印工具栏按钮的作用只是「快捷输入水印文字」（替代用户去设置页改 config），点确认后写 config + 触发当前截图 canvas 重绘（显示水印预览）。

### 3.5 实时预览

工具栏水印按钮确认后，需要让用户在标注画布上**立即看到水印**（而不是等导出）：

- `useAnnotationState` 加 `watermarkText` state（镜像 config）
- 标注画布的渲染循环（现有重绘逻辑）末尾调 `drawWatermark`
- 切换工具/绘制标注/undo-redo 都会触发重绘 → 水印自动跟随

## 4. UI 组件改动

### 4.1 AnnotationToolbar（`components/Annotation/AnnotationToolbar.tsx`）

- blur 按钮：保持现有 icon（mosaic.svg），点击行为改为弹 popover 子菜单（而非直接选中工具）
  - 但仍需保持「选中 blur 工具」语义——popover 选 blurMode 后 `setTool("blur")` + `setBlurMode(mode)`
- 新增 watermark 按钮（icon: `watermark.svg`），点击弹输入框（不走 popover，走独立 modal 或 inline input）

### 4.2 ToolPropsPopover（`pages/Screenshot/ToolPropsPopover.tsx`）

现有 blur 工具选中时弹的属性浮窗（color/width）。加 blurMode 切换可能让 popover 过载。**方案**：blurMode 切换放工具栏按钮的直接 popover（点 blur 按钮就弹），ToolPropsPopover 只管 color/width（调整 blur 强度）。

### 4.3 i18n（`locales/zh-CN.yaml` + `en.yaml`）

```yaml
screenshot:
  tool:
    blur_pixelate: 像素化   # Pixelate
    blur_gaussian: 高斯模糊 # Gaussian blur
    blur_redact: 黑条遮挡   # Redact
    watermark: 水印         # Watermark
  watermark:
    placeholder: 输入水印文字（留空清除）  # Enter watermark text (empty to clear)
    confirm: 确认                          # Confirm
```

## 5. 设置页

Screenshot 设置面板（`pages/Settings/ScreenshotPanel.tsx` 或对应组件）加水印配置卡片：

- 水印文字（input，autoCapitalize off）
- 位置（9 格 selector）
- 透明度（slider 0-1）
- 字号（number input）

这些配置写入 config（`set_config` 命令），热重载生效。

## 6. 不变量

1. **现有 drawMosaic 行为不变**——老 annotations（无 blurMode）默认 pixelate
2. **水印不进 annotations 数组**——独立全局叠加层，不受 undo/redo 影响
3. **水印渲染在所有标注之后**——保证水印在最上层
4. **3 种 blur 效果互不干扰**——每个 blur 标注独立记录自己的 blurMode
5. **后端零改动**——全前端 canvas 渲染（复用现有模式）

## 7. 测试

### 7.1 单测（lib/annotation.ts 纯函数）

加 `#[cfg(test)]` 风格的 TS 测试（如 vitest，若项目无测试基建则手写 node assert）：

- `drawBlur` 分发器：`blurMode="pixelate"` 调 drawMosaic / `"gaussian"` 调 drawGaussian / `"redact"` 调 drawRedact / `undefined` 调 drawMosaic（兼容）
- `drawWatermark` 9 格定位：bottom-right 在右下角 / top-left 在左上角 / center 居中
- `drawWatermark` 空文字 early return

### 7.2 e2e 验证（手动）

- 画 gaussian blur 区域 → 视觉模糊 + 性能可接受（拖拽不卡）
- 画 redact 区域 → 纯黑矩形
- 老截图（无 blurMode 的 blur 标注）打开仍显示 pixelate
- 工具栏水印按钮 → 输入文字 → 导出 PNG 有水印在配置位置
- 设置页改水印位置 → 重新导出水印位置变化

## 8. YAGNI 边界

明确**不做**：

| 项 | 理由 |
|---|---|
| 结晶（Crystallize）/ 半调（Halftone）等复杂效果 | 实现成本高（需手写 Voronoi/点阵算法），场景窄；3 种核心效果已覆盖隐私遮挡需求 |
| 图片 logo 水印 | 只文字；logo 需文件管理 + 缩放配置，YAGNI |
| 水印旋转 / 平铺（tiling） | 单条文字水印已满足核心场景 |
| 后端 Rust 渲染 | 全前端 canvas 够用；后端渲染需重构导出流程，过度工程 |
| 水印字体选择 | 用系统默认 `-apple-system`，不暴露字体选择 |

## 9. 风险

1. **WKWebView `ctx.filter` 支持**：Gaussian 模糊核心依赖。WKWebView 支持 `ctx.filter` 但性能未经实测。fallback：stackblur 算法（~50 行纯 JS）。e2e 验证后定方案。
2. **水印性能**：每次重绘都画水印文字，开销可忽略（fillText 单次 < 1ms）
3. **多窗口水印状态同步**：Screenshot 和 ImagePreview / RecordAnnotation 共享标注代码但水印配置在 config。RecordAnnotation 是否加水印？默认不加（录屏标注是动态的，水印无意义）——仅 Screenshot 导出加水印。

## 10. 实现顺序建议

1. **Phase 1：模糊效果**（独立可测，不依赖水印）
   - Annotation 加 blurMode 字段
   - drawGaussian + drawRedact + drawBlur 分发器
   - AnnotationToolbar blur popover 子菜单
   - 替换 3 处 drawMosaic 调用为 drawBlur
   - e2e 验证 gaussian 性能（决定是否 fallback stackblur）

2. **Phase 2：水印**
   - config 加 4 个字段
   - drawWatermark 函数
   - composeAndCropBytes 调用
   - 工具栏水印按钮 + 输入框
   - 设置页水印配置卡片
