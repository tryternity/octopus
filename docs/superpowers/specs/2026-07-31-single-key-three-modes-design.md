# 单键三模式（PTT/toggle/hands-free）状态机 — 设计规格

- **日期**：2026-07-31
- **类型**：重构（交互模式 + 状态机 + coordinator 扩展）
- **范围**：单键（右 Alt 等）覆盖全部录音交互——长按=PTT、双击=toggle、短按=hands-free
- **前置**：talk (PTT) 基础已落地（handy-keys + instant 浮窗 + coordinator InstantStart/Stop）

## 核心设计：单键三模式

一个键（默认右 Alt）通过**按键时长 + 双击检测**区分三种模式：

| 当前状态 | 短按（<260ms 松开） | 长按（≥260ms 不松） | 双击（260ms 内两次按下） |
|---|---|---|---|
| **idle** | → hands-free toggle | → PTT（松开识别+粘贴） | → toggle（弹 result_window） |
| **toggle 录音中** | → 立即润色（录音继续） | → 结束 toggle | → 结束 toggle |
| **hands-free 录音中** | → 停止（任何操作都停） | → 停止 | → 停止 |
| **PTT 录音中** | —（按着键呢） | keyup → 停止+粘贴 | — |

> **toggle 录音中单击/双击必须区分**（2026-07-31 澄清）：单击 = 润色（录音继续），
> 双击 = 结束录音。两者都基于短按，区别在 260ms 内是否有第二次 keydown。实现要点：
> toggle 中第一次短按 keyup **不立即润色**，先进 `ShortPressWait` 等判定——
> 260ms 内再 keydown = 双击 → 结束；超时无第二次 = 单击 → 润色。

### 常量

```rust
const TAP_TIMEOUT_MS: u64 = 260;  // 短按/双击判定窗口（后续可开放为配置项）
```

### 按键

默认右 Alt（handy-keys 名称 `OptRight`），可选：OptRight / ShiftRight / Fn / CtrlRight / CmdRight。
通过 `ptt_key` 配置字段控制（DB seed 默认值需从 `AltRight` 改为 `OptRight` 对齐 handy-keys）。

## PTT 状态机（ptt.rs 重构）

7 态有限状态机，替代当前简单的 Pressed→start / Released→stop：

```rust
enum PttFsm {
    Idle,
    Pending { timer_start: Instant },          // keydown 后等判定：长按 or 短按
    ShortPressWait { timer_start: Instant },   // 真 idle 短按松开后等判定：双击 or 确认 hands-free
    PttRecording,                               // PTT 录音中（长按已确认）
    ToggleInWait { timer_start: Instant },      // toggle 录音中 keydown，等判定：长按结束 or 短按
    ToggleShortWait { timer_start: Instant },   // toggle 录音中短按松开后等判定：双击结束 or 单击润色
    HandsFreeInWait { timer_start: Instant },   // hands-free 中按键，等判定（任何结果都停）
}
```

### 状态转移逻辑

```
idle + keydown → Pending(t0)
pending:
  t ≥ TAP_TIMEOUT → PttRecording（长按确认，coordinator.instant_start）
  keyup < TAP_TIMEOUT → ShortPressWait(t1)

short_press_wait:
  t1 内 keydown → 双击 → coordinator.toggle()（开始 toggle 录音）→ Idle
  t1 超时 → coordinator.hands_free_toggle()（开始/停止 hands-free）→ Idle

ptt_recording + keyup → coordinator.instant_stop() → Idle

toggle 录音中（RECORDING_MODE==1）+ keydown → ToggleInWait(t2)
toggle_in_wait:
  keyup < TAP_TIMEOUT → 短按 → ToggleShortWait(t2')（不立即润色，等双击判定）
  t2 超时 → coordinator.toggle()（长按结束 toggle）→ Idle
toggle_short_wait:
  t2' 内 keydown → 双击 → coordinator.toggle()（结束 toggle）→ Idle
  t2' 超时 → 单击确认 → coordinator.polish_now()（润色，toggle 继续录音）→ Idle

hands-free 中（RECORDING_MODE==3）+ keydown → HandsFreeInWait(t3)
hands_free_in_wait:
  keyup < TAP_TIMEOUT → coordinator.hands_free_stop() → Idle
  t3 超时 → coordinator.hands_free_stop() → Idle（任何操作都停）
```

## Coordinator 新增

### RECORDING_MODE（AtomicU8）

```rust
static RECORDING_MODE: AtomicU8 = AtomicU8::new(0);
// 0 = idle, 1 = toggle, 2 = ptt, 3 = hands_free

pub fn recording_mode() -> u8 { RECORDING_MODE.load(Ordering::Relaxed) }
```

PTT 状态机通过此静态量读取当前录音状态（不需 channel 通信）。

设置时机：
- `Command::Toggle` Idle 分支：设 1
- `Command::InstantStart`：设 2
- `Command::HandsFreeStart`：设 3
- 回 Idle 时（PasteDone/Cancel/Discard/空文本）：设 0

### HandsFreeStart / HandsFreeStop 命令

```rust
Command::HandsFreeStart,
Command::HandsFreeStop,
```

- `HandsFreeStart`（Idle 态）：`save_frontmost_pid()` + `show_instant_overlay("listening")` + `begin_recording`（设 RECORDING_MODE=3）
- `HandsFreeStop`（活跃态）：停录 + finalize + 粘贴尾段 + hide 浮窗 + 回 Idle

## Hands-free 模式行为

### 与 PTT/toggle 的区别

| 维度 | toggle | PTT | hands-free |
|---|---|---|---|
| 录音时长 | 用户手动停 | 按住期间 | 常驻直到用户停 |
| VAD 切段 | 有（伪流式） | 有（伪流式） | 有（**每段自动粘贴**） |
| 浮窗 | result_window（CM6 可编辑） | instant 浮窗 | instant 浮窗 |
| 润色 | polish_mode 决定 | 同 | 同 |
| 结束后 | 粘贴 + result_window | 粘贴 + hide 浮窗 | 粘贴尾段 + hide 浮窗 |

### 录音行为（本次实现：单段 + 自动超时）

hands-free 录音常驻，**两种停止方式**：
1. 用户按键（短按/长按/双击都触发 HandsFreeStop）
2. **静音超 60 秒自动停**（`HANDS_FREE_SILENCE_TIMEOUT_SECS = 60`）——避免忘关一直录

停止后走与 PTT 相同的 finalize 路径（尾段 drain + finalize_after_stop → do_paste →
hide 浮窗 → Idle），**一次性粘贴全部文本**。

### ~~VAD 自动切段粘贴~~（推迟，不在本次范围）

> spec 原设想：VadSegmentedPipeline 每段识别完自动粘贴（不等人停）。实现路径复杂
> （需 paste-and-continue 机制：PasteDone 后不回 Idle 而重开录音），涉及 lifecycle/
> paste/tick 多模块改动，风险大。本次先交付单段粘贴 + 静音超时兜底；段自动粘贴作为
> 后续迭代（待手感验证后再决定是否需要）。

### 静音超时实现

`dispatch_tick` 的 VadSegmented 分支里，hands-free 模式（RECORDING_MODE==3）下
读 `pipeline.silence_duration()`（VAD 累积静音秒数）≥ `HANDS_FREE_SILENCE_TIMEOUT_SECS`
→ 自动发 `Command::HandsFreeStop`。与用户按键等价。

### 实现要点

hands-free 复用 instant 模式的浮窗 + finalize 路径：
- `HandsFreeStart`：set_instant_mode(true) + RECORDING_MODE=3 + begin_recording（instant 浮窗 listening）
- `HandsFreeStop`：停录（handle_toggle 活跃态分支）+ finalize → paste → Idle
- finalize / do_paste 读 INSTANT_MODE + RECORDING_MODE，行为与 PTT 一致（instant 浮窗 done → hide）

## 不变量

1. toggle 模式完全不变（PTT 状态机的双击触发同一个 `coordinator.toggle()`）
2. PTT（长按）行为不变（经状态机判定后触发 instant_start/stop）
3. `TAP_TIMEOUT_MS` 是常量（后续可开放为配置项）
4. coordinator 的 stage 是单线程的，PTT 状态机通过 `RECORDING_MODE` AtomicU8 读取
5. asr_shortcut（Alt+Shift+A）保留——toggle 的备用入口

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/desktop/src/platform/ptt.rs` | **重写** handle_hotkey_event → 6 态状态机 + TAP_TIMEOUT_MS |
| `crates/desktop/src/engine/coordinator/mod.rs` | 加 HandsFreeStart/Stop + RECORDING_MODE + recording_mode() + Command 分发 + set_recording_mode |
| `crates/desktop/src/engine/coordinator/session.rs` | begin_recording 无需改（instant 标志 + RECORDING_MODE 在 mod.rs 命令分支设） |
| `crates/desktop/src/engine/coordinator/tick.rs` | VadSegmented 分支加 hands-free 静音超时检测 → HandsFreeStop |
| `crates/desktop/src/engine/pipeline.rs` | VadSegmentedPipeline 暴露 `silence_duration()`（已有字段，加 pub getter） |
| `crates/infra/src/db.sql` | ptt_key seed 从 AltRight 改 OptRight |
| `crates/infra/src/config.rs` | default_ptt_key + 注释 |
| `docs/architecture.md` | 更新 |

> 注：本次实现 hands-free = 单段粘贴（停录后一次性粘贴全部）+ 静音 60s 超时兜底。
> finalize_after_stop / do_paste / PasteDone **无需加 hands-free 分支**——复用 instant
> 路径（INSTANT_MODE 已覆盖浮窗 + 跳 result_window）。VAD 自动段粘贴推迟（见上文）。

## 实现状态（2026-07-31 完成）

### 已实现

- ✅ **PttFsm 7 态状态机**（ptt.rs 重写）：Idle / Pending / ShortPressWait /
  PttRecording / ToggleInWait / **ToggleShortWait** / HandsFreeInWait。常驻 manager
  线程局部变量。FSM 逻辑抽为纯方法 `next_on_keydown` / `next_on_keyup` /
  `next_on_timeout`（返回 `(PttFsm, FsmAction)`，不依赖 Coordinator），便于单测。
- ✅ **TAP_TIMEOUT_MS = 260**（常量，后续可开放为配置项）。
- ✅ **超时驱动**（`drive_timeouts`）：manager 循环每 ~10ms 检查计时态超时，触发
  Pending→PttRecording（长按）/ ShortPressWait→hands-free（短按确认）/
  ToggleInWait→toggle 结束（长按）/ **ToggleShortWait→polish_now（单击确认润色）** /
  HandsFreeInWait→hands_free_stop。
- ✅ **RECORDING_MODE AtomicU8**（mod.rs）：`recording_mode()` pub fn 供 ptt.rs 读，
  `set_recording_mode()` 在命令分支设值（Toggle Idle→1, InstantStart→2,
  HandsFreeStart→3, 各 Idle 回归点→0）。
- ✅ **HandsFreeStart / HandsFreeStop 命令** + Coordinator 公开方法
  `hands_free_start()` / `hands_free_stop()`。
- ✅ **静音 60s 超时兜底**（tick.rs VadSegmented 分支）：`HANDS_FREE_SILENCE_TIMEOUT_SECS = 60`，
  hands-free 录音中 VAD 累积静音 ≥ 阈值 → 自动发 HandsFreeStop。
- ✅ **RECORDING_MODE 归零点**：PasteDone / Cancel / Discard / finalize 空文本 /
  AgentBridge 派发后 / finalize_cloud 空文本，全部补 `set_recording_mode(0)`。
- ✅ **db.sql seed**：`ptt_key` 从 `AltRight` 改 `OptRight`（handy-keys 语义对齐）。
  注：未升 schema version（AltRight 仍可解析，旧库无需清重建）。
- ✅ **pipeline.rs**：`VadSegmentedPipeline::silence_duration()` pub(crate) getter。
- ✅ **测试**：14 个新测试（PttFsm 初始态 + timed_out 边界 + RECORDING_MODE set/read
  + **toggle 单击/双击/长按行为回归** + 真 idle 双击启动 toggle + FsmAction eq）。

### 偏差与决策

1. **toggle 录音中 keydown 直接进 ToggleInWait（不走 Pending）**：spec 原文是
   `idle + keydown → Pending`，但录音中 keydown 不应走 Pending（Pending 超时会触发
   instant_start，与 toggle 语义冲突）。改为：FSM Idle 态 + mode==1 → 直接 ToggleInWait，
   mode==3 → 直接 HandsFreeInWait，mode==0 → Pending。这是 spec 的隐含意图
   （toggle/hands-free 录音中的按键序列与 idle 不同），实现时显式化。

2. **Pending keyup ≥ TAP_TIMEOUT 的防御性处理**：理论上 drive_timeouts 会在超时
   瞬间把 Pending → PttRecording，keyup 应落在 PttRecording。但若 manager 线程
   被阻塞（如长 GC）导致 drive_timeouts 未及时执行，keyup 落在 Pending 且超时——
   防御性当作 PTT stop（instant_stop），避免状态卡死。

3. **~~ToggleInWait keyup 超时后的处理~~**（已废弃，见 #6 ToggleShortWait）。

4. **VAD 自动段粘贴推迟**：spec 原设想 hands-free 每段自动粘贴，实现路径复杂
   （paste-and-continue 机制）+ 风险大。改为单段粘贴 + 静音超时兜底。待手感验证后
   再决定是否需要段自动粘贴。

5. **HandsFreeStop 复用 handle_toggle**：不新建 finalize 路径，复用 InstantStop 的
   handle_toggle（instant 标志已置 → finalize/do_paste 走 instant 浮窗路径）。
   hands-free 与 PTT 的停止路径完全一致，仅 RECORDING_MODE 值不同。

6. **toggle 录音中单击/双击区分（2026-07-31 修复，新增 ToggleShortWait 态）**：
   初版实现 toggle 中短按 keyup 立即 `polish_now()` + 回 Idle，导致"双击"被当成两次
   独立单击（两次润色），**不会结束录音**——与 spec 表格"双击→结束 toggle"矛盾。
   修复：新增 `ToggleShortWait` 态，toggle 中短按 keyup **不立即润色**，先进
   ToggleShortWait 等判定——260ms 内再 keydown = 双击 → toggle() 结束；超时无第二次
   = 单击确认 → polish_now()。FSM 从 6 态增至 7 态。回归测试：
   `toggle_double_click_ends_recording` / `toggle_single_click_polishes` /
   `toggle_long_press_ends_recording` / `toggle_short_keyup_does_not_immediately_polish`。

### 验证

- `cargo build -p octopus-desktop --features embedded` ✅ 0 error 0 warning
- `cargo build -p octopus-desktop --features embedded,cloud,vault` ✅
- `cargo test -p octopus-desktop --features embedded` ✅ 480 passed (含 14 个新测试)
- ⏳ e2e 手动验证（待用户在桌面应用实测三模式交互）
