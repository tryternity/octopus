# 图片预览 / 标注窗（Image Preview）设计

> 日期：2026-07-01
> 状态：**设计中**（spec 已写，待评审 → writing-plans）。
> 关联：`docs/superpowers/specs/2026-06-30-compact-editor-design.md`（窗口/命令/PENDING 模式模板）、`docs/superpowers/specs/2026-06-30-notepad-design.md`。
> 分支：`worktree-feature-notepad`。**功能完整完成前不往 main 同步。**

## 1. 背景与目标

剪贴板文本条目已能用精简编辑器（compact editor）打开编辑。图片条目目前只能看 8×8 缩略图 + 尺寸，**无法看原图、无法在图上做标注**。本期补一个「图片预览 / 标注窗」：

- 从剪贴板图片条目**单击缩略图**唤起，打开原图。
- **浮动白卡工具栏**（对齐截图主工具栏的漂浮白卡形态）：标注工具（矩形/椭圆/直线/文字）+ 颜色·粗细属性浮窗 + 保存 + 复制 + OCR + 缩放 + 置顶（关窗走原生 × / Esc，工具栏不放关闭按钮）。
- 标注能力**复用截图工具栏已有的标注引擎**（抽取成共享模块，截图与预览共用）。

用户视野里有**两种图片展示形态**，本需求只做第一种，但为第二种留好基础：

| 形态 | 触发 | 长相 | 本需求 |
|---|---|---|---|
| **① 轻工具栏预览** | 剪贴板条目缩略图单击 | 原生窗口 + 灯箱画布 + 浮动白卡工具栏，可标注 | ✅ 做 |
| **② 贴图模式** | 按需（主要给**截图钉住**用） | 无工具栏、就一张图、钉屏置顶（Snipaste 风格，hover 浮出关闭/图钉按钮） | ❌ 不做，留基础 |

## 2. 范围

**做：**
- 新建独立窗口 `image_preview_window`（原生标题栏、可调大小、单例销毁、macOS 激活策略切换）。
- 后端命令：`open_image_preview` / `get_pending_image` / `close_image_preview` / `get_image_full`。
- **抽取共享标注核心** `frontend/src/lib/annotation.ts`：类型（`Tool`/`Annotation`）+ 纯绘制/命中函数。`Screenshot/index.tsx` 改为 import（行为零变化），`ImagePreview` 复用。
- `ImagePreview` 组件：全图画布（fit-to-window、无选区），点击拖拽画标注、文字点选输入、撤销、选中移动。
- 轻工具栏组件（标注工具 + 属性浮窗 + 保存/复制/OCR/置顶）。
- 入口：剪贴板图片条目缩略图单击 → 唤起预览。

**不做（YAGNI / 留给未来）：**
- **贴图模式（形态②）**：无边框置顶、hover 工具栏、多实例——本需求不建窗口、不写交互，仅在 §9 文档化基础。
- 标注**持久化到剪贴板条目**：本次标注是「按需预览」的临时操作，关窗即失；保存走「导出带标注的新图到文件」。未来可再持久化。
- ~~缩放控件（滚轮/按钮放大）~~：**已做**（见 §3.4）——默认 1:1 自然分辨率打开，工具栏放大/缩小按钮调 `zoom`（0.1×–8×），超出窗口自动滚动条 + 选择工具下抓手拖拽平移。标注用自然像素坐标空间，缩放/调窗不错位。
- 箭头/画笔/序号工具：截图有，但用户列的是「矩形/椭圆/直线/文字」四样，保持简单。共享核心仍含全部类型，预览工具栏只暴露这四种。

## 3. 架构

### 3.1 窗口与生命周期（`crates/desktop/src/image_preview_window.rs`）

镜像 `compact_editor_window.rs`：

- `WINDOW_LABEL = "image_preview_window"`。
- `create_image_preview_window(app)`：`.title("图片预览")`、`.inner_size(880, 620)`、`.min_inner_size(400, 320)`、`.decorations(true)`、`.resizable(true)`、`.center()`、`.visible(true)`。
- 单例：`get_webview_window` 命中已存在则 `show + set_focus` 并 emit load 推新 imageId（并发再开）；否则创建。
- macOS 激活策略：开窗切 `Regular`（Dock 显图标），关窗切回 `Accessory`。新增 `on_image_preview_closed(app)`，在 `main.rs` 的 `RunEvent::WindowEvent { Destroyed }` 按 label 挂载（紧邻 `on_compact_editor_closed`）。
- 生命周期：**关窗即销毁**（destroy-on-close）。

> **ACL（必做，踩过的坑）**：动态窗口 label 必须加进 `capabilities/default.json` 的 `windows` 数组，否则该窗口前端 `emit`/`invoke`/`listen` 全被静默拦。当前数组补 `image_preview_window`：
> `["main","result_window","settings_window","clipboard_window","notepad_window","compact_editor_window","image_preview_window","screenshot_*"]`。

### 3.2 后端命令（`crates/desktop/src/image_preview_commands.rs`，薄层）

镜像 `compact_editor_commands.rs` 的「写 PENDING → 建窗/聚焦 → 前端 mount 拉取」：

```rust
// 静态 PENDING：open 时写，前端 mount 时 take。
static PENDING: Mutex<Option<PendingImage>> = Mutex::new(None);

// mode 字段为贴图模式（形态②）预留：本需求仅 "preview"，pin 分支将来再加。
// 现在带上 mode 是「打好基础」的显式标记——窗口创建暂只走 preview 分支。
struct PendingImage { image_id: i64, mode: String }  // mode: "preview"（now）/ "pin"（future）

#[tauri::command]
pub fn open_image_preview(image_id: i64, mode: Option<String>, app_handle: AppHandle);
//   → *PENDING = Some({ image_id, mode: mode.unwrap_or("preview".into()) })
//   → 窗口已存在：emit("image-preview://load", { imageId, mode }) + show + focus
//   → 否则：建窗

#[tauri::command]
pub fn get_pending_image() -> Option<PendingImage>;  // 前端 mount 时 take

#[tauri::command]
pub fn close_image_preview(app_handle: AppHandle);   // close() → Destroyed → macOS 切 Accessory
```

三个命令在 `main.rs` 的 `generate_handler!` 注册（紧邻 compact_editor 三命令）。

> `PendingImage` 经 IPC 序列化为 camelCase：`{ imageId, mode }`。

**取原图命令（`crates/desktop/src/clipboard_commands.rs`，紧邻 `get_image_thumb`）：**

```rust
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<String, String> {
    // 镜像 get_image_thumb，但读 blob（原图）而非 thumb，复用 store::get_image_blob
    // 返回 "data:image/webp;base64,..."
}
```

store 层 `get_image_blob(conn, hash)` 已存在（`crates/clipboard/src/store.rs:467`），无需新增 store 函数。

### 3.3 共享标注核心抽取（`frontend/src/lib/annotation.ts`）= 「打好基础」

把 `Screenshot/index.tsx` 里**纯函数 / 纯类型**抽到 `lib/annotation.ts`，截图改为 import（行为零变化）：

```ts
// lib/annotation.ts
export type Tool = "none" | "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
export interface Annotation {
  type: "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
  x1: number; y1: number; x2: number; y2: number;
  text?: string; points?: number[][]; color?: string;
  lineWidth?: number; fontSize?: number; number?: number; circleSize?: number;
}

// 坐标系约定（重要）：Annotation 的坐标统一用「图片原始像素空间」（natural px）。
// - 显示时：scale = 显示宽 / naturalWidth，调 drawAnnotationScaled(ctx, ann, scale)。
// - 导出时：在 natural 尺寸 canvas 上 scale=1 直接画 drawAnnotation(ctx, ann)。

export function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Annotation): void;
export function drawAnnotationScaled(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number): void;
export function drawMultilineText(ctx, text, x, y, maxWidth, fontSize): void;
export function annBounds(ann: Annotation): { x: number; y: number; w: number; h: number };
export function hitTestAnnotationPrecise(anns: Annotation[], mx: number, my: number): number | null;
export function pointToSegmentDist(px, py, x1, y1, x2, y2): number;
```

> **截图坐标系的注意点**：截图现有代码里 `Annotation` 坐标是「窗口显示空间」（`window.innerWidth` 系），导出时 `scale = bg.naturalWidth / window.innerWidth` 放大。抽取时**不改截图的行为**——截图继续用自己的显示空间坐标。`ImagePreview` 则采用**原始像素空间**坐标（见 §3.4）。两套坐标都用同一组纯函数（`drawAnnotationScaled` 的 `scale` 参数天然适配两套），函数本身不绑定任何坐标约定，只是「按 ann 里存的坐标 + 给定 scale 画」。抽取只搬函数体，不语义改动，截图回归风险低。

**截图侧改动（最小）**：`Screenshot/index.tsx` 删除这 6 个函数/2 个类型的本地定义，改为 `import { Tool, Annotation, drawAnnotation, drawAnnotationScaled, drawMultilineText, annBounds, hitTestAnnotationPrecise, pointToSegmentDist } from "@/lib/annotation"`。`hitTestAnnotationPrecise` 抽取时把内部对 `annotations` 闭包的依赖改成接收 `anns: Annotation[]` 参数（截图调用处传 `annotations`）。

### 3.4 ImagePreview 组件（`frontend/src/pages/ImagePreview/index.tsx`）

**布局**（灯箱形态——深暗场让图片本身发光，工具卡与状态条均浮动其上）：

```
    ┌─ 浮动白卡工具栏（fixed 居中贴顶，见 §3.5）──┐    ← position:fixed, top:8, translateX(-50%)
    └──────────────────────────────────────────┘
┌──────────────────────────────────────────────────────┐
│  灯箱暗场 #1c1917（absolute inset-0 overflow-auto）  │
│      ┌────────────────────────────────┐              │
│      │ <canvas> 棋盘格底（透明区可见）  │             │  ← dispW=nw*zoom, dispH=nh*zoom
│      │ <img> display:none（仅作解码源） │             │     居中，超出视口自动滚动条
│      │ <textarea> 文字草稿（相对 canvas）│            │
│      └────────────────────────────────┘              │
│                  ┌─────────────────┐                 │
│                  │ 1920 × 1080 · PNG│                │  ← 底部 EXIF 状态条（fixed 居中贴底）
│                  └─────────────────┘                 │     半透+blur，等宽 tabular-nums
└──────────────────────────────────────────────────────┘
```

- 外层 `relative h-screen overflow-hidden`，背景 `#1c1917`（灯箱暗场）。
- 滚动画布容器 `absolute inset-0 overflow-auto`（图片大于视口自动出上下/左右滚动条；小于则 `flex items-center justify-center p-12` 居中）。
- 画布外层 `relative` div，尺寸 = 显示尺寸（`dispW × dispH`）。`<canvas>` 像素尺寸 `dispW*dpr × dispH*dpr`，`ctx.setTransform(dpr,…)` 后 `ctx.scale(zoom, zoom)` 画标注；`<img>` `display:none` 仅作解码源（onload 取 naturalWidth/Height）。
- **棋盘格底**（canvas CSS 背景）：`#292524` + 20px 棋盘格纹 —— 透明 PNG 的透明区可见，专业看图工具信号；不透明区自然盖住。合成导出时另用自然尺寸画布（默认透明背景），PNG 保留透明。
- **显示尺寸 = 1:1 × zoom**：默认 `zoom=1`（自然分辨率），工具栏放大/缩小按钮调 `zoom`（0.1×–8×，步长 1.25×）。`dispW = nw*zoom, dispH = nh*zoom`。选 `tool==="none"` 时鼠标在画布上呈抓手，按住拖拽平移视口（window 级 mousemove/up，鼠标出画布仍跟随），免拖滚动条。

**坐标转换**（标注存自然像素坐标，与 zoom 解耦）：
- 鼠标事件 `e.clientX/Y` → 相对 canvas 的 CSS 坐标 `(cssX, cssY)` → 自然坐标 `nx = cssX / zoom`，`ny = cssY / zoom`。
- 显示用 `ctx.scale(zoom, zoom)` 后 `drawAnnotation(ctx, ann)`（自然坐标）；命中用自然坐标调 `hitTestAnnotationPrecise(nx, ny, annotations)`。

**交互（无选区，比截图简单）**：
- `tool === "none"`：点中标注 → 选中（可拖动移动，复用截图的拖动平移逻辑思路）；空白点击 → 取消选中。
- `tool ∈ {rect, oval, line}`：mousedown 记起点（原始坐标）→ mousemove 更新 `drawingRef` → mouseup 过滤太小的后入 `annotations`。
- `tool === "text"`：click → 在该点浮一个 `<textarea>`（样式同截图文字浮层：透明背景、虚线边、200 宽）；blur/Esc 确认或取消 → 入 `annotations`。
- 撤销：删最后一个标注（工具栏按钮 / Cmd+Z）。
- 选中删除：Delete/Backspace 删选中标注。
- 双击文字标注：进入编辑（复用截图 `editTextOrigRef` 思路）。

**导出（保存 / 复制用）**：

```ts
function composeAnnotated(): string | null {
  // 在 natural 尺寸 canvas 上 drawImage(bg) + 逐个 drawAnnotation(ann)（scale=1）→ toDataURL("image/png").split(",")[1]
}
```

- 保存：`composeAnnotated()` → base64 PNG → `invoke("save_image_dialog", { pngBase64 })`（新增薄命令，见 §3.6）。
- OCR：`invoke<string>("ocr_image", { id: imageId })`（整图识别，无裁剪）→ 文本非空则 `navigator.clipboard.writeText` + `ocrCopied=true` 1.5s 反馈（工具栏 OCR 按钮换绿勾）。不开编辑器（轻量预览场景，结果直接进剪贴板）。

**mount / 事件**：
- mount：`get_pending_image()` → `{ imageId }` → `invoke<string>("get_image_full", { id: imageId })` → `new Image()` onload 后存 `bgImgRef`、计算显示尺寸、`setReady(true)`。
- `listen("image-preview://load")`：并发再开时载入新 imageId。
- Esc → 关窗（`invoke("close_image_preview")`，键盘快捷）。鼠标关窗走原生标题栏 ×（两者都触发 Destroyed → macOS 切回 Accessory）。

### 3.5 轻工具栏组件（`frontend/src/pages/ImagePreview/Toolbar.tsx`）

> 用 frontend-design skill 设计（2026-07-01 重做）：用户要求「浮窗的出现方式跟使用方式与截图的工具栏浮窗保持一致」，故工具栏复刻截图主工具栏的**浮动白卡**形态（非贴顶深色横条），属性浮窗 1:1 复刻截图 `ToolPropsPopover`。视觉 Vernacular 来自「看图工具/灯箱」：深暗场 + 棋盘格画布底 + 底部 EXIF 状态条，是与截图区分、且体现专业看图工具气质的 signature。

**风格基准**（对齐截图主工具栏 + `ToolPropsPopover`，内联 style 与截图同出处以便两处同步微调）：
- **工具卡**：`position:fixed; left:50%; top:8px; translateX(-50%)`，`padding:6px 8px`，`background:#fff`，`border-radius:8`，`box-shadow:0 4px 16px rgba(0,0,0,0.3)`（截图同款）。
- **按钮 `ToolButton`**：32×32，`border-radius:6`，激活 `background:#3b82f6; color:#fff`、否则透明 `color:#44403c` hover `rgba(0,0,0,0.06)`。图标 18px（`h-[18px] w-[18px]`）。与截图 `ToolButton` 完全一致。
- **分隔线**：`width:1; height:20; background:rgba(0,0,0,0.08); margin:0 4px`（截图 0.1，微调柔和）。
- **缩放百分比**：等宽 `SF Mono/Menlo` + `tabular-nums`，点击重置 100%。
- 图标统一 lucide-react（不走截图的本地 SVG，减少依赖）。

**布局（左→右，同一张白卡内分组）**：

| 组 | 按钮（lucide） | 行为 |
|---|---|---|
| 操作 | `Download` 保存 / `Copy` 复制 / `ScanText` OCR（成功后换 `Check` 绿勾 1.5s） | 保存→`composePngBase64`+`save_image_dialog`；复制→`composePngBase64`+`copy_image_to_clipboard`；OCR→`ocr_image` 结果写系统剪贴板 + `ocrCopied` 反馈 |
| ｜ | 分隔线 | |
| 标注工具 | `MousePointer2` 选择 / `Square` 矩形 / `Circle` 椭圆 / `Minus` 直线 / `Type` 文字 | 单选互斥；激活 `#3b82f6` 蓝底白图标；选中任一标注工具即自动浮出属性浮窗（见下），切回选择即收起 |
| | `Undo2` 撤销 | 删最后标注；`canUndo=false` 时图标 opacity 0.3 |
| ｜ | 分隔线 | |
| 缩放 | `ZoomOut` 缩小 / 百分比（点击重置 100%）/ `ZoomIn` 放大 | `zoom` 0.1×–8×，步长 1.25×；百分比等宽 tabular-nums |
| ｜ | 分隔线 | |
| 置顶 | `Pin`/`PinOff` | toggle always-on-top；激活 `#3b82f6` |

> **不放「关闭」按钮**：窗口有原生标题栏（右上角 ×）已能关窗，工具栏再放一个冗余。关窗走原生 ×（鼠标）或 Esc（键盘）。

**属性浮窗**（1:1 复刻截图 `ToolPropsPopover`）：选中任一标注工具（`tool !== "none"`）时，从工具卡左下方 `absolute; left:0; top:calc(100%+6px)` 自动浮出 —— 白卡 `border-radius:10` + `box-shadow:0 8px 24px -4px…`，宽 240，两行：
- 行 1：label（粗细/字号，10px `#a8a29e`）+ `<input type="range">`（`accentColor` 跟当前色）+ 数值（等宽 tabular-nums）+ 当前色圆（20px，3px 白边 + 阴影，与下方预设色区分）。
- 分隔线 `rgba(0,0,0,0.06)`。
- 行 2：8 预设色（`["#ef4444","#f97316","#eab308","#22c55e","#3b82f6","#8b5cf6","#000000","#ffffff"]`），18×18 r5，全 opacity；active 用蓝 ring（`box-shadow:0 0 0 2px #fff, 0 0 0 3.5px #3b82f6`）—— 比截图（靠上方当前色圆反映）更清晰，白色加细边。文字工具显「字号 10–48」、其余显「粗细 1–10」。

> 不放截图原版的「调色板 `<input type="color">`」——8 预设色已覆盖常用，保持简单（YAGNI）。

**组件 API**：

```tsx
type PreviewTool = "none" | "rect" | "oval" | "line" | "text";

interface ToolbarProps {
  tool: PreviewTool; onTool: (t: PreviewTool) => void;
  color: string; onColor: (c: string) => void;
  size: number; onSize: (n: number) => void;   // 粗细 or 字号（按 tool 切含义/范围）
  onUndo: () => void; canUndo: boolean;
  onSave: () => void;
  onCopy: () => Promise<void>;   // 复制带标注图到系统剪贴板（composeAnnotated → copy_image_to_clipboard）
  onOcr: () => void; ocrLoading: boolean;
  pinned: boolean; onTogglePin: () => void;
}
```

### 3.6 新增薄命令：保存对话框 + 复制到剪贴板

`crates/desktop/src/clipboard_commands.rs` 新增两条薄命令：

```rust
#[tauri::command]
pub async fn save_image_dialog(png_base64: String, app_handle: AppHandle) -> Result<(), String> {
    // tauri_plugin_dialog::DialogExt save dialog → 写 PNG 字节。
    // 实现与 screenshot_commands::save_screenshot_dialog 等价（可抽公共 fn write_png_dialog(ah, base64)）。
}

#[tauri::command]
pub async fn copy_image_to_clipboard(
    png_base64: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // base64 decode → RustImageData::from_bytes → handle.set_image（写系统剪贴板）。
    // 与条目行 copy_clipboard_item 区别：这里写的是 composeAnnotated 合成的「带标注图」，非原图。
}
```

两条都注册到 `generate_handler!`。

## 4. 数据流

```
ClipboardItem 缩略图 click
  │ invoke open_image_preview(imageId)          // mode 默认 "preview"
  ▼
image_preview_commands::open_image_preview
  │ PENDING = { imageId, mode:"preview" }
  │ 建窗(首次) 或 emit load + focus(已存在)
  ▼
ImagePreview mount ──get_pending_image──► PENDING.take()
  │ invoke get_image_full(imageId) → webp dataUrl → bgImg
  │ 计算显示尺寸 → canvas 就绪
  │ 工具栏选工具 → 画标注（存原始像素坐标）→ 撤销/选中移动
  │ 保存: composeAnnotated() → save_image_dialog
  │ 复制: composeAnnotated() → copy_image_to_clipboard（带标注图进系统剪贴板）
  │ OCR:  ocr_image(imageId) → openCompactEditor → set_clipboard_item_text 回写
  │ 置顶: getCurrentWindow().setAlwaysOnTop(b)
  ▼
关闭 → close_image_preview → 销毁窗（macOS 切回 Accessory）
```

## 5. 入口（`ClipboardItem.tsx`）

剪贴板图片条目（`item.item_type === "image"`）的缩略图改为可点击：

- 当前缩略图 `<img>`（ClipboardItem.tsx:196）外包一层 `button`（或直接给 img 加 onClick），`onClick` → `e.stopPropagation()` + `invoke("open_image_preview", { imageId: item.id })`。
- 光标 `cursor-zoom-in`，`title="点击预览"`，hover 轻微放大（`hover:scale-105 transition`）。
- 单击预览**不**与「双击粘贴」「单击选中」冲突：缩略图是独立点击目标，`stopPropagation` 拦住行级 click/dblclick。

> Settings 剪贴板页（`ClipboardPanel.tsx`）若同样展示图片缩略图，后续按相同模式接入（次要，可在 plan 末尾追加）。

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 原图缺失（`get_image_blob` 返回 None） | `get_image_full` `Err("图片数据不存在")` → 前端画布区显示「图片数据缺失」+ 关闭按钮 |
| 并发再开（A 预览中，B 再开） | 后端 emit load 推 B 的 `{imageId}`；A 的内容被替换（单例，预览是短时操作，可接受） |
| ACL 未授权 | `image_preview_window` 必须在 capability `windows` 数组（§3.1）；前端跨窗口 emit/invoke 一律 `await`+`try/catch`，拒绝时显式日志（不静默吞） |
| 大图 base64 经 IPC | 原图已存 WebP（有压），可接受；缩略图仍用于列表，仅预览时拉原图 |
| WebP 解码 | 浏览器原生支持 WebP，dataUrl 直接渲染 |
| 窗口缩放 | 显示尺寸随 resize 重算，标注按原始像素坐标自动跟随缩放（不变形） |
| 文字标注 IME | 用 `<textarea>` 浮层（IME 安全），与截图文字标注一致 |
| 标注未持久化 | 关窗即失；保存走导出新图（已在 §2 声明） |

## 7. 测试

**后端单测（`image_preview_commands.rs`）：**
- `open_image_preview` 写 PENDING → `get_pending_image` 读回正确 `{imageId, mode}` 并 take 清空（镜像 compact_editor 测试）。
- `get_image_full`：内存 DB 插 image_data → 取回正确 base64（镜像 `get_image_thumb` 测试）。

**共享核心单测（`lib/annotation.ts`，纯函数易测）：**
- `drawAnnotationScaled` 的 scale 正确缩放坐标/线宽/字号（用 mock ctx 断言调用参数）。
- `annBounds` 各类型返回正确包围盒；`hitTestAnnotationPrecise` 空心标注命中边、不命中内部。
- 回归保障：截图改 import 后，截图既有行为不变（靠 e2e 兜，见下）。

**前端组件（ImagePreview / Toolbar）：**
- Toolbar：工具单选互斥、置顶 toggle 切换、OCR loading 态、属性浮窗显隐（mock invoke/emit）。

**e2e（手动，跨窗口 + canvas + IME，单测覆盖不到）：**
1. 剪贴板图片条目 → 单击缩略图 → 预览窗打开、显示原图。
2. 画矩形/椭圆/直线/文字标注 → 显示正确 → 撤销撤销 → 选中移动 → 双击改文字。
3. 保存 → 对话框 → 文件内容含标注；复制 → 系统剪贴板含带标注图（粘贴到别处验证）。
4. OCR → 文本进编辑器 → 保存回写剪贴板条目 search_text。
5. 置顶 → 窗口浮于其他应用之上。
6. Esc / 关闭按钮 → 窗销毁，macOS Dock 图标切回。
7. 回归：截图工具栏标注功能仍正常（抽取未破坏截图）。

## 8. 文档同步

- `docs/architecture.md`：窗口列表 + `image_preview_window`；命令清单 + `open_image_preview` / `get_pending_image` / `close_image_preview` / `get_image_full` / `save_image_dialog` / `copy_image_to_clipboard`；前端模块树 + `lib/annotation.ts`（共享标注核心）、`pages/ImagePreview/`。
- 本 spec → `docs/superpowers/plans/2026-07-01-image-preview.md`（writing-plans 产出）。
- `docs/superpowers/specs/2026-06-30-compact-editor-design.md`：可在末尾加一行「图片预览窗复用同款窗口/PENDING 模式」交叉引用（可选）。

## 9. 贴图模式（形态②）—— 未来，仅留基础

**目标长相**（用户参考图，Snipaste 风格）：无边框、透明、置顶；就一张带阴影的图；hover 时右上角浮出「×关闭 / 📌钉住」、右下角缩放比例。

**本需求已为其留的基础：**
- `lib/annotation.ts` 共享标注核心——贴图窗将来可直接复用（贴图上也能标注）。
- `get_image_full` 命令——贴图窗加载图片用同一个。
- `PendingImage.mode` 字段——`"pin"` 分支预留（窗口创建按 mode 切 `decorations(false)/transparent/always_on_top`，现仅 preview 分支）。
- 项目已有的透明无边框置顶 + 点击穿透机器（`result_window.rs` 的 `start_click_through_poller` / `setIgnoresMouseEvents` / CSS 伪装尺寸）——贴图窗的「钉屏 + hover 显隐工具栏」可复用这套。

**本需求不做**：不建 `image_pin_window`、不写贴图交互、不接入截图「钉住」按钮。截图工具栏的「钉住」入口属截图功能域，未来单独立项。

**多实例说明**：预览是单例（短时操作）；贴图将来需多实例（每张钉图一个窗），届时用「label 加后缀」或独立 label 方案，不影响本期预览。
