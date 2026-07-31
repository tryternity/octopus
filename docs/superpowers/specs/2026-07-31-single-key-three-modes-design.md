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
| **toggle 录音中** | → 立即润色 | → 结束 toggle | → 结束 toggle |
| **hands-free 录音中** | → 停止（任何操作都停） | → 停止 | → 停止 |
| **PTT 录音中** | —（按着键呢） | keyup → 停止+粘贴 | — |

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
    ShortPressWait { timer_start: Instant },  // 短按松开后等判定：双击 or 确认 hands-free
    PttRecording,                              // PTT 录音中（长按已确认）
    ToggleInWait { timer_start: Instant },     // toggle 录音中按键，等判定：短按润色 or 结束
    HandsFreeInWait { timer_start: Instant },  // hands-free 中按键，等判定
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
  keyup < TAP_TIMEOUT → 短按 → coordinator.polish_now() → Idle（继续录音）
  t2 超时 → coordinator.toggle()（结束 toggle）→ Idle

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

### VAD 自动切段粘贴（关键差异）

hands-free 模式下，VadSegmentedPipeline 每段识别完**自动粘贴**（不等人停）：
- `finalize_after_stop` 里检测 RECORDING_MODE==3 时，段识别完走 do_paste
- do_paste 的 PasteDone 回调里，hands-free 模式**不回 Idle**——继续录音 + 浮窗回 listening
- 直到用户按键（HandsFreeStop）才真正停录 + finalize 尾段

### 实现要点

现有 `finalize_after_stop` 的段完成回调（VadSegmentedTick drain 完）走 finalize → paste → Idle。
hands-free 模式需要在此路径加分支：paste 后不 Idle，继续 listening。

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
| `crates/desktop/src/engine/coordinator/mod.rs` | 加 HandsFreeStart/Stop + RECORDING_MODE + recording_mode() + Command 分发 |
| `crates/desktop/src/engine/coordinator/session.rs` | begin_recording 加 hands-free 分支 |
| `crates/desktop/src/engine/coordinator/lifecycle.rs` | finalize_after_stop 加 hands-free 段粘贴路径 |
| `crates/desktop/src/engine/coordinator/paste.rs` | do_paste + PasteDone 加 hands-free 分支（粘贴后继续录音） |
| `crates/infra/src/db.sql` | ptt_key seed 从 AltRight 改 OptRight |
| `docs/architecture.md` | 更新 |
