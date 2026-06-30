# 精简编辑器（Compact Editor）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建一个纯文本精简编辑器窗口（工具栏 + textarea），编辑结果通过事件返回给调用方，并接入语音 Result、OCR、剪贴板文本条目三处。

**Architecture:** 独立 Tauri 窗口 `compact_editor_window`（原生标题栏、关窗即销毁）。后端 `compact_editor_commands.rs` 用静态 `PENDING` 暂存 `{text, request_id}`，前端 mount 时拉取。调用方生成 `requestId` → `open_compact_editor` → 监听 `compact-editor://result`（按 rid 过滤）→ 应用文本。共享命令 `set_clipboard_item_text` 供 OCR/剪贴板写回。

**Tech Stack:** Rust + Tauri 2（`WebviewWindowBuilder`、`emit`、`#[tauri::command]`）；React 19 + TypeScript + Vite 8 + Tailwind 4 + lucide-react；SQLite（rusqlite，`with_db`）。

---

## ✅ 实现状态（2026-06-30 同步）

本计划 **Task 1-6、8-11 已全部实现并提交**（git log：`b2e45c0`…`0d7a061`），后端 `cargo test` 全绿（55 passed, 0 failed）。

- **Task 1-6** ✅：store `update_content` / `set_clipboard_item_text` 命令 / `compact_editor_window` 窗口模块 / `compact_editor_commands` 命令层 + 单测 / `generate_handler!` 注册 4 命令 / CompactEditor 组件 + App 路由 + `compactEditor.ts` helper。
- **Task 7** ⚠️ **废弃**：旧方案（Result 弹独立编辑器窗）曾以 `85660ef` 实现，后因设计改为原地双模式，被 **Task 11 覆盖移除**（Result 不再 `openCompactEditor`）。checkbox 保持未勾。
- **Task 8-9** ✅：OCR 接入（移除系统 TextEdit）+ 剪贴板文本条目「编辑」按钮（`SquarePen`）。
- **Task 10** ✅：`architecture.md` 同步 + 全量后端 `cargo test` 绿。
- **Task 11** ✅：语音 Result 编辑框尺寸双模式（`toggleExpand` + 放大/缩小开关 + localStorage 记忆）。

**唯一剩余**：验收 e2e（手动，见文末）——需用户跑 `./run-octopus.sh` 逐项确认。

**✅ 已修 bug（真根因 `93f58a2`）**：Result 工具栏「放大」切换点击后窗口未变大——**真根因**：Tauri 2 **ACL 权限缺失**（`capabilities/default.json` 缺 `allow-set-max-size` 等，`await setMaxSize` 被拒抛错、`setSize` 因此未执行；toast 铁证 `Command plugin:window|set_max_size not allowed by ACL`）。修复：补 5 个窗口权限。另 `2195c80` 改 `resizable(true)` 为预防性双保险（文档称 `resizable(false)` 时 `setSize` 被忽略，未被独立证实但保留）。详见文末「已修 bug」节。

---

## ⚠️ 执行环境约束（每个任务都必须遵守）

1. **worktree cwd 陷阱**：Bash 的 cwd 实测可能是**主仓库**而非本 worktree。所有 cargo/npm/git 命令必须显式指向 worktree 绝对路径：
   - **worktree 根**：`/Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad`
   - cargo：`cargo test --manifest-path <worktree>/Cargo.toml -p <crate>` 或 `cargo build --manifest-path <worktree>/Cargo.toml -p octopus-desktop`
   - npm：`npm --prefix <worktree>/crates/desktop/frontend run build`
   - git：`git -C <worktree> ...`
   - Edit/Write 用绝对路径不受影响。
2. **dist 被跟踪**：`crates/desktop/dist`（36 文件）在 git 内、**非** gitignored。任何前端源码变更的任务，最后一步必须 `npm run build` 重建 dist 并把 `crates/desktop/dist` 一起提交（否则下游 `cargo run` 跑的是旧前端）。
3. **前端无单测框架**（无 vitest）。前端任务的「门」是 `npm run build`（= `tsc -b && vite build`，tsc 类型检查）。行为靠手动 e2e。
4. **分支**：当前已在 `worktree-feature-notepad` 分支（非 main），所有提交落在此分支。
5. **中文交互**：对话/注释/文档用中文。

---

## File Structure

**新建：**
- `crates/desktop/src/compact_editor_window.rs` — 窗口构建 + macOS 激活策略 + `on_compact_editor_closed`。镜像 `notepad_window.rs`。
- `crates/desktop/src/compact_editor_commands.rs` — 静态 `PENDING` + 3 个命令（`open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor`）+ 单测。
- `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` — 编辑器组件（textarea + 工具栏 + 事件收发）。
- `crates/desktop/frontend/src/lib/compactEditor.ts` — 调用方共享 helper（`openCompactEditor(text, onResult)`）。
- `crates/desktop/frontend/public/icons/expand-edit.svg` — Result「展开编辑」按钮图标。

**修改：**
- `crates/clipboard/src/store.rs` — 新增 `update_content`。
- `crates/desktop/src/clipboard_commands.rs` — 新增 `set_clipboard_item_text`；`ocr_image` 移除 TextEdit 调用。
- `crates/desktop/src/main.rs` — `generate_handler!` 注册 4 个新命令；`RunEvent::WindowEvent::Destroyed` 分支挂 `on_compact_editor_closed`；`mod` 声明。
- `crates/desktop/frontend/src/App.tsx` — 路由 `compact_editor_window`。
- `crates/desktop/frontend/src/components/SvgIcon.tsx` — 新增 `"expand-edit"` 图标。
- `crates/desktop/frontend/src/pages/Result/index.tsx` — 「展开编辑」按钮 + `applyResultText`。
- `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` — 文本条目「编辑」按钮 + `handleOcr` 改造。
- `docs/architecture.md` — 窗口/命令清单同步。

---

## Task 1: clipboard store 新增 `update_content`

**Files:**
- Modify: `crates/clipboard/src/store.rs`（在 `update_search_text` 后，约 L359 后新增函数）
- Test: `crates/clipboard/src/store.rs` 的 `mod tests`（约 L594+）

- [x] **Step 1: 写失败测试**

在 `crates/clipboard/src/store.rs` 的 `mod tests {` 内（参考 L605 `test_find_by_text_file_dedup` 的写法）新增：

```rust
    #[test]
    fn test_update_content() {
        // update_content 同时改写 content 与 search_text（OCR/剪贴板文本编辑后回写）。
        let conn = open_test_db();
        let id: i64 = 1700;
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: "原始文本".into(),
            search_text: "原始文本".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None, has_thumbnail: None,
            file_count: None, is_rich: false,
        }).unwrap();

        update_content(&conn, id, "改后文本").unwrap();

        // content 经 ClipboardItem 暴露
        let item = get_item_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(item.content, "改后文本");
        // search_text 不在 ClipboardItem 上，直接 SQL 断言
        let search: String = conn.query_row(
            "SELECT search_text FROM clipboard_history WHERE id = ?",
            params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(search, "改后文本");
    }
```

- [x] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard test_update_content
```
Expected: 编译失败，`cannot find function update_content`。

- [x] **Step 3: 实现 `update_content`**

在 `update_search_text` 函数后（L359 之后）新增：

```rust
/// 更新条目的 content 与 search_text（精简编辑器：用户编辑文本后回写剪贴板条目）。
/// 两列同写：content 是展示/粘贴源，search_text 是 FTS5 索引源，编辑后须同步以保搜索命中。
pub fn update_content(conn: &Connection, id: i64, text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET content = ?, search_text = ? WHERE id = ?",
        params![text, text, id],
    )?;
    Ok(())
}
```

- [x] **Step 4: 跑测试确认通过**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard test_update_content
```
Expected: PASS。

- [x] **Step 5: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/clipboard/src/store.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(clipboard): store 新增 update_content（content+search_text 同写）"
```

---

## Task 2: `set_clipboard_item_text` 命令

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`（新增命令，参考 `ocr_image` L379 的 `State<'_, Arc<ClipboardHandle>>` + `with_db` + `handle.write_text` 写法）

- [x] **Step 1: 新增命令**

在 `crates/desktop/src/clipboard_commands.rs` 中（`ocr_image` 函数之后）新增：

```rust
/// 精简编辑器回写：更新剪贴板条目文本（content + search_text）并同步系统剪贴板。
/// OCR 编辑、剪贴板文本条目编辑两处共用。
#[tauri::command]
pub async fn set_clipboard_item_text(
    item_id: i64,
    text: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_content(conn, item_id, &text)
    })
    .map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;
    Ok(())
}
```

> 命令是 `ClipboardHandle` + `with_db` 的薄封装，逻辑已在 Task 1 的 `update_content` 单测覆盖；本命令不另写单测（无 Tauri 运行时单测基建），靠 Task 8/9 的 e2e 验证。

- [x] **Step 2: 编译确认（命令注册在 Task 5，此处仅确认本文件编译）**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过（可能有 `unused` 警告，因尚未注册/调用，正常）。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/clipboard_commands.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 新增 set_clipboard_item_text 命令（编辑器回写剪贴板）"
```

---

## Task 3: `compact_editor_window.rs`（窗口生命周期）

**Files:**
- Create: `crates/desktop/src/compact_editor_window.rs`
- Modify: `crates/desktop/src/main.rs`（`mod compact_editor_window;` 声明 + Destroyed 分支挂 `on_compact_editor_closed`）

- [x] **Step 1: 创建窗口模块**

创建 `crates/desktop/src/compact_editor_window.rs`：

```rust
//! 精简编辑器窗口：独立 Tauri 窗口，原生标题栏，720×560 可调大小，居中。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 notepad/settings 对称。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 560.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";

/// 创建精简编辑器窗口（调用方已确保当前不存在同名窗口）。
pub fn create_compact_editor_window(app_handle: &tauri::AppHandle) {
    // macOS：编辑窗口切 Regular 让 Dock 显示图标（与 settings/notepad 一致）。
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("编辑")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .center()
    .visible(true)
    .build();
}

/// macOS: 精简编辑器窗口关闭时切回 Accessory（仅托盘）。
/// 与 notepad_window::on_notepad_closed 对称。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
```

- [x] **Step 2: 在 main.rs 声明模块 + 挂载关窗回调**

在 `crates/desktop/src/main.rs` 顶部 `mod` 声明区（找 `mod notepad_window;` 那一行，在其旁）新增：

```rust
mod compact_editor_window;
```

在 `app.run` 的 `RunEvent::WindowEvent { Destroyed, label, .. }` 分支（main.rs 约 L477-488，已有 `settings_window` / `notepad_window` 两个分支）追加一个 `else if`：

```rust
                } else if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_closed(app);
```

（即整体变成 `if label == "settings_window" {...} else if label == "notepad_window" {...} else if label == "compact_editor_window" {...}`。注意此块在 `#[cfg(target_os = "macos")]` 下，与非 mac 平台的 `on_compact_editor_closed` 缺省一致——该函数仅 mac 定义，非 mac 不引用，编译通过。）

- [x] **Step 3: 编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过。

- [x] **Step 4: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/compact_editor_window.rs crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 精简编辑器窗口模块（创建+macOS 激活策略）"
```

---

## Task 4: `compact_editor_commands.rs`（PENDING + 3 命令 + 单测）

**Files:**
- Create: `crates/desktop/src/compact_editor_commands.rs`
- Modify: `crates/desktop/src/main.rs`（`mod compact_editor_commands;` 声明；命令注册放 Task 5 统一做）

- [x] **Step 1: 写失败测试（先建文件含测试 + helper，命令后补）**

创建 `crates/desktop/src/compact_editor_commands.rs`：

```rust
//! 精简编辑器命令层：PENDING 暂存 + 开/取/关三个命令。
//!
//! PENDING 模式参考 result_window：open 时「先写 PENDING 再建窗」，前端 mount 调
//! get_pending_compact_edit 取走。编辑器是按需创建（非预建隐藏窗），故无需 ready 握手——
//! mount 必然在 create_window 之后，get 必读到。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 跨窗口传递的编辑载荷。rename_all=camelCase：事件 payload 与命令返回都给前端 {text, requestId}。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactEditPayload {
    pub text: String,
    pub request_id: String,
}

/// 待载入的初始文本。open 时写入，前端 mount/并发再开时 take 或 load 推送。
static PENDING: Mutex<Option<CompactEditPayload>> = Mutex::new(None);

fn store_pending(text: String, request_id: String) {
    *PENDING.lock().unwrap() = Some(CompactEditPayload { text, request_id });
}

fn take_pending() -> Option<CompactEditPayload> {
    PENDING.lock().unwrap().take()
}

/// 打开精简编辑器：写 PENDING；已存在则 emit load 推送新文本 + 聚焦，否则建窗。
#[tauri::command]
pub fn open_compact_editor(
    initial_text: String,
    request_id: String,
    app_handle: tauri::AppHandle,
) {
    store_pending(initial_text.clone(), request_id.clone());
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // 并发再开：窗口已 mount，PENDING 已被首次 take，改用事件推送新 {text, requestId}。
        let _ = window.emit(
            "compact-editor://load",
            CompactEditPayload { text: initial_text, request_id },
        );
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
    }
}

/// 前端 mount 时拉取初始文本（take 清空）。
#[tauri::command]
pub fn get_pending_compact_edit() -> Option<CompactEditPayload> {
    take_pending()
}

/// 关闭精简编辑器窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_compact_editor(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_store_and_take_roundtrip() {
        // 清空可能的残留（全局静态，防并行测试污染）。
        let _ = take_pending();
        store_pending("你好".into(), "rid-1".into());
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.text, "你好");
        assert_eq!(got.request_id, "rid-1");
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
```

- [x] **Step 2: 跑测试确认通过（helper 与测试同批落地，故直接验证）**

在 `main.rs` 顶部 `mod` 区加：

```rust
mod compact_editor_commands;
```

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop compact_editor_commands
```
Expected: PASS（`pending_store_and_take_roundtrip`）。命令本体是 Tauri 集成层，不单测。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/compact_editor_commands.rs crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 精简编辑器命令层（PENDING+开/取/关+单测）"
```

---

## Task 5: main.rs 注册命令

**Files:**
- Modify: `crates/desktop/src/main.rs` 的 `generate_handler!`（约 L218-258）

- [x] **Step 1: 注册 4 个新命令**

在 `generate_handler![ ... ]` 数组中：
- 紧跟 `clipboard_commands::ocr_image,`（L228）后加：`clipboard_commands::set_clipboard_item_text,`
- 紧跟 `notepad_window::open_notepad,`（L256）后加三行：
  ```rust
            compact_editor_commands::open_compact_editor,
            compact_editor_commands::get_pending_compact_edit,
            compact_editor_commands::close_compact_editor,
  ```

- [x] **Step 2: 编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过，无 warning（所有命令已注册+被前端待用）。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 注册精简编辑器 4 个命令到 generate_handler"
```

---

## Task 6: CompactEditor 前端组件 + App 路由 + 共享 helper

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`
- Create: `crates/desktop/frontend/src/lib/compactEditor.ts`
- Modify: `crates/desktop/frontend/src/App.tsx`（L46 switch 加 case）

- [x] **Step 1: 创建共享 helper `lib/compactEditor.ts`**

创建 `crates/desktop/frontend/src/lib/compactEditor.ts`：

```ts
import { invoke, listen } from "@/lib/tauri";

interface ResultPayload {
  requestId: string;
  text: string;
}
interface CancelPayload {
  requestId: string;
}

/**
 * 打开精简编辑器编辑一段文本，保存后回调 onResult。
 * 内部注册 result/cancel 两个监听，按 requestId 过滤；任一命中即清理解监听。
 * 取消/X 关窗 → 不调 onResult，仅清理。
 */
export async function openCompactEditor(
  initialText: string,
  onResult: (text: string) => void,
): Promise<void> {
  const requestId = crypto.randomUUID();
  let unlistenResult: (() => void) | undefined;
  let unlistenCancel: (() => void) | undefined;
  const cleanup = () => {
    unlistenResult?.();
    unlistenCancel?.();
  };
  // 先注册监听再开窗（保存需用户操作，无竞态；但顺序正确更稳）
  unlistenResult = await listen("compact-editor://result", (payload) => {
    const p = payload as ResultPayload;
    if (p.requestId !== requestId) return;
    onResult(p.text);
    cleanup();
  });
  unlistenCancel = await listen("compact-editor://cancel", (payload) => {
    const p = payload as CancelPayload;
    if (p.requestId !== requestId) return;
    cleanup();
  });
  await invoke("open_compact_editor", { initialText, requestId });
}
```

- [x] **Step 2: 创建编辑器组件 `pages/CompactEditor/index.tsx`**

创建 `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`：

```tsx
import { useState, useRef, useEffect, useCallback, type ReactNode } from "react";
import { invoke, listen } from "@/lib/tauri";
import { emit } from "@tauri-apps/api/event";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Search, Eraser, Save, X,
  ChevronUp, ChevronDown, Replace, Check,
} from "lucide-react";

interface PendingEdit {
  text: string;
  requestId: string;
}

const FONT_KEY = "compact-editor-font-size";
const FONT_MIN = 12;
const FONT_MAX = 24;

function CompactEditor() {
  const [text, setText] = useState("");
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [showFind, setShowFind] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [replaceQuery, setReplaceQuery] = useState("");
  const [matchIdx, setMatchIdx] = useState(-1);
  const [matches, setMatches] = useState<number[]>([]);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const requestIdRef = useRef<string>("");
  const savedRef = useRef(false); // 区分 unmount 时该发 result 还是 cancel

  // ── mount：拉取初始文本 + 监听并发再开 ──
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const pending = await invoke<PendingEdit | null>("get_pending_compact_edit");
      if (pending) {
        setText(pending.text);
        requestIdRef.current = pending.requestId;
        setTimeout(() => taRef.current?.focus(), 0);
      }
      unlisten = await listen("compact-editor://load", (payload) => {
        const p = payload as PendingEdit;
        setText(p.text);
        requestIdRef.current = p.requestId;
        savedRef.current = false;
        setMatches([]);
        setMatchIdx(-1);
        setTimeout(() => taRef.current?.focus(), 0);
      });
    })();
    return () => {
      unlisten?.();
      // 兜底：未保存的卸载（X 关窗/系统关闭）发 cancel，防调用方监听悬空。
      if (!savedRef.current && requestIdRef.current) {
        emit("compact-editor://cancel", { requestId: requestIdRef.current });
      }
    };
  }, []);

  const charCount = [...text].length;

  const doSave = useCallback(() => {
    if (!requestIdRef.current) return;
    savedRef.current = true;
    emit("compact-editor://result", { requestId: requestIdRef.current, text });
    invoke("close_compact_editor");
  }, [text]);

  const doCancel = useCallback(() => {
    if (requestIdRef.current) {
      savedRef.current = true; // 已显式发 cancel，别让 unmount 再发
      emit("compact-editor://cancel", { requestId: requestIdRef.current });
    }
    invoke("close_compact_editor");
  }, []);

  // ── 字号 ──
  const decFont = () => setFontSize((f) => Math.max(FONT_MIN, f - 1));
  const incFont = () => setFontSize((f) => Math.min(FONT_MAX, f + 1));
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 撤销/重做：execCommand 触发 textarea 原生栈（Cmd+Z/Y 原生亦生效，作可靠兜底）──
  const undo = () => { taRef.current?.focus(); document.execCommand("undo"); };
  const redo = () => { taRef.current?.focus(); document.execCommand("redo"); };

  // ── 清空（二次确认）──
  const [clearPending, setClearPending] = useState(false);
  const clearAll = () => {
    if (!clearPending) { setClearPending(true); setTimeout(() => setClearPending(false), 2000); return; }
    setText(""); setClearPending(false); setMatches([]); setMatchIdx(-1);
  };

  // ── 查找/替换 ──
  const runFind = useCallback(() => {
    const q = findQuery;
    if (!q) { setMatches([]); setMatchIdx(-1); return; }
    const ta = taRef.current;
    if (!ta) return;
    const lower = text.toLowerCase();
    const needle = q.toLowerCase();
    const idxs: number[] = [];
    let from = 0;
    while (true) {
      const i = lower.indexOf(needle, from);
      if (i === -1) break;
      idxs.push(i);
      from = i + needle.length;
    }
    setMatches(idxs);
    setMatchIdx(idxs.length > 0 ? 0 : -1);
    if (idxs.length > 0) selectRange(idxs[0], q.length);
  }, [findQuery, text]);

  useEffect(() => { if (showFind) runFind(); }, [runFind, showFind]);

  const selectRange = (start: number, len: number) => {
    const ta = taRef.current;
    if (!ta) return;
    ta.focus();
    ta.setSelectionRange(start, start + len);
    // 滚动到选中处
    const lineHeight = fontSize * 1.6;
    const lineNum = text.slice(0, start).split("\n").length;
    ta.scrollTop = Math.max(0, (lineNum - 2) * lineHeight);
  };

  const gotoMatch = (delta: number) => {
    if (matches.length === 0) return;
    const next = (matchIdx + delta + matches.length) % matches.length;
    setMatchIdx(next);
    selectRange(matches[next], findQuery.length);
  };

  const replaceOne = () => {
    if (matchIdx < 0 || !findQuery) return;
    const start = matches[matchIdx];
    const next = text.slice(0, start) + replaceQuery + text.slice(start + findQuery.length);
    setText(next);
    // 替换后重算
    setTimeout(runFind, 0);
  };

  const replaceAll = () => {
    if (!findQuery) return;
    setText(text.split(findQuery).join(replaceQuery));
    setTimeout(runFind, 0);
  };

  // ── 快捷键 ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "Enter") { e.preventDefault(); doSave(); return; }
      if (e.key === "Escape") {
        if (showFind) { setShowFind(false); return; }
        doCancel(); return;
      }
      if (mod && e.key.toLowerCase() === "f") { e.preventDefault(); setShowFind(true); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [doSave, doCancel, showFind]);

  const ToolBtn = ({ onClick, title, disabled, children }: {
    onClick: () => void; title: string; disabled?: boolean; children: ReactNode;
  }) => (
    <button
      type="button"
      disabled={disabled}
      title={title}
      onClick={onClick}
      className="p-1.5 rounded-md text-stone-600 hover:bg-stone-100 hover:text-stone-900 disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
    >{children}</button>
  );

  return (
    <div className="flex flex-col h-full bg-background">
      {/* 工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-stone-50">
        <ToolBtn onClick={undo} title="撤销 (Cmd+Z)"><Undo2 className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={redo} title="重做 (Cmd+Shift+Z)"><Redo2 className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={decFont} title="缩小字号" disabled={fontSize <= FONT_MIN}><ZoomOut className="w-4 h-4" /></ToolBtn>
        <span className="text-[11px] text-stone-500 w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={incFont} title="放大字号" disabled={fontSize >= FONT_MAX}><ZoomIn className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={() => setShowFind((v) => !v)} title="查找/替换 (Cmd+F)"><Search className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={clearAll} title="清空">
          {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
        </ToolBtn>
        <div className="flex-1" />
        <span className="text-[11px] text-stone-400 mr-2 tabular-nums">{charCount} 字</span>
        <button
          type="button"
          onClick={doCancel}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-stone-600 hover:bg-stone-200 transition-colors"
        >
          <X className="w-3.5 h-3.5" /> 取消
        </button>
        <button
          type="button"
          onClick={doSave}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-white bg-[#007aff] hover:bg-[#0066d6] transition-colors"
        >
          <Save className="w-3.5 h-3.5" /> 保存
          <span className="text-[10px] opacity-70">⌘↵</span>
        </button>
      </div>

      {/* 查找/替换条 */}
      {showFind && (
        <div className="flex-shrink-0 flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border bg-stone-100">
          <input
            autoFocus
            value={findQuery}
            onChange={(e) => setFindQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") gotoMatch(e.shiftKey ? -1 : 1); }}
            placeholder="查找"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <span className="text-[10px] text-stone-500 w-12 tabular-nums">
            {matches.length > 0 ? `${matchIdx + 1}/${matches.length}` : "0/0"}
          </span>
          <ToolBtn onClick={() => gotoMatch(-1)} title="上一个" disabled={matches.length === 0}><ChevronUp className="w-3.5 h-3.5" /></ToolBtn>
          <ToolBtn onClick={() => gotoMatch(1)} title="下一个" disabled={matches.length === 0}><ChevronDown className="w-3.5 h-3.5" /></ToolBtn>
          <input
            value={replaceQuery}
            onChange={(e) => setReplaceQuery(e.target.value)}
            placeholder="替换"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <button type="button" onClick={replaceOne} className="px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">替换</button>
          <button type="button" onClick={replaceAll} className="flex items-center gap-0.5 px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">
            <Replace className="w-3 h-3" /> 全替
          </button>
        </div>
      )}

      {/* 文本区 */}
      <textarea
        ref={taRef}
        value={text}
        onChange={(e) => setText(e.target.value)}
        style={{ fontSize: `${fontSize}px`, lineHeight: 1.6 }}
        spellCheck={false}
        className="flex-1 w-full resize-none outline-none p-4 bg-background text-foreground thin-scrollbar"
        placeholder="在此编辑…"
      />
    </div>
  );
}

export default CompactEditor;
```

> 注：`emit` 用 `@tauri-apps/api/event` 的原版（broadcast 到所有窗口，调用方按 rid 过滤）；`invoke`/`listen` 用 `@/lib/tauri` 封装（listen 已解包 payload）。

- [x] **Step 3: App.tsx 加路由**

在 `crates/desktop/frontend/src/App.tsx`：
- 顶部 import 区加：`import CompactEditor from "@/pages/CompactEditor";`
- `switch (label)` 内（L47-54 之间）加一个 case：

```tsx
          case "compact_editor_window":
            return <CompactEditor />;
```

- [x] **Step 4: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: `tsc -b` 无类型错误，`vite build` 产出新 dist。

- [x] **Step 5: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/src/pages/CompactEditor/index.tsx crates/desktop/frontend/src/lib/compactEditor.ts crates/desktop/frontend/src/App.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop-frontend): CompactEditor 组件 + App 路由 + 共享 helper"
```

---

## ⚠️ 设计修订（2026-06-30）：Task 7 废弃 → 改为 Task 11（Result 原地双模式）

用户反馈（5 条逐步明确）：语音 Result **不弹独立编辑器窗**，改为**编辑框原地尺寸双模式**（精简 520×116 / 长篇 720×480）+ 工具栏「放大/缩小」开关切换，长篇模式可拖拽调整且记忆尺寸。

- **Task 7（Result 接入独立窗「展开编辑」）废弃**——其 `applyResultText`/`openExpandEdit`/`openCompactEditor` 调用全部移除。
- **Task 6/8/9 不变**——精简编辑器独立窗保留给 OCR 与剪贴板文本。
- 替换为下方 **Task 11**。详见 spec §3.5① 重写。

### Task 11: 语音 Result 编辑框尺寸双模式（替换 Task 7）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`
- Create: `crates/desktop/frontend/public/icons/minimize.svg`（缩小态，四角向内）
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx`（ICONS 加 `"minimize"`）

**实现要点（关键代码骨架）：**

```tsx
import { LogicalSize } from "@tauri-apps/api/dpi";
// 移除：import { openCompactEditor } from "@/lib/compactEditor";

const COMPACT = { w: 520, h: 116 };
const EXPANDED_DEFAULT = { w: 720, h: 480 };
const EXPANDED_SIZE_KEY = "result-expanded-size";

function loadExpandedSize() {
  const saved = localStorage.getItem(EXPANDED_SIZE_KEY);
  if (saved) {
    const [w, h] = saved.split(",").map(Number);
    if (w > 0 && h > 0) return { w, h };
  }
  return EXPANDED_DEFAULT;
}

// state / ref
const [expanded, setExpanded] = useState(false);
const expandedRef = useRef(false);
const expandedSizeRef = useRef(loadExpandedSize());
useEffect(() => { expandedRef.current = expanded; }, [expanded]);

const toggleExpand = useCallback(async () => {
  const next = !expanded;
  expandedRef.current = next;            // 先同步 ref，防 onResized 读旧值污染记忆
  setExpanded(next);
  await win.setResizable(next);
  if (next) {
    await win.setSize(new LogicalSize(expandedSizeRef.current.w, expandedSizeRef.current.h));
  } else {
    await win.setSize(new LogicalSize(COMPACT.w, COMPACT.h));
  }
}, [expanded, win]);

// 长篇模式拖拽 → 记忆
useEffect(() => {
  let unlisten: UnlistenFn | undefined;
  let cancelled = false;
  win.onResized(async () => {
    if (!expandedRef.current) return;    // 仅长篇记
    const factor = await win.scaleFactor();
    const s = await win.outerSize();
    const w = s.width / factor, h = s.height / factor;
    expandedSizeRef.current = { w, h };
    localStorage.setItem(EXPANDED_SIZE_KEY, `${w},${h}`);
  }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
  return () => { cancelled = true; unlisten?.(); };
}, [win]);

// tools 数组：原 expand-edit 按钮改为 toggle（替换原 { id: "expand-edit", ... } 行）
{ id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName,
  label: expanded ? "缩小" : "放大", onClick: toggleExpand },

// 文本区 className（原 max-h-[63px]）：按 expanded 切换
expanded ? "h-full" : "max-h-[63px]",

// 删除：applyResultText / openExpandEdit 两个 useCallback
```

- Rust 侧 `result_window.rs` 创建改 `.resizable(true)`（`setSize` 需它），运行时由前端 `setMaxSize` 控可拖（不调 `setResizable`）。
- 边界：长篇模式向下长高，若原位置近屏幕底可能部分超出——MVP 不重算位置，e2e 观察。

- [x] Step 1: 新建 `minimize.svg` + `SvgIcon` 加 `"minimize"` 映射
- [x] Step 2: Result 加 `expanded`/`expandedRef`/`expandedSizeRef` + `toggleExpand` + `onResized` 监听
- [x] Step 3: tools 改 toggle 按钮 + 文本区 className + 移除 `openCompactEditor` import / `applyResultText` / `openExpandEdit`
- [x] Step 4: 重建 dist（`npm run build`）
- [x] Step 5: 验证（`tsc -b`、`cargo test` desktop/clipboard）
- [x] Step 6: commit

---

## Task 7: 语音 Result 接入「展开编辑」 ⚠️ 已废弃（见上方修订节 → Task 11）

**Files:**
- Create: `crates/desktop/frontend/public/icons/expand-edit.svg`
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx`（ICONS 加项）
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（import + `applyResultText` + `openExpandEdit` + tools 加按钮）

- [ ] **Step 1: 新增图标 svg**

创建 `crates/desktop/frontend/public/icons/expand-edit.svg`（一个「方框+向外箭头」的展开图标，currentColor 填充，与现有 svg 风格一致——单色、`viewBox="0 0 24 24"`）：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M15 3h6v6"/><path d="M9 21H3v-6"/><path d="M21 3l-7 7"/><path d="M3 21l7-7"/>
</svg>
```

> 与现有 `edit.svg` / `note.svg` 同为 stroke 风格（`fill="none" stroke="currentColor"`，24×24），SvgIcon 的 mask 渲染对 stroke 图标已验证可用（现有图标即如此），无需特判。

- [ ] **Step 2: SvgIcon 注册图标名**

在 `crates/desktop/frontend/src/components/SvgIcon.tsx` 的 `ICONS` 对象（L3-15）加一行：

```ts
  "expand-edit": "/icons/expand-edit.svg",
```

- [ ] **Step 3: Result 加 `applyResultText` + `openExpandEdit`**

在 `crates/desktop/frontend/src/pages/Result/index.tsx`：
- 顶部 import 加：
  ```tsx
  import { openCompactEditor } from "@/lib/compactEditor";
  ```
- 在 `saveToNote`（约 L263）之后新增 `applyResultText` 与 `openExpandEdit`：

```tsx
  // 展开编辑回写：更新展示态 + 落库（enter_edit_mode 置 editing=true 后 commit_edit 才生效；
  // 二者均门控于活跃 stage，与现有 toggleEdit 同窗口——会话结束后不落库，沿用既有契约）。
  const applyResultText = useCallback((newText: string) => {
    displayedRef.current = newText;
    setText(newText);
    invoke("enter_edit_mode");
    invoke("commit_edit", { text: newText });
  }, []);

  // 「展开编辑」：用当前显示文本打开精简编辑器，保存后回写。
  const openExpandEdit = useCallback(() => {
    if (!text.trim()) return;
    openCompactEditor(text, applyResultText);
  }, [text, applyResultText]);
```

- [ ] **Step 4: 工具栏加「展开编辑」按钮**

在 `tools` 数组（约 L380-396）中，「存入记事本」项（`{ id: "note", ... }`）之后、`...(editing ? [...] : [...])` 之前，插入：

```tsx
    { id: "expand-edit", icon: "expand-edit" as IconName, label: "展开编辑", disabled: !text.trim(), onClick: openExpandEdit },
```

- [ ] **Step 5: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [ ] **Step 6: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/public/icons/expand-edit.svg crates/desktop/frontend/src/components/SvgIcon.tsx crates/desktop/frontend/src/pages/Result/index.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop-frontend): Result 接入「展开编辑」打开精简编辑器"
```

---

## Task 8: OCR 接入（移除系统 TextEdit）

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`（`ocr_image` 删 TextEdit 调用 + 删 `open_text_editor_with_content` 函数）
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（`handleOcr` 取返回值 + 开编辑器 + 回写）

- [x] **Step 1: 后端 `ocr_image` 移除 TextEdit**

在 `crates/desktop/src/clipboard_commands.rs`：
- 删除 `ocr_image` 中的 `open_text_editor_with_content(&text);`（约 L416）这一行。
- 删除现已无引用的 `fn open_text_editor_with_content(text: &str) { ... }` 整个函数（约 L421-447）。删除前先确认无其他调用方：

Run:
```bash
grep -rn "open_text_editor_with_content" /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates
```
Expected: 仅 `clipboard_commands.rs` 内的定义处出现（调用已在上一行删除）→ 安全删除整个函数。

删除后 `ocr_image` 末尾变为：

```rust
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;

    Ok(text)
}
```

- [x] **Step 2: 后端编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过，无 dead_code 警告。

- [x] **Step 3: 前端 `handleOcr` 改造**

在 `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`：
- 顶部 import 加：`import { openCompactEditor } from "@/lib/compactEditor";`
- 改写 `handleOcr`（约 L87-106）——取 `ocr_image` 返回的文本，开编辑器，保存后 `set_clipboard_item_text` 回写 + 刷新：

```tsx
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      const text = await invoke<string>("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
      // 识别成功 → 打开精简编辑器，保存后回写剪贴板条目 + 刷新列表
      openCompactEditor(text, (edited) => {
        invoke("set_clipboard_item_text", { itemId: item.id, text: edited })
          .then(onChanged)
          .catch(console.error);
      });
    } catch (err) {
      setOcrLoading(false);
      const msg = String(err);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        console.error(err);
      }
    }
  };
```

- [x] **Step 4: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [x] **Step 5: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/clipboard_commands.rs crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(ocr): OCR 识别后打开精简编辑器（移除系统 TextEdit）+ 回写剪贴板"
```

---

## Task 9: 剪贴板文本条目「编辑」按钮

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（import + `handleEditText` + 操作区加按钮）

- [x] **Step 1: 加文本编辑处理 + 按钮**

在 `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`：
- 顶部 lucide import 中加 `SquarePen`（即 `import { ..., NotebookPen, SquarePen } from "lucide-react";`）。
- 在 `handleSaveToNote`（约 L127-138）之后新增：

```tsx
  const handleEditText = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (item.item_type === "image" || item.item_type === "file") return;
    openCompactEditor(item.content, (edited) => {
      invoke("set_clipboard_item_text", { itemId: item.id, text: edited })
        .then(onChanged)
        .catch(console.error);
    });
  };
```

- 在右侧操作区，「存入记事本」按钮（`onClick={handleSaveToNote}`，约 L198-204）之后插入「编辑」按钮，仅对文本/语音文本显示：

```tsx
        {item.item_type !== "image" && item.item_type !== "file" && (
          <button
            className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
            onClick={handleEditText}
            title="编辑"
          >
            <SquarePen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          </button>
        )}
```

- [x] **Step 2: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [x] **Step 3: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(clipboard): 文本条目新增「编辑」按钮（打开精简编辑器回写）"
```

---

## Task 10: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 同步 architecture.md**

在 `docs/architecture.md`：
- 窗口列表（已有 `notepad_window` 等的位置）加一行：`compact_editor_window` — 精简文本编辑器（工具栏+textarea，关窗即销毁，编辑结果事件返回调用方）。
- 命令清单（已有 `open_notepad` 等的位置）加：`open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor`（精简编辑器）/ `set_clipboard_item_text`（编辑器回写剪贴板）。
- 若有「Tauri 窗口」小结，补一句：精简编辑器与记事本并列，纯编辑工具不持久化。

（具体小标题与行号以文件实际结构为准，新增条目风格对齐已有 `notepad` 条目。）

- [x] **Step 2: 全量后端测试**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard -p octopus-desktop 2>&1 | tail -15
```
Expected: 全绿（含 Task 1 的 `test_update_content`、Task 4 的 `pending_store_and_take_roundtrip`，且未破坏既有测试）。

- [x] **Step 3: desktop 整体编译**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过。

- [x] **Step 4: 提交文档**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add docs/architecture.md
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "docs(architecture): 同步精简编辑器窗口与命令清单"
```

---

## 验收 e2e（手动——**本计划唯一剩余项**，交给用户跑 `./run-octopus.sh` 后逐项确认）

1. **Result 双模式**（ACL 权限已补 `93f58a2` + `resizable(true)` `2195c80`，待复验）：识别中文 → 点「放大」→ 窗口变 720×480、编辑区撑满、可拖拽调大小 → 编辑 → 保存 → 文本落库 → 点「缩小」切回 520×116；再切长篇恢复上次拖拽尺寸（localStorage 记忆）。
2. **OCR**：剪贴板图片点 OCR → 编辑器自动开 → 改 → 保存 → 该条目内容 + 系统剪贴板更新；不再弹系统 TextEdit。
3. **剪贴板文本**：文本/语音条目 hover 点「编辑」→ 编辑器开 → 改 → 保存 → 列表 + 系统剪贴板更新。
4. **边界**：取消/Esc/X 关窗不回写；字号记忆生效；查找替换命中数与跳转正确；字符计数对中文按字计；并发开窗（Result + 剪贴板同时开）不串扰。

## ✅ 已修 bug（`93f58a2`）：Result「放大」切换无响应

**现象**：语音结果窗工具栏点「放大」按钮，图标切到「缩小」但窗口尺寸没变（双模式切换失效）。

**真根因（ACL 权限缺失，铁证）**：Tauri 2 要求每个前端窗口命令必须在 `capabilities/*.json` 显式授权，未授权的命令抛 `Command plugin:window|<cmd> not allowed by ACL`。`toggleExpand` 里 `await win.setMaxSize(...)` 排在 `setSize` 之前，而 `default.json` 缺 `core:window:allow-set-max-size`，`setMaxSize` 被拒抛错 → `await` 中断 → `setSize`（一直有 `allow-set-size` 权限）从未执行。诊断 toast 暴露了真正错误 `Command plugin:window|set_max_size not allowed by ACL`。**图标变（state 变）但窗口不变 = click 生效、窗口命令被 ACL 拒**——用户确认图标会变、且 toast 报 ACL 错，排除了「`resizable(false)` 忽略 `setSize`」「工具栏 drag 吞 click」两个误判方向（曾据此改 `2195c80`，未生效）。

**修复**（`93f58a2`，真修复）：
- `crates/desktop/capabilities/default.json`：补 `core:window:allow-set-min-size` / `allow-set-max-size` / `allow-set-resizable` / `allow-outer-size` / `allow-scale-factor`（原有 `allow-set-size`）。

**预防性改动**（`2195c80`，非根因但保留为双保险）：
- `result_window.rs`：创建改 `.resizable(true)`——Tauri 文档称 `resizable(false)` 时 `setSize` 被忽略。虽未被独立证实为必要（ACL 才是真正阻塞），但 `resizable(true)` + `setMaxSize` 控拖更稳，保留。
- `Result/index.tsx::toggleExpand`：不调 `setResizable`，改用 `setMaxSize` 控可拖——精简态 `max=520×116` 锁死防拖，长篇态 `max=4000` 解除后可拖；首帧 mount 锁一次精简态 max（不设 min，避免 `min>max` 冲突致 `setMinSize` 抛错、`setSize` 不执行）。

**教训**：前端 `await` 窗口命令无 try/catch 时 ACL 错误被默默吞掉，外观似「窗口行为异常」实为「权限拒绝」。诊断 toast（读回实际尺寸 / 捕获并显示错误）是定位此类问题的关键工具。

**待复验**：e2e 第 1 项（双模式切换 + 拖拽记忆）由用户跑 `./run-octopus.sh` 确认——ACL 已补 + `resizable(true)`，理论上应生效；未实测前不谎报「已验证」。

## 不做（明确排除）

- 不合并到 main（记事本 e2e 仍待用户确认；合并是外向难逆动作，等用户回来显式授权走 `superpowers:finishing-a-development-branch`）。
- 不接入富文本（TipTap）/标题/分类/收藏——属于完整版记事本。
- 不加 vitest 前端测试框架（YAGNI，`tsc -b` + e2e 足够）。
