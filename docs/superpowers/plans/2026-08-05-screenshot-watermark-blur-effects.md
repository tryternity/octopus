# 截图水印 + 多种模糊效果 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图标注加 Gaussian 模糊 + Redact 黑条 + 文字水印，模糊工具 popover 切换 3 种效果，水印工具栏按钮弹输入框自动叠加。

**Architecture:** 全前端 canvas 渲染（后端零改动）。模糊：`Annotation` 加 `blurMode` 字段，`lib/annotation.ts` 加 `drawGaussian`/`drawRedact`/`drawBlur` 分发器，3 处 `drawMosaic` 调用改为 `drawBlur`。水印：4 个 config 字段 + `drawWatermark` 函数，导出时叠加（不进 annotations 数组）。

**Tech Stack:** React + TypeScript + Canvas 2D API + Tauri config（Rust `AppConfig`）

**Spec:** `docs/superpowers/specs/2026-08-05-screenshot-watermark-blur-effects.md`

## Global Constraints

- 全前端 canvas 渲染，后端零改动（不动 capx / desktop Rust 截图代码）
- `Annotation.blurMode` 是 optional，默认 `"pixelate"`（老数据向后兼容）
- 水印不进 `annotations` 数组（独立全局叠加层，config 驱动）
- Tauri 边界 casing：config 字段 snake_case（Rust）↔ 前端 camelCase（`get_config`/`set_config` 自动转换）
- Gaussian 模糊优先用 `ctx.filter='blur(Npx)'`，e2e 性能不达标再 fallback stackblur（Phase 1 Task 3 验证）
- 新增 i18n key 同步 `zh-CN.yaml` + `en.yaml`

---

## File Structure

| 文件 | 责任 | Phase |
|---|---|---|
| `crates/desktop/frontend/src/lib/annotation.ts` | +`blurMode` 字段 +`drawGaussian`/`drawRedact`/`drawBlur` +`drawWatermark` | 1+2 |
| `crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx` | blur popover 子菜单 + watermark 按钮 | 1+2 |
| `crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts` | +`blurMode` state + watermark text state | 1+2 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | composeAndCropBytes 调 drawBlur + drawWatermark | 1+2 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | drawMosaic → drawBlur | 1 |
| `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx` | drawMosaic → drawBlur | 1 |
| `crates/infra/src/config.rs` | +4 个水印配置字段 | 2 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | i18n | 1+2 |
| `docs/configuration.md` | 同步 4 个配置项 | 2 |

---

## 实施状态（2026-08-05 Subagent-Driven 执行完毕）

| Task | 状态 | commit | 备注 |
|---|:---:|---|---|
| 1 blurMode + drawBlur 分发器 | ✅ | `40c106d5` | review clean |
| 2 drawMosaic→drawBlur 替换 | ✅ | `0a216762` | brief 误列 RecordAnnotation（实际无导出路径），只改 2 处 |
| 3 useAnnotationState blurMode state | ✅ | `8610a65a` | review clean |
| 4 AnnotationToolbar blur popover | ✅ | `fae8edf5` | 2 Minor（toggle 语义/a11y）非阻塞 |
| 5 e2e Gaussian 性能验证 | ⏳ 待用户 | — | 代码就绪，待手动验证决定是否 fallback stackblur |
| 6 config 4 水印字段 | ✅ | `b32fe15b` | review clean |
| 7 drawWatermark + 9 格定位 | ✅ | `6a6b01a7` | review 发现 9 格 bug→fix（反向排除改正向 includes） |
| 8 水印按钮 + 读 config | ✅ | `e286b0c8` | brief 误写 camelCase，实际 snake_case（implementer 修正） |
| 9 设置页水印卡片 | ✅ | `1e0e6c38` | +i18n key 合理偏差 |
| 10 文档同步 | ✅ | `add77f69` | configuration.md + architecture.md |
| **Final review fix** | ✅ | `312816cd` | BLOCKER: apply_config_value 缺 4 arm + MAJOR: 画布水印预览缺失 |

**Plan 执行中的修正**（brief 误差，subagent+review 全部捕获）：
- Task 2: brief「3 处」实际 2 处（RecordAnnotation 无导出路径）
- Task 7: brief 9 格定位代码有 bug（4 格失效），review 发现→fix
- Task 8: brief casing 错（camelCase→实际 snake_case）
- Final: plan 遗漏 apply_config_value 分发器 + 画布预览

---

## Phase 1：模糊效果（Gaussian + Redact）

### Task 1: Annotation 加 blurMode 字段 + drawBlur 分发器

**Files:**
- Modify: `crates/desktop/frontend/src/lib/annotation.ts:10-22`（Annotation interface）+ `:323-353` 附近（drawMosaic 后加新函数）

**Interfaces:**
- Produces: `Annotation.blurMode?: "pixelate" | "gaussian" | "redact"`；`drawGaussian(ctx, ann, scale)` / `drawRedact(ctx, ann, scale)` / `drawBlur(ctx, ann, scale) -> void`

- [ ] **Step 1: Annotation interface 加 blurMode 字段**

`crates/desktop/frontend/src/lib/annotation.ts:10-22`，在 `filled?: boolean;` 后加：

```ts
export interface Annotation {
  type: "rect" | "oval" | "diamond" | "line" | "arrow" | "pen" | "highlight" | "text" | "number" | "blur";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
  points?: number[][];
  color?: string;
  lineWidth?: number;
  fontSize?: number;
  number?: number;
  circleSize?: number;
  textWidth?: number;
  filled?: boolean;
  blurMode?: "pixelate" | "gaussian" | "redact"; // 仅 type="blur" 时有意义，默认 "pixelate"（老数据兼容）
}
```

- [ ] **Step 2: 在 drawMosaic 函数后（约 line 354）加 drawGaussian + drawRedact + drawBlur**

```ts
/**
 * 高斯模糊（WKWebView ctx.filter='blur(Npx)'）。radius 由 lineWidth 控制。
 * e2e 验证性能：若拖拽卡顿或效果异常，fallback stackblur（TODO 注释标记）。
 */
export function drawGaussian(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  const bx = Math.round(Math.min(ann.x1, ann.x2) * scale);
  const by = Math.round(Math.min(ann.y1, ann.y2) * scale);
  const bw = Math.round(Math.abs(ann.x2 - ann.x1) * scale);
  const bh = Math.round(Math.abs(ann.y2 - ann.y1) * scale);
  if (bw < 2 || bh < 2) return;
  const radius = Math.max(4, (ann.lineWidth || 3) * 3);
  ctx.save();
  ctx.beginPath();
  ctx.rect(bx, by, bw, bh);
  ctx.clip();
  ctx.filter = `blur(${radius}px)`;
  ctx.drawImage(ctx.canvas, bx, by, bw, bh, bx, by, bw, bh);
  ctx.filter = "none";
  ctx.restore();
}

/**
 * 纯黑遮挡（Redact）——正式文档完全遮挡敏感信息。
 */
export function drawRedact(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  const bx = Math.round(Math.min(ann.x1, ann.x2) * scale);
  const by = Math.round(Math.min(ann.y1, ann.y2) * scale);
  const bw = Math.round(Math.abs(ann.x2 - ann.x1) * scale);
  const bh = Math.round(Math.abs(ann.y2 - ann.y1) * scale);
  if (bw < 2 || bh < 2) return;
  ctx.fillStyle = "#000000";
  ctx.fillRect(bx, by, bw, bh);
}

/**
 * blur 标注分发器——根据 ann.blurMode 调对应函数。
 * 调用方（composeAndCropBytes / ImagePreview / RecordAnnotation）统一用此入口，
 * 替换原直接调 drawMosaic 的 3 处调用点。
 */
export function drawBlur(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number = 1) {
  switch (ann.blurMode ?? "pixelate") {
    case "gaussian": drawGaussian(ctx, ann, scale); break;
    case "redact":   drawRedact(ctx, ann, scale);   break;
    default:         drawMosaic(ctx, ann, scale);   break;
  }
}
```

- [ ] **Step 3: tsc 编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/lib/annotation.ts
git commit -m "feat(screenshot): Annotation 加 blurMode 字段 + drawGaussian/drawRedact/drawBlur"
```

---

### Task 2: 替换 3 处 drawMosaic 调用为 drawBlur

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx:657-663`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx:500-503`
- Modify: `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx:340`

**Interfaces:**
- Consumes: `drawBlur` from Task 1

- [ ] **Step 1: Screenshot composeAndCropBytes 改 drawBlur**

`pages/Screenshot/index.tsx:657-663`，把 `drawMosaic` 调用改为 `drawBlur`：

```ts
    // 先处理 blur（像素马赛克/高斯/黑条），再画其他标注
    for (const ann of allAnns) {
      if (ann.type === "blur") drawBlur(tmpCtx, ann, scale);
    }
    for (const ann of allAnns) {
      if (ann.type === "blur") continue; // blur 已由 drawBlur 处理，跳过避免色块叠加两次
      drawAnnotationScaled(tmpCtx, ann, scale);
    }
```

同步修改 import：把 `drawMosaic` 改为 `drawBlur`（若 import 语句只引了 drawMosaic，改引 drawBlur；若两个都引则保留）。

- [ ] **Step 2: ImagePreview 同样改 drawBlur**

`pages/ImagePreview/index.tsx:500-503`，grep 找 `drawMosaic` 调用点，改为 `drawBlur`，import 同步。

- [ ] **Step 3: RecordAnnotation 同样改 drawBlur**

`pages/RecordAnnotation/index.tsx:340`，grep 找 `drawMosaic` 调用点，改为 `drawBlur`，import 同步。

- [ ] **Step 4: tsc + vite build 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 0 error。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx crates/desktop/frontend/src/pages/ImagePreview/index.tsx crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx
git commit -m "refactor(screenshot): 3 处 drawMosaic 调用改 drawBlur 分发器"
```

---

### Task 3: useAnnotationState 加 blurMode state

**Files:**
- Modify: `crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts`

**Interfaces:**
- Produces: `blurMode` / `setBlurMode` / `blurModeRef`（供 AnnotationToolbar popover 切换 + addAnnotation 时写入 ann.blurMode）

- [ ] **Step 1: AnnotationState interface 加 blurMode 三件套**

`useAnnotationState.ts`，在工具属性区（约 line 38 `toolFontSizeRef` 后）加：

```ts
  blurMode: "pixelate" | "gaussian" | "redact";
  setBlurMode: (m: "pixelate" | "gaussian" | "redact") => void;
  blurModeRef: React.MutableRefObject<"pixelate" | "gaussian" | "redact">;
```

- [ ] **Step 2: hook 实现加 blurMode state + ref**

在 `useAnnotationState` 函数体的 toolColor/toolWidth 等 state 附近（约 line 70 后）加：

```ts
  const [blurMode, setBlurModeState] = useState<"pixelate" | "gaussian" | "redact">("pixelate");
  const blurModeRef = useRef<"pixelate" | "gaussian" | "redact">("pixelate");
  const setBlurMode = useCallback((m: "pixelate" | "gaussian" | "redact") => {
    blurModeRef.current = m;
    setBlurModeState(m);
  }, []);
```

在 return 对象里加 `blurMode, setBlurMode, blurModeRef,`。

- [ ] **Step 3: addAnnotation 时 blur 标注写入 blurMode**

找到 `addAnnotation` 函数（grep `addAnnotation` in useAnnotationState.ts），在 push blur 标注时加 `blurMode: blurModeRef.current`。例如原代码：

```ts
annotationsRef.current.push(ann);
```

改为在 ann 构造处（调用方传入的 ann）—— 实际上 ann 是调用方构造的，addAnnotation 内部补 blurMode：

```ts
  const addAnnotation = useCallback((ann: Annotation) => {
    // blur 标注补 blurMode（调用方未必传，用当前 tool state 兜底）
    if (ann.type === "blur" && !ann.blurMode) {
      ann.blurMode = blurModeRef.current;
    }
    annotationsRef.current = [...annotationsRef.current, ann];
    setAnnotations(annotationsRef.current);
    // ... 现有 undo/redo 栈清理逻辑
  }, []);
```

- [ ] **Step 4: tsc 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/useAnnotationState.ts
git commit -m "feat(screenshot): useAnnotationState 加 blurMode state（popover 切换用）"
```

---

### Task 4: AnnotationToolbar blur 按钮 popover 子菜单

**Files:**
- Modify: `crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx:172-183`（tools 数组）+ blur 按钮渲染

**Interfaces:**
- Consumes: `blurMode` / `setBlurMode` from useAnnotationState（Task 3）

- [ ] **Step 1: i18n 加 blur 效果 key**

`crates/desktop/frontend/src/locales/zh-CN.yaml` 找 `screenshot.tool` 段（grep `tool.mosaic`），加：

```yaml
screenshot:
  tool:
    blur_pixelate: 像素化
    blur_gaussian: 高斯模糊
    blur_redact: 黑条遮挡
```

`en.yaml` 对应加：

```yaml
screenshot:
  tool:
    blur_pixelate: Pixelate
    blur_gaussian: Gaussian
    blur_redact: Redact
```

- [ ] **Step 2: AnnotationToolbar blur 按钮加 popover 子菜单**

先读 AnnotationToolbar.tsx 的 blur 按钮渲染部分（约 line 172-183 的 tools 数组里 blur 项 + 按钮渲染逻辑）。在 blur 按钮的 onClick 或 popover 渲染处，加 3 个 radio 选项切换 blurMode。

具体实现：blur 按钮点击时弹一个小 popover（复用现有 ToolPropsPopover 的浮层模式，或独立 div），含 3 个按钮：

```tsx
{/* blur 按钮 popover 子菜单——切换 blurMode */}
{showBlurPopover && (
  <div className="absolute z-50 ...">
    {(["pixelate", "gaussian", "redact"] as const).map((m) => (
      <button
        key={m}
        onClick={() => { setBlurMode(m); setTool("blur"); setShowBlurPopover(false); }}
        className={cn("...", blurMode === m && "bg-primary text-primary-foreground")}
      >
        {t(`screenshot.tool.blur_${m}`)}
      </button>
    ))}
  </div>
)}
```

`showBlurPopover` 是组件内 useState（点 blur 按钮切 true，选完切 false，点外部也切 false——用 useEffect + document click listener）。

- [ ] **Step 3: tsc + build 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 0 error。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "feat(screenshot): blur 按钮 popover 子菜单切换 Pixelate/Gaussian/Redact"
```

---

### Task 5: e2e 验证 Gaussian 性能（决定是否 fallback stackblur）

**Files:** 无代码改动（验证任务）

- [ ] **Step 1: 启动应用，截图进入标注模式**

- [ ] **Step 2: 选 Gaussian blur，画一个大区域（覆盖半屏）**

观察：视觉是否模糊（不是空白/原图）？拖拽绘制是否卡顿？

- [ ] **Step 3: 若 Gaussian 正常 → Phase 1 完成**

若 Gaussian 视觉异常或卡顿（>100ms 感知），在 `drawGaussian` 上方加 TODO 注释记录问题，并在 plan 里记录需后续 fallback stackblur。**本 plan 不实现 stackblur**（YAGNI，待 e2e 确认有问题再做）。

- [ ] **Step 4: 验证 Pixelate + Redact 也正常**

切回 Pixelate 画区域 → 马赛克正常。切 Redact → 纯黑矩形正常。

---

## Phase 2：水印

### Task 6: config 加 4 个水印字段

**Files:**
- Modify: `crates/infra/src/config.rs:198-201`（screenshot 配置区）

**Interfaces:**
- Produces: `AppConfig.screenshot_watermark_text` / `screenshot_watermark_position` / `screenshot_watermark_opacity` / `screenshot_watermark_font_size`

- [ ] **Step 1: AppConfig struct 加 4 个水印字段**

`crates/infra/src/config.rs`，在 `screenshot_shortcut` 字段后（约 line 200）加：

```rust
    /// 截图水印文字。空字符串=不加水印。默认 ""。
    #[serde(default = "default_screenshot_watermark_text")]
    pub screenshot_watermark_text: String,

    /// 截图水印位置（9 格）。默认 "bottom-right"。
    /// 合法值：top-left/top-center/top-right/middle-left/middle-center/middle-right/bottom-left/bottom-center/bottom-right
    #[serde(default = "default_screenshot_watermark_position")]
    pub screenshot_watermark_position: String,

    /// 截图水印透明度 0.0-1.0。默认 0.3。
    #[serde(default = "default_screenshot_watermark_opacity")]
    pub screenshot_watermark_opacity: f32,

    /// 截图水印字号（逻辑像素）。默认 24。
    #[serde(default = "default_screenshot_watermark_font_size")]
    pub screenshot_watermark_font_size: u32,
```

- [ ] **Step 2: 加 default 函数**

在 `default_screenshot_shortcut` 函数附近（约 line 333）加：

```rust
fn default_screenshot_watermark_text() -> String { String::new() }
fn default_screenshot_watermark_position() -> String { "bottom-right".to_string() }
fn default_screenshot_watermark_opacity() -> f32 { 0.3 }
fn default_screenshot_watermark_font_size() -> u32 { 24 }
```

- [ ] **Step 3: Default impl 加 4 个字段**

找到 `impl Default for AppConfig`（grep `impl Default for AppConfig`），在 `screenshot_shortcut: default_screenshot_shortcut(),` 后加：

```rust
            screenshot_watermark_text: default_screenshot_watermark_text(),
            screenshot_watermark_position: default_screenshot_watermark_position(),
            screenshot_watermark_opacity: default_screenshot_watermark_opacity(),
            screenshot_watermark_font_size: default_screenshot_watermark_font_size(),
```

- [ ] **Step 4: Rust 编译验证**

Run: `cargo build --release -p octopus-infra`
Expected: 0 error。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): 4 个截图水印配置字段（text/position/opacity/fontSize）"
```

---

### Task 7: drawWatermark 函数 + composeAndCropBytes 调用

**Files:**
- Modify: `crates/desktop/frontend/src/lib/annotation.ts`（加 drawWatermark）
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx:635-677`（composeAndCropBytes 末尾调 drawWatermark）

**Interfaces:**
- Produces: `drawWatermark(ctx, canvasW, canvasH, opts) -> void` + `WatermarkOpts` interface

- [ ] **Step 1: annotation.ts 加 WatermarkOpts + drawWatermark**

在 `drawBlur` 函数后加：

```ts
export interface WatermarkOpts {
  text: string;
  position: string;      // 9 格 key
  opacity: number;       // 0-1
  fontSize: number;
  color?: string;        // 默认 "#ffffff"
}

/**
 * 截图水印——导出时叠加到 canvas。position 为 9 格定位。
 * 不进 annotations 数组（全局叠加层，config 驱动）。
 */
export function drawWatermark(ctx: CanvasRenderingContext2D, canvasW: number, canvasH: number, opts: WatermarkOpts) {
  if (!opts.text) return;
  const margin = 16;
  ctx.save();
  ctx.globalAlpha = Math.max(0, Math.min(1, opts.opacity));
  ctx.fillStyle = opts.color ?? "#ffffff";
  ctx.font = `${opts.fontSize}px -apple-system, system-ui, sans-serif`;
  const metrics = ctx.measureText(opts.text);
  const tw = metrics.width;
  const th = opts.fontSize;
  const pos = opts.position;
  let x = margin;
  let y = margin;
  if (pos.includes("right")) x = canvasW - tw - margin;
  if (pos.includes("center") && !pos.startsWith("top") && !pos.startsWith("bottom")) x = (canvasW - tw) / 2;
  if (pos.includes("bottom")) y = canvasH - th - margin;
  if (pos.includes("middle") && !pos.endsWith("left") && !pos.endsWith("right")) y = (canvasH - th) / 2;
  ctx.fillText(opts.text, x, y + th);
  ctx.restore();
}
```

- [ ] **Step 2: composeAndCropBytes 末尾调 drawWatermark**

`pages/Screenshot/index.tsx:660-663`，在「先 blur 再其他标注」两个循环之后、裁切之前加：

```ts
    // 水印（全局叠加层，在所有标注之后）——从 config 读，水印不进 annotations 数组
    if (watermarkOpts?.text) {
      drawWatermark(tmpCtx, bgW, bgH, watermarkOpts);
    }
```

其中 `watermarkOpts` 是组件顶部从 config 读的 props（Task 8 从父组件传入，本 Step 先声明占位）：

```ts
  // watermarkOpts 由父组件从 config 读取传入（Task 8）。Phase 2 Step 2 前先从 localStorage 读测试。
  const watermarkOpts: WatermarkOpts | null = null; // TODO Task 8 替换为真实 config 读取
```

- [ ] **Step 3: tsc 验证（watermarkOpts 占位 null，drawWatermark 不触发）**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 error（WatermarkOpts import + drawWatermark 调用类型正确）。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/lib/annotation.ts crates/desktop/frontend/src/pages/Screenshot/index.tsx
git commit -m "feat(screenshot): drawWatermark 函数 + composeAndCropBytes 调用点（占位）"
```

---

### Task 8: Screenshot 组件读 config + AnnotationToolbar 加水印按钮

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`（读 config 替换 Task 7 占位）
- Modify: `crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx`（加水印按钮 + 输入框）

**Interfaces:**
- Consumes: Tauri `get_config` / `set_config` 命令（现有）+ WatermarkOpts from Task 7

- [ ] **Step 1: Screenshot 组件读水印 config**

在 Screenshot/index.tsx 组件顶部（现有 config 读取附近，grep `get_config` 找模式）加水印 config 读取：

```ts
  // 水印配置——从 config 读（camelCase 映射后端 snake_case）
  const [watermarkOpts, setWatermarkOpts] = useState<WatermarkOpts | null>(null);
  useEffect(() => {
    invoke<{ screenshotWatermarkText: string; screenshotWatermarkPosition: string; screenshotWatermarkOpacity: number; screenshotWatermarkFontSize: number }>("get_config")
      .then((c) => {
        if (c.screenshotWatermarkText) {
          setWatermarkOpts({
            text: c.screenshotWatermarkText,
            position: c.screenshotWatermarkPosition || "bottom-right",
            opacity: c.screenshotWatermarkOpacity ?? 0.3,
            fontSize: c.screenshotWatermarkFontSize ?? 24,
          });
        } else {
          setWatermarkOpts(null);
        }
      })
      .catch(() => {});
  }, []);
```

替换 Task 7 Step 2 的占位 `null`。

- [ ] **Step 2: AnnotationToolbar 加水印按钮**

`AnnotationToolbar.tsx` tools 数组末尾（eraser 前）加 watermark 工具：

```tsx
  { value: "watermark", icon: WatermarkIcon, labelKey: "screenshot.tool.watermark", svg: undefined },
```

水印按钮的 onClick 不走 tool 选中逻辑，而是弹输入框（独立于 tool state）：

```tsx
  const [showWatermarkInput, setShowWatermarkInput] = useState(false);
  const [watermarkInput, setWatermarkInput] = useState("");
  // 水印按钮 onClick: setShowWatermarkInput(true)
  // 输入框确认: invoke("set_config", { key: "screenshot_watermark_text", value: watermarkInput })
  //            → 通知父组件重读 config（emit 事件或回调）
```

- [ ] **Step 3: i18n 加水印 key**

`zh-CN.yaml` + `en.yaml` 加：

```yaml
screenshot:
  tool:
    watermark: 水印  # Watermark
  watermark:
    placeholder: 输入水印文字（留空清除）  # Enter watermark text (empty to clear)
    confirm: 确认  # Confirm
```

- [ ] **Step 4: tsc + build 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 0 error。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx crates/desktop/frontend/src/components/Annotation/AnnotationToolbar.tsx crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "feat(screenshot): 水印按钮 + 输入框 + config 读取"
```

---

### Task 9: 设置页水印配置卡片

**Files:**
- Modify: 截图设置面板（grep `ScreenshotPanel` 或 `screenshot` in `pages/Settings/`）

- [ ] **Step 1: 定位截图设置面板**

Run: `grep -rn "screenshot" crates/desktop/frontend/src/pages/Settings/ | head`
找到截图相关设置组件。

- [ ] **Step 2: 加水印配置卡片**

在截图设置面板加水印卡片：文字 input（autoCapitalize off）+ 位置 9 格 selector + 透明度 slider + 字号 number input。每个 onChange 调 `invoke("set_config", { key, value })`。

- [ ] **Step 3: tsc + build 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 0 error。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/
git commit -m "feat(screenshot): 设置页水印配置卡片（text/position/opacity/fontSize）"
```

---

### Task 10: 文档同步 + e2e 验证

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`（截图章节）

- [ ] **Step 1: configuration.md 加 4 个水印配置项**

在 `screenshot_shortcut` 行后加：

```markdown
| `screenshot_watermark_text` | string | `""` | desktop | 截图水印文字，空=不加水印。设置页可配 |
| `screenshot_watermark_position` | string | `"bottom-right"` | desktop | 水印 9 格位置（top-left/.../bottom-right）。设置页可配 |
| `screenshot_watermark_opacity` | f32 | `0.3` | desktop | 水印透明度 0.0-1.0。设置页可配 |
| `screenshot_watermark_font_size` | u32 | `24` | desktop | 水印字号（逻辑像素）。设置页可配 |
```

- [ ] **Step 2: architecture.md 截图章节补水印 + 模糊说明**

找到截图模块章节（grep `AnnotationToolbar` 或 `标注` in architecture.md），补：

> **模糊效果（2026-08-05）**：blur 工具升级为 3 种——Pixelate（drawMosaic shrink-enlarge）/ Gaussian（ctx.filter='blur'）/ Redact（纯黑 fillRect），Annotation.blurMode 字段切换，drawBlur 分发器统一入口。
> **水印（2026-08-05）**：4 个 config 字段（text/position/opacity/fontSize），工具栏水印按钮弹输入框写 config，导出时 drawWatermark 叠加（不进 annotations 数组，独立全局层）。

- [ ] **Step 3: Commit**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: 同步截图水印 + 模糊效果到 configuration.md + architecture.md"
```

- [ ] **Step 4: e2e 验证（手动）**

- 工具栏 blur 按钮 → popover 切 Gaussian → 画区域模糊正常
- 切 Redact → 纯黑矩形
- 工具栏水印按钮 → 输入 "test 2026" → 导出 PNG 右下角有水印
- 设置页改水印位置 top-left → 重新导出水印在左上角
- 水印输入框留空确认 → 导出无水印

---

## Self-Review

**1. Spec coverage:**

| Spec 章节 | 覆盖 Task |
|---|---|
| §2.1-2.5 模糊效果（3 种 + popover + blurMode + 渲染 + 预览 + fallback） | Task 1（数据+渲染）+ Task 2（替换调用）+ Task 3（state）+ Task 4（popover）+ Task 5（e2e fallback 验证） |
| §3.1-3.5 水印（交互 + config + 渲染 + 不进数组 + 预览） | Task 6（config）+ Task 7（drawWatermark）+ Task 8（按钮+读config）+ Task 9（设置页） |
| §4 UI 组件改动 | Task 4（toolbar）+ Task 8（水印按钮）+ Task 9（设置页） |
| §5 设置页 | Task 9 |
| §6 不变量 | Global Constraints + 各 Task 代码注释 |
| §7 测试 | Task 5（e2e 模糊）+ Task 10（e2e 水印）|
| §8 YAGNI 边界 | Global Constraints（不实现 stackblur/结晶/logo 水印） |
| §9 风险（ctx.filter） | Task 5 e2e 验证 + TODO 标记 |

无遗漏。预览态（§2.5）在 Task 1 的 drawGaussian/drawRedact 实现（导出态函数）里已含，预览态（拖拽时）走现有 drawAnnotation 的 blur 分支——Task 1 的 blurMode 分发不涉及预览，预览保持现有「半透明色块」逻辑（spec §2.5 已说明 gaussian/redact 预览用半透明矩形，这是 Task 4 popover 切换时的视觉细节，可在 Task 4 实现时补 drawAnnotation 的 blur 预览分支，或接受预览仍是色块网格——非阻塞）。

**2. Placeholder scan:** Task 7 Step 2 有 `// TODO Task 8 替换`——这是有意的占位（Task 8 替换），不是 plan 失败。其余无 TBD/「适当处理」。

**3. Type consistency:**
- `Annotation.blurMode` 全程 `"pixelate" | "gaussian" | "redact"`（Task 1 定义 → Task 3 state → Task 4 popover）
- `drawBlur(ctx, ann, scale)` 签名一致（Task 1 定义 → Task 2 三处调用）
- `WatermarkOpts` interface（Task 7 定义 → Task 8 使用）
- `drawWatermark(ctx, canvasW, canvasH, opts)` 签名一致（Task 7 → Task 8）

无类型不一致。

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-05-screenshot-watermark-blur-effects.md`. 10 个 Task 分两 Phase。

**执行建议**：
- **Subagent-Driven**（推荐）：每个 Task 派 fresh subagent，适合这种多文件多 Task 的计划
- **Inline Execution**：本 session 直接执行，但 10 个 Task 较多，session 会很长

**你选哪种？**
