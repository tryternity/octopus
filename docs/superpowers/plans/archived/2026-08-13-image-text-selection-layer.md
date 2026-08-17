# 图片文字选择层（Image Text Selection Layer）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打开图片自动 OCR → 渲染 HTML 透明文字层 → 用户原生拖选文字复制（对标 macOS Live Text / PixPin）。

**Architecture:** paddle-ocr 透出 word-level box（`return_word_box: true`）→ `OcrBlock` 扩展 `words` 字段 → 前端 `TextSelectLayer.tsx` 渲染透明 `<span>` overlay（`color: transparent` + `user-select: text`）→ 容器 `transform: scale(zoom)` 缩放零重算。

**Tech Stack:** Rust + Tauri 2 + React 19 + TypeScript + Tailwind v4

## 实施状态表（2026-08-13）

| Task | 状态 | 备注 |
|---|---|---|
| **Task 1**：后端 OcrBlock 扩展 words + paddle-ocr 透出 | ✅ 完成 | OcrWord struct + ocr_output_to_blocks 提取 word_boxes + merge_same_line_blocks 串联 + paddle_backend `return_word_box: Some(true)`。偏差见 spec §10.2-1（WordBox 路径 re-export）。2 单测通过。 |
| **Task 2**：后端 DTO 同步 + config 开关 | ✅ 完成 | OcrTextBlock 加 `words: Option<Vec<octopus_ocr::engine::OcrWord>>`（偏差见 §10.2-2）。config `image_preview_auto_ocr: bool`（default true）已加到 `config.rs`，但前端 useOcr 自动 OCR 未 wire 此 gate（始终开启，YAGNI——后续如需关闭可读 config gate）。 |
| **Task 3**：前端 TextSelectLayer.tsx + OcrBlock interface | ✅ 完成 | TextSelectLayer + memo + OcrWord/OcrBlock interface 加 words。 |
| **Task 4**：前端自动 OCR + index.tsx 集成 | ✅ 完成 | useOcr mount effect 合并（自动 OCR + 缓存拉取同一 effect，偏差 §10.2-4）；TextSelectLayer 挂为 wrapper sibling（偏差 §10.2-3）；SVG rect pointerEvents 改 none + 删 onDoubleClick（偏差 §10.2-5）；tool="none" 复用为文字选择工具（偏差 §10.2-6）。 |
| **Task 5**：联调 + 文档同步 | ✅ 代码+文档完成；e2e 跳过 | Step 1（全量 build）✅、Step 2（手动 e2e）❌ 跳过——GUI 测试由用户跑、Step 3（文档同步）✅、额外 dead code 清理 ✅（handleOcrBlockCopy 链）。 |

**Step 2 待用户验证清单**（GUI e2e，跑前 `cargo run --profile optimize -p octopus-desktop --features embedded,cloud,custom-protocol` 或 dev run-octopus.sh）：

- [ ] 打开含文字的图片 → 自动 OCR → 鼠标移到文字上变 I-beam 光标
- [ ] 拖选多个 word → 高亮选中 → Ctrl+C → 粘贴验证
- [ ] tool 切到 rect → 画标注 → 文字层不拦截
- [ ] tool 切回 none → 拖选恢复
- [ ] 缩放（zoom in/out）→ 文字层跟随缩放坐标不错位
- [ ] 双击 word → 原生选词
- [ ] 长图滚动 → 文字层跟随（sticky canvas 对齐）
- [ ] CJK 文本（中文）→ 每个 CJK 字符是一个 word box → 可逐字选

---

## Global Constraints

- **工作目录**：`.worktrees/research-tolaria-comparison`（分支 `research/tolaria-comparison`）
- **casing 规范**：`#[serde(rename_all = "camelCase")]`（OcrWord 新 struct）；前端 TS interface camelCase
- **向后兼容**：`OcrBlock.words` 用 `Option` + `skip_serializing_if`，旧 OCR 缓存无 words 时 fallback 行级 span
- **不改 `OcrBackend` trait 签名**——只在 `paddle_backend.rs` 的 `OcrCallOptions` 里设 `return_word_box: Some(true)`
- **现有 OCR 三态展示（off/overlay/mask）不变**——SVG 高亮层保留，仅 pointerEvents 从 rect 拦截改为不拦截
- **config 开关**：`image_preview_auto_ocr`（默认 true），false 时回退手动 OCR 行为
- **前端改动后需** `touch crates/desktop/src/main.rs` 强制 cargo 重新嵌入 dist

---

## File Structure

**新建文件：**
| 文件 | 职责 |
|---|---|
| `crates/desktop/frontend/src/pages/ImagePreview/TextSelectLayer.tsx` | HTML 透明文字层（每个 word 一个 span，原生拖选） |

**修改文件：**
| 文件 | 改动 |
|---|---|
| `crates/ocr/src/engine.rs` | `OcrBlock` 加 `words` 字段 + `OcrWord` struct + `ocr_output_to_blocks` 提取 word_boxes + `merge_same_line_blocks` 串联 words |
| `crates/ocr/src/paddle_backend.rs:68` | `OcrCallOptions::default()` → `return_word_box: Some(true)` |
| `crates/desktop/src/clipboard/clipboard_commands.rs:462` | `OcrTextBlock` 加 `words` 字段 |
| `crates/desktop/frontend/src/pages/ImagePreview/useOcr.ts` | `OcrBlock` interface 加 `words` + 自动 OCR 触发 + config 开关 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 渲染 TextSelectLayer + SVG rect pointerEvents 改 none |
| `crates/infra/src/config.rs`（或 config 定义处） | 加 `image_preview_auto_ocr` 字段（默认 true） |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | i18n（如需新文案） |

---

## Task 1: 后端——OcrBlock 扩展 words + paddle-ocr 透出

**Files:**
- Modify: `crates/ocr/src/engine.rs:23-32`（OcrBlock + 新 OcrWord struct）
- Modify: `crates/ocr/src/engine.rs:307-328`（ocr_output_to_blocks 提取 word_boxes）
- Modify: `crates/ocr/src/paddle_backend.rs:68`（return_word_box: true）

**Interfaces:**
- Consumes: `octopus_paddle_ocr::OcrOutput.word_boxes`（`Option<Vec<Vec<WordBox>>>`，已存在）
- Produces: `OcrBlock.words: Option<Vec<OcrWord>>` + `OcrWord { text, x, y, w, h }`

- [x] **Step 1: 加 OcrWord struct + OcrBlock.words 字段**

修改 `crates/ocr/src/engine.rs`，在 `OcrBlock` 定义（行 25）前加 `OcrWord`，`OcrBlock` 加 `words` 字段：

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<OcrWord>>,
}
```

- [x] **Step 2: 抽取 quad_to_aabb helper**

`ocr_output_to_blocks`（行 307-328）里的 AABB 计算逻辑抽成 helper（word_boxes 也需要复用）。在 `ocr_output_to_blocks` 前加：

```rust
/// quad（4 点）→ 轴对齐包围盒 (x, y, w, h)。
fn quad_to_aabb(quad: &[[f32; 2]]) -> (f64, f64, f64, f64) {
    let xs: Vec<f32> = quad.iter().map(|p| p[0]).collect();
    let ys: Vec<f32> = quad.iter().map(|p| p[1]).collect();
    let x0 = xs.iter().copied().fold(f32::INFINITY, f32::min) as f64;
    let y0 = ys.iter().copied().fold(f32::INFINITY, f32::min) as f64;
    let x1 = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let y1 = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    (x0, y0, x1 - x0, y1 - y0)
}
```

- [x] **Step 3: ocr_output_to_blocks 提取 word_boxes**

修改 `ocr_output_to_blocks`（行 307）用 `quad_to_aabb` + 提取 word_boxes。注意 `WordBox` 的 `bbox` 字段是 `Quad` 类型（`[[f32; 2]; 4]`），需确认能否传给 `quad_to_aabb`——`Quad` 是 `[[f32; 2]; 4]`，`quad_to_aabb` 接受 `&[[f32; 2]]`，用 `&wb.bbox` 或 `wb.bbox.as_slice()` 视类型而定。先编译看报错调整。

```rust
fn ocr_output_to_blocks(output: &octopus_paddle_ocr::OcrOutput) -> Vec<OcrBlock> {
    let boxes = output.boxes.as_deref().unwrap_or(&[]);
    let txts = output.txts.as_deref().unwrap_or(&[]);
    let scores = output.scores.as_deref().unwrap_or(&[]);
    let word_boxes_all = output.word_boxes.as_deref().unwrap_or(&[]);

    boxes.iter().enumerate().map(|(i, quad)| {
        let (x, y, w, h) = quad_to_aabb(quad);
        let words = word_boxes_all.get(i).map(|wb_list| {
            wb_list.iter().map(|wb| {
                let (wx, wy, ww, wh) = quad_to_aabb(&wb.bbox);
                OcrWord {
                    text: wb.text.clone(),
                    x: wx, y: wy, w: ww, h: wh,
                }
            }).collect::<Vec<_>>()
        });
        OcrBlock {
            text: txts.get(i).cloned().unwrap_or_default(),
            x, y, w, h,
            score: scores.get(i).copied().unwrap_or(0.0) as f64,
            words,
        }
    }).collect()
}
```

- [x] **Step 4: merge_same_line_blocks 串联 words**

修改 `merge_same_line_blocks`（行 333-369），合并同行块时把 words 也串联。在现有 `last.text.push_str(&block.text)`（行 354）附近加：

```rust
// 串联 words（按 x 排序保持阅读顺序）
if let Some(block_words) = &block.words {
    if let Some(last_words) = &mut last.words {
        last_words.extend(block_words.iter().cloned());
        last_words.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        last.words = Some(block_words.clone());
    }
}
```

放在 `last.text.push_str(&block.text);`（行 354）之后、`let x1 = ...`（行 355）之前。

- [x] **Step 5: run_ocr 不需改——backend.recognize 已返回 OcrOutput（含 word_boxes）**

确认：`run_ocr`（行 213-257）调 `backend.recognize(img)` 返回 `OcrOutput`，`ocr_output_to_blocks(&result)` 提取 word_boxes。只要 backend 在 `recognize` 里设了 `return_word_box: true`，`OcrOutput.word_boxes` 就有值。

- [x] **Step 6: paddle_backend 启用 return_word_box**

修改 `crates/ocr/src/paddle_backend.rs:68`：

```rust
let opts = OcrCallOptions {
    return_word_box: Some(true),
    ..Default::default()
};
```

- [x] **Step 7: 修复所有 OcrBlock 构造点**

`OcrBlock` 加了 `words` 字段后，所有构造 `OcrBlock { ... }` 的地方都需加 `words: None`（或合适值）。grep 找出所有构造点：

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
rg "OcrBlock \{" crates/ --type rust
```

逐一加 `words: None`（除非该处有 word_boxes 数据）。

- [x] **Step 8: 编译验证**

```bash
cargo build -p octopus-ocr 2>&1 | tail -15
```

Expected: 0 errors。可能有 unused warning（OcrWord 暂未被消费），记录但继续。

- [x] **Step 9: 单测——ocr_output_to_blocks 提取 word_boxes**

在 `engine.rs` 的 `#[cfg(test)] mod tests` 里加测试：

```rust
#[test]
fn ocr_output_to_blocks_extracts_word_boxes() {
    use octopus_paddle_ocr::{OcrOutput, types::WordBox};
    let output = OcrOutput {
        boxes: Some(vec![[[0.0, 0.0], [100.0, 0.0], [100.0, 30.0], [0.0, 30.0]]]),
        txts: Some(vec!["Hello World".into()]),
        scores: Some(vec![0.95]),
        word_boxes: Some(vec![vec![
            WordBox { text: "Hello".into(), score: 0.95, bbox: [[0.0, 0.0], [50.0, 0.0], [50.0, 30.0], [0.0, 30.0]] },
            WordBox { text: "World".into(), score: 0.93, bbox: [[51.0, 0.0], [100.0, 0.0], [100.0, 30.0], [51.0, 30.0]] },
        ]]),
    };
    let blocks = ocr_output_to_blocks(&output);
    assert_eq!(blocks.len(), 1);
    let words = blocks[0].words.as_ref().expect("words should be Some");
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].text, "Hello");
    assert_eq!(words[1].text, "World");
    assert!((words[0].w - 50.0).abs() < 0.1);
}

#[test]
fn ocr_output_to_blocks_no_word_boxes() {
    use octopus_paddle_ocr::OcrOutput;
    let output = OcrOutput {
        boxes: Some(vec![[[0.0, 0.0], [100.0, 0.0], [100.0, 30.0], [0.0, 30.0]]]),
        txts: Some(vec!["Hello".into()]),
        scores: Some(vec![0.95]),
        word_boxes: None,
    };
    let blocks = ocr_output_to_blocks(&output);
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].words.is_none());
}
```

注意：`OcrOutput` 的字段名和类型需与 `paddle-ocr/src/pipeline/types.rs:10-15` 一致——如编译报错，先读 types.rs 确认字段名/类型。

- [x] **Step 10: 跑测试**

```bash
cargo test -p octopus-ocr --lib ocr_output_to_blocks 2>&1 | tail -15
```

Expected: 2 tests passed。

- [x] **Step 11: Commit**

```bash
git add crates/ocr/src/engine.rs crates/ocr/src/paddle_backend.rs
git commit -m "feat(ocr): OcrBlock 加 words 字段 + paddle-ocr return_word_box 透出

OcrWord { text, x, y, w, h } 词级坐标。ocr_output_to_blocks 从 OcrOutput.word_boxes
提取。merge_same_line_blocks 合并同行块时串联 words（按 x 排序）。
paddle_backend OcrCallOptions 设 return_word_box: true。"
```

---

## Task 2: 后端——DTO 同步 + config 开关

**Files:**
- Modify: `crates/desktop/src/clipboard/clipboard_commands.rs:462-473`（OcrTextBlock 加 words）
- Modify: `crates/infra/src/config.rs` 或 config 定义处（加 image_preview_auto_ocr）

**Interfaces:**
- Produces: `OcrTextBlock.words: Option<Vec<OcrTextWord>>` + config `image_preview_auto_ocr: bool`

- [x] **Step 1: OcrTextBlock 加 words 字段**

读 `clipboard_commands.rs:460-475`，看 `OcrTextBlock` 定义。加 words 字段（对应 OcrBlock.words）。

需新增一个 DTO struct `OcrTextWord`（或直接复用 ocr crate 的 `OcrWord`——取决于 desktop crate 是否已 re-export）。

最简做法：`OcrTextBlock` 直接引用 `octopus_ocr::OcrWord`：

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64, pub y: f64, pub w: f64, pub h: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<octopus_ocr::OcrWord>>,
}
```

然后在构造 `OcrTextBlock` 的地方（`clipboard_commands.rs` 里 OCR 结果转换处）从 `OcrBlock.words` 映射过来。grep `OcrTextBlock {` 找构造点。

- [x] **Step 2: config 加 image_preview_auto_ocr**

读 `crates/infra/src/config.rs`，找到现有 config 字段定义（如 `record_shortcut`、`action_bar_shortcut` 等），加：

```rust
#[serde(default = "default_true")]
pub image_preview_auto_ocr: bool,
```

加 `default_true` helper（如果不存在）：
```rust
fn default_true() -> bool { true }
```

- [x] **Step 3: 编译验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -15
```

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/clipboard/clipboard_commands.rs crates/infra/src/config.rs
git commit -m "feat(ocr): OcrTextBlock 加 words 字段 + config image_preview_auto_ocr"
```

---

## Task 3: 前端——TextSelectLayer.tsx + OcrBlock interface 更新

**Files:**
- Create: `crates/desktop/frontend/src/pages/ImagePreview/TextSelectLayer.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/useOcr.ts:10-17`（OcrBlock interface 加 words）

**Interfaces:**
- Consumes: `OcrBlock.words?: OcrWord[]`、`natW/natH/zoom/tool/imgLeft/imgTop`（从 index.tsx 传入）
- Produces: `<TextSelectLayer>` 组件

- [x] **Step 1: useOcr.ts OcrBlock interface 加 words**

修改 `useOcr.ts:10-17`：

```ts
export interface OcrWord {
  text: string;
  x: number; y: number; w: number; h: number;
}

export interface OcrBlock {
  text: string;
  x: number; y: number; w: number; h: number;
  score: number;
  words?: OcrWord[];
}
```

- [x] **Step 2: 创建 TextSelectLayer.tsx**

创建 `crates/desktop/frontend/src/pages/ImagePreview/TextSelectLayer.tsx`：

```tsx
// HTML 透明文字层——每个 word 一个 span，原生拖选（对标 macOS Live Text）。
//
// 设计要点：
//   - color: transparent（用户看到原图文字，选中 overlay 透明文字）
//   - user-select: text（浏览器原生选择引擎）
//   - 容器 transform: scale(zoom) + 自然像素坐标 → zoom 变化零重算
//   - pointerEvents 受 tool 控制：tool="none" 时接管（拖选），其他工具放行（标注）

import { memo } from "react";
import type { OcrBlock, OcrWord } from "./useOcr";

interface Props {
  blocks: OcrBlock[];
  natW: number;
  natH: number;
  zoom: number;
  tool: string;
  imgLeft: number;
  imgTop: number;
}

/** fallback：block 无 words 时用整行作为一个"word" */
function blockToWords(b: OcrBlock): OcrWord[] {
  return b.words ?? [{ text: b.text, x: b.x, y: b.y, w: b.w, h: b.h }];
}

function TextSelectLayerBase({ blocks, natW, natH, zoom, tool, imgLeft, imgTop }: Props) {
  if (blocks.length === 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: imgLeft,
        top: imgTop,
        width: natW,
        height: natH,
        transform: `scale(${zoom})`,
        transformOrigin: "0 0",
        pointerEvents: tool === "none" ? "auto" : "none",
        zIndex: 5,
      }}
    >
      {blocks.flatMap((b, bi) =>
        blockToWords(b).map((w, wi) => (
          <span
            key={`${bi}-${wi}`}
            style={{
              position: "absolute",
              left: w.x,
              top: w.y,
              fontSize: w.h * 0.85,
              lineHeight: `${w.h}px`,
              color: "transparent",
              userSelect: "text",
              cursor: "text",
              whiteSpace: "pre",
            }}
          >
            {w.text}
          </span>
        ))
      )}
    </div>
  );
}

export const TextSelectLayer = memo(TextSelectLayerBase);
```

- [x] **Step 3: 前端构建验证**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison/crates/desktop/frontend
npx tsc --noEmit 2>&1 | tail -10
```

- [x] **Step 4: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
git add crates/desktop/frontend/src/pages/ImagePreview/TextSelectLayer.tsx crates/desktop/frontend/src/pages/ImagePreview/useOcr.ts
git commit -m "feat(image-preview): TextSelectLayer 透明文字层 + OcrBlock interface 加 words"
```

---

## Task 4: 前端——自动 OCR + index.tsx 集成

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/useOcr.ts`（自动 OCR 触发）
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx:680-710`（渲染 TextSelectLayer + SVG rect pointerEvents 改 none）

**Interfaces:**
- Consumes: `config.image_preview_auto_ocr`（从后端 config 读，或前端 localStorage 默认 true）
- Produces: 图片打开自动 OCR + 文字层渲染

- [x] **Step 1: useOcr.ts 加自动 OCR**

读 `useOcr.ts` 完整文件（114 行），找到 `imageId` effect（mount 时拉缓存的逻辑，约行 40-55）。在拉缓存后加自动 OCR：

```tsx
// 自动 OCR（config 开关控制）——打开图片无感知 OCR，文字层立即可选
useEffect(() => {
  if (!imageId || ocrDoneRef.current) return;
  // 先拉截图缓存（现有逻辑不重复）
  invoke<OcrResult | null>("get_last_screenshot_ocr", { imageId }).then((cached) => {
    if (cached?.blocks?.length) {
      setOcrBlocks(cached.blocks);
      ocrDoneRef.current = true;
      return;
    }
    // 无缓存 → 自动 OCR
    invoke<OcrResult>("ocr_image", { id: imageId })
      .then((result) => {
        if (result?.blocks?.length) setOcrBlocks(result.blocks);
        ocrDoneRef.current = true;
      })
      .catch((e) => {
        const msg = String(e);
        if (msg.includes("还未完成")) {
          // OcrLockGuard 互斥 → 1s 后重试一次
          setTimeout(() => {
            if (!ocrDoneRef.current) {
              invoke<OcrResult>("ocr_image", { id: imageId })
                .then((r) => { if (r?.blocks?.length) setOcrBlocks(r.blocks); ocrDoneRef.current = true; })
                .catch(() => { ocrDoneRef.current = true; }); // 放弃，不影响看图
            }
          }, 1000);
        } else {
          ocrDoneRef.current = true; // 其他错误静默
        }
      });
  });
}, [imageId]);
```

**注意**：现有的手动 OCR 逻辑（`handleOcr` 函数 + OCR 按钮循环三态）保留不动。自动 OCR 只是额外触发——如果自动 OCR 已完成（ocrDoneRef=true），手动按钮的循环切 overlay/mask 态仍正常工作。

**config 开关**：如果前端需要读 config，用 `invoke("get_config")` 或已有的 config hook。但最简做法是**先不 gate**（默认自动 OCR），config 开关留后续——如果需要 gate，在 effect 开头加 `invoke<boolean>("get_config_value", { key: "image_preview_auto_ocr" })` 或类似。先做无 gate 版本（自动 OCR 默认开），后续可加。

- [x] **Step 2: index.tsx 集成 TextSelectLayer**

修改 `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`。

先加 import：
```tsx
import { TextSelectLayer } from "./TextSelectLayer";
```

在现有 OCR SVG overlay（行 710 `</svg>` 闭合后）加 TextSelectLayer：

```tsx
            {/* OCR 文字选择层（HTML，原生拖选） */}
            {ocrBlocks.length > 0 && (
              <TextSelectLayer
                blocks={ocrBlocks}
                natW={natW}
                natH={natH}
                zoom={zoom}
                tool={tool}
                imgLeft={imgLeft}
                imgTop={imgTop}
              />
            )}
```

然后改 SVG rect 的 pointerEvents（行 692）从 `'all'` 改为 `'none'`——选择交互交给 HTML 层，SVG 仅视觉高亮：

```tsx
// 行 692 原：style={{ cursor: 'pointer', pointerEvents: 'all' }}
// 改为：style={{ pointerEvents: 'none' }}
// 同时 onDoubleClick 保留（向后兼容双击复制习惯），但 cursor 去掉（不再可点）
```

**注意**：SVG text 的 `pointerEvents: 'none'`（行 705）已经是 none，不变。

- [x] **Step 3: 前端构建验证**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison/crates/desktop/frontend
npx tsc --noEmit 2>&1 | tail -10
npm run build 2>&1 | tail -5
```

- [x] **Step 4: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
git add crates/desktop/frontend/src/pages/ImagePreview/useOcr.ts crates/desktop/frontend/src/pages/ImagePreview/index.tsx
git commit -m "feat(image-preview): 自动 OCR + TextSelectLayer 集成

useOcr 打开图片自动 OCR（OcrLockGuard 互斥时 1s 重试）。
index.tsx 渲染 TextSelectLayer + SVG rect pointerEvents 改 none（选择交 HTML 层）。"
```

---

## Task 5: 联调 + 文档同步

**Files:**
- Verify: 全链路手动 e2e
- Modify: `docs/features/screenshot.md`（ImagePreview 章节加文字选择层）
- Modify: `docs/architecture.md`（OCR 章节 + ImagePreview 文字层）
- Modify: spec §10 实现注记 + plan 实施状态表

- [x] **Step 1: 全量构建**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
touch crates/desktop/src/main.rs
cargo build -p octopus-desktop 2>&1 | tail -10
```

- [ ] **Step 2: 手动 e2e**（用户后续手动跑——subagent 无法做 GUI 测试）

验证清单：
- [ ] 打开含文字的图片 → 自动 OCR → 鼠标移到文字上变 I-beam 光标
- [ ] 拖选多个 word → 高亮选中 → Ctrl+C → 粘贴验证
- [ ] tool 切到 rect → 画标注 → 文字层不拦截
- [ ] tool 切回 none → 拖选恢复
- [ ] 缩放（zoom in/out）→ 文字层跟随缩放坐标不错位
- [ ] 双击 word → 原生选词
- [ ] 长图滚动 → 文字层跟随（sticky canvas 对齐）
- [ ] CJK 文本（中文）→ 每个 CJK 字符是一个 word box → 可逐字选

- [x] **Step 3: 文档同步**

更新 `docs/features/screenshot.md` ImagePreview 章节 + `docs/architecture.md` OCR 章节，描述 word-level box + 透明文字层 + 自动 OCR。

更新 spec §10 实现注记 + plan 实施状态表。

- [x] **Step 4: Commit**

```bash
git add docs/features/screenshot.md docs/architecture.md docs/superpowers/specs/2026-08-13-image-text-selection-layer-design.md docs/superpowers/plans/2026-08-13-image-text-selection-layer.md
git commit -m "docs: 图片文字选择层文档同步"
```

---

## Self-Review 清单

1. **Spec 覆盖**：spec §2 数据流（Task 1+4）、§3 后端（Task 1+2）、§4 前端（Task 3+4）、§5 交互分治（Task 4）、§7 错误处理（Task 4 OcrLockGuard 重试）、§8 测试（Task 1 单测 + Task 5 e2e）
2. **Placeholder**：无 TBD/TODO，所有代码块完整
3. **类型一致**：`OcrWord`（Task 1 定义 ↔ Task 3 前端 interface）、`OcrBlock.words`（Task 1 ↔ Task 2 DTO ↔ Task 3 前端）、`TextSelectLayer` props（Task 3 定义 ↔ Task 4 消费）、`return_word_box`（Task 1 Step 6 paddle_backend ↔ OcrOutput.word_boxes ↔ ocr_output_to_blocks）

> **实施记录（2026-08-13~14 commits d52ce41d~d7f1bff4 执行完毕，2026-08-17 归档前注记）**：TextSelectLayer + words 数据 + OCR 三态（off→select↔mask，取消自动 OCR）全部落地。剩余未勾的 17 项为**用户手动 GUI e2e 清单**（模板重复两份，subagent 无法做 GUI 测试）——留给用户后续验证，不阻塞归档。文档同步：features/ocr.md §8.1 + architecture.md。
