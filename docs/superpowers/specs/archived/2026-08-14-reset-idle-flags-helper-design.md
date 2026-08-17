# reset_idle_flags() helper 设计

> 2026-08-14 · 根治 coordinator static flag 同型遗漏

## 问题

coordinator 有 3 个 static atomics 需在每个 stage→Idle 出口清理：

| Flag | 类型 | 语义 | 漏清后果 |
|---|---|---|---|
| `INSTANT_MODE` | AtomicBool | 控制浮窗 instant vs result_window | 下次快捷键走错浮窗 |
| `TRANSLATION_ACTIVE` | AtomicBool | 控制 do_paste 是否触发翻译 | 下次录音误翻译 |
| `recording_mode` | AtomicU8 | PTT FSM 据此判定下次 keydown 落在 idle | PTT 按键卡死 |

这 3 个 flag 的手动清理分散在 4 个文件 8+ 个出口，每个出口写 2-3 行。**同型遗漏已发生 3 次**：
1. INSTANT_MODE：第三十六轮修 AgentBridge 漏清
2. TRANSLATION_ACTIVE：第四十六轮修 4 处漏 2 处 AgentBridge → 第四十八轮补
3. `reset_mode_flags_on_start_failure` 本身漏了 TRANSLATION_ACTIVE（本轮发现）——开录音失败时 TRANSLATION_ACTIVE 残留 → 下次成功录音误翻译

## 设计

### 新 helper：`reset_idle_flags()`

放在 **mod.rs**（3 个 static 定义所在处），pub(crate) 可见：

```rust
/// 清所有 stage→Idle 出口应复位的 static flag。根治「同型遗漏」——
/// 新增 flag 只需改这一处，所有出口自动覆盖。
pub(crate) fn reset_idle_flags() {
    INSTANT_MODE.store(false, Ordering::Relaxed);
    TRANSLATION_ACTIVE.store(false, Ordering::Relaxed);
    set_recording_mode(0);
}
```

### 替换范围

所有**无条件清理**的 stage→Idle 出口（条件 swap 语义不同，不替换）：

| 文件 | 出口 | 当前代码 | 替换为 |
|---|---|---|---|
| session.rs | `reset_mode_flags_on_start_failure`（被 5 处调用） | 2 行（**漏 TRANSLATION_ACTIVE**） | 删旧 helper，5 处改调 `reset_idle_flags()` |
| lifecycle.rs | finalize_after_stop 空文本 | 3 行 | `reset_idle_flags()` |
| lifecycle.rs | finalize_after_stop AgentBridge | 3 行 | `reset_idle_flags()` |
| lifecycle.rs | finalize_cloud 空文本 | 3 行 | `reset_idle_flags()` |
| lifecycle.rs | finalize_cloud AgentBridge | 3 行 | `reset_idle_flags()` |
| cancel_discard.rs | handle_cancel | 3 行 | `reset_idle_flags()` |
| cancel_discard.rs | handle_discard | 3 行 | `reset_idle_flags()` |
| polish.rs | start_final_polish_or_paste 空文本 | 2 行 | `reset_idle_flags()` |

### 不替换（条件 swap 语义）

- **mod.rs PasteDone handler**：`INSTANT_MODE.swap(false)` 读旧值决定 UI 行为（instant hide vs result_window show）——不是无条件清理
- **paste.rs do_paste**：`TRANSLATION_ACTIVE.swap(false)` 读旧值决定是否翻译——消费语义

### 不涉及（local 变量）

`editing` / `edit_buffer` / `pending_prepare` / `pending_flush` 是 coordinator 循环 scope 内的 local 变量，语义各异（editing 需可选提交，pending_* 有自己的状态机逻辑），不适合收入 static flag helper。

## 好处

1. **根治同型遗漏**：新增 flag 只改 `reset_idle_flags()` 一处
2. **顺手修 bug**：`reset_mode_flags_on_start_failure` 漏 TRANSLATION_ACTIVE → 统一后自动修复
3. **减少代码**：~20 行手动清理 → 8 次 `reset_idle_flags()` 调用
4. **可测试**：单测验证 3 个 flag 都被清零

## 不变量

- 每个调用 `reset_idle_flags()` 的出口，之前都手动清了 INSTANT_MODE + TRANSLATION_ACTIVE + recording_mode（或漏了其中某个）——替换后行为不变（或修复遗漏）
- PasteDone handler 和 do_paste 的 swap 语义不受影响
- local 变量清理不受影响

## 验证

- `cargo build -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error 0 warning
- `cargo test -p octopus-desktop --features "cloud,embedded,vault"` —— 全过
- grep 确认无残留的手动 3-flag 清理序列
