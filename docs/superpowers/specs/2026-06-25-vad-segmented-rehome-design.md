# 2c-3 VadSegmented 归位（统一 pipeline 角色）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已实施并 ff-merge main。Task 1-6（commit `cde1100`）双 feature 编译 0 error、新代码 clippy 0 新 warning；workspace 测试除 2 个 pre-existing infra 失败（`seed_then_load_round_trips` / `list_all_local_asr_models_includes_disabled`，seed `c796cbc` 重写后断言过时，与本次无关——2c-3 未触碰 `crates/infra/`）外全绿；VadSegmented 全路径 e2e 验证通过（2026-06-25）。
> **动机**：ASR pipeline 重构阶段2（spec `2026-06-23-asr-pipeline-design.md`）已收编流式（2a/2b/2c-1：`StreamingPipeline` 壳）+ cloud（2c-2：`StreamingPipelineEngine` trait + `CloudPipelineEngine`）。VadSegmented（非流式引擎的 VAD 分段伪流式）是阶段2 最后一块未归位的编排，散在 `coordinator.rs`（`handle_vad_segmented_tick` + `Stage::VadSegmented`/`WaitingCompletion` 两处 `TranscriptionDone` 乱序回填 handler）。本 spec 把它收进统一 `Pipeline` 角色，为 2d（coordinator 清理）铺路。
> **关联**：总 spec `2026-06-23-asr-pipeline-design.md`（§3.4 / §9 迁移映射）；2c-1 spec `2026-06-23-...`（`StreamingPipeline` 壳）；2c-2 spec `2026-06-24-asr-pipeline-stage2c2-design.md`（`StreamingPipelineEngine` trait）。
> **范围**：新增 `Pipeline` 上层 trait + `VadSegmentedPipeline`（封装分段编排 + 乱序回填）+ 删 `TranscriptionDone` 命令 + `Stage::VadSegmented`/`WaitingCompletion` 字段改持 pipeline。**不含**：emit/DB/polish/transcript 全收敛（2d）、cli/server。

---

## 1. 背景

### 1.1 VadSegmented 现状（非流式引擎的伪流式）

非流式 ASR 引擎（moonshine / zipformer-non-streaming 等，`is_streaming_engine()==false`）无法逐帧增量出文本，desktop 用 **VAD 分段伪流式**：tick 线程每 ~300ms 驱动 `handle_vad_segmented_tick`（`coordinator.rs:1293`）——

1. `drain_samples()` 累积到 `audio_buffer`
2. **检测 VAD**（流式、有状态，跨 tick 续接 LSTM）`compute_speech_chunks` 统计语音帧 → 更新 `silence_duration`/`has_speech`
3. 静音边界切分（`silence_cut`）/ 连续超时强制切断（`force_cut`，带 overlap）
4. **过滤 VAD**（每段独立 reset）`filter_speech_from_buffer` 取纯净语音样本
5. `spawn_offline_transcription_with_seq`——`tauri::async_runtime::spawn` 跑 `engine.transcribe`，结果以 `Command::TranscriptionDone{seq, session_id}` 回传 coordinator.tx
6. coordinator 在 `Stage::VadSegmented` / `Stage::WaitingCompletion` 两处 handler 收 `TranscriptionDone`，**按 seq 乱序回填**（`completed_results: HashMap<u64,String>` + `consume_completed_results` 消费连续 seq 追加 transcript）

### 1.2 与流式的三个本质语义差异

| | 流式（StreamingPipeline） | VadSegmented |
|---|---|---|
| tick 返回 | 同步：`tick()->Vec<TranscriptEvent>` 立即出文本 | 异步：tick 切段 → spawn → **结果下一轮才回传** |
| 段完成顺序 | 无序概念（单条流） | seq 切分顺序 ≠ 完成顺序，需乱序回填 |
| VAD | 单 VAD 实例（StreamingRunner accept/flush 静音检测） | 双 VAD（检测流式有状态 + 过滤每段 reset，分离防 LSTM 污染） |

因此 VadSegmented **不能直接塞进 `StreamingPipelineEngine::tick`（同步返回事件）**——它的输出是异步命令回传。

### 1.3 目标（用户决策：统一 pipeline 角色 · 中）

引入上层 `Pipeline` trait，VadSegmented 对外暴露**同步 `tick()`**（内部 spawn + 结果发回 pipeline 自持 channel，下个 tick drain——异步转同步）。coordinator 持 `Box<dyn Pipeline>` 不再按 stage 分流 tick 逻辑。emit/DB/polish/transcript 仍留 coordinator（2d 收）。每步零行为差异 + 可 e2e，对齐 2a/2b/2c-1/2c-2 节奏。

## 2. 用户决策（brainstorming 2026-06-25）

1. **归位深度**：统一 pipeline 角色（中）——coordinator 持 `Box<dyn Pipeline>` + tick/finish/silence/reset 统一接口；emit/DB/polish/transcript 留 coordinator（2d 收）。
2. **异步转同步**：VadSegmentedPipeline 内部持 mpsc channel，spawn 结果发回 pipeline.rx（不发 coordinator.tx），tick 内 `try_recv` drain + 乱序回填 + 消费连续 seq + set_full。
3. **上层 trait**：新增 `Pipeline` trait（不扩展既有 `StreamingPipelineEngine`）。StreamingPipeline 外层加 `impl Pipeline`（内层 StreamingPipelineEngine 两层不动）；VadSegmentedPipeline 直接 `impl Pipeline`（无内层 engine，语义不同不硬套）。
4. **2d 边界**：2c-3 只统一 pipeline 角色，2d（emit/DB/polish + transcript 全收敛）独立 spec。
5. **收尾边界**：决策 A——`Stage::WaitingCompletion` 保留为独立 stage，字段改持 pipeline，收尾复用 `VadSegmentedTick` 驱动 drain（tick 空样本仍 drain rx，channel 不积压）；`TranscriptionDone` 命令删除。
6. **finish 签名**：决策 B——`Pipeline::finish(&mut self, transcript)` 无参；coordinator stop 路径 `tick(drain_tail, transcript)` + `finish(transcript)`，语义等价于现状 `finish_with_tail(tail)`（tail 经 tick 喂入被 accept，finish 只 flush）。
7. **WaitingCompletion stage**：决策 A——保留独立 stage（字段改 `{pipeline, transcript}`），不合并进 VadSegmented（合并需改 ~10 处 match arm，与零行为差异节奏不符）。

## 3. 设计

### 3.1 `Pipeline` 上层 trait（pipeline.rs 新增）

```rust
/// desktop ASR pipeline 统一上层抽象（2c-3）。
/// StreamingPipeline（流式，内持 StreamingPipelineEngine）与 VadSegmentedPipeline（VAD 分段伪流式）
/// 各 impl。coordinator 持 Box<dyn Pipeline>，tick/finish/silence 统一调用，不再按 stage 分流。
pub trait Pipeline: Send {
    /// 喂一帧已降噪 16k 样本（VadSegmented：累积+切段+spawn+drain_rx；流式：engine tick→set_full）。
    /// 返回 changed（coordinator 据 DB + emit，保持「内容未变不落库/不重绘」幂等）。
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool;
    /// 收尾：流式 flush（tail 已由 stop 路径的 tick 喂入）；vad-seg 仅 drain 剩余 rx。
    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent;
    /// 当前累积静音时长（秒，停顿润色触发用）。
    fn silence_duration(&self) -> f64;
    /// 重置（会话间复用）。
    fn reset(&mut self);
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> { None }
    fn is_cloud(&self) -> bool { false }
}
```

既有 `StreamingPipelineEngine`（2c-1/2c-2）**不动**，降为 `StreamingPipeline` 的内层（local/cloud 各 impl），外层加 `impl Pipeline for StreamingPipeline`。

### 3.2 `VadSegmentedPipeline`（pipeline.rs 新增）

封装 `handle_vad_segmented_tick` 编排 + `TranscriptionDone` 乱序回填。

**字段**（全部来自现 `Stage::VadSegmented` + 新增 channel 对）：

| 字段 | 说明 |
|---|---|
| `engine: Arc<dyn TranscriptionEngine>` | spawn 用 |
| `language: String` / `asr_engine: String` | spawn 参数（config 子集 clone，避免持整 AppConfig） |
| `segment_silence_ms: u64` / `pause_polish_threshold_ms: u64` | 切分/润色阈值（config 子集；`SEGMENT_DURATION_S`/`SEGMENT_OVERLAP_MS` 是 coordinator.rs 既有常量，随逻辑搬迁进 pipeline.rs） |
| `detect_vad: SileroVad` | 检测 VAD（流式有状态，跨 tick 续接，录音期间从不 reset） |
| `filter_vad: SileroVad` | 过滤 VAD（每段 reset，与检测分离防 LSTM 污染） |
| `audio_buffer: Vec<f32>` / `overlap_tail: Vec<f32>` | 累积缓冲 / 强制切断重叠（overlap 由 `SEGMENT_OVERLAP_MS` 常量算） |
| `silence_duration: f64` / `has_speech: bool` | 切分判定状态 |
| `active_count: u32` / `next_seq: u64` / `completed_seq: u64` | spawn 计数 / 序号 |
| `completed_results: HashMap<u64, String>` | 乱序回填缓存 |
| `tx: Sender<SegmentResult>` | 传给 spawn（pipeline 自持 clone） |
| `rx: Receiver<SegmentResult>` | spawn 回传（替代 coordinator.tx） |

`SegmentResult { seq: u64, session_id: i64, text: Result<String, String> }`——pipeline 内部类型（`session_id` 保留用于日志，跨会话护栏由 pipeline 随 stage drop 天然保证，见 §4）。

**tick 编排**（搬迁 `handle_vad_segmented_tick` L1314-1386，零逻辑改动，仅 spawn 目标改 tx）：

```
tick(samples, transcript) -> changed:
  1. audio_buffer.extend(samples)（samples 空则跳过 1-5，仍走 6-8 drain）
  2. compute_speech_chunks(detect_vad, samples) → 更新 silence_duration/has_speech
  3. silence_cut | force_cut 判定
  4. [应切] send_buffer = overlap_tail + audio_buffer → filter_speech_from_buffer(filter_vad) → speech_samples
  5. [有语音] next_seq++ → spawn_offline(engine, tx, speech_samples, seq, session_id=transcript.id) → active_count++
  6. drain rx（try_recv 循环至空）→ completed_results 按 seq 回填（空串/失败占位保 completed_seq 连续）
  7. consume_completed_results(completed_seq, completed_results, transcript) → set_full
  8. 返回 changed
```

**finish**：drain rx 至空（带短超时或循环 N 次，排空在途段）+ consume。无 tail（tail 已由 coordinator stop 路径的 tick 喂入，可能触发最后一轮切段）。

### 3.3 `impl Pipeline for StreamingPipeline`（外层套壳）

```rust
impl Pipeline for StreamingPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        self.tick_inner(samples, transcript)  // 既有 tick 逻辑，改名/复用
    }
    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent {
        // 既有 finish_with_tail 的 flush 部分（tail 已由 stop 路径 tick 喂入 accept）
        self.engine.finish()  // StreamingRunner::finish（去 tail 参数）
    }
    fn silence_duration(&self) -> f64 { self.engine.silence_duration() }
    fn reset(&mut self) { self.engine.reset() }
    #[cfg(feature="cloud")]
    fn take_close_handle(&mut self) -> Option<CloudStreamHandle> { self.engine.take_close_handle() }
    fn is_cloud(&self) -> bool { self.engine.is_cloud() }
}
```

StreamingPipelineEngine trait 的 `finish_with_tail(&[f32])` 改 `finish()`（去 tail 参数，**Task 4 改**）——local impl 内部 `StreamingRunner::finish`（现状 `finish_with_tail` = accept tail + flush，拆成 stop 路径 tick(tail) accept + finish flush，语义等价）；cloud impl 同理（push tail 已由 tick 完成，finish 仅兜底）。**既有流式测试 + e2e 验证等价**。

> **注意**：cloud 的 stop 路径（`Stage::CloudClosing` + `take_close_handle` + spawn `close_async`）**不走** `Pipeline::finish`——cloud close 是 coordinator 直接取出 handle spawn，不调 finish。`Pipeline::finish` 只服务 local 流式（flush）+ vad-seg（drain rx）。§3.4 表格「stop 路径」行仅指 local 流式 + vad-seg。

### 3.4 coordinator 改造

| 现状 | 2c-3 后 |
|---|---|
| `Stage::VadSegmented { 11 字段 + transcript + tick_active }` | `Stage::VadSegmented { pipeline: VadSegmentedPipeline, transcript: Transcript, tick_active: Arc<AtomicBool> }` |
| `Stage::WaitingCompletion { transcript, active_count, completed_seq, completed_results }` | `Stage::WaitingCompletion { pipeline: VadSegmentedPipeline, transcript: Transcript }`（pipeline 整体从 VadSegmented move 过来） |
| `handle_vad_segmented_tick`（L1293，~95 行编排） | 删，逻辑进 `VadSegmentedPipeline::tick`；coordinator tick handler 改 `if let VadSegmented{pipeline, transcript, ..} = stage { let changed = pipeline.tick(&audio.drain_samples(), transcript); if changed { DB + emit + check_polish } }` |
| `Command::TranscriptionDone` + 两处回填 handler（L1860-1949，~90 行） | **删**；VadSegmented tick 内部 drain rx 回填；WaitingCompletion 收尾复用 VadSegmentedTick 驱动（tick 空样本仍 drain rx） |
| stop 路径 `pipeline.finish_with_tail(tail)` | `pipeline.tick(tail, transcript)` + `pipeline.finish(transcript)`（流式/vad-seg 统一） |

**emit/DB/polish 留 coordinator**（tick 返回 changed 后，coordinator 据 changed 做 DB + emit + check_and_trigger_polish——三路径 local/VadSegmented/cloud 共用，2d 收敛）。

### 3.5 WaitingCompletion 收尾驱动（决策 A）

Toggle 停止（VadSegmented，active_count>0）→ 进 `Stage::WaitingCompletion { pipeline, transcript }`。tick 线程继续发 `VadSegmentedTick`（`tick_active` 未停），handler：

```
if let WaitingCompletion{pipeline, transcript} = stage {
    let changed = pipeline.tick(&[], transcript);  // 空样本：跳过切段/spawn，仅 drain rx + consume
    if changed { DB + emit }
    if pipeline.active_count() == 0 {  // pipeline 暴露 active_count getter
        let tr = mem::replace(transcript, Transcript::new(0, Disabled));
        finalize_after_stop(stage, tr, ...);
    }
}
```

channel 不积压（每 tick drain），`TranscriptionDone` 命令删除。VadSegmented 与 WaitingCompletion 共用 tick 驱动 + 同一 pipeline 实例（move）。

## 4. 跨会话护栏（比现状更干净）

现状：`TranscriptionDone{session_id}` handler 在 VadSegmented/WaitingCompletion 两处判 `transcript.id != session_id` 忽略旧会话结果（审查 一1：润色/识别线程不持 transcript，回来时可能已是新会话）。

2c-3 后：spawn 带 `session_id`（日志用），但 **pipeline drain rx 不判 session_id**——pipeline 只服务当前 transcript，跨会话由「stage 切换 = 新 pipeline 实例」天然保证：

- Toggle/Cancel 切新会话 → 旧 stage（含旧 pipeline）drop → 旧 pipeline.rx disconnect
- 旧 spawn 线程的 `tx.send` 失败 → 忽略（线程自然结束，无泄漏）

**比现状省去 session_id 比对**，逻辑更简单。session_id 仍保留在 SegmentResult + 日志（可追溯）。

## 5. 不在范围

- **emit/DB/polish/transcript 全收敛**（2d）：独立 spec。2c-3 后这些仍留 coordinator（tick 返回 changed → coordinator 做 DB/emit/polish）。
- **cli/server**：本次只改 desktop。cli 批处理早走 `asr::transcribe_batch`（阶段1），无 VadSegmented。
- **双 VAD / 切段 / overlap 逻辑**：零改动搬迁（仅换容器：stage 字段 → pipeline 字段）。
- **VadSegmentedPipeline 走 cloud**：不会。VadSegmented 仅非流式本地引擎；云端经 DispatchEngine 路由 AliyunEngine（chunk 批处理），非流式 WS。`is_cloud()` 恒 false，无 cloud 门控。
- **StreamingPipelineEngine trait 重构**：不动（仅 `finish_with_tail`→`finish` 去 tail）。

## 6. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 删 `TranscriptionDone` 命令影响面 | grep 全仓确认仅 VadSegmented/WaitingCompletion 用（engine_aliyun chunk 模式同步 transcribe 不发命令）；收尾改 tick 驱动 |
| finish 去 tail 改变流式 accept 语义 | §3.3 论证 tick(tail)+finish 等价；Task 4 既有流式测试 + e2e 验证 |
| VadSegmentedPipeline 持 config 子集 | 不 clone 整 AppConfig，仅取 language/asr_engine/segment_silence/pause_polish_threshold（4 字段） |
| WaitingCompletion 跨会话护栏 | §4：pipeline 随 stage drop，rx disconnect，spawn send 失败忽略 |
| channel 在途段丢失 | finish drain 至空 + active_count 归零才 finalize；unbounded channel 不丢 |
| spawn 包装未单测 | spawn 是薄包装（现状已验证），单测聚焦回填/consume/set_full 纯逻辑（手动喂 rx），spawn 靠 e2e |

## 7. 测试策略

| 测试 | 验证 |
|---|---|
| VadSegmentedPipeline tick 切段 | 喂语音样本→has_speech；静音达阈值→切段标记；用 fake engine（transcribe 固定文本），手动喂 rx 验证 spawn→回填→consume→set_full |
| 乱序回填 | 构造 seq=1,3,2 回传顺序→consume 仅连续时追加（1→追加，3→缓存，2 到达→追加 2+3） |
| 空串/失败占位 | Err/空→占位空串→completed_seq 不卡 |
| finish drain | active_count>0→finish→drain rx 至空→consume |
| 双 VAD 隔离 | 检测 VAD 跨 tick 累积 vs 过滤 VAD 每段 reset（搬迁现有逻辑，验证行为保留） |
| StreamingPipeline impl Pipeline | 复用 2c-1 既有 FakePipelineEngine 测试，验证外层套壳零差异 |
| 端到端 | cargo check --workspace --all-targets + clippy + 手动 e2e（非流式引擎 VadSegmented 全路径） |

**单测不依赖 tauri/tokio runtime**：手动构造 `SegmentResult` 喂 pipeline.rx，验证 drain+回填+consume+set_full 纯逻辑；spawn 包装靠 e2e。

## 8. 迁移任务（对齐 2a/2b/2c-1/2c-2 节奏，零行为差异 + 可 e2e）

| Task | 内容 | 验证 |
|---|---|---|
| 1 | pipeline.rs 加 `Pipeline` trait + `SegmentResult` 类型 | 编译（trait 未用） |
| 2 | `VadSegmentedPipeline` 结构 + tick 编排搬迁（drain→双 VAD→切段→过滤→spawn 发 rx）+ drain_rx 回填 + consume + set_full | 单测（喂 rx 测回填/乱序/占位） |
| 3 | `impl Pipeline for VadSegmentedPipeline`（finish=drain rx；silence_duration/reset/active_count getter） | 编译 + 单测 |
| 4 | `impl Pipeline for StreamingPipeline`（外层套壳）+ `StreamingPipelineEngine::finish_with_tail`→`finish` 去 tail + coordinator stop 改 `tick(tail)+finish` | 双 feature 编译 + 既有流式测试 |
| 5 | coordinator：`Stage::VadSegmented`/`WaitingCompletion` 字段改持 pipeline + tick handler 改调 `pipeline.tick` + 删 `Command::TranscriptionDone` + 删两处回填 handler + WaitingCompletion 复用 tick 驱动 | workspace check + clippy |
| 6 | e2e 回归（非流式本地引擎 VadSegmented：onset→切段→乱序回填→停顿润色→stop WaitingCompletion drain→finalize） | 手动 |

每 Task TDD + commit + 双 feature 编译 + clippy 零新 warning；Task 6 e2e 通过后 ff-merge main。
