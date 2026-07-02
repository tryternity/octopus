# 记事本 type 迁移：content_text + type 双类型（text / markdown）

> **状态**：已实现、e2e 通过、合并 main（merge `6e004ac`，2026-07-02）。本文为最终设计；架构概览见 `docs/architecture.md` §octopus-notepad。

## 0. 设计演进：为何是双类型而非三类型

本方案最初把 `type` 放开到 `html` / `text` / `markdown` 三态（html = TipTap 富文本为主）。三类型落地后，富文本在 macOS WKWebView 下历经多轮踩坑才勉强可用——输入时序 `immediatelyRender`、Tailwind preflight 重置 `h1`–`h6` 需手补 `.prose` 样式、`window.prompt` 被禁需改内联输入框、图片需 `note-img:` 协议 + ACL 桥接——收益却不匹配：记事本定位是 ASR / OCR / 剪贴板的「内容收集箱」，纯文本 + Markdown 已足够。

用户判定「富文本对本应用无用、还不好控制」，遂彻底移除：`NoteType` 收窄为 `text` / `markdown`，TipTap 依赖全删，历史 `type=html` 笔记由 DB 迁移 v11→v12 删除。**不要再为记事本重新提议富文本编辑器 / TipTap / 恢复 `NoteType::Html`。**

> `content_html` 列在 schema 保留但恒空（`split_body` 永远写 `""`），无需删列——保留列避免再走一次 ALTER，且不占语义。

## 1. 目标与非目标

### 目标
1. notes 表采纳 `content_text + type` 结构（保留 `content_html` 列但恒空）。
2. `type` 双态：`text`（纯文本，默认）/ `markdown`（md 源码）。
3. 安全迁移链 v9 → v12（幂等、不丢历史数据；v12 删 html 笔记为预期行为）。
4. 前端双类型编辑器：`text` = textarea / `markdown` = 源码 + 可折叠预览（`marked`）。
5. `type` 端到端透传：DB → `Note` struct → IPC 命令 → 前端类型 → 编辑器分发。
6. 非手动来源（剪贴板 / OCR / ASR）存入默认 `type=text`。

### 非目标
- **不**恢复 egui UI（egui 方案暂停，记事本维持 webview）。
- **不**改 `source` 语义（source = 来源 asr/ocr/clipboard/manual，与 type = 内容格式正交）。
- **不**改 FTS5 索引结构（仍索引 `content_text`）。
- **不**支持富文本 / 图片嵌入（已移除）。

## 2. 最终 Schema（`crates/infra/src/db.sql`，新库 INIT_SQL）

```sql
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',   -- 纯文本/md源码（FTS + 预览 + 编辑 source of truth）
    content_html  TEXT    NOT NULL DEFAULT '',   -- 保留列，恒空（富文本已移除）
    type          TEXT    NOT NULL DEFAULT 'text',  -- text | markdown
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);
```
- `content_text` 是 source of truth（text 存原文、markdown 存源码）。
- `content_html` 保留恒空，**不删列**。
- FTS5 表 + 触发器 + 索引不变（索引 `content_text`，`type` 不进 FTS）。

## 3. 迁移链（v9 → v12，幂等）

| 版本跃迁 | 操作 | 说明 |
|----------|------|------|
| v9 → v10 | `ALTER TABLE notes ADD COLUMN type TEXT NOT NULL DEFAULT 'html'`（先查列存在再 ALTER） | 引入 `type` 列；历史笔记默认 `html` |
| v10 → v11 | `ALTER TABLE notes ADD COLUMN content_html ...`（先查列存在） | 兼容曾被 egui 分支重建过（无 `content_html`）的库 |
| v11 → v12 | `DELETE FROM notes WHERE type='html'` | 富文本下线，删除历史 html 笔记 |

- 全新安装（v0/v1）执行 INIT_SQL 后直接 `user_version=12`。
- 每个 ALTER 分支先用 `PRAGMA table_info(notes)` 查列是否存在，幂等保护，避免重复迁移崩溃。
- v12 删除前 `SELECT COUNT(*)` 计数并 log，无 html 笔记时为 noop。

## 4. 后端（Rust）

### 4.1 `NoteType` enum（`crates/notepad/src/model.rs`）
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Text,
    Markdown,
}
// as_str:  Text => "text", Markdown => "markdown"
// from_str: "markdown" => Markdown；"text" / 已下线的 "html" / 未知值 => Text（容错）
```
- `from_str` 未知值回退 `Text`（富文本已移除，历史 `html` 值安全降级为纯文本，不丢内容）。

### 4.2 `Note` struct
含 `note_type: NoteType` 字段（在 `content_text` 之后）。

### 4.3 `store.rs` 读写
- `create_note_at(conn, source, source_ref_id, body, note_type)` / `update_note_at(conn, id, title, body, note_type)`：签名带 `note_type`，INSERT/UPDATE 写 `type` 列。
- `split_body(body, _note_type) -> (String, String)`：返回 `("", body.to_string())`——`content_html` 恒空，`content_text` = body 原文。**无 html 抽取**（`extract_text` 已随 `serialize.rs` 删除）。
- `row_to_note` + 所有 SELECT 列表（`list_notes_at` / `query_with_search` 两分支 / `get_note_at`）在 `updated_at` 后加 `, type`。

### 4.4 IPC 命令（`crates/desktop/src/note_commands.rs`）
- `create_note` / `update_note`：加 `note_type: String` 参数 → `NoteType::from_str` → 透传 store。
- `save_transcription_to_note` / `save_ocr_to_note`：内部固定 `NoteType::Text`（纯文本来源），不再 `<p>` 包裹。
- **已删** `get_note_image` / `insert_note_image`（图片桥接随富文本移除），`main.rs` invoke_handler 同步注销。

## 5. 前端（React + TypeScript，无 TipTap）

- `types/note.ts`：`export type NoteType = "text" | "markdown";`
- `lib/notepad.ts`：`createNote` / `updateNote` 透传 `noteType`；**已删** `getNoteImage` / `insertNoteImage`。
- `NoteEditor.tsx`：按 `note_type` 分发——`markdown` → `<MarkdownEditor>`，`text` → `<textarea>`；标题 + 正文 800ms debounce 保存（同走 `textBody`）；导入（`.md`/`.txt`）→ `textBody`；导出（md→`.md` / text→`.txt`）。顶部仅类型标签 + 导入/导出/收藏/置顶。
- `MarkdownEditor.tsx`：源码 textarea + 轻量工具栏（标题/粗体/斜体/列表/引用/代码/链接）+ `marked` 可折叠预览。
- `NoteList.tsx`：`TYPE_TABS`（全部 / 纯文本 / Markdown）+ 行内 type 角标（`MD` / `T`）+ 行内删除。
- **已删**：`extensions.tsx`（TipTap 编辑器 + Image NodeView）、`index.css` 的 `.ProseMirror` 样式、`@tiptap/*` + `tiptap-markdown` 依赖（bundle 1.2M → 410K）。

## 6. 数据兼容

| 场景 | 处理 |
|------|------|
| 历史 v9 笔记（无 type） | v10 给 `type='html'`，v12 删除（富文本下线，预期行为） |
| 剪贴板 / OCR / ASR 存入 | `type='text'`（纯文本来源） |
| 手动新建 | 默认 `type='text'`，可选 markdown；**已建锁定**不可改 type |
| FTS 搜索 | 不变（索引 `content_text`：text=原文，markdown=源码） |

> 已建笔记 type 锁定：新建时选 type，一旦创建固定。想换格式 → 复制内容新建。理由：跨格式转换有损且复杂，锁定避免数据损坏。

## 7. 测试策略

- `NoteType` roundtrip（`as_str`/`from_str`）+ 未知值 / 已下线 `"html"` → `Text`。
- `create_note_at` / `update_note_at`：text / markdown 直存原文（`content_html` 空，无抽取）。
- 迁移：`migrate_v11_to_v12_deletes_html_keeps_text_markdown`（插 html×2 + text + markdown → 仅留 text/markdown，v=12）；`migrate_v11_to_v12_no_html_is_noop`。
- 现状：infra 48 / notepad 19 / desktop 56 单测全绿。

## 8. 影响面清单（最终）

| 文件 | 改动 |
|------|------|
| `crates/infra/src/db.sql` | notes 建表 `type DEFAULT 'text'`；`content_html` 保留恒空 |
| `crates/infra/src/db.rs` | INIT_SQL → v12；v9→v10 / v10→v11 / v11→v12 迁移分支 + 测试 |
| `crates/notepad/src/model.rs` | `NoteType`（text/markdown）+ `Note.note_type` |
| `crates/notepad/src/store.rs` | create/update 带 `note_type`；`split_body` 恒空；row/SELECT 加 type |
| `crates/notepad/src/serialize.rs` | **已删**（+ `Cargo.toml` 去 `scraper`） |
| `crates/desktop/src/note_commands.rs` | create/update 透传 type；save_* 固定 text；删图片桥接命令 |
| `crates/desktop/src/main.rs` | invoke_handler 注销 `get_note_image` / `insert_note_image` |
| `crates/desktop/frontend/src/types/note.ts` | `NoteType = "text" \| "markdown"` |
| `crates/desktop/frontend/src/lib/notepad.ts` | create/update 透传 noteType；删 image 封装 |
| `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx` | 按 type 分发 textarea / MarkdownEditor |
| `crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx` | md 编辑器（源码 + 工具栏 + marked 预览） |
| `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx` | TYPE_TABS + type 角标 + 行内删除 |
| `crates/desktop/frontend/src/pages/Notepad/extensions.tsx` | **已删**（TipTap 编辑器） |
| `crates/desktop/frontend/src/index.css` | 删 `.ProseMirror` 样式 |
| `crates/desktop/frontend/package.json` | 加 `marked`；删 `@tiptap/*` + `tiptap-markdown` |
