# ASR Pipeline 阶段 2c-2：云端流式接入 StreamingPipeline

> 2026-06-24 初版（brainstorming 产出）。
> **状态**：已合并 main（2026-06-24，T1-T4 + final review 共 7 commit `f8cd395`→`9928f60`，TDD + 双 feature 编译/测试通过 + clippy 零新 warning；e2e 通过——本地+云端流式识别正常，ff-merge main）。Approach 1：上层 trait `StreamingPipelineEngine`（`LocalPipelineEngine`/`CloudPipelineEngine`）+ cloud close 留 coordinator。plan `docs/superpowers/plans/2026-06-24-asr-pipeline-stage2c2.md`。
> **关联**：总 spec `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md` §3.4（阶段 2c-2）。
> **前置**：阶段 2a/2b/2c-1 已合并 main（本地流式链路已收敛进 `desktop::StreamingPipeline`）。
> **范围**：仅 cloud streaming（DashScope/ByteDance/Tencent/Baidu 长连接 WSS）。**VadSegmented（离线分段）语义模型不同，拆 2c-3 单独设计**。

---

## 1. 背景与问题

阶段 2c-1 把**本地流式**收敛进 `StreamingPipeline`（`desktop/pipeline.rs`）：`StreamingPipeline` 持 `asr::StreamingRunner`，`tick` 承载 `TranscriptEvent → set_full` 返回 `changed`，coordinator 在 `changed=true` 时 DB+emit。cloud 与 VadSegmented 两条路径原样未动。

cloud 流式（`Stage::CloudStreaming`）未进 pipeline 的根因——它与 `StreamingEngine` trait（local 实现）存在五重语义不匹配：

| 维度 | `StreamingEngine`（local） | `CloudStreamHandle`（cloud） |
|---|---|---|
| 调用模型 | `&self` 同步，`accept_samples` 即时返回文本 | `push_pcm` 不返回；`try_recv_text` 异步取 |
| 结果时机 | sample 级同步（每帧有结果） | utterance 级异步（event 流） |
| VAD 角色 | 客户端 VAD 触发 `flush` 插逗号 | 服务端分句；客户端 VAD 仅 onset + 静音→finish |
| 文本模型 | 单层 `set_full` 覆盖 | 双层 `current_partial`(预览) + `transcript`(append) |
| 收尾 / session | 同步 `finish`，单 session | `close_async`（async），多 WSS（每 utterance 一条） |

强扭 cloud `impl StreamingEngine` 不可行：`accept_samples` 的同步签名与 cloud 异步事件流冲突；StreamingRunner 的 `detect_silence_gap + flush(true)` 是给 local 插逗号的，cloud 服务端已分句，硬塞会重复标点。

## 2. 核心约束：cloud close 不可消除

cloud 的 `close_async`（`cloud_types.rs:83`）必须 async——收最终结果要 `await`，否则 `block_on` 卡 coordinator 主线程最多 8s（审查三1 正是为此改非阻塞）。而 coordinator 主循环是同步的（`std::thread` + channel，非 tokio），async 结果只能 spawn 后经 `Command::CloudStreamingDone` 回传。

**结论**：`Stage::CloudClosing` 中间态 + `session_id` 跨会话护栏（`coordinator.rs:141/1198`）本质上无法消除，必须留在 coordinator。pipeline 只收敛 cloud 的**同步 tick 部分**。任何「cloud 完全进 pipeline、close 也进」的方案（含 async trait）都是假象——中间态无论如何要在 coordinator。

## 3. 方案：上层 trait + close 留 coordinator（Approach 1）

### 3.1 架构

```
coordinator（同步主循环）
  └─ Stage::Streaming { pipeline: StreamingPipeline, transcript, streaming_active }
       └─ StreamingPipeline（承载逻辑：TranscriptEvent → transcript.set_full/append → changed）
            └─ engine: Box<dyn StreamingPipelineEngine>   ← local 或 cloud
                 ├─ LocalPipelineEngine  → 包 asr::StreamingRunner（VAD + accept/flush，2c-1 既有）
                 └─ CloudPipelineEngine  → 持 CloudStreamHandle（onset/push/drain/静音finish）

cloud close（不可消除的特例，留 coordinator）：
  Stage::Streaming stop → pipeline.finish_with_tail(tail)
                       → pipeline.take_close_handle() → Some(CloudStreamHandle)
                       → spawn close_async → Stage::CloudClosing
                       → Command::CloudStreamingDone → finalize_cloud
```

cloud 的同步 tick（onset 检测 / push_pcm / drain events / partial-transcript 双层 / 静音非阻塞 finish）迁入 `CloudPipelineEngine.tick`；coordinator 的 `handle_cloud_streaming_tick` 退化为 `pipeline.tick` + DB/emit，与本地流式对称。cloud 的 async close 路径（`CloudClosing` + `close_async` + `session_id` 护栏 + `finalize_cloud`）原样保留。

### 3.2 trait 定义

放 `desktop/src/pipeline.rs`（StreamingPipeline 所在，desktop pipeline 层抽象；`asr::StreamingEngine` 是更底层的 sample 级零件，保持不动供 cli/server 复用）。

```rust
/// desktop 流式 pipeline 引擎（上层抽象，spec §3.4 阶段2c-2）。
/// local（包 StreamingRunner）与 cloud（持 CloudStreamHandle）各 impl。
/// 同步 tick + 同步 finish_with_tail；cloud 的 async close 不在此 trait（留 coordinator，§2）。
pub trait StreamingPipelineEngine: Send {
    /// 喂一帧已降噪 16k 样本，返回本帧 TranscriptEvent（0..n）。
    fn tick(&mut self, samples: &[f32]) -> Vec<TranscriptEvent>;
    /// 收尾：吃入尾部样本 + finish。
    ///   local  → StreamingRunner.finish_with_tail（accept tail + finish，返回 Final）。
    ///   cloud  → **只 push tail**（不发 Finish——cloud 的 Finish 由 coordinator 的 close_async 发，
    ///            见 §4.3，避免重复 Finish），返回最后 current_partial 作兜底（不产 Final）。
    fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent;
    /// 当前累积静音时长（停顿润色触发用）。
    fn silence_duration(&self) -> f64;
    /// cloud 预览（current_partial），coordinator display 拼接用。local 默认空。
    /// cloud 双层文本：预览不进 transcript/DB，仅 display（§4.1/§4.2 不对称）。
    fn current_partial(&self) -> &str { "" }
    /// 重置（会话间复用）。
    fn reset(&mut self);
    /// cloud engine 的 async close 句柄（stop 时 coordinator 取出 spawn close_async）。
    /// local 返回 None（默认）；cloud 取出内置 session 后返回 Some。
    /// §2：cloud close 不可消除，此方法让 coordinator 在 stop 路径分派 cloud/local。
    fn take_close_handle(&mut self) -> Option<crate::cloud_types::CloudStreamHandle> { None }
}
```

`take_close_handle` 用默认实现（`None`），local 不覆盖（无感），cloud 覆盖返回 `Some`。选 trait 而非 enum 分派：local 用默认无感、StreamingPipeline 的承载逻辑（events → set_full/append）对 local/cloud 共享写一次。

### 3.3 两个 engine 实现

```rust
/// local：薄包 StreamingRunner，转发（VAD + accept/flush 编排仍在 asr::StreamingRunner）。
pub struct LocalPipelineEngine(StreamingRunner);
impl LocalPipelineEngine {
    pub fn new(spec: &str, correct: bool) -> anyhow::Result<Self> {
        let session = StreamingSession::new(spec)?;      // asr sample 级 session
        Ok(Self(StreamingRunner::new(Box::new(session), correct)?))
    }
}
// impl StreamingPipelineEngine：tick→runner.push_samples, finish_with_tail→runner.finish_with_tail,
//   silence_duration/reset 转发；take_close_handle 用默认 None。

/// cloud：持 CloudStreamHandle + onset/状态（搬迁 handle_cloud_streaming_tick:1632-1812 的字段）。
#[cfg(feature = "cloud")]
pub struct CloudPipelineEngine {
    vad: octopus_asr::vad::SileroVad,
    pre_roll_buffer: Vec<f32>,
    session: Option<CloudStreamHandle>,   // onset 后 Some；Finished/Failed 后 take 清 None
    current_partial: String,              // 当前 session 累积预览（未提交）
    silence_duration: f64,
    is_speaking: bool,
    speech_confirm_count: u32,            // onset 连续确认（消除单次噪声脉冲）
    is_closing: bool,                     // 已发非阻塞 finish，等 Finished
    cloud_cfg: CloudCfg,                  // endpoint/key/model/language（open session 用）
    rt: tauri::async_runtime::RuntimeHandle,
}
```

`StreamingPipeline` 从 2c-1 的「持 `StreamingRunner`」改为「持 `Box<dyn StreamingPipelineEngine>`」；`StreamingPipeline::new` 签名由 `new(Box<dyn StreamingEngine>, correct)` 改为 `new(Box<dyn StreamingPipelineEngine>)`（engine 已含 runner/状态）。`LocalPipelineEngine` 内部构造 `StreamingRunner`，故 `asr::StreamingRunner`/`StreamingEngine` 不动。

## 4. 数据流

### 4.1 cloud tick（`CloudPipelineEngine.tick`）

原样搬迁 `handle_cloud_streaming_tick`（`coordinator.rs:1632-1812`）的 ASR 部分，产 `Vec<TranscriptEvent>` 而非直接写 transcript/emit：

1. `drain_samples` → 追加 `pre_roll_buffer`（超容量弹头）
2. VAD 检测（`compute_speech_chunks`）；有语音→`silence_duration=0` + `speech_confirm_count++`；静音→累加 + 清零确认
3. 连续 2 tick 确认 onset → `open_cloud_session` + `push_pcm(samples)`，`session=Some`
4. 有 session：`push_pcm`（`!is_closing` 时）+ drain `try_recv_text`：
   - `Text(t)` 非空 → `current_partial = t`（**预览层，不进 transcript/DB**，仅 display；engine 内部持有，不发 TranscriptEvent）
   - `Finished` → `current_partial` append 进 transcript，发 `Committed`（**DB 触发点**）；`is_closing=false`、`is_speaking=false`
   - `Failed(msg)` → 发 `Error(msg)`，清 `current_partial`/状态（下次 onset 重开，瞬时抖动自动重试）
   - `!is_closing && !is_speaking` → `session.take()`（drop → channels 关 → WS task 结束）
5. `is_speaking && !is_closing && silence ≥ pause_polish_threshold` → `sess.finish()` 非阻塞，`is_closing=true`

> **transcript 双层归属（行为零差异关键）**：cloud 的 `current_partial` 是**预览层**——不进 transcript、不进 DB，仅用于 display（与现状 `render_display(transcript, current_partial)` 一致）。只有 `Finished`（→`Committed`）时 `current_partial` 才 append 进 transcript 并触发 DB。故 `CloudPipelineEngine` 内置 `current_partial`（预览，engine 自持 + 暴露 `current_partial()`）+ 已提交累积两份；`Committed` 事件携带已提交全文供 `StreamingPipeline.set_full`，`Text`（预览）**不作为进 transcript 的事件**。这与 local 的 `Partial`（即全文，直接 `set_full`）不同——是 cloud 的第二处不对称（与 §5 `Final` 不对称并列）。

### 4.2 StreamingPipeline.tick（承载，local/cloud 共享）

```rust
/// 承载：把 engine 事件落到 transcript。local 的 Partial/Committed/Final 都 set_full。
/// cloud 的预览（current_partial）不经过此——engine 自持 + 暴露 current_partial()（§4.1）。
pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
    let mut changed = false;
    for event in self.engine.tick(samples) {
        match event {
            TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                if text != transcript.full() { transcript.set_full(&text); changed = true; }
            }
            TranscriptEvent::Final(text) => { transcript.set_full(&text); changed = true; }  // local stop
            TranscriptEvent::Error(e) => warn!("pipeline event error: {}", e),
        }
    }
    changed
}
/// cloud 预览（current_partial），local 恒空。coordinator display 拼接用。
pub fn current_partial(&self) -> &str { self.engine.current_partial() }
```

承载逻辑与 2c-1 一致（幂等 `set_full`），新增 `Final` 显式承载（local `finish_with_tail` 产 `Final`）。

**local/cloud 不对称（coordinator tick 后处理）**：
- **local**：`changed` → DB + emit(`transcript.display_text()`)（幂等，无变化不 emit，与 2c-1 一致）。
- **cloud**：`changed`（= `Committed` 落 transcript）→ DB；**每 tick emit** `transcript.display_text() + engine.current_partial()`（预览频繁变化需即时反映，与现状 cloud tick 末尾总 emit 一致）。预览**不进 DB**。

这一不对称是 cloud 双层文本（预览 vs 已提交）的本质体现，与 §5 的 `Final` 不对称并列。

### 4.3 cloud stop（coordinator，close 路径不动）

```rust
Stage::Streaming { pipeline, transcript, .. } => {
    let final_samples = audio.drain_samples();
    let _ = audio.stop();
    let _ = pipeline.finish_with_tail(&final_samples);   // cloud: 只 push tail（Finish 由 close_async 发，避免重复）
    if let Some(handle) = pipeline.take_close_handle() {  // cloud → Some；local → None
        // spawn close_async + Stage::CloudClosing + session_id 护栏（与审查三1 完全一致）
        let session_id = transcript.id;
        rt.spawn(async move {
            let result = handle.close_async().await;
            let _ = tx.send(Command::CloudStreamingDone { text: result.map_err(|e| e.to_string()), session_id });
        });
        *stage = Stage::CloudClosing { transcript, current_partial: pipeline.current_partial().to_string() };
        return;
    }
    // local：同步 finalize
    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
    finalize_after_stop(stage, tr, config, app_handle, tx);
}
```

`CloudClosing` / `CloudStreamingDone` / `handle_cloud_streaming_done`（`coordinator.rs:1198`）/ `finalize_cloud`（`coordinator.rs:1141`）/ `session_id` 护栏——**全部原样保留**。

## 5. TranscriptEvent 映射（cloud 的不对称）

| cloud 原始 | TranscriptEvent | pipeline 承载 | 备注 |
|---|---|---|---|
| `StreamEvent::Text(累积全文)` | （engine 内部 `current_partial`，**不发事件**） | display 拼接，**不进 transcript/DB** | 预览层，体现双层 |
| `StreamEvent::Finished` | `Committed` | append `current_partial` 到已提交 | 跨 utterance 拼接 |
| `StreamEvent::Failed(msg)` | `Error` | coordinator 报错（`update_result`） | 重试 onset |
| `close_async` 结果 | `Final`（**coordinator 产**） | `set_full` 覆盖整段 | pipeline 对 cloud **不产 Final** |

cloud 的不对称：`Final` 不来自 `pipeline.finish_with_tail`（那只做副作用 + 返回兜底 `current_partial`），而来自 coordinator 的 `close_async` 结果（`handle_cloud_streaming_done:1217` 的 `set_full`）。这是 cloud close 必须留 coordinator的直接体现。

## 6. coordinator 改动（收敛清单）

| 项 | 改动 |
|---|---|
| `Stage::CloudStreaming`（`coordinator.rs:115`，10+ 字段） | **删除**，合并进 `Stage::Streaming { pipeline, transcript, streaming_active }`。cloud 状态字段全进 `CloudPipelineEngine`。 |
| `handle_cloud_streaming_tick`（`coordinator.rs:1632-1812`） | **删除**，合并进 `handle_streaming_tick`（统一 `pipeline.tick` + DB/emit）。 |
| `Stage::CloudClosing` / `CloudStreamingDone` / `handle_cloud_streaming_done` / `finalize_cloud` / `session_id` 护栏 | **原样保留**（cloud close 不可消除部分）。 |
| stop 路径（`coordinator.rs:883`） | 改用 `pipeline.finish_with_tail` + `take_close_handle` 分派 cloud/local。 |
| `handle_toggle` cloud 分支（`coordinator.rs:628-664`） | 建 `CloudPipelineEngine` → `StreamingPipeline::new(Box::new(cloud_engine))`，进 `Stage::Streaming`（与 local 分支对称）。 |
| `open_cloud_session` / `is_cloud_engine` / `start_cloud_streaming_tick_thread` / 常量（`CLOUD_PREROLL_BUFFER_SAMPLES` 等） | 搬进 `CloudPipelineEngine` 或其构造路径。 |

净效果：coordinator 的 cloud tick 代码（~180 行）迁出，`Stage::CloudStreaming` + `handle_cloud_streaming_tick` 删除，cloud 与 local 在 tick 层完全对称；coordinator 仅保留 cloud 独有的 close 中间态。

## 7. 行为零差异 + 测试

**零差异保证**：
- tick 逻辑原样搬迁（onset 连续确认 / pre_roll 滚动 / push / drain / partial-transcript 双层 / 静音非阻塞 finish / Failed 重试 / session take）
- close 路径完全不动（`CloudClosing` + `close_async` + `session_id` 护栏 + `finalize_cloud`）
- `TranscriptEvent` 映射保持现有 `current_partial`(预览) + `transcript`(提交) 双层语义
- **DB 时机不变**：cloud 仅 `Finished`/`Committed` 时 DB（预览 `current_partial` 不进 DB，与现状一致）；local `changed` 时 DB
- emit 频率不变：local `changed` 时（幂等，无变化不 emit）；cloud 每 tick（预览即时反映）

**单测**：
- `FakeCloudSession`（可编程 onset / `StreamEvent` 序列）→ `CloudPipelineEngine.tick` 的 `TranscriptEvent` 映射（Partial/Committed/Error、session 生命周期、静音 finish、onset 确认）
- `StreamingPipeline` 对 cloud engine 的承载（`set_full` 幂等、`Final` 覆盖）
- `take_close_handle`：cloud 取出后 `session=None`；local 返回 `None`
- `pipeline.rs` 既有 2 个测试（2c-1）：`FakeStreamingEngine` 包成 `FakePipelineEngine impl StreamingPipelineEngine`（适配新 trait）

**e2e（用户本地，需 DashScope key）**：cloud 流式 onset 开 WSS → partial 预览 → 停顿 Finished 提交 → stop close → 跨会话护栏（close 在飞时 Cancel/重开）。

## 8. 风险与边界

- **cloud `Final` 不对称**：pipeline 对 cloud 不产 `Final`，承载层 `Final` 分支仅 local 走。测试须覆盖 cloud 路径不误触 `Final`。
- **cloud 双层 DB 语义**：预览 `current_partial` 不可进 transcript/DB（仅 display）；仅 `Committed`（Finished）落 DB。StreamingPipeline 承载 + coordinator tick 后处理须区分 local/cloud（§4.2 不对称）。测试须覆盖 cloud 预览不触发 DB。
- **`StreamingPipeline::new` 签名破坏性变更**：2c-1 接 `Box<dyn StreamingEngine> + correct`，2c-2 改接 `Box<dyn StreamingPipelineEngine>`。`LocalPipelineEngine::new` 内部化 `StreamingSession::new` + `StreamingRunner::new` + correct。coordinator 两个构造点（local/cloud）同步改。
- **transcript 双层**：`CloudPipelineEngine` 内置「已提交累积」副本，`Partial` 携带拼接全文。须确认与现有 `transcript.append_segment("，")` 逗号拼接逻辑一致（`coordinator.rs:1747-1752`）。
- **cloud `finish_with_tail` 返回值**：返回最后 `current_partial` 作 `Committed` 兜底（close 失败时 coordinator 仍有文本），不产 `Final`。
- **不动**：`asr::StreamingEngine` / `StreamingRunner` / `StreamingSession`（cli/server 仍用）；`Stage::CloudClosing` 及其 close 链；denoise/resample（留 `audio.rs`）。

## 9. 后续

- **2c-3**：VadSegmented（离线分段，`OfflineAsrEngine` async `transcribe` + seq 乱序回填）归位。语义模型不同（非流式分段），单独设计。
- **2d**：coordinator 清理——`StreamingPipeline` 完整接管三条路径的 emit/DB/polish，coordinator 退化为纯路由。cloud 的 close 中间态是 2d 仍需保留的唯一 cloud 特例。
