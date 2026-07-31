# ASR 文本回写 octopus 自己的 webview 窗口 — 设计规格

- **日期**：2026-07-31
- **类型**：bugfix（粘贴路径缺陷，涉及后端 + 前端）
- **范围**：ASR 识别结果无法粘贴到 octopus 自己的 webview 窗口（terminal / 图文编辑器 / 其他）
- **严重度**：P1——terminal 是刚上线功能，图文编辑器是老 bug，用户已反馈

## 问题

ASR 识别完成后，`do_paste` → `platform::paste::paste` → `paste_via_clipboard` 走三级 dispatch。
当前台 app 是 octopus 自己（用户聚焦在 terminal / compact_editor 等 webview 窗口）时：

1. `focus_tracker::save_frontmost_pid` 在 ASR toggle 入口缓存前台 pid，**但过滤自身**
   （`name != "octopus"`）→ `CACHED_PREV` 保持 None。
2. `paste_via_clipboard` 读 `cached_pid()` → None → fallback 到 `keystroke::paste()`
   → `CGEvent.post(HID)` 全局广播。
3. 全局广播的合成 Cmd+V **WKWebView 收不到**（与 Electron/Chromium 同理——代码注释已
   指出广播对非原生接收器不可靠）→ terminal 的 xterm / compact_editor 的 CM6 都不响应
   → 文本丢失。

## 根因

`focus_tracker` 设计假设「粘贴目标总是另一个 app」，过滤了 octopus 自身。但 terminal /
compact_editor 是 octopus 自己的 webview 窗口，需要**应用内文本注入**而非系统级键盘模拟。

## 设计：self-webview 文本注入分支

`paste_via_clipboard` 的 dispatch 加第 0 级：**前台是 octopus 自己的 webview 窗口时，
emit `paste-text` 事件到该窗口，前端各自处理**。

### dispatch 优先级（更新）

```
1. cached_pid 有值（外部 app）→ 现有三级 dispatch（osascript / paste_to_pid）
2. cached_pid 无值 → 检测聚焦的 octopus webview 窗口：
   2a. 有聚焦的 webview 窗口（terminal/compact_editor/settings/...）→ emit "paste-text" 到该窗口
   2b. 无聚焦 webview（异常）→ fallback 全局广播（兼容）
```

### 后端改动

#### `crates/desktop/src/platform/paste.rs`

`paste_via_clipboard` 加 self-webview 分支。需要 `AppHandle`（改签名 `paste()` 加参数，
或从 thread-local / 全局取）：

```rust
// cached_pid 无值时，检测 octopus 自己的聚焦 webview 窗口
if cached_pid().is_none() {
    if let Some(label) = focused_octopus_webview_label(app_handle) {
        // emit "paste-text" { text } 到该窗口（定向，不广播）
        let _ = app_handle.emit_to(&label, "paste-text", text);
        return Ok(());  // 前端处理，不走键盘模拟
    }
    // 无聚焦 webview → fallback 全局广播（现有 keystroke::paste()）
}
```

#### `focused_octopus_webview_label(app) -> Option<String>`

遍历 `app.webview_windows()`，找 `is_focused() == Ok(true)` 的窗口，返回其 label。
排除浮窗（instant_overlay / overlay / result_window——这些不接收粘贴）。

#### 签名变更

`paste()` / `paste_via_clipboard()` 需 `AppHandle`。从 `do_paste`（coordinator/paste.rs）
传入——`do_paste` 已有 `app_handle`。`platform::paste::paste` 加 `app: &AppHandle` 参数。
所有调用点同步更新（do_paste / clipboard 粘贴路径）。

### 前端改动

各 webview 窗口按需监听 `paste-text` 事件（payload: `{ text: string }`）：

#### terminal（`pages/Terminal/index.tsx`）

```ts
listen<string>("paste-text", (e) => {
  // 定向到当前窗口（同 terminal://new-tab 的 target 限定）
  pty.write(e.payload);  // 直接写 PTY——最可靠，绕过 xterm
}, { target: { kind: "webview_window", label: currentWindowLabel } });
```

注：直写 PTY 比 `term.paste()` 更可靠（不经 xterm 中转，shell 直接收到）。
多 tab 时写活跃 tab 的 PTY（前端已知 activePtyId）。

#### compact_editor（`pages/CompactEditor/index.tsx`）

```ts
listen<string>("paste-text", (e) => {
  // CM6 editor insert at cursor（活跃 tab）
  cmEditor.dispatch({ changes: { from: cmEditor.state.selection.main.from, insert: e.payload } });
});
```

#### 其他窗口（settings / clipboard 等）

暂不监听（用户未报）。事件 emit 后无人 listen = 静默丢弃（无副作用）。
后续按需扩展。

## 不变量

1. 外部 app 粘贴路径**完全不变**（cached_pid 有值走原 dispatch）
2. clipboard 粘贴路径（clipboard_window 双击粘贴）同样受影响——前台是 octopus webview
   时也走 emit。但 clipboard_window 是浮窗，通常前台是外部 app，暂可不改（后续验证）
3. terminal 直写 PTY 不经剪贴板（文本不污染用户剪贴板）——但 ASR 路径已写剪贴板
   （clipboard_history），此处只是注入方式不同，不影响历史记录

## 边界

- **聚焦窗口检测**：`WebviewWindow::is_focused()` 跨平台 API。若多个窗口都报 focused
  （异常），取第一个。无聚焦窗口（用户切到桌面）→ fallback 广播。
- **事件竞态**：emit 后后端立即 return，不等前端确认。若前端未 listen（旧版本前端），
  文本静默丢弃。可接受——新版本前端会 listen。
- **多 tab terminal**：前端写活跃 tab 的 PTY（`activePtyId`）。切 tab 后下次粘贴写新 tab。

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/desktop/src/platform/paste.rs` | `paste()` 加 `app: &AppHandle` 参数；`paste_via_clipboard` 加 self-webview emit 分支；新增 `focused_octopus_webview_label` |
| `crates/desktop/src/engine/coordinator/paste.rs` | `do_paste` 传 `app_handle` 给 `platform::paste::paste` |
| `crates/desktop/src/platform/paste.rs` 其他调用点 | clipboard 粘贴路径同步加 app 参数（或保留旧签名做 wrapper） |
| `crates/desktop/frontend/src/pages/Terminal/index.tsx` | listen "paste-text" → `pty.write(text)`（活跃 tab） |
| `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` | listen "paste-text" → CM6 insert |
| `docs/architecture.md` | 更新粘贴 dispatch 说明（加 self-webview 分支） |

## 验证

- cargo build + cargo test（paste 签名变更的调用点全更新）
- tsc（前端 listen 类型正确）
- e2e：① terminal 聚焦 + ASR → 文本写入 PTY ② compact_editor 聚焦 + ASR → CM6 插入
       ③ 外部 app 聚焦 + ASR → 原路径不变（回归）
