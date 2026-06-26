# 窗口焦点追踪 + 自动粘贴设计（存档）

**日期**: 2026-06-26
**状态**: ⏸️ 暂缓——自动粘贴方案不可靠，已回滚为"复制到剪贴板，用户手动 Cmd+V"
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 1. 目标

双击剪贴板历史条目时，自动把内容粘贴到"弹出剪贴板窗口之前的那个前台应用"（如编辑器、聊天框）。

## 2. 最终实现状态

**部分工作**：豆包/备忘录可自动粘贴；Sublime Text / 微信不可靠。

**当前决策**：回滚为单击复制到剪贴板（不关闭窗口），用户手动 Cmd+V 粘贴。后续视需求重新评估。

## 3. 踩过的坑（完整记录）

### 坑 1：窗口 hide 后焦点不自动回到上一个应用

**现象**：剪贴板窗口 `hide()` 后，Cmd+V 发到了 octopus 自身而非目标编辑器。

**根因**：octopus 是 macOS `Accessory` 激活策略（无 Dock 图标）。与 `Regular` 应用（如 result_window）不同，`Accessory` 应用的窗口 `hide()` 后 **macOS 不自动把焦点还给上一个前台应用**——焦点停在 octopus 进程上。

**对比**：ASR 识别结果粘贴（`paste::paste`）有效，是因为 result_window 是 `Regular` 策略（或至少有不同焦点行为）。

**解决**：需要主动用 osascript 检测前台是否 octopus，是则切到第一个非 octopus 的前台进程。

### 坑 2：enigo Cmd+V 在非主线程不生效

**现象**：enigo 模拟 Cmd+V（`Key::Meta` + `Key::Other(9)`）在 `std::thread::spawn` 线程里执行成功（无错误），但按键事件没有到达目标应用。

**根因**：不确定。可能是 enigo 的 CGEvent 注入在非主线程时，macOS 窗口服务器把事件投递给了错误的 key window。

**验证**：日志显示 `frontmost app = sublime_text`（焦点正确），但 Sublime Text 没收到 Cmd+V。

### 坑 3：osascript `keystroke` 需要 `activate` 才生效

**现象**：直接 `tell application "System Events" to keystroke "v" using command down` 在某些应用无效（备忘录也不行）。

**根因**：`keystroke` 需要 System Events 进程有权限向目标应用的 key window 注入事件。如果 key window 不在目标应用上（可能还挂在 octopus 的隐藏窗口），keystroke 发到了错误的地方。

**部分解决**：先 `activate` 目标应用再 keystroke。豆包/备忘录有效，但引入新问题。

### 坑 4：osascript `activate` 按进程名不可靠

**现象**：`tell application "sublime_text" to activate` 报错 `-1728`（不能获得 application "sublime_text"）。

**根因**：AppleScript 的 `application` 对象用**应用名**（如 "Sublime Text"）而非**进程名**（如 "sublime_text"）。两者经常不一致。

**尝试**：改用 `System Events` 的 `process` 对象 `set frontmost of p to true`（不经过 application name）——部分有效，但微信仍不工作。

### 坑 5：微信屏蔽 AppleScript keystroke

**现象**：osascript `keystroke "v"` 对微信返回成功，但微信没有粘贴。

**根因**：微信（Electron 应用）可能屏蔽了 AppleScript 的事件注入，或其输入框不在标准 key window 链上。

### 坑 6：tokio::task::spawn_blocking 阻塞命令池

**现象**：第一次双击粘贴成功，第二次及之后失效。

**根因**：`std::thread::sleep` 在 `tokio::task::spawn_blocking` 之前的 async 命令上下文里执行，阻塞了 Tauri 的命令线程池。

**解决**：改为 `std::thread::spawn`（非 tokio 调度）。

### 坑 7：query_history size:1 只返回最新一条

**现象**：`paste_clipboard_item` 按 id 查条目时，查不到目标。

**根因**：`QueryFilter { size: 1 }` 只返回最新的 1 条记录，目标 id 的条目不在其中。

**解决**：改为 `size: 1000` + `find by id`。

## 4. 各方案对比

| 方案 | 豆包 | 备忘录 | Sublime Text | 微信 | 复杂度 |
|---|---|---|---|---|---|
| enigo Cmd+V（非主线程） | ❌ | ❌ | ❌ | ❌ | 低 |
| osascript keystroke（无 activate） | ✅ | ✅ | ❌ | ❌ | 低 |
| osascript activate(进程名) + keystroke | ✅ | ✅ | ❌(-1728) | ❌ | 中 |
| osascript set frontmost(process) + keystroke | ✅ | ✅ | ❌ | ❌ | 中 |
| paste::paste（完整 ASR 路径） | ❌ | ❌ | ❌ | ❌ | 低 |

**结论**：没有一种方案能覆盖所有应用。AppleScript keystroke 对原生 macOS 应用（备忘录/豆包）有效，但对非标准应用（Sublime Text/微信/ Electron 应用）不可靠。

## 5. EcoPaste 的参考方案

EcoPaste 的 macOS 实现：
- **不追踪 PID**（PID 存了但从不用——死代码）
- **靠 NSPanel resign_key_window** 让系统自动还焦点
- **用 osascript keystroke** 模拟 Cmd+V

EcoPaste 的关键差异：它用的是 **NSPanel**（`tauri-nspanel`），而非普通 Tauri 窗口。NSPanel 的 `resign_key_window` 能让 macOS 窗口管理器可靠地还焦点。octopus 的剪贴板窗口是普通窗口（`WebviewWindowBuilder`），没有这个能力。

## 6. 后续重启条件

如果要重新实现自动粘贴，以下任一条件满足时值得尝试：

1. **改用 NSPanel**：剪贴板窗口用 `tauri-nspanel` 创建（非激活面板），`resign_key_window` 后系统自动还焦点——最干净的方案，但需要重写窗口创建代码。
2. **CGEvent 直接注入**：用 `CGEventCreateKeyboardEvent`（Core Graphics）替代 enigo/osascript——绕过 AppleScript 和 enigo 的中间层，直接在硬件事件层注入 Cmd+V。需要 `core-graphics` crate + unsafe FFI。
3. **等待 macOS API 改进**：如果未来 macOS 提供更可靠的非前台应用键盘注入 API。

## 7. 当前实现（回滚后）

- **单击**：复制到剪贴板，不关闭窗口（用户手动 Cmd+V 粘贴）
- **双击**：同单击（不自动粘贴）
- `focus_tracker.rs` 保留代码但不用于自动粘贴
- `paste_clipboard_item` 命令保留但不再被前端双击调用
