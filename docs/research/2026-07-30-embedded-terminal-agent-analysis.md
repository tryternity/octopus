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

---

## 八、octopus 实现状态 vs Terax 功能差距（2026-07-31）

> 内嵌终端 Task 1-8 + WebGL + 控制键增强完成后，与 Terax 的完整功能对比。
> 用于后续翻阅、决定下一步补哪些功能。

### 已实现（对齐 Terax）

| 功能 | octopus | Terax | octopus 文件 |
|---|---|---|---|
| PTY spawn + 3 线程（reader/flusher/waiter） | ✅ | ✅ | `pty/src/session.rs` |
| OSC 133/777/9 agent 状态感知（含 auto-arm/finish/溢出防护） | ✅ | ✅ | `pty/src/agent_detect.rs` |
| Shell 集成脚本（zsh ZDOTDIR / bash --rcfile OSC 133 注入） | ✅ | ✅ | `pty/src/shell_init.rs` + `scripts/` |
| Tauri 命令层（pty_open/write/resize/close） | ✅ | ✅ | `commands/terminal_commands.rs` |
| Agent hook 安装（Claude/Codex/Gemini/Pi） | ✅ | ✅ | `commands/agent_hooks.rs` |
| pty_write raw body + x-pty-id header（绕 JSON） | ✅ | ✅ | `pty-bridge.ts` |
| 多 tab（visibility 保活） | ✅ | ✅ | `Terminal/index.tsx` |
| xterm.js + FitAddon + WebLinksAddon | ✅ | ✅ | `useTerminalSession.ts` |
| **WebGL renderer + context loss 250ms 重连 + 隐藏 tab 释放** | ✅ | ✅ | `useTerminalSession.ts::attachWebgl` |
| **macOS 控制键（Option 词导航 / Cmd 行首行尾 / IME 兼容 / Shift+Enter / alternate screen 智能切换）** | ✅ | ✅ | `keymap.ts` |
| agent 状态徽章（working 脉冲 / attention bell / finished 绿点） | ✅ | ✅ | `agent-activity.ts` + `index.tsx::AgentBadge` |
| **终端内搜索（Cmd+F + SearchAddon 增量搜索 + ↑↓ 导航）** | ✅ | ✅ | `SearchOverlay.tsx` + `useTerminalSession.ts` |
| **OSC 7 cwd 追踪（新 tab 继承目录 + 标题显示路径 + 安全过滤）** | ✅ | ✅ | `osc-handlers.ts` + zshrc/bashrc OSC 7 发射 |
| **OSC 133 prompt tracker（inCommand 安全过滤）** | ✅ | ✅ | `osc-handlers.ts::registerPromptTracker` |
| **rAF 节流（巨量输出 Ctrl+C 回到命令行）** | ✅ | ❌ Terax 靠 rendererPool 绕过 | `useTerminalSession.ts` |

### octopus 独有（Terax 没有）

| 功能 | 说明 |
|---|---|
| **tab 改名** | 双击 tab 标题内联编辑（customName > cwdBasename > agentName > 默认） |
| **布局切换** | 顶部 tabs ↔ 左侧 sidebar list（localStorage 持久化） |
| **多窗口** | 托盘「新建终端」多实例（`terminal_<n>`）+ ActionBar agent 单例（`terminal_action_agent`），Terax 是单窗口应用 |
| **文件树侧栏** | 右侧默认隐藏，工具条切换展开/收缩。懒加载 + gitignore 过滤 + dot 文件切换。根目录跟随当前 tab cwd（OSC 7）。Terax 有完整 explorer（git 状态/拖放/CRUD），octopus Phase 1 仅展示 |
| **panel 可调宽度** | sidebar + file-tree 拖拽边缘改宽度，全局 localStorage 记忆（min=50，max 由终端最小宽推导）。Terax 固定宽度 |
| **文件拖放进终端** | 文件树节点拖到终端内容区，插入相对当前 cwd（OSC 7 实时）的 shell 转义路径（不回车）+ 自动聚焦。`relPath`（子树内相对，外部回退绝对）+ `shellEscape`（对齐后端 `shell_escape_single`）。Terax 用 `useTerminalFileDrop.ts` + `quoteShellPath.ts` |
| **Cmd+T / Ctrl+T 新建 tab** | 不区分平台，Cmd 或 Ctrl+T 都支持 |
| **右键菜单（三区域）** | 终端内容区（复制/粘贴/全选/清屏）+ tab 标签（改名/关闭/新建）+ 文件树（复制路径/复制名称）。自绘浮层 `ContextMenu.tsx`，剪贴板走 `document.execCommand`（WKWebView `navigator.clipboard` 实测不可靠），改名复用 `forceEditing` prop（`window.prompt` 在 WKWebView 不工作）。Terax 用系统原生右键菜单 |
| **rAF 节流** | 巨量输出 Ctrl+C 回到命令行（xterm write buffer 积压修复），Terax 靠 rendererPool 绕过 |

### Terax 有、octopus 未实现（按优先级）

#### P1（高性价比，建议下一步）

| 功能 | Terax 文件 | 说明 | octopus 补法 |
|---|---|---|---|
| **字号/字体偏好** | `useTerminalFont.ts` + 设置页 | octopus 固定 13px SF Mono，用户无法调。中频 | config 加 terminalFontSize/Family，`useTerminalFont` 读 |

#### P2（中频需求，看用户反馈）

| 功能 | Terax 文件 | 说明 |
|---|---|---|
| **rendererPool（slot 池化）** | `rendererPool.ts`（~900 行） | 隐藏 tab 保活 WebGL + dormantRing 字节缓冲。octopus WebGL active 释放已兜底，多 tab 不卡就用 |
| **分屏 pane（split）** | `PaneTreeView.tsx` + `panes.ts` | 水平/垂直分割同时看多终端 |

#### P3（重功能或低频，暂缓）

| 功能 | Terax 文件 | 说明 |
|---|---|---|
| **block 模式** | `block/` 整个目录（~2000 行） | prompt-as-editor：CodeMirror 驱动 shell 输入 + 命令块装饰 + 历史 + 路径补全 + inline 建议。很重的特色功能 |
| **OSC 52 剪贴板** | `osc-handlers.ts` | 远程程序（SSH）写本地剪贴板，1MiB 上限 |
| **bracketed paste** | `terminalPaste.ts` | `\x1b[200~...\x1b[201~` 包裹粘贴，部分程序需要 |
| **终端剪贴板** | `terminalClipboard.ts` | OSC 52 + Cmd+C 选中复制（部分 xterm 默认已覆盖） |
| **光标闪烁控制** | `cursorBlink.ts` | 失焦停闪省电 |
| **字体度量** | `useTerminalFont` 精确列宽 | CJK 对齐细节 |
| **shell 选择/历史弹出** | block 模式子功能 | 随 block 模式 |

### 测试覆盖现状（octopus）

| 模块 | 测试数 |
|---|---|
| agent_detect.rs（OSC 状态机） | 17 |
| session.rs（spawn/echo/退出码/Drop） | 3 |
| shell_init.rs（classify/resolve/URL） | 6 |
| agent_hooks.rs（幂等/merge/Pi） | 12 |
| terminal_window.rs（URL/label 匹配） | 9 |
| activation.rs（float_depth/label 匹配） | 3 |
| keymap.ts（Option/Cmd/删除/readline/isFind/isNewTab） | 39 |
| agent-activity.ts（phaseForSignal/displayLabel） | 11 |
| useTerminalSession.ts（attachWebgl 降级/context loss） | 4 |
| osc-handlers.ts（parseOsc7/cwdBasename/updateShellIntegration） | 13 |
| **合计（前端）** | **68** |
| **合计（含 Rust）** | **103** |

Terax 终端模块测试文件（供后续补功能时参考其测试范式）：`keymap.test.ts` / `agentActivity.test.ts` / `cursorBlink.test.ts` / `dormantRing.test.ts` / `liveTerminals.test.ts` / `osc-handlers.test.ts` / `panes.test.ts` / `quoteShellPath.test.ts` / `terminalClipboard.test.ts` / `terminalPaste.test.ts` / `useTerminalFileDrop.test.ts` / `block/lib/` 下 6 个。

---

## 九、多 tab 性能架构对比：octopus vs Terax（2026-07-31）

> 起因：octopus 终端 `yes` 巨量输出时 Ctrl+C 后回不到命令行（xterm write buffer 积压）。修复用 rAF 节流后追问「Terax 怎么解决的」——发现 Terax 根本没这个问题，因为 rendererPool 架构。

### 核心差异：xterm 实例管理策略

| 维度 | octopus（每 tab 常驻 xterm） | Terax（rendererPool 池化） |
|---|---|---|
| 架构 | N 个 tab = **N 个 xterm 实例**（全部常驻内存） | N 个 tab = **~8 个 xterm slot**（固定池） |
| 隐藏 tab 的 PTY 输出 | 仍走 `term.write`（rAF 节流兜底） | 进 **dormantRing**（1MiB 环形缓冲），不碰 xterm |
| 切回 tab | 瞬间（xterm 还在 DOM，visibility 保活） | 有延迟（serializeAddon 快照恢复 + dormantRing drain 补播漏掉的输出） |
| WebGL context | active 切换 dispose/attach（已实现） | slot park/evict 天然管理 |
| 内存 | O(tab 数)——每 tab 一个完整 xterm + scrollback | O(固定 slot 数)——超出 slot 数的 tab evict 释放 |
| 实现复杂度 | ~300 行 useTerminalSession | ~1200 行（rendererPool + dormantRing + serializeAddon + slot 调度） |

### Terax rendererPool 工作原理（`rendererPool.ts` ~900 行）

1. **固定 slot 池**：创建有限数量的 xterm slot（默认 ~8），每个 slot 绑定一个 leaf（tab 的终端实例）
2. **leaf → slot 绑定**：活跃 tab 的 leaf 绑定到 slot，xterm 实时渲染
3. **隐藏 tab park**：tab 切走时 slot 进入 park 状态——保留 xterm buffer（scrollback 不丢），暂停渲染（省 GPU/CPU）
4. **slot 不够时 evict**：如果所有 slot 都被占用且来了新 tab，按 evictionScore（可见性/alt-screen/busy/focus 加权）选最低分的 slot evict——释放 xterm 实例
5. **evict 前快照**：`serializeAddon.serialize({ scrollback: cap })` 保存当前 buffer 快照 + 光标位置
6. **切回 evicted tab 恢复**：重新 acquire slot → `term.write(snapshot)` 恢复快照 → `dormantRing.drain()` 补播隐藏期间漏掉的输出
7. **dormantRing 背压**：隐藏期间 PTY 输出进环形缓冲（1MiB 上限），溢出丢旧行（对齐 LF 边界），不会无限增长

### octopus 的 rAF 节流方案（当前架构下的补偿）

```typescript
// useTerminalSession.ts：每帧（~16ms）最多 term.write 一次
// 积压时只保留最新块丢弃中间，避免 xterm write buffer 无限积压
let pendingOutput: Uint8Array | null = null;
let rafScheduled = false;
const flushOutput = () => {
  rafScheduled = false;
  if (disposed || !pendingOutput) return;
  const data = pendingOutput;
  pendingOutput = null;
  term.write(data);
};
onData: (bytes) => {
  pendingOutput = bytes;
  if (!rafScheduled) {
    rafScheduled = true;
    requestAnimationFrame(flushOutput);
  }
}
```

**效果**：
- 正常输出（prompt/命令回显）：每帧 flush，无感知延迟
- 巨量输出（yes/seq）：每帧只 write 最新块，xterm buffer 每帧最多积压一块
- Ctrl+C 后 xterm 很快消化完（最多一帧的数据量），prompt 立即显示
- htop（alternate screen）：每帧重绘量小，不受影响

**为什么不用 xterm write callback 做反压**（踩过的坑）：`term.write(data, callback)` 的 callback 在数据解析完后触发，但**不可靠**——某些情况不触发，导致 `writeBusy` 永久 true，新终端只能响应首键。rAF 用浏览器渲染帧节拍，稳定可靠。

### 为什么 octopus 需要 rAF 而 Terax 不需要

Terax 的隐藏 tab 输出不经过 xterm（进 dormantRing），所以不存在 write buffer 积压问题。octopus 每 tab 常驻 xterm，隐藏 tab 的输出仍走 `term.write`——必须前端限速。

### 演进路径

| 阶段 | 方案 | 适用场景 |
|---|---|---|
| **当前** | 每 tab 常驻 xterm + rAF 节流 | 少量 tab（< 10）日常使用 |
| **P2 备选** | 引入 rendererPool（移植 Terax slot 池化 + dormantRing） | 重度多 tab（> 10）+ 后台长时间跑命令 |

引入 rendererPool 后 rAF 节流可移除（rendererPool 的 dormantRing 接管隐藏 tab 输出缓冲）。代价是 ~900 行额外复杂度 + 切回 tab 有快照恢复延迟。

### tradeoff 决策记录

octopus Phase 1 选择「简单架构 + rAF 兜底」而非 rendererPool，理由：
1. octopus 是 AI 办公入口（agent CLI 为主），用户极少同时开 > 10 个终端 tab
2. rendererPool 的切回延迟（快照恢复 + drain）对交互体验有损
3. rAF 节流已解决最严重的 Ctrl+C 积压问题，日常使用无感知
4. 保留演进路径——如用户反馈 tab 多卡，再上 rendererPool（功能差距对比 P2）

