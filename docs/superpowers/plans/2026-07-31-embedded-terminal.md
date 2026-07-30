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
| `crates/desktop/src/commands/terminal_commands.rs` | Tauri 命令：pty_open/write/resize/close | **新建** |
| `crates/desktop/src/commands/agent_hooks.rs` | Claude/Codex/Gemini/Pi hook 配置文件安装 | **新建** |
| `crates/desktop/src/commands/mod.rs` | 加 `pub mod terminal_commands; pub mod agent_hooks;` | **修改** |
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

> **⚠️ 实施记录（2026-07-30 review）**：交接文档标称「Task 1-4 已完成」，但实测 `session.rs` 只有裸 struct（write/resize/kill/is_exited），**spawn 方法 + reader/flusher/waiter 3 线程完全未实现**。`cargo test -p octopus-pty --lib` 9 测试全绿是真实的，但只覆盖了 `agent_detect.rs` 的 OSC 解析和 `shell_init.rs::build_command` 的「构造成功」（后者甚至没断言任何字段），没覆盖到缺失的 spawn。Step 1-7 全部需重做。已 commit 的 `65ce2b1a` 是半成品，本 Task 真正补完 spawn + 3 线程。

**Files:**
- Create: `crates/pty/src/session.rs`（完整实现）

**Interfaces:**
- Consumes: `portable-pty::{native_pty_system, PtySize, CommandBuilder, ChildKiller, MasterPty}`
- Produces: `spawn(id, app, cols, rows, cwd, shell, on_data: Channel<Response>, on_exit: Channel<i32>, on_signal: impl Fn(AgentSignal)) → Result<Arc<PtySession>, String>`（对齐 Terax 签名——free fn，非 method）

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

> **⚠️ 实施记录（2026-07-30 review）**：交接标称「已完成」，实测为**简化版**——能用（7 测试绿），但相对 Terax 蓝本缺以下关键能力，**bash 下 agent 状态感知会失效**：
> - 缺 `finish()` —— PTY 关闭时不发 Exited，UI 会留 stale 条目
> - 缺 `Transition` enum —— 用裸 `kind: String`，类型安全弱
> - 缺 **auto-arm** —— bash 无 preexec，靠 OSC 777 marker 自我 arm（Terax `ensure_armed`），当前简化版不 arm，bash 下永远感知不到 agent
> - 缺 OSC 9 attention（generic desktop notification）
> - 缺 4-field marker（`777;notify;octopus;<agent>;<event>`）—— Codex/Gemini/Pi 用这个格式
> - 缺 `OSC_MAX` 溢出防护（>2048 字节的 OSC 会 panic 或乱）
> - 缺 `status` 字段——Working 状态会重复 emit
>
> `process` 签名已改为 `process(data, session_id) -> Vec<AgentSignal>`，与 plan 描述的 `process(data, |signal| ...)` 不同。本 Task 需重写为 Terax 完整版（回调式 `process<F: FnMut(Transition)>` + `finish` + auto-arm），去掉 octopus 不需要的 da_filter。

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

> **⚠️ 实施记录（2026-07-30 review）**：交接标称「已完成」，实测**OSC 脚本根本没注入**——`ZSH_INIT` / `BASH_INIT` 字符串定义了，但 `build_command` 里 match arm 是空的（`match shell_name { "zsh" => { /* Phase 1 简化...后续优化 */ } "bash" => {...} _ => {} }`），shell integration 形同虚设。也没有 Terax 的 ZDOTDIR 保留用户配置方案——直接覆盖用户 `~/.zshrc` 会让 starship/p10k 等 prompt 框架失效。
>
> 另外 `shell_init.rs:101,107` 的两个测试有 `unused variable: cmd` warning（AGENTS.md 要求 0 warning）。本 Task 需重做：采用 Terax 的 ZDOTDIR + `--rcfile` 方案，把 OSC 133 脚本写到 `~/.cache/octopus/shell-integration/` 下临时文件，env 指过去，保留用户原有配置（`TERAX_USER_ZDOTDIR` → 改名 `OCTOPUS_USER_ZDOTDIR`）。

**Files:**
- Create: `crates/pty/src/shell_init.rs`（完整实现）
- Create: `crates/pty/src/scripts/zshenv.zsh` + `zshrc.zsh` + `bashrc.bash`（OSC 133 注入脚本，对齐 Terax，去掉 Terax 标识改 octopus）

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

> **实施记录（2026-07-30）**：已完成。与原 plan 的偏差：
> - **文件位置**：plan 写 `crates/desktop/src/terminal_commands.rs` + 注册到 `core/mod.rs`。实际放到 `commands/` 域（`crates/desktop/src/commands/terminal_commands.rs` + `agent_hooks.rs` + `commands/mod.rs` 注册），因为 `commands/` 才是命令文件域（settings_commands/model_commands 等都在那），core 是 setup/bootstrap/config 基础设施。
> - **spawn 签名**：plan 写 `PtySession::spawn(opts, on_data, on_exit, on_signal)`（method）。实际是 free fn `spawn(id, cols, rows, cwd, shell, on_data, on_exit, on_signal) -> Result<(Arc<PtySession>, PtySize), String>`——返回 tuple，pty_open 解构只存 session。
> - **回调解耦**：pty crate 保持纯逻辑（不依赖 tauri），on_data/on_exit/on_signal 是 `Fn + Send + Sync + 'static` 闭包，terminal_commands.rs 桥接到 `Channel<Response>` / `Channel<i32>` / `app.emit("agent://signal", ...)`。
> - **agent_hooks 扩展**：plan 只提 Claude/Codex/Pi，实际实现 4 个 agent（Claude/Codex/Gemini/Pi）。Gemini 用 `matcher: "*"` + 4-field marker。Pi 走 TS 扩展机制（非 JSON hook）。加了 `agent_hooks_status` 命令查询安装状态（plan 没提，前端开关需要）。
> - **验证**：`cargo check -p octopus-desktop --features embedded,cloud,custom-protocol` 0 error 0 warning；`agent_hooks` 12 测试全绿。

**Files:**
- Create: `crates/desktop/src/commands/terminal_commands.rs`
- Create: `crates/desktop/src/commands/agent_hooks.rs`
- Modify: `crates/desktop/src/commands/mod.rs`（加 pub mod）
- Modify: `crates/desktop/src/core/invoke_handler.rs`（注册命令）
- Modify: `crates/desktop/src/core/setup.rs`（app.manage(PtyState)）
- Modify: `crates/desktop/Cargo.toml`（加 octopus-pty 依赖）

- [x] **Step 1: 实现 pty_open/write/resize/close 命令**

参考 Terax `pty/mod.rs`。

`pty_open`：`spawn_blocking` → `spawn(...)` → 解构 `(session, _size)` → 存入 PtyState → 返回 id。额外：shell 提前退出时 re-check + reap（防孤儿）。

`pty_write`：raw body + `x-pty-id` header，绕过 JSON。

`pty_resize`：`session.resize(cols, rows)`

`pty_close`：`session.kill()` + 从 PtyState 移除 + detach drop 线程（防阻塞 worker）

- [x] **Step 2: 实现 agent_enable_hooks + agent_hooks_status 命令**

参考 Terax `agent.rs`。为 Claude/Codex/Gemini/Pi 写配置文件。
- `write_atomic`：tmp + rename 原子写入
- `OWNED_MARKERS`：`["notify;octopus;", "octopus;notify", "__octopus_notify"]`——prune 旧条目
- merge 不覆盖用户已有 hook（retain foreign + append ours）
- Pi 走 TS 扩展（`octopus-notifications.ts`），非 octopus 管理的文件拒绝覆写
- `agent_hooks_status`：查所有 event 的 status_needle 是否都在配置里

- [x] **Step 3: 注册命令 + manage PtyState**

`invoke_handler.rs` 加 6 个命令（pty_open/write/resize/close + agent_enable_hooks/agent_hooks_status）。
`setup.rs` 加 `init_pty()` 方法 → `app.manage(octopus_pty::PtyState::new())`。

- [x] **Step 4: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Result: 0 error 0 warning ✓

- [x] **Step 5: Commit**

---

### Task 6: 终端窗口（terminal_window.rs + 前端 entry + HTML）

> **实施记录（2026-07-30）**：已完成。与原 plan 的偏差：
> - **文件位置**：plan 写 `crates/desktop/src/terminal_window.rs`。实际放 `ui/` 域（`crates/desktop/src/ui/terminal_window.rs`），与 settings_window / compact_editor_window 同级（都是窗口管理）。plan 提到的「参考 compact_editor_window」本身就在 ui/ 域。
> - **窗口尺寸**：plan 写 1100×680，实际 1000×640（终端窗口不需要 compact_editor 那么宽，min 560×360）。
> - **额外接入点（plan 没提但必须做）**：
>   - `platform/activation.rs` 的 `REGULAR_WINDOWS` + `WINDOWS_TO_HIDE_ON_FLOAT` 加 `terminal_window`——否则浮窗焦点协调不认它，关窗后激活策略不会正确恢复（踩坑隐患）。
>   - `main.rs` WindowEvent::Destroyed 加 `terminal_window` 分支调 `on_terminal_closed`。
>   - `tray.rs` 加「终端」菜单项（让 `open_terminal_window` 立即被使用，避免 dead_code warning；Task 8 ActionBar 接 `open_terminal_with_command`）。
> - **TDD**：`build_initial_url` + `urlencode` 提取为纯函数，7 个单测覆盖（无 cwd/bg、有 cwd、percent-encode 中文空格、组合）。
> - **前端占位**：Terminal 页面是占位（读 cwd query 验证链路），Task 7 替换为完整 xterm.js + 多 tab。
> - **i18n**：`tray.terminal` + `terminal.loading`（zh-CN + en）。
> - **验证**：cargo check 0 error 0 warning；vite build ✓（terminal.html + terminal-*.js 生成）；pty 26 测试仍绿；terminal_window 7 测试绿。

**Files:**
- Create: `crates/desktop/src/ui/terminal_window.rs`
- Create: `crates/desktop/frontend/terminal.html`
- Create: `crates/desktop/frontend/src/entries/terminal-main.tsx`
- Create: `crates/desktop/frontend/src/pages/Terminal/index.tsx`（Task 7 替换为完整实现）
- Modify: `crates/desktop/src/ui/mod.rs`（加 pub mod terminal_window）
- Modify: `crates/desktop/src/platform/activation.rs`（REGULAR_WINDOWS + WINDOWS_TO_HIDE_ON_FLOAT 加 terminal_window）
- Modify: `crates/desktop/src/main.rs`（Destroyed 事件加 terminal_window 分支）
- Modify: `crates/desktop/src/ui/tray.rs`（加「终端」菜单项）
- Modify: `crates/desktop/frontend/vite.config.ts`（加 terminal entry）
- Modify: `crates/desktop/frontend/package.json`（加 @xterm/xterm + addon-fit + addon-web-links）
- Modify: `crates/desktop/capabilities/default.json`（加 terminal_window）
- Modify: `crates/desktop/frontend/src/locales/{zh-CN,en}.yaml`（tray.terminal + terminal.loading）

- [x] **Step 1: 实现 terminal_window.rs**

参考 `compact_editor_window.rs`。窗口属性：
- 原生标题栏、1000×640 可调（min 560×360）、居中
- 单例：已存在则 show+focus，否则创建
- macOS 开窗切 Regular、关窗切回 Accessory
- `build_initial_url(cwd, bg)` 纯函数：cwd percent-encode + bg hex 注入 URL query（7 单测）

`open_terminal_window(app, cwd)` → 创建或聚焦终端窗口

`open_terminal_with_command(app, cwd, command)` → 打开窗口 + emit "terminal://new-tab" { cwd, command }（Task 8 用，暂 `#[allow(dead_code)]`）

- [x] **Step 2: 创建 terminal.html**

复制 compact-editor.html 结构（theme bootstrap + bg 注入），entry 指向 terminal-main.tsx。

- [x] **Step 3: 创建 terminal-main.tsx**

`mountApp(<Terminal />)`，Terminal 当前为占位（Task 7 替换）。

- [x] **Step 4: vite.config.ts 加 entry**

`terminal: "terminal.html",`

- [x] **Step 5: npm install xterm + addon**

`@xterm/xterm` + `@xterm/addon-fit` + `@xterm/addon-web-links`（Task 7 用）

- [x] **Step 6: capabilities 加 terminal_window + activation 注册 + tray 菜单项 + i18n**

capabilities/default.json windows 数组加 `terminal_window`；
activation.rs REGULAR_WINDOWS + WINDOWS_TO_HIDE_ON_FLOAT 加 terminal_window；
main.rs Destroyed 事件加 on_terminal_closed；
tray.rs 加「终端」菜单项（tray.terminal i18n key）。

- [x] **Step 7: 编译验证**

Run: `npm run build` + `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Result: vite build ✓（terminal.html + terminal-*.js 生成）；cargo 0 error 0 warning ✓

- [x] **Step 8: Commit**

---

### Task 7: 前端终端组件（多 tab + xterm.js + agent 状态徽章）

> **实施记录（2026-07-30）**：已完成。相对 Terax 大幅简化（Phase 1）：
> - **无 rendererPool**：直接在 hook 里 `new Terminal()` + fitAddon，每 tab 一个 xterm 实例（Terax 用池化 + dormantRing + slot retain/park，octopus Phase 1 不需要）。
> - **无 zustand**：agent 状态用模块级 state + subscribe 模式（与 octopus i18n 同构），避免引入新依赖。
> - **无分屏 pane 树**：每 tab 单 pane（Terax 有 PaneTreeView 分屏）。
> - **pty_write 用 raw body**：`invoke("pty_write", textEncoder.encode(data), { headers: { "x-pty-id": String(id) } })`——对齐 Task 5 Rust 的 `InvokeBody::Raw` + header 设计（plan 原写的 `invoke("pty_write", { id, data })` 是 JSON 路径，与 Rust 不匹配）。
> - **拆分为 4 个文件**（plan 只写了 index.tsx + pty-bridge.ts）：`pty-bridge.ts` / `useTerminalSession.ts` / `TerminalPane.tsx` / `index.tsx` + `agent-activity.ts`（agent 状态 store）。
> - **TDD**：`phaseForSignal` 纯函数 6 单测（vitest）。
> - **验证**：tsc + vite build ✓（terminal-*.js 348kB，含 xterm）；cargo check 0 error 0 warning；agent-activity 6 测试绿。

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/pty-bridge.ts`
- Create: `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts`
- Create: `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx`
- Create: `crates/desktop/frontend/src/pages/Terminal/index.tsx`（替换 Task 6 占位）
- Create: `crates/desktop/frontend/src/pages/Terminal/agent-activity.ts`
- Create: `crates/desktop/frontend/src/pages/Terminal/agent-activity.test.ts`
- Modify: `crates/desktop/frontend/src/index.css`（终端 CSS + agent 徽章脉冲动画）
- Modify: `crates/desktop/frontend/src/locales/{zh-CN,en}.yaml`（terminal.title/newTab/closeTab）

- [x] **Step 1: 实现 pty-bridge.ts**

`openPty(cols, rows, handlers, cwd) → Promise<PtySession>`。返回的 session 持有 write/resize/close。
- `pty_write` 走 raw body + `x-pty-id` header（对齐 Task 5 Rust）
- releaseHandlers 防退出后回调再触发
- close 幂等（closed flag）

- [x] **Step 2: 实现 useTerminalSession hook + TerminalPane**

useTerminalSession(container, cwd, onExit)：
- new Terminal（深色 #0c0c0f 主题，SF Mono 13px，cursorBlink）+ FitAddon + WebLinksAddon
- openPty → onData 喂 term.write；term.onData → pty.write；term.onResize → pty.resize
- ResizeObserver 监听容器尺寸 → fitAddon.fit（隐藏容器跳过，防 0 尺寸）
- cleanup：pty.close + term.dispose

TerminalPane：调 hook，上报 ptyId + 消费 pendingCommand（ActionBar 联动写命令 + 回车）。

- [x] **Step 3: index.tsx 主组件 + agent 状态徽章**

多 tab：tabs 数组，每 tab 持 ptyId（TerminalPane 上报）+ pendingCommand。
- tab 切换用 visibility:hidden 保活（不卸载 xterm，scrollback 保留）
- 新 tab 按钮（Plus 图标）；关 tab（X 图标，最后关一个不关窗口，建新空 tab）
- ActionBar 联动：listen "terminal://new-tab" { cwd, command } → 新 tab + 写命令
- URL query cwd（Rust 注入）→ 首个 tab 的 cwd

agent-activity.ts：模块级 state + subscribe（替代 zustand），listen "agent://signal"，
phaseForSignal 纯映射 + finished 6s TTL 自动 idle + exited 清理。

agent 徽章（CSS signature 元素）：
- working → amber 圆点脉冲（1.4s，box-shadow 扩散）
- attention → 红色 Bell 图标摇晃
- finished → 绿色圆点淡出
- 全部尊重 prefers-reduced-motion

- [x] **Step 4: i18n + CSS**

zh-CN + en 各加 `terminal.title` / `newTab` / `closeTab` / `loading`。
index.css 加 `.terminal-window` / `.terminal-tabbar` / `.terminal-agent-*` 全套样式（主题 token 自适应 + agent 动画）。

- [x] **Step 5: tsc + vite build + cargo check**

Run: `npm run build` + `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Result: ✓ built（terminal-*.js 348kB）；cargo 0 error 0 warning；agent-activity 6 测试绿。

- [ ] **Step 6: Commit**

---

### Task 8: ActionBar 整合 + 手动冒烟

> **实施记录（2026-07-31）**：已完成代码改动 + 自动化验证。手动冒烟测试待用户执行（GUI 应用，AI 无法操作窗口）。
> - **agent 分支替换**：`TerminalAppLauncher.spawn()` → `open_terminal_with_command(&app, Some(&cwd), &command)`，失败 fallback 回 Terminal.app（保留旧路径做兜底）。
> - **线程安全**：`open_terminal_with_command` 内部用 `run_on_main_thread` 调度 AppKit，可在 async worker 线程安全调用（无需 spawn_blocking，与原 Terminal.app 路径不同——osascript 才需 spawn_blocking）。
> - **移除 dead_code allow**：Task 6 加的 `#[allow(dead_code)]` 现在接入消费方，已移除。
> - **端到端链路验证**（代码审查）：前端 invoke `execute_action_bar` → agent 分支 derive_cwd + render_command → `open_terminal_with_command` → `open_terminal_window`（新建/聚焦）+ emit `"terminal://new-tab" {cwd, command}` → 前端 listen → addTab → TerminalPane → openPty(cwd) + write(command + "\n")。类型链 `&AppHandle` 匹配 ✓。
> - **验证**：cargo check 0 error 0 warning；全测试回归绿（pty 26 + agent_hooks 12 + terminal_window 7 = 45 测试）。

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs`（agent 分支）
- Modify: `crates/desktop/src/ui/terminal_window.rs`（移除 `#[allow(dead_code)]`）

- [x] **Step 1: 替换 agent 分支**

agent 分支改为优先内嵌终端，失败 fallback Terminal.app：
```rust
match crate::ui::terminal_window::open_terminal_with_command(&app, Some(&cwd), &command) {
    Ok(_) => log::info!("[action-bar] agent 已启动到内嵌终端"),
    Err(e) => {
        log::warn!("[action-bar] 内嵌终端失败，fallback 到 Terminal.app: {}", e);
        // 旧路径保留做兜底（osascript spawn_blocking）
    }
}
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded,cloud,custom-protocol`
Result: 0 error 0 warning ✓

- [ ] **Step 3: 手动冒烟测试（待用户执行）**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_feature_0729
./run-octopus.sh
```

测试项：
1. 托盘菜单点「终端」→ 终端窗口打开 → shell 可交互（`ls` / `echo hi` 正常）
2. 设置页 ActionBarPanel 选 agent 项 → 终端窗口聚焦 + 新 tab + agent 命令自动写入执行
3. 终端输入命令 → 正常执行；多 tab 创建/切换/关闭（最后一个关了建新空 tab）
4. 安装 agent hook 后（设置页触发 `agent_enable_hooks`），agent 运行时 tab 徽章变色（working 脉冲 / attention bell / finished 绿点）

- [x] **Step 4: Commit**

---

## Self-Review

**Spec coverage:**
- ✅ PTY 后端（portable-pty + 3 线程）— Task 1-2
- ✅ OSC 133/777 agent 状态感知 — Task 3
- ✅ Shell init 脚本注入 — Task 4
- ✅ Tauri 命令层 — Task 5
- ✅ 终端窗口 — Task 6
- ✅ xterm.js 前端 + agent 状态徽章 — Task 7
- ✅ ActionBar 替换（内嵌终端优先 + Terminal.app fallback）— Task 8
- ✅ Agent hook 安装（Claude/Codex/Gemini/Pi）— Task 5 Step 2

**Placeholder scan:** 无 TODO/TBD。每步有具体代码或参考路径。唯一未完成项是 Task 8 Step 3 手动冒烟（需 GUI 操作，留给用户）。

**Type consistency:** `PtyState`/`PtySession`/`AgentSignal`/`Transition` 在 Task 1-3 定义，Task 5-8 消费。`Tab`（含 ptyId/pendingCommand）在 Task 7 定义，Task 8 的 emit payload `NewTabPayload` 与前端 listen 的 `{cwd, command}` 结构对齐。casing：`NewTabPayload` 用 `#[serde(rename_all = "camelCase")]`（cwd/command 单词无变化，符合规范）。

**测试覆盖（51 测试）：**
- pty crate：26（OSC 状态机 17 + spawn/echo/退出码/Drop 3 + shell_init 6）
- desktop agent_hooks：12（幂等/merge 保留 foreign/迁移/Pi 扩展等）
- desktop terminal_window：7（URL 构造 + percent-encode）
- frontend agent-activity：6（phaseForSignal 纯映射）
