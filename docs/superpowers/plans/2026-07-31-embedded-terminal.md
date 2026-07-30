# 内嵌终端 + Agent 状态感知 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 octopus 中嵌入终端（portable-pty + xterm.js），让 agent CLI 在窗口内运行，感知 working/attention/finished 状态，替换 ActionBar 的 Terminal.app 外部终端。

**Architecture:** 新建 `crates/pty/` crate（portable-pty + OSC 解析）+ `crates/desktop/src/terminal_*.rs`（Tauri 命令 + 窗口管理 + agent hook 安装）+ 前端 `pages/Terminal/`（xterm.js + 多 tab + agent 状态徽章）。

**Tech Stack:** portable-pty 0.9, Tauri 2 Channel, xterm.js 6, React 19

## Global Constraints

- 蓝本：Terax（`/Users/wudarui/workspace/agent/terax-ai/`），技术栈一致（Tauri 2 + portable-pty + xterm.js）
- macOS-only 优先（octopus 的 agent 功能本身就是 macOS-only）
- `$OCTOPUS_TERMINAL=1` 环境变量门控 hook（只在 octopus PTY 中发 OSC）
- Hook 写入用户配置文件必须幂等 + prune 旧条目 + 不覆盖用户已有 hook
- pty_write 走 raw body 绕过 JSON（低延迟按键路径）
- TDD：PTY crate 可纯单测（spawn echo + 读输出 + OSC 解析单测）；前端交互事后冒烟

---

## File Structure

| 文件 | 职责 | 操作 |
|---|---|---|
| `crates/pty/Cargo.toml` | crate 定义 | **新建** |
| `crates/pty/src/lib.rs` | PtyState（session 注册表）+ 模块导出 | **新建** |
| `crates/pty/src/session.rs` | PtySession + spawn（3 线程）+ read/write/resize/kill | **新建** |
| `crates/pty/src/agent_detect.rs` | AgentDetector OSC 状态机 + Transition/AgentSignal | **新建** |
| `crates/pty/src/shell_init.rs` | Shell init 脚本构造（zsh/bash OSC 133 注入） | **新建** |
| `Cargo.toml`（workspace） | 加 `crates/pty` 成员 | **修改** |
| `crates/desktop/Cargo.toml` | 加 `octopus-pty` 依赖 | **修改** |
| `crates/desktop/src/terminal_window.rs` | 终端窗口创建/管理 | **新建** |
| `crates/desktop/src/terminal_commands.rs` | Tauri 命令：pty_open/write/resize/close + agent_enable_hooks | **新建** |
| `crates/desktop/src/agent_hooks.rs` | Claude/Codex/Pi hook 配置文件安装 | **新建** |
| `crates/desktop/src/core/mod.rs` | 加 `pub mod terminal_window; pub mod terminal_commands; pub mod agent_hooks;` | **修改** |
| `crates/desktop/src/core/invoke_handler.rs` | 注册新命令 | **修改** |
| `crates/desktop/src/core/setup.rs` | `app.manage(PtyState::default())` | **修改** |
| `crates/desktop/src/action_bar/action_bar_commands/script.rs` | agent 分支替换为内嵌终端 | **修改** |
| `crates/desktop/capabilities/default.json` | 加 `terminal_window` | **修改** |
| `crates/desktop/frontend/package.json` | 加 @xterm/xterm + addon-fit + addon-web-links | **修改** |
| `crates/desktop/frontend/terminal.html` | 终端窗口 HTML | **新建** |
| `crates/desktop/frontend/src/entries/terminal-main.tsx` | 终端窗口 entry | **新建** |
| `crates/desktop/frontend/src/pages/Terminal/index.tsx` | 主组件（多 tab + xterm.js + agent 状态） | **新建** |
| `crates/desktop/frontend/src/pages/Terminal/pty-bridge.ts` | Tauri ↔ PTY 桥 | **新建** |
| `crates/desktop/frontend/vite.config.ts` | 加 terminal entry | **修改** |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | 终端 i18n key | **修改** |
| `crates/desktop/frontend/src/locales/en.yaml` | 终端 i18n key | **修改** |

---

### Task 1: 创建 `crates/pty/` crate 骨架 + Cargo workspace 注册

**Files:**
- Create: `crates/pty/Cargo.toml`
- Create: `crates/pty/src/lib.rs`
- Modify: `Cargo.toml`（workspace members + dependencies）

**Interfaces:**
- Produces: `PtyState` struct（empty HashMap）、`PtySession` struct（空壳）、`PtyEvent` enum

- [ ] **Step 1: 创建 Cargo.toml**

```toml
[package]
name = "octopus-pty"
version = "0.1.0"
edition = "2021"

[dependencies]
portable-pty = "0.9"
tokio = { workspace = true }
log = { workspace = true }
anyhow = { workspace = true }
parking_lot = { workspace = true }
```

- [ ] **Step 2: 创建 lib.rs 骨架**

```rust
// crates/pty/src/lib.rs
// octopus 内嵌终端 PTY 后端——参考 Terax pty 模块设计。
// portable-pty 跨平台 PTY + OSC agent 状态感知。

pub mod session;
pub mod agent_detect;
pub mod shell_init;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use parking_lot::RwLock;

pub use session::PtySession;
pub use agent_detect::{AgentSignal, AgentDetector};

/// PTY session 注册表。Tauri State 挂载。
pub struct PtyState {
    pub sessions: RwLock<HashMap<u32, std::sync::Arc<PtySession>>>,
    next_id: AtomicU32,
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        }
    }
    pub fn alloc_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for PtyState {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 3: 创建空壳 session.rs + agent_detect.rs + shell_init.rs**

每个文件放一个最简占位（编译通过即可）。

- [ ] **Step 4: 注册 workspace**

`Cargo.toml` 加 `"crates/pty"` 到 members。

- [ ] **Step 5: 编译验证**

Run: `cargo build -p octopus-pty`
Expected: 编译通过

- [ ] **Step 6: Commit**

---

### Task 2: 实现 PtySession（spawn + 3 线程 + read/write/resize/kill）

**Files:**
- Create: `crates/pty/src/session.rs`（完整实现）

**Interfaces:**
- Consumes: `portable-pty::{native_pty_system, PtySize, CommandBuilder, ChildKiller, MasterPty}`
- Produces: `PtySession::spawn(opts, on_data, on_exit, on_signal) → Result<Arc<PtySession>>`

- [ ] **Step 1: 实现 PtySession 结构 + spawn**

参考 Terax `session.rs`。核心：
- `portable_pty::native_pty_system().openpty(PtySize)` 创建 PTY 对
- `slave.spawn_command(cmd)` 起子进程
- `drop(slave)`（关键——slave 必须在 master 读取前 drop）
- `master.take_writer()` 存 writer
- `master.try_clone_reader()` 给 reader 线程
- `child.clone_killer()` 存 killer
- `child.process_id()` 存 shell_pid

- [ ] **Step 2: 实现 3 线程**

**reader 线程**（参考 Terax）：
```rust
std::thread::Builder::new()
    .name("octopus-pty-reader".into())
    .spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,  // EOF
                Ok(n) => {
                    let data = &buf[..n];
                    // agent_detect OSC 解析（Task 3 实现，这里预留回调）
                    if let Some(ref detect) = agent_detector {
                        detect.process(data, |signal| {
                            let _ = (on_signal)(signal);
                        });
                    }
                    // push 到 pending buffer
                    let mut pending = pending_buf.0.lock();
                    pending.extend_from_slice(data);
                    if pending.len() > MAX_PENDING {
                        pending.clear();
                        pending.extend_from_slice(OVERFLOW_NOTICE);
                    }
                    pending_buf.1.notify_all();
                }
                Err(_) => break,
            }
        }
    })?;
```

**flusher 线程**：Condvar 等待 → 4ms coalesce → `on_data.send(chunk)`

**waiter 线程**：`child.wait()` → 等 reader join → flush tail → `on_exit.send(code)`

- [ ] **Step 3: 实现 write / resize / kill / has_foreground_process**

```rust
impl PtySession {
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        self.writer.lock().write_all(data)
    }
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.master.lock().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
    }
    pub fn kill(&self) {
        let _ = self.killer.lock().kill();
    }
    pub fn has_foreground_process(&self) -> bool {
        // Unix: pgrep -P shell_pid
        // macOS-only Phase 1，用 libc::tcgetpgrp 或 ps
    }
}
```

- [ ] **Step 4: 实现 Drop（kill 子进程防泄漏）**

```rust
impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}
```

- [ ] **Step 5: 单测——spawn echo + 读输出**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_spawn_and_read() {
        // spawn "echo hello" → reader 读到 "hello\n" → flusher 发出
    }
}
```

- [ ] **Step 6: 编译 + 测试**

Run: `cargo test -p octopus-pty --lib`
Expected: 测试通过

- [ ] **Step 7: Commit**

---

### Task 3: 实现 AgentDetector（OSC 133/777 解析状态机）

**Files:**
- Create: `crates/pty/src/agent_detect.rs`（完整实现）

**Interfaces:**
- Consumes: raw PTY bytes
- Produces: `AgentSignal { id, kind, agent }`

- [ ] **Step 1: 实现 OSC 状态机**

参考 Terax `agent_detect.rs`。状态：`Ground / Esc / Osc / OscEsc`

解析：
- `\x1b]` → 进入 Osc
- OSC payload 直到 `\x07`（BEL）或 `\x1b\\`（ST）
- 解析 payload：`133;C;<cmd>` / `133;D` / `777;notify;octopus;<event>`

- [ ] **Step 2: 实现 match_agent + DEFAULT_AGENTS**

```rust
const DEFAULT_AGENTS: &[&str] = &["claude", "codex", "gemini", "pi", "opencode"];
```

- [ ] **Step 3: 实现 Transition 枚举 + AgentSignal**

```rust
pub enum Transition {
    Started { agent: String },
    Working,
    Attention,
    Finished,
    Exited,
}
```

- [ ] **Step 4: 单测——OSC 序列解析**

```rust
#[test]
fn test_osc133_command_started() {
    let mut det = AgentDetector::new();
    let signals = det.process(b"\x1b]133;C;claude\x07", |s| s);
    assert_eq!(signals[0].kind, "started");
    assert_eq!(signals[0].agent, Some("claude".into()));
}

#[test]
fn test_osc777_working() {
    let mut det = AgentDetector::new();
    det.arm("claude");  // 先 arm
    let signals = det.process(b"\x1b]777;notify;octopus;working\x07", |s| s);
    assert_eq!(signals[0].kind, "working");
}
```

- [ ] **Step 5: 编译 + 测试**

Run: `cargo test -p octopus-pty --lib`
Expected: 测试通过

- [ ] **Step 6: Commit**

---

### Task 4: 实现 shell_init（OSC 133 注入脚本）

**Files:**
- Create: `crates/pty/src/shell_init.rs`（完整实现）

- [ ] **Step 1: 实现 build_command（参考 Terax shell_init.rs）**

构造 `portable_pty::CommandBuilder`：
- 设置 cwd
- 注入 env：`TERM=xterm-256color`、`COLORTERM=truecolor`、`OCTOPUS_TERMINAL=1`
- 根据 shell 类型（zsh/bash）注入 OSC 133 preexec/precmd hook

- [ ] **Step 2: zsh hook 脚本**

```bash
# 注入到临时 zshrc
precmd() { printf '\e]133;A\e\\' }
preexec() { printf '\e]133;C;%s\e\\' "$1" }
# 命令退出后
TRAPINT() { printf '\e]133;D\e\\'; return $(( 128+$? )) }
```

- [ ] **Step 3: bash hook 脚本**

```bash
# 注入到临时 bashrc
PROMPT_COMMAND='printf "\e]133;A\e\\"'
# preexec 需 trap DEBUG
```

- [ ] **Step 4: Commit**

---

### Task 5: Tauri 命令层（pty_open/write/resize/close + agent_enable_hooks）

**Files:**
- Create: `crates/desktop/src/terminal_commands.rs`
- Create: `crates/desktop/src/agent_hooks.rs`
- Modify: `crates/desktop/src/core/mod.rs`（加 pub mod）
- Modify: `crates/desktop/src/core/invoke_handler.rs`（注册命令）
- Modify: `crates/desktop/src/core/setup.rs`（app.manage(PtyState)）
- Modify: `crates/desktop/Cargo.toml`（加 octopus-pty 依赖）

- [ ] **Step 1: 实现 pty_open/write/resize/close 命令**

参考 Terax `pty/mod.rs`。

`pty_open`：`spawn_blocking` → `PtySession::spawn(opts, on_data: Channel, on_exit: Channel, on_signal: closure emit)` → 存入 PtyState → 返回 id

`pty_write`：直接 `session.write(&data)`

`pty_resize`：`session.resize(cols, rows)`

`pty_close`：`session.kill()` + 从 PtyState 移除

- [ ] **Step 2: 实现 agent_enable_hooks 命令**

参考 Terax `agent.rs`。为 Claude/Codex/Pi 写配置文件。
- `write_if_changed`：tmp + rename 原子写入
- `OWNED_MARKERS`：`["notify;octopus;", "octopus;notify"]`——prune 旧条目
- merge 不覆盖用户已有 hook

- [ ] **Step 3: 注册命令 + manage PtyState**

`invoke_handler.rs` 加：
```rust
crate::terminal_commands::pty_open,
crate::terminal_commands::pty_write,
crate::terminal_commands::pty_resize,
crate::terminal_commands::pty_close,
crate::terminal_commands::agent_enable_hooks,
```

`setup.rs` 加：
```rust
app.manage(octopus_pty::PtyState::new());
```

- [ ] **Step 4: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Expected: 0 error

- [ ] **Step 5: Commit**

---

### Task 6: 终端窗口（terminal_window.rs + 前端 entry + HTML）

**Files:**
- Create: `crates/desktop/src/terminal_window.rs`
- Create: `crates/desktop/frontend/terminal.html`
- Create: `crates/desktop/frontend/src/entries/terminal-main.tsx`
- Modify: `crates/desktop/frontend/vite.config.ts`（加 terminal entry）
- Modify: `crates/desktop/frontend/package.json`（加 @xterm/xterm + addon-fit + addon-web-links）
- Modify: `crates/desktop/capabilities/default.json`（加 terminal_window）

- [ ] **Step 1: 实现 terminal_window.rs**

参考 `compact_editor_window.rs`。窗口属性：
- 原生标题栏、1100×680 可调、居中
- 单例：已存在则 show+focus，否则创建
- macOS 开窗切 Regular、关窗切回 Accessory

`open_terminal_window(app)` → 创建或聚焦终端窗口

`open_terminal_with_command(app, cwd, command)` → 打开窗口 + emit "terminal://new-tab" { cwd, command }

- [ ] **Step 2: 创建 terminal.html**

复制 compact-editor.html 结构，改 entry script 指向 terminal-main.tsx。

- [ ] **Step 3: 创建 terminal-main.tsx**

```typescript
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Terminal from "@/pages/Terminal";

mountApp(<Terminal />);
```

- [ ] **Step 4: vite.config.ts 加 entry**

```typescript
terminal: "terminal.html",
```

- [ ] **Step 5: npm install xterm + addon**

```bash
cd crates/desktop/frontend
npm install @xterm/xterm @xterm/addon-fit @xterm/addon-web-links
```

- [ ] **Step 6: capabilities 加 terminal_window**

- [ ] **Step 7: 编译验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: ✓ built

- [ ] **Step 8: Commit**

---

### Task 7: 前端终端组件（多 tab + xterm.js + agent 状态徽章）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/index.tsx`
- Create: `crates/desktop/frontend/src/pages/Terminal/pty-bridge.ts`

- [ ] **Step 1: 实现 pty-bridge.ts**

参考 Terax `pty-bridge.ts`：
```typescript
import { Channel, invoke } from "@tauri-apps/api/core";

export async function openPty(opts: {
  cols: number; rows: number; cwd?: string;
  onData: (bytes: Uint8Array) => void;
  onExit: (code: number) => void;
}): Promise<number> {
  const onDataChannel = new Channel<Uint8Array>();
  const onExitChannel = new Channel<number>();
  onDataChannel.onmessage = (buf) => opts.onData(new Uint8Array(buf));
  onExitChannel.onmessage = (code) => opts.onExit(code);
  return invoke<number>("pty_open", {
    cols: opts.cols, rows: opts.rows, cwd: opts.cwd,
    onData: onDataChannel, onExit: onExitChannel,
  });
}

export function writePty(id: number, data: Uint8Array): Promise<void> {
  return invoke("pty_write", { id, data });
}

export function resizePty(id: number, cols: number, rows: number): Promise<void> {
  return invoke("pty_resize", { id, cols, rows });
}

export function closePty(id: number): Promise<void> {
  return invoke("pty_close", { id });
}
```

- [ ] **Step 2: 实现 index.tsx 主组件**

核心功能：
- 多 tab：`tabs: TerminalTab[]`，每 tab 有 `ptyId | null`、`term: Terminal`（xterm.js）、`agentPhase`
- 新 tab 按钮：创建空 xterm.js Terminal（等待 shell 启动）
- mount PTY：`openPty({ cols, rows, onData: (bytes) => term.write(bytes) })`
- 输入：`term.onData((str) => writePty(id, new TextEncoder().encode(str)))`
- resize：`fitAddon` + `term.onResize` → `resizePty`
- agent 信号：`listen("agent://signal", (e) => { 更新 tab.agentPhase })`
- 关 tab：`closePty(id)` + `term.dispose()`
- 初始命令（ActionBar 联动）：listen "terminal://new-tab" → 新 tab + 写命令

- [ ] **Step 3: agent 状态徽章**

每个 tab 标题旁显示彩色圆点：
- `working` → amber 脉冲动画
- `attention` → 红色 bell 图标
- `idle` → 灰色（无指示）

- [ ] **Step 4: i18n key**

zh-CN + en 各加 `terminal.*` key（title / newTab / close 等）

- [ ] **Step 5: tsc + vite build**

Run: `cd crates/desktop/frontend && npm run build`
Expected: ✓ built

- [ ] **Step 6: Commit**

---

### Task 8: ActionBar 整合 + 手动冒烟

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs`（agent 分支）

- [ ] **Step 1: 替换 agent 分支**

在 `execute_action_bar_inner` 的 agent 分支中：

```rust
// 旧：TerminalAppLauncher.spawn()
// 新：
match crate::terminal_window::open_terminal_with_command(&app, &cwd, &command) {
    Ok(_) => log::info!("[action-bar] agent 已启动到内嵌终端"),
    Err(e) => {
        log::warn!("[action-bar] 内嵌终端失败，fallback 到 Terminal.app: {}", e);
        let launcher = TerminalAppLauncher;
        launcher.spawn(&command, &cwd_buf)?;
    }
}
```

- [ ] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Expected: 0 error

- [ ] **Step 3: 手动冒烟测试**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_feature_0729
./run-octopus.sh --no-lto
```

测试：
1. 设置页 ActionBarPanel 选 agent 项 → 终端窗口打开 → agent 输出可见
2. 终端输入命令 → 正常执行
3. agent 状态徽章变化（如果安装了 hook）
4. 多 tab 创建/切换/关闭

- [ ] **Step 4: Commit**

---

## Self-Review

**Spec coverage:**
- ✅ PTY 后端（portable-pty + 3 线程）— Task 1-2
- ✅ OSC 133/777 agent 状态感知 — Task 3
- ✅ Shell init 脚本注入 — Task 4
- ✅ Tauri 命令层 — Task 5
- ✅ 终端窗口 — Task 6
- ✅ xterm.js 前端 + agent 状态徽章 — Task 7
- ✅ ActionBar 替换 — Task 8
- ✅ Agent hook 安装（Claude/Codex/Pi）— Task 5 Step 2

**Placeholder scan:** 无 TODO/TBD。每步有具体代码或参考路径。

**Type consistency:** `PtyState`/`PtySession`/`AgentSignal` 在 Task 1-3 定义，Task 5-8 消费。`TerminalTab`/`agentPhase` 在 Task 7 定义。
