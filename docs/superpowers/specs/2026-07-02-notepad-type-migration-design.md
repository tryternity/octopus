# 记事本 type 迁移：webview 适配 content_text + type 表结构

> **状态**：设计阶段（brainstorming 产出，待用户 review）
> **背景**：egui 记事本方案已暂停（分支 `tag-egui-notebook`），但其数据库表结构重构（引入 `type` 列区分内容格式）有保留价值。本方案在 **webview（TipTap 富文本）现役实现**上采纳该重构，并把 `type` 放开到 `text` / `markdown` / `html`（富文本）三态。

## 1. 目标与非目标

### 目标
1. 采纳 egui 分支的 `content_text + type` 表结构思想，但**保留 `content_html` 列**（富文本必需），形成 `content_text + content_html + type` 三列结构。
2. `type` 放开为三态：`text`（纯文本）/ `markdown`（md 源码）/ `html`（TipTap 富文本）。
3. 提供安全的 v9→v10 迁移：`ALTER TABLE ADD COLUMN type`，**不丢历史数据**（egui 分支的 drop+recreate 迁移废弃不用）。
4. webview 端三类型编辑器全做：html=TipTap（现有保留）/ text=纯 textarea / markdown=md 编辑器（源码+可折叠预览）。
5. `type` 字段端到端透传：DB → `Note` struct → IPC 命令 → 前端类型 → 编辑器分发。
6. 非手动来源（剪贴板/OCR/ASR）存入笔记默认 `type=text`。

### 非目标
- **不**恢复 egui UI（egui 方案放弃，记事本维持 webview）。
- **不**改 `source` 语义（source=来源 asr/ocr/clipboard/manual，与 type=内容格式正交，二者独立）。
- **不**改 FTS5 索引结构（仍索引 `content_text`）。
- **不**改图片 BLOB 桥接（`get_note_image`/`insert_note_image` 不受影响；图片仅 html 类型通过 TipTap 嵌入）。

## 2. 现状（main，v9）

### Schema（`crates/infra/src/db.sql:292`）
```sql
CREATE TABLE notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT,
    content_html TEXT NOT NULL DEFAULT '',   -- TipTap 富文本，source of truth
    content_text TEXT NOT NULL DEFAULT '',   -- 从 html 抽取的纯文本（FTS + 列表预览）
    source TEXT NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned INTEGER NOT NULL DEFAULT 0,
    is_favorite INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
-- user_version = 9，无 type 列
```

### 数据流
- **编辑**：前端 TipTap → `getHTML()` → `updateNote(id, title, contentHtml)` → IPC `update_note` → `store::update_note_at` → `content_text = extract_text(content_html)` → 存 `content_html` + `content_text`。
- **抽取**：`serialize::extract_text(html)` 用 `scraper` 解析，块级元素间换行、`<img>` 转「[图片]」。
- `content_html` 是 source of truth（前端传），`content_text` 是后端派生（FTS/预览用）。

### 写入点（需适配 type）
| 位置 | 操作 | 现状 |
|------|------|------|
| `notepad/src/store.rs:231` `create_note_at` | INSERT | 接 `initial_html`，`content_text=extract_text(html)` |
| `notepad/src/store.rs:249` `update_note_at` | UPDATE | 接 `content_html`，`content_text=extract_text(html)` |
| `clipboard/src/store.rs:976` | INSERT（剪贴板一键存入） | `content_html`+`content_text` 直写 |
| `desktop/src/note_commands.rs` `save_transcription_to_note` | 新建（ASR） | 纯文本 `<p>` 包裹 → create |
| `desktop/src/note_commands.rs` `save_ocr_to_note` | 新建（OCR） | 纯文本 → create |

### 前端（React + TipTap）
- `frontend/src/types/note.ts`：`Note` interface（`content_html` + `content_text`，无 type）
- `frontend/src/lib/notepad.ts`：`createNote(source, sourceRefId, initialHtml)` / `updateNote(id, title, contentHtml)`
- `frontend/src/pages/Notepad/NoteEditor.tsx`：TipTap 编辑器
- `frontend/src/pages/Notepad/NoteList.tsx` / `index.tsx` / `useNotes.ts`

### egui 分支（`tag-egui-notebook`，参考但不直接采用）
- v10 schema：**删 `content_html`**、加 `type`(text/markdown)、`content_text` 存源码
- v9→v10 迁移：**drop+recreate notes，丢弃旧数据**（破坏性，本方案废弃）
- `NoteType` enum（Text/Markdown）、`Note.note_type` 字段、`create_note(NoteSource, ref, text, NoteType)`

## 3. 关键决策（已与用户确认）

| 决策 | 选择 | 理由 |
|------|------|------|
| html 富文本存储 | **方案 A：保留 `content_html` 列** | FTS 永远索引纯文本 `content_text`（搜索质量最好）；html 类型完整复用 main 现有 TipTap + extract_text 逻辑，零回归风险；egui 重构真正有价值的是 `type` 列，而非删 `content_html` |
| 编辑器范围 | **三类型全做** | html=TipTap（现有）/ text=textarea / markdown=md 编辑器，UI 可切换 |

## 4. Schema 设计

### 目标建表（新库 INIT_SQL，user_version=0）
基于 main 现有 notes 表**加一列 `type`**：
```sql
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',   -- 纯文本/md源码/html抽取（FTS+预览）
    content_html  TEXT    NOT NULL DEFAULT '',   -- 富文本原始（仅 type=html 用）
    type          TEXT    NOT NULL DEFAULT 'html',  -- text | markdown | html（新增）
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);
```
- 列顺序：`content_text` 在前（承接 egui 思路，且 FTS/预览主用），`content_html` 次之。
- `type DEFAULT 'html'`：新库直接建笔记默认富文本（与 main 现状一致）。
- FTS5 表、触发器、索引**不变**（仍索引 `content_text`，`type` 不进 FTS）。

### 迁移 v9 → v10（已有库）
```sql
ALTER TABLE notes ADD COLUMN type TEXT NOT NULL DEFAULT 'html';
PRAGMA user_version = 10;
```
- **安全**：`ALTER ADD COLUMN` 不锁表、不丢数据，SQLite 原生支持。
- 历史笔记（均为 TipTap 富文本）自动获得 `type='html'`，**零感知**。
- `content_html` / `content_text` 列原样保留。
- **明确废弃** egui 分支的 drop+recreate 迁移：本方案不引用、不复用其 v10 迁移代码，重写为 ALTER ADD。
- 注：main 线的 v10 与 egui 分支的 v10（drop+recreate）**无关**——egui 分支不合并，两套迁移互不影响。

## 5. 后端设计（Rust）

### 5.1 `NoteType` enum（`crates/notepad/src/model.rs`）
在 egui 分支 Text/Markdown 基础上**加 Html**：
```rust
/// 笔记内容格式。DB `notes.type` 列。
/// - `Html`：TipTap 富文本（content_html 存原始，content_text 存抽取纯文本）。
/// - `Text`：纯文本（content_text 存原文，content_html 空）。
/// - `Markdown`：md 源码（content_text 存源码，content_html 空，预览端渲染）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    Html,
    Text,
    Markdown,
}

impl NoteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Html => "html",
            NoteType::Text => "text",
            NoteType::Markdown => "markdown",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => NoteType::Text,
            "markdown" => NoteType::Markdown,
            // 含 "html" 及未知值 → Html（保守：历史数据/异常值保持富文本不丢格式）
            _ => NoteType::Html,
        }
    }
}
```
- `from_str` 未知值回退 `Html`（不是 egui 的 Text）：历史数据默认 html，容错也偏 html，避免富文本被误降级。

### 5.2 `Note` struct（`crates/notepad/src/model.rs`）
加 `note_type` 字段：
```rust
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_text: String,
    pub content_html: String,
    pub note_type: NoteType,        // 新增
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

### 5.3 `store.rs` 读写适配
- **`create_note_at`** 签名加 `note_type`：
  ```rust
  pub fn create_note_at(
      conn, source, source_ref_id, body: &str, note_type: NoteType,
  ) -> Result<i64>
  ```
  - `note_type=Html`：`body` 视为 html，`content_text = extract_text(body)`，`content_html = body`。
  - `note_type=Text|Markdown`：`content_text = body`（原文/源码），`content_html = ""`。
- **`update_note_at`** 签名加 `note_type` + body 语义同上：
  ```rust
  pub fn update_note_at(conn, id, title, body: &str, note_type: NoteType) -> Result<()>
  ```
- **行映射**（`row_to_note`）：读取 `type` 列 → `NoteType::from_str`，SELECT 列表加 `type`。
- `extract_text` **仅在 `note_type=Html` 时调用**；text/markdown 的 `content_text` 直接取原文，不经抽取。

### 5.4 `clipboard/src/store.rs:976` 适配
剪贴板一键存入为纯文本 → `type='text'`：
```sql
INSERT INTO notes (title, content_text, content_html, type, source, created_at, updated_at) ...
-- content_text = 剪贴板文本，content_html = ''，type = 'text'
```
（或改调 `notepad::store::create_note_at(_, Clipboard, _, text, NoteType::Text)` 统一入口，二选一，plan 阶段定。）

### 5.5 IPC 命令（`desktop/src/note_commands.rs`）
- `create_note`：加 `note_type: String` 参数 → `NoteType::from_str` → 传 store。
- `update_note`：加 `note_type: String` 参数。
- `save_transcription_to_note` / `save_ocr_to_note`：内部 `create_note_at(_, ..., NoteType::Text)`（纯文本来源固定 text）。

## 6. 前端设计（React + TipTap）

### 6.1 类型（`frontend/src/types/note.ts`）
```ts
export type NoteType = "html" | "text" | "markdown";

export interface Note {
  id: number;
  title: string | null;
  content_text: string;
  content_html: string;
  note_type: NoteType;       // 新增
  source: NoteSource;
  // ... 其余不变
}
```

### 6.2 IPC 封装（`frontend/src/lib/notepad.ts`）
```ts
export const createNote = (source, sourceRefId, body: string, noteType: NoteType) =>
  invoke<number>("create_note", { source, sourceRefId, body, noteType });

export const updateNote = (id: number, title: string, body: string, noteType: NoteType) =>
  invoke<void>("update_note", { id, title, body, noteType });
```
- 参数名 `initialHtml`/`contentHtml` → 统一为 `body`（语义随 type 变化：html 时是 html，text 时纯文本，markdown 时 md 源码）。

### 6.3 编辑器分发（`NoteEditor.tsx`）
按当前笔记 `note_type` 渲染对应编辑器：
- **`html` → TipTap**（现有 `NoteEditor.tsx` 逻辑保留）：编辑产出 `getHTML()`，调 `updateNote(id, title, html, "html")`。
- **`text` → `<textarea>`**：等宽字体，产出纯文本，调 `updateNote(id, title, text, "text")`。
- **`markdown` → md 编辑器**（新增组件 `MarkdownEditor.tsx`）：
  - 左：源码 `<textarea>`（等宽）+ 轻量工具栏（标题/粗体/斜体/列表/代码/链接，插入 md 语法）。
  - 右：可折叠预览面板，用 `marked`（或 `markdown-it`）渲染。
  - 产出 md 源码，调 `updateNote(id, title, md, "markdown")`。

### 6.4 type 切换 UX（**已定：方案①**）
**已建笔记 type 锁定不可改**：新建笔记时可选 type（默认 html）；一旦创建，该笔记 type 固定。想换格式 → 复制内容新建。理由：跨格式内容转换有损且复杂（html↔markdown 双向转换易丢排版），锁定避免数据损坏。

> 备选方案②（已建可切换 + best-effort 有损转换）评估后否决，原因同上。

### 6.5 列表标记（`NoteList.tsx`）
列表项显示 type 小标记（如角标 `MD` / `TXT` / 不标=html），帮助区分。低优先，可放 plan 末尾。

## 7. 数据兼容

| 场景 | 处理 |
|------|------|
| 历史 v9 笔记（无 type） | 迁移后 `type='html'`（DEFAULT），TipTap 正常编辑，零感知 |
| 剪贴板/OCR/ASR 存入 | 默认 `type='text'`（纯文本来源） |
| 手动新建 | 默认 `type='html'`（富文本，与现状一致） |
| FTS 搜索 | 不变（索引 `content_text`：html=抽取纯文本，text=原文，markdown=源码） |
| 图片嵌入 | 仅 `type=html`（TipTap）支持，text/markdown 不支持图片（无富文本） |

## 8. 已定决策（用户 review 确认）

1. **type 切换 UX = 方案①**：新建时选 type（默认 html），已建锁定不可改。见 §6.4。
2. **markdown 预览库 = `marked`**（轻量、广泛）。
3. **剪贴板存入路径 = 改调 `notepad::store::create_note_at`** 统一入口（DRY，单一写入点）。clipboard crate 已在 workspace 内，可依赖 notepad。

## 9. 测试策略

### 后端（Rust 单测，TDD）
- `NoteType` roundtrip：`as_str`/`from_str` 三态 + 未知值→Html。
- `create_note_at` 三类型：html 抽取 content_text、text/markdown 直存。
- `update_note_at` 三类型同上。
- **迁移测试**（参考 egui 分支 `migrate_v9_to_v10_rebuilds_notes_schema`，但断言相反）：
  - 建旧 v9 库（有 content_html，无 type）+ 插一条真实数据 → init_schema →
  - 断言：`type` 列存在；**旧数据保留**（COUNT 不变）；`content_html`/`content_text` 未丢；该行 `type='html'`；`user_version=10`；FTS 触发器仍工作（插新行能 MATCH 命中）。
- `extract_text` 仅 html 调用：text/markdown 不经抽取（content_text=原文）。

### 前端
- 三编辑器分别渲染 + 产出正确 body 类型。
- type 切换 UX（依 §8 决策）。
- 列表 type 标记显示。

### e2e（真实运行）
- 历史笔记迁移后可正常打开编辑（html）。
- 新建 text/markdown 笔记 → 编辑保存 → 重开内容正确 → 搜索能命中。
- 剪贴板存入 → type=text → 列表可见可搜。

## 10. 影响面清单

| 文件 | 改动 |
|------|------|
| `crates/infra/src/db.sql` | notes 建表加 `type` 列 |
| `crates/infra/src/db.rs` | INIT_SQL 更新；加 v9→v10 ALTER ADD 迁移分支；init log |
| `crates/notepad/src/model.rs` | `NoteType` enum（+Html）、`Note.note_type` 字段 |
| `crates/notepad/src/store.rs` | create/update 加 note_type 参数 + 分发抽取；row 映射加 type |
| `crates/notepad/src/serialize.rs` | 注明 extract_text 仅 html 用（逻辑不变） |
| `crates/clipboard/src/store.rs:976` | 剪贴板存入加 type='text'（或改调 notepad） |
| `crates/desktop/src/note_commands.rs` | create/update 命令加 note_type 参数；save_transcription/save_ocr 固定 text |
| `crates/desktop/frontend/src/types/note.ts` | NoteType + Note.note_type |
| `crates/desktop/frontend/src/lib/notepad.ts` | createNote/updateNote 加 noteType |
| `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx` | 按 type 分发编辑器 |
| `crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx` | 新增 md 编辑器组件 |
| `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx` | type 标记（低优） |
| `crates/desktop/frontend/src/pages/Notepad/index.tsx` | 新建 type 选择 / 切换 UX |
| `crates/desktop/frontend/package.json` | 加 `marked` 依赖 |
| `crates/desktop/dist/*` | 前端 rebuild + 提交（dist 已跟踪） |

## 11. 风险

- **markdown 编辑器是新前端工作**：工作量集中在 `MarkdownEditor.tsx` + 工具栏 + 预览。用轻量 `marked`+textarea 控制 scope。
- **type 切换内容转换**（若选方案②）：html↔markdown 双向转换有损，需充分测试或直接选方案①规避。
- **迁移幂等**：`ALTER ADD COLUMN` 在已有 type 列的库上会报错，迁移代码需先查 `PRAGMA table_info(notes)` 判断列是否存在（幂等保护），避免重复迁移崩溃。
- **dist 提交**：前端改完须 `npm run build` 并提交 dist，否则 Tauri 打包用旧前端。
