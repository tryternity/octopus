# OCR 文本块可视化实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 或 superpowers:executing-plans 逐任务实施。

**Goal:** OCR 后在图片预览窗叠加文本块可视化层（边界框 + 文字），独立 toggle，与标注正交。

**Architecture:** 后端 `ocr_image` 返回结构化 `{text, blocks}`；前端 SVG 叠加层渲染文本块；OCR 按钮双态（首次识别 + 后续 toggle）。

**Tech Stack:** Rust（ocr-rs → octopus-ocr → octopus-desktop）、React + SVG overlay。

---

## Task 1: 后端 engine — recognize 返回 blocks

**Files:**
- Modify: `crates/ocr/src/engine.rs`

- [ ] **Step 1：定义 OcrBlock 结构体 + recognize_with_blocks 方法**

在 `engine.rs` 加公开结构体和方法：

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
}

/// 识别图片字节，返回完整文本 + 带坐标的文本块。
pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)> {
    let img = ::image::load_from_memory(image_bytes)
        .context("Failed to decode image")?;
    let blocks = if img.height() > SPLIT_HEIGHT_THRESHOLD {
        self.recognize_long_image_with_blocks(&img)?
    } else {
        self.recognize_image_with_blocks(&img)?
    };
    let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
    Ok((text, blocks))
}
```

- [ ] **Step 2：recognize_image_with_blocks（不丢弃 bbox）**

```rust
fn recognize_image_with_blocks(&self, img: &::image::DynamicImage) -> Result<Vec<OcrBlock>> {
    let results = self
        .inner
        .recognize(img)
        .map_err(|e| anyhow::anyhow!("OCR recognize failed: {:?}", e))?;
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

- [ ] **Step 3：recognize_long_image_with_blocks（坐标 offset 合并）**

```rust
fn recognize_long_image_with_blocks(&self, img: &::image::DynamicImage) -> Result<Vec<OcrBlock>> {
    let (w, h) = (img.width(), img.height());
    let mut all_blocks = Vec::new();
    let mut prev_last_text: Option<String> = None;
    for (idx, &(top, chunk_h)) in Self::plan_chunks(h).iter().enumerate() {
        let sub = ::image::imageops::crop_imm(img, 0, top, w, chunk_h);
        let chunk = ::image::DynamicImage::from(sub.to_image());
        let mut blocks = self.recognize_image_with_blocks(&chunk)?;
        log::info!("[ocr-engine] chunk#{} top={} h={} → {} blocks", idx, top, chunk_h, blocks.len());
        // 去重：跳过与上一块末行 text 相同的起始 blocks
        if let Some(ref last_text) = prev_last_text {
            let skip = blocks.iter().position(|b| b.text != *last_text).unwrap_or(blocks.len());
            blocks.drain(..skip);
        }
        // offset y 加 top
        for b in &mut blocks { b.y += top as f64; }
        prev_last_text = blocks.last().map(|b| b.text.clone());
        all_blocks.extend(blocks);
    }
    Ok(all_blocks)
}
```

- [ ] **Step 4：编译验证**

Run: `cargo build -p octopus-ocr`

- [ ] **Step 5：提交**

```bash
git add crates/ocr/src/engine.rs
git commit -m "feat(ocr): recognize_with_blocks 返回带坐标的文本块"
```

---

## Task 2: 后端命令 — ocr_image 返回结构化结果

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`

- [ ] **Step 1：定义 OcrResult 结构体**

在 `clipboard_commands.rs` 加：

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64, pub y: f64, pub w: f64, pub h: f64,
    pub score: f64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub text: String,
    pub blocks: Vec<OcrTextBlock>,
}
```

- [ ] **Step 2：ocr_image 返回类型改为 OcrResult**

```rust
pub async fn ocr_image(id: i64) -> Result<OcrResult, String> {
    // ... 前置不变（锁、取 item、取 blob、engine）...
    let (text, blocks) = engine.recognize_with_blocks(&webp_blob)
        .map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }
    let blocks = blocks.into_iter().map(|b| OcrTextBlock {
        text: b.text, x: b.x, y: b.y, w: b.w, h: b.h, score: b.score,
    }).collect();
    Ok(OcrResult { text, blocks })
}
```

- [ ] **Step 3：编译验证**

Run: `cargo build -p octopus-desktop`

- [ ] **Step 4：提交**

```bash
git add crates/desktop/src/clipboard_commands.rs
git commit -m "feat(desktop): ocr_image 返回结构化 {text, blocks}"
```

---

## Task 3: 前端 — OCR 叠加层 + 按钮双态

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`

- [ ] **Step 1：index.tsx 加 ocrBlocks / ocrOverlay state**

```ts
interface OcrBlock { text: string; x: number; y: number; w: number; h: number; score: number; }
// state
const [ocrBlocks, setOcrBlocks] = useState<OcrBlock[]>([]);
const [ocrOverlay, setOcrOverlay] = useState(false);
```

- [ ] **Step 2：换图重置（imageId useEffect 中加）**

```ts
setOcrBlocks([]);
setOcrOverlay(false);
```

- [ ] **Step 3：handleOcr 改为双态**

```ts
const handleOcr = async () => {
  if (imageId == null) return;
  // 已有结果 → toggle
  if (ocrBlocks.length > 0) {
    setOcrOverlay(!ocrOverlay);
    return;
  }
  // 首次识别
  try {
    const result = await invoke<{text: string; blocks: OcrBlock[]}>("ocr_image", { id: imageId });
    if (result.text) {
      setOcrBlocks(result.blocks);
      setOcrOverlay(true);
      const ocrId = await invoke<number>("insert_ocr_clipboard_item", { text: result.text });
      await openCompactEditorTab(ocrId);
      setOcrCopied(true);
      setTimeout(() => setOcrCopied(false), 1500);
    }
  } catch (e) {
    const msg = String(e);
    if (msg.includes("还未完成")) {
      setOcrWarn(true);
      setTimeout(() => setOcrWarn(false), 1800);
    } else {
      console.error(e);
    }
  }
};
```

- [ ] **Step 4：SVG 叠加层（在标注 SVG 前面，zIndex 更低）**

在 wrapper 内 canvas 后、标注 SVG 前加：

```tsx
{ocrOverlay && ocrBlocks.length > 0 && (
  <svg className="absolute inset-0 block"
    viewBox={`0 0 ${natW} ${natH}`}
    preserveAspectRatio="none"
    style={{ width: dispW, height: dispH, pointerEvents: "none" }}>
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

- [ ] **Step 5：OCR 按钮 active 加 ocrOverlay**

index.tsx 传 prop：
```tsx
ocrCopied={ocrCopied || ocrOverlay}
```

- [ ] **Step 6：构建验证**

Run: `npm run build`

- [ ] **Step 7：提交**

```bash
git add crates/desktop/frontend/src/pages/ImagePreview/
git commit -m "feat(ImagePreview): OCR 文本块可视化叠加层 + 按钮双态 toggle"
```

---

## Task 4: 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1：更新 architecture.md 的 OCR 描述**

- [ ] **Step 2：提交**

```bash
git add docs/
git commit -m "docs: 同步 OCR 文本块可视化到 architecture.md"
```
