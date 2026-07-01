# 记事本（内容收集箱）功能设计

> ⚠️ 已被 egui 方案替代（2026-07-01）。记事本迁至独立 egui 进程，见 `docs/superpowers/specs/2026-07-01-notepad-egui-design.md`。本文档保留作历史参考（webview + TipTap + content_html 方案已下线）。

**日期**: 2026-06-30
**状态**: 设计稿，待评审
**分支**: `worktree-feature-notepad`（worktree: `.claude/worktrees/feature-notepad`）

## 0. 概述

为 octopus 新增一个「内容收集箱」式的记事本：ASR / OCR / 转译记录的识别结果可一键存入记事本做整理，并在记事本内继续编辑。形态为独立窗口（左侧笔记列表 + 右侧富文本编辑器）。

- **编辑格式**：富文本为内部模型（所见即所得），Markdown / 纯文本作为序列化与导入导出格式——一个引擎三种格式互通。
- **存入语义**：每次「存入记事本」= 新建一条笔记，自动记录来源（语音 / OCR / 剪贴板）与时间戳，关联原记录 id，来源徽标可点击回溯。
- **技术选型**：前端富文本引擎用 TipTap（基于 ProseMirror，React 生态最成熟，有 markdown 序列化扩展）。

后端为新建独立 crate `octopus-notepad`（仅依赖 `octopus-infra`），承载笔记全部业务逻辑；`infra/db` 加表与迁移；`desktop` 加薄 Tauri command 层 + 窗口 + 前端页面。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── notepad/            # octopus-notepad — 新增，仅依赖 infra
│   ├── Cargo.toml      # infra, scraper（HTML→text）, anyhow
│   └── src/
│       ├── lib.rs      # pub use model / store / serialize / export
│       ├── model.rs    # Note / NoteSource / NoteFilter / NoteSort
│       ├── store.rs    # CRUD + FTS 搜索（infra::with_db）
│       ├── serialize.rs# content_html → content_text 抽取（scraper）
│       └── export.rs   # 导入/导出文件 I/O（~/Documents/octopus/notes/）
├── infra/              # db：加 notes / notes_fts 表 + v9 迁移 + 触发器
└── desktop/            # Tauri 命令 + 窗口 + 前端
    ├── src/
    │   ├── note_commands.rs   # 薄 command 层，转调 octopus-notepad
    │   └── notepad_window.rs  # notepad_window 窗口管理
    └── frontend/src/
        ├── pages/Notepad/
        │   ├── index.tsx          # 三栏布局
        │   ├── NoteList.tsx       # 列表 + 搜索 + 来源筛选 + 分页
        │   ├── NoteEditor.tsx     # TipTap 编辑器 + 工具栏 + 自动保存
        │   └── editor/extensions.ts # TipTap 扩展 + Image NodeView
        ├── lib/notepad.ts         # invoke 封装
        └── hooks/useNotes.ts      # 列表状态 + notepad://changed 监听
```

**依赖关系**：`infra ← notepad ← desktop`

### 1.2 为什么独立 crate

与 `octopus-clipboard` / `octopus-ocr` / `octopus-capx` 一致：核心能力做成仅依赖 `infra` 的独立 crate，业务逻辑（CRUD、序列化、文件 I/O）下沉到 crate，`desktop` 只留薄 command 层与 UI。好处：逻辑可单测、可被未来 cli/server 复用、与 desktop 的 Tauri/前端耦合解耦。

`infra` 只承担最底层——表 schema、迁移、`with_db` 访问入口（与 `clipboard_history` 表同级）。所有笔记业务逻辑在 `octopus-notepad`。

### 1.3 模块边界

| 单元 | 职责 | 依赖 |
|---|---|---|
| `infra::db` | notes/notes_fts 表 schema + v9 迁移 + 触发器 + `with_db` | 无 |
| `notepad::model` | 数据结构定义 | infra |
| `notepad::store` | CRUD + FTS 搜索 + 排序分页 | infra |
| `notepad::serialize` | HTML→纯文本抽取（生成 content_text，后端为 source of truth） | scraper |
| `notepad::export` | 导入读文件 / 导出写文件 | std |
| `desktop::note_commands` | Tauri command 转调 notepad crate | notepad |
| 前端 `Notepad/*` | UI + TipTap 编辑器 | react, @tiptap/* |

## 2. 数据模型

### 2.1 `notes` 表（infra/db，v9 迁移新增）

```sql
CREATE TABLE IF NOT EXISTS notes (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  title         TEXT,                         -- 可空，空则列表显示正文截取
  content_html  TEXT    NOT NULL DEFAULT '',   -- 富文本内部格式（TipTap getHTML）
  content_text  TEXT    NOT NULL DEFAULT '',   -- 纯文本抽取，FTS 索引 + 列表预览
  source        TEXT    NOT NULL DEFAULT 'manual',  -- asr/ocr/clipboard/manual
  source_ref_id INTEGER,                       -- 关联 transcription_id 或 clipboard_history.id
  is_pinned     INTEGER NOT NULL DEFAULT 0,
  is_favorite   INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT    NOT NULL,
  updated_at    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_notes_source  ON notes(source);
```

**字段说明**：
- `content_html`：TipTap `getHTML()` 产物。图片节点为 `<img src="note-img:<hash>" alt="...">`（见 §6.2），引用 `image_data.hash`，不存临时 blob URL。
- `content_text`：由后端 `serialize::extract_text(html)` 抽取（scraper 去 tag），**前端 `update_note` 只传 `content_html`**，后端生成 text——后端为 source of truth，避免前端漏传/篡改导致 FTS 失真。
- `source_ref_id`：溯源外键。`PRAGMA foreign_keys` 关闭下不做 DB 级约束（与 clipboard 一致），引用有效性由应用层查询判断（原记录删除则徽标灰显）。

### 2.2 `notes_fts`（FTS5，trigram，仿 `clipboard_history_fts`）

```sql
CREATE VIRTUAL TABLE notes_fts USING fts5(
  title, content_text,
  content='notes', content_rowid='id', tokenize='trigram'
);
-- 3 触发器（AFTER INSERT / DELETE / UPDATE OF title,content_text）自动同步
```

迁移幂等策略：v8→v9 时 `notes_fts` drop+create 使旧库生效（仿 v7→v8 的 fts 重建手法）。

### 2.3 搜索规则（store.rs）

- 搜索词 ≥3 字符：包成 phrase 走 trigram MATCH（`notes_fts MATCH '"query"'`）。
- <3 字符：`content_text`/`title` LIKE 子串 fallback。
- 排序：`is_pinned DESC, updated_at DESC, id DESC`（置顶优先 + 二级排序消除同秒抖动，仿 clipboard）。
- 分页：`limit/offset`，前端手动「加载更多」（仿 Settings 管理页）。

## 3. `octopus-notepad` crate 接口

### 3.1 model.rs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NoteSource { Asr, Ocr, Clipboard, Manual }

impl NoteSource {
    pub fn as_str(&self) -> &'static str { /* "asr"/"ocr"/"clipboard"/"manual" */ }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NoteFilter {
    pub source: Option<NoteSource>,
    pub favorite: bool,
    pub pinned: bool,
    pub search: Option<String>,  // None 或 <3 字符 → LIKE；≥3 → FTS MATCH
    pub limit: i64,
    pub offset: i64,
}
```

### 3.2 store.rs

```rust
pub fn list_notes(filter: &NoteFilter) -> Result<Vec<Note>>;
pub fn count_notes(filter: &NoteFilter) -> Result<i64>;
pub fn get_note(id: i64) -> Result<Option<Note>>;

/// 新建。initial_html 由调用方提供（识别结果转 <p>.../；手建为空）。
/// content_text 由内部 serialize 抽取。created_at/updated_at = now。
pub fn create_note(source: NoteSource, source_ref_id: Option<i64>, initial_html: &str) -> Result<i64>;

/// 更新正文/标题。content_text 由 content_html 重新抽取；updated_at = now。
/// title 为空串则存 NULL（列表显示用 content_text 截取）。
pub fn update_note(id: i64, title: &str, content_html: &str) -> Result<()>;

pub fn delete_notes(ids: &[i64]) -> Result<usize>;
pub fn toggle_pinned(id: i64) -> Result<()>;
pub fn toggle_favorite(id: i64) -> Result<()>;
```

全部经 `infra::with_db(|conn| ...)`，错误用 `anyhow::Result`。`update_note`/`toggle_*`/`delete_notes` 在 desktop command 层成功后 `emit("notepad://changed")`（store 层不 emit，保持纯逻辑可单测）。

### 3.3 serialize.rs

```rust
/// content_html → 纯文本：scraper 解析，按块拼接（<p>/<h*>/<li> 间加换行），<img> 转 "[图片]"。
pub fn extract_text(html: &str) -> String;
```

### 3.4 export.rs

```rust
pub const NOTES_DIR: &str = "octopus/notes";  // 相对 Documents，跨平台用 dirs::document_dir()

/// 导出：把前端序列化好的字符串写到 ~/Documents/octopus/notes/<safe_title>.<ext>。返回绝对路径。
pub fn write_export(filename_stem: &str, ext: &str, content: &str) -> Result<PathBuf>;

/// 导入：读 .md 文件原文返回（md→HTML 的解析在前端 TipTap，后端只做 I/O）。
pub fn read_import(path: &Path) -> Result<String>;
```

格式转换（HTML↔md↔txt）放前端 TipTap（最准），后端只落盘/读文件。

## 4. desktop Tauri commands（`note_commands.rs`）

薄封装，转调 `octopus-notepad`，成功写操作 `emit("notepad://changed")`：

```rust
#[tauri::command] fn list_notes(filter: NoteFilter) -> Result<Vec<Note>, String>;
#[tauri::command] fn count_notes(filter: NoteFilter) -> Result<i64, String>;
#[tauri::command] fn get_note(id: i64) -> Result<Option<Note>, String>;
#[tauri::command] fn create_note(source, source_ref_id, initial_html) -> Result<i64, String>;
#[tauri::command] fn update_note(id: i64, title: String, content_html: String) -> Result<(), String>;  // 自动保存；title 空串=无标题
#[tauri::command] fn delete_notes(ids: Vec<i64>) -> Result<usize, String>;
#[tauri::command] fn toggle_pinned(id) -> Result<(), String>;
#[tauri::command] fn toggle_favorite(id) -> Result<(), String>;
#[tauri::command] fn export_note(stem, ext, content) -> Result<String, String>;   // 返回路径
#[tauri::command] fn import_note_from_file(path) -> Result<String, String>;        // 返回 md 原文
```

### 4.1 集成入口 command（识别结果 → 笔记）

```rust
/// 语音结果 → 新建笔记：取转写文本 → <p> 包裹 → create_note(Asr, Some(transcription_id))
#[tauri::command] fn save_transcription_to_note(transcription_id: i64) -> Result<i64, String>;

/// OCR 结果 → 新建笔记：text → <p> 包裹 → create_note(Ocr, None)
#[tauri::command] fn save_ocr_to_note(text: String) -> Result<i64, String>;
```

`save_transcription_to_note` 内部查 `transcriptions` 拿内容；查不到对应记录时返回错误（不静默建空笔记）。成功后 `emit("notepad://changed")`。（`save_clipboard_to_note` 已于 2026-07-01 移除——剪贴板条目不再存入记事本。）

### 4.2 溯源回溯

`get_note` 已返回 `source` + `source_ref_id`。前端「查看来源」按钮：
- `asr` → 复用 `open_settings(initial_page="history")` + 定位到 `transcription_id`（HistoryPanel 已有按 id 定位能力则复用，否则滚动高亮）
- `clipboard` → `open_settings(initial_page="clipboard")` + 定位 `clipboard_history.id`
- `ocr` / `manual` / `source_ref_id` 已失效 → 徽标灰显 + tooltip「原记录已删除」，不提供跳转

## 5. 窗口与入口

### 5.1 `notepad_window`

- Rust 动态创建（`WebviewWindowBuilder`，label=`notepad_window`），独立窗口、原生标题栏、可调大小、位置记忆（复用 settings_window 的窗口位置记忆机制）。
- `App.tsx` 加 `case "notepad_window": return <Notepad />`。
- 托盘菜单加「记事本」项 → `open_notepad()`（show + set_focus；已开则聚焦）。

### 5.2 全局快捷键

**默认不绑**（octopus 快捷键已拥挤，避免冲突）。设置页留一个可配置项（后续接入现有 shortcut 配置体系），MVP 可不做。

### 5.3 各识别结果「存入记事本」入口

lucide `NotebookPen` 图标按钮，点击调对应 §4.1 command：

| 位置 | 调用 |
|---|---|
| `Settings/HistoryPanel.tsx` 识别记录行操作 | `save_transcription_to_note(...)` |
| OCR 流程（OCR 后文本） | `save_ocr_to_note(text)` |

> **已移除入口**（2026-07-01）：`Result/index.tsx` 结果窗工具栏（长篇模式原地编辑替代）、`Clipboard/ClipboardItem.tsx` 剪贴板浮窗条目、`Settings/ClipboardPanel.tsx` 剪贴板管理页行操作的「存入记事本」按钮均已移除——后端 `save_clipboard_to_note` 命令 + `saveClipboardToNote` helper 一并删除。

存入成功 toast 提示「已存入记事本」（不强制弹出窗口，避免打断当前流程）。

## 6. 前端

### 6.1 TipTap 编辑器配置（`editor/extensions.ts`）

- `StarterKit`（段落 / H1-3 / 列表 / 引用 / 代码块 / 粗斜体 / 历史）
- `Link`
- 自定义 `Image` NodeView（见 §6.2）
- `tiptap-markdown`（md 序列化；实施时锁定兼容 React 19 的 TipTap v3 版本）

工具栏：粗 / 斜 / H1-3 / 无序列表 / 有序列表 / 引用 / 代码 / 分割线 / 图片 / 链接 / 撤销 / 重做 + 导入 / 导出按钮。

### 6.2 Image NodeView（关键）

内部 src 用稳定协议 `note-img:<hash>`，引用 `image_data.hash`：

- **插入图片**：调用 `insert_image(hash, alt)` → 编辑器 `image` 节点 `attrs = { src: "note-img:" + hash, alt }`。
- **渲染（NodeView 组件）**：解析 `src` 的 `note-img:` 前缀取 hash → `invoke('get_image_blob', hash)` 拿 WebP bytes → `URL.createObjectURL` → 渲染 `<img src={blobUrl}>`。blob URL 在组件卸载时 `revokeObjectURL`。
- **序列化（getHTML）**：TipTap 输出 `<img src="note-img:<hash>" alt="...">`，src 始终是稳定协议，**不存临时 blob URL**——笔记内容可持久化、跨会话还原。
- **取图（`get_note_image(hash)` command）**：新增于 `note_commands.rs`，调 `octopus_clipboard::store::get_image_blob(conn, hash)` 取原图 WebP → 编码为 `data:image/webp;base64,...` 返回（仿现有 `get_image_thumb` 的 data URL 手法，避免 IPC 字节数组 4-5x 膨胀）。**notepad crate 不依赖 clipboard**——图片 BLOB 获取由 desktop command 层桥接。
- **插入图片**：文件选择 → 读图 → `clipboard::store::insert_image_data` 入库（复用 `image_data` + SHA-256 去重）得 hash → 编辑器插入 `note-img:<hash>`。

### 6.3 NoteEditor.tsx

- `useEditor`（TipTap），`content` = 当前 note 的 `content_html`。
- `onUpdate` debounce 800ms → `update_note(id, title, getHTML())`（自动保存，防丢失）。
- 标题输入框；空标题传空串 `update_note(id, "", ...)`，列表显示用正文截取。
- 导出：编辑器序列化（md 用 `editor.storage.markdown.getMarkdown()` / txt 用 `getText()` / html 用 `getHTML()`）→ `export_note(stem, ext, content)` → toast 路径。**HTML 导出**前遍历 `<img src="note-img:...">` 调 `get_note_image(hash)` 替换为 data URL，使导出 HTML 自包含可在外部打开；md/txt 导出图片以 "[图片]" 占位。
- 导入：文件选择 → `import_note_from_file(path)` 取 md 原文 → 解析为 TipTap JSON（tiptap-markdown）→ `setContent` + `update_note`。

### 6.4 NoteList.tsx

- 搜索框 + 来源筛选 tab（全部 / 语音 / OCR / 剪贴板 / 收藏）→ 改 `NoteFilter` → `list_notes`。
- 笔记项：标题（或正文截取）/ 预览（content_text 前 N 字）/ 相对时间 / 来源徽标（可点回溯）/ 置顶图钉 / 收藏星。
- 手动「加载更多」分页（`offset += limit`）。
- 监听 `notepad://changed` 事件 → 自动 `list_notes` 刷新（仿 `useClipboardHistory`）。

### 6.5 lib / hooks

- `lib/notepad.ts`：所有 invoke 封装（仿 `lib/tauri.ts`）。
- `hooks/useNotes.ts`：列表 + filter + 分页状态 + `notepad://changed` 监听。

## 7. 数据流

```
存入：
  识别结果[存入按钮] → save_*_to_note(ref_id)
    → 查 transcriptions/clipboard_history 取内容 → 转 HTML
    → notepad::create_note（写 notes + content_text 抽取 + fts 触发器同步）
    → emit("notepad://changed") → 记事本窗口(若开) 列表刷新
    → toast「已存入记事本」

编辑：
  TipTap onUpdate(debounced 800ms) → update_note(id, title, getHTML())
    → notepad::update_note（重抽 content_text + 更新 fts + updated_at）
    → emit("notepad://changed")（列表预览/时间刷新；当前编辑项不重渲染避免光标跳动）

导入/导出：
  导出：TipTap 序列化(md/txt/html) → export_note 写 ~/Documents/octopus/notes/ → toast 路径
  导入：选 .md → import_note_from_file 读原文 → tiptap-markdown 解析 → setContent → update_note

溯源：
  徽标点击 → open_settings(history|clipboard) + 定位 ref_id；ref 失效 → 灰显
```

## 8. 错误处理

| 场景 | 处理 |
|---|---|
| DB CRUD 失败 | command 返回 `Err(String)`，前端 toast |
| `save_*_to_note` 查不到原记录 | 返回错误（不静默建空笔记），toast「原记录不存在」 |
| 溯源 `source_ref_id` 对应记录已删 | 徽标灰显 + tooltip「原记录已删除」，不跳转 |
| 图片 `get_image_blob` 失败（hash 失效） | NodeView 显示占位图 + alt |
| 自动保存冲突 | MVP 单窗口编辑同一 note 无并发；多窗口同时开同一 note 不在 MVP 范围 |
| 导出目录不可写 | 返回错误，toast 提示路径 |
| TipTap 内容过大 | SQLite text 无硬限；前端编辑器可加软限提示（可选） |

## 9. 测试

**后端 infra/db**（单测）：
- notes CRUD 正确性
- `content_text` 由 `content_html` 抽取正确（含 `<img>` → "[图片]"）
- FTS：≥3 字符 MATCH / <3 字符 LIKE fallback
- 触发器同步（insert/update/delete 后 fts 一致）
- v9 迁移幂等（drop+create fts 后旧库可搜）
- 排序：置顶优先 + updated_at + id 二级

**后端 notepad crate**（单测）：
- store：list/count filter 各分支、toggle、delete 批量
- serialize：各种 HTML 结构抽取
- export：文件读写 + 路径安全（stem 含特殊字符转义）

**后端 desktop note_commands**（集成）：
- `save_transcription_to_note` / `save_ocr_to_note` → 笔记存在 + `source`/`source_ref_id`/`content_html` 正确；原记录不存在时报错

**前端**（组件 + 序列化）：
- TipTap 渲染 content_html + 反序列化一致
- HTML ↔ md ↔ txt 三向序列化（导出文本正确）
- Image NodeView：`note-img:<hash>` → blob 渲染；getHTML 仍为稳定协议
- 自动保存 debounce 触发 update_note

**e2e**：
- 语音识别 → 结果窗「存入」→ 记事本列表可见 + 来源徽标=语音 + 点击溯源跳到识别记录
- 剪贴板图片条目「存入」→ 笔记含 `<img src="note-img:...">` → 编辑器渲染图片
- 编辑笔记 → 搜索命中 → 导出 `.md` 文本正确

## 10. 范围与非目标（YAGNI）

- ❌ 标签 / 文件夹 / 多级目录（MVP 扁平 + 来源筛选 + 收藏/置顶 + 搜索）
- ❌ 多端同步 / cli / server 接入（crate 已留口，MVP 仅 desktop）
- ❌ 全局快捷键默认绑定（设置项留位，MVP 可不做）
- ❌ 笔记内嵌非图片附件（仅图片，复用 image_data）
- ❌ md/txt 导出保留图片（MVP 以 "[图片]" 占位；仅 HTML 导出把图片 inline 成 data URL 自包含）
- ❌ 协同编辑 / 多窗口同 note 并发（单窗口编辑）
