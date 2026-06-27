# OCR 模块设计

**日期**: 2026-06-27
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 0. 概述

为 octopus 新增 OCR（光学字符识别）能力，基于 PaddleOCR（PP-OCRv6）模型 + `ocr-rs` Rust 库（MNN 推理后端）。一期仅用于剪贴板图片识别——用户在剪贴板浮窗或管理页对图片条目点「OCR」按钮，识别文本写入 `search_text`（FTS5 可搜索）+ 写入系统剪贴板 + 用系统文本编辑器新建无标题文档展示结果。

OCR 作为独立 crate（`octopus-ocr`），与 `octopus-asr-local` 平级，一期仅剪贴板场景调用，未来可被 CLI/Server 复用。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── ocr/              # octopus-ocr — 新增，依赖 infra
│   ├── Cargo.toml    # ocr-rs = "2.3", image = "0.25"
│   └── src/
│       ├── lib.rs    # pub use，模块入口
│       ├── engine.rs # OcrEngine 封装：模型加载 + recognize() + 单例缓存
│       └── model.rs  # 模型路径管理 + 就绪检测
├── infra/            # octopus-infra — 复用 models 表 + app_config
├── clipboard/        # octopus-clipboard — 不直接依赖 ocr（由 desktop 调用）
└── desktop/          # octopus-desktop — Tauri 命令 + 前端按钮
```

**依赖关系**：`infra ← ocr ← desktop`（clipboard 不依赖 ocr，desktop 作为编排层调用 ocr + clipboard）

### 1.2 为什么不用 ort（ONNX Runtime）

项目 ASR 用 `ort` 做 ONNX 推理。OCR 选择 `ocr-rs`（MNN 后端）而非 ort 手动实现，原因：

- `ocr-rs` 封装了完整 pipeline（det → crop → cls → rec），API 干净
- `ort` 路线需自己实现 DBNet 后处理（expand/shrink boxes）、CRNN CTC 解码、图片预处理——工作量大且调参痛苦
- OCR 是独立 crate，推理后端隔离合理（MNN 比 ONNX Runtime 更轻量）
- HF 上的 PaddlePaddle 模型是 ONNX 格式，但 `ocr-rs` 官方仓库提供对应的 MNN 转换模型 + 字典文件

### 1.3 engine.rs 核心接口

```rust
pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

impl OcrEngine {
    /// 全局单例，首次调用时懒加载模型（OnceLock<Arc<OcrEngine>>）。
    /// model_name 从 app_config.ocr_model 读取。
    pub fn instance() -> Result<Arc<OcrEngine>>;

    /// 识别图片字节（PNG），返回识别文本（多行用 \n 连接）。
    pub fn recognize(&self, png_bytes: &[u8]) -> Result<String>;
}
```

### 1.4 model.rs

```rust
/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf;

/// 检查模型组三件套是否就绪（det.mnn + rec.mnn + keys.txt）
pub fn is_model_ready(model_name: &str) -> bool;

/// 默认模型名
pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv6-small";
```

## 2. 模型管理

### 2.1 det/rec 分离的存储设计

HuggingFace 上 det 和 rec 是独立 repo（`PaddlePaddle/PP-OCRv6_small_det_onnx` / `_rec_onnx`），但 `ocr-rs` 用的是 MNN 格式，从 ocr-rs 官方 GitHub 仓库下载。

**一个 OCR 模型组 = det.mnn + rec.mnn + keys.txt**，对用户呈现为一个可选项（如「PP-OCRv6-small」）。

### 2.2 models 表复用（零 schema 变更）

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, ...)
VALUES
  ('ocr', 'paddleocr', 'ocr', 'PP-OCRv6-small',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',  -- source: det 下载地址
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',  -- secret_key: rec 下载地址（本地模型复用此字段存下载 URL）
   ...);
```

**字段复用语义**：

| 字段 | ASR local 用途 | OCR 用途 |
|---|---|---|
| `domain` | `'asr'` | `'ocr'` |
| `source` | HF repo | det 模型下载 URL |
| `secret_key` | 空（本地模型） | rec 模型下载 URL |
| `category` | 引擎族 | `'ocr'`（统一） |
| `is_local` | 1 | 1 |
| `is_streaming` | 0/1 | 0 |

### 2.3 app_config

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
  ('ocr_model', 'PP-OCRv6-small', 'setting');
```

### 2.4 文件布局

```
~/.octopus/models/ocr/
└── PP-OCRv6-small/
    ├── det.mnn       4.7M   文本检测模型
    ├── rec.mnn       10M    文本识别模型
    └── keys.txt      73K    字符字典
```

keys.txt 与 rec 模型配套（同仓库下载，固定 URL）。

### 2.5 下载流程

一期手动放置（已就绪）。后续接入模型管理页时：
1. 用户点「下载」
2. 读 `models` 表 source（det URL）→ 下载 det.mnn
3. 读 secret_key（rec URL）→ 下载 rec.mnn
4. keys.txt 从固定 URL 或 rec URL 同目录下载
5. 三件套就位 → `is_enabled` 置 1

## 3. OCR 触发流程

### 3.1 完整流程

```
用户点击图片条目「OCR」按钮
         │
         ▼
┌──────────────────────────────────┐
│  前端 invoke("ocr_image", { id }) │
│  按钮 → loading（spin + 不可点）   │
└──────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────┐
│  后端 ocr_image 命令              │
│  1. 查 DB 拿 blob_hash            │
│  2. 读 ~/.octopus/clipboard_      │
│     images/<hash>.png             │
│  3. 读 app_config.ocr_model       │
│     → 无配置？Err                 │
│  4. is_model_ready(model_name)    │
│     → 否？Err("需下载模型")        │
│  5. OcrEngine::instance()         │
│     → recognize(png_bytes)        │
│  6. 识别文本写入 search_text       │
│  7. 文本写入系统剪贴板             │
│  8. 系统文本编辑器新建无标题文档    │
│  9. 返回识别文本                   │
└──────────────────────────────────┘
```

### 3.2 结果处理（三步，无临时文件）

1. 识别文本写入 `clipboard_history.search_text`（FTS5 触发器自动更新索引，图片变可搜索）
2. 文本写入系统剪贴板（`ClipboardHandle::write_text`，用户可直接 Cmd+V）
3. 系统文本编辑器新建无标题文档，内容为 OCR 文本（用户可编辑/保存/丢弃）

**新建文档方式**：
- **macOS**：osascript 让 TextEdit 新建文档 + 设置文本
  ```applescript
  tell application "TextEdit"
    activate
    make new document with properties {text:"OCR文本"}
  end tell
  ```
- **Windows**：启动 notepad，剪贴板已有文本（用户 Ctrl+V 或后续 SendInput）
- **Linux**：启动 gedit/文本编辑器，剪贴板已有文本

**不落盘临时文件**——避免系统污染和遗忘清理。

### 3.3 模型下载检测

首次点击 OCR 时：
```
is_model_ready？ → false → toast「请先在设置中下载 OCR 模型」
```

一期模型已手动放置，下载流程后续接入。

## 4. 前端集成

### 4.1 入口位置

剪贴板浮窗（`ClipboardItem.tsx`）+ Settings 剪贴板管理页（`ClipboardPanel.tsx`），仅 `item_type === "image"` 的条目显示 OCR 按钮（`ScanText` 图标，lucide-react）。

### 4.2 按钮状态机（三态 + 过程提示）

```
idle ──→ loading ──→ done（✓ 0.7s）──→ idle
              │
              └─→ error（toast）──→ idle
```

**loading 态分阶段反馈**：
- 按钮 → `Loader2` spin，`disabled`（不可重复触发）
- 模型下载中（如需）→ toast「正在下载 OCR 模型…」
- OCR 识别中 → 按钮旁浮现「识别中…」小字（`animate-pulse`，stone-400，10px）

**done 态**：
- 成功 → ✓ emerald-600（0.7s）→ toast「已识别」+ 系统 .txt 弹出
- 无文字 → ✓ amber-500（0.7s）→ toast「未识别到文本」

**error 态**：
- toast「OCR 失败：…」→ 按钮恢复 idle

### 4.3 与现有按钮的关系

- OCR 与「保存图片」独立操作，不互斥
- OCR 不改变图片条目视觉（只更新 `search_text`）
- 已 OCR 过的图片可重复点击（覆盖 `search_text`）

### 4.4 Settings 模型管理页

后续迭代接入。在 ModelsPanel 新增 OCR 分区，与 ASR 引擎并列，支持下载/切换/删除。一期不做。

## 5. 数据流与存储

### 5.1 DB（零 schema 变更）

OCR 只写 `clipboard_history.search_text`：
```
OCR 前：search_text = ""（空，图片条目一直是空的）
OCR 后：search_text = "识别出的文本" → FTS5 触发器 clip_fts_au 自动更新索引
```

**不变量**：
- OCR 只写 `search_text`，不改 `content`（content 始终是 blob_hash）
- OCR 不碰 `transcriptions` 表（ASR 专属）
- 重复 OCR 覆盖 `search_text`，不做版本历史
- `item_type` 保持 `image`，不因 OCR 变成 text

### 5.2 models 表 seed

`db.sql` 新增 OCR 模型 seed：
```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
  ('ocr','paddleocr','ocr','PP-OCRv6-small',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',
   'auto','PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日',
   1,1,0);
```

`is_enabled=1`（一期手动放置模型，标记为已就绪）。

### 5.3 app_config seed

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
  ('ocr_model', 'PP-OCRv6-small', 'setting');
```

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 模型未下载 | `is_model_ready` 返回 false → toast「请先在设置中下载 OCR 模型」 |
| 模型加载失败 | MNN 初始化异常 → toast「OCR 模型加载失败」+ 记 error 日志 |
| 图片读取失败 | blob 文件丢失/损坏 → toast「图片文件读取失败」 |
| OCR 结果为空 | 纯图片无文字 → toast「未识别到文本」 |
| OCR 结果含特殊字符 | 原样写入 search_text，FTS5 trigram 正常索引 |
| 重复 OCR | 覆盖 search_text，不做版本 |
| 多语言文本 | PP-OCRv6 small 支持中/英/繁体/日，无需切换 |
| 超大图片 | 无限制（det 阶段内部 resize），>50MB PNG 可能内存紧张 → 记日志继续 |

**并发安全**：`OcrEngine::instance()` 用 `OnceLock<Arc<OcrEngine>>`，全局单例，无锁读取。模型加载只在首次调用时发生一次。

**降级**：OCR 是可选功能，不影响剪贴板核心流程。OCR 失败只 toast 报错，图片条目仍可正常复制/保存/收藏。

## 7. 依赖变更

**新增（Rust）**：
- `ocr-rs = "2.3"`（MNN 推理 + PaddleOCR pipeline 封装）
- `image = "0.25"`（图片解码，infra 已有）

**构建依赖**：
- `cmake` + `cc`（ocr-rs 的 MNN FFI 编译需要）

**新增（前端）**：
- `ScanText` 图标（lucide-react，已有）

## 8. 实施分期

| 阶段 | 范围 | 依赖 |
|---|---|---|
| **Step 1** | octopus-ocr crate（engine.rs + model.rs + 集成测试） | 模型已就绪 |
| **Step 2** | desktop ocr_image Tauri 命令 + DB seed（models + app_config） | Step 1 |
| **Step 3** | 前端 OCR 按钮（ClipboardItem + ClipboardPanel）+ 状态机 | Step 2 |
| **Step 4**（后续） | Settings 模型管理页 OCR 分区 + 下载流程 | Step 2 |

## 9. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| ocr-rs MNN 编译问题（macOS arm64） | 中 | 无法构建 | 提前验证 cargo build；MNN 预编译库覆盖主流平台 |
| ocr-rs API 变更（v2.x 仍在快速迭代） | 低 | 接口不兼容 | 锁定版本 `= "2.3.1"` |
| macOS osascript TextEdit 权限 | 低 | 新建文档失败 | 降级为只写剪贴板 + toast |
| MNN 与 ort 共存内存冲突 | 极低 | 推理崩溃 | 独立 crate 隔离 + 测试验证 |

## 10. 实施偏差与补充

### 10.1 图片存储迁移影响

OCR spec 原设计从文件系统 `~/.octopus/clipboard_images/<hash>.png` 读取原图。实施过程中图片存储迁移到 DB BLOB（详见 `2026-06-27-image-storage-blob-design.md`），OCR 读取路径相应调整：
- `ocr_image` 命令从 `image_data` 表读 WebP BLOB（不再读文件）
- `OcrEngine::recognize` 改用 `image::load_from_memory`（自动检测格式，支持 WebP）

### 10.2 ocr-rs 实际 API

- `OcrEngine::new(det_path, rec_path, charset_path, config)` — 接受 `impl AsRef<Path>`
- `recognize(&DynamicImage)` → `OcrResult<Vec<OcrResult_>>`，`OcrResult_` 有 `.text` 字段
- MNN 预编译库从 GitHub 自动下载（build script），release 构建需手动放置预编译包

### 10.3 osascript 输出静默

osascript 创建 TextEdit 文档时会在 stderr 输出「document 未命名」，需 `.stdout(Stdio::null()).stderr(Stdio::null())` 静默。
