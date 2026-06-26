# 窗口焦点追踪 + 自动粘贴 Implementation Plan（暂缓）

> **状态: ⏸️ 暂缓**——macOS 自动粘贴方案不可靠（Sublime Text/微信等应用不工作），已回滚为"双击复制到剪贴板，用户手动 Cmd+V"。Windows/Linux 焦点追踪不再实施。后续重启条件见 `specs/2026-06-26-focus-tracker-design-todo.md`。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 实现三平台（macOS/Windows/Linux）全局焦点追踪，双击剪贴板条目时自动恢复焦点到上一个前台应用并粘贴内容。

**Architecture:** 新增 `focus_tracker.rs` 模块封装平台特定 FFI。macOS 用 NSWorkspace 通知 + osascript 粘贴；Windows 用 SetWinEventHook + enigo；Linux 用 X11 focus event + enigo。降级策略：Wayland/权限不足时只复制不粘贴。

**Tech Stack:** Rust（objc / windows / x11rb crate）+ enigo + osascript

**Spec:** `docs/superpowers/specs/2026-06-26-focus-tracker-design.md`

---

## File Structure

| 文件 | 责任 |
|---|---|
| `crates/desktop/src/focus_tracker.rs` | 三平台焦点追踪 + 恢复 + 粘贴模拟（新增） |
| `crates/desktop/src/clipboard_commands.rs` | `paste_clipboard_item` 改用 focus_tracker（改） |
| `crates/desktop/src/main.rs` | setup 中初始化 FocusTracker（改） |
| `crates/desktop/Cargo.toml` | 加平台依赖（改） |

---

## Task 1: macOS 焦点追踪 + 粘贴

**Files:**
- Create: `crates/desktop/src/focus_tracker.rs`
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: Cargo.toml 加 macOS 依赖**

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc = "0.2"
cocoa = "0.26"
```

- [ ] **Step 2: 实现 macOS NSWorkspace 焦点监听**

参考 EcoPaste `macos.rs`：
- `observe_app()`：独立线程，动态注册 ObjC 类 `AppObserver`，监听 `NSWorkspaceDidActivateApplicationNotification`
- 回调：`notification.userInfo["NSWorkspaceApplicationKey"]` → `app.localizedName` → 过滤自身 → `app.processIdentifier` 存入 `static PREVIOUS_WINDOW: Mutex<Option<i32>>`
- 线程跑 `[[NSRunLoop currentRunLoop] run]` 阻塞

- [ ] **Step 3: 实现 macOS 粘贴**

```rust
#[cfg(target_os = "macos")]
pub fn simulate_paste() {
    use std::process::Command;
    let script = r#"tell application "System Events" to keystroke "v" using command down"#;
    let _ = Command::new("osascript").args(["-e", script]).output();
}
```

- [ ] **Step 4: 实现 macOS restore_focus**

macOS 不主动恢复——靠窗口 hide 后系统自动还焦点。验证 hide() + 200ms 是否够。

- [ ] **Step 5: 编译验证**

```bash
cargo check -p octopus-desktop
```

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/focus_tracker.rs crates/desktop/Cargo.toml
git commit -m "feat(desktop): macOS 焦点追踪（NSWorkspace 通知 + osascript 粘贴）"
```

---

## Task 2: Windows 焦点追踪 + 粘贴

**Files:**
- Modify: `crates/desktop/src/focus_tracker.rs`
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: Cargo.toml 加 Windows 依赖**

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.61", features = ["Win32_Foundation", "Win32_UI_WindowsAndMessaging", "Win32_Graphics_Gdi"] }
```

- [ ] **Step 2: 实现 Windows SetWinEventHook 焦点监听**

参考 EcoPaste `windows.rs`：
- `event_hook_callback`：`EVENT_SYSTEM_FOREGROUND` → `GetWindowTextW(hwnd)` → 过滤自身 → 存入 `static PREVIOUS_WINDOW: Mutex<Option<isize>>`
- `observe_app()`：`SetWinEventHook(..., WINEVENT_OUTOFCONTEXT)` 注册回调

- [ ] **Step 3: 实现 Windows restore_focus + 粘贴**

```rust
#[cfg(target_os = "windows")]
pub fn restore_focus(hwnd: isize) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(windows::Win32::Foundation::HWND(hwnd));
    }
    std::thread::sleep(Duration::from_millis(100));
}

#[cfg(target_os = "windows")]
pub fn simulate_paste() {
    let mut enigo = Enigo::new(&Settings::default()).unwrap();
    enigo.key(Key::Shift, Direction::Press).unwrap();
    enigo.key(Key::Other(0x2D), Direction::Click).unwrap(); // Insert
    enigo.key(Key::Shift, Direction::Release).unwrap();
}
```

- [ ] **Step 4: 编译验证**

```bash
cargo check -p octopus-desktop
```

- [ ] **Step 5: Commit**

---

## Task 3: Linux 焦点追踪 + 粘贴

**Files:**
- Modify: `crates/desktop/src/focus_tracker.rs`
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: Cargo.toml 加 Linux 依赖**

```toml
[target.'cfg(target_os = "linux")'.dependencies]
x11rb = "0.13"
```

- [ ] **Step 2: 实现 Linux X11 焦点监听**

参考 EcoPaste `linux.rs`：
- `observe_app()`：独立线程，`XOpenDisplay` → `XSelectInput(root, FocusChangeMask)` → `XNextEvent` 阻塞循环
- 回调：`XGetInputFocus` → 读 `_NET_WM_NAME` 过滤自身 → 存入 `static PREVIOUS_WINDOW: Mutex<Option<u64>>`
- `XOpenDisplay` 返回 null 时（Wayland）→ log error + 返回

- [ ] **Step 3: 实现 Linux restore_focus + 粘贴**

```rust
#[cfg(target_os = "linux")]
pub fn restore_focus(window: u64) {
    // XRaiseWindow + XSetInputFocus
}

#[cfg(target_os = "linux")]
pub fn simulate_paste() {
    let mut enigo = Enigo::new(&Settings::default()).unwrap();
    enigo.key(Key::Shift, Direction::Press).unwrap();
    enigo.key(Key::Insert, Direction::Click).unwrap();
    enigo.key(Key::Shift, Direction::Release).unwrap();
}
```

- [ ] **Step 4: 编译验证**

- [ ] **Step 5: Commit**

---

## Task 4: 集成到 paste_clipboard_item + main.rs

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: main.rs setup 中初始化 FocusTracker**

```rust
let focus_tracker = Arc::new(FocusTracker::new());
match focus_tracker.start() {
    Ok(()) => { app.manage(focus_tracker); log::info!("Focus tracker started"); }
    Err(e) => { log::warn!("Focus tracker not available: {}", e); }
}
```

- [ ] **Step 2: paste_clipboard_item 改用 focus_tracker**

```rust
#[tauri::command]
pub async fn paste_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
    handle: State<'_, Arc<ClipboardHandle>>,
    focus: State<'_, Arc<FocusTracker>>,
) -> Result<(), String> {
    // 1. 从 DB 读 content
    // 2. handle.write_text(&content)
    // 3. hide clipboard window
    if let Some(win) = app_handle.get_webview_window("clipboard_window") {
        let _ = win.hide();
    }
    // 4. 恢复焦点
    focus.restore_focus();
    // 5. 延迟
    std::thread::sleep(Duration::from_millis(200));
    // 6. 模拟粘贴
    focus.simulate_paste();
    Ok(())
}
```

- [ ] **Step 3: 编译验证**

```bash
cargo check -p octopus-desktop
```

- [ ] **Step 4: Commit**

---

## Task 5: 前端调整 + 测试

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1: 双击不再前端 hide + sleep**

前端 `handleDoubleClick` 改为只调 `invoke("paste_clipboard_item", { id })`——hide + restore_focus + paste 全在后端处理。

- [ ] **Step 2: 构建验证**

```bash
cd crates/desktop/frontend && npm run build
```

- [ ] **Step 3: 手动测试（macOS）**

1. 在编辑器中点击
2. Alt+V 唤起剪贴板
3. 双击一条文本
4. 验证文本出现在编辑器中（不是剪贴板窗口）

- [ ] **Step 4: Commit**
