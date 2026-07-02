# Compact Editor 原生化试水 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 compact editor 的 webview 内核换成 macOS 原生控件(`NSWindow`+`NSTextView`),作为 webview→原生迁移路径的试水。

**Architecture:** 用 Tauri v2 `tauri::window::WindowBuilder` 建无 webview 原生窗,objc2 挂 `NSScrollView`+`NSTextView`;通信复用 `compact-editor://result|cancel`,emit 方从 JS 换 Rust。**第一步是 go/no-go spike,失败即止。**

**Tech Stack:** Tauri v2、objc2 0.6 + objc2-app-kit 0.3(`NSWindow`/`NSTextView`/`NSScrollView`/`NSButton`/`NSFont`)、Rust。

**Spec:** `docs/superpowers/specs/2026-07-02-compact-editor-native-design.md`

---

## ⚠️ 条件式执行(必读)

**Phase 1 的 Task 3(spike)是 go/no-go gate。** 三条红线任一不达,**立即停止、删除 worktree、Phase 2-4 不执行**:

1. 中文 IME(拼音/候选词)能正常输入
2. 长文本鼠标滚动能到底
3. 内存增量是个位数 M(远低于 webview ~50M)

Phase 2-4 的 objc2 代码骨架基于标准模式,**精确实现以 spike 确认的 API 为准**(每个相关 task 的第一步是"复用 spike 验证的 textview/control 持有与调用方式")。试水的本质就是验证这些 API,因此 plan 不假装能预知 spike 的全部细节。

## 文件结构

| 文件 | 责任 | 改动 |
|---|---|---|
| `crates/desktop/Cargo.toml` | 补 objc2-app-kit features | 改 |
| `crates/desktop/src/compact_editor_native.rs` | **macOS 原生**:objc2 建 NSScrollView/NSTextView/工具栏、取文本、emit 桥 | **新建** |
| `crates/desktop/src/compact_editor_window.rs` | 窗口创建分流:macOS 原生 / 非 macOS webview | 改 |
| `crates/desktop/src/compact_editor_commands.rs` | open 建原生窗+塞文本(去 PENDING)、保存/取消 emit、关窗兜底;macOS 删 get_pending/load | 改 |
| `crates/desktop/src/window_position.rs` | 函数收 `&tauri::Window`(适配原生窗) | 改 |
| `crates/desktop/src/main.rs` | 命令注册 `#cfg` 调整 | 改 |
| `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` | 非 macOS fallback 保留;macOS 不再加载 | 不改(保留) |

**职责边界**:`compact_editor_native.rs` 只管"原生控件的创建/读写/事件→Rust 回调",不碰通信协议;`compact_editor_commands.rs` 只管"PENDING/requestId/emit 协议 + 窗口调度",不碰 objc2。两文件通过明确的函数接口耦合(native 模块暴露 `create/set_text/get_text/on_save/on_cancel` 等)。

---

## Phase 0 — 准备

### Task 1: 开独立 worktree(从 main)

**Files:** 无代码改动,只建 worktree。

- [ ] **Step 1: 从 main 开 worktree**

当前 session 已在 notepad worktree。compact editor 试水在**另一个**独立 worktree:

```bash
# 在主仓库根(/Users/wudarui/workspace/agent/octopus)执行
git worktree add .claude/worktrees/compact-editor-native -b compact-editor-native main
```

- [ ] **Step 2: 确认本 plan 已在该 worktree 可见**

```bash
ls .claude/worktrees/compact-editor-native/docs/superpowers/plans/2026-07-02-compact-editor-native.md
```

若不可见(本 plan 尚未合并 main),先把本 plan 所在分支合并 main,或直接 `git` cherry-pick 本 plan commit 到 `compact-editor-native` 分支。

- [ ] **Step 3: 后续所有命令都在新 worktree 执行**

```bash
cd .claude/worktrees/compact-editor-native
```

> 注:worktree 内 Bash cwd 可能实测为主仓库(见 memory `worktree-cwd-trap`),所有 cargo/git 命令须显式指 worktree 路径(`--manifest-path` / `-C` / 绝对路径)。

---

### Task 2: 补 objc2-app-kit features + 编译验证

**Files:**
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: 改 objc2-app-kit features**

把 `crates/desktop/Cargo.toml` 里的:

```toml
objc2-app-kit = { version = "0.3", features = ["NSWorkspace", "NSRunningApplication", "NSWindow", "NSApplication", "NSImage", "NSImageView", "NSView", "NSColor"] }
```

改为:

```toml
objc2-app-kit = { version = "0.3", features = [
    "NSWorkspace", "NSRunningApplication", "NSWindow", "NSApplication",
    "NSImage", "NSImageView", "NSView", "NSColor",
    "NSTextView", "NSText", "NSScrollView",
    "NSButton", "NSControl", "NSTextField", "NSFont",
] }
```

- [ ] **Step 2: cargo check 确认依赖解析**

```bash
cargo check --manifest-path crates/desktop/Cargo.toml
```

Expected: 编译通过(features 名有效)。若某 feature 报 unknown,查 `~/.cargo/registry/src/objc2-app-kit-0.3.*` 的 `Cargo.toml` 实际 feature 列表修正。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/Cargo.toml
git commit -m "build(desktop): 补 objc2-app-kit features(NSTextView/NSButton/NSFont)为 compact editor 原生化"
```

---

## Phase 1 — 可行性 spike(🔴 GO / NO-GO GATE)

### Task 3: spike —— 无 webview 原生窗 + NSTextView 显示静态中文

**目标:** 用最小代码验证「Tauri 原生窗能建、NSTextView 能挂、中文能显示/输入/滚动、文本能取回」。**这一步不接通信、不做工具栏**,纯 spike。

**Files:**
- Create: `crates/desktop/src/compact_editor_native.rs`
- Modify: `crates/desktop/src/compact_editor_window.rs`
- Modify: `crates/desktop/src/lib.rs`(或 main.rs,模块声明)

- [ ] **Step 1: 新建 native 模块骨架**

`crates/desktop/src/compact_editor_native.rs`:

```rust
//! macOS 原生 compact editor:NSWindow + NSScrollView + NSTextView。
//! 试水 spike:验证原生控件能挂、中文 IME/滚动/取文本可行。
//! 非 macOS 不编译本文件,回退 webview(见 compact_editor_window.rs 分流)。

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSColor, NSFont, NSScrollView, NSTextView, NSView, NSWindow,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use tauri::{Emitter, Manager, WebviewWindow};

    /// 临时 spike 入口:在给定的 webview 窗口上(借用其 NSWindow)挂一个 NSTextView
    /// 显示静态中文。spike 验证完即删,正式实现见 Task 5+。
    ///
    /// spike 阶段先复用一个普通 webview 窗的 NSWindow 来挂 NSTextView——绕开
    /// WindowBuilder 尚未接入,先验证「挂控件 + IME + 滚动 + 取文本」这条链。
    pub fn spike_attach_textview(window: &WebviewWindow) {
        let win = window.clone();
        let _ = window.run_on_main_thread(move || {
            let Ok(ptr) = win.ns_window() else { return };
            if ptr.is_null() { return; }
            let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };

            // 建 NSTextView(纯文本)+ 包进 NSScrollView
            let text_view: Retained<NSTextView> = unsafe { NSTextView::new() };
            unsafe {
                text_view.setRichText(false);
                text_view.setString(&NSString::from_str(
                    "这是一段用于 spike 的中文文本。\n你可以用输入法编辑我。\n",
                ));
                text_view.setFont(Some(&NSFont::systemFontOfSize(15.0)));
                text_view.setEditable(true);
                text_view.setSelectable(true);
            }

            let scroll: Retained<NSScrollView> = unsafe { NSScrollView::new() };
            let frame = unsafe { ns_win.contentView().frame() };
            unsafe {
                scroll.setFrame(frame);
                scroll.setDocumentView(Some(&text_view.clone()));
                scroll.setHasVerticalScroller(true);
                scroll.setAutoresizesSubviews(true);
            }
            unsafe { ns_win.setContentView(Some(&scroll)) };

            // 取文本验证:打印字符数到日志,证明能读回
            let s = unsafe { text_view.string().to_string() };
            log::info!("[spike] NSTextView attached, chars={}", s.chars().count());
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;

#[cfg(not(target_os = "macos"))]
// 非 macOS:无原生实现,回退 webview。
```

> **objc2 API 说明(以 cargo 编译为准)**:objc2-app-kit 0.3 的 owned 对象用 `Retained<T>`;`NSTextView::new()` / `NSScrollView::new()` 返回 `Retained`;方法名是 ObjC selector 的 snake_case(`setString` / `setDocumentView` / `setContentView` / `setFont` / `string`)。若 cargo 报方法不存在,查 `~/.cargo/registry/src/objc2-app-kit-0.3.*` 里 `NSTextView`/`NSScrollView` 的实际方法签名(可能需调整 `clone()` / `as_ref()` 的使用)。**这正是 spike 要验证的。**

- [ ] **Step 2: 在 compact_editor_window 建窗后调 spike**

`crates/desktop/src/compact_editor_window.rs` 现有 `create_compact_editor_window` 用 `WebviewWindowBuilder`。**spike 临时**在 `.build()` 后加一行调 `spike_attach_textview`(spike 完成后 Task 5 会把这里整体换成 `WindowBuilder`):

```rust
// 临时 spike:建 webview 窗后挂 NSTextView 覆盖在 webview 上方验证
match WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
    .title("编辑")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .center()
    .visible(true)
    .build()
{
    Ok(window) => {
        #[cfg(target_os = "macos")]
        crate::compact_editor_native::spike_attach_textview(&window);
    }
    Err(e) => log::warn!("compact editor window build failed: {e}"),
}
```

(把原 `let _ = WebviewWindowBuilder::new(...).build();` 改成上面的 match。)

- [ ] **Step 3: cargo build + 运行**

```bash
cargo build --manifest-path crates/desktop/Cargo.toml
# 运行 app(按项目惯常方式,如 cargo tauri dev 或现成启动脚本)
```

- [ ] **Step 4: 🔴 手动验证清单(GO/NO-GO)**

从剪贴板窗点「编辑」打开 compact editor,逐项确认:

- [ ] 窗口出现,可见 NSTextView 覆盖了 webview(显示「这是一段用于 spike 的中文文本…」)
- [ ] **中文 IME**:切到拼音输入法,能输入中文(出候选词、选词上屏)—— 红线 1
- [ ] **长文本滚动**:粘贴一长段文本(几百行),鼠标滚轮/触摸板能滚到底 —— 红线 2
- [ ] 能编辑(删字、打字)
- [ ] 日志出现 `[spike] NSTextView attached, chars=...`(证明取文本链路通)
- [ ] Cmd+Z 撤销生效(NSTextView undo manager)

**任一红线失败 → 停止,记录失败现象,删 worktree(`git worktree remove`),本 plan 终止。** 三条全过 → 继续 Phase 2。

- [ ] **Step 5: Commit(仅 spike 全过才提交)**

```bash
git add crates/desktop/src/compact_editor_native.rs crates/desktop/src/compact_editor_window.rs
git commit -m "spike(compact-editor): NSTextView 挂载验证通过(IME/滚动/取文本)"
```

> spike 的临时 webview-overlay 写法在 Task 5 会被正式 `WindowBuilder` 替换;这里先留 spike 痕迹便于回溯。

---

## Phase 2 — 通信改造(spike 通过后)

> 以下 task 假设 spike 已确认 objc2 API 模式。所有「持有 textview」「调方法」均复用 spike 验证的写法。

### Task 4: 状态机单测(CURRENT_REQUEST_ID / SAVED)

**Files:**
- Modify: `crates/desktop/src/compact_editor_commands.rs`(加状态 + 单测)

- [ ] **Step 1: 写失败测试**

在 `compact_editor_commands.rs` 的 `#[cfg(test)] mod tests` 加:

```rust
#[test]
fn session_state_set_and_clear() {
    use super::{current_request_id, set_session, mark_saved, take_unsaved_cancel};
    let _ = take_unsaved_cancel(); // 清残留
    set_session("rid-state-1".into());
    assert_eq!(current_request_id().as_deref(), Some("rid-state-1"));
    assert!(take_unsaved_cancel().is_none(), "未 mark_saved 前 take 不应消费");

    mark_saved();
    assert!(take_unsaved_cancel().is_none(), "已 saved,take 应 None");

    set_session("rid-state-2".into()); // 并发再开换文本
    // 未 saved + 关窗 → take 应返回该 requestId
    assert_eq!(take_unsaved_cancel().as_deref(), Some("rid-state-2"));
    assert!(take_unsaved_cancel().is_none(), "二次 take 应 None");
}
```

- [ ] **Step 2: 运行确认失败**

```bash
cargo test --manifest-path crates/desktop/Cargo.toml session_state_set_and_clear
```

Expected: FAIL(函数未定义)。

- [ ] **Step 3: 实现状态机**

在 `compact_editor_commands.rs`(替换原 `PENDING` 相关静态,见 Task 5 一并清理;此处先加状态访问层):

```rust
use std::sync::atomic::{AtomicBool, Ordering};

/// 当前编辑会话的 requestId(单例窗,同时一会话)。
static CURRENT_REQUEST_ID: Mutex<Option<String>> = Mutex::new(None);
/// 本会话是否已显式发 result/cancel(关窗兜底据此决定是否补 cancel)。
static SAVED: AtomicBool = AtomicBool::new(false);

pub fn set_session(request_id: String) {
    *CURRENT_REQUEST_ID.lock().unwrap() = Some(request_id);
    SAVED.store(false, Ordering::Relaxed);
}

pub fn current_request_id() -> Option<String> {
    CURRENT_REQUEST_ID.lock().unwrap().clone()
}

pub fn mark_saved() {
    SAVED.store(true, Ordering::Relaxed);
}

/// 关窗兜底:若未 saved 且有会话,返回 requestId 让调用方补发 cancel,并清空会话。
pub fn take_unsaved_cancel() -> Option<String> {
    let saved = SAVED.load(Ordering::Relaxed);
    let rid = CURRENT_REQUEST_ID.lock().unwrap().take();
    if !saved { rid } else { None }
}
```

- [ ] **Step 4: 运行确认通过**

```bash
cargo test --manifest-path crates/desktop/Cargo.toml session_state_set_and_clear
```

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/compact_editor_commands.rs
git commit -m "feat(compact-editor): 编辑会话状态机(requestId/SAVED)+ 单测"
```

---

### Task 5: open 改造 —— 建原生窗 + 塞文本(去 PENDING)

**Files:**
- Modify: `crates/desktop/src/compact_editor_native.rs`(加 `create_native_window` + `set_text`)
- Modify: `crates/desktop/src/compact_editor_window.rs`(macOS 分流到 native)
- Modify: `crates/desktop/src/compact_editor_commands.rs`(`open_compact_editor` 改造)

- [ ] **Step 1: native 模块加正式建窗 + 塞文本**

在 `compact_editor_native.rs` 的 `#[cfg(target_os="macos")] mod imp` 里,把 spike 的 `spike_attach_textview` 替换/扩展为正式接口(复用 spike 验证的 objc2 写法):

```rust
// WINDOW_LABEL 复用 compact_editor_window::WINDOW_LABEL(单一来源,勿重复定义)
use crate::compact_editor_window::WINDOW_LABEL;

/// 建无 webview 原生窗 + 挂 NSScrollView/NSTextView。返回后窗口已显示。
pub fn create_native_window(app: &tauri::AppHandle) {
    use tauri::window::WindowBuilder;
    let window = WindowBuilder::new(app, WINDOW_LABEL)
        .title("编辑")
        .inner_size(720.0, 560.0)
        .min_inner_size(480.0, 360.0)
        .decorations(true)
        .resizable(true)
        .center()
        .visible(true)
        .build();
    match window {
        Ok(w) => {
            attach_textview(&w);
            // 监听 Moved 存位置(Task 9 用),先留 hook
            // 关窗兜底见 Task 6
        }
        Err(e) => log::warn!("native compact editor build failed: {e}"),
    }
}

/// 在原生窗上挂 NSScrollView+NSTextView,并塞入 text。
fn attach_textview(window: &tauri::window::Window) {
    let win = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(ptr) = win.ns_window() else { return };
        if ptr.is_null() { return; }
        let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
        // …复用 spike 验证的 text_view/scroll 创建写法,初始 setString("")…
        // scroll 设为 contentView,autoresizing 跟随窗口尺寸
    });
}

/// 把 text 塞进当前窗的 NSTextView(并发再开 / 首次塞文本共用)。
pub fn set_text(app: &tauri::AppHandle, text: &str) {
    let Some(w) = app.get_webview_window(WINDOW_LABEL).or_else(|| {
        // 原生窗用 get_window(非 webview)
        app.webview_windows().get(WINDOW_LABEL).cloned()
    }) else { return };
    // 注:WindowBuilder 建的是原生 Window,取它需 app.get_window(LABEL)(Tauri v2)
    // …run_on_main_thread 里 text_view.setString(text)…
}
```

> **API 待 spike 后确认**:`WindowBuilder` 建的窗在 Tauri v2 用 `app.get_window(LABEL)` 还是 `get_webview_window` 取回——原生 Window 应走 `Manager::get_window`。取回后 `ns_window()` 同理。spike(用 WebviewWindow 验证 objc2)与正式(用 Window)的差异仅在建窗 API,objc2 挂控件部分完全复用。

- [ ] **Step 2: compact_editor_window.rs 分流**

```rust
use tauri::{WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "compact_editor_window";

pub fn create_compact_editor_window(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        crate::settings_window::set_dock_icon();
        crate::compact_editor_native::create_native_window(app_handle); // macOS 原生
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 非 macOS:回退 webview(现状实现原样保留)
        let _ = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
            .title("编辑")
            .inner_size(720.0, 560.0)
            .min_inner_size(480.0, 360.0)
            .decorations(true)
            .resizable(true)
            .center()
            .visible(true)
            .build();
    }
}
```

- [ ] **Step 3: open_compact_editor 去掉 PENDING,改直接塞文本**

`compact_editor_commands.rs` 把 `open_compact_editor` 改为:

```rust
#[tauri::command]
pub fn open_compact_editor(initial_text: String, request_id: String, app_handle: tauri::AppHandle) {
    set_session(request_id.clone());
    if let Some(_w) = app_handle.get_window(WINDOW_LABEL).or_else(|| app_handle.get_webview_window(WINDOW_LABEL).map(|w| w.as_ref().window().clone())) {
        // 并发再开:窗已存在,塞新文本 + 聚焦
        #[cfg(target_os = "macos")]
        crate::compact_editor_native::set_text(&app_handle, &initial_text);
        let _ = _w.show();
        let _ = _w.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
        #[cfg(target_os = "macos")]
        crate::compact_editor_native::set_text(&app_handle, &initial_text);
    }
}
```

并**删除** `static PENDING`、`store_pending`、`take_pending` 及 `get_pending_compact_edit` 命令(Task 7 在前端侧删 listener,但命令这里先删)。同步删 `compact_editor_commands.rs` 顶部的 PENDING 相关代码与旧测试 `pending_store_and_take_roundtrip`。

- [ ] **Step 4: main.rs 命令注册去掉 get_pending_compact_edit**

`crates/desktop/src/main.rs` 的 `invoke_handler` 里删掉 `compact_editor_commands::get_pending_compact_edit` 一行(`open_compact_editor` / `close_compact_editor` 保留)。

- [ ] **Step 5: cargo check + test**

```bash
cargo check --manifest-path crates/desktop/Cargo.toml
cargo test --manifest-path crates/desktop/Cargo.toml compact_editor
```

Expected: 编译通过;状态机测试通过。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(compact-editor): open 走原生窗+直接塞文本,去 PENDING(去 get_pending/load)"
```

---

### Task 6: 保存 / 取消 emit(后端)+ 关窗兜底

**Files:**
- Modify: `crates/desktop/src/compact_editor_native.rs`(工具栏保存/取消按钮 → Rust emit;Task 8 工具栏复用这两个出口)
- Modify: `crates/desktop/src/compact_editor_commands.rs`(emit 函数 + 关窗兜底)

- [ ] **Step 1: emit 出口 + 关窗兜底**

`compact_editor_commands.rs` 加:

```rust
use tauri::Emitter;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultPayload { request_id: String, text: String }

#[derive(Clone, serde::Serialize)]
struct CancelPayload { request_id: String }

/// 保存:取 NSTextView 文本 → emit result → 关窗。
#[cfg(target_os = "macos")]
pub fn do_save(app: &tauri::AppHandle) {
    let Some(rid) = current_request_id() else { return };
    let text = crate::compact_editor_native::get_text(app).unwrap_or_default();
    mark_saved();
    let _ = app.emit("compact-editor://result", ResultPayload { request_id: rid, text });
    close_compact_editor(app.clone());
}

/// 取消:emit cancel → 关窗。
pub fn do_cancel(app: &tauri::AppHandle) {
    if let Some(rid) = current_request_id() {
        mark_saved();
        let _ = app.emit("compact-editor://cancel", CancelPayload { request_id: rid });
    }
    close_compact_editor(app.clone());
}

/// 关窗兜底(挂在窗口 Destroyed 事件):未 saved 则补 cancel。
pub fn on_window_destroyed(app: &tauri::AppHandle) {
    if let Some(rid) = take_unsaved_cancel() {
        let _ = app.emit("compact-editor://cancel", CancelPayload { request_id: rid });
    }
    #[cfg(target_os = "macos")]
    { let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory); }
}
```

- [ ] **Step 2: native get_text(取 NSTextView 文本)**

`compact_editor_native.rs` 加(复用 spike 的 textview 取法):

```rust
/// 取当前 NSTextView 全文。run_on_main_thread 内同步取(主线程)。
pub fn get_text(app: &tauri::AppHandle) -> Option<String> {
    use crate::compact_editor_window::WINDOW_LABEL;
    let win = app.get_window(WINDOW_LABEL)?;
    let cell = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let cell2 = cell.clone();
    let _ = win.run_on_main_thread(move || {
        // 复用 spike 验证:win.ns_window() → contentView → documentView(textview) → string()
        // 拿到文本后:*cell2.lock().unwrap() = Some(s);
        //
        // spike 待确认:run_on_main_thread 是同步阻塞还是排队?
        //  - 同步阻塞:此处 cell 已填,直接返回 clone。
        //  - 排队(异步):优先改成「do_save 整体在主线程闭包内完成(get+emit)」,
        //    避免跨线程回传文本。spike 时验证 run_on_main_thread 语义后定方案。
    });
    cell.lock().unwrap().clone()
}
```

> 注:`get_text` 需在主线程读 objc2 文本。实现用 `run_on_main_thread` + `std::sync::mpsc` 或 `Mutex<Option<String>>` 回传。这是 spike 后的确定性实现,无创造性绘制。

- [ ] **Step 3: 挂关窗兜底事件**

在 `create_native_window`(Task 5)建窗后加:

```rust
let app_clone = app.clone();
window.on_window_event(move |event| {
    if let tauri::WindowEvent::Destroyed = event {
        crate::compact_editor_commands::on_window_destroyed(&app_clone);
    }
});
```

- [ ] **Step 4: cargo check**

```bash
cargo check --manifest-path crates/desktop/Cargo.toml
```

- [ ] **Step 5: 手动验证**

打开 compact editor → 编辑 → ⌘↵(暂用快捷键,工具栏按钮 Task 8 加)→ 剪贴板窗对应项文本更新;Esc → 不更新;X 关窗 → 不更新(兜底 cancel)。验证剪贴板窗 `openCompactEditor` 的 onResult 只在保存时触发。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(compact-editor): 保存/取消 emit 走后端 Rust + 关窗兜底 cancel"
```

---

### Task 7: 前端清理(macOS 不再加载 CompactEditor)

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`(删 `get_pending_compact_edit` / `compact-editor://load` 监听)

- [ ] **Step 1: 删前端 PENDING/load 相关代码**

`CompactEditor/index.tsx` 的 `useEffect` 里,删 `invoke("get_pending_compact_edit")` 和 `listen("compact-editor://load", ...)` 两段(它们对应已删的后端命令/事件)。保留 `result`/`cancel` 的 unmount 兜底 emit(非 macOS fallback 仍用前端 emit)。

> macOS 原生窗不再加载该前端页面(WindowBuilder 无 webview),故本文件改动只影响非 macOS fallback;前端 emit 路径在 fallback 下仍需保留。

- [ ] **Step 2: 前端构建验证**

```bash
cd crates/desktop/frontend && npm run build
```

Expected: 构建通过(无引用已删命令的 TS 报错)。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/index.tsx
git commit -m "chore(compact-editor): 前端删 get_pending/load 监听(macOS 走原生)"
```

---

## Phase 3 — 工具栏 + 功能复刻

### Task 8: 工具栏复刻(NSView + NSButton 横排)

**Files:**
- Modify: `crates/desktop/src/compact_editor_native.rs`

**目标:** 顶部一条 NSView 容器,横排 NSButton:撤销 / 重做 / 字号− / 字号(显示)/ 字号+ / 查找 / 清空 / [弹力] / 取消 / 保存。撤销/重做/查找按钮直接调 NSTextView 能力;保存/取消调 Task 6 的 `do_save`/`do_cancel`。

- [ ] **Step 1: 工具栏容器 + 一个完整按钮样例(保存)**

`compact_editor_native.rs` `attach_textview` 里,在 contentView 之上加工具栏容器(或用 NSWindow 的 contentView 分上下:上 toolbar / 下 scroll)。给「保存」按钮完整实现(NSButton + target/action + 回调到 `do_save`):

```rust
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSControl, NSView};
use objc2_foundation::NSRect;

// 工具栏容器:顶部 36pt 高
let toolbar: Retained<NSView> = unsafe { NSView::new() };
unsafe {
    toolbar.setFrame(NSRect::new(NSPoint::new(0., HEIGHT - 36.), NSSize::new(WIDTH, 36.)));
}

// 保存按钮(完整样例,其余按钮照此模式)
let save_btn: Retained<NSButton> = unsafe { NSButton::new() };
unsafe {
    save_btn.setTitle(&NSString::from_str("保存"));
    save_btn.setFrame(NSRect::new(NSPoint::new(WIDTH - 80., 6.), NSSize::new(70., 24.)));
    // target/action:点按触发 Rust 回调。用 Block 或注册 selector 调 do_save(app)。
    // objc2 0.6 绑 action 需定义一个 ObjC selector/类方法回调到 Rust fn。
    // …见 Step 2 的 action 绑定模式…
    toolbar.addSubview(&save_btn);
}
unsafe { ns_win.contentView().addSubview(&toolbar) };
```

- [ ] **Step 2: NSButton action → Rust 回调模式**

objc2 0.6 绑定按钮 action:定义一个 Rust 侧的 target 对象(实现 `define_class!` 一个轻量类,持有 `AppHandle`,其 `onClick:` selector 调对应 Rust fn)。**给一个完整 target 类定义**作为模板(所有按钮复用,按 tag/title 分发):

```rust
use objc2::define_class;
// define_class! 一个 CompactEditorTarget,持有 AppHandle + 一个 ButtonKind 枚举
// onClick: → match kind { Save => do_save, Cancel => do_cancel, Undo => textview.undo(), ... }
```

> **action 绑定是本 task 的核心验证点**(objc2 define_class 模式)。spike 已验证 textview;按钮回调是新验证。若 define_class 过重,退路:每按钮一个 target 类。以 spike 后实际编译通过的写法为准。

- [ ] **Step 3: 完整按钮映射表(照 Step 1/2 模式逐个加)**

| 按钮 | action |
|---|---|
| 撤销 | `textview.undoManager().undo()` |
| 重做 | `textview.undoManager().redo()` |
| 字号 − | `set_font_size(size - 1)`(Task 9) |
| 字号(显示,NSTextField 只读) | 显示当前 size |
| 字号 + | `set_font_size(size + 1)` |
| 查找 | `textview.orderFrontFindPanel(nil)`(系统 find bar,Cmd+F 同) |
| 清空 | 二次确认后 `textview.setString("")`(Task 10) |
| 取消 | `do_cancel(app)` |
| 保存 | `do_save(app)` |

- [ ] **Step 4: 布局自适应**

工具栏 + scroll 的 frame 在窗口 resize 时要重算。用 `setAutoresizingMask`(`NSViewWidthSizable` | `NSViewHeightSizable` for scroll;toolbar `NSViewWidthSizable` | `NSViewMinYMargin`)让两者跟随 contentView 尺寸。

- [ ] **Step 5: 手动验证**

- [ ] 所有按钮显示、可点
- [ ] 撤销/重做/查找/保存/取消 各按钮 action 正确
- [ ] 窗口缩放,工具栏 + 文本区不错位

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/compact_editor_native.rs
git commit -m "feat(compact-editor): 复刻工具栏(NSView+NSButton 横排,action 桥 Rust)"
```

---

### Task 9: 字号 ± + 持久化(app_config)

**Files:**
- Modify: `crates/desktop/src/compact_editor_native.rs`(set_font_size + 持久化)
- Modify: `crates/desktop/src/window_position.rs`(参考其 app_config 模式)

- [ ] **Step 1: 字号存取(app_config,仿 window_position)**

```rust
const FONT_KEY: &str = "compact_editor.font_size";
const FONT_MIN: f64 = 12.0;
const FONT_MAX: f64 = 24.0;

fn load_font_size() -> f64 {
    octopus_infra::db::load_config_key(FONT_KEY)
        .ok().flatten()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|&s| (FONT_MIN..=FONT_MAX).contains(&s))
        .unwrap_or(15.0)
}
fn save_font_size(s: f64) {
    let _ = octopus_infra::db::save_config_key(FONT_KEY, &s.to_string());
}
```

- [ ] **Step 2: set_font_size 应用到 NSTextView**

```rust
fn set_font_size(app: &tauri::AppHandle, delta: f64) {
    let size = (load_font_size() + delta).clamp(FONT_MIN, FONT_MAX);
    save_font_size(size);
    // run_on_main_thread: text_view.setFont(Some(&NSFont::systemFontOfSize(size)))
    // 同步更新工具栏字号显示 NSTextField
}
```

- [ ] **Step 3: 字数统计(工具栏右侧 NSTextField)**

```rust
fn update_char_count(text_view: &NSTextView, label: &NSTextField) {
    let n = unsafe { text_view.string().to_string() }.chars().count();
    unsafe { label.setString(&NSString::from_str(&format!("{n} 字"))) };
}
// 在 textview 变更回调(textViewDidChangeTypingAttributes / NSTextDelegate)里调
```

- [ ] **Step 4: 手动验证 + Commit**

- [ ] 字号 ± 生效且持久化(重启 app 仍记得)
- [ ] 字数随输入更新

```bash
git add -A
git commit -m "feat(compact-editor): 字号±持久化(app_config)+ 字数统计"
```

---

### Task 10: 清空(二次确认)

**Files:**
- Modify: `crates/desktop/src/compact_editor_native.rs`

- [ ] **Step 1: 清空二次确认(Rust 侧状态,仿前端 clearPending)**

```rust
use std::sync::atomic::{AtomicBool, Ordering};
static CLEAR_PENDING: AtomicBool = AtomicBool::new(false);

fn on_clear_clicked(app: &tauri::AppHandle) {
    if !CLEAR_PENDING.load(Ordering::Relaxed) {
        CLEAR_PENDING.store(true, Ordering::Relaxed);
        // 工具栏清空按钮图标/文字切到「确认」态(复刻前端 clearPending)
        // 2 秒后自动复位
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            CLEAR_PENDING.store(false, Ordering::Relaxed);
            // 复位按钮态
            let _ = app2;
        });
        return;
    }
    CLEAR_PENDING.store(false, Ordering::Relaxed);
    // run_on_main_thread: text_view.setString("")
}
```

- [ ] **Step 2: 手动验证 + Commit**

- [ ] 点清空→按钮变确认态→2 秒内再点才清空

```bash
git add -A
git commit -m "feat(compact-editor): 清空二次确认(复刻前端 clearPending)"
```

---

## Phase 4 — 验证 + 收尾

### Task 11: 🔴 内存实测

**Files:**
- 临时验证脚本/手动(不入库,或记入 worktree 验证笔记)

- [ ] **Step 1: 测现状 webview 版基线**

在 main 分支(或当前 worktree 的 webview fallback 路径)启动 app,记录打开 compact editor 前/后的进程 RSS:

```bash
# app pid 记为 $PID
ps -o rss= -p $PID   # 打开前
# 打开 compact editor(剪贴板点编辑)
ps -o rss= -p $PID   # 打开后
# 差值 = webview 增量(预期 ~50M)
```

- [ ] **Step 2: 测原生版**

切回 compact-editor-native 分支启动 app,同法测打开前/后 RSS 差值。

- [ ] **Step 3: 判据**

- [ ] **原生增量 << webview 增量,且为个位数 M → 红线 3 通过**
- [ ] 若原生增量 ≥ 30M → 红线 3 失败 → 回到 spec §7.2 重评估(可能需纯 objc2 自建 NSWindow,绕开 WindowBuilder 底层开销)

- [ ] **Step 4: 记录数值到验证笔记**

在 worktree 根写 `VALIDATION.md`(或追加到 spec),记录:webview 增量 / 原生增量 / 结论。作为推广到语音识别/剪贴板窗的依据。

- [ ] **Step 5: Commit 笔记**

```bash
git add VALIDATION.md
git commit -m "docs(compact-editor): 试水内存实测记录(webview vs 原生)"
```

---

### Task 12: 完整验证清单(spec §7.1 全 10 项)

- [ ] **Step 1: 逐项手测 + 截图存证**

对照 spec §7.1 十项,逐条手测,关键项(IME/滚动/保存回写)截图存到 worktree `screenshots/`。

- [ ] **Step 2: 任一不过 → 回到对应 task 修;三条红线不过 → 回 spec 重评估**

- [ ] **Step 3: Commit 截图清单**

```bash
git add screenshots/ VALIDATION.md
git commit -m "test(compact-editor): 完整验证清单通过(10/10)"
```

---

### Task 13: 非 macOS fallback 确认

- [ ] **Step 1: 确认 #cfg 分流正确**

```bash
cargo check --manifest-path crates/desktop/Cargo.toml --target x86_64-pc-windows-gnu 2>/dev/null || echo "(Windows target 未装,跳过;逻辑上 #cfg(not(macos)) 走 webview)"
```

- [ ] **Step 2: 代码审查 #cfg 边界**

确认所有 `objc2` / `compact_editor_native` 调用都在 `#[cfg(target_os="macos")]` 下,非 macOS 编译不触达(避免 link 错误)。`compact_editor_native.rs` 整文件已 `#[cfg(target_os="macos")]`。

- [ ] **Step 3: Commit(若有 #cfg 修正)**

```bash
git add -A && git commit -m "fix(compact-editor): 收紧 #cfg 边界,非 macOS 不触达 objc2"
```

---

### Task 14: 文档同步

**Files:**
- Modify: `docs/architecture.md`(compact editor 窗口类型说明:macOS 原生 / 非 macOS webview)
- Modify: spec(标注试水结果:通过/红线数值)

- [ ] **Step 1: architecture.md 更新 compact editor 条目**

说明 compact editor 在 macOS 为原生 NSWindow+NSTextView 窗口、非 macOS 为 webview fallback、及「webview→原生试水」背景。

- [ ] **Step 2: spec 追加试水结论**

在 spec 末尾追加「实施结果」小节:三条红线数值、内存对比、是否达成推广条件。

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: 同步 compact editor 原生化试水结果(architecture + spec)"
```

---

## 收尾:合并决策

试水三红线全过 + 内存达标 → 可考虑 ff-merge `compact-editor-native` → main(或按项目惯例 PR)。**合并前用 `superpowers:finishing-a-development-branch`。**

试水失败 → `git worktree remove .claude/worktrees/compact-editor-native`,main 不受影响。memory 记录失败原因(供下次评估:是否换纯 objc2 自建窗 / 或放弃原生方向)。
