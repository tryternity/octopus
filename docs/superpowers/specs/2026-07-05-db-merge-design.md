# DB 表合并重构设计（clipboard_history 吞并 transcriptions）

> 日期：2026-07-05
> 状态：✅ 已实现（e2e 通过）
> 分支：`image-viewer-perf`

## 1. 背景与目标

当前 `clipboard_history` 和 `transcriptions` 两表存在大量冗余：每条 ASR 识别的 text、created_at、engine、polish_status 在两表各存一份。两表通过 `transcription_id` 外键关联，维护复杂（级联删除、状态同步）。

**目标**：clipboard_history 吞并 transcriptions，消除冗余列，精简为 `content` + `ref_data` + `meta_info` 三层数据模型。废弃 transcriptions 表。

## 2. 新表结构

```sql
CREATE TABLE IF NOT EXISTS clipboard_history (
    id              INTEGER PRIMARY KEY,       -- 毫秒戳
    item_type       TEXT    NOT NULL,          -- 'text' | 'voice' | 'ocr' | 'image' | 'file'
    content         TEXT    NOT NULL DEFAULT '',  -- voice/ocr/text: 文本全文; image/file: ""
    ref_data        TEXT,                      -- image: blob_hash; file: JSON 路径数组; voice/ocr/text: NULL
    meta_info       TEXT,                      -- JSON，按 item_type 存不同元数据（见下）
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    is_rich         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    has_thumbnail   INTEGER NOT NULL DEFAULT 0,
    segments        TEXT                       -- 段 JSON（仅 voice，段模型真相源）
);
```

### 2.1 item_type 枚举

旧值 `text`（source=clipboard）拆分为：
- `text` — 剪贴板文本
- `voice` — 语音识别文本（旧 source=asr + item_type=text）
- `ocr` — OCR 识别文本（旧 source=ocr + item_type=text）

`image` 和 `file` 不变。**删除 source 列**——item_type 已覆盖来源。

### 2.2 content + ref_data 分层

| item_type | content | ref_data |
|-----------|---------|----------|
| text/voice/ocr | 文本全文 | NULL |
| image | `""` | blob_hash |
| file | `""` | JSON 路径数组 |

### 2.3 meta_info JSON（按 item_type）

所有 Option 字段序列化时 `skip_serializing_if` —— None 字段不出现在 JSON 中，避免 null 膨胀。

```jsonc
// image
{"w": 1386, "h": 916, "size": "2.3M"}

// voice
{"engine": "sensevoice", "asr_mode": "streaming", "char_count": 42, "polished": false}
//   ASR 侧：engine + asr_mode（streaming/vad_segmented）
//   LLM 侧：polish_model + polished

// ocr
{"engine": "paddle", "model": "PP-OCRv6-small", "char_count": 42}

// text
{"char_count": 42}

// file
{"files": [{"size": "1.2M", "type": "pdf"}]}
```

### 2.4 FTS5 索引

FTS5 触发器只用 `content` 做索引源。image/file 的 content 为空字符串 → FTS5 不产生索引项（自动跳过）。voice/ocr/text 的全文被索引。

删除 `search_text` 列——不再单独存储搜索文本。

### 2.5 segments

仅 voice 条目使用。从 transcriptions.segments 迁移过来。其他 item_type 为 NULL。

## 3. 迁移策略

DB 版本号 v16 → v17。

**不迁移历史数据**（用户确认：历史数据可丢弃）。直接 DROP + CREATE：

```sql
-- 1. 废弃旧表
DROP TABLE IF EXISTS clipboard_history_fts;
DROP TABLE IF EXISTS clipboard_history;
DROP TABLE IF EXISTS transcriptions;

-- 2. 建新表（db.sql 更新）
CREATE TABLE IF NOT EXISTS clipboard_history (...);
CREATE VIRTUAL TABLE clipboard_history_fts USING fts5(...);
CREATE TRIGGER clip_fts_ai / clip_fts_ad / clip_fts_au ...;
CREATE INDEX idx_clip_created ON clipboard_history(created_at DESC);
```

无数据迁移、无 JOIN、无临时表。db.sql 里 transcriptions 建表 + 索引删除，clipboard_history 改为新 schema。init_schema 里 v16→v17 跑 DROP + 重跑 INIT_SQL。

`ensure_db` 改为 loop `init_schema` 直到 v17——`init_schema` 每次只走一个分支（一步迁移），旧版库（v2-v16）需多次调用才能到 v17，loop 保证一次 `ensure_db` 跑到最新。

## 4. 代码影响

### 4.1 Rust（clipboard crate）

`store.rs` — 所有 CRUD 改为新表结构：
- `insert_asr_item(conn, text, engine, model, segments)`：item_type='voice'，content=text，meta_info={engine,model,char_count}，segments=...
- `insert_ocr_item(conn, text, engine, model)`：item_type='ocr'，content=text，meta_info={engine,model,char_count}
- `insert_clipboard_item(conn, &NewClipboardItem)`：按类型填 content/ref_data/meta_info
- `delete_item`：不需要级联删 transcriptions（已废弃）
- 查询：`row_to_item` 按 item_type 从 content 或 ref_data 取数据，meta_info JSON 解析

`watcher.rs` — 剪贴板监听写入也补全 meta_info：
- text：`{char_count}`
- image：`{w, h, size}`（size = WebP blob 字节数可读化）
- file：`{files: [{size, type}]}`（stat 每个路径）

`model.rs` — `ClipboardItem` 结构体改为：
```rust
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: String,  // text/voice/ocr/image/file
    pub content: String,
    pub ref_data: Option<String>,
    pub meta_info: Option<MetaInfo>,
    pub is_favorite: bool,
    pub is_rich: bool,
    pub created_at: String,
    pub has_thumbnail: bool,
    pub segments: Option<String>,
}

pub struct MetaInfo {
    // image
    pub w: Option<u32>, pub h: Option<u32>, pub size: Option<String>,
    // voice（ASR 侧）
    pub engine: Option<String>, pub model: Option<String>,
    pub asr_mode: Option<String>,
    // voice（LLM 侧）
    pub polish_model: Option<String>, pub polished: Option<bool>,
    // voice / ocr / text
    pub duration_ms: Option<u64>, pub char_count: Option<usize>,
    // file
    pub files: Option<Vec<FileEntry>>,
}

pub struct FileEntry {
    pub size: Option<String>,  // "1.2M"
    pub file_type: Option<String>,  // 扩展名 "pdf"
}
```

### 4.2 Rust（desktop crate）

- `coordinator.rs`：paste 路径不再 `insert_asr_item`（重复），改 `touch_created_at` 顶到列表顶部（录音过程 `insert_transcription_at_id` 已建条目）
- `coordinator.rs`：`update_transcription_raw` 走 `DbCommand::Insert/UpdateTextSegments/Finalize`，对应 `infra::db` 改写后的函数（全部写 clipboard_history）
- `clipboard_commands.rs`：`cascade_delete_transcriptions` 废弃（不再跨表）；`image_meta.blob_hash` → `ref_data`；`current_ocr_meta()` 返回 `(engine, model)` 元组
- `screenshot_commands.rs`：4 处 `NewClipboardItem` 适配新字段 + meta_info 补 size
- `compact_editor_commands.rs`：`get_transcription_text` 改为从 clipboard_history 读（voice 条目的 content 列）
- `settings_commands.rs`：`delete_history` 直接调 `delete_transcriptions`（已写 clipboard_history），不再重复 `delete_items`

### 4.3 前端

**类型系统适配：**
- `ClipboardItem` 类型：`source` 删除，`item_type` 加 'voice'/'ocr'，新增 `ref_data` / `meta_info` / `has_thumbnail` / `segments`
- 展示逻辑：按 `item_type` 决定图标（Mic/ScanText/Type/ImageIcon/FileText）
- `meta_info` 前端解析为显示文本

**元数据展示（visual design）：**

每条条目底部统一显示元数据行，按 item_type 展示不同信息：

| item_type | content 区 | 底部元数据行 |
|-----------|-----------|-------------|
| text | 文本预览（≤200字截断） | `时间 · N字` |
| voice | 文本预览 | `时间 · N字 · Xs` |
| ocr | 文本预览 | `时间 · N字` |
| image | `[缩略图] WxH · size` | `时间` |
| file | 文件路径（ref_data JSON 解析） | `时间` |

三层视觉层次（content > 类型元数据 > 时间）：
- 内容文本 12.5px foreground → 最强
- 类型元数据 10px typeAccent → 中（分隔点 `·` 更淡 30%）
- 时间戳 10px muted/50% → 最弱

**类型色编码（signature element）：**
```ts
typeAccent = {
  text: "text-stone-500",
  voice: "text-amber-600",
  ocr: "text-teal-600",
  image: "text-indigo-500",
  file: "text-emerald-600",
}
```
色编码仅用于类型图标 + 元数据数字，不侵入内容区——保持克制。voice 额外保留左侧 2px 竖条。

**Helper 函数（types/clipboard.ts）：**
- `metaParts(item)`：生成底部元数据片段（text/voice/ocr 的字数/时长）
- `imageMeta(item)`：生成图片条目 `WxH · size`（放在缩略图旁，不在底部行）
- `typeAccent[item.item_type]`：类型强调色 class

两处组件（浮窗 ClipboardItem.tsx + 设置页 ClipboardPanel.tsx）共用以上 helper。

## 5. 不变量

1. `content` 为空字符串仅当 item_type ∈ {image, file}
2. `ref_data` 仅 image/file 有值
3. FTS5 只索引 content（image/file 不被搜索）
4. `meta_info` 是 JSON 字符串，按 item_type 有不同 schema
5. `segments` 仅 voice 条目有值

## 6. 风险

- **无数据迁移**：用户确认历史数据可丢弃（DROP + CREATE）。
- **file meta_info 初始为空**：旧 file 条目已随 DROP 清空，新 file 条目写入时从文件系统获取 size/type。
- **FTS5 重建**：新表创建时自动重建，无历史数据瞬间完成。
