# DB 表合并重构设计（clipboard_history 吞并 transcriptions）

> 日期：2026-07-05
> 状态：📋 设计中
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

```jsonc
// image
{"w": 1920, "h": 1080, "size": "2.3M"}

// voice
{"engine": "sensevoice", "duration_ms": 5000, "char_count": 42, "model": "...", "engine_mode": "vad_segmented", "polish_model": "...", "polished": false}

// ocr
{"engine": "PP-OCRv6-small", "char_count": 42}

// text
{"char_count": 42}

// file
{"size": "1.2M", "type": "pdf"}
```

### 2.4 FTS5 索引

FTS5 触发器只用 `content` 做索引源。image/file 的 content 为空字符串 → FTS5 不产生索引项（自动跳过）。voice/ocr/text 的全文被索引。

删除 `search_text` 列——不再单独存储搜索文本。

### 2.5 segments

仅 voice 条目使用。从 transcriptions.segments 迁移过来。其他 item_type 为 NULL。

## 3. 迁移策略

DB 版本号 v16 → v17。

### 3.1 新建临时表 + 数据迁移

```sql
-- 1. 创建新表（临时名）
CREATE TABLE clipboard_history_new (...);

-- 2. 从旧 clipboard_history 迁移（含 JOIN transcriptions 补全 meta_info）
INSERT INTO clipboard_history_new (id, item_type, content, ref_data, meta_info, ...)
SELECT
    ch.id,
    CASE
        WHEN ch.source = 'asr' THEN 'voice'
        WHEN ch.source = 'ocr' THEN 'ocr'
        ELSE ch.item_type  -- text/image/file
    END,
    CASE WHEN ch.item_type IN ('image', 'file') THEN '' ELSE ch.content END,
    CASE WHEN ch.item_type = 'image' THEN ch.blob_hash
         WHEN ch.item_type = 'file' THEN ch.content
         ELSE NULL END,
    -- meta_info：按类型组装 JSON
    CASE
        WHEN ch.source = 'asr' THEN json_object(
            'engine', ch.engine,
            'model', ch.model,
            'duration_ms', t.duration_ms,
            'char_count', t.char_count,
            'engine_mode', t.engine_mode,
            'polish_model', t.polish_model,
            'polished', CASE WHEN t.polish_status IN ('applied', 'edited') THEN 1 ELSE 0 END
        )
        WHEN ch.source = 'ocr' THEN json_object(
            'engine', ch.engine,
            'char_count', length(ch.content)
        )
        WHEN ch.item_type = 'image' THEN json_object(
            'w', ch.width, 'h', ch.height
        )
        WHEN ch.item_type = 'text' THEN json_object(
            'char_count', length(ch.content)
        )
        WHEN ch.item_type = 'file' THEN json_object()
    END,
    ch.is_favorite, ch.is_rich, ch.created_at, ch.has_thumbnail,
    CASE WHEN ch.source = 'asr' THEN t.segments ELSE NULL END
FROM clipboard_history ch
LEFT JOIN transcriptions t ON ch.transcription_id = t.id;

-- 3. 替换旧表
DROP TABLE clipboard_history;
ALTER TABLE clipboard_history_new RENAME TO clipboard_history;

-- 4. 重建索引 + FTS5
CREATE INDEX ...;
CREATE VIRTUAL TABLE clipboard_history_fts USING fts5(...);
CREATE TRIGGER ...;

-- 5. 废弃 transcriptions
DROP TABLE transcriptions;
```

### 3.2 image size 补充

image_data 表的 blob 大小在迁移时 JOIN 查：
```sql
LEFT JOIN image_data id ON ch.blob_hash = id.hash
-- meta_info 加 'size': format_size(length(id.blob))
```

### 3.3 file size/type 补充

file 条目的 content 是 JSON 路径数组。迁移时无法从 DB 查文件大小（需文件系统访问）。**迁移时 meta_info 留空 JSON `{}`，后续条目写入时填充。**

## 4. 代码影响

### 4.1 Rust（clipboard crate）

`store.rs` — 所有 CRUD 改为新表结构：
- `insert_asr_item`：item_type='voice'，content=text，meta_info={engine,duration_ms,...}，segments=...
- `insert_ocr_item`：item_type='ocr'，content=text，meta_info={engine,char_count}
- `insert_clipboard_item`：按类型填 content/ref_data/meta_info
- `delete_item`：不需要级联删 transcriptions（已废弃）
- 查询：`row_to_item` 按 item_type 从 content 或 ref_data 取数据，meta_info JSON 解析

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
    // voice
    pub engine: Option<String>, pub model: Option<String>,
    pub duration_ms: Option<u64>, pub char_count: Option<usize>,
    pub engine_mode: Option<String>, pub polish_model: Option<String>,
    pub polished: Option<bool>,
    // file
    pub file_type: Option<String>, pub file_size: Option<String>,
}
```

### 4.2 Rust（desktop crate）

- `coordinator.rs`：`set_current_transcription_id` → 废弃（不再写 transcriptions）
- `transcript.rs`：`update_transcription_raw` 改为直接写 clipboard_history segments
- `clipboard_commands.rs`：`cascade_delete_transcriptions` 废弃（不再跨表）
- `compact_editor_commands.rs`：`get_transcription_text` 改为从 clipboard_history 读（source/voice 类）

### 4.3 前端

- `ClipboardItem` 类型：`source` 删除，`item_type` 加 'voice'/'ocr'
- 展示逻辑：按 `item_type` 决定图标（Mic/ScanText/Type/ImageIcon/FileText）
- `meta_info` 前端解析为显示文本

## 5. 不变量

1. `content` 为空字符串仅当 item_type ∈ {image, file}
2. `ref_data` 仅 image/file 有值
3. FTS5 只索引 content（image/file 不被搜索）
4. `meta_info` 是 JSON 字符串，按 item_type 有不同 schema
5. `segments` 仅 voice 条目有值

## 6. 风险

- **迁移不可逆**：DROP transcriptions 后无法回退。需要在迁移前备份。
- **file size 缺失**：迁移时无法补充，旧 file 条目 meta_info 为 `{}`。
- **FTS5 重建**：迁移期间搜索不可用（瞬间完成，无感知）。
- **segments 迁移**：transcription_id 为 NULL 的旧 voice 条目（如有）segments 会丢失。
