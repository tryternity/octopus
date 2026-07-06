# Vendor paddle-ocr-rs 替换 ocr-rs 设计规格

**日期**：2026-07-06
**范围**：从 `/Users/wudarui/workspace/agent/paddle-ocr-rs` 按需拷贝源码到 `crates/paddle-ocr/`，替换现有 `ocr-rs`（MNN），消除 MNN C++ 编译链 + bindgen + libclang 依赖。

---

## 1. 动机

| 维度 | ocr-rs（当前） | vendored paddle-ocr |
|------|---------------|-------------------|
| 推理后端 | MNN（C++ cmake 编译 / 预编译下载） | ort ONNX Runtime（预编译，与 ASR 共用） |
| 模型格式 | `.mnn`（需自行转换） | `.onnx`（官方标准格式） |
| 原生编译依赖 | cmake + bindgen + libclang + MNN | **零**（全部纯 Rust + ort 预编译） |
| SIMD 崩溃风险 | MNN 对 SSE/AVX 强依赖 | 无 |

## 2. 拷贝策略：保留什么，删除什么

### 保留（核心 OCR 流水线）

| 模块 | 文件 | 作用 |
|------|------|------|
| `det/` | detector.rs, preprocess.rs, postprocess/ | 文本检测（DB 后处理） |
| `rec/` | recognizer.rs, preprocess.rs, decode.rs, bidi.rs, word_boxes.rs | 文本识别（CTC 解码 + 双向文本 + 词级框） |
| `cls/` | classifier.rs, preprocess.rs, postprocess.rs | 方向分类（0°/180°） |
| `pipeline/` | rapid_ocr.rs, config.rs, image_ops.rs, types.rs | 三阶段编排 + 图像操作 |
| `runtime/` | session.rs, provider.rs | ort session 管理 + EP 解析 |
| `vision/` | image_backend.rs, resize.rs, rotate_crop.rs, backend.rs(简化) | 纯 Rust 图像预处理 |
| `config.rs` | — | 配置类型（删 YAML 解析） |
| `error.rs` | — | 错误类型 |
| `types.rs` | — | 公开类型 |

### 删除（octopus 不需要）

| 删除项 | 原依赖 | 理由 |
|--------|--------|------|
| `src/bin/` | clap | CLI 工具，octopus 是库消费者 |
| `src/pipeline/compat_rapidocr/` | serde_yaml | RapidOCR YAML 兼容层 |
| `src/model_store.rs` | reqwest, sha2 | 模型下载——octopus 有 `crates/download` |
| `src/model_registry.rs` | serde_yaml | 内置模型清单（含 URL + SHA256） |
| `src/input/` | reqwest, turbojpeg, kamadak-exif | 图片加载——octopus 直接传内存字节 |
| `src/output/visualize.rs` | — | 框线可视化 |
| `src/output/markdown.rs` | — | Markdown 输出 |
| `src/output/json.rs` | — | JSON 输出 |

### 需修改的文件

| 文件 | 修改内容 |
|------|---------|
| `Cargo.toml` | 删 clap/opencv/reqwest/serde_yaml/sha2/turbojpeg/exif/clap；ort 版本 → `2.0.0-rc.12`；crate name → `octopus-paddle-ocr` |
| `src/lib.rs` | 精简导出（移除 input/output/compat/model_store 相关 re-export） |
| `src/config.rs` | 删 serde_yaml 解析；简化 `EngineConfig` 构造 |
| `src/pipeline/config.rs` | 删 serde_yaml 解析；保留结构体定义 |
| `src/vision/backend.rs` | 硬编码 `PureRust`（删 opencv 分支） |
| `src/error.rs` | 删 `Reqwest`/`Yaml` error variant |
| `src/pipeline/rapid_ocr.rs` | 入口接受 `RecImage` 而非 `OcrInput`（去掉 input loader 依赖） |

## 3. 最终依赖链

```toml
[dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["ndarray", "std"] }
image = { version = "0.25", default-features = true, features = ["png", "jpeg"] }
imageproc = "0.25"
ndarray = "0.16"
nalgebra = "0.33"
rayon = "1.10"
num_cpus = "1.16"
geo-types = "0.7"
geo-clipper = "0.9.0"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
unicode-bidi = "0.3"
```

全部纯 Rust + ort 预编译。零 cmake / bindgen / libclang / NASM。

## 4. crates/ocr 封装层改动

`crates/ocr/src/engine.rs` 对外接口（`recognize` / `recognize_with_blocks`）不变，内部替换：

```rust
// 旧
use ocr_rs::engine::OcrEngine as InnerEngine;
// 新
use octopus_paddle_ocr::RapidOcr as InnerEngine;
```

模型路径从 `.mnn` 三件套改为 `.onnx` 三件套（det.onnx + rec.onnx + keys.txt，可选 cls.onnx）。

## 5. 模型迁移

PP-OCRv5 ONNX 模型组：
- `det.onnx` — 文本检测（`ch_PP-OCRv5_mobile_det.onnx`）
- `rec.onnx` — 文本识别（`ch_PP-OCRv5_rec_mobile_infer.onnx`）
- `keys.txt` — 字符表（`ppocr_keys_v5.txt`）
- `cls.onnx` — 方向分类（`ch_ppocr_mobile_v2.0_cls_infer.onnx`，可选）

存放路径：`~/.octopus/models/ocr/PP-OCRv5/`

## 6. Workspace 集成

```toml
# Cargo.toml (workspace)
members = [..., "crates/paddle-ocr"]
```

`crates/ocr/Cargo.toml`：
```toml
# 旧
ocr-rs = "2.3"
# 新
octopus-paddle-ocr = { path = "../paddle-ocr" }
```
