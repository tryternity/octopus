# coordinator.rs 拆分 plan（desktop crate 大文件重构 #1）

> **对应 spec**: `docs/superpowers/specs/2026-07-29-coordinator-split.md`
> **分支**: `daily_refactor_coordinator`
> **原则**: 纯代码搬家，每步可独立验证。按难度从低到高、行数从小到大推进。

## 阶段 0：目录化（零逻辑改动，建立拆分骨架）

### Task 0.1 — coordinator.rs → coordinator/mod.rs

**变更**：
- `git mv crates/desktop/src/coordinator.rs crates/desktop/src/coordinator/mod.rs`
- 确认 `main.rs` 的 `mod coordinator;` 声明无需改（Rust 自动识别目录模块）

**验证**：
```bash
cargo build -p octopus-desktop --features embedded
cargo test -p octopus-desktop
```
**预期**：编译通过，441 测试全绿。git diff 仅为文件移动。

---

## 阶段 1：低难度子模块（行数小、依赖少）

### Task 1.1 — paste.rs（~110 行）

**搬出函数**（coordinator/mod.rs → coordinator/paste.rs）：
- `do_paste`
- `update_transcription_raw`
- `stage_name`
- `now_millis`
- `active_asr_engine_name`
- `active_llm_name`
- `sync_runtime_fields`

**可见性**：`pub(crate)`（被 mod.rs 的 loop 和其他 handler 调用）
**依赖**：`Stage` / `Transcript` / `AppConfig` / `DbCommand` → `use super::*`

**验证**：
```bash
cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop
```

### Task 1.2 — edit.rs（~110 行）

**搬出函数**：
- `handle_enter_edit_mode`
- `commit_edit_apply`
- `stage_transcript`

**可见性**：`pub(crate)`
**依赖**：`Stage` / `Transcript` → `use super::*`

**验证**：同上

### Task 1.3 — tick.rs（~180 行）

**搬出函数**：
- `dispatch_tick`
- `apply_pipeline_events`
- `start_vad_segmented_tick_thread`
- `start_tick_thread`（流式）
- `start_cloud_streaming_tick_thread`（`#[cfg(feature = "cloud")]`）
- `check_audio_stall`
- `log_tick_heartbeat`

**搬出测试**：
- `check_audio_stall_no_trigger_when_not_recording` → `#[cfg(test)] mod tests`

**可见性**：`pub(crate)`
**依赖**：`Stage` / `Command` / `AUDIO_STALL_THRESHOLD` / `VAD_SEGMENTED_TICK_INTERVAL_MS` / `STREAMING_TICK_INTERVAL_MS` / `CLOUD_STREAMING_TICK_INTERVAL_MS` → `use super::*`

**验证**：
```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo test -p octopus-desktop
```

### Task 1.4 — agent.rs（~180 行）

**搬出函数 + 类型**：
- `execute_agent_task`
- `parse_agent_context`（已是 `pub fn`）
- `retry_agent_task`（已是 `pub fn`）
- `dispatch_by_record_type`
- `AgentContext` struct

**搬出测试**：
- `parse_agent_context_*`（8 个）→ `#[cfg(test)] mod tests`

**可见性**：`AgentContext` → `pub(crate)`（被 polish.rs 的 dispatch_by_record_type 路径用）；其余 `pub(crate)`
**依赖**：`Stage` / `RecordType` / `Transcript` → `use super::*`

**验证**：同 Task 1.3

### Task 1.5 — cancel_discard.rs（~220 行）

**搬出函数 + 类型**：
- `handle_cancel`
- `handle_discard`
- `DiscardDbInfo` struct
- `agent_task_id_in_stage`

**可见性**：`pub(crate)`；`DiscardDbInfo` → `pub(super)`
**依赖**：`Stage` / `RecordType` / `Command` → `use super::*`；`agent_task_id_in_stage` 可能被 lifecycle.rs 用 → 如是则 `pub(crate)`

**验证**：同 Task 1.3

---

## 阶段 2：中等难度子模块

### Task 2.1 — session.rs（~260 行）

**搬出函数**：
- `begin_recording`
- `prepare_streaming_session`
- `prepare_cloud_streaming_session`（`#[cfg(feature = "cloud")]`）
- `prepare_vad_segmented_session`

**可见性**：`pub(crate)`
**依赖**：`Stage` / `Command` / `RecordType` / `StreamingSessionManager` / `StreamingPipeline` → `use super::*`

**验证**：
```bash
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo test -p octopus-desktop
```

### Task 2.2 — polish.rs（~520 行）

**搬出函数**：
- `spawn_polish_thread`
- `handle_polish_done`
- `handle_polish_now`
- `check_and_trigger_polish`
- `handle_final_polish_done`
- `start_final_polish_or_paste`
- `polish_input_to_regions`

**可见性**：`pub(crate)`
**依赖**：`Stage` / `Command` / `PolishMode` / `Transcript` → `use super::*`
**注意**：cloud gate（`#[cfg(feature = "cloud")]`）在 `handle_polish_done` 内分支——原样保留

**验证**：同 Task 2.1

### Task 2.3 — lifecycle.rs（~480 行）

**搬出函数**：
- `handle_toggle`
- `finalize_after_stop`
- `restart_capture_keep_transcript`
- `finalize_cloud`（`#[cfg(feature = "cloud")]`）
- `handle_cloud_streaming_done`（`#[cfg(feature = "cloud")]`）

**可见性**：`pub(crate)`
**依赖**：`Stage` / `Command` / `RecordType` / `StreamingPipeline` → `use super::*`；调用 session.rs / polish.rs / paste.rs 函数 → `use super::{session, polish, paste}`
**注意**：这是最大的子模块，cloud gate 最密集（finalize_cloud / handle_cloud_streaming_done 整函数 + handle_toggle 内分支）

**验证**：
```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo test -p octopus-desktop
```

---

## 阶段 3：收尾

### Task 3.1 — 测试整理

**检查**：mod.rs 的 `#[cfg(test)] mod tests` 只剩 `translation_active_*` + `record_type_*`（6 个）。确认无跨模块私有引用。

**验证**：
```bash
cargo test -p octopus-desktop 2>&1 | tail -5
# 预期：441 passed, 0 failed, 1 ignored
```

### Task 3.2 — 文档同步

- 更新 `docs/architecture.md`：coordinator.rs → coordinator/ 目录，补子模块说明
- 更新 spec status → ✅ 已实现
- review plan：把实际偏差回写（函数最终归属、可见性调整）

**验证**：
```bash
rg "coordinator\.rs" docs/ crates/ --type rust  # 确认无遗留引用
```

### Task 3.3 — 全量验证

```bash
# 4 feature 组合
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo build -p octopus-desktop --features remote-ws
cargo build -p octopus-desktop --features remote-grpc
# 测试
cargo test -p octopus-desktop
```

---

## 验证 checklist（每步必跑）

- [x] `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning（涉及 cloud gate 的 task）
- [x] `cargo build -p octopus-desktop --features remote-ws` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features remote-grpc` — 0 error 0 warning
- [x] `cargo test -p octopus-desktop` — 441 passed, 0 failed, 1 ignored
- [x] git diff 确认：只搬函数，无逻辑改动（`git diff --stat` 看增删行数对称）

## 实施完成（2026-07-29）

全部 9 个 Task 完成，每个独立 commit：

| Task | commit | 内容 |
|---|---|---|
| 0.1 | fbf86a9d | coordinator.rs → coordinator/mod.rs 目录化 |
| 1.1 | 07041dc2 | paste.rs（do_paste + 通用工具） |
| 1.2 | d9bad686 | edit.rs（编辑态） |
| 1.3 | 855fb001 | tick.rs（tick 线程 + pipeline 事件路由 + 看门狗） |
| 1.4 | ab34a428 | agent.rs（命令面板 agent 集成） |
| 1.5 | 0e82fbcc | cancel_discard.rs（Cancel/Discard 出口） |
| 2.1 | 566c3b47 | session.rs（3 引擎分支会话准备） |
| 2.2 | 4880cf78 | polish.rs（润色相关） |
| 2.3 | 6eb2bd2d | lifecycle.rs（Toggle/Stop/Finalize 生命周期） |

**最终结果**：coordinator.rs 3085 → mod.rs 860 行 + 8 个子模块（每个 141–522 行）。

## 回滚策略

每个 Task 是独立 commit。如某步 build 失败且难修，`git reset --hard HEAD~1` 回退该 Task，不影响已完成的前序 Task。
