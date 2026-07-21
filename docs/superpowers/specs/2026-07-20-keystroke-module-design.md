# 2026-07-20 Keystroke 基础能力模块 + restore_focus key window 修复

## 背景

`focus_tracker.rs` 的 `simulate_copy` / `simulate_paste` 用 osascript 发 Cmd+C/V，每次 ~200ms 启动开销。`paste.rs` 用 enigo（CGEvent 包装）。两套机制做同一件事。抽统一基础模块。

## 设计

### 模块位置

`crates/desktop/src/keystroke.rs`（与 input_source.rs / focus_tracker.rs 同级）。

放 desktop 而非 infra——infra 是纯逻辑 crate（无 platform-specific deps），加 CGEvent/enigo 会破坏其纯净性。

### 实现

用 `core-graphics` 0.24 的 CGEvent（项目已有依赖），不用 enigo——更底层、依赖更少、可控。

```rust
pub fn send_key_combo(modifier: KeyModifier, key_code: u8) -> Result<()>;
pub fn copy()  -> Result<()>;  // Cmd+C
pub fn paste() -> Result<()>;  // Cmd+V
pub fn cut()   -> Result<()>;  // Cmd+X
pub fn select_all() -> Result<()>;  // Cmd+A
```

核心实现：
```rust
let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)?;
let key_down = CGEvent::new_keyboard_event(source.clone(), key_code, true)?;
key_down.set_flags(flags);  // CGEventFlagCommand 等
key_down.post(CGEventTapLocation::HID);
// 同样发 keyUp
```

AX 权限检查 FFI（`AXIsProcessTrustedWithOptions`），防 CGEvent.post 静默失败。

## 改动

### 1. focus_tracker.rs
- `simulate_copy_platform` / `simulate_paste_platform` 改用 `crate::keystroke::copy()` / `paste()`
- `restore_focus_platform` **修复 key window 问题**：原仅当 octopus 是 frontmost 时切换——但 macOS frontmost app ≠ key window holder，hide clipboard_window 后目标 app 即便 frontmost，窗口可能不是 key window。改为**无条件 re-set frontmost** 触发 `windowDidBecomeKey`。
- `simulate_paste` 加 50ms sleep（IME guard 切输入源 Carbon TIS API 可能短暂抢焦点）

### 2. paste.rs
- `paste_via_clipboard` 的 macOS enigo 三段式改用 `crate::keystroke::paste()`
- 非 macOS 保留 enigo

### 3. main.rs
- 注册 `mod keystroke;`

## 实测

| 应用 | 粘贴 | 备注 |
|---|---|---|
| Sublime Text | ✅ | |
| iTerm2 | ✅ | |
| RustRover (IntelliJ) | ✅ | |
| 微信 | ✅ | |
| 豆包 | ❌ | Electron app，CGEvent 不触发其菜单快捷键 |

### 关键发现

**CGEvent 发 Cmd+V 需要 key window**——frontmost app ≠ key window holder。hide clipboard_window 后即便 Sublime 是 frontmost，窗口可能不是 key window，CGEvent 发的 Cmd+V 进了 NSApp.sendEvent 队列但不触发菜单快捷键匹配。`restore_focus` 改为无条件 re-set frontmost（触发 windowDidBecomeKey）后修复。

### Electron 兼容性

豆包（Chromium/Electron）不接收 CGEvent Cmd+V——Electron 事件处理路径跟原生 app 不同。可能需要 `CGEventPostToPid` 定向发送。Electron 已知问题，非本代码 bug。

## 已知限制 + 后续

- **restore_focus 仍用 osascript**（System Events），下一步实验 NSRunningApplication.activate
- **autotype activate_app**（scope 3）不在本次范围——autotype 的 osascript activate 不属于 keystroke 范畴
- **Electron 兼容**：需要时研究 CGEventPostToPid 或 osascript fallback
