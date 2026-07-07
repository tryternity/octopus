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
| `src/vision/backend.rs` | 硬编码 `PureRust`（opencv 分支已完全删除） |
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

PP-OCRv6-small 也支持（`~/.octopus/models/ocr/PP-OCRv6-small/`，det.onnx + rec.onnx + keys.txt 18708 行 `ppocrv6_dict.txt` + cls.onnx 复用 v5）。

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

## 7. 实施中发现的关键 bug 与后处理（增补）

1. **`read_character_file` trim() 误删全角空格（关键 bug）**
   - Rust `str::trim()` 把 U+3000（全角空格，PP-OCR 字典首行）当 whitespace 去掉 → 字符表少一行 → CTC 索引全体偏移 1 位 → 输出文字每位偏移一位（`Hello`→`Ifmmp`）。
   - 修复：改用 `strip_suffix('\r')` 只去掉 CRLF 残留。

2. **同行文本框合并（`merge_same_line_blocks`）**
   - det 检测器常把同一视觉行拆成多个独立框（尤其中英混排）→ 输出多余换行。
   - 保持 det 原始输出顺序，相邻块 y 中心距离 < 平均行高一半时合并；水平间隙 > 行高 30% 时补空格。

3. **英文单词分词（`segment_english_words`，仅 PP-OCRv5 需要）**
   - PP-OCR 中文 rec 模型不输出英文单词间空格（CTC space token 未激活）。
   - 内嵌 17.7K 英文词库（`crates/ocr/assets/words_common.txt`，168KB，top 10K 高频词 + 技术后缀词），贪心最长匹配分词。
   - PP-OCRv6 的 CTC space token 被正确激活，输出自带空格 → `use_word_segmentation` 按 model_name 前缀判断，v6 跳过分词。

4. **ort rc.10→rc.12 API 适配**
   - `Session.outputs/inputs` 字段→方法调用（`.outputs()`/`.inputs()`）。
   - `Outlet.name` 字段→`.name()` 方法；`Outlet.input_type`→`.dtype()`。
   - Builder 方法返回 `Result<SessionBuilder, Error<SessionBuilder>>`（非 `ort::Error`），需 `.map_err()`。
   - `ort::inputs!` 宏返回数组（非 Result），不再需要 `?`。
   - `ndarray` 版本 `0.16`→`0.17`（对齐 ort rc.12）。

5. **ort 依赖配置**
   - 不能用 `default-features = false`（会去掉 `tls-native`，导致 ort-sys build script 的 download 编译失败）。
   - 与 asr-local 一致：`ort = { version = "2.0.0-rc.12", features = ["ndarray", "download-binaries"] }`。

6. **opencv 死代码深度清理（2026-07-06 增补）**
   - 原 paddle-ocr-rs 支持 PureRust 和 OpenCV 两种图像处理后端，通过 `#[cfg(feature = "opencv-backend")]` 门控切换。octopus 永远只用 PureRust 后端。
   - 第一阶段：删除全部 opencv 死代码（~1000 行），涉及 8 个文件：`det/postprocess/mod.rs`（最大，~495 行）、`vision/image_backend.rs`、`vision/rotate_crop.rs`、`vision/backend.rs`、`rec/word_boxes.rs`、`config.rs`、`vision/resize.rs`、`rec/preprocess.rs`。
   - 第二阶段：彻底删除 `VisionBackend` enum。`crates/ocr` 和 `crates/desktop` 零引用 VisionBackend，确认完全内部类型。移除 `VisionBackend` enum、`RuntimeConfig.vision_backend` 字段、`resolve_backend_strict` / `resolve_backend_or_pure_rust` / `OPENCV_BACKEND_DISABLED_MESSAGE` / `default_backend`。所有 `_with_backend` 函数变体的 `backend` 参数全部移除，`match backend` 分支直接内联为 PureRust 实现。`vision/backend.rs` 清空为占位文件。涉及 12 个文件。
   - 算法名函数（`sklansky_like_opencv`、`convex_hull_like_opencv`、`unclip_polygon_like_opencv_db`）是纯 Rust 实现，正确保留。
   - Cargo.toml 删除 `[features] opencv-backend = []`。

---

## 8. 后续功能审计修复（2026-07-07）

vendor 合入 main 后对 `crates/paddle-ocr` 核心计算逻辑做的独立功能审计，固化为设计的 3 项决策（纯 bug 修复如 `word_boxes` 对齐、`get_word_info` debug_assert 见 commit，不入此节）：

1. **`sort_boxes_like_python` 保真移植——j+1 相邻插入排序**
   - 复刻 PaddleOCR `tools/infer/predict_system.py` 的 `sorted_boxes`：先按 (y,x) 预排，再对每个 i+1 从 i 往前扫**相邻对 (j, j+1)** 比较，Y 差 ≤ 阈值且 `order[j+1].x < order[j].x` 则 `order.swap(j, j+1)`，否则 `break`（前序已有序，早停）。
   - **陷阱**：曾误移植为固定 `i+1` 目标（`order.swap(j, i+1)`）——目标框无法跨越多位冒泡，同行 N≥3 逆序框排不完全。验证保真移植**必须直接引用上游官方源码逐字比对**，不可用自身移植实现反推（否则 bug 自我证明正确）。

2. **长图切分按绝对 y 坐标去重（非文本去重）**
   - `recognize_long_image_with_blocks` 相邻 chunk 有 `CHUNK_OVERLAP=200` 重叠区，重叠行被两块都识别。
   - 用「已收录行最大 y 底部 `covered_until_y`」做坐标去重：下块中 y 中心 ≤ 该值的行视为重叠区已收录，`drop_overlapped_blocks` 丢弃。
   - **决策理由**：此前按文本逐字相等去重，OCR 轻微波动（`hello` vs `hello!`）即致文本不等、去重失败（残留重复行），也易误删天然重复行。坐标去重对 OCR 波动鲁棒。

3. **行优先假设处用 `as_slice` 而非 `as_slice_memory_order`**
   - `argmax_with_prob`（decode.rs）按 `r*cols` 行优先偏移取行、`masked_mean_in_roi_contiguous`（box_score.rs）按 `(ymin+y)*ncols` 行优先 stride 访问切片——两处都用 `as_slice()`（仅 C-contiguous 返回 Some），而非 `as_slice_memory_order()`（C/F-contiguous 都返回 Some）。
   - **理由**：若切片 F-contiguous（列优先内存），`as_slice_memory_order` 返回 Some 但切片为列优先，行优先偏移会跨行读错。`as_slice` 在 F-contiguous 时返回 None，安全降级到布局无关的 fallback（`axis_iter` / 索引访问）。C-contiguous 常态下两者等价。`threshold.rs` 逐元素阈值化布局无关，仍用 `as_slice_memory_order`（正当的「连续则 SIMD」优化）。

