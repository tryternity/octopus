# 集成终端与 Agent 状态感知调研

**日期**：2026-07-30
**目标**：为 octopus 内嵌终端 + agent 交互 + 手机遥控场景选型
**调研对象**：Terax / Codux / Zed / OxideTerm / Alacritty / Zellij（6 个项目源码深度分析）

---

## 一、各项目核心架构对比

### PTY 后端选型

| 项目 | PTY 库 | VT 解析 | 输出传输到前端 | License |
|---|---|---|---|---|
| **Terax** | `portable-pty` | xterm.js（前端 JS） | Tauri `Channel<Response>` 原始字节 | Apache-2.0 |
| **Codux** | `portable-pty` | `alacritty_terminal` Term + vte（后端 Rust） | `TerminalEvent::Output` emit → 前端渲染 | GPL-3.0 |
| **Zed** | alacritty fork 自带 `tty`（非 portable-pty） | `alacritty_terminal` Term + vte | GPUI poll + `snapshot()` | GPL-3.0 |
| **OxideTerm** | alacritty `tty`（非 portable-pty） | `alacritty_terminal` Term + vte | GPUI poll + `snapshot()` | Apache-2.0 |
| **Alacritty** | 自研 `tty`（rustix_openpty + libc） | `vte::ansi::Processor` + `Term` | winit EventLoop | Apache-2.0 |
| **Zellij** | 自研（nix::pty::openpty + login_tty） | `vte` crate（tokenizer 层） | tokio AsyncFd → Screen 线程 → grid | MIT |

**关键发现**：PTY 后端分两派——`portable-pty`（Terax/Codux）和 `alacritty tty`（Zed/OxideTerm/Alacritty）。portable-pty 更易用（跨平台 API 统一），alacritty tty 更底层（自己 fork/exec + signal_hook）。

### Agent 状态感知

| 项目 | 感知方式 | 精度 | 复杂度 |
|---|---|---|---|
| **Terax** | OSC 序列（133 shell prompt marker + 777 agent hook） | 🟢 高（agent 主动通知 working/attention/finished） | 🟡 中（需给每个 agent 写 hook 配置文件） |
| **Codux** | 5 路信号融合（进程树 + OSC + 屏幕匹配 + transcript 文件 + wrapper hook） | 🟢 最高（多信号互验 + token 统计） | 🔴 高（每个 agent 一张 driver 表 + probe 解析器） |
| **Zed** | PTY exit code + completion_tx（仅知道命令结束，不跟踪 agent 运行中状态） | 🔴 低（agent 运行中无感知） | 🟢 低 |
| **Zellij** | 无（不感知 agent，exit code 给插件） | 🔴 无 | 🟢 零 |
| **OxideTerm** | 无 agent 感知 | 🔴 无 | 🟢 零 |
| **Alacritty** | 无 agent 感知（纯终端模拟器） | 🔴 无 | 🟢 零 |

---

## 二、Terax 架构详解（与 octopus 最相关——同 Tauri 2 技术栈）

### PTY 模块（`src-tauri/src/modules/pty/`）

**三线程模型**：
1. **reader 线程**：16KB 块读 PTY → 过滤 DA/CPR 查询 → agent_detect OSC 解析 → push 到 pending buffer（4MiB 上限反压）
2. **flusher 线程**：Condvar 等待 → 4ms coalesce → `Channel<Response>` 发送（最大 50ms idle 触发）
3. **waiter 线程**：`child.wait()` → 等 reader EOF → flush tail → `Channel<i32>` 发 exit code → drop session

**关键设计点**：
- `pty_write` 绕过 JSON——读 `x-pty-id` header + raw body 直接写 writer（低延迟按键路径）
- Windows ConPTY 用 `SPAWN_LOCK` 串行化 + Job Object 进程树清理（`KILL_ON_JOB_CLOSE`）
- DA 过滤器：启动期拦截 DA1/DA2/DA3/CPR 查询并自动应答（防 pwsh/PSReadLine 启动阻塞）

### Agent 状态感知（纯 OSC 序列，无进程轮询）

**两层信号**：

**第一层 OSC 133**（shell prompt marker）——zshrc/bashrc preexec hook 发出：
- `133;C;<cmd>` → 匹配 `claude/codex/gemini/pi/opencode/grok` → emit `Started { agent }`
- `133;D` → emit `Exited`

**第二层 OSC 777**（agent hook 主动通知）——为每个 agent 写配置文件：
- Claude → `.claude/settings.json`：`UserPromptSubmit→working`、`Notification→attention`、`Stop→finished`
- Codex → `.codex/hooks.json`：同模式
- Gemini → `.gemini/settings.json`：同模式
- Pi → `.pi/agent/extensions/terax-notifications.ts`：TS 扩展

**关键**：注入 `$TERAX_TERMINAL=1` 环境变量，让 hook 只在 Terax PTY 中发 OSC（外部终端静默）。

### Shell 模块（三种执行模式）

| 模式 | 用途 | 有 TTY | 输出方式 |
|---|---|---|---|
| `pty` | 交互式终端 + agent UI | ✅ | Channel 流式 |
| `shell_run_command` | AI tool-call（一次性） | ❌ | 返回结构化结果（256KiB 截断 + 30s 超时） |
| `shell_session_*` | 持久 agent shell（跨调用保 cwd） | ❌ | sentinel 标记解析 cwd |
| `shell_bg_*` | 后台进程（dev server） | ❌ | 4MiB 环形缓冲 + offset 轮询 |

---

## 三、Codux 架构详解（agent 状态感知最完整）

### 5 路信号融合

| 信号源 | 方法 | 精度 | 侵入性 |
|---|---|---|---|
| **进程树探测** | `ps -axo pid,ppid,command` DFS 子进程树匹配 agent 可执行名 | 中（知道在跑什么 agent） | 零 |
| **OSC 序列** | 自写 parser 扫 PTY 字节流，识别 OSC 133/9;4/0 | 高（实时状态变化） | 零（只读） |
| **屏幕模式匹配** | 渲染后屏幕最后 16 行匹配 waiting/running 字符串 | 中（fallback） | 零 |
| **transcript 文件** | 监控 agent 的 session/状态文件变化（如 codex rollout-*.jsonl） | 最高（权威 runtime state + token） | 零（只读文件） |
| **wrapper hook** | 给 agent CLI 注入 hook 调 codux-wrapper-helper 二进制 | 高 | 中（写 agent 配置） |

### 每 agent 一张 driver 表

```rust
struct AIRuntimeToolDriver {
    id, aliases, process_names,   // 进程匹配
    screen_patterns,              // 屏幕匹配字符串
    probe: Option<AIRuntimeProbeFn>,  // transcript 解析
    lifecycle_hooks,              // hook 配置
    memory_injection,            // agent 记忆注入
}
```

### Token 统计

两条路径：
1. **实时**：probe 解析 agent 状态文件中的 usage 字段
2. **历史**：`codux-ai-history` SQLite 索引引擎，解析每个 CLI 的 session 历史

---

## 四、Zed 架构详解（AI ↔ 终端桥接最干净）

### 核心链路

```
模型调用 terminal tool
  → ThreadEnvironment::create_terminal()
  → AcpThread::create_terminal()
    → ShellBuilder 包命令（redirect_stdin_to_dev_null 防挂起）
    → prepare_sandbox_wrap（可选 OS 沙箱）
    → Project::create_terminal_task(SpawnInTerminal)
      → TerminalBuilder::new(completion_tx)
        → alacritty tty open_pty + EventLoop::spawn
  → terminal.wait_for_exit()
    → completion_tx.send(ExitStatus)
  → current_output() 截断后返回给模型
```

**关键设计**：
- agent 命令 stdin 重定向到 `/dev/null`（防 PTY 挂起等待输入）
- 注入 `PAGER=""` + `GIT_PAGER=cat`（防分页器阻塞）
- `completion_tx: Sender<Option<ExitStatus>>` 是 agent 拿 exit code 的钩子
- `Shared<Task<TerminalExitStatus>>` 让多个等待方共享同一个退出 future

### Zed 的进程状态追踪（三层）

1. **PTY 前台进程组**（`tcgetpgrp`）→ `sysinfo` 读 cwd/argv/name（刷新标题）
2. **ExitStatus 传递**（alacritty event loop 检测 EOF → `completion_tx`）
3. **Agent 等待层**（`Shared<Task>` + timeout + output 快照）

---

## 五、alacritty_terminal crate API（可复用的终端仿真库）

### 核心类型

```rust
// 构造
Term::new(config, &size, event_proxy) → Term<T>

// 喂数据（实现 vte::ansi::Handler）
let mut parser = Processor::new();
parser.advance(&mut *term.lock(), bytes);

// 读取 grid
term.grid() → &Grid<Cell>
term.grid()[Line(..)][Column(..)] → Cell { c: char, fg: Color, bg: Color, flags: Flags }
term.renderable_content() → RenderableContent (cursor + grid)
term.damage() → TermDamage (增量损坏信息)

// 进程管理
EventLoop::new(term, listener, pty, drain_on_exit).spawn() → IO 线程
Notifier(loop_tx) → 实现 Notify（写 stdin）+ OnResize（resize）
```

### 最小集成（~40 行）

```rust
let terminal = Term::new(config, &size, event_proxy);
let terminal = Arc::new(FairMutex::new(terminal));
let pty = tty::new(&options, window_size, window_id)?;
let event_loop = EventLoop::new(Arc::clone(&terminal), event_proxy, pty, true, false)?;
let loop_tx = event_loop.channel();
let _io = event_loop.spawn();
// UI 侧用 Notifier(loop_tx) 写输入/resize
// terminal 通过 event_proxy 收 Event::Wakeup 触发重绘
```

---

## 六、对 octopus 的架构建议

### 技术选型

| 维度 | 推荐 | 理由 |
|---|---|---|
| **PTY 库** | `portable-pty` | Terax 验证了 Tauri 2 + portable-pty 全链路可行；API 比 alacritty tty 简单（不需要自己 fork/exec + signal_hook） |
| **VT 解析** | 方案 A：xterm.js 前端（同 Terax）；方案 B：`alacritty_terminal` 后端 + 自定义面板（同 Codux） | xterm.js 省事但 200KB+；alacritty_terminal 后端解析+grid snapshot 更可控 |
| **输出传输** | Tauri `Channel<Vec<u8>>` | Terax 验证了 Channel 原始字节传输（绕过 JSON/base64）低延迟可行 |
| **Agent 状态感知** | Terax 模式（OSC 133 + OSC 777 agent hook） | 最平衡——精度够高（agent 主动通知），侵入性可控（每个 agent 写一个 hook 配置） |

### 推荐架构

```
crates/
└── pty/                    # 新 crate：PTY session 管理
    ├── src/
    │   ├── lib.rs          # PtySession + PtyEvent
    │   ├── session.rs      # portable-pty 封装（参考 Terax session.rs）
    │   └── agent_detect.rs # OSC 133/777 解析（参考 Terax agent_detect.rs）
    └── Cargo.toml          # portable-pty 依赖

crates/desktop/src/
├── pty_commands.rs         # Tauri 命令：pty_open/write/resize/close
└── agent_hooks.rs          # agent hook 安装（参考 Terax agent.rs）
```

### Agent 状态感知方案（参考 Terax）

```
用户在 octopus 启动 agent CLI（如 claude）→
  portable-pty spawn shell + agent →
  shell zshrc preexec hook 发 OSC 133;C;claude →
  agent_detect.rs 解析到 → emit "agent://signal" { kind: "started", agent: "claude" } →
  前端收到 → 标记 tab 状态为 "working"

agent 运行中 →
  Claude hook（.claude/settings.json）发 OSC 777;notify;octopus;working →
  agent_detect.rs 解析 → emit "agent://signal" { kind: "working" } →

agent 需要用户确认 →
  Claude hook 发 OSC 777;notify;octopus;attention →
  emit "agent://signal" { kind: "attention" } →
  前端弹通知 + 手机推送（未来）

agent 完成 →
  Claude hook 发 OSC 777;notify;octopus;finished →
  emit "agent://signal" { kind: "finished" } →
  前端标记 idle + 可选通知
```

### 手机遥控场景的架构启示

octopus 的"手机遥控 agent"需要：

1. **状态推送**：agent 状态变化时通知手机（OSC → emit → WebSocket → 手机）——Terax 的 OSC 777 模式完美适配
2. **远程输入**：手机发文本 → WebSocket → 写 PTY stdin ——Terax 的 `pty_write` 原始字节路径可复用
3. **输出同步**：agent 输出实时推送到手机 ——Channel + WebSocket 桥接
4. **会话管理**：agent session 生命周期（创建/恢复/销毁）——portable-pty session + ID 管理

### 渐进式实施建议

| 阶段 | 内容 | 预估 |
|---|---|---|
| **Phase 1** | `portable-pty` + 基本 PTY session（spawn/read/write/resize/kill） | 2-3 天 |
| **Phase 2** | xterm.js 前端面板 + Tauri Channel 传输 | 2-3 天 |
| **Phase 3** | OSC 133/777 agent 状态感知 + hook 安装 | 2-3 天 |
| **Phase 4** | 替换 ActionBar Terminal.app spawn → 内嵌 PTY | 1-2 天 |
| **Phase 5** | 手机遥控（WebSocket 桥接 PTY + 状态推送） | 3-5 天 |

---

## 七、不推荐的方案

| 方案 | 理由 |
|---|---|
| Fork Terax/Codux 整个项目 | 产品方向不同（它们是 ADE/IDE，octopus 是 AI 办公入口） |
| 用 alacritty_terminal 做后端 VT 解析 | 需要自己做 grid → 渲染的桥接（Codux 2787 行 headless_screen.rs），工程量大 |
| 用 Zellij 的自研 PTY | 过度工程化（nix::pty + login_tty + tokio AsyncFd），portable-pty 已封装 |
| Zed 的 ACP 协议 | 过重（完整 agent-client-protocol JSON-RPC），octopus 不需要标准化的 agent 协议 |
| Codux 的 5 路信号融合 | 过于复杂（每 agent 一张 driver 表 + transcript probe），Terax 的 OSC 模式已够用 |
