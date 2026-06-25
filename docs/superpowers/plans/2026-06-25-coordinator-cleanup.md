# 2d coordinator 清理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 把散在 coordinator 三处的 emit/DB/polish 触发逻辑收敛进 pipeline 事件流（`PipelineEvent`），coordinator 退化为统一事件路由，零行为差异。

**Architecture:** `Pipeline::tick` 产 `Vec<PipelineEvent>`（PersistRaw/Emit/Polish/Error）；coordinator 抽 `apply_pipeline_events`（dispatch_tick + stop 共用概念，但 stop 实际丢弃事件保持现状）+ `dispatch_tick`（三 Tick 命令合一）。transcript 留 Stage，finalize/cloud close/Transcript 状态机不动。迁移用「先加 `tick_events` inherent(Vec) → coordinator 切 → trait 合并」4 步，每 task 自洽编译。

**Tech Stack:** Rust，tauri 2 desktop crate，`crates/desktop/src/{pipeline.rs,coordinator.rs}`。spec `docs/superpowers/specs/2026-06-25-coordinator-cleanup-design.md`。

**迁移策略说明（重要）：** `Pipeline::tick` 签名 `bool → Vec` 是全局原子改动（trait + StreamingPipeline + VadSegmentedPipeline + coordinator 全调用点）。若一步改全，中间编译断。故：
- Task 1：pipeline 加 inherent `tick_events(..) -> Vec<PipelineEvent>`（新方法，复用现有 `tick`/`run_tick`，不重复 set_full），trait `tick(bool)` 不动，coordinator 不动 → 编译过。
- Task 2：coordinator 切 `tick_events`（apply_pipeline_events + dispatch_tick），删旧 handler → 编译过。
- Task 3：trait `tick` 签名改 Vec（合并：`tick_events` → `tick`），删旧 inherent `tick(bool)` + trait `silence_duration`/`took_segment_cut` + 清 `#[allow(unused)]` → 编译过。
- Task 4：验证 + 文档 + ff-merge。

---

### Task 1: PipelineEvent + 两 pipeline 加 inherent tick_events（Vec）+ 单测

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（加 `PipelineEvent` enum；`StreamingPipeline::tick_events`；`VadSegmentedPipeline::tick_events`；`FakePipelineEngine` 加 `is_cloud` 可配；streaming tick_events 单测）

**目的：** 新增事件 enum + 两 pipeline 的 inherent `tick_events`（产事件流，复用现有 tick/run_tick 不碰 set_full 逻辑）。trait 与 coordinator 不动，编译自洽。

- [x] **Step 1: 加 PipelineEvent enum**

在 `pipeline.rs` 的 `SegmentResult` struct（L30）之前加：

```rust
/// pipeline tick 产出的「该做什么」事件。coordinator `apply_pipeline_events` 据此执行端动作
/// （DB/emit/polish/错误上报）。不携带 transcript 状态（transcript 留 Stage，coordinator 持 &mut）
/// ——只携带「决定 + 必要字符串」。（2d，spec §3.2）
#[derive(Debug, PartialEq)]
pub enum PipelineEvent {
    /// 落库 raw_text（pipeline 已判文本变化）。engine_mode = DB engine_mode 列（"streaming"/"vad_segmented"）。
    /// coordinator 调 update_transcription_raw(&mut transcript, &config.asr_engine, engine_mode)。
    PersistRaw { engine_mode: &'static str },
    /// 刷新结果窗口。display 已由 pipeline 算好（local=transcript.display_text()；cloud=display+current_partial）。
    /// coordinator 调 result_window::update_result(app_handle, &display)。
    Emit { display: String },
    /// 触发停顿润色。silence = 停顿时长（streaming 传 silence_duration；vad-seg 段边界传 f64::INFINITY 必过，
    /// 等价原 after_vad_tick 传 pause_polish_threshold_ms 让 check_and_trigger_polish 静音检查自动达标）。
    /// coordinator 调 check_and_trigger_polish(&mut transcript, silence, config, tx)（防抖五重检查原样在彼处）。
    Polish { silence: f64 },
    /// 用户可见错误（cloud WSS 开启失败 / StreamEvent::Failed；local 错误只在承载层 warn，不产此事件）。
    Error(String),
}
```

- [x] **Step 2: 加 StreamingPipeline::tick_events（复用 inherent tick）**

在 `StreamingPipeline::tick` inherent 方法（L197-218）之后、`finish`（L225）之前加：

```rust
    /// 产 tick 事件流（2d，spec §3.4）。coordinator `dispatch_tick` 调此 + `apply_pipeline_events`。
    /// 复用 inherent `tick` 的 set_full/last_error 逻辑（不重复），按 `is_cloud` 决定事件序列：
    /// - local：`changed`→`[PersistRaw, Emit]`；每 tick 追加 `[Polish{silence_duration}]`；空样本→`[]`（早退）
    /// - cloud：`changed`→`[PersistRaw, Polish]`；每 tick 追加 `[Emit{display+partial}]`；`error`→追加 `[Error]`
    pub fn tick_events(
        &mut self,
        samples: &[f32],
        transcript: &mut Transcript,
    ) -> Vec<PipelineEvent> {
        let is_cloud = self.engine.is_cloud();
        // local 空样本早退（等价原 handle_streaming_tick L1370）；cloud 不早退（仍 emit 预览/drain）
        if !is_cloud && samples.is_empty() {
            return Vec::new();
        }
        let changed = self.tick(samples, transcript); // set_full + 设 last_error（复用，不重复逻辑）
        let mut events = Vec::new();
        if is_cloud {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
            }
            if let Some(e) = self.last_error.take() {
                events.push(PipelineEvent::Error(e));
            }
            // 每 tick emit（display + current_partial 预览，预览不进 DB）
            let base = transcript.display_text();
            let partial = self.engine.current_partial();
            let display = if partial.is_empty() {
                base
            } else {
                format!("{}{}", base, partial)
            };
            events.push(PipelineEvent::Emit { display });
        } else {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Emit { display: transcript.display_text() });
            }
            // local 每 tick 查停顿润色（等价原 handle_streaming_tick L1408）
            events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
        }
        events
    }
```

- [x] **Step 3: 加 VadSegmentedPipeline::tick_events（复用 run_tick）**

在 `VadSegmentedPipeline::run_tick`（L431-485）之后、`impl Pipeline for VadSegmentedPipeline`（L488）之前加：

```rust
    /// 产 tick 事件流（2d，spec §3.4）。复用 `run_tick`（双 VAD+切段+spawn+drain+set_full，不重复），
    /// 按 `changed`/`segment_cut` 产事件：
    /// `changed`→`[PersistRaw{vad_segmented}, Emit]`；`segment_cut`→追加 `[Polish{INFINITY}]`
    ///（段边界 silence 必过，等价原 after_vad_tick L1221 传 pause_polish_threshold_ms）。
    /// WaitingCompletion 收尾也走此（空样本 run_tick 跳过切段仅 drain，segment_cut 恒 false → 无 Polish）。
    pub(crate) fn tick_events(
        &mut self,
        samples: &[f32],
        transcript: &mut Transcript,
    ) -> Vec<PipelineEvent> {
        let changed = self.run_tick(samples, transcript);
        let segment_cut = self.segment_cut_this_tick;
        let mut events = Vec::new();
        if changed {
            events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
            events.push(PipelineEvent::Emit { display: transcript.display_text() });
        }
        if segment_cut {
            events.push(PipelineEvent::Polish { silence: f64::INFINITY });
        }
        events
    }
```

- [x] **Step 4: FakePipelineEngine 加 is_cloud 可配（供 cloud tick_events 测试）**

改 `tests` 模块里的 `FakePipelineEngine`（L536-562）。struct 加 `is_cloud` 字段，`new` 设 false，加 `new_cloud` 构造器，impl `is_cloud`：

```rust
    struct FakePipelineEngine {
        tick_out: Mutex<Vec<TranscriptEvent>>,
        partial: String,
        finish_out: TranscriptEvent,
        silence: f64,
        is_cloud: bool,
    }
    impl FakePipelineEngine {
        fn new(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self { tick_out: Mutex::new(tick), partial: partial.to_string(), finish_out: finish, silence: 0.0, is_cloud: false }
        }
        fn new_cloud(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self { tick_out: Mutex::new(tick), partial: partial.to_string(), finish_out: finish, silence: 0.0, is_cloud: true }
        }
    }
    impl StreamingPipelineEngine for FakePipelineEngine {
        fn tick(&mut self, _samples: &[f32]) -> Vec<TranscriptEvent> {
            std::mem::take(&mut *self.tick_out.lock().unwrap())
        }
        fn finish(&mut self) -> TranscriptEvent { self.finish_out.clone() }
        fn silence_duration(&self) -> f64 { self.silence }
        fn current_partial(&self) -> &str { &self.partial }
        fn reset(&mut self) {}
        fn is_cloud(&self) -> bool { self.is_cloud }
    }
```

- [x] **Step 5: 写 streaming tick_events 单测（local changed/no-change/empty + cloud）**

在 `tests` 模块（`finish_delegates_to_engine` 测试之后）加：

```rust
    #[test]
    fn tick_events_local_changed_produces_persist_emit_polish() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![
            PipelineEvent::PersistRaw { engine_mode: "streaming" },
            PipelineEvent::Emit { display: "你好".to_string() },
            PipelineEvent::Polish { silence: 0.0 },
        ]);
    }

    #[test]
    fn tick_events_local_empty_samples_returns_empty() {
        let mut p = pipeline(FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".into())));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        assert!(p.tick_events(&[], &mut t).is_empty());
    }

    #[test]
    fn tick_events_local_no_change_only_polish() {
        // Committed 与 full 同 → changed=false → 只产 Polish（local 每 tick）
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".into())], "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("一样");
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![PipelineEvent::Polish { silence: 0.0 }]);
    }

    #[test]
    fn tick_events_cloud_changed_emits_display_with_partial() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Committed("已提交".into())],
            "预览中", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        // changed → PersistRaw + Polish；每 tick Emit(display+partial) = "已提交预览中"
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Emit { display } if display == "已提交预览中")));
    }

    #[test]
    fn tick_events_cloud_error_produces_error_event() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Error("boom".into())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Error(msg) if msg == "boom")));
    }
```

> VadSegmentedPipeline::tick_events 不加单测——构造依赖 SileroVad 模型文件（`find_silero_vad`），单测难；逻辑简单（run_tick + 产事件），靠 Task 4 e2e 覆盖。

- [x] **Step 6: 跑测试**

Run: `cargo test -p octopus-desktop pipeline::tests`
Expected: 全绿（含 5 个新 tick_events 测试 + 既有测试）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): PipelineEvent + 两 pipeline tick_events 产事件（2d Task 1）"
```

---

### Task 2: coordinator apply_pipeline_events + dispatch_tick + 删旧 handler + 三命令合一 + stop 适配

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（加 `apply_pipeline_events` + `dispatch_tick`；删 `after_vad_tick` + `handle_streaming_tick` + `handle_vad_segmented_tick`；三 Tick 命令 dispatch 合一；stop 路径 tick 适配丢弃事件）

**目的：** coordinator 切事件流——三 Tick 命令合一调 `dispatch_tick`，emit/DB/polish 由 `apply_pipeline_events` 统一路由。stop 路径丢弃 tick 事件（保持现状 stop 无 DB/emit，零行为差异）。

- [x] **Step 1: 加 apply_pipeline_events（事件循环体）**

在 `update_transcription_raw`（L2046）之前加：

```rust
/// pipeline 事件 → 端动作（DB/emit/polish/错误上报）。2d 统一路由，消除三路径重复。（spec §3.5）
fn apply_pipeline_events(
    events: Vec<crate::pipeline::PipelineEvent>,
    transcript: &mut Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    use crate::pipeline::PipelineEvent;
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
```

- [x] **Step 2: 加 dispatch_tick（三 Tick 命令合一的 dispatch）**

在 `apply_pipeline_events` 之后加：

```rust
/// VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令合一的 dispatch（2d，spec §3.5）。
/// 各 Stage 变体调对应 pipeline 的 `tick_events` → `apply_pipeline_events` 统一路由。
/// WaitingCompletion 额外做 active_count==0 收尾判定（沿用 2c-3 既有逻辑）。
fn dispatch_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let samples = audio.drain_samples();
    match stage {
        Stage::Streaming { pipeline, transcript, .. } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::VadSegmented { pipeline, transcript, .. } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::WaitingCompletion { pipeline, transcript, tick_active } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            // 所有在途段完成 → 收尾（停 tick 线程 + finalize）
            if pipeline.active_count() == 0 {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
        _ => {}
    }
}
```

> 借用：`pipeline.tick_events(&samples, transcript)` 借 `&mut pipeline` + `&mut transcript`（同 Stage 两字段，disjoint borrow），调用结束释放；随后 `apply_pipeline_events(.., transcript, ..)` 再借 `&mut transcript`。WaitingCompletion 收尾的 `pipeline.active_count()`(&self) 与 `mem::replace(transcript,..)`(&mut) disjoint。编译验证。

- [x] **Step 3: 删 after_vad_tick + handle_streaming_tick + handle_vad_segmented_tick**

删除三个函数（逻辑已进 `tick_events` + `apply_pipeline_events` + `dispatch_tick`）：
- `after_vad_tick`（L1202-1223，整函数）。
- `handle_streaming_tick`（L1351-1410，整函数）。
- `handle_vad_segmented_tick`（L1163-1199，整函数）。

- [x] **Step 4: 三 Tick 命令 dispatch 合一调 dispatch_tick**

改 command dispatch（L231-281）。三 arm 各自保留 `polish_mode` 读取 + `set_mode` 前置，把 `handle_streaming_tick(..)` / `handle_vad_segmented_tick(..)` 调用改为 `dispatch_tick(..)`：

```rust
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    #[cfg(feature = "cloud")]
                    Command::CloudStreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. }
                        | Stage::WaitingCompletion { transcript, .. } = &mut stage
                        {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

- [x] **Step 5: stop 路径 tick 丢弃事件（保持 stop 无 DB/emit，零行为差异）**

stop 路径三处 `pipeline.tick(..)` 改 `pipeline.tick_events(..)` 并丢弃返回 Vec（事件丢弃 = 等价现状 stop 只 set_full 不 DB/emit）：

- VadSegmented stop（L706）：
  ```rust
            if !remaining.is_empty() {
                let _ = pipeline.tick_events(&remaining, &mut transcript);
            }
  ```
- Streaming cloud stop（L736）：
  ```rust
                if !final_samples.is_empty() {
                    let _ = pipeline.tick_events(&final_samples, transcript);
                }
  ```
- Streaming local stop（L773）：
  ```rust
            if !final_samples.is_empty() {
                let _ = pipeline.tick_events(&final_samples, transcript);
            }
  ```

> 说明：现状 stop 路径的 `pipeline.tick` 只 set_full（更新 transcript），无 DB/emit/polish——副作用靠 `finalize_after_stop` 的 `show_result`。丢弃 `tick_events` 的返回事件保持这一行为（pipeline 内部 set_full/spawn/drain 照常，仅 emit/DB/polish 信号不执行）。零行为差异。

- [x] **Step 6: 编译 + clippy**

Run: `cargo check -p octopus-desktop --all-targets 2>&1 | tail -5`
Expected: 0 error（删除的函数无残留引用；`dispatch_tick` 覆盖三命令）。

Run: `cargo clippy -p octopus-desktop --all-targets --features cloud 2>&1 | grep -E "^warning" | wc -l`
Expected: 0 新 warning（与基线比）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): coordinator dispatch_tick 统一事件循环 + 删旧 handler（2d Task 2）"
```

---

### Task 3: Pipeline trait tick 签名 Vec + 删旧 inherent tick(bool) + 删 silence_duration/took_segment_cut + 清 allow(unused)

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（trait `tick` → Vec；删旧 inherent `tick(bool)` + trait `silence_duration`/`took_segment_cut` + 两 impl 对应；`tick_events` 改名 `tick`；清 `#[allow(unused)]`；删 `take_error` inherent）
- Modify: `crates/desktop/src/coordinator.rs`（`tick_events` 调用 → `tick`）

**目的：** 合并迁移——trait `tick` 签名收敛为 Vec，删旧 bool 版本与不再用的 trait 方法，清 `#[allow(unused)]`。

- [x] **Step 1: Pipeline trait tick 签名改 Vec + 删 silence_duration/took_segment_cut**

改 `Pipeline` trait（L95-117）：
- L95 的 `#[allow(unused)]` 改 `#[allow(dead_code)]`（coordinator 持具体类型走 inherent `tick`，trait `tick` 不经 trait 路径调用而 dead；trait 的 finish/reset/is_cloud 仍被用。详见 spec §3.7）。
- `tick` 签名 `-> bool` 改 `-> Vec<PipelineEvent>`。
- 删 `silence_duration`（L104-105）+ `took_segment_cut`（L114-116）。

改后 trait（保留 finish/reset/take_close_handle/is_cloud）：
```rust
pub trait Pipeline: Send {
    /// 喂一帧已降噪 16k 样本，返回本 tick 事件流（PersistRaw/Emit/Polish/Error）。
    /// coordinator `apply_pipeline_events` 据此执行 DB/emit/polish/错误上报。（2d）
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent>;
    /// 收尾：流式 flush（tail 已由 stop 路径的 tick 喂入 accept）；vad-seg 仅 drain 剩余 rx。
    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent;
    /// 重置（会话间复用）。
    fn reset(&mut self);
    /// cloud engine 的 async close 句柄（stop 时 coordinator 取出 spawn `close_async`）。
    /// local/vad-seg 返回 `None`（默认）。cfg cloud。
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> { None }
    /// 是否 cloud 引擎。vad-seg 恒 false。
    fn is_cloud(&self) -> bool { false }
}
```

- [x] **Step 2: StreamingPipeline 合并 tick_events → tick + 删旧 inherent tick(bool) + 删 take_error + 删 inherent silence_duration**

在 `StreamingPipeline`：
- 删旧 inherent `tick`（L197-218，返回 bool）。
- 把 Task 1 加的 inherent `tick_events` 改名为 `tick`（返回 Vec<PipelineEvent>）——方法体不变（已调 `self.tick` 处改为内联原 tick 的 set_full 逻辑，因为旧 tick 删了）。

改后 inherent `tick`（合并版，内联 set_full + 产事件）：
```rust
    /// 喂一帧已降噪 16k 样本：engine 产事件 → set_full，返回 tick 事件流（2d 合并）。
    /// - local：changed→[PersistRaw,Emit]；每 tick→[Polish]；空样本→[]（早退）
    /// - cloud：changed→[PersistRaw,Polish]；每 tick→[Emit{display+partial}]；error→[Error]
    pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        let is_cloud = self.engine.is_cloud();
        if !is_cloud && samples.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        for event in self.engine.tick(samples) {
            match event {
                TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                    if text != transcript.full() {
                        transcript.set_full(&text);
                        changed = true;
                    }
                }
                TranscriptEvent::Final(text) => {
                    transcript.set_full(&text);
                    changed = true;
                }
                TranscriptEvent::Error(e) => {
                    warn!("StreamingPipeline event error: {}", e);
                    self.last_error = Some(e);
                }
            }
        }
        let mut events = Vec::new();
        if is_cloud {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
            }
            if let Some(e) = self.last_error.take() {
                events.push(PipelineEvent::Error(e));
            }
            let base = transcript.display_text();
            let partial = self.engine.current_partial();
            let display = if partial.is_empty() { base } else { format!("{}{}", base, partial) };
            events.push(PipelineEvent::Emit { display });
        } else {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Emit { display: transcript.display_text() });
            }
            events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
        }
        events
    }
```

- 删 inherent `take_error`（L240-242，2d 后 coordinator 不再调，error 进事件流）。
- 删 inherent `silence_duration`（L230-232，2d 后无人调——dispatch_tick 从 Polish 事件读，stop 不用）。`current_partial`（L235）**保留**（cloud stop L739 取 partial 给 CloudClosing）。

- [x] **Step 3: StreamingPipeline 的 trait impl 适配（删 silence_duration，tick 转发 inherent）**

改 `impl Pipeline for StreamingPipeline`（L262-285）：
- `tick` 改转发 inherent：`fn tick(&mut self, samples, transcript) -> Vec<PipelineEvent> { self.tick(samples, transcript) }`。
- 删 trait `silence_duration`（L271-273）。
- `took_segment_cut` 无 impl（用默认 false）——无需删（本就没 impl）。
- 保留 finish/reset/take_close_handle/is_cloud。

```rust
impl Pipeline for StreamingPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        self.tick(samples, transcript) // 转发 inherent
    }
    fn finish(&mut self, _transcript: &mut Transcript) -> TranscriptEvent {
        self.engine.finish()
    }
    fn reset(&mut self) { self.engine.reset(); }
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
        self.engine.take_close_handle()
    }
    fn is_cloud(&self) -> bool { self.engine.is_cloud() }
}
```

- [x] **Step 4: VadSegmentedPipeline 合并 tick_events → tick（trait）+ 删 trait silence_duration/took_segment_cut**

`VadSegmentedPipeline`：删 Task 1 加的 inherent `tick_events`（pub(crate)），其逻辑搬进 trait `tick`。

改 `impl Pipeline for VadSegmentedPipeline`（L488-527）：
```rust
impl Pipeline for VadSegmentedPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        let changed = self.run_tick(samples, transcript);
        let segment_cut = self.segment_cut_this_tick;
        let mut events = Vec::new();
        if changed {
            events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
            events.push(PipelineEvent::Emit { display: transcript.display_text() });
        }
        if segment_cut {
            events.push(PipelineEvent::Polish { silence: f64::INFINITY });
        }
        events
    }

    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent {
        self.drain_rx_and_consume(transcript);
        TranscriptEvent::Committed(String::new())
    }

    fn reset(&mut self) {
        self.audio_buffer.clear();
        self.overlap_tail.clear();
        self.silence_duration = 0.0;
        self.has_speech = false;
        self.active_count = 0;
        self.next_seq = 0;
        self.completed_seq = 0;
        self.completed_results.clear();
        self.detect_vad.reset();
        self.filter_vad.reset();
        while self.rx.try_recv().is_ok() {}
        self.segment_cut_this_tick = false;
    }

    // take_close_handle / is_cloud 用默认（None / false）。
    // 删原 trait silence_duration / took_segment_cut（信息进 Polish 事件）。
}
```
> `silence_duration` 字段（struct L352）保留——`run_tick` 内部累加用（L440/444）。仅删 trait 方法。

- [x] **Step 5: coordinator 调用 tick_events → tick（改名跟随）**

`coordinator.rs` 的 `dispatch_tick`（Task 2）+ stop 路径（Task 2 Step 5）里的 `pipeline.tick_events(..)` 全改 `pipeline.tick(..)`：
- `dispatch_tick` 三 arm：`pipeline.tick_events(&samples, transcript)` → `pipeline.tick(&samples, transcript)`。
- stop 三处：`pipeline.tick_events(..)` → `pipeline.tick(..)`。

- [x] **Step 6: 既有 pipeline 测试适配（inherent tick 签名 Vec）**

`pipeline.rs` tests 里调 inherent `tick`（返回 bool）的测试改用新签名（返回 Vec）。受影响测试：
- `tick_partial_updates_transcript_and_signals_changed`（L568）：`let changed = p.tick(..)` → 改断言 events 非空 + transcript。
- `tick_final_overrides_transcript`（L581）：同。
- `tick_committed_idempotent_no_change_skip`（L596）：`assert!(!changed)` → `assert!(p.tick(..).is_empty())` 或断言只含 Polish。
- `tick_stashes_error_for_take_error`（L610）：take_error 已删——改为断言 `tick` 返回含 `Error` 事件。
- `finish_delegates_to_engine`（L635）：不动（finish 不变）。

具体改 `tick_partial_updates_transcript_and_signals_changed`：
```rust
    #[test]
    fn tick_partial_updates_transcript_and_signals_changed() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "", TranscriptEvent::Final("你好。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "你好");
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }
```
`tick_committed_idempotent_no_change_skip`：
```rust
    #[test]
    fn tick_committed_idempotent_no_change_skip() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".to_string())],
            "", TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("一样");
        let events = p.tick(&[0.0; 1600], &mut t);
        // changed=false → 只产 Polish（local 每 tick），无 PersistRaw/Emit
        assert!(!events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert_eq!(events, vec![PipelineEvent::Polish { silence: 0.0 }]);
    }
```
`tick_stashes_error_for_take_error` 改名为 `tick_error_produces_error_event`：
```rust
    #[test]
    fn tick_error_produces_error_event() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Error("boom".to_string())],
            "", TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Error(msg) if msg == "boom")));
    }
```
`tick_final_overrides_transcript`：
```rust
    #[test]
    fn tick_final_overrides_transcript() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Final("最终。".to_string())],
            "", TranscriptEvent::Final("最终。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("旧的");
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "最终。");
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }
```
> Task 1 加的 `tick_events_*` 测试（local/cloud）改方法名 `tick_events` → `tick`（逻辑不变，断言不变）。

- [x] **Step 7: 编译 + clippy + 测试**

Run: `cargo check -p octopus-desktop --all-targets --features cloud 2>&1 | tail -5`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --all-targets --features cloud 2>&1 | grep -E "^warning" | wc -l`
Expected: 0 新 warning（`#[allow(unused)]` 已清，无残留 unused）。

Run: `cargo test -p octopus-desktop pipeline::tests`
Expected: 全绿。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/pipeline.rs crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Pipeline trait tick 签名 Vec + 删 silence_duration/took_segment_cut（2d Task 3）"
```

---

### Task 4: 双 feature check + clippy + workspace 测试 + e2e 回归 + 文档同步 + ff-merge

**Files:**
- Verify: `crates/desktop/`（双 feature 编译 + 测试 + e2e）
- Modify: `docs/superpowers/specs/2026-06-25-coordinator-cleanup-design.md`（横幅状态）
- Modify: `docs/superpowers/plans/2026-06-25-coordinator-cleanup.md`（复选框）
- Modify: memory `parallel-workstreams.md`（item 7 的 2d 状态）

**目的：** 全量验证 + e2e 回归 + 文档同步 + ff-merge main。

- [x] **Step 1: 全量编译 + 测试矩阵**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo check --workspace --all-targets --features cloud 2>&1 | tail -5
cargo clippy --workspace --features cloud --all-targets 2>&1 | grep -E "^warning" | wc -l
cargo test --workspace 2>&1 | grep "test result"
```
Expected: 双 feature 0 error；clippy 无新 warning（与基线比）；workspace 测试全绿（除 2 个 pre-existing infra 失败 `seed_then_load_round_trips`/`list_all_local_asr_models_includes_disabled`——seed c796cbc 重写后断言过时，与本次无关，2d 未触碰 crates/infra/）。

- [x] **Step 2: 手动 e2e（事件流收敛后零行为差异回归）** — 通过（2026-06-25，用户本地验三路径零行为差异）

启动 desktop（`cargo tauri dev` 或既有启动方式），验证：

**streaming local（流式本地引擎，如 zipformer-streaming / qwen3-streaming）：**
1. 录音 → result window 增量显示 partial → finalize 后整句。
2. 停顿（≥pause_polish_threshold）→ 中间润色触发（mode=2）。
3. 停止 → finalize 粘贴（含润色）。

**streaming cloud（云端流式，cfg cloud）：**
4. 云端流式 → 每 tick emit（display + partial 预览）→ commit 后 DB + 润色。
5. WSS 错误（断网/坏 key）→ result window 显示错误（Error 事件上报）。
6. 停止 → close_async → finalize_cloud。

**VadSegmented（非流式本地引擎，如 moonshine / zipformer-non-streaming）：**
7. onset：「正在聆听…」→ 说话 → 段识别乱序回填按 seq 拼接。
8. 强制切段（≥20s）→ overlap 衔接连贯。
9. 停顿切段 → segment_cut 触发停顿润色（mode=2）。
10. stop WaitingCompletion → tick drain → active_count==0 → finalize（文本完整）。
11. 跨会话护栏：停止后立刻重开 → 旧会话迟到段不污染新会话。
12. Cancel/Discard → tick 停止、无泄漏、无迟到粘贴。

- [x] **Step 3: 同步 spec 横幅 + plan 复选框**

spec `docs/superpowers/specs/2026-06-25-coordinator-cleanup-design.md` 顶部状态行改：
```
> **状态**：✅ 已实施（待 ff-merge main）。Task 1-4 双 feature 编译 0 error、clippy 0 新 warning、workspace 测试除 2 pre-existing infra 外全绿；e2e 验证通过（2026-06-25）。
```
本 plan 所有 `- [x]` → `- [x]`。

- [x] **Step 4: Commit 文档**

```bash
git add docs/superpowers/specs/2026-06-25-coordinator-cleanup-design.md docs/superpowers/plans/2026-06-25-coordinator-cleanup.md
git commit -m "docs(spec/plan): 2d coordinator 清理自动化验证通过、状态同步"
```

- [x] **Step 5: 收尾（finishing-a-development-branch）** — ff-merge main（2026-06-25）

e2e 通过后，用 superpowers:finishing-a-development-branch 选 ff-merge main（对齐 2a/2b/2c-1/2c-2/2c-3 节奏）。合并后更新 memory `parallel-workstreams.md` item 7 的 2d 状态（2d 从「待」→「已 ff-merge main（SHA）」）。

---

## Self-Review

**1. Spec coverage：**
- §3.2 PipelineEvent → Task 1 Step 1。
- §3.3 tick 签名 Vec → Task 1（tick_events）+ Task 3（trait 合并）。
- §3.4 三路径事件序列 → Task 1 Step 2/3（streaming local/cloud + vad-seg）。
- §3.5 apply_pipeline_events + dispatch_tick → Task 2 Step 1/2。
- §3.6 边界（Stage 不变/finalize/cloud close/stop 丢弃事件）→ Task 2 Step 5（stop 丢弃）+ 全 task 不碰 finalize/cloud close。
- §3.7 trait 精简（删 silence_duration/took_segment_cut + 清 allow）→ Task 3 Step 1/3/4。
- §8 测试（pipeline 单测 + e2e）→ Task 1 Step 5 + Task 4 Step 2。
- §9 迁移映射 → Task 1-3 各步。
- **修正点**（plan 精确化 spec）：stop 路径丢弃事件（spec §3.5/§3.6 说复用 apply，plan 改为丢弃——现状 stop 无 DB/emit，丢弃保零行为差异）；current_partial 保留 pub（spec §3.7 说收回内部，plan 改为仅 take_error 收回——cloud stop L739 用 current_partial）。

**2. Placeholder scan：** 无 TBD/TODO；每步含确切代码或命令。

**3. Type consistency：** `PipelineEvent` 变体（PersistRaw{engine_mode:&'static str}/Emit{display:String}/Polish{silence:f64}/Error(String)）在 Task 1 定义、Task 1-3 测试与 impl 一致；`tick_events`（Task 1）→ `tick`（Task 3）改名贯穿；`dispatch_tick`/`apply_pipeline_events` 签名 Task 2 定义、Task 3 调用一致。
