# OCR Backend Trait 抽象实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 OcrEngine 从焊死 RapidOcr 重构为持有 Box<dyn OcrBackend>，现有 PP-OCRv6 逻辑搬入 PaddleOcrBackend，desktop 调用零改动。

**Architecture:** 新建 OcrBackend trait + PaddleOcrBackend（搬现有逻辑）+ OcrEngine inner 改 Box<dyn OcrBackend> + 构造路由按 source_type 选 backend。

**Tech Stack:** Rust + octopus-paddle-ocr（现有依赖）

**Spec:** `docs/superpowers/specs/2026-07-27-ocr-backend-trait-design.md`

## Global Constraints

- OcrEngine 公开 API 签名不变（desktop 调用零改动）
- OcrBlock / OcrOutput 类型不变
- 后处理链（merge/segment/to_markdown）不变
- probe / idle 60s / OcrLockGuard 不变
- 长图切分留在 OcrEngine 层
- 云端 VLM backend 不做（预留 source_type=2 路由，返回 Err）

---

## File Structure

| 文件 | 操作 | 职责 |
|---|---|---|
| `crates/ocr/src/backend.rs` | 新建 | OcrBackend trait 定义 |
| `crates/ocr/src/paddle_backend.rs` | 新建 | PaddleOcrBackend（现有 RapidOcr 逻辑搬入） |
| `crates/ocr/src/engine.rs` | 修改 | inner 改 Box<dyn OcrBackend> + 构造路由 + provides_layout 判断 |
| `crates/ocr/src/lib.rs` | 修改 | pub mod backend + pub mod paddle_backend |

---

### Task 1: OcrBackend trait 定义

**Files:**
- Create: `crates/ocr/src/backend.rs`
- Modify: `crates/ocr/src/lib.rs`

**Interfaces:**
- Produces: `OcrBackend` trait（Send + recognize + provides_layout + unload + name）

- [ ] **Step 1: 创建 backend.rs**

Read `crates/ocr/src/engine.rs` 了解 OcrOutput 类型和现有 recognize 返回值。然后创建 `crates/ocr/src/backend.rs`：

```rust
//! OCR 后端抽象。本地（PP-OCRv6）和云端（VLM，后续）统一接口。
//!
//! 详见 spec 2026-07-27-ocr-backend-trait-design.md。

use anyhow::Result;
use image::DynamicImage;
use octopus_paddle_ocr::OcrOutput;

/// OCR 后端 trait。
///
/// - `recognize` 返回 paddle-ocr 格式的 OcrOutput（text + Quads + scores）
/// - `provides_layout` VLM=true 跳过 to_markdown 后处理；PP-OCR=false 走全链
/// - `unload` 释放模型内存（PP-OCR drop RapidOcr；VLM 空实现）
pub trait OcrBackend: Send {
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput>;
    fn provides_layout(&self) -> bool { false }
    fn unload(&mut self);
    fn name(&self) -> &str;
}
```

- [ ] **Step 2: 在 lib.rs 注册模块**

Read `crates/ocr/src/lib.rs`，加 `pub mod backend;`。

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-ocr`
Expected: 0 error

- [ ] **Step 4: Commit**

```bash
git add crates/ocr/src/backend.rs crates/ocr/src/lib.rs
git commit -m "feat(ocr): OcrBackend trait 定义（Send + recognize + provides_layout + unload）"
```

---

### Task 2: PaddleOcrBackend（搬现有 RapidOcr 逻辑）

**Files:**
- Create: `crates/ocr/src/paddle_backend.rs`
- Modify: `crates/ocr/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 的 OcrBackend trait
- Produces: `PaddleOcrBackend` impl OcrBackend

- [ ] **Step 1: 读 engine.rs 找到 RapidOcr 相关逻辑**

Read `crates/ocr/src/engine.rs` 完整文件。找到：
- 构造 RapidOcr 的代码（build_engine_config / EngineConfig 构造）
- recognize 调用 RapidOcr::run 的代码
- use_word_segmentation 判断
- unload / drop RapidOcr 的代码
- model_name 获取

- [ ] **Step 2: 创建 paddle_backend.rs**

把上述逻辑搬入 `crates/ocr/src/paddle_backend.rs`，实现 OcrBackend trait：

```rust
//! PP-OCRv6 本地 OCR 后端（现有 RapidOcr 逻辑，从 engine.rs 搬入）。

use anyhow::Result;
use image::DynamicImage;
use octopus_paddle_ocr::{OcrOutput, OcrCallOptions, RapidOcr, EngineConfig};
use super::backend::OcrBackend;

pub struct PaddleOcrBackend {
    inner: Option<RapidOcr>,
    model_name: String,
}

impl PaddleOcrBackend {
    /// 从 DB 激活的 ocr 模型名构造。
    pub fn new(model_name: &str) -> Result<Self> {
        // 搬 engine.rs 里的 build_engine_config 逻辑
        // ...
        let ocr = RapidOcr::new(config)?;
        Ok(Self { inner: Some(ocr), model_name: model_name.to_string() })
    }
}

impl OcrBackend for PaddleOcrBackend {
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput> {
        // 搬 engine.rs 里的 RapidOcr::run 调用 + use_word_segmentation 判断
        // ...
    }

    fn provides_layout(&self) -> bool { false }

    fn unload(&mut self) {
        self.inner = None;
    }

    fn name(&self) -> &str { &self.model_name }
}
```

IMPORTANT: 搬代码时保持逻辑完全一致（不重构不优化）。use_word_segmentation 的逻辑也要搬入（按 model_name 前缀判断）。

- [ ] **Step 3: 在 lib.rs 注册模块**

加 `pub mod paddle_backend;`。

- [ ] **Step 4: 编译验证**

Run: `cargo build -p octopus-ocr`
Expected: 0 error（可能 warning unused——engine.rs 还在用 RapidOcr，后续 Task 3 删）

- [ ] **Step 5: Commit**

```bash
git add crates/ocr/src/paddle_backend.rs crates/ocr/src/lib.rs
git commit -m "feat(ocr): PaddleOcrBackend（现有 RapidOcr 逻辑搬入，impl OcrBackend）"
```

---

### Task 3: OcrEngine 重构（inner 改 Box<dyn OcrBackend>）

**Files:**
- Modify: `crates/ocr/src/engine.rs`

**Interfaces:**
- Consumes: Task 1 的 OcrBackend + Task 2 的 PaddleOcrBackend

- [ ] **Step 1: 改 inner 类型**

Read `crates/ocr/src/engine.rs`。把 `inner: Mutex<Option<RapidOcr>>` 改为 `inner: Mutex<Option<Box<dyn OcrBackend>>>`。

- [ ] **Step 2: 改构造逻辑**

原来直接构造 RapidOcr 的地方，改为调 `PaddleOcrBackend::new(model_name)`：

```rust
fn new_backend() -> Result<Box<dyn OcrBackend>> {
    let model_name = crate::model::get_active_ocr_model_name()?;
    Ok(Box::new(PaddleOcrBackend::new(&model_name)?))
}
```

如果有 source_type 路由，加 match（source_type=2 返回 Err 占位）。

- [ ] **Step 3: 改 recognize 调用**

原来调 `RapidOcr::run` 的地方，改为调 `inner.recognize(image)`。

- [ ] **Step 4: 改 idle 释放**

原来 drop RapidOcr 的地方，改为调 `inner.unload()`。

- [ ] **Step 5: 删除 engine.rs 里搬入 paddle_backend.rs 的旧代码**

build_engine_config / use_word_segmentation 判断 / RapidOcr 构造等——已在 PaddleOcrBackend 里，engine.rs 不再需要。

- [ ] **Step 6: 删除 engine.rs 对 octopus-paddle-ocr 的直接依赖引用**

engine.rs 不再直接用 RapidOcr / EngineConfig 等（都经 PaddleOcrBackend）。但 Cargo.toml 依赖保留（PaddleOcrBackend 用）。

- [ ] **Step 7: 编译验证**

Run: `cargo build -p octopus-ocr`
Expected: 0 error 0 warning

- [ ] **Step 8: ocr crate 测试**

Run: `cargo test -p octopus-ocr`
Expected: 全过（现有测试黑盒，内部 backend 切换不改变行为）

- [ ] **Step 9: Commit**

```bash
git add crates/ocr/src/engine.rs
git commit -m "refactor(ocr): OcrEngine inner 改 Box<dyn OcrBackend> + 构造路由"
```

---

### Task 4: desktop 集成验证 + 文档

- [ ] **Step 1: desktop 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 0 error（OcrEngine 公开 API 不变，desktop 调用零改动）

- [ ] **Step 2: desktop 测试**

Run: `cargo test -p octopus-desktop`
Expected: 全过

- [ ] **Step 3: 更新 architecture.md**

OCR 引擎描述处补 backend trait 架构：

> OcrEngine 持有 Box<dyn OcrBackend>（2026-07-27 重构），按 source_type 选后端（0/1=PaddleOcrBackend 现有 PP-OCRv6，2=VlmOcrBackend 后续）。desktop 调用零改动。详见 spec。

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture.md 补 OCR backend trait 架构"
```
