# 剪贴板浮窗边缘吸附 + 迷你模式 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 剪贴板浮窗拖到屏幕边缘自动吸附收缩为 8px 细条，hover 展开，点击外部收回。

**Architecture:** 窗口物理尺寸始终 300×600 不变。收缩态靠 CSS 隐藏大部分区域 + NSWindow `setIgnoresMouseEvents(true)` 全窗口穿透 + NSTrackingArea 标记 8px 细条可点击。展开/收回用 CSS transition。完全在当前屏幕内，不越界。

**Tech Stack:** Rust + Tauri 2 + objc2（NSWindow 操作）+ React + CSS transition

## Global Constraints

- **物理/逻辑坐标**：`Monitor::position()` / `Monitor::size()` 返回物理像素，必须除以 `scale_factor()` 转逻辑坐标（AGENTS.md 坐标踩坑章节）。`CGEvent::location()` 返回逻辑坐标不除 scale。
- **窗口物理尺寸不变**：始终 300×600，收缩/展开只改 CSS 可见区域 + 窗口位置 + `ignoresMouseEvents`，不用 `setSize`
- **仅 macOS**：`ignoresMouseEvents` / NSTrackingArea / NSEvent global monitor 是 macOS API
- **NSWindow 操作必须在主线程**：`setIgnoresMouseEvents` 等涉及 NSWindow 的操作必须用 `app.run_on_main_thread()` 或 `objc2` main thread
- **capabilities 白名单**：clipboard_window 已在 `capabilities/default.json`，无需新增
- **transparent + decorations(false)**：`setSize` 不可靠（result_window 踩坑确认），此功能不碰物理尺寸

**Spec:** [`2026-07-10-clipboard-dock-design.md`](../specs/2026-07-10-clipboard-dock-design.md)

---

### Task 1: dock 状态持久化（window_position.rs）

**Files:**
- Modify: `crates/desktop/src/window_position.rs`
- Test: `crates/desktop/src/window_position.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Produces: `save_dock_state(label: &str, edge: &str)` / `load_dock_state(label: &str) -> Option<String>`

- [x] **Step 1: 写 save_dock_state / load_dock_state**

在 `window_position.rs` 末尾（`parse_position` 后）加：

```rust
/// 保存窗口 dock 状态到 app_config。
/// key 格式：`window_dock.{label}`，value: "right" | "left" | "none"。
pub fn save_dock_state(label: &str, edge: &str) {
    let key = format!("window_dock.{}", label);
    if let Err(e) = octopus_infra::db::save_config_key(&key, edge) {
        log::warn!("Failed to save dock state for {}: {}", label, e);
    } else {
        debug!("Saved dock state {}: {}", label, edge);
    }
}

/// 从 app_config 读取窗口 dock 状态。
pub fn load_dock_state(label: &str) -> Option<String> {
    let key = format!("window_dock.{}", label);
    let value = octopus_infra::db::load_config_key(&key).ok().flatten()?;
    let edge = value.trim().to_string();
    if edge.is_empty() {
        None
    } else {
        debug!("Loaded dock state {}: {}", label, edge);
        Some(edge)
    }
}
```

- [x] **Step 2: 写单元测试**

在 `window_position.rs` 的 `#[cfg(test)]` mod 中加：

```rust
#[test]
fn dock_state_round_trip() {
    use octopus_infra::db;
    let label = "test_window_dock_roundtrip";
    // 清理
    let _ = db::save_config_key(&format!("window_dock.{}", label), "none");
    // save right
    crate::window_position::save_dock_state(label, "right");
    let loaded = crate::window_position::load_dock_state(label);
    assert_eq!(loaded.as_deref(), Some("right"));
    // save left
    crate::window_position::save_dock_state(label, "left");
    let loaded = crate::window_position::load_dock_state(label);
    assert_eq!(loaded.as_deref(), Some("left"));
    // save none
    crate::window_position::save_dock_state(label, "none");
    let loaded = crate::window_position::load_dock_state(label);
    assert_eq!(loaded.as_deref(), Some("none"));
}
```

- [x] **Step 3: 编译 + 测试**

Run: `cargo test -p octopus-desktop --bin octopus-desktop window_position 2>&1 | tail -5`
Expected: PASS

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/window_position.rs
git commit -m "feat(clipboard-dock): dock 状态持久化 save/load_dock_state"
```

---

### Task 2: 吸附检测逻辑（clipboard_window.rs）

**Files:**
- Modify: `crates/desktop/src/clipboard_window.rs:40-50`（Moved 事件 handler）

**Interfaces:**
- Consumes: Task 1 的 `save_dock_state` / `load_dock_state`
- Produces: `detect_and_apply_dock(window: &WebviewWindow) -> Option<String>`（返回 "right"/"left"/None）

- [x] **Step 1: 写吸附检测函数**

在 `clipboard_window.rs` 中 `create_clipboard_window` 函数之后加：

```rust
/// 检测窗口是否应吸附到屏幕边缘。
/// 返回 Some("right") / Some("left") / None。
///
/// 逻辑：
/// 1. 获取窗口外边框逻辑坐标
/// 2. 找窗口中心所在显示器
/// 3. 检测窗口外边框距该显示器左/右边缘距离 ≤ 10px
fn detect_dock_edge(window: &tauri::WebviewWindow) -> Option<&'static str> {
    const DOCK_THRESHOLD: f64 = 10.0;

    let pos = window.outer_position().ok()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let win_x = pos.x as f64 / scale;
    let win_y = pos.y as f64 / scale;
    let win_w = 300.0_f64; // 剪贴板窗口固定宽
    let win_h = 600.0_f64;
    let center_x = win_x + win_w / 2.0;
    let center_y = win_y + win_h / 2.0;

    let monitors = window.available_monitors().unwrap_or_default();
    // 找窗口中心所在显示器
    let current = monitors.iter().find(|m| {
        let ms = m.scale_factor();
        let mx = m.position().x as f64 / ms;
        let my = m.position().y as f64 / ms;
        let mw = m.size().width as f64 / ms;
        let mh = m.size().height as f64 / ms;
        center_x >= mx && center_x <= mx + mw && center_y >= my && center_y <= my + mh
    })?;

    let ms = current.scale_factor();
    let mon_right = current.position().x as f64 / ms + current.size().width as f64 / ms;
    let mon_left = current.position().x as f64 / ms;

    let dist_right = (mon_right - (win_x + win_w)).abs();
    let dist_left = (win_x - mon_left).abs();

    if dist_right <= DOCK_THRESHOLD && dist_right <= dist_left {
        Some("right")
    } else if dist_left <= DOCK_THRESHOLD {
        Some("left")
    } else {
        None
    }
}
```

- [x] **Step 2: 改造 Moved 事件 handler**

将 `clipboard_window.rs` 中的 `Moved` 事件分支改为：

```rust
tauri::WindowEvent::Moved(_) => {
    // 先保存位置（现有逻辑）
    crate::window_position::save_current_position(&win_clone, WINDOW_LABEL);

    // 检测吸附
    if let Some(edge) = detect_dock_edge(&win_clone) {
        crate::window_position::save_dock_state(WINDOW_LABEL, edge);
        let _ = app_clone.emit("clipboard://dock-changed", edge);
        log::info!("clipboard docked to {}", edge);
    }
}
```

需要 import：`use tauri::{Emitter, Manager};`（如尚未导入）。

> **注意**：此 Task 只做检测 + 保存 + emit。前端收到事件后的 CSS 变化、NSWindow 操作在后续 Task。

- [x] **Step 3: 编译**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/clipboard_window.rs
git commit -m "feat(clipboard-dock): 吸附检测逻辑 + Moved 事件集成"
```

---

### Task 3: NSWindow ignoresMouseEvents + TrackingArea（新建 clipboard_dock.rs）

**Files:**
- Create: `crates/desktop/src/clipboard_dock.rs`
- Modify: `crates/desktop/src/main.rs`（加 `mod clipboard_dock;`）

**Interfaces:**
- Produces: `set_ignores_mouse_events(window: &WebviewWindow, ignore: bool)` / `apply_dock_collapsed(window: &WebviewWindow, edge: &str)` / `apply_dock_expanded(window: &WebviewWindow)`

- [x] **Step 1: 创建 clipboard_dock.rs 基础结构**

```rust
//! 剪贴板浮窗 dock（吸附收缩）的 NSWindow 操作。
//! 仅 macOS——ignoresMouseEvents + NSTrackingArea 是 macOS API。

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, AllocAnyThread, DefinedClass, MainThreadOnly, DefinedClass};
    use objc2_app_kit::{NSView, NSWindow};
    use objc2_foundation::MainThreadMarker;

    /// 设置窗口的 ignoresMouseEvents 属性。
    /// 必须在主线程调用。
    pub fn set_ignores_mouse_events(window: &tauri::WebviewWindow, ignore: bool) {
        let mtm = MainThreadMarker::new().expect("must be main thread");
        // 从 Tauri WebviewWindow 获取 NSWindow
        let ns_window = window.ns_window().expect("get NSWindow");
        // safety: ns_window 返回的是 retained 指针
        unsafe {
            let _: () = msg_send![&ns_window, setIgnoresMouseEvents: if ignore { 1i8 } else { 0i8 }];
        }
        log::debug!("clipboard_dock: setIgnoresMouseEvents = {}", ignore);
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

/// 收缩态：设 ignoresMouseEvents(true)。
/// TrackingArea 的设置在 Task 4（需要前端配合）。
#[cfg(target_os = "macos")]
pub fn apply_dock_collapsed(window: &tauri::WebviewWindow) {
    set_ignores_mouse_events(window, true);
}

/// 展开态：设 ignoresMouseEvents(false)。
#[cfg(target_os = "macos")]
pub fn apply_dock_expanded(window: &tauri::WebviewWindow) {
    set_ignores_mouse_events(window, false);
}

#[cfg(not(target_os = "macos"))]
pub fn apply_dock_collapsed(_window: &tauri::WebviewWindow) {}
#[cfg(not(target_os = "macos"))]
pub fn apply_dock_expanded(_window: &tauri::WebviewWindow) {}
```

- [x] **Step 2: main.rs 加 mod 声明**

在 `crates/desktop/src/main.rs` 中 `mod clipboard_window;` 后加：

```rust
mod clipboard_dock;
```

- [x] **Step 3: 编译**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过（可能需要调整 objc2 import——参考 `activation.rs` 的现有模式）

> ⚠️ objc2 msg_send 语法可能与 `activation.rs` 现有模式不同——编译时报错则参考 `activation.rs` 的 FFI 方式调整。Tauri 的 `ns_window()` 返回 `Result<Retained<objc2::runtime::AnyObject>>`（objc2 0.6），需要正确处理。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/clipboard_dock.rs crates/desktop/src/main.rs
git commit -m "feat(clipboard-dock): NSWindow ignoresMouseEvents 封装"
```

---

### Task 4: 前端 dock 状态 + CSS 展开/收缩（Clipboard/index.tsx + DockBar.tsx）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`
- Create: `crates/desktop/frontend/src/pages/Clipboard/DockBar.tsx`

**Interfaces:**
- Consumes: Task 2 的 `clipboard://dock-changed` 事件
- Produces: 前端 dockMode/dockEdge 状态 + CSS 展开/收缩动画

- [x] **Step 1: 新建 DockBar.tsx 组件**

```tsx
// crates/desktop/frontend/src/pages/Clipboard/DockBar.tsx
// 8px 细条——吸附收缩态时显示在吸附边缘侧。
// onMouseEnter 触发展开。

interface DockBarProps {
  edge: "right" | "left";
  onMouseEnter: () => void;
}

export function DockBar({ edge, onMouseEnter }: DockBarProps) {
  return (
    <div
      className={`absolute top-0 bottom-0 w-2 bg-voice/80 shadow-[0_0_8px_rgba(0,0,0,0.3)]
        ${edge === "right" ? "right-0" : "left-0"}
        hover:bg-voice transition-colors duration-150`}
      onMouseEnter={onMouseEnter}
    />
  );
}
```

- [x] **Step 2: 在 index.tsx 加 dock 状态**

在 `Clipboard/index.tsx` 的组件顶部加：

```tsx
const [dockEdge, setDockEdge] = useState<"right" | "left" | null>(null);
const [dockMode, setDockMode] = useState<"none" | "collapsed" | "expanded">("none");

// 监听 dock-changed 事件
useEffect(() => {
  const unlisten = listen("clipboard://dock-changed", (edge: string) => {
    if (edge === "right" || edge === "left") {
      setDockEdge(edge);
      setDockMode("collapsed");
    } else {
      setDockEdge(null);
      setDockMode("none");
    }
  });
  const unlistenExpand = listen("clipboard://expand", () => setDockMode("expanded"));
  const unlistenCollapse = listen("clipboard://collapse", () => setDockMode("collapsed"));
  return () => { unlisten.then(f => f()); unlistenExpand.then(f => f()); unlistenCollapse.then(f => f()); };
}, []);
```

> **注意**：`listen` 来自 `lib/tauri.ts`（自动解包 payload）。如果 payload 是字符串而非对象，`listen` 回调参数就是字符串本身。需确认事件 emit 的 payload 格式——Task 2 中 `emit("clipboard://dock-changed", edge)` 传的是 `&str`，Tauri 序列化为 JSON 字符串。

- [x] **Step 3: CSS class 切换**

修改 `index.tsx` 的最外层容器 div，根据 dockMode 切换 class：

```tsx
<div
  className={`flex flex-col h-screen select-none overflow-hidden rounded-xl border border-border shadow-2xl shadow-black/8
    ${dockMode === "collapsed" ? "w-2" : "w-[300px]"}
    ${dockMode === "collapsed" ? "transition-[width] duration-300 ease-out" : ""}
    ${dockEdge === "right" ? "ml-auto" : ""}
    data-tauri-drag-region`}
  style={{ background: dockMode === "collapsed" ? "transparent" : "var(--color-background)" }}
>
```

收缩态时隐藏内容、显示 DockBar：

```tsx
{dockMode === "collapsed" && dockEdge && (
  <DockBar edge={dockEdge} onMouseEnter={() => {
    invoke("clipboard_dock_expand");
    setDockMode("expanded");
  }} />
)}
{dockMode !== "collapsed" && (
  <>
    {/* 现有标题栏 + 搜索框 + 列表 */}
  </>
)}
```

- [x] **Step 4: tsc 检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 errors

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/DockBar.tsx crates/desktop/frontend/src/pages/Clipboard/index.tsx
git commit -m "feat(clipboard-dock): 前端 dock 状态 + DockBar 组件 + CSS 展开/收缩"
```

---

### Task 5: Rust 展开/收缩命令 + 全局点击监听

**Files:**
- Modify: `crates/desktop/src/clipboard_window.rs`
- Modify: `crates/desktop/src/clipboard_dock.rs`

**Interfaces:**
- Consumes: Task 3 的 `apply_dock_collapsed` / `apply_dock_expanded`，Task 4 的 `clipboard://expand` / `clipboard://collapse` 事件
- Produces: `clipboard_dock_expand` / `clipboard_dock_collapse` Tauri 命令

- [x] **Step 1: 新增 Tauri 命令**

在 `clipboard_window.rs` 中加两个命令：

```rust
#[tauri::command]
pub fn clipboard_dock_expand(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        crate::clipboard_dock::apply_dock_expanded(&window);
        let _ = app.emit("clipboard://expand", ());
    }
}

#[tauri::command]
pub fn clipboard_dock_collapse(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        crate::clipboard_dock::apply_dock_collapsed(&window);
        let _ = app.emit("clipboard://collapse", ());
    }
}
```

> 命令在主线程执行（Tauri command 默认），满足 NSWindow 操作的主线程要求。

在 `main.rs` 的 `invoke_handler!` 中注册这两个命令。

- [x] **Step 2: 全局鼠标点击监听（Expanded 态收缩触发）**

在 `clipboard_dock.rs` 中加全局监听器。用 `NSEvent.addGlobalMonitorForEvents`：

```rust
/// 启动全局鼠标点击监听。
/// Expanded 态下，点击窗口外部 → 触发收缩。
/// 返回 monitor handle，停止时调 stop_global_click_monitor。
#[cfg(target_os = "macos")]
pub fn start_global_click_monitor(app: tauri::AppHandle) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, AllocAnyThread};
    use objc2_app_kit::NSEventMask;
    use objc2_foundation::MainThreadMarker;

    // NSEvent 全局 monitor 需要主线程
    let mtm = MainThreadMarker::new().expect("must be main thread");

    let handler = objc2::rc::Retained::new(unsafe {
        // 创建 block 回调——监听 leftMouseDown | rightMouseDown
        // 检查点击位置是否在 clipboard_window frame 外
        // 如果在外 → app.emit("clipboard://collapse", ())
    });

    // 具体实现参考 activation.rs 的 objc2 block 模式
    // 存 monitor handle 到 static 或 State 以便后续 stop
    log::info!("clipboard_dock: global click monitor started");
}
```

> ⚠️ objc2 block 语法复杂。如果 objc2 block 回调太难写，退回方案：用 NSEvent tap 或 `CGEventTap`（已有 FFI 基础），或者简化为前端检测（`onBlur` + 延迟——但 onBlur 不可靠）。实施时先试 objc2 block，不行用 CGEventTap。

- [x] **Step 3: 编译**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/clipboard_window.rs crates/desktop/src/clipboard_dock.rs crates/desktop/src/main.rs
git commit -m "feat(clipboard-dock): 展开/收缩命令 + 全局点击监听"
```

---

### Task 6: 窗口打开时恢复 dock 状态

**Files:**
- Modify: `crates/desktop/src/clipboard_window.rs`（`create_clipboard_window` 和 `toggle_clipboard_window`）

- [x] **Step 1: create_clipboard_window 读 dock 状态**

在 `create_clipboard_window` 的 `restore_window_position` 之后加：

```rust
// 恢复 dock 状态
let dock_edge = crate::window_position::load_dock_state(WINDOW_LABEL);
if let Some(ref edge) = dock_edge {
    if edge == "right" || edge == "left" {
        // 以 collapsed 态打开：位置已恢复（贴边），通知前端
        let _ = app.emit("clipboard://dock-changed", edge.as_str());
        crate::clipboard_dock::apply_dock_collapsed(&window);
    }
}
```

> **吸附位置修正**：如果 dock=right，窗口 x 应为 `monitor.right() - 300`；如果 dock=left，x 应为 `monitor.left()`。位置记忆存的 x,y 可能不精确——需要在此处根据 dock edge 修正 x 坐标。但 Task 1 已存了 dock edge，此处可精确计算。

修正位置：

```rust
if let Some(ref edge) = dock_edge {
    if edge == "right" || edge == "left" {
        if let Ok(Some(monitor)) = window.current_monitor().or(window.primary_monitor()) {
            let scale = monitor.scale_factor();
            if let Ok(pos) = window.outer_position() {
                let y = pos.y as f64 / scale;
                let x = if edge == "right" {
                    monitor.position().x as f64 / scale + monitor.size().width as f64 / scale - 300.0
                } else {
                    monitor.position().x as f64 / scale
                };
                let _ = window.set_position(tauri::Position::Logical(
                    tauri::LogicalPosition::new(x, y),
                ));
            }
        }
        let _ = app.emit("clipboard://dock-changed", edge.as_str());
        crate::clipboard_dock::apply_dock_collapsed(&window);
    }
}
```

- [x] **Step 2: 解吸附检测（Docked 态拖拽恢复 Normal）**

在 Moved 事件 handler 中，如果当前 docked 但窗口已远离边缘 → 清除 dock：

```rust
tauri::WindowEvent::Moved(_) => {
    crate::window_position::save_current_position(&win_clone, WINDOW_LABEL);

    // 先检测是否有新吸附
    if let Some(edge) = detect_dock_edge(&win_clone) {
        if edge != "none" {
            crate::window_position::save_dock_state(WINDOW_LABEL, edge);
            let _ = app_clone.emit("clipboard://dock-changed", edge);
            log::info!("clipboard docked to {}", edge);
            return; // 吸附了，不检测解吸附
        }
    }

    // 如果之前 docked 但现在不在边缘 → 解吸附
    let prev_dock = crate::window_position::load_dock_state(WINDOW_LABEL);
    if let Some(ref prev) = prev_dock {
        if prev == "right" || prev == "left" {
            // 不在边缘了 → 清除 dock
            crate::window_position::save_dock_state(WINDOW_LABEL, "none");
            crate::clipboard_dock::apply_dock_expanded(&win_clone);
            let _ = app_clone.emit("clipboard://dock-changed", "none");
            log::info!("clipboard undocked");
        }
    }
}
```

- [x] **Step 3: 编译 + 手动测试**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`

手动测试：
1. 拖到右边缘 → 应自动收缩
2. 关闭窗口 → 快捷键重新唤出 → 应以收缩态打开
3. 拖离边缘 → 应恢复 Normal

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/clipboard_window.rs
git commit -m "feat(clipboard-dock): 打开时恢复 dock 状态 + 解吸附检测"
```

---

### Task 7: 文档更新（action_bar 定位策略明确 + architecture.md）

**Files:**
- Modify: `docs/superpowers/specs/2026-07-08-action-bar-design.md` §4.1
- Modify: `docs/architecture.md` 窗口管理表

- [x] **Step 1: action_bar-design.md §4.1 加定位策略说明**

在 §4.1 `action_bar_window` 表格后加一段：

```markdown
> **定位策略（不变）**：action_bar_window 每次唤出定位在鼠标光标上方，不做位置记忆 / 边缘吸附 / 拖拽 / 尺寸变更。这是 action_bar 的设计选择——它是一个瞬态操作面板（选中→操作→消失），不需要常驻。位置记忆和吸附功能仅适用于剪贴板浮窗（`clipboard_window`）等常驻窗口，详见 [clipboard-dock spec](./2026-07-10-clipboard-dock-design.md)。
```

- [x] **Step 2: architecture.md 窗口管理表更新**

在 `clipboard_window` 行末尾追加（现有描述很长，在末尾加）：

```
。**边缘吸附（2026-07-10）**：拖到屏幕边缘 ≤10px 自动吸附收缩为 8px 细条（`window_dock.clipboard_window` 持久化），hover 展开，点击外部收回。窗口物理尺寸不变（300×600），收缩靠 CSS 隐藏 + `ignoresMouseEvents`。详见 [spec](superpowers/specs/2026-07-10-clipboard-dock-design.md)。
```

在 `action_bar_window` 行（§9 描述）末尾补注：

```
。**定位策略固定**：鼠标上方定位，不做位置记忆 / 吸附 / 拖拽 / 尺寸变更（瞬态操作面板，非常驻窗口）。
```

- [x] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-07-08-action-bar-design.md docs/architecture.md
git commit -m "docs: 明确 action_bar 固定定位 + clipboard dock 架构记录"
```

---

## Spec Coverage 检查

| Spec 章节 | 对应 Task |
|-----------|----------|
| §1 背景与动机 | —（设计文档，无需实现） |
| §2 核心方案 | Task 3+4（NSWindow 操作 + CSS） |
| §3 状态机 | Task 2+5+6（吸附检测 + 展开/收缩命令 + 恢复） |
| §4 吸附检测 | Task 2 |
| §5 前端交互 | Task 4 |
| §6 NSWindow 交互 | Task 3+5 |
| §7 文件变更 | Task 1-7 全覆盖 |
| §8 不变式 | Global Constraints 覆盖 |
| §9 边界场景 | Task 6 覆盖（恢复 + 解吸附 + 多显示器） |

---

## 与原 plan 的偏差

1. **穿透方案三次迭代**——plan 原写 NSWindow `setIgnoresMouseEvents` + NSTrackingArea，实际实现经 v1（CSS pointer-events 不穿透）→ v2（CGEvent 轮询，run_on_main_thread 调度延迟）→ **v3 终版**（Tauri `cursor_position()` + tokio interval 33ms，与 result_window 统一）。
2. **DockBar.tsx 未独立**——细条内联在 `index.tsx`（`absolute` 定位），未创建独立组件文件。
3. **状态机简化**——原 5 态（none/collapsed/expanding/expanded/collapsing）简化为 3 态（none/collapsed/expanded），动画用 CSS transition 而非显式 expanding/collapsing 状态。
4. **DOCK_EXPANDED 原子状态**——原 plan 依赖 `is_focused()`，实际用 `AtomicBool DOCK_EXPANDED` 作为 Rust 侧真相源（macOS 收缩态焦点不可靠）。
5. **收缩触发改为失焦**——原 plan 写 NSEvent global monitor 监听外部点击，实际简化为 `Focused(false)` 事件触发收缩。
6. **展开触发加 onMouseDown fallback**——macOS 非 key window 不交付 hover，需点击作为 fallback。
7. **POLL_ID 防竞态**（审查修复）——`AtomicU64` 自增保证同时只有一个轮询线程，旧线程自动退出。
8. **吸附态防重入**（审查修复）——已吸附同 edge 收缩态时 Moved 跳过 save_dock/start_poll，防高频 DB 写 + 线程重建。
9. **解吸附重置 DOCK_EXPANDED**（审查修复）——undocked 分支补 `DOCK_EXPANDED.store(false)`。
10. **窗口隐藏不空转**（审查修复）——`is_visible()` 只保护 `start_edge_poll`，不阻断状态重置。
11. **非 macOS cfg gate**（审查修复）——Moved/Focused/create 的 dock 逻辑全部 `#[cfg(target_os = "macos")]`。
12. **位置保存秒级节流**（审查优化）——`LAST_SAVE_SEC: AtomicI64`，同一秒最多写 1 次；失焦时无视节流强制兜底写。
13. **多屏横跳防护**（审查修复）——已吸附某 edge 收缩态时不切换到另一个 edge。
