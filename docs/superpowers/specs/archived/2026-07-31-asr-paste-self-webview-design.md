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

#### 关键：在 toggle 入口缓存窗口 label，而非 paste 时检测

初版在 `paste_via_clipboard` 时调 `focused_octopus_webview_label(app)` 检测聚焦窗口，
但实测失效——**paste 瞬间 is_focused 已不可靠**：ASR 录音期间 `result_window`（toggle）
或 `instant_overlay`（PTT/hands-free）show 过程会改焦点，paste 时聚焦的可能不是
terminal 而是这些展示窗（被排除）→ None → fallback 广播 → 失败。

修复：在 `save_frontmost_pid`（toggle/PTT/hands-free 入口，**terminal 正聚焦的瞬间**）
捕获聚焦的自身 webview 窗口 label，存 `CACHED_SELF_WINDOW`。paste 时读此缓存。

#### `crates/desktop/src/platform/focus_tracker.rs`

- `save_frontmost_pid(app)` 改签名加 `AppHandle`：前台是自身时，调
  `focused_self_webview_label(app)` 找聚焦的非浮窗 webview 窗口 label，存 `CACHED_SELF_WINDOW`。
- 新增 `CACHED_SELF_WINDOW: Mutex<Option<String>>`。
- 新增 `cached_self_window() -> Option<String>`。
- `clear_cached_pid` 同时清 `CACHED_SELF_WINDOW`。
- `focused_self_webview_label(app)`：遍历 webview_windows，排除浮窗/指示窗/展示窗
  （instant_overlay/overlay/result_window/clipboard/pin/download/onboarding/settings），
  返回 `is_focused()` 的 label。

#### `crates/desktop/src/platform/paste.rs`

`paste_via_clipboard` 加 self-webview 分支：
```rust
if cached_pid().is_some() { /* 外部 app 三级 dispatch */ }
else if let Some(label) = cached_self_window() {
    app.emit_to(&label, "paste-text", text.to_string());  // 定向到 toggle 时缓存的窗口
    return Ok(());
} else { keystroke::paste()?; }  // fallback 全局广播
```
`paste()` 加 `app: &AppHandle` 参数。

#### 签名变更

`paste()` / `paste_via_clipboard()` 需 `AppHandle`。从 `do_paste`（coordinator/paste.rs）
传入——`do_paste` 已有 `app_handle`。`platform::paste::paste` 加 `app: &AppHandle` 参数。
所有调用点同步更新（do_paste / clipboard 粘贴路径）。

### 前端改动

各 webview 窗口按需监听 `paste-text` 事件（payload: `{ text: string }`）。
**关键实现细节**（e2e 调试后定稿）：
- target 必须用 `{ kind: "WebviewWindow", label }` 精确匹配，字符串（AnyLabel）不可靠
- listener 依赖项要稳定——`useTerminalSession` 返回的对象每次渲染新引用，放 effect deps
  会导致反复 unlisten/listen 间隙丢事件；用 `ref` 持有最新 session，effect 只挂一次

#### terminal（`pages/Terminal/TerminalPane.tsx`）

每个 pane 各自监听，仅 `active` pane 响应——直写 PTY（最可靠，绕过 xterm/键盘模拟）：
```ts
const sessionRef = useRef(session);
sessionRef.current = session;
useEffect(() => {
  if (!active) return;
  const currentLabel = getCurrentWebviewWindow().label;
  listen<string>("paste-text", (e) => {
    const s = sessionRef.current;
    if (s.ptyId != null) s.write(e.payload);  // 直写活跃 tab 的 PTY
  }, { target: { kind: "WebviewWindow", label: currentLabel } })
    .then((fn) => { /* unlisten */ });
}, [active]);  // 只依赖 active，不依赖 session（引用不稳定）
```

#### compact_editor（`pages/CompactEditor/MarkdownPane.tsx`）

```ts
listen<string>("paste-text", (e) => {
  const view = viewRef.current;
  if (!view) return;
  const sel = view.state.selection.main;
  view.dispatch({ changes: { from: sel.from, insert: e.payload } });  // CM6 光标处插入
}, { target: { kind: "WebviewWindow", label: currentLabel } });
```

#### 其他窗口（settings / clipboard 等）

暂不监听（用户未报）。事件 emit 后无人 listen = 静默丢弃（无副作用）。
后续按需扩展。

## 不变量

1. 外部 app 粘贴路径**完全不变**（cached_pid 有值走原 dispatch）
2. clipboard 粘贴路径（clipboard_window 双击粘贴）同样受影响——前台是 octopus webview
   时也走 emit。**e2e 验证通过（2026-07-31）**：clipboard_window 的 self-webview 场景正常。
3. terminal 直写 PTY 不经剪贴板（文本不污染用户剪贴板）——但 ASR 路径已写剪贴板
   （clipboard_history），此处只是注入方式不同，不影响历史记录
4. **terminal 多 tab**：ASR 回写活跃 tab 的 PTY。**e2e 验证通过（2026-07-31）**：多 tab
   切换 + 回写正常。

## 边界

- **聚焦窗口检测时机**：必须在 toggle 入口（save_frontmost_pid）缓存，paste 时已不可靠
  （见上「关键：在 toggle 入口缓存窗口 label」）。`focused_self_webview_label` 用
  `WebviewWindow::is_focused()`，排除浮窗/展示窗。
- **事件竞态**：emit 后后端立即 return，不等前端确认。若前端未 listen（旧版本前端），
  文本静默丢弃。可接受——新版本前端会 listen。
- **多 tab terminal**：前端写活跃 tab 的 PTY（active pane 的 session.write）。切 tab 后
  下次粘贴写新活跃 tab。
- **target 格式**：必须 `{ kind: "WebviewWindow", label }`，字符串（AnyLabel）实测不可靠。

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/desktop/src/platform/focus_tracker.rs` | `save_frontmost_pid(app)` 加参数；前台是自身时缓存窗口 label 到 `CACHED_SELF_WINDOW`；新增 `cached_self_window()` + `focused_self_webview_label()`；`clear_cached_pid` 同步清 |
| `crates/desktop/src/platform/paste.rs` | `paste()` 加 `app: &AppHandle` 参数；`paste_via_clipboard` 加 self-webview emit 分支（读 `cached_self_window()`） |
| `crates/desktop/src/engine/coordinator/paste.rs` | `do_paste` 传 `app_handle_emit` 给 `platform::paste::paste` |
| `crates/desktop/src/engine/coordinator/mod.rs` | 3 处 `save_frontmost_pid(app_handle)` 传参（Toggle / InstantStart / HandsFreeStart） |
| `crates/desktop/src/clipboard/clipboard_window.rs` | 3 处 `save_frontmost_pid(app)` 传参 |
| `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx` | listen "paste-text" → `session.write(text)`（active pane，sessionRef 稳定化） |
| `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx` | listen "paste-text" → CM6 光标处 insert |
| `docs/architecture.md` | 更新粘贴 dispatch 说明（加 self-webview 分支） |

## 实现状态（2026-07-31 完成，e2e 验证基本通过）

### 已实现

- ✅ **focus_tracker 缓存自身窗口 label**：`save_frontmost_pid(app)` 前台是 octopus 自己时，
  经 `focused_self_webview_label` 找聚焦的非浮窗 webview 窗口 label，存 `CACHED_SELF_WINDOW`。
- ✅ **paste self-webview 分支**：`cached_pid()` 为 None 时读 `cached_self_window()`，
  `emit_to(label, "paste-text", text)` 定向到该窗口。
- ✅ **TerminalPane listener**：active pane 监听 paste-text，直写 PTY（sessionRef 稳定化，
  避免引用不稳定反复 unlisten/listen）。
- ✅ **MarkdownPane listener**：非只读时监听，CM6 光标处 insert。
- ✅ **target 精确匹配**：`{ kind: "WebviewWindow", label }`（非字符串 AnyLabel）。

### 偏差与决策

1. **缓存时机从 paste 时改为 toggle 时**（关键修复）：初版在 paste 时调
   `focused_octopus_webview_label(app)` 检测聚焦窗口，但 paste 瞬间 is_focused 已不可靠
   （result_window/instant_overlay show 改焦点）。改为 toggle 入口缓存窗口 label。
   `focused_octopus_webview_label` 函数已从 paste.rs 删除（逻辑移到 focus_tracker 的
   `focused_self_webview_label`，在缓存时调用）。

2. **listener 依赖项稳定化**：`useTerminalSession` 返回对象每次渲染新引用，放 effect deps
   导致反复 unlisten/listen 间隙丢事件。用 `sessionRef` 持有最新 session，effect 只依赖
   `[active]`。

3. **target 格式**：字符串（AnyLabel）实测对 WebviewWindow 不可靠，改为
   `{ kind: "WebviewWindow", label }` 精确匹配。

### 验证

- `cargo build -p octopus-desktop --features embedded` ✅ 0 error 0 warning
- `cargo test -p octopus-desktop --features embedded` ✅ 488 passed
- `npx tsc --noEmit` ✅ 0 error
- e2e 实测：terminal + 图文编辑器 ASR 回写基本通过（用户确认「基本功能都 ok」）

## 验证

- cargo build + cargo test（paste 签名变更的调用点全更新）
- tsc（前端 listen 类型正确）
- e2e：① terminal 聚焦 + ASR → 文本写入 PTY ② compact_editor 聚焦 + ASR → CM6 插入
       ③ 外部 app 聚焦 + ASR → 原路径不变（回归）
