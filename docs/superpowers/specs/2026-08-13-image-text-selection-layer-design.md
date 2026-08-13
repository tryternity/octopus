# 图片文字选择层（Image Text Selection Layer）

- 日期：2026-08-13
- 类型：功能增强（ImagePreview / OCR）
- 优先级：P2（体验增强，对标 macOS Live Text / PixPin）
- 依赖：现有 OCR blocks SVG overlay（`ImagePreview/index.tsx:680-710`）+ paddle-ocr word_boxes 基建（`paddle-ocr/src/rec/word_boxes.rs`）+ `OcrLockGuard` 互斥 + `ocr_image` 命令

## 1. 背景与动机

### 1.1 现状

ImagePreview 打开图片后，OCR 是**手动触发**（工具栏 OCR 按钮），OCR 结果以 SVG overlay 三态展示（off/overlay/mask）。交互仅支持**整块双击复制**（`onDoubleClick` → `handleOcrBlockCopy`），不支持拖选、不支持跨块选择、不支持字/词级精度。

### 1.2 竞品差距

macOS Live Text / PixPin / eSearch 支持「打开图片直接拖选文字」——用户用鼠标在图片文字上拖动，像选原生文本一样选中并复制，无需感知 OCR 的存在。

### 1.3 本 spec 范围

1. **自动 OCR**：打开图片后台自动跑 OCR（无感知），受 config 开关控制
2. **HTML 透明文字层**：渲染一层透明文字（`color: transparent` + `user-select: text`），用户看到原图文字、选中 overlay 文字
3. **word-level box 透出**：现有 blocks 是 line-level，透出词级坐标实现精细选择
4. **原生拖选**：浏览器原生文本选择（`user-select: text`），无需手写选择逻辑

**不在范围**（YAGNI）：
- 字符级像素精确映射（做不到，Live Text 靠 Vision framework 原生能力）
- 右键菜单（复制/翻译/搜索选中文字）——后续增强
- OCR 结果编辑修正——后续

## 2. 架构（数据流）

```
[打开图片 ImagePreview mount]
   ↓ useEffect(imageId)
[自动 OCR] invoke("ocr_image", { id, returnWordBox: true })   ← OcrLockGuard 互斥
   ↓ emit("ocr-screenshot://result") 或 ocr_image 返回 OcrResult
[OCR 完成，blocks 含 word-level boxes]
   ↓
[HTML 透明文字层渲染 TextSelectLayer.tsx]
   ├─ 容器 transform: scale(zoom) + 尺寸 natW×natH（自然像素坐标）
   ├─ 每个 word 一个 <span> 绝对定位（自然像素坐标）
   │   color: transparent  ← 看到原图文字
   │   user-select: text   ← 原生拖选
   │   cursor: text        ← I-beam 光标
   └─ pointerEvents: tool==="none" ? auto : none  ← 仅选择工具时接管
   ↓
[用户拖选] tool="none" + 鼠标在 word 上 → I-beam → 原生拖选
   ↓ Ctrl+C / 浮动复制提示
[复制选中文字]
```

### 2.1 三个关键架构决策

**决策 1：HTML 透明文字层，不用 SVG text**

SVG `<text>` 在 WKWebView 里文本选择不可靠（无法跨元素拖选、选择高亮不渲染）。改用 HTML `<span>` overlay——每个 word 一个 span，`user-select: text` 走浏览器原生选择引擎。文字 `color: transparent`（用户看到原图像素文字，但选中的是 overlay 透明文字）。

**决策 2：容器 `transform: scale(zoom)`，不用逐 word 算坐标**

文字层容器尺寸 = 自然像素（`natW × natH`），`transform: scale(zoom)` + `transform-origin: 0 0`。每个 word span 用自然像素坐标绝对定位。zoom 变化时只需更新容器的 `transform`，无需重算任何 word 坐标（GPU 加速）。这与现有 SVG overlay 的 `viewBox=natural + width=dispW` 策略等价。

**决策 3：word-level box 从 paddle-ocr 透出，不需新算**

paddle-ocr pipeline 已有完整 word_boxes 基建：
- `compute_word_boxes`（`paddle-ocr/src/rec/word_boxes.rs:17`）已实现词级坐标计算
- `OcrOutput.word_boxes: Option<Vec<Vec<WordBox>>>`（`paddle-ocr/src/pipeline/types.rs:15`）已有字段
- `OcrCallOptions.return_word_box`（`types.rs:294`）已有开关

ocr engine 层的 `run_ocr` 调 `backend.recognize` 时传 `return_word_box: true`，`ocr_output_to_blocks` 提取 word_boxes 填入 `OcrBlock.words`。零新算法。

## 3. 后端组件

### 3.1 OcrBlock 扩展 words 字段

`crates/ocr/src/engine.rs:25-32`：

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
    /// 词级 box（英文/数字按空格切，CJK 按字切）。None = 未启用 word_boxes。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<OcrWord>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}
```

### 3.2 ocr_output_to_blocks 提取 word_boxes

`crates/ocr/src/engine.rs:307-328` 的 `ocr_output_to_blocks` 扩展，从 `output.word_boxes` 提取：

```rust
fn ocr_output_to_blocks(output: &octopus_paddle_ocr::OcrOutput) -> Vec<OcrBlock> {
    // ... 现有 box/txts/scores 提取逻辑不变 ...
    let word_boxes_all = output.word_boxes.as_deref().unwrap_or(&[]);
    boxes.iter().enumerate().map(|(i, quad)| {
        // ... 现有 AABB 计算 ...
        let words = word_boxes_all.get(i).map(|wb_list| {
            wb_list.iter().map(|wb| {
                let (wx, wy, ww, wh) = quad_to_aabb(&wb.bbox);
                OcrWord { text: wb.text.clone(), x: wx, y: wy, w: ww, h: wh }
            }).collect::<Vec<_>>()
        });
        OcrBlock { text, x, y, w, h, score, words }
    }).collect()
}
```

`quad_to_aabb` 是现有内联 AABB 逻辑（行 313-318）抽取的 helper。

### 3.3 run_ocr 启用 return_word_box

`crates/ocr/src/engine.rs:213-257` 的 `run_ocr`，调 `backend.recognize` 时传 `return_word_box: true`。具体取决于 backend.recognize 的签名——如果 backend 是 `RapidOcr` 且 recognize 接受 `OcrCallOptions`，加 `return_word_box: Some(true)`。

**注意**：`merge_same_line_blocks`（行 333）合并同行块时需正确处理 words——合并后的块 words = 各子块 words 串联（坐标不变，按 x 排序）。

### 3.4 DTO 同步

`crates/desktop/src/clipboard/clipboard_commands.rs:462-473` 的 `OcrTextBlock` + `OcrResult` 加 `words` 字段（同 OcrBlock 结构）。前端 `useOcr.ts:10-17` 的 `OcrBlock` interface 加 `words?: OcrWord[]`。

### 3.5 ocr_image 命令不变

`ocr_image` 命令签名不变（仍返回 `OcrResult`），只是 `OcrResult.blocks` 内的 `OcrTextBlock` 多了 `words` 字段。序列化向后兼容（`skip_serializing_if = "Option::is_none"`）。

## 4. 前端组件

### 4.1 TextSelectLayer.tsx（新建）

`crates/desktop/frontend/src/pages/ImagePreview/TextSelectLayer.tsx`：

```tsx
interface Props {
  blocks: OcrBlock[];
  natW: number;
  natH: number;
  zoom: number;
  tool: string;  // "none" = 选择工具
  imgLeft: number;
  imgTop: number;
}

export default function TextSelectLayer({ blocks, natW, natH, zoom, tool, imgLeft, imgTop }: Props) {
  if (blocks.length === 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: imgLeft, top: imgTop,           // 与 wrapper 图片对齐
        width: natW, height: natH,             // 自然像素尺寸
        transform: `scale(${zoom})`,           // 缩放跟随
        transformOrigin: "0 0",
        pointerEvents: tool === "none" ? "auto" : "none",
        zIndex: 5,                             // 在图片之上、标注之下
      }}
    >
      {blocks.flatMap((b, bi) =>
        (b.words ?? [{ text: b.text, x: b.x, y: b.y, w: b.w, h: b.h }]).map((w, wi) => (
          <span
            key={`${bi}-${wi}`}
            style={{
              position: "absolute",
              left: w.x, top: w.y,
              width: w.w, height: w.h,
              fontSize: w.h * 0.85,
              lineHeight: `${w.h}px`,
              color: "transparent",
              userSelect: "text",
              cursor: "text",
              whiteSpace: "pre",
              overflow: "hidden",
            }}
          >
            {w.text}
          </span>
        ))
      )}
    </div>
  );
}
```

**fallback**：`b.words` 为空时（旧缓存或 return_word_box 未启用），用 line-level block 整行作为一个 span。精度降为行级，但仍可选中复制。

### 4.2 自动 OCR 触发

`useOcr.ts` 的 `imageId` effect（现有 mount 拉缓存逻辑旁）加自动 OCR：

```tsx
useEffect(() => {
  if (!imageId) return;
  ocrDoneRef.current = false;
  // 1. 先拉缓存（现有逻辑：截图 OCR 的 LAST_SCREENSHOT_OCR）
  invoke("get_last_screenshot_ocr", { imageId }).then((cached) => {
    if (cached) { setOcrBlocks(cached.blocks); ocrDoneRef.current = true; }
  });
  // 2. 无缓存 → 自动 OCR（受 config 开关控制）
  if (!ocrDoneRef.current) {
    invoke("ocr_image", { id: imageId })
      .then((result) => { if (result) setOcrBlocks(result.blocks); })
      .catch((e) => { /* 静默——OcrLockGuard 互斥/引擎错误不影响看图 */ });
  }
}, [imageId]);
```

**config 开关**：`image_preview_auto_ocr`（默认 true）。config 为 false 时不自动 OCR，保持现有手动按钮行为。

**OcrLockGuard 互斥处理**：`ocr_image` 返回错误含"还未完成"时，1s 后重试一次（最多 3 次）。快速翻多张图片时不阻塞看图。

### 4.3 index.tsx 集成

`ImagePreview/index.tsx` 的渲染区分层调整：
- **SVG overlay**（现有，行 680-710）：视觉高亮层，`pointerEvents: "none"`（不再拦截事件，之前 rect 的 `pointerEvents: 'all'` 改为 `none`，选择交互交给 HTML 层）
- **TextSelectLayer**（新增）：选择交互层，`pointerEvents` 受 tool 控制
- **AnnotationSvg**（现有）：标注层，不变

```tsx
{/* OCR 视觉高亮层（SVG，纯展示不再拦截事件） */}
{ocrOverlay !== 'off' && ocrBlocks.length > 0 && <OcrHighlightSvg ... />}

{/* OCR 文字选择层（HTML，原生拖选） */}
{ocrBlocks.length > 0 && <TextSelectLayer blocks={ocrBlocks} natW={natW} natH={natH}
  zoom={zoom} tool={tool} imgLeft={imgLeft} imgTop={imgTop} />}
```

### 4.4 复制交互

浏览器原生 `user-select: text` 选中后，用户可 Ctrl+C 复制。额外加一个**浮动复制按钮**：选中文字（`selectionchange` 事件检测 window.getSelection() 非空）时，在选区附近显示「复制」按钮，点击复制选中文字。

**简化方案**（推荐先做）：不检测 selectionchange，仅依赖 Ctrl+C + 现有双击复制（保留 SVG rect 的 onDoubleClick）。浮动按钮留后续增强。

## 5. 交互分治

| 场景 | 事件归谁 | 行为 |
|---|---|---|
| `tool="none"` + 鼠标在 word span 上 | TextSelectLayer | I-beam 光标 → 原生拖选 |
| `tool="none"` + 鼠标在空白区 | wrapper（抓手平移） | 拖拽滚动（文字层 pointerEvents auto 但空白区无 span，事件穿透到 wrapper） |
| `tool="rect"/"pen"/"text"/...` | AnnotationSvg | TextSelectLayer pointerEvents: none 放行 |
| 双击 word | TextSelectLayer | 原生双击选词（浏览器默认） |

**冲突解决**：文字层的 span 只覆盖有文字的区域，空白区无 span → 事件穿透到 wrapper 的 onMouseDown（抓手平移 / 标注 hitTest）。这是 CSS pointerEvents 的天然行为——子元素 `pointer-events: auto` 只在元素自身区域拦截，透明区域不拦截。

**注意**：wrapper 的 onMouseDown（`index.tsx:482-515`）不需改——文字层 span 拦截 mousedown 后浏览器原生选择接管，不会冒泡到 wrapper。

## 6. 不变量

1. **现有 OCR 三态展示（off/overlay/mask）不变**——SVG 高亮层保留，只是 pointerEvents 从 rect 拦截改为 none
2. **现有标注工具不受影响**——tool 非 "none" 时文字层 pointerEvents: none
3. **现有双击复制保留**——SVG rect 的 onDoubleClick 不删（向后兼容旧交互习惯）
4. **ocr_image 命令签名不变**——返回值 OcrResult.blocks 内的 block 多了 words 字段，向后兼容
5. **config 开关默认 true 但可关**——`image_preview_auto_ocr: false` 时回退到手动 OCR 行为

## 7. 错误处理 / 降级

| 场景 | 处理 |
|---|---|
| OCR 失败（引擎错误） | 静默降级——无文字层，图片正常显示（不影响看图） |
| OcrLockGuard 互斥 | 1s 后重试，最多 3 次；超限放弃 |
| 无 word-level box（旧缓存 / return_word_box 失败） | fallback 用 line-level block 整行作为一个 span（行级选择） |
| 大图 OCR 延迟（1-3s） | 用户可先看图；OCR 完成后文字层淡入（CSS transition opacity） |
| 文字层与原图对齐偏差 | 以 word 为单位高亮，容差合理（PixPin/eSearch 同样限制） |
| config `image_preview_auto_ocr: false` | 不自动 OCR，保持手动按钮行为 |

## 8. 测试

### 8.1 后端单测

- `ocr_output_to_blocks` 提取 word_boxes：mock OcrOutput 含 word_boxes → 验证 OcrBlock.words 正确填充
- `merge_same_line_blocks` 合并时 words 串联：两个同行块各有 words → 合并后 words 按 x 排序串联
- word_boxes 为 None 时 OcrBlock.words = None（向后兼容）

### 8.2 前端

- TextSelectLayer 渲染：blocks 含 words → 每个 word 一个 span；words 为空 → fallback 整行 span
- pointerEvents 受 tool 控制：tool="none" → auto；其他 → none
- transform scale 跟随 zoom

### 8.3 手动 e2e

1. 打开含文字的图片 → 自动 OCR → 文字层出现（透明，鼠标移上去变 I-beam）
2. 拖选多个 word → 高亮选中 → Ctrl+C → 粘贴验证
3. tool 切到 rect → 画标注 → 文字层不拦截（pointerEvents: none）
4. tool 切回 none → 拖选恢复
5. 缩放（zoom in/out）→ 文字层跟随缩放，坐标不错位
6. 双击 word → 原生选词
7. config 关闭自动 OCR → 打开图片不自动 OCR，手动点 OCR 按钮后才出现

## 9. 文档同步

实现完成后同步：
- `docs/features/screenshot.md`：ImagePreview 章节加「文字选择层」
- `docs/architecture.md`：OCR 章节加 word-level box 透出 + ImagePreview 文字层
- 本 spec 对应的 plan

## 10. 实现注记

> 实现过程中发现的偏差、新增决策写在这里。
