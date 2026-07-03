# 清除记事本 + 多 tab CompactEditor + OCR 统一改造设计

> 日期：2026-07-03 ｜ 分支：`worktree-clean-used-feature`
> 主题：移除记事本子系统、CompactEditor 升级为多 tab 常驻编辑器、统一三处 OCR 入口、剪贴板新增 OCR 类别

## 1. 背景与目标

记事本（Notepad）目前是一个独立的持久化笔记库（`notes` 表 + FTS5 + 前端页面 + 托盘入口 + 12 个 tauri 命令），与剪贴板历史定位重叠：OCR/ASR 结果既入剪贴板又入笔记，造成数据双写、入口分散。CompactEditor 目前是无状态单文档编辑器（请求-响应回传一段文本）。

四个目标内在统一：**移除记事本后，CompactEditor 升级为多 tab 常驻编辑器承担「多条目查看/编辑」职责；OCR 文本归宿从「笔记」改为「剪贴板 OCR 类别」，编辑统一在 CompactEditor 的 tab 里完成。**

1. **彻底移除记事本子系统**，只保留 CompactEditor。
2. **CompactEditor 升级为多 tab 常驻编辑器**：每个 tab 绑定一个剪贴板条目，可同时编辑/查看多个。
3. **统一三处 OCR 入口**：识别后立即入剪贴板 OCR 类别，作为 tab 打开编辑。
4. **剪贴板新增 OCR 类别**：`source=ocr` + `ocr_meta{engine, model}`。

## 2. 现状

### 2.1 记事本子系统
- **crate**：`crates/notepad/`（`model.rs`、`store.rs`）——仅 `crates/desktop` 依赖（`Cargo.toml: octopus-notepad`）。
- **后端窗口**：`crates/desktop/src/notepad_window.rs`（`open_notepad` / `open_notepad_with_note` / `get_pending_note` / `on_notepad_closed`；模块级 `static PENDING_NOTE: Mutex<Option<i64>>`）。
- **后端命令**：`crates/desktop/src/note_commands.rs` —— `list_notes` / `count_notes` / `get_note` / `create_note` / `update_note` / `delete_notes` / `toggle_note_pinned` / `toggle_note_favorite` / `export_note` / `import_note_from_file` / `save_transcription_to_note` / `save_ocr_to_note`（共 12 个）。
- **托盘**：`tray.rs` 「记事本」菜单项（id=`notepad`）。
- **前端**：`pages/Notepad/`（index/NoteEditor/NoteList）、`types/note.ts`、`hooks/useNotes.ts`、`lib/notepad.ts`、`App.tsx` 路由分支。
- **DB**：`notes` 表 + `notes_fts`（FTS5）+ 3 个同步触发器；当前 15 条数据。
- **capability**：`capabilities/default.json` 的 `windows` 数组含 `notepad_window`。

### 2.2 三处 OCR 入口
| 入口 | 前端 | 后端命令 | 现状行为 |
|---|---|---|---|
| 截图工具栏 | `Screenshot/index.tsx` `doOcr` | `ocr_screenshot`(PNG bytes) | 入库图片条目 + OCR + `write_text` + 新建 note + `open_notepad` |
| 图片预览 | `ImagePreview/index.tsx` `handleOcr` | `ocr_image`(id) + `save_ocr_to_note` + `open_notepad_with_note` | 识别文本存为笔记，开记事本选中 |
| 剪贴板图片条目 | `ClipboardItem.tsx` `handleOcr` | `ocr_image`(id) + `openCompactEditor` + `set_clipboard_item_text` | 识别→CompactEditor 编辑→回写到图片条目 |

- `ocr_image`（`clipboard_commands.rs:389`）：**已是纯识别命令**，按 image id 返回 `text`。
- `ocr_screenshot`（`screenshot_commands.rs:190`）：带入库图片 + note + 开记事本逻辑，内部调 `open_notepad_with_content`。

### 2.3 剪贴板类别
- `clipboard/src/model.rs`：`ItemType{Text, Image, File}`、`Source{Clipboard, Asr}`。
- `store.rs` `build_where`：`asr` / `text` / `image` / `file` / `favorite`。
- 前端 `FilterTabs.tsx` `TABS`：全部 / 语音(asr) / 文本(text) / 图片(image) / 文件(file) / 收藏。
- `clipboard_history` 表**已有 `engine` / `model` 列**（ASR 在用）。

### 2.4 CompactEditor（单文档现状）
- `compact_editor_commands.rs`：`open_compact_editor(initialText, requestId)` → store pending + 开窗/`emit compact-editor://load`；`get_pending_compact_edit`（take）；`close_compact_editor`。
- 前端 `lib/compactEditor.ts` `openCompactEditor(initialText, onResult)`：生成 requestId，注册 `compact-editor://result` / `cancel` 监听，编辑确认后回传。
- 前端 `pages/CompactEditor/index.tsx`：单文档，mount 取 pending，`do_save` emit `compact-editor://result`。
- 当前唯一用途：剪贴板条目就地编辑，`onResult` → `set_clipboard_item_text` 回写。

## 3. 整体新流程

```
【CompactEditor = 多 tab 常驻编辑器】
  每个 tab 绑定一个剪贴板条目(item_id)；可同时打开多个、切换、各自编辑保存。
  打开条目（编辑按钮 / OCR 结果）→ open_compact_editor_tab(item_id)
    · 窗口未开：开窗 + store pending item_id
    · 窗口已开：emit compact-editor://open-tab {item_id}
    · 该 item_id 已在某 tab：直接 activate，不重复开
  tab 内编辑 → Ctrl+S 或关 tab 时 set_clipboard_item_text(item_id, text) 回写
  tab 标题 = 条目内容前 5 字 + "-" + hex(item_id) 后 5 字符

【OCR 新链路】（三处入口统一）
  ocr_screenshot / ocr_image 识别 → 返回 text
  → insert_ocr_clipboard_item(text) 立即入库 source=ocr，返回 item_id（engine/model 后端自填）
  → open_compact_editor_tab(item_id) 作为 tab 打开编辑
  → 用户编辑 Ctrl+S 回写；不编辑直接关 tab 也保留条目（由剪贴板列表自管删除）

【ASR】不变 —— coordinator.rs 已 insert_asr_item 入剪贴板「语音」类别；
       仅去掉"额外存一份笔记"的可选入口（HistoryPanel 按钮）

【记事本】整个子系统移除（crate + 窗口 + 命令 + 前端 + 托盘 + DB 表 + capability）
```

## 4. 任务 1：记事本清除

### 4.1 Rust 后端
- 删 `crates/notepad/` 整个 crate；workspace `Cargo.toml` 的 members 移除 `"crates/notepad"`；`crates/desktop/Cargo.toml` 移除 `octopus-notepad` 依赖。
- 删 `crates/desktop/src/notepad_window.rs`、`note_commands.rs`。
- `main.rs` 的 `generate_handler!` 移除全部 note/notepad 命令（15 个）：`list_notes` / `count_notes` / `get_note` / `create_note` / `update_note` / `delete_notes` / `toggle_note_pinned` / `toggle_note_favorite` / `export_note` / `import_note_from_file` / `save_transcription_to_note` / `save_ocr_to_note` / `open_notepad` / `open_notepad_with_note` / `get_pending_note`。
- `tray.rs`：移除 id=`notepad` 菜单项及其 handler。
- `screenshot_commands.rs`：删 `open_notepad_with_content`；`ocr_screenshot` 改造（见 §6）。
- `compact_editor_window.rs`：仅更新注释（去掉「与 notepad 对称」措辞），**无代码依赖**。

### 4.2 前端
- 删 `pages/Notepad/`（`index.tsx` / `NoteEditor.tsx` / `NoteList.tsx`）、`types/note.ts`、`hooks/useNotes.ts`、`lib/notepad.ts`。
- `App.tsx`：移除 `Notepad` import + 路由分支。
- `pages/Settings/HistoryPanel.tsx`：移除「保存为笔记」按钮（`save_transcription_to_note` 调用，~L304）。

### 4.3 capability
- `capabilities/default.json`：`windows` 数组移除 `"notepad_window"`。

### 4.4 DB 迁移 v12 → v13
- 先 `DROP TABLE notes_fts`（含其触发器依赖），再 `DROP TABLE notes`；3 个同步触发器随 `notes_fts` 一并清除。
- `clipboard_history` **无 schema 变更**（OCR 复用 `engine` / `model` 列）。
- 更新 `infra/src/db.rs` 版本常量 v12 → v13 + 迁移日志。

## 5. 任务 2：CompactEditor 多 tab 改造

### 5.1 形态
单例窗口内多 tab，每个 tab 绑定一个剪贴板条目 `item_id`。tab 状态由前端持有：
```
Tab { itemId: number, text: string, dirty: boolean, title: string }
```
- 同时打开多个、切换查看、各自独立编辑。
- 窗口标题随 active tab 的 title。

### 5.2 交互
- **打开 tab**：调用方 `openCompactEditorTab(itemId)`；若该 `itemId` 已在某 tab → activate；否则新增 tab。
- **加载内容**：新增 tab 时 `invoke get_clipboard_item_text(itemId)` 拉取文本（对称于 `set_clipboard_item_text`；若无此命令则新增）。
- **编辑**：更新 active tab `text`，置 `dirty=true`。
- **保存**：`Ctrl+S`（或关 tab 时）→ `invoke set_clipboard_item_text({ itemId, text })` → 清 `dirty` + 刷新。
- **关 tab**：`dirty` 时提示「保存/放弃/取消」；关闭后从 tabs 移除；关掉最后一个 tab 时窗口保留空状态（不自动关窗，支持再打开新条目）。
- **tab 标题**：`text.slice(0,5) + "-" + itemId.toString(16).slice(-5)`（如 `识别结果-bcd15`）；`text` 不足 5 字按实际；hex 不足 5 位按实际。内容变更保存后刷新标题。

### 5.3 后端命令变化（`compact_editor_commands.rs` / `compact_editor_window.rs`）
- **新增** `open_compact_editor_tab(item_id)`：窗口未开 → 开窗 + store pending `item_id`；窗口已开 → `emit compact-editor://open-tab { itemId }`。
- **新增** `get_pending_compact_tab() -> Option<i64>`：CompactEditor mount 时 take pending `item_id`（开首个 tab）。
- **新增 / 复用** `get_clipboard_item_text(item_id) -> String`：供 tab 加载内容（若已存在等价命令则复用）。
- **删除** 旧的请求-响应机制：`open_compact_editor(initialText, requestId)`、`get_pending_compact_edit`、`compact-editor://load` / `://result` / `://cancel` 事件、`CompactEditPayload`。
- 保留 `close_compact_editor`、`set_clipboard_item_text`。
- pending 静态量从 `CompactEditPayload{text, request_id}` 改为 `Option<i64>`（item_id）。

### 5.4 前端变化
- `pages/CompactEditor/index.tsx`：重写为多 tab —— tabs 状态、tab 栏 UI、activate、Ctrl+S、关 tab（dirty 提示）、标题生成；mount 取 `get_pending_compact_tab` + listen `compact-editor://open-tab`。
- `lib/compactEditor.ts`：`openCompactEditor(initialText, onResult)` → `openCompactEditorTab(itemId)`（`invoke open_compact_editor_tab`，不再注册 result/cancel 监听）。
- 调用方改造：
  - `ClipboardItem.tsx handleEditText`：`openCompactEditor(item.content, onResult)` → `openCompactEditorTab(item.id)`。
  - `ClipboardItem.tsx handleOcr`：见 §6（OCR 入库后 `openCompactEditorTab(itemId)`）。

## 6. 任务 3：OCR 统一新流程

### 6.1 命令变化
- **`ocr_screenshot`（`screenshot_commands.rs`）**：改为纯识别返回 `Result<String, String>`。剥离：入库图片（`insert_clipboard_item`）、`update_search_text`、`write_text`、`open_notepad_with_content`、`emit clipboard://changed`。**保留** `close_all_screenshot_windows`。
- **`ocr_image`（`clipboard_commands.rs`）**：不变（已纯识别返回 text）。
- **新增 `insert_ocr_clipboard_item(text: String) -> Result<i64, String>`**：后端读 `ocr_model` config + OCR 引擎信息自填 `engine` / `model`，调 `insert_ocr_item` 入 `source='ocr'` 条目，返回 `item_id`；`emit clipboard://changed`。注册到 `generate_handler!`。
- **删除** `save_ocr_to_note`、`open_notepad_with_note`（随 note_commands 一并移除）。

### 6.2 三处入口前端改造（统一：识别 → 入库 → 开 tab）
| 入口 | 改造 |
|---|---|
| 截图工具栏 `doOcr` | `const text = await invoke("ocr_screenshot", bytes)` → `const itemId = await invoke("insert_ocr_clipboard_item", { text })` → `openCompactEditorTab(itemId)` |
| 图片预览 `handleOcr` | `const text = await invoke("ocr_image", { id })` → `const itemId = await invoke("insert_ocr_clipboard_item", { text })` → `openCompactEditorTab(itemId)`；删 `save_ocr_to_note` / `open_notepad_with_note` |
| 剪贴板图片条目 `handleOcr` | `const text = await invoke("ocr_image", { id })` → `const itemId = await invoke("insert_ocr_clipboard_item", { text })` → `openCompactEditorTab(itemId)`；**原图片条目保留不动** |

- OCR 入库后条目即持久化；用户在 tab 里编辑后 Ctrl+S 回写该 OCR 条目；不编辑直接关 tab 也保留（由剪贴板列表自管删除）。

### 6.3 运行时约束（e2e 阶段增强）

- **全局并发互斥**：同一时刻仅允许一个 OCR 任务。`ocr_image` / `ocr_screenshot` 入口经 `OcrLockGuard`（`octopus-ocr::engine`，`AtomicBool` + `compare_exchange` 的 RAII guard）`try_acquire`，忙则立即返回 `Err("前一个 OCR 还未完成，请稍后")`、不进推理；guard drop（含 async future cancel）自动释放。任一入口 OCR 进行中时，其他入口再点 OCR → 前端可见提示（剪贴板列表 / 图片预览按钮显琥珀三角 `ocrWarn`、截图屏幕中央 toast、设置页 `showToast`）。
- **超长图切分**：`height > 1600`px 的长截图按块（高 1280、重叠 200）切分逐块识别 + 末行去重合并，避免整图缩放到 det `max_side_len=960` 致短边过小、检测不到文本（2032×15796 长图曾 text_len=0）。
- 这两项是 engine 层能力，详见 `docs/architecture.md` octopus-ocr 节（单一权威）。

## 7. 任务 4：OCR 类别数据结构

### 7.1 后端（`crates/clipboard`）
- **`model.rs`**：
  - `Source` 枚举新增 `Ocr` 变体 + `as_str()` / `from_str()`（容错：未知值回落 `Clipboard`）。
  - 新增 `OcrMeta { engine: String, model: String }`。
  - `ClipboardItem` 新增 `pub ocr_meta: Option<OcrMeta>`。
- **`store.rs`**：
  - 新增 `insert_ocr_item(conn, text: &str, ocr_meta: OcrMeta) -> Result<i64>`：写入 `item_type='text'`, `source='ocr'`, `content=text`, `search_text=text`, `engine`, `model`。
  - `build_where` 新增 `"ocr" => "source = 'ocr'"`。
  - `row_to_item`：`source='ocr'` 时反序列化 `ocr_meta`。
- **DB**：无 schema 变更（复用 `engine` / `model` 列）。

### 7.2 前端
- `types/clipboard.ts`：`Source = "clipboard" | "asr" | "ocr"`；`ClipboardItem` 新增 `ocr_meta?: { engine: string; model: string }`。
- `FilterTabs.tsx`：`TABS` 新增 `{ value: "ocr", label: "OCR" }`（置于「语音」与「文本」之间）。
- `ClipboardItem.tsx`：`source === "ocr"` 条目加来源标记（icon 复用 `ScanText`，与 asr 的 `Mic` 区分）。

## 8. 影响面与风险
- **不可逆**：`notes` 表 DROP（15 条数据丢失，已确认）。
- **cargo workspace**：删 notepad crate 前确认无残留引用——已核实仅 desktop 依赖，`compact_editor_window.rs` 仅注释引用。
- **DB 迁移顺序**：`notes_fts` 先于 `notes` 表 DROP。
- **CompactEditor 重写**：从单文档请求-响应 → 多 tab 状态机，是本次最大前端改动；旧的 `compact-editor://result|cancel` + `requestId` 机制彻底废弃，所有调用方迁移到 `openCompactEditorTab(itemId)`。
- **跨窗口 tab 传递**：剪贴板/截图/预览窗口 → CompactEditor 窗口，经后端 `open_compact_editor_tab` + `compact-editor://open-tab` 事件传递 `item_id`（沿用现状跨窗口事件模式）。
- **命令 ACL**：`default.json` 仅列 `core:window:*`，命令走 `generate_handler!` 注册即可调用；新增命令无需额外 capability，实施时验证。
- **ASR 回归**：`coordinator.rs` `insert_asr_item` 不变；删除 HistoryPanel「存笔记」按钮不影响 ASR 主流程。

## 9. 测试策略
- **单元测试（Rust）**：
  - `clipboard/store`：`insert_ocr_item` + `build_where("ocr")` + `row_to_item` 的 `ocr_meta` 往返。
  - DB 迁移 v12→v13：迁移后 `notes` / `notes_fts` 表消失，`clipboard_history` 结构与数据不变。
- **手动 / e2e**：
  - 截图工具栏 / 图片预览 / 剪贴板图片条目 三处 OCR → 均入库 OCR 类别 + 在 CompactEditor 打开为 tab（engine/model 已填）。
  - 多 tab：连续打开多个条目 → tab 栏显示多个；切换、各自编辑、Ctrl+S 回写；重复打开同一 item → activate 而非新 tab。
  - tab 标题格式正确（前 5 字 + "-" + hex 后 5）。
  - 关 dirty tab → 提示保存/放弃；关最后一个 tab → 窗口保留。
  - OCR 后不编辑直接关 tab → 条目仍保留在剪贴板 OCR 类别。
  - FilterTabs「OCR」筛选仅显示 `source=ocr`。
  - 记事本入口全消失（托盘无「记事本」、无 Notepad 路由、无相关命令）。
  - ASR 仍正常入「语音」类别；文本/ASR 就地编辑改为开 tab 编辑可用。
- **构建回归**：`cargo build -p octopus-desktop` 通过；frontend `npm run build` 通过；现有 clipboard/asr 测试绿。

## 10. 文档同步
- `docs/architecture.md`：移除 notepad 模块章节；CompactEditor 改述为多 tab 常驻编辑器；clipboard 类别表新增 OCR。
- 本 spec 配套 plan（`docs/superpowers/plans/2026-07-03-clean-used-feature.md`）。
