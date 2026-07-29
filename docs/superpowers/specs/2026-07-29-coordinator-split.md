# coordinator.rs 拆分 spec（desktop crate 大文件重构 #1）

> **Status: ✅ 已实现**（2026-07-29，分支 `daily_refactor_coordinator`）

## 背景

`crates/desktop/src/coordinator.rs` 3085 行，是 desktop crate 最大文件。承载录音生命周期协调器（actor 模式）：单线程串行化所有 ASR 事件，状态机驱动录音→识别→润色→粘贴全流程。

run() 入口拆分系列（setup_all / 工具函数搬家 / invoke_handler 提取）已让 main.rs 降至 235 行。coordinator.rs 是下一个最大的复杂度债务。

## 现状结构分析

### Actor 模式（拆分的有利条件）

`Coordinator` struct **只有一个字段** `tx: Mutex<Sender<Command>>`。所有运行时状态是 `build_coordinator_loop` 闭包的局部变量（`stage` / `editing` / `edit_buffer` / `use_streaming` / `pending_prepare` 等），handler 全是**自由函数**（非 method），通过参数接收状态：

```rust
handle_toggle(&mut stage, &audio, &config, &app_handle, &tx, ...)
handle_cancel(&mut stage, &audio, &app_handle)
handle_polish_done(&mut stage, result, session_id, &config, &app_handle, &tx)
```

**关键推论**：拆分不需要改函数签名，只需把函数搬到子模块 + 提升可见性。这是纯代码搬家。

### 职责聚类（12 组 → 8 子模块 + mod.rs）

| 子模块 | 行数 | 内容 | 拆分难度 |
|---|---|---|---|
| `mod.rs`（留） | ~750 | types + `build_coordinator_loop` + tauri commands + struct + 常量 | 基础 |
| `session.rs` | ~260 | `begin_recording` / `prepare_streaming_session` / `prepare_cloud_streaming_session` / `prepare_vad_segmented_session` | 低 |
| `lifecycle.rs` | ~480 | `handle_toggle` / `finalize_after_stop` / `restart_capture_keep_transcript` / `finalize_cloud` / `handle_cloud_streaming_done` | 中 |
| `polish.rs` | ~520 | `spawn_polish_thread` / `handle_polish_done` / `handle_polish_now` / `check_and_trigger_polish` / `handle_final_polish_done` / `start_final_polish_or_paste` / `polish_input_to_regions` | 中 |
| `cancel_discard.rs` | ~220 | `handle_cancel` / `handle_discard` / `DiscardDbInfo` / `agent_task_id_in_stage` | 低 |
| `edit.rs` | ~110 | `handle_enter_edit_mode` / `commit_edit_apply` / `stage_transcript` | 低 |
| `agent.rs` | ~180 | `execute_agent_task` / `parse_agent_context` / `retry_agent_task` / `dispatch_by_record_type` / `AgentContext` | 低 |
| `tick.rs` | ~180 | `dispatch_tick` / `apply_pipeline_events` / `start_*_tick_thread`（3 个）/ `check_audio_stall` / `log_tick_heartbeat` | 低 |
| `paste.rs` | ~110 | `do_paste` / `update_transcription_raw` / `stage_name` / `now_millis` / `active_asr_engine_name` / `active_llm_name` / `sync_runtime_fields` | 低 |

## 目标目录结构

```
crates/desktop/src/coordinator/
├── mod.rs           # ~750 行：types + build_coordinator_loop + tauri commands + struct + 常量
├── session.rs       # 录音会话准备（3 引擎分支）
├── lifecycle.rs     # Toggle/Stop/Finalize 生命周期
├── polish.rs        # 润色相关（立即润色 / 最终润色 / 停顿驱动润色）
├── cancel_discard.rs# Cancel/Discard 出口
├── edit.rs          # 编辑态（enter/commit）
├── agent.rs         # 命令面板 agent 集成
├── tick.rs          # tick 线程 + pipeline 事件路由 + 看门狗
└── paste.rs         # 粘贴 + DB helpers + 通用工具函数
```

`coordinator.rs` 从 3085 → mod.rs ~750 行，其余分散到 8 个子模块（每个 110–520 行）。

## 拆分约束（不变量）

### 1. 共享类型可见性

mod.rs 保留的共享类型/常量，子模块通过 `use super::*` 引入：

| 类型 | 位置 | 可见性 |
|---|---|---|
| `Stage` / `Command` / `RecordType` / `RestartStageKind` | mod.rs | `pub(crate)` |
| `AgentContext` | agent.rs | `pub(crate)`（parse_agent_context 是 pub fn） |
| `DiscardDbInfo` | cancel_discard.rs | `pub(super)` |
| 常量（`AUDIO_STALL_THRESHOLD` / `*_TICK_INTERVAL_MS` / `DB_FLUSH_INTERVAL_MS` / `FALLBACK_STREAMING_SPEC`） | mod.rs | `pub(super)` |
| 全局 static（`CURRENT_TRANSCRIPTION_ID` / `TRANSLATION_ACTIVE`） | mod.rs | 留 mod.rs（set/get 函数也在 mod.rs） |

**判断规则**：mod.rs 内 handler 函数原本 `pub(crate)`（被 build_coordinator_loop 调用）→ 搬到子模块后保持 `pub(crate)`；helper（只被同模块用）保持私有。

### 2. cloud feature gate

37 处 `#[cfg(feature = "cloud")]` 散布在 lifecycle / polish / cancel / session 等模块。搬到子模块时 cfg 守卫**原样跟随**，不调整。关键 cloud-gated 项：
- `Stage::CloudClosing` 变体（mod.rs）
- `prepare_cloud_streaming_session`（session.rs）
- `finalize_cloud` / `handle_cloud_streaming_done`（lifecycle.rs）
- `is_cloud_engine`（mod.rs 或 tick.rs，多处调用）
- `start_cloud_streaming_tick_thread`（tick.rs）

### 3. 测试分布

129 行测试覆盖三块，按被测函数搬到对应子模块：

| 测试 | 被测函数 | 目标模块 |
|---|---|---|
| `translation_active_*`（3 个） | `TRANSLATION_ACTIVE` static | mod.rs |
| `record_type_*`（3 个） | `RecordType` enum | mod.rs |
| `parse_agent_context_*`（8 个） | `parse_agent_context` | agent.rs |
| `check_audio_stall_no_trigger_when_not_recording` | `check_audio_stall` | tick.rs |

### 4. 逻辑完全不变

纯代码搬家——不改函数体、不改签名、不改执行顺序。每个子模块搬完后 `cargo build + cargo test` 验证。

## 风险

| 风险 | 等级 | 应对 |
|---|---|---|
| 可见性遗漏（某个 helper 没提 pub(crate)） | 低 | 编译器报错精确指出，逐个补 |
| cloud gate 跨模块后 cfg 不匹配 | 低 | 每步 build 验证 cloud feature |
| 测试搬错位置（引用了另一模块的私有项） | 低 | 测试跟着被测函数走，引用用 `use super::*` |
| mod 转目录时 git diff 巨大 | 中 | 第一步只做「文件重命名 + mod 声明」，确认编译后再逐个搬函数 |

## 不做

- 不改 actor 模式（不把闭包局部状态提取成 struct 字段）
- 不改 Command enum（不拆分消息类型）
- 不改 Stage 状态机（不变体、不合并）
- 不重构 handler 内部逻辑（只搬家）
- 不处理 coordinator.rs 以外的文件

## 实施记录（review）

### 最终目录结构

```
crates/desktop/src/coordinator/
├── mod.rs           # 860 行（原 3085）：types + build_coordinator_loop + tauri commands + struct + 常量 + 6 个 mod.rs 本地测试
├── paste.rs         # 217 行：do_paste / update_transcription_raw / 通用工具（now_millis / active_*_name / stage_name / sync_runtime_fields）
├── edit.rs          # 141 行：handle_enter_edit_mode / commit_edit_apply / stage_transcript
├── tick.rs          # 241 行：dispatch_tick / apply_pipeline_events / 3 个 tick 线程 / check_audio_stall / log_tick_heartbeat / is_cloud_engine
├── agent.rs         # 235 行：dispatch_by_record_type / execute_agent_task / parse_agent_context / retry_agent_task / AgentContext / agent_task_id_in_stage
├── cancel_discard.rs# 250 行：handle_cancel / handle_discard / DiscardDbInfo
├── session.rs       # 296 行：begin_recording / prepare_streaming_session / prepare_cloud_streaming_session / prepare_vad_segmented_session
├── polish.rs        # 512 行：spawn_polish_thread / polish_input_to_regions / check_and_trigger_polish / handle_polish_done / handle_final_polish_done / handle_polish_now / start_final_polish_or_paste
└── lifecycle.rs     # 522 行：handle_toggle / restart_capture_keep_transcript / finalize_after_stop / finalize_cloud / handle_cloud_streaming_done
```

### 偏差与决策

1. **agent_task_id_in_stage 归属**：spec 原列在 cancel_discard.rs，实际放在 agent.rs（它从 Stage 提取 AgentBridge task_id，语义属 agent 域；cancel_discard.rs 直接 `use super::agent::agent_task_id_in_stage` 引用）。

2. **re-export 渐进精简**：阶段 1 每个 Task 在 mod.rs 加 `pub(crate) use self::<module>::{...}` re-export 让裸调用零改动。阶段 2 Task 2.3 发现主循环不再直接调用一批函数（do_paste / active_asr_engine_name / start_*_tick_thread / dispatch_by_record_type 等），随相关函数搬入子模块后这些 re-export 变成死代码，全部移除 + 清理不再使用的 type imports（PolishMode / StreamingPipeline / StreamingSessionManager / TranscriptEvent / Manager）。

3. **finalize_after_stop 直接路径**：tick.rs 与 polish.rs 用 `use super::lifecycle::finalize_after_stop`（直接模块路径），不经 mod.rs re-export 中转。

4. **通用工具函数放 paste.rs**：spec 原列 `now_millis` / `active_asr_engine_name` / `active_llm_name` / `sync_runtime_fields` / `stage_name` 在 paste.rs——实际确认这些是跨多模块的通用工具，放 paste.rs（第一个搬出的子模块）合理，其他子模块用 `use super::paste::<fn>` 引用。

5. **共享符号可见性**：Stage / Command / RecordType / RestartStageKind / CURRENT_TRANSCRIPTION_ID / TRANSLATION_ACTIVE / 各 tick 常量 / FALLBACK_STREAMING_SPEC / DB_FLUSH_INTERVAL_MS / MIN_POLISH_INTERVAL_SEC 全部提升为 `pub(crate)`。

### 验证结果

- ✅ `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- ✅ `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- ✅ `cargo build -p octopus-desktop --features remote-ws` — 0 error 0 warning
- ✅ `cargo build -p octopus-desktop --features remote-grpc` — 0 error 0 warning
- ✅ `cargo test -p octopus-desktop` — **441 passed, 0 failed, 1 ignored**
