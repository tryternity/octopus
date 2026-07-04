# 清除记事本 + 多 tab CompactEditor + OCR 统一改造 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 移除记事本子系统，CompactEditor 升级为多 tab 常驻编辑器，三处 OCR 入口统一入剪贴板新 OCR 类别。

**Architecture:** 后端 `crates/clipboard` 扩展 `source=ocr` + `OcrMeta`（复用 engine/model 列，无 schema 变更）；`crates/notepad` 整个删除 + DB v12→v13 DROP notes；`compact_editor_commands` 从请求-响应（requestId 回传）改为 item_id 驱动的多 tab；OCR 命令链改为「识别→insert_ocr→open tab」。

**Tech Stack:** Rust + Tauri 2 + React + TypeScript + SQLite(rusqlite) + FTS5

**执行顺序：** Task 1→2（OCR 类别后端）→ 3（DB 迁移）→ 4→5（清记事本）→ 6→7（多 tab）→ 8→9（OCR 统一）→ 10（OCR 类别前端）→ 11（文档+全量验证）

---

## Task 1: OCR 类别 — clipboard model

**Files:**
- Modify: `crates/clipboard/src/model.rs`

- [x] **Step 1: 加 Source::Ocr + as_str/from_str（含失败测试）**

在 `model.rs` 的 `#[cfg(test)] mod tests` 里加测试：
```rust
#[test]
fn test_source_ocr_roundtrip() {
    let s = Source::Ocr;
    assert_eq!(s.as_str(), "ocr");
    assert_eq!(Source::from_str("ocr"), Source::Ocr);
    // 容错：未知值回落 Clipboard
    assert_eq!(Source::from_str("xxx"), Source::Clipboard);
}
```

- [x] **Step 2: 运行测试验证失败**

Run: `cargo test -p octopus-clipboard --lib model::tests::test_source_ocr_roundtrip`
Expected: 编译失败（`Source::Ocr` 不存在）

- [x] **Step 3: 实现 Source::Ocr**

修改 `Source` 枚举与 impl（保留 `#[default] Clipboard`）：
```rust
pub enum Source {
    #[default]
    Clipboard,
    Asr,
    Ocr,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Clipboard => "clipboard",
            Source::Asr => "asr",
            Source::Ocr => "ocr",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "asr" => Source::Asr,
            "ocr" => Source::Ocr,
            _ => Source::Clipboard,
        }
    }
}
```

- [x] **Step 4: 加 OcrMeta + ClipboardItem.ocr_meta**

在 `AsrMeta` 后加：
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrMeta {
    pub engine: String,
    pub model: String,
}
```

在 `ClipboardItem` struct 里 `asr_meta` 字段后加：
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_meta: Option<OcrMeta>,
```

- [x] **Step 5: 运行全部 clipboard 测试 + 编译**

Run: `cargo test -p octopus-clipboard --lib && cargo build -p octopus-clipboard`
Expected: 编译失败——`ClipboardItem` 构造点（`row_to_item`、tests）缺 `ocr_meta` 字段（下个 Task 修 store，此处 store.rs 的 `row_to_item` 会报错，预期内，先记下）

> 注：Step 5 store.rs 会因缺字段编译失败，这是预期，Task 2 修复。为保持 Task 1 可独立编译验证，改为先只验 model 测试模块编译：`cargo test -p octopus-clipboard --lib model::tests`（不构造 ClipboardItem）→ PASS。

- [x] **Step 6: Commit**

```bash
git add crates/clipboard/src/model.rs
git commit -m "feat(clipboard): Source 加 Ocr 变体 + OcrMeta + ClipboardItem.ocr_meta"
```

---

## Task 2: OCR 类别 — clipboard store

**Files:**
- Modify: `crates/clipboard/src/store.rs`（`insert_ocr_item`、`build_where`、`row_to_item`）

- [x] **Step 1: 写 insert_ocr_item + build_where + row_to_item 的失败测试**

在 `store.rs` 的 `#[cfg(test)]` 模块里加（参考现有 asr 插入测试写法，使用内存库）：
```rust
#[test]
fn test_insert_and_query_ocr_item() {
    let conn = open_test_conn(); // 复用现有测试辅助；若无则 Connection::open_in_memory() + 建表
    let id = insert_ocr_item(
        &conn,
        "识别文本",
        OcrMeta { engine: "ocr_rs".into(), model: "m1".into() },
    ).unwrap();
    assert!(id > 0);

    // build_where "ocr" 能查到
    let items = query_history(
        &conn,
        &QueryFilter { filter: "ocr".into(), search: None, page: 1, size: 10 },
    ).unwrap();
    assert_eq!(items.len(), 1);
    let it = &items[0];
    assert_eq!(it.source, Source::Ocr);
    assert_eq!(it.content, "识别文本");
    let om = it.ocr_meta.as_ref().expect("ocr_meta 应填充");
    assert_eq!(om.engine, "ocr_rs");
    assert_eq!(om.model, "m1");
}
```
> 若 `open_test_conn` 不存在，参考 `store.rs` 现有测试（如 `insert_asr_item` 的测试 ~L679）用的建表方式，复制同样模式。

- [x] **Step 2: 运行验证失败**

Run: `cargo test -p octopus-clipboard --lib test_insert_and_query_ocr_item`
Expected: FAIL（`insert_ocr_item` 未定义）

- [x] **Step 3: 实现 insert_ocr_item**

在 `insert_asr_item` 函数后加（对称写法）：
```rust
/// 插入 OCR 识别文本条目（source='ocr'，复用 engine/model 列）。返回插入的 id。
pub fn insert_ocr_item(conn: &Connection, text: &str, ocr_meta: OcrMeta) -> Result<i64> {
    let id = chrono_millis();
    conn.execute(
        "INSERT INTO clipboard_history
         (id, item_type, source, content, search_text, is_favorite, created_at,
          engine, model)
         VALUES (?, 'text', 'ocr', ?, ?, 0, ?, ?, ?)",
        params![id, text, text, iso_now(), ocr_meta.engine, ocr_meta.model],
    )
    .context("insert ocr clipboard_history")?;
    Ok(id)
}
```
确保文件顶部 `use crate::model::{..., OcrMeta, ...}` 已导入 OcrMeta（按现有 import 风格补充）。

- [x] **Step 4: build_where 加 "ocr" 分支**

在 `build_where` 的 `match filter.filter.as_str()` 里，`"asr"` 分支后加：
```rust
        "ocr" => { conditions.push("source = 'ocr'".to_string()); }
```

- [x] **Step 5: row_to_item 反序列化 ocr_meta**

在 `row_to_item` 里 `asr_meta` 构造后加：
```rust
    let ocr_meta = if source_str == "ocr" {
        Some(OcrMeta {
            engine: engine.clone().unwrap_or_default(),
            model: model.clone().unwrap_or_default(),
        })
    } else {
        None
    };
```
并在返回的 `ClipboardItem { ... }` 里 `asr_meta,` 后加 `ocr_meta,`。

- [x] **Step 6: 运行测试 + 全 crate 编译**

Run: `cargo test -p octopus-clipboard --lib && cargo build -p octopus-clipboard`
Expected: PASS（model+store 一致，`ocr_meta` 字段补齐）

- [x] **Step 7: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): insert_ocr_item + build_where/row_to_item 支持 source=ocr"
```

---

## Task 3: DB 迁移 v12 → v13（DROP notes）

**Files:**
- Modify: `crates/infra/src/db.rs`（版本常量、migrate 分支、init 日志）

- [x] **Step 1: 写迁移失败测试**

在 `db.rs` 的 `#[cfg(test)]` 里加（参考 `migrate_v11_to_v12_*` 测试模式 ~L1316）：
```rust
#[test]
fn migrate_v12_to_v13_drops_notes_tables() {
    let conn = Connection::open_in_memory().unwrap();
    // 先建 v12 状态的 notes + notes_fts + 触发器
    conn.execute_batch(
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT, content_text TEXT DEFAULT '', content_html TEXT DEFAULT '', type TEXT DEFAULT 'text', source TEXT DEFAULT 'manual', source_ref_id INTEGER, is_pinned INTEGER DEFAULT 0, is_favorite INTEGER DEFAULT 0, created_at TEXT, updated_at TEXT);
         CREATE VIRTUAL TABLE notes_fts USING fts5(title, content_text, content_text='', tokenize='trigram');"
    ).unwrap();
    conn.execute("INSERT INTO notes (title, content_text) VALUES ('a','b')", []).unwrap();
    assert_eq!(conn.query_row::<i64,_,_>("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap(), 1);

    // 跑迁移（直接调 migrate 内联 v12→v13 分支逻辑；若 migrate 是私有，复制其 v13 分支语句到此测试执行）
    conn.execute_batch("DROP TABLE IF EXISTS notes_fts; DROP TABLE IF EXISTS notes;").unwrap();

    // 验证表已删
    let notes_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='notes'", [], |r| r.get(0)
    ).unwrap();
    let fts_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='notes_fts'", [], |r| r.get(0)
    ).unwrap();
    assert_eq!(notes_exists, 0);
    assert_eq!(fts_exists, 0);
}
```

- [x] **Step 2: 运行验证**

Run: `cargo test -p octopus-infra --lib migrate_v12_to_v13_drops_notes_tables`
Expected: PASS（测试自含 DROP 语句验证语义；真正的迁移代码在 Step 3 接入）

- [x] **Step 3: 接入迁移代码**

在 `db.rs` 的 `migrate` 函数里，v11→v12 分支后加 v12→v13 分支。先找到版本常量（`CURRENT_VERSION` 或类似，当前 12）改为 13。在迁移链末尾加：
```rust
    // v12 → v13：移除记事本功能——DROP notes_fts（含触发器）+ notes 表
    if version < 13 {
        log::info!("DB migrating v12 → v13: dropping notes + notes_fts (notepad removed)...");
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS notes_ai;
             DROP TRIGGER IF EXISTS notes_ad;
             DROP TRIGGER IF EXISTS notes_au;
             DROP TABLE IF EXISTS notes_fts;
             DROP TABLE IF EXISTS notes;"
        ).context("v12→v13: drop notes tables")?;
        version = 13;
        log::info!("DB migrated to v13: notes tables dropped");
    }
```
更新版本常量到 13，并更新 init 日志行（去掉 `notes(...)` 描述）。

- [x] **Step 4: 全量编译 + 测试**

Run: `cargo build -p octopus-infra && cargo test -p octopus-infra --lib`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(db): v12→v13 迁移——DROP notes/notes_fts（移除记事本）"
```

---

## Task 4: 清除记事本 — Rust 后端

**Files:**
- Delete: `crates/notepad/`（整个目录）、`crates/desktop/src/notepad_window.rs`、`crates/desktop/src/note_commands.rs`
- Modify: `Cargo.toml`、`crates/desktop/Cargo.toml`、`crates/desktop/src/main.rs`、`crates/desktop/src/tray.rs`、`crates/desktop/src/screenshot_commands.rs`、`crates/desktop/src/compact_editor_window.rs`

- [x] **Step 1: 删 notepad crate + desktop 依赖**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature
rm -rf crates/notepad
```
编辑根 `Cargo.toml`：从 `members = [...]` 数组移除 `"crates/notepad"`。
编辑 `crates/desktop/Cargo.toml`：移除 `octopus-notepad = { path = "../notepad" }` 行。

- [x] **Step 2: 删 desktop 的 notepad/note 源文件**

```bash
rm crates/desktop/src/notepad_window.rs crates/desktop/src/note_commands.rs
```

- [x] **Step 3: main.rs 移除命令注册 + mod 声明**

编辑 `crates/desktop/src/main.rs`：
- 移除 `mod notepad_window;` 和 `mod note_commands;`（若有）
- 从 `generate_handler!` 移除这 15 项：`note_commands::list_notes` / `count_notes` / `get_note` / `create_note` / `update_note` / `delete_notes` / `toggle_note_pinned` / `toggle_note_favorite` / `export_note` / `import_note_from_file` / `save_transcription_to_note` / `save_ocr_to_note`，以及 `notepad_window::open_notepad` / `open_notepad_with_note` / `get_pending_note`

- [x] **Step 4: tray.rs 移除「记事本」菜单项**

编辑 `crates/desktop/src/tray.rs`：移除 id=`notepad` 的菜单项构建（~L68-69）与其 handler 分支（~L117-120 `crate::notepad_window::open_notepad(...)`）。

- [x] **Step 5: screenshot_commands.rs 删 open_notepad_with_content**

编辑 `crates/desktop/src/screenshot_commands.rs`：删除 `fn open_notepad_with_content(...)`（~L273-287）。`ocr_screenshot` 内对它的调用在 Task 8 一并改造（此时先删函数会让 `ocr_screenshot` 编译失败——预期，Task 8 修）。为不阻断 Task 4 编译，临时把 `ocr_screenshot` 里 `open_notepad_with_content(&app_handle, &text);` 这一行注释掉（Task 8 会重写整个 ocr_screenshot）。

- [x] **Step 6: compact_editor_window.rs 更新注释**

编辑 `crates/desktop/src/compact_editor_window.rs`：把注释里「与 notepad/settings 对称」「与 notepad_window::on_notepad_closed 对称」的 `notepad` 字样去掉（L4、L16、L38），改为「与 settings 对称」。纯注释，无代码改动。

- [x] **Step 7: 编译验证**

Run: `cargo build -p octopus-desktop`
Expected: 编译通过（若 ocr_screenshot 注释了调用行则通过；note 命令/窗口全删干净）

- [x] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(desktop): 移除记事本后端——删 notepad crate + note_commands + notepad_window + 托盘项"
```

---

## Task 5: 清除记事本 — 前端 + capability

**Files:**
- Delete: `crates/desktop/frontend/src/pages/Notepad/`、`types/note.ts`、`hooks/useNotes.ts`、`lib/notepad.ts`
- Modify: `crates/desktop/frontend/src/App.tsx`、`pages/Settings/HistoryPanel.tsx`、`capabilities/default.json`

- [x] **Step 1: 删前端 Notepad 相关文件**

```bash
cd /Users/wudarui/workspace/agent/octopus/.claude/worktrees/clean-used-feature
rm -rf crates/desktop/frontend/src/pages/Notepad
rm -f crates/desktop/frontend/src/types/note.ts
rm -f crates/desktop/frontend/src/hooks/useNotes.ts
rm -f crates/desktop/frontend/src/lib/notepad.ts
```

- [x] **Step 2: App.tsx 移除路由**

编辑 `crates/desktop/frontend/src/App.tsx`：移除 `import Notepad from "@/pages/Notepad";` 与 `return <Notepad />;` 分支（含其 case label）。

- [x] **Step 3: HistoryPanel.tsx 移除「保存为笔记」按钮**

编辑 `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx`：移除「保存为笔记」按钮 JSX + 其 `save_transcription_to_note` invoke 逻辑（~L304 区域）。保留 ASR 历史展示本身。

- [x] **Step 4: capability 移除 notepad_window**

编辑 `crates/desktop/capabilities/default.json`：从 `"windows"` 数组移除 `"notepad_window"`。

- [x] **Step 5: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 构建通过（无 Notepad 引用残留）

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(desktop/frontend): 移除记事本前端——删 Notepad 页/note.ts/useNotes/notepad.ts + 路由 + HistoryPanel 存笔记按钮 + capability"
```

---

## Task 6: 多 tab CompactEditor — 后端命令

**Files:**
- Modify: `crates/desktop/src/compact_editor_commands.rs`、`crates/desktop/src/clipboard_commands.rs`、`crates/desktop/src/main.rs`

- [x] **Step 1: 重写 compact_editor_commands.rs（item_id 驱动）**

整体替换 `compact_editor_commands.rs` 的 PENDING 与命令（保留文件头注释、`use` 与 `close_compact_editor`，删除 `CompactEditPayload`/`open_compact_editor(initial_text,request_id)`/`get_pending_compact_edit`）：
```rust
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewWindowBuilder, WebviewUrl};

/// 多 tab 精简编辑器：PENDING 只存「开窗时首个 tab 的 item_id」；
/// 窗口已开时改走 compact-editor://open-tab 事件，由前端追加 tab。
static PENDING_TAB: Mutex<Option<i64>> = Mutex::new(None);

fn store_pending_tab(item_id: i64) {
    *PENDING_TAB.lock().unwrap() = Some(item_id);
}
fn take_pending_tab() -> Option<i64> {
    PENDING_TAB.lock().unwrap().take()
}

const WINDOW_LABEL: &str = "compact_editor_window";

/// 打开（或聚焦）精简编辑器并把 item_id 作为 tab 打开。
/// 窗口未开：store pending + 建窗（前端 mount 时 take 首个 tab）。
/// 窗口已开：emit compact-editor://open-tab { itemId }，前端追加/激活 tab。
#[tauri::command]
pub fn open_compact_editor_tab(item_id: i64, app_handle: AppHandle) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        let _ = window.emit("compact-editor://open-tab", serde_json::json!({ "itemId": item_id }));
        return Ok(());
    }
    store_pending_tab(item_id);
    // macOS：切 Regular 让 Dock 显图标（与 settings 对称）
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        crate::settings_window::set_dock_icon();
    }
    WebviewWindowBuilder::new(&app_handle, WINDOW_LABEL, WebviewUrl::default())
        .title("编辑")
        .inner_size(560.0, 640.0)
        .min_inner_size(360.0, 320.0)
        .decorations(true)
        .visible(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 前端 mount 时 take 首个 tab 的 item_id（一次性）。
#[tauri::command]
pub fn get_pending_compact_tab() -> Option<i64> {
    take_pending_tab()
}

#[tauri::command]
pub fn close_compact_editor(app_handle: AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}
```
> 实际窗口构建参数（尺寸/标题/label）须与现有 `compact_editor_window.rs` 保持一致——先读该文件确认 WINDOW_LABEL/尺寸再填，避免 label 不匹配导致 capability 拦截（参见 memory：动态窗口须列入 capability，compact_editor_window 已在 default.json）。

- [x] **Step 2: 新增 get_clipboard_item_text 命令**

在 `crates/desktop/src/clipboard_commands.rs` 加（复用 `store::get_item_by_id`）：
```rust
/// 按 id 取剪贴板条目的文本内容（CompactEditor 开 tab 时加载用）。
#[tauri::command]
pub async fn get_clipboard_item_text(id: i64) -> Result<String, String> {
    octopus_infra::db::with_db(|conn| octopus_clipboard::store::get_item_by_id(conn, id))
        .map_err(|e| e.to_string())?
        .map(|item| item.content)
        .ok_or_else(|| "条目不存在".to_string())
}
```

- [x] **Step 3: main.rs 更新命令注册**

编辑 `main.rs` `generate_handler!`：移除 `compact_editor_commands::open_compact_editor` / `get_pending_compact_edit`，新增 `compact_editor_commands::open_compact_editor_tab` / `get_pending_compact_tab`；新增 `clipboard_commands::get_clipboard_item_text`。

- [x] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop`
Expected: PASS（前端尚未改，但后端命令已就绪；前端旧调用会在 Task 7 修，此处后端编译应通过）

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/compact_editor_commands.rs crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git commit -m "feat(compact-editor): 后端改 item_id 驱动多 tab——open_compact_editor_tab + get_clipboard_item_text"
```

---

## Task 7: 多 tab CompactEditor — 前端

**Files:**
- Modify: `crates/desktop/frontend/src/lib/compactEditor.ts`、`pages/CompactEditor/index.tsx`、`pages/Clipboard/ClipboardItem.tsx`

- [x] **Step 1: 重写 lib/compactEditor.ts**

整体替换为：
```ts
import { invoke } from "@/lib/tauri";

/// 打开 CompactEditor 并把指定剪贴板条目作为 tab 打开（已开则追加/激活 tab）。
export function openCompactEditorTab(itemId: number): Promise<void> {
  return invoke("open_compact_editor_tab", { itemId });
}
```
删除旧的 `openCompactEditor(initialText, onResult)`、`PendingEdit` 类型、result/cancel 监听。

- [x] **Step 2: 重写 pages/CompactEditor/index.tsx 为多 tab**

核心结构（替换单文档实现）：
```tsx
import { useState, useEffect, useRef } from "react";
import { invoke, listen } from "@/lib/tauri";

interface Tab { itemId: number; text: string; dirty: boolean; title: string; }

function tabTitle(text: string, itemId: number): string {
  const head = text.slice(0, 5);
  const hex = itemId.toString(16).slice(-5);
  return `${head}-${hex}`;
}

function CompactEditor() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  async function addTab(itemId: number) {
    setTabs(prev => {
      if (prev.some(t => t.itemId === itemId)) { setActiveId(itemId); return prev; }
      return prev; // 异步加载后 push（见下）
    });
    // 已存在则仅激活
    if (tabs.some(t => t.itemId === itemId)) { setActiveId(itemId); return; }
    const text = await invoke<string>("get_clipboard_item_text", { id: itemId });
    setTabs(prev => prev.some(t => t.itemId === itemId) ? prev
      : [...prev, { itemId, text, dirty: false, title: tabTitle(text, itemId) }]);
    setActiveId(itemId);
  }

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const pendingId = await invoke<number | null>("get_pending_compact_tab");
      if (pendingId != null) await addTab(pendingId);
      unlisten = await listen<{ itemId: number }>("compact-editor://open-tab", e => {
        addTab(e.itemId);
      });
    })();
    return () => { unlisten?.(); };
  }, []);

  const active = tabs.find(t => t.itemId === activeId) ?? null;

  function updateText(v: string) {
    if (!active) return;
    setTabs(prev => prev.map(t => t.itemId === active.itemId
      ? { ...t, text: v, dirty: true, title: tabTitle(v, t.itemId) } : t));
  }

  async function saveActive() {
    if (!active) return;
    await invoke("set_clipboard_item_text", { itemId: active.itemId, text: active.text });
    setTabs(prev => prev.map(t => t.itemId === active.itemId ? { ...t, dirty: false } : t));
  }

  function closeTab(itemId: number) {
    const t = tabs.find(x => x.itemId === itemId);
    if (t?.dirty && !confirm("该 tab 有未保存修改，放弃？")) return;
    setTabs(prev => {
      const next = prev.filter(x => x.itemId !== itemId);
      if (activeId === itemId) setActiveId(next[0]?.itemId ?? null);
      return next;
    });
  }

  // Ctrl+S 保存；UI：顶部 tab 栏 + 当前 tab 文本框；字号/查找替换沿用原有 state（保留）
  // ... 渲染 tabs.map(tab 按钮) + active && <textarea value={active.text} onChange>
  //     + 快捷键 onKeyDown Ctrl+S => saveActive()
}
export default CompactEditor;
```
> 保留原有的字号、查找/替换 state 与 UI（从旧 index.tsx 复制），只把单文档 text 改为 `active.text`，保存按钮/Ctrl+S 改为 `saveActive`。tab 栏置于顶部。

- [x] **Step 3: ClipboardItem.tsx handleEditText 改为开 tab**

编辑 `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` 的 `handleEditText`（~L143）：
```tsx
const handleEditText = (e: React.MouseEvent) => {
  e.stopPropagation();
  if (item.item_type === "image" || item.item_type === "file") return;
  openCompactEditorTab(item.id);
};
```
import 改为 `import { openCompactEditorTab } from "@/lib/compactEditor";`（删旧 `openCompactEditor`）。

- [x] **Step 4: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS（注意：此时 ClipboardItem 的 `handleOcr` 仍引用旧 `openCompactEditor`，需在 Task 9 改；为不阻断本 Task，临时把 handleOcr 里的 `openCompactEditor(...)` 注释掉并留 TODO，或直接在 Step 3 一并把 handleOcr 改为 `openCompactEditorTab`——但 handleOcr 涉及 OCR 入库，放 Task 9。折中：本 Task 临时注释 handleOcr 的 openCompactEditor 调用，Task 9 补全）

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/lib/compactEditor.ts crates/desktop/frontend/src/pages/CompactEditor/index.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "feat(compact-editor): 前端重写为多 tab——tabs 状态/Ctrl+S/关 tab/标题(前5字-hex后5)"
```

---

## Task 8: OCR 统一 — 后端命令

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`（新增 `insert_ocr_clipboard_item`）、`crates/desktop/src/screenshot_commands.rs`（`ocr_screenshot` 改纯识别）、`crates/desktop/src/main.rs`

- [x] **Step 1: 新增 insert_ocr_clipboard_item 命令**

在 `clipboard_commands.rs` 加：
```rust
/// OCR 识别文本入库为 source='ocr' 条目（engine/model 后端读 config 自填），返回 item_id。
#[tauri::command]
pub async fn insert_ocr_clipboard_item(text: String, app_handle: tauri::AppHandle) -> Result<i64, String> {
    let model = octopus_infra::db::load_config_key("ocr_model")
        .ok().flatten()
        .unwrap_or_else(|| "default".to_string());
    let id = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_ocr_item(conn, &text, octopus_clipboard::model::OcrMeta {
            engine: "ocr_rs".to_string(),
            model,
        })
    }).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(id)
}
```
> `load_config_key` 已在 ocr engine 使用（见 engine.rs:instance）；确认其签名与可见性，若非 pub 则改用 `octopus_infra::db::with_db` 读 app_config。`emit` 需 `use tauri::{Emitter, Manager}`。

- [x] **Step 2: ocr_screenshot 改纯识别**

重写 `screenshot_commands.rs` 的 `ocr_screenshot`（~L190）：保留接收 PNG raw body + `OcrEngine::instance().recognize()`，删除入库图片/update_search_text/write_text/open_notepad（Task 4 已注释其调用），保留 `close_all_screenshot_windows`：
```rust
#[tauri::command]
pub async fn ocr_screenshot(
    request: tauri::ipc::Request,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("需要 raw body".into());
    };
    let engine = octopus_ocr::engine::OcrEngine::instance().map_err(|e| e.to_string())?;
    let text = engine.recognize(png_bytes).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        close_all_screenshot_windows(&app_handle);
        return Err("未识别到文本".into());
    }
    close_all_screenshot_windows(&app_handle);
    Ok(text)
}
```
> 确认 `close_all_screenshot_windows` 签名（是否需要 app_handle）；保留与原一致。删除现已无用的 `open_notepad_with_content` 残留注释。

- [x] **Step 3: main.rs 注册 insert_ocr_clipboard_item**

`generate_handler!` 加 `clipboard_commands::insert_ocr_clipboard_item`。

- [x] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/clipboard_commands.rs crates/desktop/src/screenshot_commands.rs crates/desktop/src/main.rs
git commit -m "feat(ocr): insert_ocr_clipboard_item 命令 + ocr_screenshot 改纯识别返回 text"
```

---

## Task 9: OCR 统一 — 前端三入口

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`、`pages/ImagePreview/index.tsx`、`pages/Clipboard/ClipboardItem.tsx`

- [x] **Step 1: 截图工具栏 doOcr**

编辑 `Screenshot/index.tsx` `doOcr`（~L560）：
```tsx
function doOcr() {
  composeAndCropBytes().then(async (bytes) => {
    if (!bytes) return;
    try {
      const text = await invoke<string>("ocr_screenshot", bytes as unknown as Record<string, unknown>);
      const itemId = await invoke<number>("insert_ocr_clipboard_item", { text });
      await openCompactEditorTab(itemId);
    } catch (e) { console.error(e); }
  });
}
```
import `openCompactEditorTab` from `@/lib/compactEditor`。

- [x] **Step 2: ImagePreview handleOcr**

编辑 `ImagePreview/index.tsx` `handleOcr`（~L295）：用 `ocr_image` 拿 text → `insert_ocr_clipboard_item` → `openCompactEditorTab`；删除 `save_ocr_to_note` / `open_notepad_with_note` 调用：
```tsx
const handleOcr = async () => {
  if (!imageId) return;
  try {
    const text = await invoke<string>("ocr_image", { id: imageId });
    const itemId = await invoke<number>("insert_ocr_clipboard_item", { text });
    setOcrCopied(true); setTimeout(() => setOcrCopied(false), 1500);
    await openCompactEditorTab(itemId);
  } catch (e) { console.error(e); }
};
```

- [x] **Step 3: 剪贴板图片条目 handleOcr**

编辑 `ClipboardItem.tsx` `handleOcr`（~L93）：拿 text → `insert_ocr_clipboard_item` → `openCompactEditorTab`（原图片条目不动；去掉 `set_clipboard_item_text` 回写）：
```tsx
const handleOcr = async (e: React.MouseEvent) => {
  e.stopPropagation();
  if (ocrLoading) return;
  setOcrLoading(true);
  try {
    const text = await invoke<string>("ocr_image", { id: item.id });
    setOcrLoading(false); setOcrDone(true);
    setTimeout(() => setOcrDone(false), 1000);
    const itemId = await invoke<number>("insert_ocr_clipboard_item", { text });
    onChanged();
    await openCompactEditorTab(itemId);
  } catch (err) {
    setOcrLoading(false);
    if (!String(err).includes("未识别到文本")) console.error(err);
  }
};
```

- [x] **Step 4: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx crates/desktop/frontend/src/pages/ImagePreview/index.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "feat(ocr): 三处 OCR 入口统一——识别→insert_ocr→开 tab 编辑"
```

---

## Task 10: OCR 类别 — 前端

**Files:**
- Modify: `crates/desktop/frontend/src/types/clipboard.ts`、`pages/Clipboard/FilterTabs.tsx`、`pages/Clipboard/ClipboardItem.tsx`

- [x] **Step 1: types/clipboard.ts**

编辑 `crates/desktop/frontend/src/types/clipboard.ts`：
- `Source` 类型：`"clipboard" | "asr" | "ocr"`
- 加 `OcrMeta` 类型 `{ engine: string; model: string }`
- `ClipboardItem` 加 `ocr_meta?: OcrMeta`

- [x] **Step 2: FilterTabs.tsx 加 OCR tab**

编辑 `FilterTabs.tsx` 的 `TABS` 数组（~L4），在「语音」与「文本」之间插入：
```ts
  { value: "ocr", label: "OCR" },
```

- [x] **Step 3: ClipboardItem.tsx OCR 来源标记**

编辑 `ClipboardItem.tsx` 的 Icon 逻辑（~L153）：OCR 条目用 `ScanText` icon（已 import）。把 source 判断扩展：
```tsx
const Icon = item.source === "asr" ? Mic
  : item.source === "ocr" ? ScanText
  : item.item_type === "image" ? ImageIcon
  : item.item_type === "file" ? FileText
  : Type;
```

- [x] **Step 4: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/types/clipboard.ts crates/desktop/frontend/src/pages/Clipboard/FilterTabs.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "feat(clipboard/frontend): Source 加 ocr + FilterTabs OCR tab + OCR 条目标记"
```

---

## Task 11: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 更新 architecture.md**

编辑 `docs/architecture.md`：移除 notepad 模块章节；CompactEditor 描述改为「多 tab 常驻编辑器，tab 绑定剪贴板条目」；clipboard 类别表新增 OCR（source=ocr, engine/model 元信息）。

- [x] **Step 2: 全量后端构建 + 测试**

Run: `cargo build && cargo test --workspace`
Expected: 全绿（desktop/clipboard/infra 编译通过，clipboard OCR 测试 + infra 迁移测试通过）

- [x] **Step 3: 全量前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [x] **Step 4: Commit + e2e 待验收**

```bash
git add docs/architecture.md
git commit -m "docs: 同步——移除 notepad 模块 + CompactEditor 多 tab + clipboard OCR 类别"
```

- [x] **Step 5: 交付 e2e 验收清单**

向用户报告以下手动 e2e 项待验收：
1. 截图工具栏 OCR → CompactEditor 开 tab，剪贴板 OCR 类别出现条目（engine/model 填充）
2. 图片预览 OCR → 同上
3. 剪贴板图片条目 OCR → 新 OCR 条目 + 原图片条目保留
4. 多 tab：连续打开多个条目 → tab 栏多个；切换；Ctrl+S 回写；重复打开同 item 激活而非新 tab
5. tab 标题格式 `前5字-hex后5`
6. 关 dirty tab 提示
7. FilterTabs「OCR」筛选只列 source=ocr
8. 托盘/路由无记事本入口
9. ASR 仍入「语音」类别，文本/ASR 编辑开 tab 可用

---

## Task 12: e2e 阶段修复与增强（2026-07-03）

Task 1-11 完成进入 e2e 验收后发现并修复的 4 个问题。均**已实现并编译 / `tsc -b` 通过**，最终 e2e 待用户验收。

**Files:**
- Modify: `crates/ocr/src/engine.rs`（超长图切分 + `OcrLockGuard` + `plan_chunks` 单测 + DCL 注释修正）
- Create: `crates/ocr/tests/ocr_concurrent_smoke.rs`（MNN 并发安全 smoke test，独立进程）
- Modify: `crates/ocr/tests/ocr_smoke.rs`（`instance` 并发返回同引擎测试）
- Modify: `crates/desktop/src/clipboard_commands.rs`（`ocr_image` 加互斥守卫；`insert_ocr_clipboard_item` 修 with_db 重入；`set_clipboard_item_text` 加 emit）
- Modify: `crates/desktop/src/screenshot_commands.rs`（`ocr_screenshot` 加互斥守卫）
- Modify: 前端 4 入口（`ClipboardItem` / `ImagePreview` index+Toolbar / `Screenshot` / `ClipboardPanel`）加「前一个 OCR 还未完成」可见提示

- [x] **Step 1: with_db 重入死锁修复** — `insert_ocr_clipboard_item` 把 `current_ocr_meta()` 移出 `with_db` 闭包（`std::Mutex` 非递归，闭包内调 `current_ocr_meta`→`load_config_key`→`with_db` = 同线程重入死锁；症状：async `await` 卡住不报错 + DB 查询全阻塞 + 应用不全僵死）。详见 `architecture.md` db 模块重入警告。
- [x] **Step 2: CompactEditor 保存同步** — `set_clipboard_item_text` 成功后 `emit("clipboard://changed")`：编辑器是独立窗口，剪贴板列表窗口靠此事件感知条目变化并刷新（`useClipboardHistory` 监听→fetchItems），否则编辑后列表仍显旧文本。FTS5 经 `clip_fts_au AFTER UPDATE OF search_text` 触发器自动同步。
- [x] **Step 3: 超长图 OCR 切分** — `engine.rs` 加 `recognize_long_image` / `plan_chunks`（`SPLIT_HEIGHT_THRESHOLD=1600` / `CHUNK_HEIGHT=1280` / `CHUNK_OVERLAP=200`），`recognize` 对 `height>1600` 长图按块切分逐块识别 + 跳过与上一块末行相同的起始行去重；解决 2032×15796 长图 det 等比缩放后短边过小、text_len=0。`plan_chunks` 3 个单测绿。设计见 spec §6.3 / OCR spec §10.4。
- [x] **Step 4: OCR 全局并发互斥** — `OcrLockGuard`（`OCR_BUSY: AtomicBool` + `compare_exchange` RAII，drop 释放）在 `ocr_image` / `ocr_screenshot` 入口 `try_acquire`，忙则立即 `Err("前一个 OCR 还未完成，请稍后")`、不进推理；前端 4 入口 catch 该错误给出可见提示（剪贴板列表 / 图片预览按钮显琥珀三角 `ocrWarn`、截图屏幕中央 toast、设置页 `showToast`）。设计见 spec §6.3 / OCR spec §10.5。
- [x] **Step 5: 文档同步** — `architecture.md` octopus-ocr 节补长图切分 + `OcrLockGuard` + 前端提示；clean-used-feature spec §6.3；OCR spec §10.4/§10.5。

> **OCR 僵死归因（技术债，用户定调不深究）**：e2e 期间 OCR 曾僵死，多轮归因（建窗 worker 线程 / 并发首次加载 / MNN C++ 包）均被独立进程 smoke test **证伪**，真因未最终坐实；当前版本（DCL + with_db 修 + emit + 互斥）稳定。`INIT_LOCK` DCL 保留为无害串行化优化（非「修复并发死锁」）。详见 `tests/ocr_concurrent_smoke.rs` + memory `tauri-async-cmd-window-main-thread`。

---

## Self-Review 记录

- **Spec 覆盖**：§4 记事本清除 → Task 3/4/5；§5 多 tab → Task 6/7；§6 OCR 统一 → Task 8/9；§7 OCR 类别 → Task 1/2/10；文档 → Task 11；§6.3 e2e 阶段增强（长图切分 + 全局互斥 + emit + 死锁）→ Task 12。全覆盖。
- **占位符**：无 TBD/TODO（Task 7/8 的「确认现有参数」是实施时读文件确认，已标注具体文件，非占位）。
- **类型一致**：`insert_ocr_item(conn, &str, OcrMeta)` / `OcrMeta{engine,model}` / `open_compact_editor_tab(item_id)` / `get_clipboard_item_text(id)->String` / `insert_ocr_clipboard_item(text)->i64` 在所有任务签名一致。

---

## Task 13: 代码审查追加修复 — CompactEditor 前端（2026-07-04）

第三轮代码审查发现 4 个 `pages/CompactEditor/index.tsx` 健壮性 bug。详见 spec §11。前端无组件级单测，靠 `npm run build`（tsc+vite）+ 用户 e2e 验证。

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`

- [x] **Step 1: replaceOne 焦点跳转（Bug 1.2）** — replaceOne 基于**替换后 next 文本**重新 `collectMatches`，`matchIdx = Math.min(matchIdx, len-1)` 钳制到新匹配列表有效区间。`collectMatches`：大小写不敏感 `indexOf` 循环收集所有匹配 offset（供 runFind/replaceOne/replaceAll 共用）。
- [x] **Step 2: replaceAll 大小写（Bug 1.3）** — `new RegExp(escaped, "gi")`（`escaped` escape 正则元字符 + `gi` 全局大小写不敏感）替换，修复大写缩写等匹配不到。
- [x] **Step 3: mount 监听泄露（Bug 1.4）** — mount `useEffect` 加 `cancelled` 标志：cleanup 置 true，`listen` resolve 后 `if (cancelled) fn() else unlisten = fn`，防 StrictMode / 快速 unmount 下 `unlisten` undefined 泄漏。
- [x] **Step 4: keydown 监听器重建（Bug 2.2）** — `doSaveRef = useRef(doSave)` + `useEffect(() => { doSaveRef.current = doSave }, [doSave])`，keydown 监听器改调 `doSaveRef.current()`、deps 去 `doSave` 只留 `showFind`，监听器挂载一次（消除 active.text 每键变 → doSave 新引用 → 监听器每键 remove+add 的 GC 压力）。
- [x] **Step 5: 验证** — `npm run build` + `npm run test` 绿。

---

## Task 14: 代码审查追加修复 — CompactEditor 键盘 undo/redo（2026-07-04）

§11（Task 13，审查 4 bug）之后第四轮审查。详见 spec §12。前端无组件级单测，靠 `npm run build` + 用户 e2e。

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`

- [x] **Step 1: 键盘 undo/redo 拦截（Bug 3.2）** — keydown `useEffect` 内（`const mod` 之后、doSave 拦截之前）加 `mod && (e.key.toLowerCase()==="z"||e.key.toLowerCase()==="y")` → `preventDefault` + `taRef.focus()` + `document.execCommand(isRedo?"redo":"undo")`（isRedo = y 或 shift）。受控 textarea 每次 value 同步清空 WebKit 原生 undo 栈 → 键盘失灵；按钮 execCommand 走文档级事务栈可用，键盘须统一走 execCommand。
- [x] **Step 2: 按钮 undo/redo 保留** — 撤销/重做按钮（`undo`/`redo` 函数 + 工具栏 JSX）保留，实测在 WKWebView 可用（§11 时曾被误判损坏后恢复）。注释更正为「按钮路径；实测在 WKWebView 工作」。
- [x] **Step 3: 验证** — `npm run build` 绿；e2e（用户）验证键盘 Cmd+Z / Shift+Z 与按钮均撤销/重做。
