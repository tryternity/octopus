# 粘贴崩溃修复（enigo macOS SIGTRAP）设计

> 日期：2026-06-17
> 状态：✅ 已实现

## 现象

`paste_method: clipboard`（默认）模式下，识别完成触发粘贴时应用闪退，终端报：

```
Trace/BPT trap: 5
```

无 Rust panic backtrace、无 macOS 崩溃报告（`.ips`）。日志停在：

```
[paste] step 6: mod pressed, clicking V
./run-octopus.sh: line 23: <pid> Trace/BPT trap: 5
```

即崩溃发生在 `enigo.key(Key::Unicode('v'), Direction::Click)` 调用内部。

## 根因

enigo 0.6.1 在 macOS 上对 `Key::Unicode(c)` 的处理（`enigo/src/macos/macos_impl.rs:1005`）会调用 `get_layoutdependent_keycode(&c.to_string())`。该函数（`:1034`）循环 128 个 keycode，每个都调用 Carbon HIToolbox API：

- `TISCopyCurrentKeyboardInputSource()` / `TISCopyCurrentKeyboardLayoutInputSource()`
- `TISGetInputSourceProperty(.., kTISPropertyUnicodeKeyLayoutData)`
- `UCKeyTranslate(..)`

这些 HIToolbox API **非线程安全**。而粘贴在 `coordinator::do_paste` → `tauri::async_runtime::spawn` → `tokio::task::spawn_blocking` 的**非主线程**中执行（`coordinator.rs:696-697`），触发 macOS 线程断言 → **SIGTRAP**（`Trace/BPT trap: 5`）。

SIGTRAP 不是 Rust panic（不走 `std::panic::set_hook`），故无 backtrace；macOS 对 trap 进程不生成 `.ips` 崩溃报告，只能靠在组件边界插桩日志逐步二分定位。

> 为什么 `Key::Meta`（Cmd）不崩：Cmd 键在 enigo 的 `TryFrom<Key> for CGKeyCode` 中直接映射到固定 keycode `COMMAND=55`（`:1012`），不经过 `get_layoutdependent_keycode`，不触碰 Carbon layout API。

## 方案

macOS 上用**固定虚拟键码** `Key::Other(9)`（`kVK_ANSI_V = 0x09`）替代 `Key::Unicode('v')`。`Key::Other(u32)` 在 `try_from` 中（`:1006-1011`）直接作为 keycode 使用，绕过 `get_layoutdependent_keycode` → 不调用非线程安全的 Carbon API。

Linux / Windows 不受影响（它们的 `Key::Unicode` 处理是线程安全的），保留 `Key::Unicode('v')`。

### 注入点

`crates/desktop/src/paste.rs::paste_via_clipboard`（唯一使用 `Key::Unicode('v')` 的地方）：

```rust
#[cfg(target_os = "macos")]
let v_key = Key::Other(9);          // kVK_ANSI_V，绕过 Carbon layout 查找
#[cfg(not(target_os = "macos"))]
let v_key = Key::Unicode('v');
```

### 附带：panic hook

`crates/desktop/src/main.rs` 安装 `std::panic::set_hook`，把 panic 信息 + backtrace 同时打到 `log` 和 stderr。本 bug 是 SIGTRAP 不被捕获，但 hook 对未来 Rust panic 类故障（如 `unwrap` on None）有诊断价值，故保留。

## 验证

- `cargo check -p octopus-desktop --features embedded` 通过
- E2E：识别→粘贴→结果落地，无闪退（用户确认）

## 非目标

- 不升级 enigo（0.6→0.7 架构变动大，且非线程安全的 Carbon layout 查找在新版仍存在，根治需 enigo 在主线程做 keycode 解析或缓存）
- 不把粘贴移到主线程（会阻塞 UI，违反现有"粘贴异步化"架构）
- 不改 `paste_direct`（direct 模式用 `enigo.text()`，走的是 CGEvent Unicode payload 而非 keycode 映射，不受此 bug 影响）
