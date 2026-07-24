# 音频采集看门狗 + 自动重连设计

> **日期**：2026-07-24
> **状态**：✅ 已实现（e2e 待用户验证）
> **关联**：Bug2 VAD 卡死；[`docs/superpowers/specs/2026-07-19-asr-edit-stall-observability.md`](2026-07-19-asr-edit-stall-observability.md)（诊断日志体系）

---

## 1. 问题

cpal 音频流断推（设备拔出 / 系统挂起 / 驱动错误）后，pipeline 永久空转到用户手动停。

**日志特征**（用户实测，2026-07-24 23:15:16-41，持续 25 秒）：
```
[TICK-DETAIL] pipeline-vad-seg silence=0.02 has_speech=true speaking=true samples=0 buffer_s=0.2
```
连续 25 个 tick（1Hz 打点）samples=0、状态冻结、active_count=0、buffer 不增长。用户说了话但完全没被录到，且无任何提示。

## 2. 根因链

1. **cpal 断推**：设备层错误（拔出 / 挂起 / 驱动），回调不再触发。
2. **samples 持续空**：回调不触发 → `audio.rs` 回调闭包不执行 → `samples` 缓冲不增长 → `drain_samples()`（`audio.rs:304`）每次返回空 Vec。
3. **pipeline 早退**：`StreamingPipeline::tick`（`pipeline.rs:219`）`samples.is_empty()` 直接 return；`VadSegmentedPipeline::run_tick`（`pipeline.rs:491`）`if !samples.is_empty()` 整块跳过切段/VAD 更新。
4. **状态冻结**：`silence_duration` / `has_speech` 只在 samples 非空时更新（`pipeline.rs:494-501`）→ 冻结在断流前最后值。
5. **force_cut 永不触发**：`force_cut` 判定 `buffer_duration_s >= SEGMENT_DURATION_S`（`pipeline.rs:508`），但 buffer 不增长（samples 空）→ 永不达标。
6. **无看门狗**：全代码库无"连续 N tick 无样本 → 报错/停止/重连"机制。
7. **错误回调形同虚设**：`audio.rs` 三个 `build_input_stream` 臂的错误回调（行 178/193/210）只 `log::error!`，不翻转 `is_recording`、不通知上层。

**结果**：录音状态停留在 Streaming/VadSegmented，tick 线程空转，结果窗卡在最后状态，用户以为还在录但其实没在录，直到手动 Toggle/Discard/Cancel。

## 3. 设计决策

### 3.1 看门狗阈值：3 秒

| 候选 | 取舍 |
|---|---|
| **3s（选定）** | VadSegmentedTick 100ms × 30 tick / StreamingTick 200ms × 15 tick。远早于用户感知卡死（实测 30s），又不误伤瞬时调度卡顿（<3s） |
| 5s | 更保守，但多 2s 无价值等待 |
| 10s | 过保守，接近用户感知阈值 |

**关键判据**：正常静音时 cpal 回调**仍推送底噪样本**（samples≠0，仅 VAD 判为非语音）。`samples=0` 意味着回调**根本没触发** = 流真的断了，不是静音。因此 3s 阈值不会误判正常静音为断流。

### 3.2 触发后动作：自动重连（而非只提示 / 主动停止）

经与用户讨论确认三个候选：

| 候选 | 体验 | 问题 |
|---|---|---|
| 只提示 mic-error | 用户看到提示自己决定 | 录音继续空转；用户可能没注意提示 |
| 主动 Toggle 停止 + 粘贴 | 走 finalize→粘贴，与手动停一致 | 用户没主动操作却看到粘贴发生，更困惑 |
| **自动重连（选定）** | 中断+重启录音，复用 transcript，两次录音文本拼一起 | 重连可能失败（→ 二次降级） |

**用户原话**：「尝试恢复识别，相当于先中断录音，再重启录音。语音识别框一直在，只是把两次的录音放在一起而已。」

自动重连对用户最友好：识别框不消失，短暂"重连中"提示后继续可说，已识别文本保留。

### 3.3 二次失败降级

重连也可能失败（设备真坏了 / 驱动崩溃）。避免无限重连循环：

- `restart_capture` 内若 `audio.start` 失败 → `emit("mic-error", "麦克风采集中断，自动重连失败，请检查设备后重试")` + 走正常 `finalize_after_stop`（用保留的 transcript，粘贴已识别文本，让用户至少拿到成果）
- **不再自动重连**。用户看到 mic-error 后需手动重开录音。

### 3.4 不自动重连 cloud

cloud 引擎走独立 WebSocket 连接（`cloud_pipeline.rs`），断流语义是网络问题，与本地 cpal 采集断流不同。本次只修本地 cpal。cloud 的 `Stage::Streaming`（cloud 复用此 stage）若触发看门狗，`restart_capture` 在识别为 cloud 引擎时 no-op + warn（cloud 有自己的连接重试/错误处理）。

### 3.5 transcript 引擎基准对齐（关键技术点）

重连 = 重建 pipeline（engine 状态清零）。但复用的 transcript 保留了旧 `engine_cumulative`（引擎层累积全量，`transcript.rs:36`）。两者基准不一致会导致：

> 重建 engine 状态空 + transcript 旧 engine_cumulative 很长 → 首个 `apply_engine_full` 的 `is_prefix` 判定失败（空 engine 输出不是旧长 cum 的前缀）→ 走 diverted 分支 → `diverted_pending` 异常累积。

**解法**：新增 `Transcript::reset_engine_baseline()`，重连时清 `engine_cumulative` / `engine_consumed_chars` / `diverted_pending`，保留 `segments` / `caret_gap` / `id` / `db_inserted`（用户已识别文本 + 落库状态）。副作用：丢失引擎层纠正能力（可接受——断流本就是异常，重连后从空基准重新累积）。

## 4. 实现组件

### 4.1 组件 1：音频采集看门狗

**`audio.rs`**：
- `SharedAudioState` 新增 `last_sample_time: Mutex<Option<Instant>>`
- cpal 三回调臂（F32/I16/U16）`extend` 后更新 `last_sample_time = Some(Instant::now())`
- 新增 `pub fn sample_stall_duration(&self) -> Duration`：`is_recording=true` 且 `last_sample_time` 距今 > N → 返回差值；否则 `Duration::ZERO`
- `start` 开头清 `last_sample_time = None`（避免上次会话残留）
- **`start` 时序修正**：`is_recording.store(true)` 从行 223 移到 `build_stream`+`play` 之后，避免建流失败标志残留 true

**`coordinator.rs::dispatch_tick`（行 2661）**：
- 三活跃 stage 分支 tick 后，检查 `audio.sample_stall_duration() >= STALL_THRESHOLD`
- `STALL_THRESHOLD = Duration::from_secs(3)`
- 触发 → `tx.send(Command::RestartCapture { stage_kind })` + 停当前 tick 线程
- 诊断 `[WATCHDOG] stall={:.1}s threshold=3.0 → restart` 日志

### 4.2 组件 2：自动重连

新增 `Command::RestartCapture { stage_kind: StageKind }`（StageKind = Streaming | VadSegmented | WaitingCompletion）。

新增 `restart_capture_keep_transcript`（`coordinator.rs`，`handle_toggle` 之后）：

**停止阶段**（参考 `handle_toggle` 停止分支，**不走 finalize**）：
1. 停 tick 线程
2. `audio.drain_samples()` + `audio.stop()` 取尾部
3. 喂尾给旧 pipeline + `pipeline.finish()` flush 在途 partial 进 transcript
4. `mem::replace(stage, Idle)` 取出 pipeline + transcript，**保留 transcript**

**重连阶段**（参考 `prepare_*_session`，**复用 transcript**）：
5. `transcript.reset_engine_baseline()`
6. `audio.start()` 重连（失败 → 二次降级）
7. 引擎 Arc 取用 + reset（复用常驻引擎，不重载模型）
8. 新建 pipeline（重建，清断流污染的 VAD/buffer）
9. transcript 放回 Stage
10. 重启 tick 线程
11. `result_window::update_result` 刷新显示（窗口一直可见）
12. emit `mic-reconnecting`（前端可选 toast）

### 4.3 组件 3：Transcript 引擎基准重置

`transcript.rs` 新增 `reset_engine_baseline()`（见 §3.5）。

### 4.4 组件 4：回归测试

- `transcript.rs`：`reset_engine_baseline_clears_cumulative_keeps_segments`
- `coordinator.rs`/`pipeline.rs`：看门狗触发/不触发测试（纯函数抽出可测）

## 5. 误判边界与不变量

- **正常静音不误判**：samples≠0（底噪），`sample_stall_duration` 始终 <3s
- **WaitingCompletion 也守**：stop 后在途段识别中，此时 samples 本就会空（已 stop），但 WaitingCompletion 不应触发看门狗——靠 `is_recording` 已 false（stop 时翻转）天然免疫
- **重连后状态一致**：transcript 的 segments/caret_gap/id/db_inserted 保留，engine 基准清零，pipeline 全新，audio 重建
- **结果窗不隐藏**：全程不调 `hide_result`/`clear_result`

## 6. 验证

- `cargo build -p octopus-desktop --features embedded` 0 error 0 warning
- `cargo test -p octopus-desktop` 全过（含新回归测试）
- 人工 e2e：录音中拔麦克风 → 3s 后自动重连提示 + 插回后继续可说
