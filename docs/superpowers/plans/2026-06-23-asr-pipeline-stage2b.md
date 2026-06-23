# ASR Pipeline 阶段2b：desktop 本地流式路径迁移实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development 或 superpowers:executing-plans 按任务实施。Steps 用 checkbox（`- [ ]`）跟踪。

**Goal:** desktop 本地流式路径迁移——coordinator `Stage::Streaming` 委托 `asr::StreamingRunner`（阶段2a 交付），`handle_streaming_tick` 改为消费 `TranscriptEvent`，stop 路径用 `finish_with_tail`。**运行时行为不变**（逐字等价迁移）。

**Architecture:** 2b 只迁本地流式（`Stage::Streaming`）。cloud（`CloudStreaming`）、`VadSegmented`、`StreamingPipeline` 抽象**留 2c/2d**——2b 让 coordinator 直接持 `StreamingRunner`（单路径无需抽象）。asr 小增量：`StreamingRunner` 补 `preroll_vad`（搬自 coordinator，补齐 2a 遗漏的 VAD 预热）+ `finish_with_tail`（stop 收尾，精确等价原 `accept(tail)+finish`）。coordinator `Stage::Streaming` 四字段（`engine/vad/silence_duration/flushed`）合并为 `runner`，保留 `transcript`+`streaming_active`。

**Tech Stack:** Rust、`octopus_asr::streaming_runner::{StreamingRunner, StreamingEngine, TranscriptEvent}`、`octopus_asr::streaming_engine::StreamingSession`、Tauri、`Transcript`。

---

## 设计要点（务必读完再动）

1. **行为不变铁律**：2b 是搬迁不是重写。`handle_streaming_tick` 的幂等去重（`text != transcript.full()`）、DB 写、emit、`check_and_trigger_polish` 全保留；只是 ASR 编排（VAD+标点+accept/flush）从 coordinator 内联代码换成 `runner.push_samples`。
2. **VAD 预热补齐**：coordinator 创建 VAD 后调 `vad_preroll`（静音帧 ×10 预热 LSTM，`coordinator.rs:1468`）。阶段2a `StreamingRunner::new` 未预热——2b 把 `preroll_vad` 搬进 runner，`new` 内调用，**与原行为等价**。coordinator 的 `vad_preroll`/`VAD_PREROLL_FRAMES` 保留（`VadSegmented` 仍用，2c 再议）。
3. **stop 路径精确等价**：原 stop（`coordinator.rs:864-884`）= `accept_samples(tail,false)` + `finish()` + `reset()`，**不**走 VAD/flush。`finish_with_tail` 封装此顺序，避免 `push_samples`（会 VAD/flush）引入多余标点。
4. **`StreamingPipeline` 不在 2b 引入**：单本地路径直接持 runner；cloud 接入（2c）再抽 `StreamingPipeline` 统一 local/cloud 分发。
5. **无桌面单测**：coordinator 改动靠 `cargo check --workspace --all-targets` + clippy + **手动 e2e 清单**（Task 6）验证。asr 新方法（`finish_with_tail`/`preroll`）有单测。

---

## File Structure

- **Modify:** `crates/asr/src/streaming_runner.rs` —— 加常量 `VAD_PREROLL_FRAMES`、私有 `preroll_vad`、`new` 内调预热、`pub fn finish_with_tail` + 单测。
- **Modify:** `crates/desktop/src/coordinator.rs` —— `Stage::Streaming` 字段重构、`handle_toggle` use_streaming 创建 runner、`handle_streaming_tick` 重写、stop 路径（`Stage::Streaming` 非 Idle 分支）用 runner。
- **不动：** `crates/desktop/src/audio.rs`（denoise/resample 留，drain_samples 返回 16k 降噪样本）、`transcript.rs`、`VadSegmented`/`CloudStreaming` 路径（2c）。

---

## Task 1: asr `StreamingRunner` 补 `preroll_vad` + `finish_with_tail`

**Files:**
- Modify: `crates/asr/src/streaming_runner.rs`

- [x] **Step 1: 加 `VAD_PREROLL_FRAMES` 常量 + `preroll_vad` 私有函数**

在 `streaming_runner.rs` 现有常量块（`PUNCTUATION_SILENCE_THRESHOLD` 后）追加：

```rust
/// VAD LSTM 预热帧数（搬自 `coordinator.rs:VAD_PREROLL_FRAMES`）。
const VAD_PREROLL_FRAMES: usize = 10;

/// VAD 预热：喂静音帧让 Silero LSTM 状态稳定（搬自 `coordinator.rs:vad_preroll`）。
/// 未预热时开头几帧 prob 偏高/偏低，导致标点检测开头不准。
fn preroll_vad(vad: &mut SileroVad) {
    let silence = vec![0.0_f32; VAD_CHUNK_SIZE];
    for _ in 0..VAD_PREROLL_FRAMES {
        let _ = vad.compute(&silence);
    }
}
```

- [x] **Step 2: `new` 内 VAD 构造后调 `preroll_vad`**

把 `StreamingRunner::new` 的 VAD 构造从：

```rust
        let vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
```

改为：

```rust
        let mut vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
        if let Some(v) = vad.as_mut() {
            preroll_vad(v);
        }
```

- [x] **Step 3: 加 `finish_with_tail` 方法**

在 `impl StreamingRunner` 的 `finish` 方法后追加：

```rust
    /// 收尾并先吃入尾部样本（stop 路径用）。
    ///
    /// 精确等价 `coordinator.rs:864-881` 的 stop 顺序：`engine.accept_samples(tail, false)`
    /// （**不**走 VAD/flush，`was_silent=false` 不插逗号）→ `engine.finish()`。与 [`push_samples`]
    /// 的区别：push_samples 会 VAD 检测 + 静音冲刷标点，stop 尾部不应触发标点。
    pub fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        if !tail.is_empty() {
            if let Err(e) = self.engine.accept_samples(tail, false) {
                log::warn!("StreamingRunner finish_with_tail accept error: {e}");
            }
        }
        self.finish()
    }
```

- [x] **Step 4: 加单测**

在 `mod tests` 末尾（最后一个 `#[test]` 后、`}` 闭合前）追加：

```rust
    #[test]
    fn finish_with_tail_emits_final() {
        // accept 队列给 1 个（tail 吃入），finish 返回固定串
        let mut r = runner(FakeStreamingEngine::new(vec!["尾"], vec![], "最终。"));
        let ev = r.finish_with_tail(&[0.0; 512]);
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }

    #[test]
    fn finish_with_tail_empty_tail_still_finishes() {
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], "空尾。"));
        let ev = r.finish_with_tail(&[]);
        assert_eq!(ev, TranscriptEvent::Final("空尾。".to_string()));
    }
```

> 注：`finish_with_tail` 内部 `accept_samples(tail,false)` 会消耗 accept_out 队列 1 项；`finish_with_tail_empty_tail_still_finishes` 传空 tail → 不调 accept → 队列不消耗（FakeStreamingEngine::new(vec![],…) 的 accept_out 本就空，finish 直接返回）。`finish_with_tail_emits_final` 传 `[0.0;512]` → 调 accept → 消耗 `"尾"` → finish 返回 `"最终。"`。

- [x] **Step 5: 验证 asr**

Run: `cargo test -p octopus-asr streaming_runner`
Expected: 原 7 个 + 新增 2 个 = 9 个全过。

Run: `cargo clippy -p octopus-asr --all-targets 2>&1 | grep streaming_runner`
Expected: 无输出（无新 warning）。

- [x] **Step 6: 暂不提交（与 Task 2-5 合并提交，或单独提交 asr 增量）**

> 推荐单独提交 asr 增量（Task 1），再提交 coordinator 迁移（Task 2-5），便于回滚定位。

---

## Task 2: coordinator `Stage::Streaming` 字段重构

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: 改 `Stage::Streaming` 变体字段**

找到 `Stage::Streaming` 枚举变体（约 67-180 区间的 enum 定义），把：

```rust
    Streaming {
        engine: StreamingSession,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
        vad: Option<octopus_asr::vad::SileroVad>,
        silence_duration: f64,
        flushed: bool,
    },
```

改为：

```rust
    Streaming {
        /// 流式编排 runner（持 StreamingSession + VAD + 静音/标点状态，阶段2a）。
        runner: octopus_asr::streaming_runner::StreamingRunner,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
```

- [x] **Step 2: 加 import**

coordinator.rs 顶部 `use` 区，在 `StreamingSession` 相关 import 附近加（若无则加）：

```rust
use octopus_asr::streaming_runner::{StreamingRunner, TranscriptEvent};
```

> 若 coordinator 用 `use octopus_asr::streaming_engine::StreamingSession;`，保留（handle_toggle 创建仍用）。

- [x] **Step 3: 验证编译（预期大量错误，Task 3-5 修复）**

Run: `cargo check -p octopus-desktop`
Expected: 报错集中在 `handle_toggle` 创建点、`handle_streaming_tick`、stop 路径（引用了已删字段 `engine/vad/silence_duration/flushed`）。这是预期的，下面 Task 3-5 逐一修复。

---

## Task 3: `handle_toggle` use_streaming 创建 runner

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（use_streaming 分支，约 670-736）

- [x] **Step 1: 删原 VAD 创建块，改建 runner**

原代码（约 708-736）：

```rust
                // 初始化 VAD（用于静音检测 + 标点）
                let vad = match octopus_asr::config::find_silero_vad() {
                    Ok(path) => match octopus_asr::vad::SileroVad::new(&path) {
                        Ok(mut v) => {
                            vad_preroll(&mut v);
                            Some(v)
                        }
                        Err(e) => {
                            warn!("VAD init failed: {}, punctuation disabled", e);
                            None
                        }
                    },
                    Err(e) => {
                        warn!("VAD not found: {}, punctuation disabled", e);
                        None
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    engine: streaming_engine,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                    vad,
                    silence_duration: 0.0,
                    flushed: false,
                };
```

改为（VAD + preroll 由 runner 内部处理；`streaming_engine` 创建不变）：

```rust
                // VAD + 预热由 StreamingRunner 内部处理（阶段2a/2b）
                let runner = match StreamingRunner::new(Box::new(streaming_engine), false) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("StreamingRunner init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(
                            app_handle,
                            crate::tray::TrayState::Idle,
                        );
                        return;
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    runner,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                };
```

> `correct=false`：desktop 流式现无纠错（与原行为一致，hook 预留）。

- [x] **Step 2: 验证此分支编译**

Run: `cargo check -p octopus-desktop`
Expected: handle_toggle 创建点不再报错；剩余报错在 `handle_streaming_tick` + stop 路径（Task 4-5）。

---

## Task 4: `handle_streaming_tick` 重写委托 runner

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_streaming_tick`，约 1968-2037）

- [x] **Step 1: 整体替换 `handle_streaming_tick` 函数体**

原函数（1968-2037，含内联 detect/accept/flush）整体替换为：

```rust
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let Stage::Streaming {
        runner,
        transcript,
        ..
    } = stage
    else {
        return;
    };

    let samples = audio.drain_samples();
    if samples.is_empty() {
        return;
    }

    // ASR 编排（VAD 静音 + 标点 + accept/flush）委托 runner（阶段2a）
    for event in runner.push_samples(&samples) {
        match event {
            TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                // 幂等：内容未变不重绘（消除静音期/同文本反复 update 闪烁 + 无谓 DB 写）
                if text != transcript.full() {
                    transcript.set_full(&text);
                    if let Err(e) =
                        update_transcription_raw(transcript, &config.asr_engine, "streaming")
                    {
                        warn!("DB (streaming) failed: {}", e);
                    }
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
            }
            TranscriptEvent::Final(_) => {
                // Final 只在 stop 路径产生（finish），tick 不应收到；防御性忽略
                debug!("Streaming tick got unexpected Final event, ignored");
            }
            TranscriptEvent::Error(e) => warn!("Streaming event error: {}", e),
        }
    }

    // 停顿润色（留端，spec §3.8）
    check_and_trigger_polish(transcript, runner.silence_duration(), config, tx);
}
```

> 行为等价原 `handle_streaming_tick`：accept→Partial 与 flush→Committed 都走同一幂等 `set_full+DB+emit`（原代码两条分支逻辑完全一致，合并）；`check_and_trigger_polish` 用 `runner.silence_duration()` 取代原 `*silence_duration`。

- [x] **Step 2: 删已无用的内联 VAD helper（若仅 Streaming 用）**

检查 `detect_silence_gap`（原 2045-2099）的调用方。`grep -n detect_silence_gap crates/desktop/src/coordinator.rs`：
- 若仅 `handle_streaming_tick`（已删）调用 → **删除** `detect_silence_gap` 函数（逻辑已迁 asr `streaming_runner::detect_silence_gap`）。
- 若 `VadSegmented`/cloud 也调 → 保留。

`compute_speech_chunks`（1444）同理检查：被 `handle_vad_segmented_tick`/`handle_cloud_streaming_tick` 调用 → **保留**（2c 才动）。

`VAD_CHUNK_SIZE`/`VAD_SPEECH_THRESHOLD`/`PUNCTUATION_SILENCE_THRESHOLD` 常量：检查是否仍被 coordinator 其他处用（`detect_silence_gap` 删后可能 unused）→ 仅当确认无其他引用才删，否则保留。

- [x] **Step 3: 验证编译**

Run: `cargo check -p octopus-desktop`
Expected: `handle_streaming_tick` 不再报错；剩余报错在 stop 路径（Task 5）。

---

## Task 5: stop 路径用 `runner.finish_with_tail`

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_toggle` 的 `Stage::Streaming` 非 Idle 分支，约 852-897）

- [x] **Step 1: 替换 stop 分支**

原代码（852-897）：

```rust
        Stage::Streaming {
            engine: streaming_engine,
            transcript,
            streaming_active,
            ..
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            if !final_samples.is_empty() {
                if let Err(e) = streaming_engine.accept_samples(&final_samples, false) {
                    warn!("Error processing final samples: {}", e);
                }
            }
            let final_text = match streaming_engine.finish() {
                Ok(text) => text,
                Err(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
            };
            streaming_engine.reset();
            let _ = audio.stop();
            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

改为：

```rust
        Stage::Streaming {
            runner,
            transcript,
            streaming_active,
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            // 尾部样本 + finish（精确等价原 accept(tail,false)+finish；不走 VAD/标点）
            let final_text = match runner.finish_with_tail(&final_samples) {
                TranscriptEvent::Final(text) => text,
                TranscriptEvent::Error(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
                _ => transcript.edited_display().unwrap_or_else(|| transcript.db_text()),
            };
            runner.reset();
            let _ = audio.stop();
            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

> 行为等价：`finish_with_tail` 内部 `accept(tail,false)+finish`；Error 兜底 `edited_display||db_text` 与原 `finish()` Err 分支一致。

- [x] **Step 2: 检查 `StreamingSession` import 是否仍需**

`grep -n "StreamingSession" crates/desktop/src/coordinator.rs`：handle_toggle use_streaming 仍用 `StreamingSession::new` → **保留** import。

- [x] **Step 3: 全量编译**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。若仍有引用已删字段（`engine/vad/silence_duration/flushed` on `Stage::Streaming`），按报错逐一改（应已无）。

---

## Task 6: 验证 + 提交

- [x] **Step 1: workspace check + clippy**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -E "streaming|StreamingRunner|error" | head`
Expected: 无新增 streaming 相关 warning（desktop 既存 warning 维持）。若有 `unused import`/`dead_code`（如删 detect_silence_gap 后残留常量），按提示清理。

Run: `cargo clippy -p octopus-asr --all-targets 2>&1 | grep streaming_runner`
Expected: 无输出。

- [x] **Step 2: asr + cli 回归（不应受影响）**

Run: `cargo test -p octopus-asr`
Expected: 83 tests（原 81 + Task 1 新增 2）全过（75+2 pass + 6 ignored）。

Run: `cargo test -p octopus-cli`
Expected: 4 tests 全过。

- [x] **Step 3: desktop 构建（确认 Tauri 链接无误）**

Run: `cargo build -p octopus-desktop`
Expected: 0 error（desktop 无单测，靠 build + 手动 e2e）。

- [ ] **Step 4: 手动 e2e 清单（行为不变验证）**（代码完成，待用户本地 e2e）

本地运行 desktop（`cargo tauri dev` 或既有启动方式），逐项验证本地流式（非 cloud、非 VadSegmented）：

- [ ] 开录音（use_streaming 配置）→ result window 显示「正在聆听…」
- [ ] 说一句中文 → 实时增量文本出现（Partial）
- [ ] 停顿 >0.5s → 文本插入逗号（Committed，VAD 标点）
- [ ] 继续说 → 新增文本，逗号标点正常（验证 preroll 后 VAD 标点开头不偏）
- [ ] 停录音（toggle off）→ 追加句号 + 走润色/粘贴（Final，stop 路径）
- [ ] DB（`~/.octopus/`）有 streaming 记录、文本正确
- [ ] 静音期无闪烁（幂等去重生效）

> 若本地无法 e2e，至少完成 Step 1-3 并在提交信息标注「e2e 待本地验证」。

- [x] **Step 5: 提交**

asr 增量（Task 1）：

```bash
git add crates/asr/src/streaming_runner.rs
git commit -m "feat(asr): StreamingRunner 补 VAD 预热 + finish_with_tail（阶段2b 接线）

- preroll_vad 搬自 coordinator（补 2a 遗漏的 LSTM 预热，VAD_PREROLL_FRAMES=10）
- new() 内构造 VAD 后预热，与 desktop 原行为等价
- finish_with_tail(tail)：accept(tail,false)+finish，供 desktop stop 收尾
  （精确等价原 stop 顺序，不走 VAD/标点）
- 2 个新单测（finish_with_tail 有/无 tail）"
```

coordinator 迁移（Task 2-5）：

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Stage::Streaming 委托 StreamingRunner（阶段2b）

本地流式路径迁移，运行时行为不变：
- Stage::Streaming {engine,vad,silence_duration,flushed} → {runner,transcript,streaming_active}
- handle_streaming_tick 改消费 TranscriptEvent（Partial/Committed 幂等 set_full+DB+emit）
- stop 路径用 runner.finish_with_tail（精确等价 accept(tail)+finish+reset）
- handle_toggle 创建 StreamingRunner（VAD+preroll 由 runner 内部）
- 删已迁 asr 的 detect_silence_gap（仅 Streaming 用时）

cloud/VadSegmented/StreamingPipeline 抽象留 2c/2d。e2e 清单见
docs/superpowers/plans/2026-06-23-asr-pipeline-stage2b.md。"
```

> 分类器提示：`git add` 与 `git commit` 分两行（换行分隔，**不要** `&&`）。

---

## Self-Review（writing-plans 自检）

**1. Spec 覆盖：** spec §3.4 desktop `StreamingPipeline` → 2b 设计要点 §4 明确**留 2c**（单路径不抽象），coordinator 直接持 runner；§3.3 `StreamingRunner` 接入 → Task 3-5；§3.8 润色留端 → Task 4 `check_and_trigger_polish` 保留。denoise 留 audio.rs（2a 设计调整延续）→ 未动 audio.rs。✅

**2. 占位符扫描：** 无 TBD/TODO。Task 4 Step 2 的「检查 detect_silence_gap 调用方」是条件性删除（依赖 grep 结果），给出了两种分支的处理，非占位。Task 6 Step 4 e2e 清单是验证项不是实现占位。✅

**3. 类型一致性：** `StreamingRunner::new(Box<dyn StreamingEngine>, bool)`（2a）← Task 3 `StreamingRunner::new(Box::new(streaming_engine), false)` 一致；`finish_with_tail(&[f32]) -> TranscriptEvent`（Task 1 定义）← Task 5 调用一致；`push_samples(&[f32]) -> Vec<TranscriptEvent>` + `silence_duration() -> f64`（2a）← Task 4 调用一致；`TranscriptEvent::{Partial,Committed,Final,Error}`（2a）← Task 4/5 match 一致。✅

**4. 行为不变性：** Task 1 preroll 补齐 2a 遗漏（与 coordinator 原 vad_preroll 等价）；Task 4 合并 accept/flush 两条幂等分支（原代码两条分支 set_full+DB+emit 逻辑完全相同）；Task 5 finish_with_tail 精确等价 accept(tail,false)+finish+reset。无单测靠 e2e 清单（Task 6 Step 4）+ 编译/clippy 兜底。✅
