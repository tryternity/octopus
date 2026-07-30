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
- ✅ OSC 133 shell prompt marker（通用，检测命令开始/结束）
- ✅ OSC 777 agent hook（Claude/Codex/Pi，检测 working/attention/finished）
- ✅ ActionBar agent 替换：选 agent 后打开内嵌终端（不再开 Terminal.app）
- ❌ 手机遥控 WebSocket（Phase 2）
- ❌ shell 一次性/session/bg 三模式（Terax 的 shell 模块，暂不做）
- ❌ renderer pool（xterm.js 实例复用，暂不做——单 session 单实例）

## 架构

### 新增 crate：`crates/pty/`

```
crates/pty/
├── Cargo.toml          # portable-pty 0.9, tokio, log
└── src/
    ├── lib.rs          # 模块导出 + PtyState（session 注册表）
    ├── session.rs      # PtySession + spawn（3 线程）+ read/write/resize/kill
    └── agent_detect.rs # AgentDetector OSC 状态机 + Transition/AgentSignal
```

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

**`AgentDetector` 状态机**：`Ground / Esc / Osc / OscEsc`

解析三种 OSC 序列：

**OSC 133**（shell prompt marker，由 zshrc/bashrc preexec hook 发出）：
- `133;C;<cmd>` → 匹配 `claude/codex/gemini/pi/opencode` → emit `Started { agent }`
- `133;D` → emit `Exited`

**OSC 777**（agent hook 主动通知，由 agent 配置文件的 hook 发出）：
- `777;notify;octopus;working` → emit `Working`
- `777;notify;octopus;attention` → emit `Attention`
- `777;notify;octopus;finished` → emit `Finished`

**`AgentSignal`**：
```rust
pub struct AgentSignal {
    pub id: u32,       // PTY session id
    pub kind: String,  // "started" | "working" | "attention" | "finished" | "exited"
    pub agent: Option<String>,  // agent 名称（仅 started 携带）
}
```

### Agent Hook 安装（参考 Terax agent.rs）

为每个 agent 写配置文件，注入 OSC 777 hook。注入 `$OCTOPUS_TERMINAL=1` 环境变量，让 hook 只在 octopus PTY 中发 OSC。

**Claude Code** → `~/.claude/settings.json`：
```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "[ -n \"$OCTOPUS_TERMINAL\" ] && printf '\u001b]777;notify;octopus;working\u0007' || true"}]}],
    "Notification": [{"hooks": [{"type": "command", "command": "[ -n \"$OCTOPUS_TERMINAL\" ] && printf '\u001b]777;notify;octopus;attention\u0007' || true"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "[ -n \"$OCTOPUS_TERMINAL\" ] && printf '\u001b]777;notify;octopus;finished\u0007' || true"}]}]
  }
}
```

**Codex CLI** → `~/.codex/hooks.json`：同模式，用 `/dev/tty` 输出。

**Pi** → `~/.pi/agent/extensions/octopus-notifications.ts`：TS 扩展。

Hook 命令安装函数 `agent_enable_hooks(agent: String)`：
- 原子写入（tmp + rename）
- 幂等（用 `OWNED_MARKERS` 识别并 prune 旧条目再重插）
- 不覆盖用户已有 hook（merge 注入）

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
├── terminal_window.rs       # 终端窗口创建/管理（参考 compact_editor_window）
├── terminal_commands.rs     # Tauri 命令
└── agent_hooks.rs           # Agent hook 安装
```

**Tauri 命令**：
```rust
#[tauri::command]
async fn pty_open(
    app: AppHandle,
    state: State<PtyState>,
    cols: u16, rows: u16,
    cwd: Option<String>,
    shell: Option<String>,
    on_data: Channel<Vec<u8>>,
    on_exit: Channel<i32>,
) -> Result<u32, String>

#[tauri::command]
async fn pty_write(state: State<PtyState>, id: u32, data: Vec<u8>) -> Result<(), String>

#[tauri::command]
async fn pty_resize(state: State<PtyState>, id: u32, cols: u16, rows: u16) -> Result<(), String>

#[tauri::command]
async fn pty_close(state: State<PtyState>, id: u32) -> Result<(), String>

#[tauri::command]
async fn agent_enable_hooks(agent: String) -> Result<(), String>
```

`pty_write` 使用 raw body（绕过 JSON）——参考 Terax 的 `x-pty-id` header 方案。

### 终端窗口（前端）

```
crates/desktop/frontend/
├── entries/terminal-main.tsx    # vite entry
├── terminal.html                # HTML（主题恢复 + bg 注入，同 compact-editor.html）
└── pages/Terminal/
    ├── index.tsx                # 主组件：多 tab + xterm.js + agent 状态
    ├── pty-bridge.ts            # Tauri ↔ PTY 桥（Channel + invoke pty_write）
    └── AgentStatusBadge.tsx     # agent 状态徽章（working/attention/idle）
```

**`index.tsx` 核心逻辑**：
- 多 tab：`tabs: TerminalTab[]`，每 tab 持有 `ptyId` + `xterm Terminal` + `agentPhase`
- 新 tab：创建 xterm.js Terminal → invoke `pty_open`（传 cols/rows）→ `onData` → `term.write(bytes)`
- 输入：`term.onData(str)` → `invoke("pty_write", { id, data: new TextEncoder().encode(str) })`
- resize：`term.onResize({ cols, rows })` → `invoke("pty_resize", { id, cols, rows })`
- agent 信号：`listen("agent://signal", (e) => { 更新对应 tab 的 agentPhase })`
- 状态徽章：`agentPhase` 映射颜色——`working`=amber pulse / `attention`=red bell / `idle`=灰色

### ActionBar 整合

`execute_action_bar_inner` 的 agent 分支：

```rust
// 当前（fire-and-forget Terminal.app）
let launcher = TerminalAppLauncher;
launcher.spawn(&command, &cwd_buf)?;

// 改为（内嵌终端窗口）
crate::terminal_window::open_terminal_with_command(app, &cwd, &command)?;
```

`open_terminal_with_command`：
1. 打开终端窗口（或聚焦已有）
2. 在新 tab 中 spawn PTY session（cwd + command）
3. 如果是已识别的 agent（claude/codex/pi），自动安装 hook

Terminal.app 路径保留为 fallback（终端窗口创建失败时）。

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

1. **portable-pty macOS 兼容性**：Terax 已验证 Tauri 2 + macOS 全链路可行，风险低
2. **OSC 777 hook 写入用户配置文件**：需幂等 + prune 旧条目 + 不覆盖用户已有 hook——参考 Terax `write_if_changed` + `OWNED_MARKERS`
3. **xterm.js 在 Tauri webview 中的性能**：Terax 用 WebGL renderer；Phase 1 先用默认 Canvas renderer，性能不足再切 WebGL
4. **pty_write raw body**：Tauri 2 的 `ipc::Request` raw body 方案需验证——如果 Tauri 2 不支持 header + raw body，退化为 base64（性能略降但可接受）
5. **shell init 脚本注入**：zsh 临时 ZDOTDIR 方案可能与用户已有 .zshrc 冲突——需测试 + fallback 到 $HOME/.zshrc source
