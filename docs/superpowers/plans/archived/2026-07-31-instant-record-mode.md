# 录音模式（talk/PTT + hands-free）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`).

**Goal:** 新增 talk (PTT) 模式——按住说话、松开识别+粘贴，只读指示浮窗，与现有 toggle 模式并存。

**Architecture:** `handy-keys` crate 跨平台 keydown/keyup 监听 → coordinator `InstantStart`/`InstantStop` 新命令 → 跳过两阶段 prepare + result_window，用 instant 浮窗 → 复用引擎/润色/粘贴。

**Spec:** `docs/superpowers/specs/2026-07-31-instant-record-mode-design.md`

## Global Constraints

- **三模式命名**：toggle（现有不变）/ talk (PTT)（新增）/ hands-free（后续迭代）
- **首版只做 talk (PTT)**：hands-free 标 TODO
- **首版不做设置 UI**：seed 默认值写死（`record_mode = "toggle"`, `ptt_key = "AltRight"`）
- **toggle 模式完全不变**：所有现有行为/测试/UI 不受影响
- **talk 复用引擎/润色/粘贴**：不另起 ASR/polish/paste 流程
- **PasteDone 收尾不能省**：talk 也要走 DB Finalize + 回 Idle
- **权限**：handy-keys macOS 需「输入监控」权限；缺失降级 toggle
- **工作目录**：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730`

---

## Task 1: handy-keys 依赖 + PTT 模块骨架

**Files:**
- Modify: `crates/desktop/Cargo.toml`（加 `handy-keys` 依赖）
- Create: `crates/desktop/src/platform/ptt.rs`（PTT 监听骨架）
- Modify: `crates/desktop/src/platform/mod.rs`（`pub mod ptt;`）

**说明**：引入依赖 + PTT 监听骨架。先确认 handy-keys 在 octopus workspace 能编译。

- [x] **Step 1: 加依赖**

`crates/desktop/Cargo.toml` `[dependencies]` 加：
```toml
handy-keys = "0.3"
```

- [x] **Step 2: 创建 ptt.rs 骨架**

```rust
//! PTT（Push-to-Talk）按键监听——跨平台 keydown/keyup 全局监听。
//!
//! 用 handy-keys crate（Handy 同款），macOS 底层 CGEventTap 绕过
//! Tauri 插件只发 keydown 的限制。
//!
//! keydown → coordinator InstantStart
//! keyup   → coordinator InstantStop

use tauri::AppHandle;

/// 注册 PTT 键监听。
/// key: PTT 键名（如 "AltRight" / "ShiftRight" / "ControlRight" / "MetaRight"）。
///
/// 在独立线程持有 HotkeyManager（同 Handy handy_keys.rs manager_thread 模式），
/// 命令通过 mpsc channel 传递，避免 HotkeyManager 跨线程问题。
pub fn register_ptt(_app: &AppHandle, _key: &str) -> Result<(), String> {
    // TODO: 实现 HotkeyManager 线程 + keydown/keyup callback
    log::info!("[ptt] register_ptt: key={} (skeleton)", _key);
    Ok(())
}

/// 注销 PTT 键监听。
pub fn unregister_ptt(_app: &AppHandle) -> Result<(), String> {
    // TODO: 关闭 HotkeyManager 线程
    log::info!("[ptt] unregister_ptt (skeleton)");
    Ok(())
}
```

- [x] **Step 3: mod.rs 加 pub mod ptt**

`crates/desktop/src/platform/mod.rs` 加 `pub mod ptt;`

- [x] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|Finished" | tail -3`
Expected: Finished（0 error）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/platform/ptt.rs crates/desktop/src/platform/mod.rs
git commit -m "feat(desktop): handy-keys 依赖 + PTT 模块骨架"
```

---

## Task 2: PTT 监听实现（HotkeyManager 线程 + keydown/keyup）

**Files:**
- Modify: `crates/desktop/src/platform/ptt.rs`（实现 HotkeyManager + callback）

**说明**：实现真正的 PTT 监听。参考 Handy `handy_keys.rs` 的 manager_thread 模式。keydown → coordinator toggle() 开始录音，keyup → coordinator toggle() 停止录音。

注意：首版用 `coordinator.toggle()` 复用现有入口（而非新增 InstantStart/InstantStop），简化改动。PTT 模式下 toggle() 被快速连续调用（keydown + keyup），行为上等同「按一次开始 + 按一次停止」。后续如需 instant 专属路径（跳过 result_window），再新增 Command。

- [x] **Step 1: 实现 HotkeyManager 线程**

参考 Handy `src-tauri/src/shortcut/handy_keys.rs:88-184`：
- `HandyKeysState` 等价物：`Mutex<Sender<ManagerCommand>>` + `JoinHandle`
- manager_thread：`HotkeyManager::new_with_blocking()` → loop { try_recv events + recv_timeout commands }
- keydown (`HotkeyState::Pressed`) → `app.emit("ptt-keydown", ())` 或直接调 coordinator
- keyup (`HotkeyState::Released`) → `app.emit("ptt-keyup", ())` 或直接调 coordinator

- [x] **Step 2: register_ptt / unregister_ptt 实现**

```rust
pub fn register_ptt(app: &AppHandle, key: &str) -> Result<(), String> {
    // 启动 manager 线程（如未启动）
    // 注册 hotkey: key.parse::<Hotkey>() + manager.register(hotkey)
    // callback 判断 Pressed/Released → coordinator.toggle() 或 emit 事件
}

pub fn unregister_ptt(app: &AppHandle) -> Result<(), String> {
    // manager.unregister(hotkey_id)
}
```

- [x] **Step 3: 编译验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | tail -5`

- [x] **Step 4: Commit**

---

## Task 3: 配置字段 + seed

**Files:**
- Modify: `crates/infra/src/config.rs`（加 `record_mode` + `ptt_key`）
- Modify: `crates/infra/src/db.sql`（app_config seed）

- [x] **Step 1: config.rs 加字段**

```rust
/// 录音模式: "toggle"（默认）| "talk"（PTT 按住说话）
#[serde(default = "default_record_mode")]
pub record_mode: String,

/// talk 模式 PTT 键（默认 "AltRight"）
#[serde(default = "default_ptt_key")]
pub ptt_key: String,
```

加 default 函数 + Default impl 补两行。

- [x] **Step 2: db.sql seed**

```sql
('record_mode', 'toggle', '录音模式 toggle/talk'),
('ptt_key', 'AltRight', 'PTT 按键（右侧修饰键）'),
```

- [x] **Step 3: 编译 + 测试**

Run: `cargo test -p octopus-infra --lib 2>&1 | tail -3`

- [x] **Step 4: Commit**

---

## Task 4: setup.rs 快捷键注册分流

**Files:**
- Modify: `crates/desktop/src/core/setup.rs`

- [x] **Step 1: register_shortcuts 按 record_mode 分流**

```rust
fn register_shortcuts(&mut self) {
    // ... 现有 asr_shortcut / edit_global / polish_global 注册 ...
    
    // PTT 模式注册（record_mode == "talk"）
    if self.config.record_mode == "talk" {
        if let Err(e) = crate::platform::ptt::register_ptt(self.app.handle(), &self.config.ptt_key) {
            log::warn!("[ptt] 注册失败，降级 toggle: {}", e);
        }
    }
}
```

注意：talk 模式下 asr_shortcut 仍注册（用户可两者并用），或按需注销 asr_shortcut（首版保留两者）。

- [x] **Step 2: 编译**

- [x] **Step 3: Commit**

---

## Task 5: Instant 指示浮窗

**Files:**
- Create: `crates/desktop/src/ui/instant_overlay.rs`（窗口创建 + show/hide/update）
- Create: `crates/desktop/frontend/src/entries/instant-overlay-main.tsx`
- Create: `crates/desktop/frontend/src/pages/InstantOverlay/index.tsx`
- Create: `crates/desktop/frontend/instant-overlay.html`
- Modify: `crates/desktop/vite.config.ts`（加 entry）

**说明**：只读指示浮窗。参考 Handy `RecordingOverlay.tsx` 的状态流转。

- [x] **Step 1: Rust 窗口创建（instant_overlay.rs）**

```rust
pub const WINDOW_LABEL: &str = "instant_overlay";

pub fn show_instant_overlay(app: &AppHandle, state: &str, text: &str) {
    // 如不存在则创建（build_float_window，底部居中，transparent，focused: false）
    // show + emit_to("instant-state", { state, text })
}

pub fn hide_instant_overlay(app: &AppHandle) {
    // hide（不销毁，复用）
}
```

- [x] **Step 2: 前端页面**

`InstantOverlay/index.tsx`：
- listen `instant-state` 事件
- 状态：listening（波形动画）/ processing（spinner）/ polishing（spinner）/ done（文字展示）
- CSS 动画波形（简易 div 条形）

- [x] **Step 3: HTML 入口 + vite.config.ts**

`instant-overlay.html` + `entries/instant-overlay-main.tsx`（同其他浮窗 entry 模式）。
vite.config.ts `build.rollupOptions.input` 加 `instant-overlay`。

- [x] **Step 4: tsc + vite build**

- [x] **Step 5: Commit**

---

## Task 6: coordinator InstantStart/InstantStop + instant 路径

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/mod.rs`（新 Command 变体）
- Modify: `crates/desktop/src/engine/coordinator/session.rs`（begin_recording instant 分支）
- Modify: `crates/desktop/src/engine/coordinator/lifecycle.rs`（finalize instant 分支）
- Modify: `crates/desktop/src/engine/coordinator/paste.rs`（do_paste instant 分支）
- Modify: `crates/desktop/src/platform/ptt.rs`（callback 改用 InstantStart/Stop 替代 toggle）

**说明**：核心集成。PTT keydown → InstantStart（跳过 prepare，show instant 浮窗）；keyup → InstantStop（停录，走 finalize 但跳过 result_window，用 instant 浮窗）。

- [x] **Step 1: Command enum 加变体**

```rust
InstantStart,
InstantStop,
```

- [x] **Step 2: InstantStart 分发（Idle 态）**

```rust
Command::InstantStart => {
    if !stage.is_idle() { return; } // busy 保护
    save_frontmost_pid();
    // 直接 begin_recording（跳过两阶段 prepare）
    begin_recording(..., instant: true);
}
```

- [x] **Step 3: InstantStop 分发（活跃态）**

```rust
Command::InstantStop => {
    if stage.is_idle() { return; } // 不在录音忽略
    // 同 handle_toggle 停录，但 finalize 走 instant 路径
    handle_toggle(..., instant: true);
}
```

- [x] **Step 4: begin_recording instant 分支**

instant=true 时：跳过 `show_result`，改 `show_instant_overlay(app, "listening", "")`。

- [x] **Step 5: finalize_after_stop instant 分支**

instant=true 时：跳过 `show_result`（最终文本不弹 result_window）。

- [x] **Step 6: do_paste instant 分支**

instant=true 时：跳过 `show_result`，改 `show_instant_overlay(app, "done", &text)` + 500ms 后 `hide_instant_overlay`。

- [x] **Step 7: ptt.rs callback 改用 InstantStart/Stop**

keydown → coordinator 发 `InstantStart`；keyup → 发 `InstantStop`。

- [x] **Step 8: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test`

- [x] **Step 9: Commit**

---

## Task 7: 文档 + 手动 e2e

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: architecture.md 同步**

录音模式段落：toggle / talk (PTT) / hands-free 三模式说明。

- [x] **Step 2: 手动 e2e**

- [x] talk 模式：按住 AltRight → 浮窗"正在聆听…" → 松开 → 识别 → 粘贴到目标窗口
- [x] 浮窗状态流转（listening → processing → polishing → done → hide）
- [x] 录音为空（按住不说话松开）→ 不粘贴、hide 回 Idle
- [x] toggle 模式不受影响

- [x] **Step 3: Commit**

---

## Self-Review

**1. Spec coverage**：
- handy-keys PTT → Task 1-2 ✓
- 配置 seed → Task 3 ✓
- setup 分流 → Task 4 ✓
- instant 浮窗 → Task 5 ✓
- coordinator instant 路径 → Task 6 ✓
- 文档 → Task 7 ✓
- hands-free → TODO（首版不做）
- 设置 UI → TODO（首版不做）

**2. Type consistency**：
- `Command::InstantStart`/`InstantStop` Task 6 定义 + ptt.rs 消费 ✓
- `record_mode`/`ptt_key` Task 3 定义 + setup.rs 消费 ✓
- `show_instant_overlay`/`hide_instant_overlay` Task 5 定义 + coordinator 消费 ✓

**3. 风险**：
- handy-keys 编译/运行兼容性（Task 1 先验证编译）
- coordinator 状态机复杂度（InstantStart/Stop 是独立路径，不侵入 Toggle）
- 浮窗与 result_window 不冲突（不同 label）
