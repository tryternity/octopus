# 窗口焦点追踪 + 自动粘贴设计

**日期**: 2026-06-26
**状态**: 设计中
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 1. 背景

剪贴板历史窗口双击条目时，需要把内容粘贴到"弹出剪贴板窗口之前的那个前台应用"（如编辑器、聊天框）。当前实现 `hide() + sleep(200ms) + Cmd+V` 不可靠——窗口隐藏后焦点回到哪里由系统决定，不确定。

参考 EcoPaste 的 `eco-paste` 插件实现（~393 行 Rust，42% unsafe），三平台各有独立的焦点监听 + 恢复 + 模拟粘贴机制。

## 2. 架构

### 2.1 新增模块：`crates/desktop/src/focus_tracker.rs`

独立模块，封装三平台的"记住上一个前台窗口"逻辑。在应用启动时开始监听，记录非自身窗口的前台窗口标识。

```rust
pub struct FocusTracker {
    // 各平台存储上一个前台窗口的标识
    // macOS: PID (i32)
    // Windows: HWND (isize)
    // Linux: X11 Window (u64)
}

impl FocusTracker {
    /// 启动全局焦点监听（各平台独立线程）
    pub fn start(&self) -> Result<()>;
    /// 获取上一个前台窗口标识（粘贴目标）
    pub fn previous_window(&self) -> Option<WindowId>;
    /// 把焦点恢复到上一个前台窗口
    pub fn restore_focus(&self) -> Result<()>;
}
```

### 2.2 各平台实现

#### macOS — NSWorkspace 通知

| 步骤 | 实现 |
|---|---|
| 监听 | `NSWorkspaceDidActivateApplicationNotification`（独立线程跑 NSRunLoop） |
| 存储 | `Mutex<Option<i32>>`（PID） |
| 过滤 | `app.localizedName == "octopus"` 跳过自身 |
| 恢复焦点 | 不主动恢复——靠 NSPanel resign。剪贴板窗口 `hide()` 时系统自动把 key window 还给上一个应用 |
| 粘贴 | `osascript` 执行 `tell application "System Events" to keystroke "v" using command down`（需辅助功能权限） |
| 依赖 | `objc` crate（动态注册 ObjC 类 `AppObserver`） |

**macOS 特殊优化**：如果剪贴板窗口用的是 NSPanel（非激活面板），`resign_key_window()` 后 macOS 自动还焦点。此时不需要 ObjC FFI 焦点追踪——只需 `hide()` + 200ms 延迟 + osascript 粘贴。**先验证此路径是否可靠，可靠则 macOS 跳过 PID 追踪**。

#### Windows — SetWinEventHook

| 步骤 | 实现 |
|---|---|
| 监听 | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`（回调在主线程消息泵） |
| 存储 | `Mutex<Option<isize>>`（HWND） |
| 过滤 | `GetWindowTextW(hwnd) == "octopus"` 跳过自身 |
| 恢复焦点 | `SetForegroundWindow(hwnd)` + `sleep(100ms)` |
| 粘贴 | enigo `Shift+Insert`（比 `Ctrl+V` 兼容性更好） |
| 依赖 | `windows` crate（Win32 API） |

#### Linux — X11 focus event

| 步骤 | 实现 |
|---|---|
| 监听 | `XSelectInput(root, FocusChangeMask)`（独立线程跑 `XNextEvent` 阻塞循环） |
| 存储 | `Mutex<Option<u64>>`（X11 Window） |
| 过滤 | `XGetInputFocus` 排除自身 + 读 `_NET_WM_NAME` 排除标题为 "octopus" 的窗口 |
| 恢复焦点 | `XRaiseWindow(win)` + `XSetInputFocus(win, RevertToNone, CurrentTime)` |
| 粘贴 | enigo `Shift+Insert`（Linux 上 enigo 已有 `try_linux_direct_typing` 兜底 wtype） |
| 依赖 | `x11rb` crate（X11 协议） |
| **限制** | **Wayland 不支持**——`XOpenDisplay` 在纯 Wayland 下返回 null → 监听静默失败 → 双击只复制不粘贴 |

### 2.3 粘贴流程（双击条目）

```
双击剪贴板条目
  → 前端调 invoke("paste_clipboard_item", { id })
  → 后端 paste_clipboard_item：
      1. 从 DB 按 id 读 content
      2. 写剪贴板（ClipboardHandle.write_text，设 suppress flag）
      3. hide() 剪贴板窗口
      4. focus_tracker.restore_focus()（Windows/Linux 主动恢复）
      5. sleep(100ms)（等焦点切换）
      6. 模拟粘贴（macOS: osascript / Windows+Linux: enigo Shift+Insert）
```

### 2.4 降级策略

| 场景 | 行为 |
|---|---|
| Linux 纯 Wayland（无 XWayland） | 焦点追踪不可用 → 双击只复制到剪贴板，不自动粘贴 |
| macOS 辅助功能权限未授权 | osascript 静默失败 → 只复制到剪贴板 |
| Windows 前台锁定阻止 SetForegroundWindow | 焦点恢复可能失败 → 粘贴可能进入错误窗口（可接受，用户可 Cmd+Z 撤销） |
| 焦点追踪线程启动失败 | `start()` 返回 Err → 日志记录，双击降级为只复制 |

## 3. 接口设计

### 3.1 Tauri 命令

```rust
/// 双击条目：写剪贴板 + 恢复焦点 + 模拟粘贴
#[tauri::command]
pub async fn paste_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String>
```

内部流程改为：写剪贴板 → hide → restore_focus → delay → simulate paste。

### 3.2 FocusTracker 初始化

在 `main.rs` setup 中：
```rust
let focus_tracker = Arc::new(FocusTracker::new());
match focus_tracker.start() {
    Ok(()) => { app.manage(focus_tracker); }
    Err(e) => { log::warn!("Focus tracker not available: {}", e); }
}
```

启动失败不阻断应用——双击降级为只复制。

## 4. 平台依赖

| 平台 | crate | octopus 是否已有 |
|---|---|---|
| macOS | `objc` / `cocoa`（ObjC FFI） | ❌ 新增（但 tauri-nspanel 已间接依赖） |
| macOS | `std::process::Command`（osascript） | ✅ 已有 |
| Windows | `windows`（Win32 API） | ❌ 新增 |
| Windows | `enigo`（键盘模拟） | ✅ 已有 |
| Linux | `x11rb`（X11 协议） | ❌ 新增（clipboard-rs 已间接依赖） |

## 5. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| macOS ObjC FFI 代码不正确导致 crash | 中 | 应用崩溃 | 参考 EcoPaste 成熟实现 + 充分测试 |
| Windows 前台锁定导致粘贴到错误窗口 | 中 | 用户体验差 | 可接受（Cmd+Z 可撤销） |
| Linux Wayland 完全不支持 | 确定 | 双击只复制 | 降级策略已覆盖 |
| 辅助功能权限引导缺失 | 低 | macOS 粘贴静默失败 | 首次双击时检测权限并提示 |
| 窗口标题匹配失效（标题被修改） | 低 | 自身窗口被误追踪 | 用 window label 而非标题匹配（Tauri API） |
