# OCR 识别

> `octopus-ocr` crate——封装 PaddleOCR pipeline（det→cls→rec），ONNX Runtime 推理后端。支持 PP-OCRv5 + PP-OCRv6-small，手动触发，结果入库 + CompactEditor 编辑。

源文件：`crates/ocr/src/`、`crates/paddle-ocr/`（vendored）。

---

## 1. 支持模型

| 模型 | det | rec | cls | keys | 说明 |
|------|-----|-----|-----|------|------|
| PP-OCRv5 | 4.5M | 16M | 572K | 18383 行 | CTC 不输出英文空格，需英文分词 |
| PP-OCRv6-small | 9.7M | 21.5M | 可选 | 18708 行（`ppocrv6_dict.txt`） | 默认；CTC space token 已激活，`use_word_segmentation` 跳过 |

ONNX 标准格式，软链到 HF 缓存。DB config 按 model_name 选择 DB config。

---

## 2. 模块结构

| 模块 | 职责 |
|------|------|
| `engine` | `OcrEngine`：全局单例（`OnceLock`），懒加载模型。`recognize(image_bytes)` 支持任意格式（image crate 自动检测） |
| `model` | 模型路径管理（`~/.octopus/models/ocr/<name>/`）+ `is_model_ready`（det.onnx + rec.onnx + keys.txt 三件套检测，cls.onnx 可选） |

---

## 3. OcrEngine

- **全局单例**（`OnceLock`），懒加载模型
- `instance()` 用 double-checked locking（`INIT_LOCK: Mutex<()>`）串行化首次加载、保证模型只加载一次
- 内部 `Mutex<Option<RapidOcr>>` 提供可变性（`run` 需 `&mut self`）；`None` 表示模型已 idle 释放、下次 `run_ocr` 自动重载
- 模型名从 `app_config.ocr_model` 读取（默认 PP-OCRv5），存 `model_name: String` 供释放时拼 probe id

### 3.1 内存管理：idle 60s 自动释放（2026-07-08）

OCR 偶尔使用，常驻占内存浪费。ASR/VAD 是常驻工作不动；OCR 改为 idle 60s（`OCR_IDLE_TIMEOUT`）后自动释放，下次使用自动重载：

- `OcrEngine` 字段：`inner: Mutex<Option<RapidOcr>>` + `last_used: Mutex<Option<Instant>>` + `model_name: String`
- 首次 `instance()` 用 `INSTANCE.set` 成功后 spawn **std::thread 守护线程**（ocr crate 被 cli/server 共享，不能假设 tokio runtime；`std::thread::spawn` + `sleep` 零 runtime 依赖），每 30s（`OCR_DAEMON_TICK`）检查 `now - last_used > 60s` → `*inner = None`（drop RapidOcr，释放 ort session + mmap 权重）+ `probe(Unload, "ocr:{name}")` 通知系统状态页清条目
- `run_ocr` 入口先刷新 `last_used`（防重载数秒期间误判 idle）；inner 为 None 时重载（`load_rapid_ocr`，重载不调 probe，避免刷新 registry 首次估算）；**重载与 `run` 合并到同一 inner lock 作用域**——消除守护线程在「重载后、run 前」无锁窗口竞态释放导致 `expect` panic
- 与系统状态页联动：`LoadPhase::Unload`（infra/model_probe）经 desktop probe 闭包 → `registry.remove(id)` 移除 OCR 内存条目（释放后状态页不再显示 OCR，下次重载重新估算）

ASR/VAD 用各自 cache 常驻，无 idle 释放。

---

## 4. 超长图切分

`recognize` 对 `height > 1600px` 的长截图按块切分：

| 参数 | 值 | 说明 |
|------|----|------|
| `CHUNK_HEIGHT` | 1280 | 每块高度 |
| `CHUNK_OVERLAP` | 200 | 块间重叠 |

逐块识别 + **跳过与上一块末行相同的起始连续行去重**合并。

---

## 5. 全局并发互斥

`OcrLockGuard`（`static OCR_BUSY: AtomicBool` + `compare_exchange`）做 RAII 互斥。

某入口 OCR 进行中、他入口再点被 `OcrLockGuard` 拒绝时，前端 4 处给出反馈：
- 剪贴板列表 / 图片预览 OCR 按钮显琥珀三角（`ocrWarn` 1.8s）
- 截图屏幕中央黑底 toast
- 设置页 `showToast`（该错误去掉原 `OCR 失败：` 前缀直接显示）

---

## 6. 后处理

| 函数 | 职责 |
|------|------|
| `merge_same_line_blocks` | det 同行多框合并 + 间隙补空格 |
| `segment_english_words` | 17.7K 英文词库 `words_common.txt` 贪心分词（仅 PP-OCRv5 需要——v5 CTC 不输出英文空格；v6 CTC space token 已激活，`use_word_segmentation` 按 model_name 前缀判断跳过） |

---

## 7. 推理后端迁移（2026-07-06）

从 ocr-rs（MNN C++ 推理）迁移到 vendored paddle-ocr-rs（ONNX Runtime），消除 MNN cmake + bindgen + libclang 依赖。ort 与 ASR 引擎共用同一推理后端，跨平台零原生编译。

`crates/paddle-ocr/` 是从 `paddle-ocr-rs` 按需拷贝的精简版：
- **删**：bin/input/model_store/model_registry/output/compat_rapidocr/turbojpeg/clap/opencv/serde_yaml
- **保留**：det/rec/cls/pipeline/runtime/vision 核心

**opencv 死代码清理（~1000 行）**：
1. 删除全部 `#[cfg(feature = "opencv-backend")]` 门控代码
2. 删除 `VisionBackend` enum 本身（`crates/ocr` 和 `crates/desktop` 零引用确认完全内部类型）
3. 移除后所有 `_with_backend` 函数变体合并为单一 pure rust 实现

**关键 bug**：`read_character_file` 原 `trim()` 误删全角空格 U+3000（字典首行）致 CTC 索引偏移 1 位 → 改 `strip_suffix('\r')`。

---

## 8. 触发方式

**手动**——剪贴板浮窗/管理页图片条目点 OCR 按钮（ScanText 图标）。**不支持自动 OCR**。

---

## 9. 结果处理

三处入口（截图工具栏 / 图片预览 / 剪贴板图片条目）识别文本后统一走 `insert_ocr_clipboard_item`（desktop 命令）：

1. `store::insert_ocr_item(conn, text, engine, model)`（item_type='ocr'，meta_info={engine,model,char_count}）
2. `emit("clipboard://changed")`
3. 返回新 id
4. `open_compact_editor_tab(itemId)` 在精简编辑器打开绑定 tab 编辑
5. Ctrl+S 经 `set_clipboard_item_text` 回写 DB

**截图 OCR 后端闭环**（`ocr_screenshot`）：图片入库 + 识别 + insert_ocr + 同进程 open tab（图片 tab + 文本 tab）。

不再写 search_text / 系统剪贴板 / osascript TextEdit。

---

## 10. vision/numeric.rs 工具函数集中

paddle-ocr 内重复的工具函数集中到 `vision/numeric.rs`：

| 函数 | 原重复次数 |
|------|-----------|
| `l2` | 3 处完全相同 |
| `saturate_cast_i16_from_f32` | 2 处 |
| `cv_round_ties_even_f32` / `saturate_cast_i32_round` / `saturate_cast_i16` / `interpolate_cubic_coeffs` / `clip_i32_exclusive_upper` / `clamp_i32_inclusive` | 各处统一 |

同步修 clamp/clip 命名混淆（原 `clip_i32` hi_exclusive vs `clamp_i32` hi_inclusive 语义不可见），改名让 inclusive/exclusive 在函数名上可见。

---

## 11. det/postprocess/ 拆分

原 2226 行 `mod.rs` 拆为 7 子模块：

| 子模块 | 职责 |
|--------|------|
| `threshold.rs` | 二值化 + AVX2/SSE4.1 |
| `contour.rs` | 轮廓提取 + 2x2 膨胀 + AVX2 |
| `box_score.rs` | box/contour 得分计算 + SIMD |
| `geometry.rs` | 最小外接矩形 + 凸包 + Sklansky |
| `unclip.rs` | 多边形扩展 + 填充 + 周长/面积 |
| `filter.rs` | 检测框过滤/排序 |
| `tests.rs` | 单测 |

`mod.rs` 仅保留 `DbPostProcess` struct/impl + `CandidateScratch`/`ScaleTarget` + 模块声明。
