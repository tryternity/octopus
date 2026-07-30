# octopus 内嵌终端 + Agent 状态感知设计

**日期**：2026-07-31
**类型**：新功能（内嵌终端 + agent 交互）
**蓝本**：Terax（Tauri 2 + portable-pty + xterm.js + OSC agent 感知）
**调研**：`docs/research/2026-07-30-embedded-terminal-agent-analysis.md`

## 目标

octopus 作为 AI 办公第一入口，需要一个内嵌终端——让 agent CLI（claude/codex/pi）在 octopus 窗口内运行，用户看到输出、输入追问、感知 agent 状态（working/attention/finished），而非依赖外部 Terminal.app。

## 范围（Phase 1）

- ✅ PTY 后端（portable-pty）：spawn/read/write/resize/kill
- ✅ 独立终端窗口（多 tab，每 tab 一个 PTY session）
- ✅ xterm.js 前端渲染
- ✅ tab 改名（双击内联编辑）+ 布局切换（顶部 tabs ↔ 左侧 sidebar）——详见 [tab 改名/布局 spec](2026-07-31-terminal-tab-rename-layout.md)
- ✅ OSC 133 shell prompt marker（通用，检测命令开始/结束）
- ✅ OSC 777 agent hook（Claude/Codex/Gemini/Pi，检测 working/attention/finished）
- ✅ ActionBar agent 替换：选 agent 后打开内嵌终端（Terminal.app 保留 fallback）
- ❌ 手机遥控 WebSocket（Phase 2）
- ❌ shell 一次性/session/bg 三模式（Terax 的 shell 模块，暂不做）
- ❌ renderer pool（xterm.js 实例复用，暂不做——单 session 单实例）

## 架构

### 新增 crate：`crates/pty/`

```
crates/pty/
├── Cargo.toml          # portable-pty 0.9, serde, dirs, libc, log, anyhow, parking_lot
└── src/
    ├── lib.rs          # 模块导出 + PtyState（session 注册表）+ spawn re-export
    ├── session.rs      # PtySession struct + spawn() free fn（3 线程）+ write/resize/kill
    ├── agent_detect.rs # AgentDetector OSC 状态机 + Transition enum + AgentSignal
    ├── shell_init.rs   # build_command（ZDOTDIR / --rcfile 脚本注入）
    └── scripts/        # shell 集成脚本（include_str!）
        ├── zshenv.zsh
        ├── zshrc.zsh
        └── bashrc.bash
```

> **实施偏差**：plan 原写 `spawn` 是 `PtySession::spawn()` method。实际是 free fn `spawn(id, cols, rows, cwd, shell, on_data, on_exit, on_signal) -> Result<(Arc<PtySession>, PtySize), String>`，闭包解耦 tauri（pty crate 保持纯净，无 tauri 依赖）。on_data/on_exit/on_signal 是 `Fn + Send + Sync + 'static`。

**`PtyState`**（参考 Terax `PtyState`）：
```rust
pub struct PtyState {
    sessions: RwLock<HashMap<u32, Arc<PtySession>>>,
    next_id: AtomicU32,  // 从 1 开始，永不复用
}
```

**`PtySession`**（参考 Terax `Session`）：
```rust
pub struct PtySession {
    pub id: u32,
    pub shell_pid: u32,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub killer: Mutex<Box<dyn ChildKiller + Send>>,
    pub exited: Arc<AtomicBool>,
}
```

**`spawn()` 三线程模型**（参考 Terax session.rs）：

1. **reader 线程**：16KB 块读 PTY → `agent_detect.process(&buf, |signal| app.emit("agent://signal", signal))` → push 到 pending buffer（4MiB 上限反压）
2. **flusher 线程**：Condvar 等待 → 4ms coalesce → `on_data.send(chunk)`（Channel<Vec<u8>>）
3. **waiter 线程**：`child.wait()` → 等 reader EOF → flush tail → `on_exit.send(code)` → drop session

关键常量（从 Terax 直接采用）：
```rust
const FLUSH_COALESCE: Duration = Duration::from_millis(4);
const FLUSH_MAX_IDLE: Duration = Duration::from_millis(50);
const READ_BUF: usize = 16 * 1024;
const MAX_PENDING: usize = 4 * 1024 * 1024;
```

### Agent 状态感知（OSC 解析，参考 Terax agent_detect.rs）

**`AgentDetector` 状态机**：`Ground / Esc / Osc / OscEsc`（OSC_MAX=2048 溢出防护）

解析 OSC 序列（PS=第一段，PT=剩余）：

**OSC 133**（shell prompt marker，由 zshrc/bashrc preexec hook 发出）：
- `133;C;<cmd>` → 匹配 `claude/codex/gemini/pi/opencode/grok`（支持路径前缀/npx 包装/连字符后缀）→ emit `Started { agent }`
- `133;D` → emit `Exited`（仅 armed 时）

**OSC 777**（agent hook 主动通知，由 agent 配置文件的 hook 发出）：
- 3-field `777;notify;octopus;<event>` → Claude（默认 agent=claude auto-arm）
- 4-field `777;notify;octopus;<agent>;<event>` → Codex/Gemini/Pi（带 agent 名）
- event ∈ `working`/`attention`/`finished` → 对应 Transition
- **auto-arm**：bash 无 preexec，OSC 777 来了才 `ensure_armed`（自我 arm）

**OSC 9**（generic desktop notify，非 `9;4` taskbar 进度）→ armed 时 emit `Attention`

**`Transition` enum**（类型安全，替代裸 String）+ **`AgentSignal`**（emit 到前端）：
```rust
pub enum Transition { Started{agent}, Working, Attention, Finished, Exited }

#[derive(serde::Serialize)]
pub struct AgentSignal {
    pub id: u32,                  // PTY session id
    pub kind: &'static str,       // "started"|"working"|"attention"|"finished"|"exited"
    pub agent: Option<String>,    // agent 名称（仅 started 携带）
}
```

`finish()`：PTY 关闭时若 armed 发 Exited（shell 中途死，没发 133;D 的情况），避免 UI stale。
`status` 字段防 Working 重复 emit。

### Agent Hook 安装（参考 Terax agent.rs）

为每个 agent 写配置文件，注入 OSC 777 hook。注入 `$OCTOPUS_TERMINAL=1` 环境变量，让 hook 只在 octopus PTY 中发 OSC。

四种 agent（Delivery 区分命令发射方式）：

| Agent | 配置文件 | Delivery | 事件映射 |
|---|---|---|---|
| Claude | `~/.claude/settings.json` | TerminalSequence（`terminalSequence` JSON 字段，v2.1.139+ 丢了 /dev/tty） | UserPromptSubmit→working / Notification→attention / Stop→finished |
| Codex | `~/.codex/hooks.json` | Osc（`> /dev/tty` + stdout `{}` no-op） | UserPromptSubmit→working / PermissionRequest→attention / Stop→finished |
| Gemini | `~/.gemini/settings.json` | Osc（`> /dev/tty`） + `matcher:"*"` | BeforeAgent→working / Notification→attention / AfterAgent→finished |
| Pi | `~/.pi/agent/extensions/octopus-notifications.ts` | TS 扩展（`process.stdout.write` OSC 777） | agent_start→working / agent_settled→finished |

Hook 命令：
- `agent_enable_hooks(agent: String)` → 原子写入（tmp + rename）+ 幂等（`OWNED_MARKERS` prune 旧条目）+ 不覆盖用户已有 hook（merge 注入）
- `agent_hooks_status(agent: String) -> bool` → 查所有 event 的 status_needle 是否都在配置里（前端开关用）

### Shell 集成脚本

PTY spawn 时注入 shell init 脚本（参考 Terax shell_init.rs）：
- zsh：临时 ZDOTDIR + zshrc 注入 OSC 133 preexec/precmd hook
- bash：`--rcfile` 注入
- 注入 `$OCTOPUS_TERMINAL=1` + `$TERM=xterm-256color` + `$COLORTERM=truecolor`

OSC 133 hook 脚本（zsh 示例）：
```bash
# precmd: prompt 开始
precmd() { printf '\e]133;A\e\\' }
# preexec: 命令开始
preexec() { printf '\e]133;C;%s\e\\' "$1" }
# 命令结束（zsh 4.3.0+）
add-zsh-hook zshaddhistory printf '\e]133;D\e\\'
```

### Tauri 命令层（`crates/desktop/src/`）

```
crates/desktop/src/
├── ui/terminal_window.rs         # 终端窗口创建/管理（ui 域，与 settings_window 同级）
└── commands/
    ├── terminal_commands.rs      # pty_open/write/resize/close
    └── agent_hooks.rs            # agent_enable_hooks + agent_hooks_status
```

> **实施偏差**：plan 原写 `terminal_window.rs` + `terminal_commands.rs` + `agent_hooks.rs` 在 src 根 + 注册到 core/mod.rs。实际：window 放 `ui/` 域（与 settings_window/compact_editor_window 同级），commands 放 `commands/` 域（与 settings_commands 等同级）。

**Tauri 命令**：
```rust
#[tauri::command]
async fn pty_open(app, state, cols, rows, cwd, shell, on_data: Channel<Response>, on_exit: Channel<i32>) -> Result<u32, String>

// raw body + x-pty-id header（绕过 JSON，按键延迟敏感路径）
fn pty_write(state, request: tauri::ipc::Request) -> Result<(), String>

#[tauri::command]
fn pty_resize(state, id, cols, rows) -> Result<(), String>

#[tauri::command]
fn pty_close(state, id) -> Result<(), String>

#[tauri::command]
fn agent_enable_hooks(agent: String) -> Result<(), String>

#[tauri::command]
fn agent_hooks_status(agent: String) -> bool
```

`pty_write` 用 `InvokeBody::Raw` + `x-pty-id` header（对齐 Terax），前端 `invoke("pty_write", textEncoder.encode(data), { headers: { "x-pty-id": String(id) } })`。
`pty_open` 额外：shell 提前退出时 re-check + reap（防 PTY 孤儿）。

### 终端窗口（前端）

```
crates/desktop/frontend/
├── entries/terminal-main.tsx    # vite entry（mountApp(<Terminal/>)）
├── terminal.html                # HTML（主题恢复 + bg 注入，同 compact-editor.html）
└── pages/Terminal/
    ├── index.tsx                # 主组件：多 tab + 布局切换 + TabButton/SidebarItem + AgentBadge
    ├── TerminalPane.tsx         # 单面板：useTerminalSession + 上报 ptyId + 消费 pendingCommand
    ├── useTerminalSession.ts    # hook：new Terminal + FitAddon + PTY 接线 + ResizeObserver + dispose
    ├── pty-bridge.ts            # openPty → PtySession（write raw body / resize / close）
    └── agent-activity.ts        # 模块级 state + subscribe（替代 zustand）+ finished TTL
```

**简化策略**（相对 Terax）：无 rendererPool 池化、无 dormantRing、无 zustand、无分屏——每 tab 一个 xterm 实例直接管理。tab 切换用 `visibility:hidden` 保活（不卸载 xterm）。

**`index.tsx` 核心逻辑**：
- 多 tab：`tabs: Tab[]`，每 tab 持有 `ptyId | null` + `pendingCommand?` + `customName?`
- 新 tab：`makeTab()` → TerminalPane mount → `useTerminalSession` → openPty → onData 喂 term.write
- 输入：`term.onData(str)` → `pty.write(str)`（raw body + header）
- resize：`term.onResize` → `pty.resize`；`ResizeObserver` 容器变化 → `fitAddon.fit`
- agent 信号：`listen("agent://signal")` → `agent-activity.ts` store → `subscribeAgentActivity` 触发重渲染
- 状态徽章：`AgentBadge` 按 phase 渲染——`working`=amber pulse / `attention`=red bell / `finished`=green / `idle`=隐藏
- ActionBar 联动：`listen("terminal://new-tab" {cwd, command})` → addTab + pendingCommand → TerminalPane 写命令 + 回车
- tab 改名 + 布局切换：详见 [tab 改名/布局 spec](2026-07-31-terminal-tab-rename-layout.md)

### ActionBar 整合

`execute_action_bar_inner` 的 agent 分支：优先内嵌终端，失败 fallback Terminal.app。

```rust
match crate::ui::terminal_window::open_terminal_with_command(&app, Some(&cwd), &command) {
    Ok(_) => log::info!("[action-bar] agent 已启动到内嵌终端"),
    Err(e) => {
        log::warn!("[action-bar] 内嵌终端失败，fallback 到 Terminal.app: {}", e);
        // 旧路径保留做兜底（osascript spawn_blocking）
    }
}
```

`open_terminal_with_command(app, cwd, command)`：
1. `open_terminal_window`（新建或聚焦）——单例，已存在则 show+focus
2. emit `"terminal://new-tab" { cwd, command }` → 前端新 tab + 写命令

内嵌终端可在 async worker 线程安全调用（内部 `run_on_main_thread` 调度 AppKit），无需 spawn_blocking（与原 Terminal.app osascript 路径不同）。

### capabilities

`capabilities/default.json` 加 `terminal_window` 到 windows 数组。

## 依赖

| 新依赖 | 版本 | 用途 |
|---|---|---|
| `portable-pty` | 0.9 | PTY 跨平台 |
| `@xterm/xterm` | 6.x | 终端渲染 |
| `@xterm/addon-fit` | 0.10.x | 自适应尺寸 |
| `@xterm/addon-web-links` | 1.x | 链接点击 |

## 验证

```bash
cargo build -p octopus-pty --lib         # PTY crate 编译
cargo test -p octopus-pty --lib           # PTY 单测
cargo check -p octopus-desktop            # desktop 编译
cd crates/desktop/frontend && npm run build  # 前端编译
# 手动：ActionBar 选 agent → 终端窗口打开 → agent 输出可见 → 输入追问 → 状态徽章变化
```

## 风险

1. **portable-pty macOS 兼容性**：Terax 已验证 Tauri 2 + macOS 全链路可行，风险低 ✅
2. **OSC 777 hook 写入用户配置文件**：幂等 + prune 旧条目 + 不覆盖用户已有 hook——`write_atomic` + `OWNED_MARKERS` + merge 注入，12 测试覆盖 ✅
3. **xterm.js 在 Tauri webview 中的性能**：Terax 用 WebGL renderer；Phase 1 先用默认 Canvas renderer，性能不足再切 WebGL
4. ~~**pty_write raw body**：Tauri 2 的 `ipc::Request` raw body 方案需验证~~ ✅ 已验证——Tauri 2 `InvokeArgs` 接受 `Uint8Array`，`invoke("pty_write", textEncoder.encode(data), { headers: { "x-pty-id": String(id) } })` 可行
5. **shell init 脚本注入**：zsh ZDOTDIR 方案保留用户配置（`OCTOPUS_USER_ZDOTDIR`），starship/p10k 照常工作 ✅
