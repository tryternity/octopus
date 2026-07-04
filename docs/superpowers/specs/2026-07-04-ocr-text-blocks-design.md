# OCR 文本块可视化设计

> 日期：2026-07-04
> 状态：✅ 已实现（图片预览三态 toggle + 截图关窗→预览展示）
> 前置：`2026-07-03-image-viewer-perf-design.md`（图片预览视口渲染 + SVG overlay）
> 分析参考：`2026-07-03-snow-shot-analysis.md`（snow-shot OCR 功能对比）
> 分支：`image-viewer-perf`

## 1. 背景与目标

当前 OCR（`ocr_image`）只返回纯文本字符串——后端 `ocr_rs` 引擎实际返回了 `OcrResult_ { text, confidence, bbox: TextBox { rect, score, points } }`，但 `engine.rs:110` 丢弃了坐标只取 text。

用户识别后看不到"图上哪些区域被识别出来了"，无法验证识别准确度，也无法对照图上内容做标注。

**目标**：OCR 后在图片预览窗叠加文本块可视化层（边界框 + 文字），作为独立 toggle 层与标注工具正交。

## 2. 范围

**做：**
- 后端 `ocr_image` 返回值从 `String` 改为结构化 `{ text, blocks }`
- 前端图片预览窗加 OCR 文本块叠加层（SVG，与标注 overlay 并列）
- OCR 按钮双态：首次点击 = 触发识别（耗时）+ 显示叠加层；后续点击 = toggle 显示/隐藏
- 换图重置 OCR 叠加状态
- 超长图分块识别的坐标合并（offset 累加）

**不做（YAGNI / 留后续）：**
- 逐块可复制 / 可编辑（A 方案纯展示）
- 截图窗口的 OCR 文本块叠加（图片预览优先）
- 方向检测 / 旋转文字识别

## 3. 架构

### 3.1 后端：`ocr_image` 返回结构化结果

**新返回类型**（`clipboard_commands.rs`）：

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64,      // 文本块左上角 x（自然像素）
    pub y: f64,      // 文本块左上角 y（自然像素）
    pub w: f64,      // 宽
    pub h: f64,      // 高
    pub score: f64,  // 置信度
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub text: String,           // 完整识别文本（\n 连接，与原行为一致）
    pub blocks: Vec<OcrTextBlock>, // 带坐标的文本块
}
```

**`ocr_image` 命令改为返回 `OcrResult`**（替代 `String`）。

**`recognize` 改为返回 blocks**（`engine.rs`）：

```rust
pub struct OcrBlock {
    pub text: String,
    pub x: f64, pub y: f64, pub w: f64, pub h: f64,
    pub score: f64,
}

pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)>
```

`recognize_image` 不再丢弃 `bbox`：

```rust
fn recognize_image_with_blocks(&self, img: &DynamicImage) -> Result<Vec<OcrBlock>> {
    let results = self.inner.recognize(img)?;
    Ok(results.into_iter().map(|r| OcrBlock {
        text: r.text,
        x: r.bbox.rect.x as f64,
        y: r.bbox.rect.y as f64,
        w: r.bbox.rect.width as f64,
        h: r.bbox.rect.height as f64,
        score: r.confidence as f64,
    }).collect())
}
```

**超长图分块识别坐标合并**（`recognize_long_image`）：

每个 chunk 的 top offset 加到 y 坐标上：

```rust
fn recognize_long_image_with_blocks(&self, img: &DynamicImage) -> Result<Vec<OcrBlock>> {
    let (w, h) = (img.width(), img.height());
    let mut all_blocks = Vec::new();
    let mut prev_last_text: Option<String> = None;
    for &(top, chunk_h) in Self::plan_chunks(h).iter() {
        let sub = crop_imm(img, 0, top, w, chunk_h);
        let chunk = DynamicImage::from(sub.to_image());
        let mut blocks = self.recognize_image_with_blocks(&chunk)?;
        // 去重（同 recognize_long_image 逻辑）
        // offset y 加 top
        for b in &mut blocks { b.y += top as f64; }
        all_blocks.extend(blocks);
    }
    Ok(all_blocks)
}
```

### 3.2 前端：OCR 文本块叠加层

**数据流（图片预览 OCR）**：

```
点 OCR 按钮 → invoke("ocr_image", {id})
  → 返回 { text, blocks: [{text,x,y,w,h,score}] }
  → blocks 存入 ocrBlocks state
  → ocrOverlay state = 'overlay'（三态：off → overlay → mask → off）
  → SVG 渲染文本块（独立 overlay layer）
  → text 入库 + openCompactEditorTab（保持现有行为）
```

**数据流（截图 OCR）**：

```
截图点 OCR → invoke("ocr_screenshot", bytes)
  → 后端：识别 + 入库 + 关截图窗 + 开编辑器 + 开图片预览
  → emit("ocr-screenshot://result", { text, blocks })
  → 图片预览 listen → setOcrBlocks + setOcrOverlay('overlay')
  → 截图窗已关，叠加层显示在图片预览中
```

**SVG 渲染**（与标注 overlay 并列，在标注层下面）：

```tsx
{/* OCR 文本块叠加层（独立 toggle，与标注 tool 正交） */}
{ocrOverlay && ocrBlocks.length > 0 && (
  <svg className="absolute inset-0 block"
    viewBox={`0 0 ${natW} ${natH}`}
    preserveAspectRatio="none"
    style={{ width: dispW, height: dispH, pointerEvents: "none", zIndex: 1 }}
  >
    {ocrBlocks.map((b, i) => (
      <g key={i}>
        <rect x={b.x} y={b.y} width={b.w} height={b.h}
          fill="rgba(59,130,246,0.08)"
          stroke="rgba(59,130,246,0.4)" strokeWidth={1} rx={2} />
        <text x={b.x + 2} y={b.y + b.h - 2}
          fontSize={Math.min(b.h * 0.8, 14)}
          fill="rgba(59,130,246,0.7)"
          dominantBaseline="alphabetic">
          {b.text}
        </text>
      </g>
    ))}
  </svg>
)}
```

**坐标空间**：文本块坐标在自然像素空间（和标注一样），SVG viewBox = `0 0 natW natH`，自动随 zoom 缩放。

### 3.3 OCR 按钮三态 toggle

```ts
const [ocrBlocks, setOcrBlocks] = useState<OcrBlock[]>([]);
const [ocrOverlay, setOcrOverlay] = useState<'off' | 'overlay' | 'mask'>('off');

const handleOcr = async () => {
  // 已有结果 → 三态循环：off → overlay → mask → off
  if (ocrBlocks.length > 0) {
    setOcrOverlay(ocrOverlay === 'off' ? 'overlay' : ocrOverlay === 'overlay' ? 'mask' : 'off');
    return;
  }
  // 首次识别
  const result = await invoke<{text:string; blocks:OcrBlock[]}>("ocr_image", { id: imageId });
  if (result.text) {
    setOcrBlocks(result.blocks);
    setOcrOverlay('overlay');
    const ocrId = await invoke<number>("insert_ocr_clipboard_item", { text: result.text });
    await openCompactEditorTab(ocrId);
    setOcrCopied(true);
    setTimeout(() => setOcrCopied(false), 1500);
  }
};
```

**三态效果**：
| 状态 | 效果 | 图标 |
|------|------|------|
| overlay | 半透蓝边框 + 蓝字浮在原图上 | ocr-all.svg |
| mask | 白底覆盖原文 + 黑字（密集文本不干扰） | ocr-text.svg |
| off | 无叠加 | ocr-ai.svg |

**双击复制**：双击任意文本块 rect → `clipboard.writeText(b.text)` + 绿色浮泡提示 2 秒消失。两遍渲染（先所有 rect 后所有 text）防遮挡。

**OCR 按钮 active 状态**：`ocrCopied || ocrWarn || ocrMode !== 'off'`。

### 3.4 截图 OCR → 图片预览展示

截图 OCR 不在截图全屏窗叠加（信息过载 + 全屏窗遮挡），改为：
1. 后端 `ocr_screenshot` 识别 + 入库 + **关截图窗** + 开编辑器 + 开图片预览
2. `emit("ocr-screenshot://result", { text, blocks })` 推送结果
3. 图片预览 `listen` 收到 → `setOcrBlocks + setOcrOverlay('overlay')`
4. 截图窗已关，叠加层在图片预览中展示（三态 toggle / 双击复制 / 标注 / 缩放）

imageId useEffect 中清空：

```ts
setOcrBlocks([]);
setOcrOverlay(false);
```

## 4. 样式设计

- 文本块矩形：`fill: rgba(59,130,246,0.08)`（极淡蓝底）, `stroke: rgba(59,130,246,0.4)`（半透蓝框）, `rx: 2`
- 文字：`fontSize: min(block.h * 0.8, 14)`（自适应块高度，上限 14px），`fill: rgba(59,130,246,0.7)`（半透蓝字）
- 不遮挡原图内容（极淡叠加），滚动/缩放时随 SVG overlay 一起移动

## 5. 边界情况

- **OCR 无结果**（空文本）：不显示叠加层，`ocrBlocks = []`
- **超长图分块识别**：后端合并坐标（每块 y += chunk top offset），前端不感知分块
- **OCR 与标注同时显示**：OCR 叠加层 zIndex < 标注层 zIndex，标注在上方
- **toggle off 后再 on**：使用缓存的 `ocrBlocks`（不重新识别）
- **换图**：清空 `ocrBlocks` + `ocrOverlay`，OCR 按钮回到非激活

## 6. 不变量

1. OCR 文本块坐标在自然像素空间（与标注坐标系统一致）
2. OCR 叠加层与 tool 状态正交（可同时显示标注 + OCR 文本块）
3. 首次 OCR = 触发识别 + 入库 + 编辑器（保持现有行为）+ 显示叠加层
4. 后续 OCR 按钮点击 = toggle 叠加层（不重新识别、不入库）
