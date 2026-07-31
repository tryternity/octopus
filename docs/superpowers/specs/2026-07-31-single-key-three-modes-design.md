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
| **toggle 录音中** | → 立即润色（录音继续） | → 结束 toggle | → 两次润色（**不识别双击**，= 两次独立短按） |
| **hands-free 录音中** | → 停止（任何操作都停） | → 停止 | → 停止 |
| **PTT 录音中** | —（按着键呢） | keyup → 停止+粘贴 | — |

> **toggle 录音中不识别双击**（2026-07-31 e2e 后定稿）：曾试过 `ToggleShortWait`
> 态等双击判定（260ms 内第二次 keydown = 双击结束），但 260ms 窗口手感不佳——双击
> 难稳定触发。最终改为：toggle 中**只识别单击（润色）和长按（结束）**，不识别双击。
> 短按 keyup 立即 `polish_now()` + 回 Idle（无 260ms 延迟）。双击的实际效果是两次
> 独立单击 = 两次润色（录音继续）。

### 常量

```rust
const TAP_TIMEOUT_MS: u64 = 260;  // 短按/双击判定窗口（后续可开放为配置项）
```

### 按键

默认右 Alt（handy-keys 名称 `OptRight`），可选：OptRight / ShiftRight / Fn / CtrlRight / CmdRight。
通过 `ptt_key` 配置字段控制（DB seed 默认值需从 `AltRight` 改为 `OptRight` 对齐 handy-keys）。

## PTT 状态机（ptt.rs 重构）

6 态有限状态机，替代当前简单的 Pressed→start / Released→stop：

```rust
enum PttFsm {
    Idle,
    Pending { timer_start: Instant },         // keydown 后等判定：长按 or 短按
    ShortPressWait { timer_start: Instant },  // 真 idle 短按松开后等判定：双击 or 确认 hands-free
    PttRecording,                              // PTT 录音中（长按已确认）
    ToggleInWait { timer_start: Instant },     // toggle 录音中 keydown，等判定：长按结束 or 短按润色
    HandsFreeInWait { timer_start: Instant },  // hands-free 中按键，等判定（任何结果都停）
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
  keyup < TAP_TIMEOUT → 短按（单击）→ coordinator.polish_now() → Idle（录音继续）
  t2 超时 → coordinator.toggle()（长按结束 toggle）→ Idle

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
2. **静音超 10 秒自动停**（`HANDS_FREE_SILENCE_TIMEOUT_SECS = 10`）——避免忘关一直录

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
→ 自动发 `Command::HandsFreeStop`。与用户按键等价。**VadSegmented + Streaming 两个分支
都检测**（hands-free 可能用任一引擎）。

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
| `crates/desktop/src/ui/window_position.rs` | 提取多屏 helper（get_mouse_location / find_monitor_at_mouse / find_window_display_id）+ per-display save/load（按 display_id 分键） |
| `crates/desktop/src/ui/result_window.rs` | show_result 加 reposition_to_mouse_monitor（取鼠标所在屏的保存坐标）+ Moved 改 per-display save |
| `crates/desktop/src/ui/instant_overlay.rs` | 删本地 monitor helper 副本，use window_position 公共函数 |
| `docs/architecture.md` | 更新 |

### result_window 多屏跟随（2026-07-31 增补）

toggle 的 result_window 在 `show_result` 时定位到**鼠标所在显示器**：
- **按屏存位置**：key = `window_pos.result@{display_id}`（每屏独立）。Moved 事件经
  `find_window_display_id`（窗口所在 Tauri Monitor 的逻辑 origin → 匹配
  CGDisplay::active_displays 找 display_id）分键保存。
- **show 时取鼠标所在屏坐标**：`find_monitor_at_mouse`（CGEvent 鼠标位置 → CGDisplay
  bounds 命中 → display_id + bounds）→ load 该 display_id 坐标 → set_position。
  该屏没存过 → 用 bounds 算顶部居中 + 存。
- **热插拔不管**：display_id 变 → key 对不上 → fallback 默认顶部居中（符合预期）。
- **仅首次显示 reposition**（e2e 修复）：`reposition_to_mouse_monitor` 只在窗口**从不可见到
  可见**（`is_visible() == false`）时执行。同一会话的后续 show（listening→润色中→最终文本）
  保持位置不动——避免录音期间鼠标移到副屏，结束时窗口跳走。窗口 hide 后下次 show 重新定位。
- **不变量**：clipboard_window 不受影响（仍用单值 save/load_window_position）；
  instant_overlay 不变（底部居中，复用同一套 monitor helper）。
- **坐标系**：CGEvent/CGDisplay bounds = 逻辑坐标（不除 scale）；Tauri Monitor/
  outer_position = 物理像素（÷ scale 转逻辑）。display_id 经 CGDisplay::active_displays 拿。

> 注：本次实现 hands-free = 单段粘贴（停录后一次性粘贴全部）+ 静音 10s 超时兜底。
> finalize_after_stop / do_paste / PasteDone **无需加 hands-free 分支**——复用 instant
> 路径（INSTANT_MODE 已覆盖浮窗 + 跳 result_window）。VAD 自动段粘贴推迟（见上文）。

## 实现状态（2026-07-31 完成）

### 已实现

- ✅ **PttFsm 6 态状态机**（ptt.rs 重写）：Idle / Pending / ShortPressWait /
  PttRecording / ToggleInWait / HandsFreeInWait。常驻 manager 线程局部变量。
  FSM 逻辑抽为纯方法 `next_on_keydown` / `next_on_keyup` / `next_on_timeout`
  （返回 `(PttFsm, FsmAction)`，不依赖 Coordinator），便于单测。
- ✅ **TAP_TIMEOUT_MS = 260**（常量，后续可开放为配置项）。
- ✅ **超时驱动**（`drive_timeouts`）：manager 循环每 ~10ms 检查计时态超时，触发
  Pending→PttRecording（长按）/ ShortPressWait→hands-free（短按确认）/
  ToggleInWait→toggle 结束（长按）/ HandsFreeInWait→hands_free_stop。
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
- ✅ **测试**：11 ptt 测试（PttFsm 初始态 + timed_out 边界 + toggle 单击立即润色/
  长按结束/长按 keyup noop + 真 idle 双击启动 + FsmAction eq）+ 2 RECORDING_MODE 测试
  + 4 window_position 多屏测试（per-display save/load round trip + key 隔离 + 非 macOS None）。

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

3. **toggle 录音中不识别双击（2026-07-31 e2e 后定稿，回退）**：曾两轮迭代——
   ① 初版：toggle 中短按 keyup 立即 `polish_now()` + 回 Idle（双击 = 两次单击 = 两次润色）。
   ② 中间版：新增 `ToggleShortWait` 态等双击判定（短按不立即润色，260ms 内再 keydown
   = 双击结束），但 e2e 实测 260ms 窗口手感不佳——双击难稳定触发，单击润色反而被延迟。
   ③ 最终版（当前）：**回退到不识别双击**——toggle 中短按 keyup 立即润色 + 回 Idle，
   长按（≥260ms）结束录音。移除 `ToggleShortWait` 态，FSM 回到 6 态。双击的实际效果
   是两次润色（录音继续），用户用长按结束。回归测试：
   `toggle_short_click_polishes_immediately` / `toggle_long_press_ends_recording` /
   `toggle_long_press_keyup_after_timeout_noop`。

4. **VAD 自动段粘贴推迟**：spec 原设想 hands-free 每段自动粘贴，实现路径复杂
   （paste-and-continue 机制）+ 风险大。改为单段粘贴 + 静音超时兜底。待手感验证后
   再决定是否需要段自动粘贴。

5. **HandsFreeStop 复用 handle_toggle**：不新建 finalize 路径，复用 InstantStop 的
   handle_toggle（instant 标志已置 → finalize/do_paste 走 instant 浮窗路径）。
   hands-free 与 PTT 的停止路径完全一致，仅 RECORDING_MODE 值不同。

### 验证

- `cargo build -p octopus-desktop --features embedded` ✅ 0 error 0 warning
- `cargo build -p octopus-desktop --features embedded,cloud,vault` ✅
- `cargo test -p octopus-desktop --features embedded` ✅ 488 passed（含 11 ptt + 2 RECORDING_MODE + 4 window_position 多屏测试）
- ⏳ e2e 手动验证（待用户在桌面应用实测三模式交互）
