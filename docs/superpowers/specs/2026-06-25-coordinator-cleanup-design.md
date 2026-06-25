# 2d coordinator 清理（emit/DB/polish 触发逻辑收敛进 pipeline）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已实施并 ff-merge main（Task 1-4，2026-06-25）。双 feature 编译 0 error、clippy 无 2d 引入的 dead_code、workspace 测试除 2 pre-existing infra 失败外全绿；e2e 零行为差异回归通过（2026-06-25）。
> **动机**：ASR pipeline 重构阶段2（总 spec `2026-06-23-asr-pipeline-design.md`）已把三条 ASR 编排路径收进统一 `Pipeline` 角色——流式（2a/2b/2c-1：`StreamingPipeline` 壳）+ cloud（2c-2：`StreamingPipelineEngine` trait + `CloudPipelineEngine`）+ VadSegmented（2c-3：`VadSegmentedPipeline` + 删 `TranscriptionDone`）。但 `Pipeline::tick` 目前只返回 `changed: bool`，**emit/DB/polish 的触发逻辑仍散在 coordinator 三处**（`handle_streaming_tick` / `after_vad_tick` / WaitingCompletion 内联），每处重复 `if changed { DB + emit } + polish` 的变体，cloud 还多出「每 tick emit 预览 + 错误上报」特判。2d 把这些**触发逻辑**收敛进 pipeline 产事件，coordinator 退化为统一事件路由。
> **关联**：总 spec `2026-06-23-asr-pipeline-design.md`（§3.4/§9/§11）；2c-1 `2026-06-23-...`（StreamingPipeline 壳）；2c-2 `2026-06-24-asr-pipeline-stage2c2-design.md`（StreamingPipelineEngine trait）；2c-3 `2026-06-25-vad-segmented-rehome-design.md`（VadSegmentedPipeline + Pipeline trait）。
> **范围**：`Pipeline::tick` 返回 `Vec<PipelineEvent>`；pipeline 内部产事件（changed/segment_cut/silence/error/cloud partial 全算好）；coordinator 三个 tick handler 合一为统一事件循环（抽 `apply_pipeline_events`，dispatch_tick + stop 路径共用）；删 `after_vad_tick`；Pipeline trait 精简（去 `silence_duration`/`took_segment_cut`，清 `#[allow(unused)]`）。**不含**：finalize 链、cloud close、Transcript 状态机、audio.rs、transcript 物理位置（留 Stage）。

---

## 1. 背景

2c-3 后，三条 ASR 路径都有 pipeline 壳（`StreamingPipeline` / `VadSegmentedPipeline`，impl `Pipeline` trait），`Pipeline::tick(&[f32], &mut Transcript) -> bool` 统一了编排 + set_full。但 **emit/DB/polish 的触发**（何时落库、何时刷窗口、何时润色）仍在 coordinator，且三处重复：

- `handle_streaming_tick`（coordinator.rs:1351）：local `changed→DB+emit` + 每 tick `polish(silence)`；cloud `changed→DB+polish` + 每 tick `emit(display+partial)` + `error 上报`。
- `after_vad_tick`（coordinator.rs:1202，vad-seg 专用）：`changed→DB+emit` + `segment_cut→polish(threshold)`。
- `handle_vad_segmented_tick` WaitingCompletion 分支（coordinator.rs:1181）：`changed→DB+emit`（内联，不复用 after_vad_tick）。

三处都是 `if changed { update_transcription_raw + update_result } + check_and_trigger_polish` 的变体，差异仅在：DB 的 engine_mode（`"streaming"`/`"vad_segmented"`）、polish 的 silence 参数（`silence_duration` / `pause_threshold`）、emit 是否含 cloud partial、是否每 tick emit、是否上报 error。

问题：coordinator 既管 Tauri 命令路由又管 ASR 结果分发细节；新增第四种路径（或调整 polish 节奏）要改三处；cloud/local/vad-seg 的分发差异散在 if-else，没有一个地方能看清「这条路径每个 tick 产出什么副作用」。

## 2. 现状（已探明）

### 2.1 Pipeline::tick 只回 bool
`pipeline.rs:101`：`fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool`。pipeline 内部 set_full，返回 `changed`。emit/DB/polish 全留 coordinator 据 `changed` + 额外查询（`silence_duration()`/`took_segment_cut()`/`take_error()`/`current_partial()`）自行决定。

### 2.2 emit/DB/polish 三处重复（coordinator.rs）
| 位置 | 行 | 逻辑 |
|---|---|---|
| `handle_streaming_tick` local 分支 | 1399-1409 | `changed→update_transcription_raw("streaming")+update_result(display)`；每 tick `check_and_trigger_polish(silence_duration)` |
| `handle_streaming_tick` cloud 分支 | 1376-1398 | `changed→update_transcription_raw("streaming")+check_and_trigger_polish(silence_duration)`；每 tick `update_result(display+current_partial)`；`take_error→update_result(e)` |
| `after_vad_tick`（vad-seg）| 1202-1222 | `changed→update_transcription_raw("vad_segmented")+update_result(display)`；`segment_cut→check_and_trigger_polish(pause_threshold)` |
| WaitingCompletion 内联 | 1181-1194 | `changed→update_transcription_raw("vad_segmented")+update_result(display)`（收尾，无 polish） |

### 2.3 transcript 留 Stage
`Stage::Streaming{pipeline, transcript}` / `VadSegmented{pipeline, transcript, tick_active}` / `WaitingCompletion{pipeline, transcript, tick_active}` 各持 `transcript`。`update_transcription_raw` / `check_and_trigger_polish` 都吃 `&mut Transcript`（含 `db_text()`/`db_inserted()`/`take_polish_input()`/`mark_polish_pending()` 等内部状态访问）。

### 2.4 finalize/cloud 不对称（2c-2 约束，须保留）
- `finalize_after_stop`（coordinator.rs:831）持 transcript 值传递：`polish_pending→StoppingPolish`；否则 `combined` 标点补全 + `skip_final_polish` 判定 + `do_paste`/`start_final_polish_or_paste`。
- cloud `close_async` 必须 async（`block_on` 卡主线程 8s），留 coordinator `Stage::CloudClosing` + session_id 护栏（2c-2 spec §2）。

## 3. 设计

### 3.1 核心决策（brainstorming 2026-06-25）
- **收敛深度 = 方案 A（事件流路由）**：pipeline 产事件，coordinator 统一遍历执行端动作。
- **transcript 留 Stage**（非解法 Y）：emit/DB/polish 吃 `&mut Transcript`，transcript 进 pipeline 会让 coordinator 拿不到 `&mut`；留 Stage 则 coordinator 统一事件循环仍持 `&mut transcript`，polish 防抖五重检查原位不动（不搬迁，风险最低）。transcript 物理位置未变，但 emit/DB/polish **触发逻辑**收敛进 pipeline 产事件，三处重复消除——达成 2d 实质目标（coordinator 退化为路由）。

### 3.2 PipelineEvent（pipeline.rs 新增）
```rust
/// pipeline tick 产出的「该做什么」事件。coordinator 据此执行端动作（DB/emit/polish/错误上报）。
/// 不携带 transcript 状态（transcript 留 Stage，coordinator 持 &mut）——只携带「决定 + 必要字符串」。
pub enum PipelineEvent {
    /// 落库 raw_text（pipeline 已判文本变化）。engine_mode = DB engine_mode 列（"streaming"/"vad_segmented"）。
    /// coordinator 调 update_transcription_raw(&mut transcript, &config.asr_engine, engine_mode)。
    PersistRaw { engine_mode: &'static str },
    /// 刷新结果窗口。display 已由 pipeline 算好（local=transcript.display_text()；cloud=display+current_partial）。
    /// coordinator 调 result_window::update_result(app_handle, &display)。
    Emit { display: String },
    /// 触发停顿润色。silence = 停顿时长（streaming 传 silence_duration；vad-seg 段边界传 pause_threshold 自动达标）。
    /// coordinator 调 check_and_trigger_polish(&mut transcript, silence, config, tx)（防抖五重检查在彼处原样）。
    Polish { silence: f64 },
    /// 用户可见错误（cloud WSS 开启失败 / StreamEvent::Failed；local 错误只在承载层 warn，不产此事件）。
    Error(String),
}
```

### 3.3 tick 签名 `bool → Vec<PipelineEvent>`
```rust
fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent>;
```
pipeline 内部完成所有「决定」：
- set_full（现状）+ 算 `changed`。
- cloud `current_partial` 拼接进 `Emit{display}`。
- `take_error` 取出进 `Error`（cloud）。
- 空样本早退（local streaming → `[]`）/ 仍 drain（cloud、vad-seg 收尾）的差异进 tick 内部（coordinator 不再 `if !is_cloud && samples.is_empty()` 特判）。

### 3.4 三路径 → 事件序列（pipeline 内部产）
| 路径 | tick 产事件（按序） |
|---|---|
| streaming local | `changed`→`[PersistRaw{streaming}, Emit{display}]`；每 tick 追加 `[Polish{silence_duration}]`；空样本→`[]`（早退） |
| streaming cloud | `changed`→`[PersistRaw{streaming}, Polish{silence_duration}]`；每 tick 追加 `[Emit{display+partial}]`；`error`→追加 `[Error(e)]` |
| vad-segmented | `changed`→`[PersistRaw{vad_segmented}, Emit{display}]`；`segment_cut`→追加 `[Polish{pause_threshold}]` |
| WaitingCompletion（vad-seg 收尾）| `changed`→`[PersistRaw{vad_segmented}, Emit{display}]`（无 polish，收尾不再切段） |

事件顺序保持现状副作用顺序：local `set_full→DB→emit`（PersistRaw 在 Emit 前）；cloud `set_full→DB→polish→emit`（PersistRaw/Polish 在 Emit 前）。

### 3.5 coordinator 统一事件循环（抽 `apply_pipeline_events`，dispatch_tick + stop 共用；删 after_vad_tick）
```rust
/// 事件循环体：dispatch_tick 与 stop 路径共用，保证「最后一段 tick 的 DB/emit 副作用不丢」。
fn apply_pipeline_events(
    events: Vec<PipelineEvent>,
    transcript: &mut Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    for ev in events {
        match ev {
            PipelineEvent::PersistRaw { engine_mode } => {
                if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, engine_mode) {
                    warn!("DB ({}) failed: {}", engine_mode, e);
                }
            }
            PipelineEvent::Emit { display } => {
                if !display.is_empty() {
                    crate::result_window::update_result(app_handle, &display);
                }
            }
            PipelineEvent::Polish { silence } => {
                check_and_trigger_polish(transcript, silence, config, tx);
            }
            PipelineEvent::Error(e) => {
                crate::result_window::update_result(app_handle, &e);
            }
        }
    }
}

/// VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令合一的 dispatch。
fn dispatch_tick(
    stage: &mut Stage, audio: &Arc<SharedAudioState>, config: &AppConfig,
    app_handle: &tauri::AppHandle, tx: &Sender<Command>,
) {
    let (events, is_waiting): (Vec<PipelineEvent>, bool) = match stage {
        Stage::Streaming { pipeline, transcript, .. }
        | Stage::VadSegmented { pipeline, transcript, .. }
        | Stage::WaitingCompletion { pipeline, transcript, .. } => {
            (pipeline.tick(&audio.drain_samples(), transcript), matches!(stage, Stage::WaitingCompletion { .. }))
        }
        _ => return,
    };
    // 取 transcript 的 &mut 给 apply（match arm 里 pipeline 已借 &mut self，transcript 同 arm 独立 &mut）
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        _ => return,
    };
    apply_pipeline_events(events, transcript, config, app_handle, tx);
    // WaitingCompletion 收尾判定保留：active_count==0 → tick_active=false + finalize_after_stop
    if is_waiting {
        if let Stage::WaitingCompletion { pipeline, transcript, tick_active } = stage {
            if pipeline.active_count() == 0 {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
    }
}
```
> 借用说明：Rust 借用检查下，`pipeline.tick(&mut self, …)` 与后续 `apply_pipeline_events(.., transcript, ..)` 不能在同一 match arm 内连续 `&mut` 同一 stage 的两个字段。实现时 pipeline.tick 先在第一段 match 取出 `events`（消费 `&mut pipeline`，结束其借用），第二段 match 再取 `&mut transcript` 喂 apply。WaitingCompletion 收尾用 2c-3 既有的 `mem::replace` 提取 owned transcript。plan 给出确切写法。

stop 路径**不复用 apply_pipeline_events，丢弃 tick 事件**（保持现状 stop 无 DB/emit/polish，副作用靠 finalize 的 show_result；零行为差异）：
```rust
// VadSegmented / Streaming stop 分支
let _ = pipeline.tick(&remaining, transcript);  // 仅 set_full（内部），事件 Vec 丢弃
pipeline.finish(transcript);  // 或 cloud finish，结构不动
```
> 设计修正（plan 实施时定性）：原设计拟让 stop 复用 apply_pipeline_events，但现状 stop 的 tick 只 set_full 无 DB/emit/polish（副作用全靠 `finalize_after_stop` 的 show_result）。若 stop 调 apply 会引入额外 DB/emit 改变行为。故 stop 丢弃事件，保零行为差异。

VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令在 command dispatch 处合一调 `dispatch_tick`。

### 3.6 边界（不动 — 降风险）
- **Stage 字段不变**：transcript + pipeline 都留。
- **finalize 链**：`finalize_after_stop` / `StoppingPolish` / `Pasting` / `start_final_polish_or_paste` / `do_paste` 保留 coordinator（持 transcript 值传递，2c-3 既有）。
- **cloud close**：`close_async` + `Stage::CloudClosing` + session_id 护栏保留 coordinator（2c-2 约束）。
- **stop 路径结构不动**：`pipeline.tick` 调用保留但**丢弃返回事件**（`let _ = pipeline.tick(..)`，不调 apply_pipeline_events，保 stop 现状无 DB/emit）；`pipeline.finish` / active_count 判定 / finalize 调用不变。

### 3.7 Pipeline trait 精简
- **去掉** trait 方法 `silence_duration` / `took_segment_cut`（信息进 `Polish` 事件，coordinator 不再直调）。原 `#[allow(unused)]` 改为 `#[allow(dead_code)]`：coordinator 持具体类型走 inherent `tick`（Rust inherent 优先于 trait），trait `tick` 不经 trait 路径调用（无 `dyn Pipeline`），故 `impl Pipeline::*::tick` 为 dead；trait 的 `finish`/`reset`/`is_cloud`/`take_close_handle` 仍经具体类型调用（无 inherent 同名）非 dead。未来若 coordinator 改 `Box<dyn Pipeline>` 统一 dispatch，trait tick 方经 trait 路径调用，届时可去 allow。
- 保留 `tick` / `finish` / `reset` / `take_close_handle` / `is_cloud`。
- `StreamingPipeline` inherent `take_error` 删（error 进 `Error` 事件，coordinator 不再取）；`current_partial` **保留 pub**（cloud stop 取 partial 给 `CloudClosing`——spec 原拟收回 internal，实施时 cloud stop 仍需它，故保留）。
- `VadSegmentedPipeline::active_count` 保留 `pub(crate)`（WaitingCompletion 收尾判定用）。

## 4. 接口契约
| 接口 | 变化 |
|---|---|
| `Pipeline` trait | `tick` 返回 `Vec<PipelineEvent>`（原 `bool`）；删 `silence_duration` / `took_segment_cut` |
| `pipeline.rs` | 新增 `PipelineEvent` enum；`StreamingPipeline` / `VadSegmentedPipeline` tick 内部产事件；inherent `take_error` 删；`current_partial` 保留 pub（cloud stop 用）；trait `#[allow(unused)]`→`#[allow(dead_code)]` |
| `coordinator.rs` | 新增 `apply_pipeline_events` + `dispatch_tick`；删 `after_vad_tick`；`handle_streaming_tick` / `handle_vad_segmented_tick` 移除（逻辑进 dispatch_tick）；三 Tick 命令 dispatch 合一；stop 路径 tick 适配 |
| `update_transcription_raw` / `check_and_trigger_polish` | **不改**（仍吃 `&mut Transcript`，由 apply_pipeline_events 调用） |
| `finalize_after_stop` / cloud close 链 | **不改** |
| `Stage` enum | **不改**（字段不变） |

## 5. 数据流
```
audio.drain_samples() ─&[f32]─▶ pipeline.tick(samples, &mut transcript)
                                   │ 内部：set_full + 算 changed / cloud partial / error / segment_cut / silence
                                   ▼
                              Vec<PipelineEvent>
                                   │
                   apply_pipeline_events(events, &mut transcript, cfg, app, tx):
                                   ├── PersistRaw{em} → update_transcription_raw(&mut t, asr_engine, em)
                                   ├── Emit{display}  → result_window::update_result(app, display)
                                   ├── Polish{silence}→ check_and_trigger_polish(&mut t, silence, cfg, tx)
                                   └── Error(e)       → update_result(app, e)
```

## 6. 错误处理
| 场景 | 行为（与现状一致） |
|---|---|
| DB 失败 | `update_transcription_raw` 返 Err，`apply_pipeline_events` `warn!` 不阻塞（local/cloud/vad-seg 统一） |
| emit 空串 | `Emit{display}` 空 → 跳过（`if !display.is_empty()`，等价现状 cloud `if !display.is_empty()`） |
| polish 防抖拦 | `check_and_trigger_polish` 五重检查（mode/pending/empty/has_increase/silence/interval）原样，不触发则无副作用 |
| cloud WSS 失败 | pipeline 承载层 warn + 产 `Error(e)`，apply `update_result(e)` 上报 |
| tick 空 Vec | 事件循环空转，无副作用（local 空样本早退 / 无变化 tick） |

## 7. 范围边界（不做）
- **transcript 物理位置不动**：留 Stage（不进 pipeline）——解法 Y 的 DB/polish 决定搬迁风险高，2d 不做。
- **finalize 链不动**：`finalize_after_stop` / StoppingPolish / Pasting / start_final_polish_or_paste / do_paste 原样。
- **cloud close 不动**：`close_async` / CloudClosing / session_id 护栏原样（2c-2 约束）。
- **Transcript 状态机不动**：`db_text` / `take_polish_input` / `mark_*` / display 分层原样。
- **audio.rs / asr helper 不动**。
- **stop 路径结构不动**：仅 tick 返回值适配。

## 8. 测试
- **pipeline 单测扩展**（pipeline.rs tests）：fake `StreamingPipelineEngine` + 断言 `tick → Vec<PipelineEvent>` 序列：
  - local：`Partial`(changed) → `[PersistRaw{streaming}, Emit{display}]` + `[Polish]`；与 full 相同（changed=false）→ `[]`；空样本 → `[]`（早退）。
  - cloud：`Committed`(changed) → `[PersistRaw, Polish]` + `[Emit{display+partial}]`；`Error` → `[Error(e)]`。
  - vad-seg：`segment_cut` → `[Polish{threshold}]`；纯 drain（无切段）→ 无 Polish。
- **coordinator dispatch_tick / apply_pipeline_events**：持 `app_handle` / `tx`，无单测；靠 `cargo check --workspace --all-targets`（双 feature）+ clippy 0 新 warning + 手动 e2e（VadSegmented 全路径 onset/force_cut/silence_cut/stop WaitingCompletion/跨会话/Cancel + cloud 流式识别 + 错误上报）。

## 9. 迁移映射
| 现状 | 归到 | 动作 |
|---|---|---|
| `handle_streaming_tick` local 分支 emit/DB/polish | `StreamingPipeline::tick` 产事件 + `apply_pipeline_events` 路由 | 删 if-changed-DB-emit + 每 tick polish，改产 `[PersistRaw,Emit]+[Polish]` |
| `handle_streaming_tick` cloud 分支 | `StreamingPipeline::tick`（cloud）产事件 | 改产 `[PersistRaw,Polish]+[Emit]+[Error]` |
| `after_vad_tick` | `VadSegmentedPipeline::tick` 产事件 | 删 after_vad_tick，改产 `[PersistRaw,Emit]+[Polish{threshold}]` |
| WaitingCompletion 内联 emit/DB | `VadSegmentedPipeline::tick`（收尾）产事件 | 改产 `[PersistRaw,Emit]`，收尾判定留 dispatch_tick |
| coordinator 直调 `silence_duration()`/`took_segment_cut()`/`take_error()`/`current_partial()` | pipeline tick 内部消费，进事件 | coordinator 不再直调这些 |
| `Pipeline` trait `silence_duration` / `took_segment_cut` | 删（信息进 Polish 事件） | 清 `#[allow(unused)]` |

## 10. 任务分解（概览，详见 plan）
1. `PipelineEvent` enum + `Pipeline::tick` 签名 `bool→Vec`（trait + 两 impl 编译过，事件先返占位 Vec）。
2. `StreamingPipeline::tick` 产事件（local + cloud 分支）+ inherent `current_partial` / `take_error` 收回内部。
3. `VadSegmentedPipeline::tick` 产事件（含 segment_cut / 收尾）。
4. coordinator `apply_pipeline_events` + `dispatch_tick` 统一事件循环 + 删 `after_vad_tick` + 三命令 dispatch 合一 + stop 路径适配。
5. Pipeline trait 精简（删 `silence_duration` / `took_segment_cut` + 清 `#[allow(unused)]`）。
6. 双 feature check + clippy + workspace 测试 + e2e 回归 + 文档同步。
