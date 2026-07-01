# 图片预览（剪贴板图片项）实施计划

> **状态（2026-07-01）：Task 1–7 已全部实施完成，双向同步合并 main（`e387933`）。** 下方代码块为实施时指引快照；**Task 4 的 draw/坐标方案在落地时由「contain-fit」改为「1:1 + zoom 倍率」**（Step 2–3 的 contain-fit 代码块已废弃，实际见 `index.tsx` 及各 Step 注）；**OCR 在落地时由「写系统剪贴板+贴画面」改为「`save_ocr_to_note` + `open_notepad_with_note` 存笔记开记事本」**（见 Task 4 Step 4）。**以实际源码为准。**

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实施。步骤用 checkbox（`- [ ]`）跟踪。

**Goal:** 为剪贴板图片项新增一个轻工具栏预览窗口（含画圈/直线/矩形/文字标注 + 撤销 + 保存/复制/OCR/置顶），并为未来「贴图钉屏」模式打好共享基础。

**Architecture:** 镜像 `compact_editor_window` 的动态窗口 + PENDING 暂存模式（open 写 PENDING → 建窗/聚焦；前端 mount 调 `get_pending_image` 取走）。标注核心从 `Screenshot/index.tsx` 抽取到共享 `frontend/src/lib/annotation.ts`（纯函数，DRY）。预览用**自然像素坐标空间**（窗口可缩放，标注存图像本征分辨率，resize 不错位）；合成保存/复制时在自然尺寸画布 1:1 重绘。

**Tech Stack:** Rust + Tauri 2（`#[tauri::command]`、`generate_handler!`、ACL capabilities）、React 19 + TypeScript + Vite 8 + Tailwind 4 + lucide-react。前端无 vitest（项目惯例：后端 `#[cfg(test)]` + `npm run build` 类型检查 + 手动 e2e）。

---

## 关键约束（贯穿所有任务）

- **不往 main 同步**：功能完整完成前，所有提交留在 `worktree-feature-notepad` 分支。
- **worktree cwd 陷阱**：Bash cwd 是主仓库；cargo 用 `--manifest-path <WT>/Cargo.toml`，npm 用 `--prefix <WT>/crates/desktop/frontend`，git 用 `git -C <WT>`，读写用绝对路径。
- **dist 已纳入 git**：前端变更必须 `npm run build` 并提交 `crates/desktop/dist`。
- **绝对路径根**（下文 `<WT>` = `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad`）。

## 文件结构

| 文件 | 责任 | 动作 |
|------|------|------|
| `crates/desktop/frontend/src/lib/annotation.ts` | 共享标注纯函数（Tool/Annotation 类型 + draw/hit/bounds） | **新建** |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | 改为从 annotation.ts import（去重） | 改 |
| `crates/desktop/src/image_preview_window.rs` | 预览窗口创建 + macOS 激活策略 | **新建** |
| `crates/desktop/src/image_preview_commands.rs` | PENDING 暂存 + open/get/close 三命令 | **新建** |
| `crates/desktop/src/clipboard_commands.rs` | 加 get_image_full / save_image_dialog / copy_image_to_clipboard | 改 |
| `crates/desktop/src/main.rs` | mod 声明 + generate_handler! 注册 6 命令 + RunEvent 路由 | 改 |
| `crates/desktop/capabilities/default.json` | windows 数组加 `image_preview_window` | 改 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 预览主组件（画布 + 标注交互） | **新建** |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | 工具栏（工具按钮 + 颜色粗细浮窗 + 动作按钮） | **新建** |
| `crates/desktop/frontend/src/App.tsx` | 路由 case `image_preview_window` | 改 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | 图片项加「预览」入口 | 改 |
| `docs/architecture.md` | 同步新模块说明 | 改 |

---

## Task 1: 抽取共享标注核心到 lib/annotation.ts

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/lib/annotation.ts`
- Modify: `<WT>/crates/desktop/frontend/src/pages/Screenshot/index.tsx`（删除 6 个内联函数 + Tool/Annotation 定义，改为 import）

**背景**：Screenshot 现把 `Tool`/`Annotation`/`drawAnnotation`/`drawAnnotationScaled`/`drawMultilineText`/`annBounds`/`hitTestAnnotationPrecise`/`pointToSegmentDist` 全部内联在组件里（`index.tsx` 约 11-23 行类型、212-494 行函数）。它们除 `hitTestAnnotationPrecise` 闭包了 `annotations` 外都是纯函数。抽到共享模块后，Screenshot 与 ImagePreview 共用，避免双份维护。

- [ ] **Step 1：新建 `lib/annotation.ts`，写入类型与纯函数**

完整内容（drawAnnotation / drawAnnotationScaled 从 Screenshot `index.tsx:212-390` **逐字搬迁**，不改逻辑；`drawMultilineText` 从 `index.tsx:286-313` 搬迁作为模块私有函数；`annBounds`/`pointToSegmentDist` 从 `:416-494` 搬迁；`hitTestAnnotationPrecise` 从 `:443-483` 搬迁但**加 `anns: Annotation[]` 参数**取代闭包）：

```ts
// 共享标注类型 + 纯绘制/命中函数。Screenshot 与 ImagePreview 共用，坐标空间由调用方决定。

export type Tool = "none" | "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";

export interface Annotation {
  type: "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
  points?: number[][];
  color?: string;
  lineWidth?: number;
  fontSize?: number;
  number?: number;
  circleSize?: number;
}

// —— 以下 drawAnnotation / drawAnnotationScaled / drawMultilineText / annBounds /
//    pointToSegmentDist / hitTestAnnotationPrecise 逐字来自 Screenshot/index.tsx ——
//    （实现见该文件；搬迁时保持完全一致，仅 hitTestAnnotationPrecise 改签名）
```

`hitTestAnnotationPrecise` 新签名（其余函数体逐字搬迁）：

```ts
const HIT_DIST = 8;

export function hitTestAnnotationPrecise(
  mx: number,
  my: number,
  anns: Annotation[],
): number | null {
  for (let i = anns.length - 1; i >= 0; i--) {
    const ann = anns[i];
    if (ann.type === "rect") {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      const onEdge = (Math.abs(mx - x) <= HIT_DIST || Math.abs(mx - (x + w)) <= HIT_DIST) && my >= y - HIT_DIST && my <= y + h + HIT_DIST
        || (Math.abs(my - y) <= HIT_DIST || Math.abs(my - (y + h)) <= HIT_DIST) && mx >= x - HIT_DIST && mx <= x + w + HIT_DIST;
      if (onEdge) return i;
    } else if (ann.type === "oval") {
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.abs(ann.x2 - ann.x1) / 2;
      const ry = Math.abs(ann.y2 - ann.y1) / 2;
      if (rx < 1 || ry < 1) continue;
      const dx = (mx - cx) / rx;
      const dy = (my - cy) / ry;
      const dist = Math.abs(Math.sqrt(dx * dx + dy * dy) - 1) * Math.min(rx, ry);
      if (dist <= HIT_DIST) return i;
    } else if (ann.type === "line" || ann.type === "arrow") {
      if (pointToSegmentDist(mx, my, ann.x1, ann.y1, ann.x2, ann.y2) <= HIT_DIST) return i;
    } else if (ann.type === "pen" && ann.points) {
      for (let j = 1; j < ann.points.length; j++) {
        const [px1, py1] = ann.points[j - 1];
        const [px2, py2] = ann.points[j];
        if (pointToSegmentDist(mx, my, px1, py1, px2, py2) <= HIT_DIST) return i;
      }
    } else {
      const b = annBounds(ann);
      if (mx >= b.x && mx <= b.x + b.w && my >= b.y && my <= b.y + b.h) return i;
    }
  }
  return null;
}
```

导出清单：`Tool`, `Annotation`, `drawAnnotation`, `drawAnnotationScaled`, `annBounds`, `hitTestAnnotationPrecise`, `pointToSegmentDist`。`drawMultilineText`/`HIT_DIST` 模块私有不导出（仅 drawAnnotation 内部用）。

- [ ] **Step 2：Screenshot 改为 import**

在 `Screenshot/index.tsx` 顶部加：

```ts
import { type Annotation, type Tool, drawAnnotation, drawAnnotationScaled, annBounds, hitTestAnnotationPrecise, pointToSegmentDist } from "@/lib/annotation";
```

删除组件内 `type Tool = ...`、`interface Annotation {...}`（11-23 行）及 6 个内联函数（212-494 行区间内的 `drawAnnotation`/`drawAnnotationScaled`/`drawMultilineText`/`annBounds`/`hitTestAnnotationPrecise`/`pointToSegmentDist`/`HIT_DIST`）。

- [ ] **Step 3：更新 hitTestAnnotationPrecise 调用点**

`hitTestAnnotationPrecise` 现需 `anns` 参数。grep 出所有调用点并补参：

```bash
cd <WT> && grep -rn "hitTestAnnotationPrecise(" crates/desktop/frontend/src/pages/Screenshot/
```

每个 `hitTestAnnotationPrecise(mx, my)` → `hitTestAnnotationPrecise(mx, my, annotations)`。

- [ ] **Step 4：类型检查 + 构建验证（截图回归不破）**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 构建成功，无 unused / type error。

- [ ] **Step 5：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/lib/annotation.ts crates/desktop/frontend/src/pages/Screenshot/index.tsx
git -C <WT> commit -m "refactor(desktop): 抽取标注核心到 lib/annotation.ts 供截图与图片预览共用"
```

---

## Task 2: 后端 — 预览窗口 + PENDING 命令 + 注册 + ACL

**Files:**
- Create: `<WT>/crates/desktop/src/image_preview_window.rs`
- Create: `<WT>/crates/desktop/src/image_preview_commands.rs`
- Modify: `<WT>/crates/desktop/src/main.rs`（mod 声明 + generate_handler! + RunEvent）
- Modify: `<WT>/crates/desktop/capabilities/default.json`（windows 数组）

- [ ] **Step 1：新建 `image_preview_window.rs`**（镜像 `compact_editor_window.rs`）

```rust
//! 图片预览窗口：动态创建（非预建隐藏窗）。
//! 打开 → macOS 切 Regular（Dock 出现）；关闭 → RunEvent::Destroyed 路由回 Accessory。

use tauri::{ActivationPolicy, Manager, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 620.0;
const MIN_WIDTH: f64 = 400.0;
const MIN_HEIGHT: f64 = 320.0;

pub const WINDOW_LABEL: &str = "image_preview_window";

pub fn create_image_preview_window(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
        .title("图片预览")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .decorations(true)
        .resizable(true)
        .center()
        .visible(true)
        .build();
}

/// 窗口销毁后恢复 Accessory（Dock 图标隐藏），与 compact_editor 一致。
#[cfg(target_os = "macos")]
pub fn on_image_preview_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
}
```

- [ ] **Step 2：新建 `image_preview_commands.rs`**（镜像 `compact_editor_commands.rs` 的 PENDING 模式）

```rust
//! 图片预览命令层：PENDING 暂存 + 开/取/关三个命令。
//! 模式同 compact_editor：open 先写 PENDING 再建窗/聚焦；前端 mount 调 get_pending_image 取走。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::image_preview_window::{create_image_preview_window, WINDOW_LABEL};

/// 跨窗口传递的预览载荷。rename_all=camelCase → 前端拿到 { imageId }。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingImage {
    pub image_id: i64,
}

static PENDING: Mutex<Option<PendingImage>> = Mutex::new(None);

fn store_pending(image_id: i64) {
    *PENDING.lock().unwrap() = Some(PendingImage { image_id });
}

fn take_pending() -> Option<PendingImage> {
    PENDING.lock().unwrap().take()
}

/// 打开图片预览：写 PENDING；已存在则 emit load 推送新 id + 聚焦，否则建窗。
#[tauri::command]
pub fn open_image_preview(image_id: i64, app_handle: tauri::AppHandle) {
    store_pending(image_id);
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("image-preview://load", PendingImage { image_id });
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_image_preview_window(&app_handle);
    }
}

/// 前端 mount 时拉取（take 清空）。
#[tauri::command]
pub fn get_pending_image() -> Option<PendingImage> {
    take_pending()
}

/// 关闭预览窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_image_preview(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_store_and_take_roundtrip() {
        let _ = take_pending(); // 清残留
        store_pending(42);
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.image_id, 42);
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
```

- [ ] **Step 3：main.rs 注册 mod + 命令 + RunEvent 路由**

定位锚点：

```bash
cd <WT> && grep -n "mod compact_editor_commands\|mod compact_editor_window\|open_compact_editor\|compact_editor_window =>" crates/desktop/src/main.rs
```

3a. mod 声明区（紧跟 compact_editor 两行后）加：

```rust
mod image_preview_commands;
mod image_preview_window;
```

3b. `generate_handler!` 宏里，compact_editor 三命令后加：

```rust
image_preview_commands::open_image_preview,
image_preview_commands::get_pending_image,
image_preview_commands::close_image_preview,
```

3c. RunEvent::WindowEvent { Destroyed } 的 match 里，`compact_editor_window` 分支旁加：

```rust
"image_preview_window" => image_preview_window::on_image_preview_closed(&app_handle),
```

（macOS 分支内；非 macOS 不需调用，`on_image_preview_closed` 本身 `#[cfg(target_os="macos")]`。若 RunEvent 处理器在非 macOS 编译报「未使用」，参考 compact_editor 的同位置写法保持一致——它在 main.rs 已通过 `#[cfg]` 处理。）

- [ ] **Step 4：capabilities ACL — windows 数组加 label**

`<WT>/crates/desktop/capabilities/default.json` 第 4 行 windows 数组追加 `"image_preview_window"`：

```json
"windows": ["main", "result_window", "settings_window", "clipboard_window", "notepad_window", "compact_editor_window", "image_preview_window", "screenshot_*"],
```

> 理由：动态窗口 label 未列入 capability → 前端 invoke/emit/listen 全被 ACL 静默拦（见 memory `tauri-dynamic-window-capability`）。诊断信号：后端 emit 能收、前端 emit 回不来。

- [ ] **Step 5：测试 + 编译验证**

Run: `cargo test --manifest-path <WT>/Cargo.toml -p octopus-desktop image_preview_commands`
Expected: `pending_store_and_take_roundtrip` PASS。

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 编译成功（含 RunEvent 处理器）。

- [ ] **Step 6：提交**

```bash
git -C <WT> add crates/desktop/src/image_preview_window.rs crates/desktop/src/image_preview_commands.rs crates/desktop/src/main.rs crates/desktop/capabilities/default.json
git -C <WT> commit -m "feat(desktop): 图片预览窗口 + PENDING 命令 + ACL 注册"
```

---

## Task 3: 后端 — 图片获取/保存/复制命令

**Files:**
- Modify: `<WT>/crates/desktop/src/clipboard_commands.rs`（加 3 命令）
- Modify: `<WT>/crates/desktop/src/main.rs`（generate_handler! 注册 3 命令）

**背景**：`clipboard_commands.rs` 顶部已 `use base64::{Engine, engine::general_purpose};` + `use octopus_clipboard::{ClipboardHandle, ...}` + `State<'_, Arc<ClipboardHandle>>` 模式已建立。`ClipboardHandle::write_image(&[u8])`（handle.rs:50）内部已 `from_bytes`+`set_image`，故复制无需碰 `RustImageData`。

- [ ] **Step 1：加 `get_image_full`**（镜像 `get_image_thumb`，读 `blob` 而非 `thumb`）

定位：`grep -n "pub async fn get_image_thumb\|get_image_blob" <WT>/crates/desktop/src/clipboard_commands.rs`。在 `get_image_thumb` 旁加：

```rust
/// 取图片全分辨率（image_data.blob）→ data URL（base64 + WebP 前缀）。
/// 前端 ImagePreview 用它加载到 <img>/canvas。
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<String, String> {
    let hash = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &image_hash_for(conn, id)?)
            .ok_or_else(|| "图片数据缺失".to_string())
    })
    .map_err(|e| e.to_string())??;
    // 注意：上面的闭包返回需对齐 get_image_thumb 的写法（见现有实现）。
    // 简化版：直接复用 get_image_thumb 的取 hash → 取 blob → encode 逻辑，仅把 get_image_thumb 换成 get_image_blob。
    Ok(format!("data:image/webp;base64,{}", general_purpose::STANDARD.encode(&hash)))
}
```

> **落地注意**：上面伪表达仅为示意。实现时**逐字对照现有 `get_image_thumb` 的函数体**（它已正确处理「取 hash + 取 thumb + encode + 返回 data URL」），把其中 `get_image_thumb(conn, &hash)` 换成 `get_image_blob(conn, &hash)`、mime 仍是 `image/webp`（blob 存的是 WebP）。保持错误处理、Option 展开方式与之一致，避免类型不齐。

- [ ] **Step 2：加 `save_image_dialog`**（镜像 `screenshot_commands::save_screenshot_dialog`，**去掉截图专属清理**）

```rust
/// 弹系统保存对话框，把前端合成的标注 PNG（base64）存到用户指定路径。
#[tauri::command]
pub async fn save_image_dialog(
    png_base64: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    use tauri_plugin_dialog::DialogExt;
    let save_path = app_handle
        .dialog()
        .file()
        .add_filter("PNG 图片", &["png"])
        .set_file_name("image.png")
        .blocking_save_file();

    if let Some(path) = save_path {
        let path = path.as_path().ok_or("无效路径")?;
        std::fs::write(path, &png_bytes).map_err(|e| e.to_string())?;
        log::info!("Image preview saved to {}", path.display());
    }
    Ok(())
}
```

- [ ] **Step 3：加 `copy_image_to_clipboard`**（decode → `handle.write_image`）

```rust
/// 把前端合成的标注 PNG（base64）写入系统剪贴板。
#[tauri::command]
pub async fn copy_image_to_clipboard(
    png_base64: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;
    handle.write_image(&png_bytes).map_err(|e| e.to_string())
}
```

- [ ] **Step 4：main.rs generate_handler! 注册 3 命令**

定位 `get_image_thumb` 注册处（`grep -n "get_image_thumb" <WT>/crates/desktop/src/main.rs`），其旁加：

```rust
clipboard_commands::get_image_full,
clipboard_commands::save_image_dialog,
clipboard_commands::copy_image_to_clipboard,
```

- [ ] **Step 5：编译验证**

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 成功。若 `get_image_full` 闭包返回类型报错，回到 Step 1 对齐 `get_image_thumb` 的写法。

- [ ] **Step 6：提交**

```bash
git -C <WT> add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git -C <WT> commit -m "feat(desktop): get_image_full / save_image_dialog / copy_image_to_clipboard 命令"
```

---

## Task 4: 前端 — ImagePreview 组件（画布 + 标注交互）

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**坐标空间约定**（自然像素）：
- 标注 `Annotation` 的坐标/线宽/字号均为**图像本征像素**（与显示尺寸无关，resize 不错位）。
- `dispW/dispH` = contain-fit 后的显示尺寸；`natW/natH` = 图像本征尺寸。
- 鼠标 CSS 坐标 → 自然：`nx = cssX / dispW * natW`，`ny = cssY / dispH * natH`。
- 绘制：`ctx.save(); ctx.scale(dispW/natW, dispH/natH); drawAnnotation(ctx, ann); ctx.restore();`
- 合成保存/复制：离屏画布 natW×natH，`drawImage` + `drawAnnotation` 1:1（无 scale）。

- [ ] **Step 1：新建组件骨架 + 加载图片**

```tsx
import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import {
  type Annotation,
  type Tool,
  drawAnnotation,
  annBounds,
  hitTestAnnotationPrecise,
} from "@/lib/annotation";
import Toolbar from "./Toolbar";

export default function ImagePreview() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);

  const [imageId, setImageId] = useState<number | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [natW, setNatW] = useState(0);
  const [natH, setNatH] = useState(0);
  // dispW/dispH 由 contain-fit 在 draw 时算（依赖窗口尺寸），存 ref 避免重渲染抖动
  const dispRef = useRef({ w: 0, h: 0, ox: 0, oy: 0 });

  const [tool, setTool] = useState<Tool>("none");
  const [toolColor, setToolColor] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSize] = useState(20);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);

  // 交互 refs
  const drawingRef = useRef<Annotation | null>(null);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);
  const startRef = useRef({ x: 0, y: 0 });
  const textDraftRef = useRef<{ nx: number; ny: number } | null>(null);
  const [textDraft, setTextDraft] = useState<{ nx: number; ny: number; val: string } | null>(null);

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  useEffect(() => {
    invoke<{ imageId: number } | null>("get_pending_image").then((p) => {
      if (p) setImageId(p.imageId);
    });
    const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
      setImageId(e.payload.imageId);
      setAnnotations([]); // 切图清空标注
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // —— imageId 变 → 拉全图 ——
  useEffect(() => {
    if (imageId == null) return;
    invoke<string>("get_image_full", { id: imageId })
      .then((url) => {
        setDataUrl(url);
        setAnnotations([]);
      })
      .catch((e) => console.error(e));
  }, [imageId]);
  // ...（继续 Step 2 draw、Step 3 鼠标、Step 4 工具栏接线、Step 5 compose）
}
```

- [x] **Step 2：`draw`** ~~—— contain-fit + 图片 + 标注 + 草稿~~

  > ⚠️ **方案已改（2026-07-01）**：落地时放弃 contain-fit，改为 **默认 1:1（zoom=1）+ zoom 倍率缩放 + 超窗滚动条 + 抓手平移**。下方 contain-fit 代码块**已废弃**，实际 `draw` 见 `index.tsx`（`dispW=natW*zoom`、`ctx.scale(zoom,zoom)` 画标注、`onLoad` 取 naturalWidth/Height、棋盘格 CSS 底）。保留此块仅作历史。

```tsx
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img || !natW || !natH) return;
    const cssW = canvas.clientWidth;
    const cssH = canvas.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(cssW * dpr);
    canvas.height = Math.round(cssH * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // contain-fit
    const scale = Math.min(cssW / natW, cssH / natH);
    const dispW = natW * scale;
    const dispH = natH * scale;
    const ox = (cssW - dispW) / 2;
    const oy = (cssH - dispH) / 2;
    dispRef.current = { w: dispW, h: dispH, ox, oy };
    ctx.drawImage(img, ox, oy, dispW, dispH);

    // 标注：自然坐标 → 先平移到显示原点 + 缩放到 disp
    ctx.save();
    ctx.translate(ox, oy);
    ctx.scale(scale, scale);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    if (drawingRef.current) drawAnnotation(ctx, drawingRef.current);
    ctx.restore();

    // 文字草稿（DOM <textarea> 叠加，此处不画）
    void textDraft; void tool;
  }, [natW, natH, annotations, textDraft, tool]);

  useEffect(() => { draw(); }, [draw]);
  // 窗口 resize 重绘
  useEffect(() => {
    const onResize = () => draw();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [draw]);
```

`<img>` 元素：渲染一个隐藏 `<img>`（`display:none`，仅作解码源），`onLoad` 记录 `natW/natH`：

```tsx
  {dataUrl && (
    <img
      ref={imgRef}
      src={dataUrl}
      alt=""
      style={{ display: "none" }}
      onLoad={(e) => {
        setNatW(e.currentTarget.naturalWidth);
        setNatH(e.currentTarget.naturalHeight);
      }}
    />
  )}
```

- [x] **Step 3：鼠标交互（自然坐标转换 + 各工具）**

  > ⚠️ **方案已改（2026-07-01）**：下方 `toNatural` 用 `dispRef`（contain-fit 的 ox/oy/scale）**已废弃**；实际用 zoom：`nx = cssX / zoom, ny = cssY / zoom`（见 `index.tsx` 的 `toNatural`/`canvasCoords`）。鼠标交互逻辑（down/move/up、文字草稿、撤销）本身不变；实际还补了 **pen（画笔点序列 push）** 与 **抓手平移 `startPan`**（tool==="none" 未命中标注时拖拽平移视口，window 级 mousemove/up）。

```tsx
  // CSS 坐标（相对 canvas）→ 自然坐标
  const toNatural = (cssX: number, cssY: number) => {
    const { w, h, ox, oy } = dispRef.current;
    const scale = w && h ? (natW / w) : 1; // nat/disp
    return { nx: (cssX - ox) * scale, ny: (cssY - oy) * scale };
  };

  const canvasCoords = (e: React.MouseEvent) => {
    const rect = canvasRef.current!.getBoundingClientRect();
    return { cssX: e.clientX - rect.left, cssY: e.clientY - rect.top };
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    startRef.current = { x: nx, y: ny };

    // 文字草稿确认
    if (textDraftRef.current && textDraftRef.current.val.trim()) {
      commitText();
    } else {
      setTextDraft(null);
      textDraftRef.current = null;
    }

    if (tool === "none") {
      // 选择/移动：命中已有标注
      const idx = hitTestAnnotationPrecise(nx, ny, annotations);
      if (idx != null) {
        dragRef.current = { idx, dx: nx - annotations[idx].x1, dy: ny - annotations[idx].y1 };
      }
      return;
    }

    if (tool === "text") {
      textDraftRef.current = { nx, ny };
      setTextDraft({ nx, ny, val: "" });
      return;
    }

    // rect/oval/line 开始绘制
    drawingRef.current = {
      type: tool as Annotation["type"],
      x1: nx, y1: ny, x2: nx, y2: ny,
      color: toolColor, lineWidth: toolWidth,
    };
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    if (dragRef.current) {
      const { idx, dx, dy } = dragRef.current;
      setAnnotations((prev) => prev.map((a, i) => {
        if (i !== idx) return a;
        const mx = nx - dx, my = ny - dy;
        const w = a.x2 - a.x1, h = a.y2 - a.y1;
        return { ...a, x1: mx, y1: my, x2: mx + w, y2: my + h };
      }));
      return;
    }
    if (drawingRef.current) {
      drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
      draw();
    }
  };

  const onMouseUp = () => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触（过小）
      if (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3) {
        setAnnotations((prev) => [...prev, ann]);
      } else {
        draw();
      }
    }
    dragRef.current = null;
  };

  const commitText = () => {
    const d = textDraftRef.current;
    if (d && d.val.trim()) {
      setAnnotations((prev) => [...prev, {
        type: "text", x1: d.nx, y1: d.ny, x2: d.nx, y2: d.ny,
        text: d.val, color: toolColor, fontSize: toolFontSize,
      }]);
    }
    textDraftRef.current = null;
    setTextDraft(null);
  };

  const undo = () => setAnnotations((prev) => prev.slice(0, -1));
```

- [ ] **Step 4：compose 出口（保存/复制/OCR/置顶）**

```tsx
  // 把 图像 + 标注 合成到自然尺寸 PNG → base64（不含 data: 前缀）
  const composePngBase64 = async (): Promise<string> => {
    const img = imgRef.current!;
    const c = document.createElement("canvas");
    c.width = natW; c.height = natH;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(img, 0, 0, natW, natH);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    const dataUrl = c.toDataURL("image/png");
    return dataUrl.substring(dataUrl.indexOf(",") + 1);
  };

  const handleSave = async () => {
    try {
      const b64 = await composePngBase64();
      await invoke("save_image_dialog", { pngBase64: b64 });
    } catch (e) { console.error(e); }
  };

  const handleCopy = async () => {
    try {
      const b64 = await composePngBase64();
      await invoke("copy_image_to_clipboard", { pngBase64: b64 });
    } catch (e) { console.error(e); }
  };

  // ⚠️ 实际落地版（2026-07-01 改）：OCR 结果不再写系统剪贴板、不贴画面，
  //    而是 save_ocr_to_note 存为笔记 + open_notepad_with_note 打开记事本选中。
  const handleOcr = async () => {
    if (imageId == null) return;
    try {
      const text = await invoke<string>("ocr_image", { id: imageId });
      if (text) {
        const noteId = await invoke<number>("save_ocr_to_note", { text });
        await invoke("open_notepad_with_note", { noteId });
        setOcrCopied(true);
        setTimeout(() => setOcrCopied(false), 1500);  // OCR 按钮换 Check 绿勾 1.5s
      }
    } catch (e) { console.error(e); }
  };

  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    setAlwaysOnTop(next);
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(next);
    } catch (e) { console.error(e); }
  };

  const close = async () => {
    try { await invoke("close_image_preview"); } catch (e) { console.error(e); }
  };
  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      if ((e.metaKey || e.ctrlKey) && e.key === "z") undo();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [annotations]);
```

- [ ] **Step 5：渲染（灯箱暗场 + 棋盘格画布 canvas + 文字草稿 textarea + 浮动工具栏 + 底部 EXIF 条）**

> 2026-07-01 按 frontend-design 重做：外层从「贴顶深条 flex-col」改为「灯箱暗场 `#1c1917` + 全屏滚动画布 + 浮动白卡工具栏（fixed 居中）+ 底部 EXIF 状态条」。canvas 加棋盘格 CSS 底（透明 PNG 可读）。默认 1:1（zoom=1），缩放/平移见 §3.4。

```tsx
  return (
    // 灯箱暗场：工具卡与底部 EXIF 条均 fixed 浮于其上
    <div className="relative h-screen overflow-hidden select-none" style={{ background: "#1c1917" }}>
      <Toolbar
        tool={tool} setTool={setTool}
        toolColor={toolColor} setToolColor={setToolColorSync}
        toolWidth={toolWidth} setToolWidth={setToolWidthSync}
        toolFontSize={toolFontSize} setToolFontSize={setToolFontSizeSync}
        alwaysOnTop={alwaysOnTop} onToggleTop={toggleAlwaysOnTop}
        onSave={handleSave} onCopy={handleCopy} onOcr={handleOcr}
        onUndo={undo} canUndo={annotations.length > 0}
      />
      <div className="relative flex-1 overflow-hidden">
        <canvas
          ref={canvasRef}
          className="absolute inset-0 w-full h-full"
          style={{ cursor: tool === "none" ? "default" : "crosshair" }}
          onMouseDown={onMouseDown}
          onMouseMove={onMouseMove}
          onMouseUp={onMouseUp}
        />
        {/* 文字草稿：DOM textarea 叠在画布上，输入完点别处 commit */}
        {textDraft && (() => {
          const { w, ox, oy } = dispRef.current;
          const scale = w / natW;
          const left = ox + textDraft.nx * scale;
          const top = oy + textDraft.ny * scale;
          return (
            <textarea
              autoFocus
              value={textDraft.val}
              onChange={(e) => {
                const v = e.target.value;
                setTextDraft({ ...textDraft, val: v });
                textDraftRef.current = { nx: textDraft.nx, ny: textDraft.ny, val: v } as any;
                // 注意：textDraftRef 需同步 val，见下注
              }}
              onBlur={commitText}
              className="absolute bg-white/90 text-black outline-none resize-none px-1"
              style={{ left, top, fontSize: toolFontSize * scale, minWidth: 120 }}
            />
          );
        })()}
      </div>
    </div>
  );
}
```

> **注**：`textDraftRef` 同步——为保证 `commitText` 读到最新 val，`onChange` 里把 ref 也更新。上面的 `as any` 占位需在实现时用正确类型（`{nx,ny,val}` 全字段）。落地时确保 `textDraftRef.current` 与 `textDraft.val` 同步（参考 Screenshot 的 `textDraftRef`/`editTextOrigRef` 双写模式）。

- [ ] **Step 6：类型检查验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc 通过（Toolbar 尚未建会有 import 报错 → 先建 Task 5 的 Toolbar 骨架再 build，或把 Step 6 放到 Task 5 后）。本任务内可先 `npx tsc -b` 查类型。

- [ ] **Step 7：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/pages/ImagePreview/index.tsx
git -C <WT> commit -m "feat(desktop): ImagePreview 组件（画布 + 标注交互 + compose 出口）"
```

---

## Task 5: 前端 — 工具栏 Toolbar 组件

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`

**设计**（2026-07-01 按 frontend-design 重做：浮动白卡对齐截图主工具栏，属性浮窗 1:1 复刻截图 `ToolPropsPopover`；内联 style 与截图同出处）：
- 工具卡：`position:fixed; left:50%; top:8; translateX(-50%)`，白底 r8 + `box-shadow:0 4px 16px rgba(0,0,0,0.3)`（截图同款）。
- `ToolButton`：32×32 r6，激活 `#3b82f6` 蓝底白字、否则透明 `#44403c` hover `rgba(0,0,0,0.06)`，图标 18px。`Divider` 竖线 `rgba(0,0,0,0.08)`。
- 布局分组（左→右）：操作(保存/复制/OCR) ｜ 标注(选择/矩形/椭圆/直线/文字/撤销) ｜ 缩放(缩小/百分比/放大) ｜ 置顶。
- 属性浮窗：`tool !== "none"` 时从工具卡左下 `absolute top:calc(100%+6px)` 自动浮出（无单独调色板按钮）；白卡 r10 + 两行（滑轨+当前色圆 / 8 预设色 active 蓝环）；文字→字号 10–48、其余→粗细 1–10；不放 `<input type="color">` 调色板（YAGNI）。
- 缩放百分比等宽 `SF Mono` + `tabular-nums`，点击重置 100%；OCR 成功后按钮换绿勾 1.5s。
- 无关闭按钮（用窗口右上角 × 或 Esc）

- [x] **Step 1：新建 Toolbar.tsx**

> 已实现（`crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`）。结构见本 Task 顶部「设计」段：浮动白卡（fixed 居中）+ `ToolButton`(32×32/激活 `#3b82f6`) + `Divider`，分组 操作｜标注｜缩放｜置顶，属性浮窗 `tool!=="none"` 时自动浮出。
>
> **演进**：初版是贴顶 `neutral-800` 横条 + 单独 `Palette` 按钮触发浮窗；2026-07-01 按 frontend-design 重做为浮动白卡 + 自动浮出（对齐截图），旧版代码已废弃不再保留于此。
```

- [ ] **Step 2：类型检查 + 构建**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 成功（ImagePreview + Toolbar 一起编译通过）。

- [ ] **Step 3：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx
git -C <WT> commit -m "feat(desktop): ImagePreview 工具栏（工具 + 颜色粗细浮窗 + 动作）"
```

---

## Task 6: 前端 — 路由 + 剪贴板入口

**Files:**
- Modify: `<WT>/crates/desktop/frontend/src/App.tsx`
- Modify: `<WT>/crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1：App.tsx 加路由 case**

定位 `switch (label)`（`grep -n "compact_editor_window\|switch (label)\|case \"" <WT>/crates/desktop/frontend/src/App.tsx`）。在 `case "compact_editor_window"` 旁加：

```tsx
case "image_preview_window": return <ImagePreview />;
```

并在文件顶部 import：

```tsx
import ImagePreview from "./pages/ImagePreview";
```

- [ ] **Step 2：ClipboardItem.tsx 加「预览」入口**

图片项操作组（save/ocr 按钮所在 `<div className="flex-shrink-0 flex items-center gap-0.5">`）最前加一个预览按钮。import 加 `Maximize2`：

```tsx
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, ScanText, Loader2, Check, SquarePen, Maximize2 } from "lucide-react";
```

在 `{item.item_type === "image" && (` 的保存按钮**之前**插入：

```tsx
{item.item_type === "image" && (
  <button
    className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
    onClick={(e) => {
      e.stopPropagation();
      invoke("open_image_preview", { imageId: item.id }).catch(console.error);
    }}
    title="预览"
  >
    <Maximize2 className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
  </button>
)}
```

> 双击仍走 `paste_clipboard_item`（不变）；预览走独立按钮，互不冲突。

- [ ] **Step 3：构建验证 + 提交 dist**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 成功，`crates/desktop/dist` 更新。

```bash
git -C <WT> add crates/desktop/frontend/src/App.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C <WT> commit -m "feat(desktop): 图片预览路由 + 剪贴板图片项预览入口"
```

---

## Task 7: 构建总验 + 文档同步

**Files:**
- Modify: `<WT>/docs/architecture.md`

- [ ] **Step 1：后端总编译 + 测试**

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop && cargo test --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 编译成功，所有测试通过（含新 `pending_store_and_take_roundtrip`）。

- [ ] **Step 2：前端总构建**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 成功。

- [ ] **Step 3：architecture.md 同步**

在桌面模块说明里补：图片预览窗口（`image_preview_window` / `image_preview_commands` / `ImagePreview` 组件）、共享标注核心 `frontend/src/lib/annotation.ts`、新增命令（`open_image_preview`/`get_pending_image`/`close_image_preview`/`get_image_full`/`save_image_dialog`/`copy_image_to_clipboard`）。

- [ ] **Step 4：提交 + 交 e2e**

```bash
git -C <WT> add docs/architecture.md
git -C <WT> commit -m "docs(desktop): 同步图片预览模块到 architecture.md"
```

至此代码完整、构建全绿。**交用户做 e2e**（功能完成且 e2e 通过后再考虑合并 main）。

---

## Spec Coverage

| Spec（2026-07-01-image-preview-design.md）section | 覆盖 Task |
|---|---|
| 轻工具栏预览（窗口 + 工具栏） | T2, T5 |
| 标注：圆/线/矩形/文字 | T1, T4 |
| 选择(移动)/撤销/颜色·粗细浮窗 | T4(选择/撤销), T5(浮窗) |
| 保存/复制/OCR/置顶 | T3(命令), T4(compose/接线), T5(按钮) |
| 共享核心抽取（不重复） | T1 |
| 数据流：open→PENDING→get_pending→get_image_full | T2, T3, T4 |
| 动态窗口 ACL | T2 Step 4 |
| macOS 激活策略 Regular/Accessory | T2 Step 1/3 |
| 贴图钉屏（未来，仅打基础） | T1 共享核心 + T4 compose 复用（本期不建贴图窗口） |

## 风险提示

- **`hitTestAnnotationPrecise` 调用点**：抽取后签名变 `(mx,my,anns)`，Screenshot 内所有调用必须补参（Task 1 Step 3 grep 全覆盖）。漏改 → tsc 报错（能拦住）。
- **`get_image_full` 闭包返回类型**：须对齐 `get_image_thumb` 现有写法，别凭空写返回逻辑。
- **textDraft ref 同步**：textarea 受控 + ref 双写（参考 Screenshot 模式），否则 commit 读不到最新输入。
- **dist 提交**：Task 6/任何前端变更后必须 build 并提交 dist，否则 Tauri 跑旧前端。
- **不合并 main**：所有提交留 worktree 分支，e2e 通过后再议。
