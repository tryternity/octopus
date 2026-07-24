# 音频采集看门狗 + 自动重连实施计划

> **Spec:** `docs/superpowers/specs/2026-07-24-audio-watchdog.md`
> **状态**：✅ 已实现

---

## Task 概览

| # | Task | 状态 |
|---|---|---|
| 1 | 写 spec | ✅ |
| 2 | audio.rs：last_sample_time + sample_stall_duration + start 时序修正 | ✅ |
| 3 | transcript.rs：reset_engine_baseline + 回归测试 | ✅ |
| 4 | coordinator.rs：Command::RestartCapture + restart_capture_keep_transcript | ✅ |
| 5 | coordinator.rs：dispatch_tick 看门狗检测 + check_audio_stall | ✅ |
| 6 | 回归测试（sample_stall_duration 4 场景 + check_audio_stall + reset_engine_baseline 2 场景） | ✅ |
| 7 | architecture.md 同步 + plan 回写 | ✅ |

---

## 详细 Task（实施记录）

### Task 1：spec
- [x] `docs/superpowers/specs/2026-07-24-audio-watchdog.md`：根因链、3s 阈值依据、自动重连设计、二次失败降级、transcript 基准对齐、误判边界

### Task 2：audio.rs
- [x] `SharedAudioState` 加 `last_sample_time: Arc<Mutex<Option<Instant>>>`
- [x] cpal 三回调臂（F32/I16/U16）extend 后更新 `last_sample_time`
- [x] 新增 `pub fn sample_stall_duration(&self) -> Duration`：is_recording=false → 0；None → 0（冷启动保护）；Some(t) → elapsed
- [x] `start` 时序修正：`is_recording.store(true)` 从开头移到 `build_stream`+`play` 之后
- [x] `start` 开头清 `last_sample_time = None`

### Task 3：transcript.rs
- [x] `pub fn reset_engine_baseline(&mut self)`：清 engine_cumulative/consumed/diverted_pending，保留 segments/caret_gap/id/db_inserted
- [x] 测试 `reset_engine_baseline_clears_cumulative_keeps_segments`
- [x] 测试 `reset_engine_baseline_then_apply_starts_fresh`（重连后首词正常 apply，不走 diverted）

### Task 4：coordinator.rs 重连
- [x] `Command::RestartCapture { stage_kind: RestartStageKind }` + `RestartStageKind` enum（Streaming/VadSegmented；无 WaitingCompletion——该 stage is_recording=false 天然不触发）
- [x] `restart_capture_keep_transcript`：停止阶段（停 tick + drain + stop + 喂尾 + finish + 取出 transcript 保留）+ 重连阶段（reset_engine_baseline + audio.start + 引擎 reset + 新建 pipeline + transcript 放回 Stage + 重启 tick + update_result + emit mic-reconnecting）
- [x] cloud 引擎 `is_cloud()` no-op + warn（独立 WS 连接语义不同）
- [x] 二次失败降级：`audio.start` 失败 → emit mic-error + finalize_after_stop 粘贴已识别文本
- [x] Command 分发：stage_kind 校验（跨命令竞态防护，不匹配则 warn + skip）

### Task 5：coordinator.rs 看门狗
- [x] `AUDIO_STALL_THRESHOLD = Duration::from_secs(3)` 常量
- [x] `check_audio_stall(audio, stage) -> bool` 纯函数（抽离便于单测）
- [x] `dispatch_tick` Streaming/VadSegmented 分支 tick 后调 `check_audio_stall`，命中则 `tx.send(Command::RestartCapture)`
- [x] WaitingCompletion 不检测（is_recording=false 天然免疫）
- [x] `[WATCHDOG]` 诊断日志

### Task 6：回归测试
- [x] `audio::tests::sample_stall_duration_zero_when_not_recording`
- [x] `audio::tests::sample_stall_duration_zero_when_just_started_no_callback_yet`（冷启动保护）
- [x] `audio::tests::sample_stall_duration_positive_when_recording_and_stalled`（断推 5s）
- [x] `audio::tests::sample_stall_duration_small_when_recent_callback`（正常采集）
- [x] `coordinator::tests::check_audio_stall_no_trigger_when_not_recording`（跨模块私有字段限制，其余场景由 audio.rs sample_stall_duration 覆盖）

### Task 7：文档
- [x] architecture.md §音频采集章节加「看门狗 + 自动重连」子节
- [x] plan 回写（本文件）

---

## 实际偏差与决策记录

1. **`RestartStageKind` 删掉 WaitingCompletion 变体**：原计划含此变体，但实施时发现 WaitingCompletion 的 `is_recording=false`（stop 时翻转）→ `sample_stall_duration` 返回 0 → 看门狗天然不触发，此变体永远无构造点，删掉避免 dead_code warning。
2. **看门狗测试分两处**：`sample_stall_duration` 的 4 场景在 `audio.rs` 测试（可访问私有字段 `last_sample_time`/`is_recording`）；`check_audio_stall` 在 `coordinator.rs` 测试但只能测不录音场景（跨模块无法设 audio 私有字段）。核心逻辑由 audio.rs 测试充分覆盖。
3. **`restart_capture_keep_transcript` 的 WaitingCompletion 分支保留为防御**：虽 stage_kind 校验不会让此分支被触发，但作为防御性代码保留（万一 stage 在发命令到处理间切换）。

---

## 验证

- [x] `cargo build -p octopus-desktop --features embedded` 0 error 0 warning
- [x] `cargo test -p octopus-desktop` 410 passed 0 failed（含 7 个新测试：4 audio + 1 coordinator + 2 transcript）
- [ ] e2e 待用户验证：录音中拔麦克风 → 3s 后自动重连 + 插回继续可说
