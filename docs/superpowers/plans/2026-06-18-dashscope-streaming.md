# DashScope 云端流式 ASR 实施计划

> **状态：✅ 已实现**（2026-06-18）
>
> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 或 superpowers:executing-plans 逐任务实现。

**Goal:** 实现 VAD-gated per-utterance streaming——VAD 检测语音 onset → 开 WSS 长连接推 PCM 收 partial → 静音 ≥ 700ms 断开。

**Architecture:** 新建 `dashscope_stream.rs`（`DashScopeStreamSession`），coordinator 新增 `Stage::CloudStreaming` + `CloudStreamingTick`。详见 spec `docs/superpowers/specs/2026-06-18-dashscope-streaming-design.md`。

**Tech Stack:** Rust + Tauri；tokio-tungstenite（WS）；tokio::select!（双向异步循环）。

---

## File Structure

- **新建：** `crates/desktop/src/dashscope_stream.rs`
- **改：** `crates/desktop/src/coordinator.rs`（Stage + Command + handler + toggle routing）
- **改：** `crates/desktop/src/main.rs`（注册模块）
- **改：** `crates/desktop/src/engine_dashscope.rs`（`samples_to_pcm_s16le` 改 `pub(crate)`）

---

## Task 1: `dashscope_stream.rs` — DashScopeStreamSession  ✅

**文件：** `crates/desktop/src/dashscope_stream.rs`（新建）

实现 `DashScopeStreamSession`：
- `open(rt, endpoint, key, model, language, pre_roll_samples) -> Result<Self>`
  - 在 tokio runtime 上 spawn `run_ws_session`
  - 两条 unbounded channel：PCM（coordinator→sender）、result（reader→coordinator）
- `push_pcm(&[f32]) -> Result<()>`：非阻塞
- `try_recv_text() -> Option<StreamEvent>`：非阻塞
- `close(self, rt) -> Result<String>`：发 Finish + 阻塞等最终结果

`run_ws_session` async fn：
- 建连（`connect_async` + bearer header）
- 发 run-task（含 `max_sentence_silence: 600`）
- 推 pre-roll PCM
- `tokio::select!` 双向循环：
  - `pcm_rx.recv()` → send binary / finish-task
  - `ws.next()` → parse result-generated / task-finished / task-failed

`StreamEvent` enum：`Text(String)` / `Finished` / `Failed(String)`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 2: coordinator — Stage + Command + 常量  ✅

**文件：** `crates/desktop/src/coordinator.rs`

1. 新增 `Command::CloudStreamingTick`（`#[cfg(feature = "dashscope")]`）
2. 新增 `Stage::CloudStreaming` 变体（`#[cfg(feature = "dashscope")]`）：
   - `vad: SileroVad`（检测用，有状态累积）
   - `session: Option<DashScopeStreamSession>`（活跃 WSS）
   - `pre_roll_buffer: Vec<f32>`（滚动窗口 200ms）
   - `transcript: Transcript`
   - `silence_duration: f64`
   - `is_speaking: bool`
   - `tick_active: Arc<AtomicBool>`
3. 新增常量：`CLOUD_STREAMING_TICK_INTERVAL_MS=100` / `CLOUD_PREROLL_BUFFER_SAMPLES=3200` / `CLOUD_PREROLL_SAMPLES=1600`
4. 新增 `is_cloud_engine(&AppConfig) -> bool`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 3: coordinator — Toggle 路由 + Tick 线程  ✅

**文件：** `crates/desktop/src/coordinator.rs`

1. Toggle Idle → CloudStreaming 分支：
   - 创建 VAD + pre-roll
   - `start_cloud_streaming_tick_thread`（tick → sleep，首 tick 立即触发）
2. `handle_toggle` 签名加 `use_cloud_streaming` 参数
3. command dispatch 加 `Command::CloudStreamingTick` → `handle_cloud_streaming_tick`
4. Toggle 停止（CloudStreaming → WaitingCompletion/Pasting）：
   - 停 tick + audio.stop
   - close WSS（如有）→ 拼接最终文本
   - 进入 Pasting

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 4: coordinator — `handle_cloud_streaming_tick`  ✅

**文件：** `crates/desktop/src/coordinator.rs`

实现 tick handler 逻辑：
1. `drain_samples()` → 追加到 pre_roll_buffer（超容量弹头）
2. VAD 检测 → `compute_speech_chunks`
3. 语音检测（≥2 chunks）→ `silence_duration=0` / 非语音 → `silence_duration += tick`
4. 无活跃 WSS + onset → 解析 DB（endpoint+key+model）→ `session.open()` + pre-roll + push PCM
5. 有活跃 WSS → push PCM + `try_recv_text` 更新 transcript + UI
6. 有活跃 WSS + silence ≥ pause_polish_threshold_ms → `close()` + 拼接文本 + 触发润色 + `session=None`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 5: main.rs 注册 + engine_dashscope pub  ✅

**文件：** `crates/desktop/src/main.rs` + `crates/desktop/src/engine_dashscope.rs`

1. `main.rs` 加 `mod dashscope_stream;`（`#[cfg(feature = "dashscope")]`）
2. `engine_dashscope.rs`：`samples_to_pcm_s16le` 改 `pub(crate)`
3. 编译验证 + 测试

**验证：** `cargo test -p octopus-desktop --features "embedded dashscope"`

---

## Task 6: 文档同步 + 提交  ✅

- `docs/architecture.md`：补 CloudStreaming stage 说明
- spec/plan 标完成
- commit + merge
