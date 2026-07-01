# 精简编辑器（Compact Editor）设计

> 日期：2026-06-30
> 状态：**已实现**（OCR/剪贴板走独立精简编辑器窗；语音 Result 改为原地编辑框尺寸双模式）。后端 `cargo test` 全绿；e2e 待用户手动验证。详见 plan 顶部「实现状态」。
> 关联：`docs/superpowers/plans/2026-06-30-compact-editor.md`
> 关联：`docs/superpowers/specs/2026-06-30-notepad-design.md`（完整版记事本，本设计与之并列、不替代）

> **2026-06-30 设计修订（语音 Result 改为原地双模式）：** 原 §3.5① 让语音 Result「展开编辑」弹独立精简编辑器窗——**取消**。
> 语音 Result 改为**编辑框尺寸双模式**（精简 520×116 小条 / 长篇 720×480 撑满）+ 工具栏「放大/缩小」开关切换。窗口物理固定 720×480（setSize 在透明无边框悬浮窗被 NSWindow 拒绝，改 CSS 伪装切容器尺寸 + 透明区点击穿透）。Result **不再调用** `openCompactEditor`。
> 精简编辑器（独立窗）**仅保留给 OCR 与剪贴板文本**。详见 §3.5① 重写。

## 1. 背景与目标

完整版记事本（notepad_window）已实现，但本期更需要一个**精简编辑器**：纯编辑工具，只有「轻工具栏 + 文本正文区」，没有标题输入框、没有左侧分类侧栏、**不持久化为笔记**——编辑结果**还给调用方**。

核心诉求：
1. **OCR 识别**后，用户目前只能在剪贴板浮窗的小框里改文本，不舒服。希望「点编辑 → 迅速展开成舒适的大编辑器」。
2. **剪贴板文本条目**（含 ASR 语音文本）希望「快速打开编辑」。

本设计提供一个可被三处复用的纯文本编辑器窗口，编辑后的文本通过事件返回给发起调用的窗口。

## 2. 范围

**做：**
- 新建独立窗口 `compact_editor_window`（纯文本编辑器）。
- 顶部轻工具栏：撤销/重做、字号 −/+、字符计数、查找/替换、清空、保存/取消。
- 跨窗口「文本返回」事件契约（request_id 区分多调用方）。
- 三处集成：① 语音 Result（编辑框**原地双模式** + 放大/缩小开关，**不用独立窗**）；② OCR（替换原系统 TextEdit，用独立窗）；③ 剪贴板文本条目（新增「编辑」按钮，用独立窗）。独立精简编辑器窗仅 ②③ 复用。

**不做（YAGNI）：**
- 不做富文本（TipTap/ProseMirror）——三个调用方都是纯文本，textarea 对中文输入法（IME）友好且所见即所得，足够。
- 不做标题、分类、收藏、搜索——这些属于完整版记事本。
- 不持久化为笔记——编辑器是无状态工具，结果只还调用方。
- 不做窗口 keep-alive——关窗即销毁、再开重建（页面轻，重建快；如日后觉得卡再改 show/hide）。

## 3. 架构

### 3.1 窗口与生命周期

新建窗口管理 `crates/desktop/src/compact_editor_window.rs`，镜像 `notepad_window.rs`：

- `WINDOW_LABEL = "compact_editor_window"`。
- 单例：`get_webview_window` 命中已存在则 `show + set_focus` 并通过事件推送新文本（见 §3.3 并发开窗）；否则创建。
- 创建参数：`.title("编辑")`、`.inner_size(720, 560)`、`.min_inner_size(480, 360)`、`.decorations(true)`（原生标题栏）、`.visible(true)`、`.resizable(true)`、居中。
- macOS 激活策略：开窗切 `Regular`（Dock 显图标），关窗切回 `Accessory`，与 notepad/settings 对称。新增 `on_compact_editor_closed(app)`，并在 `main.rs` 的 `RunEvent::WindowEvent { Destroyed }` 分支按 label 挂载。
- 生命周期：**关窗即销毁**（destroy-on-close，`close()` 触发 Destroyed 走清理）。再次打开重建窗口。

### 3.2 后端命令（`crates/desktop/src/compact_editor_commands.rs`，薄层）

复用 `result_window.rs` 的「写 PENDING → 建窗/聚焦 → 前端 mount 拉取」模式，但 PENDING 携带结构体：

```rust
// 静态 PENDING：open 时写入，前端 mount 时 take。
// 用 Mutex<Option<PendingEdit>>，无 ready 握手——编辑器窗口是按需创建（非预建隐藏窗），
// open 必然「先写 PENDING 再建窗」，React mount 时 get 一定能读到，无 TOCTOU。
static PENDING: Mutex<Option<PendingEdit>> = Mutex::new(None);

struct PendingEdit { text: String, request_id: String }

#[tauri::command]
pub fn open_compact_editor(initial_text: String, request_id: String, app_handle: AppHandle);

#[tauri::command]
pub fn get_pending_compact_edit() -> Option<PendingEdit>;  // 前端 mount 时 take

#[tauri::command]
pub fn close_compact_editor(app_handle: AppHandle);
```

`open_compact_editor` 内部：
1. `*PENDING.lock() = Some(PendingEdit { text, request_id })`。
2. 若窗口已存在：`emit("compact-editor://load", { text, request_id })` + show + focus（前端 mount 时已 take 过首次 PENDING，并发再开走事件推送）。
3. 否则：建窗（React mount → 调 `get_pending_compact_edit` → take → 载入）。

`close_compact_editor`：`get_webview_window(...)?.close()`（触发 Destroyed → macOS 切 Accessory）。

三个命令在 `main.rs` 的 `generate_handler!` 注册（紧邻 `notepad_window::open_notepad`）。

> `PendingEdit` 经 Tauri IPC 序列化为 camelCase：`{ text, requestId }`。前端取 `requestId`。

### 3.3 事件契约（核心：文本如何还回去）

> 仅 OCR（②）与剪贴板文本（③）走此跨窗口契约。语音 Result（①）改为原地双模式编辑，不跨窗口、不参与此契约。

**request_id 由调用方前端生成**（`crypto.randomUUID()`，跨窗口无碰撞）。一次完整握手：

1. **调用方**：生成 `requestId` → `invoke("open_compact_editor", { initialText, requestId })` → 记住 `requestId` → `listen("compact-editor://result", handler)`，handler 内按 `requestId` 过滤命中才应用。
2. **保存**：编辑器 `emit("compact-editor://result", { requestId, text })` → `invoke("close_compact_editor")`。
3. **取消 / X 关窗**：编辑器 `emit("compact-editor://cancel", { requestId })` → 关窗（unmount 时也兜底发一次 cancel，防悬空监听）。
4. **load（并发再开）**：后端向已存在窗口 `emit("compact-editor://load", { text, requestId })`，前端监听载入新文本。

`emit` 广播到所有窗口；各调用方按 `requestId` 过滤，互不串扰。

### 3.4 编辑器组件（`crates/desktop/frontend/src/pages/CompactEditor/index.tsx`）

- 主体：全高 `<textarea>`（IME 友好），填充窗口剩余空间。
- 顶部工具栏（lucide-react 图标，风格对齐剪贴板浮窗）：

  | 工具 | 实现 |
  |---|---|
  | ↶ / ↷ 撤销·重做 | `document.execCommand('undo'/'redo')` 触发 textarea 原生栈（实用；Cmd+Z/Y 原生也生效） |
  | A− / A+ 字号 | 调 textarea `style.fontSize`（可读性），记忆到 localStorage |
  | 123 字符计数 | `[...text].length`（按码点，中文 1 字） |
  | 🔍 查找/替换 | 顶部展开查找条：输入框 + 命中数 + 上一个/下一个 + 替换框 + 替换/全部替换；用 `setSelectionRange` 高亮+滚动 |
  | ⌫ 清空 | 清空 textarea（二次确认） |
  | 保存 / 取消 | 见 §3.3 事件 |

- 快捷键：`Cmd/Ctrl+Enter` 保存、`Esc` 取消、`Cmd/Ctrl+F` 唤出查找。
- mount：`invoke("get_pending_compact_edit")` → 有则载入 `{text, requestId}` 并 focus textarea；同时 `listen("compact-editor://load")` 处理并发再开。
- unmount：兜底 `emit("compact-editor://cancel", { requestId })`（已保存则不发；用 ref 标记 saved 状态区分）。
- 关闭按钮走「取消」语义（X 关 = 不保存）。

### 3.5 三处集成

**① 语音 Result（`pages/Result/index.tsx`）—— 编辑框尺寸双模式（CSS 伪装，不弹独立窗）**

物理窗口固定 **720×480**（`result_window`，`resizable(true)` / `decorations(false)` / `transparent` / `always_on_top`，创建即定死尺寸），前端按模式用 CSS 切「可见容器」尺寸，**全程不调 setSize**——透明无边框悬浮窗上 `setSize`/`setFrame` 被 NSWindow 拒绝（min/max 放宽到 [100,4000]、720×480 在区间内仍读回旧值，实锤），故改 CSS 伪装 + 透明区点击穿透：

| 维度 | 精简态（默认） | 长篇态 |
|---|---|---|
| 物理窗口 | 720×480（固定） | 720×480（同左） |
| 可见容器（CSS） | 顶部居中 520×116 小条 | 撑满 720×480 |
| 文本区 | `max-h-[63px]` | `h-full` 撑满 |
| 透明区 | 小条下方透明（`body{background:transparent}`） | 无 |
| 点击穿透 | 透明区穿透到后方应用 | 整窗可交互 |

- 外层透明包裹 `relative w-full h-full`，内层 `#result-container` 绝对定位 `top-0 left-1/2 -translate-x-1/2`，className 按 `expanded` 切 `w-[720px] h-[480px]`（长篇）或 `w-[520px] h-[116px]`（精简），加 `transition-all duration-200`；`visible` 控 `opacity-0/100` 显隐。
- 工具栏「放大/缩小」开关按钮：精简态「放大」（`expand-edit` 四角向外）→ 长篇；长篇态「缩小」（`minimize` 四角向内）→ 精简。
- `toggleExpand`（纯 CSS，无 setSize）：`setExpanded(next)` + `invoke("set_result_click_through", { expanded: next })`。
- **点击穿透（必须 Rust 轮询）**：精简态小条下方透明区要穿透到后方应用。前端 `setIgnoreCursorEvents(true)` 不可行——一旦 ignore，NSWindow 零鼠标事件、连 tracking area 都禁，前端检测不到光标重新进入小条 → 重入失效。故 `result_window.rs::start_click_through_poller` 后台线程（~33ms）读全局鼠标 `CGEvent.location()`，按窗口 `outer_position()`/`scale_factor` 算小条屏幕矩形，光标在矩形外 → `setIgnoresMouseEvents(true)` 穿透、在内 → `false` 可交互；直调 NSWindow `setIgnoresMouseEvents`（比 Tauri `set_ignore_cursor_events` 封装可靠，复用 `screenshot_commands` 同款）。切长篇时前端调 `set_result_click_through(expanded=true)` 立即关穿透。
- 编辑**完全不变**：仍走现有 `toggleEdit`（`contentEditable` + `enter_edit_mode`/`commit_edit`/`cancel_edit`），两种模式下均可编辑，零新编辑逻辑。
- **移除**原 `openExpandEdit` / `applyResultText` 与 `openCompactEditor` import——Result 不再依赖精简编辑器窗口。
- **移除**「存入记事本」工具按钮 + `saveToNote` 回调——长篇模式已可直接原地大窗口编辑，无需导入记事本（后端 `save_transcription_to_note`/`current_transcription_id` 命令保留作基础设施）。
- Rust 侧 `result_window.rs` 创建 `.resizable(true)` + 固定 `.inner_size(720,480)`；移除 `set_result_window_mode` 命令，新增 `set_result_click_through`。窗口位置仍由 `window_position` 机制保存恢复。
- 边界：长篇态向下展开占满 720×480，若原位置近屏幕底部可能部分超出——MVP 不重算位置（用户可拖动窗口），e2e 观察。

**② OCR（`clipboard_commands.rs::ocr_image` + `ClipboardItem.tsx::handleOcr`）**
- 后端：`ocr_image` **删除** `open_text_editor_with_content(&text)` 调用（不再打开系统 TextEdit）；`open_text_editor_with_content` 函数本身若仅此处引用则一并删除。`ocr_image` 仍返回 `text`（前端拿到）。
- 前端 `handleOcr`：OCR 成功后生成 `requestId` → `invoke("open_compact_editor", { initialText: text(需 ocr_image 返回值), requestId })` → `listen` 按 rid 过滤 → 命中 `invoke("set_clipboard_item_text", { itemId: item.id, text })` + `onChanged()` 刷新列表。
  - 注意：当前 `handleOcr` 调 `ocr_image` 未取返回值，需改为 `const text = await invoke<string>("ocr_image", { id })`。
- `update_search_text`（识别后落 search_text）+ `handle.write_text`（写系统剪贴板）保留——这些是「识别结果」的落库与剪贴板同步，编辑器只负责让用户随后修改文本。

**③ 剪贴板文本条目（`ClipboardItem.tsx`）**
- hover 操作区新增「编辑」按钮（lucide `SquarePen`/`Pencil`，挨着「存入记事本」`NotebookPen`）。
- 显示条件：`item.item_type !== "image" && item.item_type !== "file"`（即文本/语音文本可编辑；图片走 OCR、文件不可编辑）。
- 点击 → 生成 `requestId` → `invoke("open_compact_editor", { initialText: item.content, requestId })` → `listen` 按 rid 过滤 → 命中 `invoke("set_clipboard_item_text", { itemId: item.id, text })` + `onChanged()`。

**共享：`set_clipboard_item_text` 命令（②③共用）**
- 新增 `#[tauri::command] set_clipboard_item_text(item_id: i64, text: String, handle: State<ClipboardHandle>)` 于 `clipboard_commands.rs`：
  1. `octopus_clipboard::store::update_content(conn, item_id, &text)`（新增 store 函数，镜像现有 `update_search_text`，同时写 `clipboard_history.content` 与 `search_text`）。
  2. `handle.write_text(&text)` 同步系统剪贴板。
  3. 注册到 `generate_handler!`。

## 4. 数据流图

```
调用方(OCR/Clipboard)
  │ requestId = uuid()
  │ invoke open_compact_editor(initialText, requestId)
  ▼
compact_editor_commands::open_compact_editor
  │ PENDING = {text, requestId}
  │ 建窗(首次) 或 emit load + focus(已存在)
  ▼
CompactEditor mount  ──get_pending_compact_edit──► PENDING.take()
  │ 用户编辑 textarea（撤销/重做/字号/查找替换/清空）
  │ 保存: emit("compact-editor://result", {requestId, text}) + close
  ▼
调用方 listen(result) ──rid 命中──► 应用文本
  • OCR/Clipboard → set_clipboard_item_text(itemId, text)

（语音 Result 不走此流——原地双模式编辑，见 §3.5①）
```

## 5. 错误处理与边界

| 场景 | 处理 |
|---|---|
| X 关窗 / unmount | emit cancel（rid 兜底），调用方清 pending 监听，不应用 |
| 并发再开（A 开着，B 再开） | 后端 emit load 推 B 的 {text,rid}；A 的 rid ≠ B 的 rid，A 的 listener 不命中 → A 不应用（无害） |
| 空文本 | textarea 正常，保存返回空字符串，调用方按需处理（Result/Clipboard 接受空） |
| 超长 OCR 文本 | textarea 原生滚动 + 字数提示，无上限 |
| 中文输入法 | textarea 原生 IME 安全（优于 contentEditable/TipTap） |
| 编辑器窗口被系统关闭（非取消按钮） | unmount 兜底 emit cancel |
| OCR 未识别到文本 | `ocr_image` 现已 `Err`，前端 catch 走原 `ocrDone` 提示，不开编辑器 |

## 6. 测试

**后端单测（`compact_editor_commands.rs`）：**
- `open_compact_editor` 写 PENDING → `get_pending_compact_edit` 读回正确 `{text, requestId}` 并 take 清空。窗口创建本身是 Tauri 集成层，不单测。
- `set_clipboard_item_text`：调用后 `clipboard_history.content` 与 `search_text` 均更新（用 `_at` 内存 DB 变体，镜像现有剪贴板 store 测试）。
- `update_content` store 函数单测（`crates/clipboard`）。

**前端单测（CompactEditor）：**
- 字符计数、字号增减、查找/替换匹配与高亮逻辑（纯函数抽离可单测）。
- 保存/取消 emit 事件（mock `invoke`/`emit`，断言 payload `{requestId, text}`）。

**e2e（手动，跨窗口+IME，单测覆盖不到）：**
1. Result（双模式）：识别中文 → 点「放大」切长篇 → 可见容器撑满 720×480、编辑区撑满 → 编辑 → 保存 → 文本落库 → 点「缩小」切回 520×116 小条；精简态小条下方透明区可点击穿透到后方应用。
2. OCR：图片识别 → 自动开编辑器 → 改文本 → 保存 → 确认剪贴板条目内容 + 系统剪贴板均更新。
3. 剪贴板文本条目：点「编辑」→ 改 → 保存 → 确认列表与系统剪贴板更新。
4. 边界：取消 / X 关窗不应用文本；并发开窗不串扰。

## 7. 文档同步

- `docs/architecture.md`：crate 树与窗口列表新增 `compact_editor_window`；命令清单加 `open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor` / `set_clipboard_item_text`。
- 本 spec → `docs/superpowers/plans/2026-06-30-compact-editor.md`（writing-plans 产出）。

## 8. 与完整版记事本的关系

- 完整版 notepad_window 保留不动（未来需要标题/分类/富文本/持久化时用它）。
- 精简编辑器是「纯编辑工具」，与 notepad 不共享状态、不共享窗口、不互调。
- 唯一共享资源是 `image_data`（已由 C1 修复保证 note-img 引用不被剪贴板清理误删）；精简编辑器只处理纯文本，不涉及图片。
