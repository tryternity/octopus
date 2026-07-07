# 剪贴板管理

> 独立的剪贴板历史核心库（`octopus-clipboard` crate），仅依赖 `octopus-infra`，基于 `clipboard-rs` 跨平台读写 + 监听。统一存储 text/voice/ocr/image/file 五类条目，FTS5 全文搜索，图片 WebP BLOB 压缩，自动清理。

源文件：`crates/clipboard/src/`。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `model` | 数据结构：`ItemType`（Text/Image/File）、`Source`（Clipboard/Asr）、`ClipboardItem`（含 `ImageMeta`/`FileMeta`/`AsrMeta`）、`QueryFilter`（6 种过滤 + 分页 + 搜索） |
| `handle` | `ClipboardHandle`：`Mutex<ClipboardContext>` 全局单例（Windows 防锁竞争）+ `AtomicBool` suppress flag + `AtomicBool` recording_enabled gate |
| `watcher` | `ClipboardWatcher`：后台线程跑 `ClipboardWatcherContext::start_watch()`（阻塞），`on_clipboard_change` 回调链 |
| `store` | DB CRUD：通用插入 / ASR 插入 / OCR 插入 / FTS5 JOIN 搜索 / 分页 / 去重 / 引用计数清理 |
| `image` | RGBA → PNG → SHA-256 去重 → WebP 编码 → 缩略图 240×240 |
| `cleanup` | 自动清理：按天数（默认 30）+ 按数量（默认 1000）删除非收藏 + 孤立 blob 回收 + FTS5 索引重建 |

---

## 2. 数据结构

```rust
enum ItemType { Text, Image, File }
enum Source   { Clipboard, Asr }

struct ClipboardItem {
    id: i64,
    item_type: ItemType,
    source: Source,
    content: String,       // voice/ocr/text 全文；image/file 为空串
    ref_data: String,      // image=blob_hash；file=JSON 路径数组
    meta_info: Value,      // JSON，按 item_type 存不同 schema（见 §10）
    segments: Option<String>, // 仅 voice 段 JSON
    is_favorite: bool,
    is_rich: bool,
    has_thumbnail: bool,
    created_at: String,
}

struct QueryFilter {
    item_type: Option<ItemType>,  // 6 种过滤（全部/语音/文本/图片/文件/收藏）
    search: Option<String>,       // >=3 字符 FTS5 MATCH；<3 字符 LIKE
    limit: i64,
    offset: i64,
}
```

`AsrMeta`：`{engine, asr_mode, char_count, polished, polish_model, duration_ms}`（voice 条目）
`ImageMeta`：`{w, h, size}`
`OcrMeta`：`{engine, model, char_count}`
`FileMeta`：`{files: [{size, type}]}`

---

## 3. 监听机制（clipboard-rs 内置）

| 平台 | 机制 | 间隔 |
|------|------|------|
| macOS | 轮询 `NSPasteboard.changeCount` | 500ms |
| Windows | 事件驱动 `AddClipboardFormatListener` | 即时 |
| Linux X11 | `XFixes` 事件驱动 | 即时 |
| Linux Wayland | 两级轮询（MIME 类型 + text 内容） | 500ms |

---

## 4. ClipboardHandle

- **`Mutex<ClipboardContext>` 全局单例**：Windows 上 `ClipboardContext::new()` 与 `empty()` 竞争锁，故全局化避免重复建实例
- **`AtomicBool` suppress flag**：区分 ASR 写入与外部复制。ASR `do_paste` 写剪贴板前置 suppress=true，watcher 的 `on_clipboard_change` 命中 suppress 后直接 return、不执行 `on_change` 闭包（不入库不 emit），paste 完成后置 false
- **`AtomicBool` recording_enabled**：`clipboard_enabled` 运行时镜像。false 时 `on_clipboard_change` 不入库（直接 return）；`set_config` 热重载翻转；`main.rs` setup 启动时按 DB 值 `set_recording_enabled` 一次性同步——`new()` 默认 true，否则重启会复活已关闭的监听

---

## 5. watcher 回调链

`on_clipboard_change` 依次执行：

1. **suppress 检查**：suppress=true → return（ASR 自身写入，跳过）
2. **recording_enabled gate**：recording_enabled=false → return（用户暂停了监听）
3. **类型判断**（优先级 `files > image > text`，非三者则静默跳过避免 `read_text` 失败日志污染）
4. **去重**：文本/文件按 `find_by_text(text, ItemType)` 匹配；图片按 `find_by_content_hash`
5. **存 DB**：`insert_clipboard_item`
6. **通知前端**：`emit("clipboard://changed")`

---

## 6. DB schema v18

### clipboard_history 表

统一存储 text/voice/ocr/image/file——`item_type` 枚举区分。

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | `INTEGER PRIMARY KEY` | voice = 识别开始毫秒时间戳；其他 = 自增 |
| `item_type` | `TEXT` | `text` / `voice` / `ocr` / `image` / `file` |
| `source` | `TEXT` | `clipboard` / `asr` |
| `content` | `TEXT` | voice/ocr/text 全文；image/file 为空串 |
| `ref_data` | `TEXT` | image=blob_hash；file=JSON 路径数组；text/voice/ocr 为空 |
| `meta_info` | `TEXT` | JSON，按 item_type 存不同 schema（见 §10） |
| `segments` | `TEXT` | 仅 voice 段 JSON `[{kind:raw\|polished\|edited, text}]` |
| `is_favorite` | `INTEGER` | 0/1 |
| `is_rich` | `INTEGER` | 0/1 |
| `has_thumbnail` | `INTEGER` | 0/1 |
| `created_at` | `TEXT` | iso 时间戳 |

### clipboard_history_fts 虚表

FTS5 trigram tokenizer，索引 `content` 列。voice/ocr/text 被搜索，image/file content 为空串自动跳过。

### image_data 表

图片 BLOB 存储：

| 列 | 类型 | 说明 |
|---|---|---|
| `hash` | `TEXT PRIMARY KEY` | SHA-256 内容哈希 |
| `blob` | `BLOB` | WebP 编码的图片数据 |
| `thumb` | `BLOB` | 240×240 缩略图 |
| `image_type` | `TEXT` | 编码格式（webp/jpeg） |
| `width` / `height` | `INTEGER` | 原图尺寸 |
| `created_at` | `TEXT` | 创建时间 |

`clipboard_history.ref_data` 引用 `image_data.hash`；删除条目时引用计数为 0 才删 image_data 行（`cleanup_unreferenced_images`）。

### 触发器

3 个触发器自动同步 FTS5 索引：`clip_fts_ai`（INSERT）、`clip_fts_ad`（DELETE）、`clip_fts_au`（UPDATE OF content）。

v17 废弃原 `transcriptions` 表（db.sql 不再含此表）。

---

## 7. FTS5 搜索

`query_history` 搜索逻辑：

| 查询长度 | 路径 |
|----------|------|
| ≥3 字符 | FTS5 MATCH phrase（trigram 倒排索引，包成 `"phrase"` 走 phrase query） |
| <3 字符 | LIKE `%text%` 子串 fallback（trigram 无法生成 3-gram） |

- 6 种 item_type 过滤 + 分页（`LIMIT ? OFFSET ?`）
- `ORDER BY created_at DESC, id DESC` 二级排序——消除秒级 `iso_now` 同秒不稳定（同秒内按 id 保证确定顺序）

`rebuild_fts_index`：FTS5 索引重建，仅启动时一次性 populate；删除路径由 db.sql 触发器 `clip_fts_ad` 增量同步，无需周期 rebuild。运行中删除计数器达 10 自动 rebuild。

---

## 8. 图片存储

`crates/clipboard/src/image.rs`——RGBA → PNG → SHA-256 去重 → WebP 编码 → 缩略图。

**编码流程**：
1. 从剪贴板读取 RGBA 像素
2. `encode_and_hash`：PNG 编码 + SHA-256 计算内容哈希
3. 去重：`find_by_content_hash` 命中则复用已有 image_data 行
4. WebP 编码（`webp` crate，lossless 优先）
5. 缩略图 240×240（Triangle 滤波降采样）

**`encode_to_webp`** 接 `&DynamicImage`，复用调用方已解码的像素（watcher 从 RGBA 构造 DynamicImage；screenshot/migration 复用 `load_from_memory`）。

**编码降级链**（`consts::IMAGE_SAVE_QUALITY` = `"webp:80;jpeg:80"`，`;` 分割、`:` 解析格式与质量）：
1. 正常尺寸先 lossless WebP
2. 失败后走降级链（webp:80 → jpeg:80，依次尝试首个成功）
3. 超尺寸（>16383px，VP8 上限）跳过 lossless 直接进降级链
4. 每次编码 `catch_unwind` 兜底防超大图 panic

`ImageMeta.size` 经 `(SELECT length(blob) FROM image_data WHERE hash = blob_hash)` 子查询算（query_history / get_item_by_id / LIKE / FTS5 四处 SELECT 同步），供列表显示存储大小。

---

## 9. 自动清理

`crates/clipboard/src/cleanup.rs`：

- **按天数**（默认 30 天）+ **按数量**（默认 1000 条）删除非收藏记录
- **孤立 blob 回收**：删除条目后引用计数为 0 的 image_data 行
- **FTS5 索引重建**：仅在有删除/回收时 rebuild，避免定时清理无删除时无谓全表重建

**接入定时调用**：
- `main.rs` setup 启动时跑一次（image_migration 迁入旧图片后）
- 后台线程每小时从 DB 重读 `clipboard_max_items` / `clipboard_max_age_days` 跑一次（让设置页「最大保留条数 / 自动清理天数」真正生效；用户运行时改限额 1 小时内自动生效）

---

## 10. meta_info schema（按 item_type 分）

| item_type | meta_info JSON | 说明 |
|-----------|----------------|------|
| `text` | `{char_count}` | 字符数 |
| `voice` | `{engine, asr_mode, char_count, polished, polish_model, duration_ms}` | ASR 引擎/模式/润色状态/时长 |
| `ocr` | `{engine, model, char_count}` | OCR 引擎/模型（默认 paddle / PP-OCRv5） |
| `image` | `{w, h, size}` | 宽高 + 存储字节数 |
| `file` | `{files: [{size, type}]}` | 文件列表（大小 + MIME 类型） |

序列化时 None 字段跳过不写 null。

---

## 11. ASR 集成

`coordinator.rs::do_paste` 中剪贴板联动：

1. `store::touch_created_at`：录音过程 `insert_transcription_at_id` 已在 clipboard_history 创建 voice 条目（item_type='voice'），paste 时只需 touch `created_at` 顶到列表顶部（不重复创建）
2. **成功后主动 `emit("clipboard://changed")`**：`paste::paste` 写剪贴板设 suppress flag，watcher 的 `on_clipboard_change` 命中后直接 return、不执行含 emit 的 `on_change` 闭包，故 ASR 记录需主动广播前端才能即时渲染
3. 调 `paste::paste`（写系统剪贴板，suppress flag 阻止 watcher 重复记录）

**删除已统一**：transcriptions 表已废弃（v17 DROP），所有 ASR 数据在 clipboard_history（item_type='voice'）；`delete_history` 直接调 `delete_transcriptions`（已写 clipboard_history），删除行数 >0 时主动 `emit("clipboard://changed")` 广播浮窗/设置页双端刷新。
