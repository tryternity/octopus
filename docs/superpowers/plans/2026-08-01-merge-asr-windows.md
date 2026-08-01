# 合并 result_window 与 instant_overlay 为单 WebView 实例 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把两个透明穿透窗（`result_window` + `instant_overlay`）合并为一个 WebView 实例，按录音模式在前端切换视图，减少一份 WebView 内存。

**Architecture:** `result_window`（720×480 固定透明窗）作为唯一实例，吸收 `instant_overlay` 的指示逻辑。后端按 `INSTANT_MODE` 决定位置（toggle 顶部 / instant 贴底）+ emit `record-mode` 事件；前端 React 在同一页面内 `display:none` 切换 toggle 视图（现有 Result）与 instant 视图（搬入的 InstantOverlay）。穿透 poller 按模式切换可交互区（顶部 BAR / 底部指示卡）。

**Tech Stack:** Rust + Tauri 2 + React + CodeMirror 6 + xterm-free（指示卡复用 InstantOverlay 组件）

## Global Constraints

- 物理窗口尺寸**固定 720×480**，不运行时 `setSize`（透明无边框窗口 setSize 被 NSWindow 拒绝，已踩坑多次）
- 视图切换用 `display:none`（不卸载组件），保留 CM6 编辑状态
- 窗口位置仅**首次显示**（从不可见到可见）时 reposition（已有逻辑，instant 态遵守）
- `INSTANT_MODE: AtomicBool` 语义变（选窗口 → 选视图+位置），仍由 InstantStart/HandsFreeStart 设 true、PasteDone/Cancel 清 false
- 外部 app 粘贴路径、ASR 回写（paste-text）、全局快捷键不受影响
- `result_window` label 保持不变（paste-text 路径的 `focused_self_webview_label` 排除清单不变）

**Spec:** `docs/superpowers/specs/2026-08-01-merge-asr-windows-design.md`

---

## File Structure

**保留并扩展：**
- `crates/desktop/src/ui/result_window.rs` — 唯一窗口主体，吸收 instant 逻辑（`show_instant` + 底部位置 + emit record-mode + poller 双 BAR）
- `crates/desktop/frontend/src/pages/Result/` — 改造为 AsrWindow 根，挂载 instant 视图分支

**新建：**
- `crates/desktop/frontend/src/pages/Result/InstantView.tsx` — 从 InstantOverlay 搬来的指示卡组件（适配 720×480 底部居中渲染）

**删除：**
- `crates/desktop/src/ui/instant_overlay.rs`
- `crates/desktop/frontend/src/pages/InstantOverlay/`（搬到 InstantView）
- `crates/desktop/frontend/instant-overlay.html`

**修改（调用点迁移）：**
- `crates/desktop/src/engine/coordinator/{mod,session,polish,paste,lifecycle,cancel_discard}.rs` — `instant_overlay::show/hide` → `result_window::show_instant/hide_result`
- `crates/desktop/src/core/setup.rs` — 删 `instant_overlay::precreate`
- `crates/desktop/capabilities/default.json` — 删 `instant_overlay` window 权限

---

## Task 1: 后端 result_window 加 `show_instant` + 位置策略 + emit record-mode

**Files:**
- Modify: `crates/desktop/src/ui/result_window.rs`

**Interfaces:**
- Produces: `pub fn show_instant(app: &tauri::AppHandle, state: &str, text: &str)` — show 窗口 + emit `instant-state {state, text}` + 按 instant 模式设底部位置 + emit `record-mode: "instant"`
- Produces: `show_result` 改为先 emit `record-mode: "toggle"` 再走原逻辑
- Produces: `INSTANT_BAR_H: f64 = 80.0`（底部指示卡高度，poller 用）

- [x] **Step 1: 加 show_instant 函数 + record-mode emit**

在 `result_window.rs` 的 `show_result` 之后加 `show_instant`。先读 `show_result` 现状（line 228-255）作模板。`show_instant` 结构同 `show_result`，但：
- emit `record-mode: "instant"`（show 前）
- emit `instant-state` 而非 `show-result`（payload `{state, text}`）
- 位置用底部贴底（Step 2 加的 `position_bottom_center`）

在 `show_result` 开头（`window.show()` 前）加 emit `record-mode: "toggle"`：
```rust
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    let _ = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let need_emit = { /* 原逻辑不变 */ };
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let was_visible = window.is_visible().unwrap_or(false);
        if !was_visible {
            reposition_to_mouse_monitor(&window);
        }
        // 新增：toggle 模式 emit record-mode（仅首次显示时，避免重复 emit）
        if !was_visible {
            let _ = app.emit_to(WINDOW_LABEL, "record-mode", "toggle");
        }
        let _ = window.show();
        if need_emit {
            let _ = app.emit_to(WINDOW_LABEL, "show-result", text);
        }
    }
}
```

新增 `show_instant`（紧接 `show_result` 后）：
```rust
/// instant 模式（PTT/hands-free）show 窗口：emit instant-state + 底部定位 + record-mode。
pub fn show_instant(app: &tauri::AppHandle, state: &str, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let was_visible = window.is_visible().unwrap_or(false);
        if !was_visible {
            position_bottom_center(&window);
            let _ = app.emit_to(WINDOW_LABEL, "record-mode", "instant");
        }
        let _ = window.show();
        let _ = app.emit_to(WINDOW_LABEL, "instant-state", serde_json::json!({ "state": state, "text": text }));
    }
}
```

- [x] **Step 2: 加 position_bottom_center（从 instant_overlay 搬）**

在 `reposition_to_mouse_monitor` 附近加。从 `instant_overlay.rs:99-128` 的 `position_bottom_center` 搬入，改用 result_window 的常量（窗口宽 720，指示卡底部居中）。需要 import `crate::ui::window_position::{get_mouse_location, find_monitor_at_mouse}`（已在 reposition 用过）：
```rust
/// instant 模式定位：窗口底部贴鼠标所在屏底（指示卡在 720×480 透明区底部居中）。
fn position_bottom_center(win: &tauri::WebviewWindow) {
    let app = win.app_handle();
    let mouse = crate::ui::window_position::get_mouse_location();
    // 用 RESULT_WIDTH 居中；指示卡底部贴屏底（窗口底边 = 屏底 - BOTTOM_MARGIN）
    const INSTANT_BOTTOM_MARGIN: f64 = 8.0;
    if let Some((_did, ox, oy, w, h)) = crate::ui::window_position::find_monitor_at_mouse(mouse) {
        let x = ox + (w - RESULT_WIDTH) / 2.0;
        // 窗口底边贴屏底：窗口 y = 屏底 - 窗口高(480) - margin
        let y = oy + h - RESULT_HEIGHT - INSTANT_BOTTOM_MARGIN;
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        return;
    }
    // fallback：primary monitor
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let pos = m.position();
        let size = m.size();
        let x = (size.width as f64 / scale - RESULT_WIDTH) / 2.0;
        let y = pos.y as f64 / scale + (size.height as f64 / scale - RESULT_HEIGHT - INSTANT_BOTTOM_MARGIN);
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
    }
}
```

- [x] **Step 3: 加 INSTANT_BAR_H 常量（poller 用，Task 3）**

在 `BAR_H` 附近加：
```rust
/// instant 模式底部指示卡高度（穿透 poller 用，Task 3）。
const INSTANT_BAR_H: f64 = 80.0;
```

- [x] **Step 4: 加 imports（serde_json / Emitter 如缺）**

确认 `use tauri::Emitter;` 已在 result_window.rs（show_result 用了 emit_to）。`serde_json::json!` 用于 instant-state payload——确认 `serde_json` 在 desktop crate 依赖（已在）。

- [x] **Step 5: build 验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning"`
Expected: 无 error/warning（show_instant 暂未被调用，可能有 dead_code warning——Task 2 调用点迁移后消除）

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/ui/result_window.rs
git commit -m "feat(result_window): 加 show_instant + 底部定位 + record-mode emit"
```

---

## Task 2: 后端调用点迁移 instant_overlay → result_window + 删除 instant_overlay.rs

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/{mod,session,polish,paste,lifecycle,cancel_discard}.rs`
- Modify: `crates/desktop/src/core/setup.rs`
- Delete: `crates/desktop/src/ui/instant_overlay.rs`
- Modify: `crates/desktop/src/ui/mod.rs`（删 `pub mod instant_overlay;`）

**Interfaces:**
- Consumes: `result_window::show_instant(state, text)` / `result_window::hide_result`（Task 1 产出）

调用点替换规则（精确对照 grep 结果）：
- `instant_overlay::show_instant_overlay(app, state, text)` → `result_window::show_instant(app, state, text)`（5 处：session.rs:31,71 / polish.rs:61 / paste.rs:93,127 / lifecycle.rs:368,488）
- `instant_overlay::hide_instant_overlay(app)` → `result_window::hide_result(app)`（5 处：cancel_discard.rs:87,253 / mod.rs:554 / lifecycle.rs:396,462）
- `instant_overlay::precreate(app)` → 删除（setup.rs:62）

- [x] **Step 1: 迁移 session.rs 两处**

`crates/desktop/src/engine/coordinator/session.rs`:
- line 31: `crate::ui::instant_overlay::show_instant_overlay(app_handle, "listening", "");` → `crate::ui::result_window::show_instant(app_handle, "listening", "");`
- line 71: `crate::ui::instant_overlay::show_instant_overlay(app_handle, "done", "麦克风不可用");` → `crate::ui::result_window::show_instant(app_handle, "done", "麦克风不可用");`

- [x] **Step 2: 迁移 polish.rs 一处**

`crates/desktop/src/engine/coordinator/polish.rs`:
- line 61: `crate::ui::instant_overlay::show_instant_overlay(app_handle, "polishing", "");` → `crate::ui::result_window::show_instant(app_handle, "polishing", "");`

- [x] **Step 3: 迁移 paste.rs 两处**

`crates/desktop/src/engine/coordinator/paste.rs`:
- line 93: `crate::ui::instant_overlay::show_instant_overlay(app_handle, "polishing", "");` → `crate::ui::result_window::show_instant(app_handle, "polishing", "");`
- line 127: `crate::ui::instant_overlay::show_instant_overlay(app_handle, "done", text_to_paste);` → `crate::ui::result_window::show_instant(app_handle, "done", text_to_paste);`

- [x] **Step 4: 迁移 lifecycle.rs 三处 show + 两处 hide**

`crates/desktop/src/engine/coordinator/lifecycle.rs`:
- line 368: `show_instant_overlay(app_handle, "polishing", "")` → `result_window::show_instant(app_handle, "polishing", "")`
- line 396: `hide_instant_overlay(app_handle)` → `result_window::hide_result(app_handle)`
- line 462: `hide_instant_overlay(app_handle)` → `result_window::hide_result(app_handle)`
- line 488: `show_instant_overlay(app_handle, "polishing", "")` → `result_window::show_instant(app_handle, "polishing", "")`

- [x] **Step 5: 迁移 cancel_discard.rs 两处 hide**

`crates/desktop/src/engine/coordinator/cancel_discard.rs`:
- line 87: `crate::ui::instant_overlay::hide_instant_overlay(app_handle);` → `crate::ui::result_window::hide_result(app_handle);`
- line 253: 同上

- [x] **Step 6: 迁移 mod.rs 一处 hide（PasteDone 分支）**

`crates/desktop/src/engine/coordinator/mod.rs`:
- line 554: `crate::ui::instant_overlay::hide_instant_overlay(&app_handle);` → `crate::ui::result_window::hide_result(&app_handle);`

- [x] **Step 7: 删 setup.rs precreate 调用**

`crates/desktop/src/core/setup.rs`:
- line 62: 删 `crate::ui::instant_overlay::precreate(self.app.handle());`

- [x] **Step 8: 删 instant_overlay.rs + mod.rs 声明**

- 删文件 `crates/desktop/src/ui/instant_overlay.rs`
- `crates/desktop/src/ui/mod.rs`: 删 `pub mod instant_overlay;`

- [x] **Step 9: build 验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning"`
Expected: 0 error 0 warning（所有调用点已迁移，instant_overlay 删除后无残留引用）

- [x] **Step 10: cargo test 验证**

Run: `cargo test -p octopus-desktop --features embedded 2>&1 | tail -3`
Expected: 全部通过（488+ passed，0 failed）

- [x] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor: 迁移 instant_overlay 调用点到 result_window + 删除 instant_overlay.rs"
```

---

## Task 3: 穿透 poller 适配 instant 模式（底部 BAR 区域）

**Files:**
- Modify: `crates/desktop/src/ui/result_window.rs`（poller 主体 line 134-205）

**Interfaces:**
- Consumes: `INSTANT_MODE`（coordinator/mod.rs 的 `pub(crate) static INSTANT_MODE`）+ `INSTANT_BAR_H`（Task 1）

现有 poller（line 195-205 区域）读 `RESULT_CLICK_THROUGH` + 用 `BAR_W`/`BAR_H`/`BAR_OFFSET_X` 算顶部小条矩形判光标命中。需按模式切两组坐标：
- toggle 精简态（`RESULT_CLICK_THROUGH=true` + `INSTANT_MODE=false`）：顶部 720×116（现状）
- instant 态（`INSTANT_MODE=true`）：底部 720×80（指示卡区域）

- [x] **Step 1: 抽 BAR 区域计算为按模式返回 (offset_x, offset_y, bar_w, bar_h)**

在 poller 的光标命中判定处（line ~195），把固定的 `BAR_OFFSET_X` / `BAR_H` 改为按模式：
```rust
// 按模式决定可交互区（顶部 toggle 小条 / 底部 instant 指示卡）
let (bar_off_x, bar_off_y, bar_h) = if crate::engine::coordinator::INSTANT_MODE
    .load(std::sync::atomic::Ordering::Relaxed)
{
    // instant：底部指示卡，水平居中（指示卡 400 宽，但可交互区放宽到窗口宽 720 便于点击）
    (BAR_OFFSET_X, RESULT_HEIGHT - INSTANT_BAR_H, INSTANT_BAR_H)
} else {
    // toggle 精简态：顶部小条
    (BAR_OFFSET_X, 0.0, BAR_H)
};
```
然后光标命中判定用 `bar_off_x` / `bar_off_y` / `bar_h`（替换原硬编码 `0.0` 偏移 + `BAR_H`）。

- [x] **Step 2: 确认 INSTANT_MODE 可见性**

`coordinator/mod.rs` 的 `INSTANT_MODE` 是 `pub(crate)`——result_window 在同 crate，可直接 `crate::engine::coordinator::INSTANT_MODE`。如不可见，改 `pub(crate)`。

- [x] **Step 3: build 验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning"`
Expected: 0 error 0 warning

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/ui/result_window.rs
git commit -m "feat(result_window): poller 适配 instant 模式底部 BAR 区域"
```

---

## Task 4: 前端 AsrWindow 根 + InstantView 组件 + record-mode 切换

**Files:**
- Create: `crates/desktop/frontend/src/pages/Result/InstantView.tsx`
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（加 record-mode 监听 + instant 视图分支）
- Delete: `crates/desktop/frontend/src/pages/InstantOverlay/`
- Delete: `crates/desktop/frontend/instant-overlay.html`

**Interfaces:**
- Consumes: 后端 `record-mode` 事件（`"toggle" | "instant"`）+ `instant-state` 事件（`{state, text}`）

- [x] **Step 1: 新建 InstantView.tsx（从 InstantOverlay/index.tsx 搬）**

`crates/desktop/frontend/src/pages/Result/InstantView.tsx`——从 `InstantOverlay/index.tsx` 搬入组件逻辑（4 态指示 + 波形/spinner），但：
- 移除 `getCurrentWebviewWindow().listen`（监听上移到 AsrWindow 根，props 传入 state/text）
- 渲染容器改为 720×480 透明区底部居中（CSS `position:absolute; bottom:0; left:50%; transform:translateX(-50%); width:400px;`）
- 组件签名改为 `export function InstantView({ state, text }: { state: string; text: string })`

- [x] **Step 2: 改造 Result/index.tsx 为 AsrWindow 根**

在 Result page 根组件加：
- state: `recordMode: "toggle" | "instant"`（默认 "toggle"）
- 监听 `record-mode` 事件 → setRecordMode
- 监听 `instant-state` 事件 → 存到 `instantState` / `instantText`（供 InstantView）
- 渲染：toggle 视图（现有 Result 内容）与 InstantView 用 `display:none` 切换
```tsx
const [recordMode, setRecordMode] = useState<"toggle" | "instant">("toggle");
const [instantState, setInstantState] = useState("");
const [instantText, setInstantText] = useState("");
// useEffect 监听 record-mode / instant-state（现有 show-result 监听保留）
return (
  <div className="asr-window-root">
    <div style={{ display: recordMode === "toggle" ? "block" : "none" }}>
      {/* 现有 Result 全部内容 */}
    </div>
    <div style={{ display: recordMode === "instant" ? "block" : "none" }}>
      <InstantView state={instantState} text={instantText} />
    </div>
  </div>
);
```

- [x] **Step 3: 删 InstantOverlay page + instant-overlay.html**

- 删 `crates/desktop/frontend/src/pages/InstantOverlay/`
- 删 `crates/desktop/frontend/instant-overlay.html`

- [x] **Step 4: tsc 验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 0 error（InstantOverlay 删除后无残留引用；InstantView 类型正确）

- [x] **Step 5: vite build 验证**

Run: `cd crates/desktop/frontend && npm run build 2>&1 | tail -3`
Expected: build 成功

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(frontend): AsrWindow 合并 toggle + instant 视图（record-mode 切换）"
```

---

## Task 5: capabilities 清理 + 文档同步

**Files:**
- Modify: `crates/desktop/capabilities/default.json`
- Modify: `docs/architecture.md`

- [x] **Step 1: 删 instant_overlay window 权限**

`crates/desktop/capabilities/default.json` line 4 的 `windows` 数组：删 `"instant_overlay"`。

- [x] **Step 2: 更新 architecture.md**

更新窗口说明段：result_window 与 instant_overlay 合并为单实例；INSTANT_MODE 语义变更（选视图+位置而非选窗口）；穿透 poller 双 BAR 区域。

- [x] **Step 3: 全量验证**

Run:
```bash
cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"
cargo test -p octopus-desktop --features embedded 2>&1 | tail -3
cd crates/desktop/frontend && npx tsc --noEmit && npm run build 2>&1 | tail -3
```
Expected: build 0 error 0 warning；test 全过；tsc 0 error；vite build 成功

- [x] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: 删 instant_overlay capabilities + 文档同步"
```

- [x] **Step 5: spec 标记实现状态**

在 `docs/superpowers/specs/2026-08-01-merge-asr-windows-design.md` 末尾加「实现状态」段（已实现清单 + 偏差 + 验证结果）。

- [x] **Step 6: e2e 提示**

提示用户 e2e 验证：① toggle 录音→顶部 result（CM6 可编辑）② PTT→底部 instant 指示卡 ③ hands-free→底部指示卡 ④ 穿透：instant 态透明区可点穿 ⑤ toggle 精简/长篇态穿透不变 ⑥ ASR 回写外部窗口不受影响。

---

## Self-Review

**Spec coverage:**
- ✅ 单 WebView 实例（Task 2 删 instant_overlay）+ 720×480 固定（Global Constraint）
- ✅ 两视图切换（Task 4 record-mode + display:none）
- ✅ 位置策略 toggle 顶部 / instant 贴底（Task 1 show_instant + position_bottom_center）
- ✅ 穿透 poller 双 BAR（Task 3）
- ✅ INSTANT_MODE 语义变更（Task 1/3 读它）
- ✅ 调用点迁移 ~15 处（Task 2 逐文件列）
- ✅ 删除 instant_overlay.rs / InstantOverlay / instant-overlay.html（Task 2/4）
- ✅ capabilities 清理（Task 5）
- ✅ 不变量：外部粘贴/ASR 回写/快捷键不受影响（label 不变，无任务触碰这些路径）

**Placeholder scan:** 无 TBD/TODO；每个 Step 有具体代码或精确命令。

**Type consistency:** `show_instant(state: &str, text: &str)` 在 Task 1 定义、Task 2 调用一致；`InstantView({state, text})` Task 4 定义与使用一致；`INSTANT_BAR_H` Task 1 定义、Task 3 使用一致。
