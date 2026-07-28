# OCR Backend Trait 抽象设计

> **日期**：2026-07-27
> **状态**：✅ 已实现（2026-07-27；spec 状态 2026-07-28 补标）
> **来源**：[竞品分析报告](../../research/2026-07-27-competitive-analysis.md) §2 OCR P0 缺口

## 实现注记（2026-07-28 补标）

已实现，代码结构与 spec 设计一致：

| 组件 | 文件 | 说明 |
|---|---|---|
| `OcrBackend` trait | `crates/ocr/src/backend.rs` | 4 方法：recognize / provides_layout / use_word_segmentation / unload / name |
| `PaddleOcrBackend` | `crates/ocr/src/paddle_backend.rs` | PP-OCR 实现（持有 RapidOcr 实例） |
| `OcrEngine` | `crates/ocr/src/engine.rs` | `inner: Mutex<Option<Box<dyn OcrBackend>>>`（trait object） |
| `new_backend` 工厂 | `engine.rs:100` | 当前固定路由 PaddleOcrBackend；注释标注未来按 source_type 分流 VLM |

**未实现（未来扩展）**：VlmOcrBackend（云端 VLM OCR）——trait 已就绪，加新 backend 只需 impl OcrBackend + 在 new_backend 按 source_type 分流。

---

## 1. 问题

当前 `OcrEngine` 焊死 `RapidOcr`（`inner: Mutex<Option<RapidOcr>>`），无法切换 OCR 后端。这导致：
- 无法接入云端 VLM OCR（覆盖表格/公式长尾）
- 无法像 umi-OCR 那样插件式切换引擎（PaddleOCR / RapidOCR / Tesseract / VLM）
- 后续加任何新后端都要改 OcrEngine 内部

## 2. 方案

抽象 `OcrBackend` trait，`OcrEngine` 持有 `Box<dyn OcrBackend>`。现有 PP-OCRv6 逻辑搬入 `PaddleOcrBackend`。

```
OcrEngine（OnceLock 单例 + idle 60s + OcrLockGuard 不变）
  └─ inner: Mutex<Option<Box<dyn OcrBackend>>>  // 原来是 Mutex<Option<RapidOcr>>
      ├─ PaddleOcrBackend（现有逻辑，持有 RapidOcr）
      └─ VlmOcrBackend（后续加，本次不实现）
```

### 架构参考

- ASR `OfflineAsrEngine` trait（`crates/asr-local/src/engine.rs:15-26`）：sync + Send + 默认方法
- ASR CLI 分流（`cli/src/pipeline.rs`）：按 spec 字符串选本地/云端
- Translation `TranslationEngine` trait（`crates/translation/src/engine.rs`）：async，云端 HTTP

本次选 **sync trait**（与 ASR 一致），云端 VLM 内部用 `block_on` 或 `reqwest::blocking`。

## 3. 组件

### 3.1 `OcrBackend` trait（新增 `crates/ocr/src/backend.rs`）

```rust
use anyhow::Result;
use image::DynamicImage;
use octopus_paddle_ocr::{OcrOutput, OcrCallOptions};

/// OCR 后端抽象。本地（PP-OCRv6）和云端（VLM）统一接口。
pub trait OcrBackend: Send {
    /// 识别图片文字。
    /// 返回 paddle-ocr 格式的 OcrOutput（text + Quads + scores）。
    /// 云端 VLM 可只填 text（blocks 为空），由 OcrEngine 后处理决定是否补充布局。
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput>;

    /// 是否自带布局信息（VLM=true → 跳过 to_markdown 后处理；PP-OCR=false → 走全链）。
    fn provides_layout(&self) -> bool { false }

    /// 卸载模型释放内存（PP-OCR drop RapidOcr；VLM 空实现）。
    fn unload(&mut self);

    /// 模型名（诊断用，如 "PP-OCRv6-small" / "gpt-4o"）。
    fn name(&self) -> &str;
}
```

### 3.2 `PaddleOcrBackend`（现有逻辑搬入 `crates/ocr/src/paddle_backend.rs`）

把 `engine.rs` 里 RapidOcr 相关逻辑搬入：
- `build_engine_config`（从 DB 读 model_name → 拼路径 → EngineConfig）
- `RapidOcr::new(config)` + `run(image, options)`
- `use_word_segmentation` 判断（按 model_name 前缀）
- `unload`：`*self.inner = None`（drop RapidOcr）

### 3.3 `OcrEngine` 重构（`crates/ocr/src/engine.rs`）

改动点（公开 API 不变）：

| 原代码 | 改后 |
|---|---|
| `inner: Mutex<Option<RapidOcr>>` | `inner: Mutex<Option<Box<dyn OcrBackend>>>` |
| 构造 RapidOcr | 按 `get_active_model("ocr")` 的 source_type 选 backend（当前只 PaddleOcrBackend） |
| `recognize` 调 RapidOcr::run | 调 `inner.recognize(image)` |
| idle 释放 drop RapidOcr | 调 `inner.unload()` |
| 后处理链无条件走 | 按 `backend.provides_layout()` 决定：true 跳过 to_markdown，false 走全链 |

**不变**：
- 公开 API（`instance()` / `recognize()` / `recognize_with_blocks_from_image()`）签名不变
- OcrBlock / OcrOutput 类型不变
- 后处理链（merge_same_line_blocks / segment_english_words / to_markdown）不变
- probe / idle 60s / OcrLockGuard 不变
- 长图切分（recognize_long_image_with_blocks）在 OcrEngine 层，不进 backend

### 3.4 构造路由

`OcrEngine::new_backend()` 按 source_type 选 backend：
```rust
fn new_backend() -> Result<Box<dyn OcrBackend>> {
    let model = get_active_model("ocr")?;
    match model.source_type {
        0 | 1 => Ok(Box::new(PaddleOcrBackend::new(&model.name)?)),
        2 => Err(anyhow!("云端 VLM OCR 尚未实现")),  // 后续：VlmOcrBackend::new(&model)
        _ => Err(anyhow!("未知 source_type")),
    }
}
```

## 4. 不变量

| # | 不变量 | 保证 |
|---|---|---|
| INV-1 | desktop 调用零改动 | OcrEngine 公开 API 签名不变 |
| INV-2 | OcrBlock / OcrOutput 类型不变 | PaddleOcrBackend 仍返回 paddle-ocr 格式 |
| INV-3 | 后处理链不变（PP-OCR 场景） | PaddleOcrBackend `provides_layout()=false` → 走全链 |
| INV-4 | probe / idle / OcrLockGuard 不变 | 留在 OcrEngine 层 |
| INV-5 | 长图切分不变 | 留在 OcrEngine 层（调多次 backend.recognize） |

## 5. 测试

`backend.rs` 内联测试：
- PaddleOcrBackend 构造 + recognize 基本可用
- provides_layout() 返回 false
- unload() 后 inner 为 None

`engine.rs` 现有测试不变（OcrEngine 黑盒测试，内部 backend 切换不改变行为）。

## 6. 不做

- **VlmOcrBackend 实现**（云端 VLM OCR，留后续）
- **本地 VLM**（与轻量 CPU 定位冲突）
- **OcrEngine 公开 API 变更**
- **DB schema 变更**（source_type=2 已支持）
- **长图切分 / 后处理链重构**

## 7. 文件清单

| 文件 | 操作 | 职责 |
|---|---|---|
| `crates/ocr/src/backend.rs`（新建） | 新建 | OcrBackend trait 定义 |
| `crates/ocr/src/paddle_backend.rs`（新建） | 新建 | PaddleOcrBackend（现有 RapidOcr 逻辑搬入） |
| `crates/ocr/src/engine.rs` | 修改 | inner 改 Box<dyn OcrBackend> + 构造路由 + provides_layout 判断 |
| `crates/ocr/src/lib.rs` | 修改 | pub mod backend + pub mod paddle_backend |
