# 流式 ASR 引擎复用设计（StreamingSessionManager）

> 日期：2026-07-06
> 分支：worktree-arch-fixes
> 关联：② 流式 ASR 优化（原「动静分离」立项，经核实降维为「引擎复用」）

---

## 1. 背景与问题

流式 ASR（`StreamingSession`）当前每次创建都重新加载 ONNX Session：

- **desktop**：每次开始录音 `coordinator.rs:811` 调 `StreamingSession::new(...)`，加载 encoder+decoder 两个 Session（磁盘读 + 图编译，秒级）。录音结束随局部变量 drop，引擎销毁，下次录音重复加载。`reset()`（coordinator.rs:1051/1617/1773）只用于**录音进行中**的轮次间冲刷，**跨录音不复用**。
- **server**：每个 WS 连接 `main.rs:230` new 一个，连接结束 drop。
- **cli**：不做流式录音（流式是 desktop/server 的事，cli 走离线 batch）。

痛点：**desktop 每次录音付秒级启动延迟**（加载 + 编译），体验差。离线（`OfflineAsrEngine`）无此问题——已由 `AsrEngineManager`（cached + active 两段式，启动预热常驻）解决。流式缺对等的管理层。

---

## 2. 关键约束（决定方案边界）

### 2.1 ort `Session::run` 是 `&mut self`

确证：`ort 2.0.0-rc.12/src/session/mod.rs:212`

```rust
pub fn run<'s, 'i, 'v: 'i, const N: usize>(&'s mut self, input_values: impl Into<SessionInputs<'i, 'v, N>>) -> Result<SessionOutputs<'s>>
```

→ Session 无法被多连接并发只读共享。**原设想「动静分离 → 多连接真并发」不成立**。

### 2.2 流式 `StreamingSession` 有连接级状态

`StreamingSession` enum 变体持：引擎（`StreamingParaformer`/`StreamingZipformer`）+ `punct_prefix`/`accumulated`/`committed_chars` 等连接级文本状态；引擎内还有 `decoder_caches`/`alpha_cache`/`feat_cache` 等解码状态。

→ 与离线无状态 `transcribe(&self, samples, lang)` 不同，多连接**不能共享同一个 `StreamingSession`**（状态污染）。

### 2.3 推论

「动静分离」作为字面手段（拆 Static/Dynamic struct）在 ort `&mut` 下**无并发收益**。真实可行的优化 = **进程级常驻引擎 + reset 复用**（避免每次重载），对齐离线 `AsrEngineManager` 的模式。

---

## 3. 目标

- **主要**：desktop 录音启动加速（秒级 → 毫秒级），通过进程级常驻 `StreamingSession` + `reset()` 复用。
- **次要**：架构与离线 `AsrEngineManager` 对称。
- **非目标**：server 大并发池化（server 是桌面端辅助，非服务端大并发）；字面动静字段拆分。

---

## 4. 方案选择

经 brainstorming 选定**方案 A**：建 `StreamingSessionManager`（对齐离线），仅 desktop 接入，server 保持现状。

否决项：
- **方案 B**（A + server 多实例池）：server 非大并发，池化 + 借出归还要处理状态隔离（每实例独立），复杂度高，过度设计。
- **方案 C**（desktop 硬编码常驻单例，不建 manager）：改动最小，但模型切换/多模型逻辑散落 desktop，与离线不对称、不可扩展。

---

## 5. 架构

### 5.1 `StreamingSessionManager`（新增，`crates/asr-local/src/streaming_engine.rs`）

```rust
pub struct StreamingSessionManager {
    cached: RwLock<HashMap<String, Arc<StreamingSession>>>,
    active_name: RwLock<String>,
}

impl StreamingSessionManager {
    pub fn new() -> Self { /* 空缓存 */ }

    /// 切换/加载模型：解析类型(Paraformer/ZipformerCtc/Transducer)
    /// → StreamingSession::new 加载 → 入缓存 → 设 active。
    pub fn switch_model(&self, spec: &str, language: &str) -> Result<()>;

    /// 取 active session 的 Arc clone。命中缓存直接返回；未命中
    /// (active_name≠spec 或缓存空) 则先 switch_model(spec, lang) 懒加载再返回。
    pub fn active_session(&self, spec: &str, language: &str) -> Result<Arc<StreamingSession>>;

    pub fn active_name(&self) -> String;
}
```

照搬离线 `AsrEngineManager` 两段式（cached + active）。差异：
- 离线缓存 `Arc<dyn OfflineAsrEngine>`（无状态可共享并发，引擎内 `Mutex<Session>` 串行 run）。
- 流式缓存 `Arc<StreamingSession>`（有连接级状态，靠 **reset 复用** 而非并发共享）。

### 5.2 复用机制（核心）

`StreamingSession` 方法全 `&self`（内部 `Mutex`），故 `Arc<StreamingSession>` 可被 manager 与 pipeline 同时持有：

- 录音开始：manager 给 pipeline 一个 Arc clone + `reset()` 清状态
- 录音结束：`StreamingPipeline` drop → 仅释放 pipeline 那个 Arc clone；manager 原 Arc 仍在 → **引擎不销毁**
- 下次录音：`active_session()` 取同一 Arc + `reset()`（毫秒级，不 new）

`StreamingParaformer::reset()`（`streaming_paraformer.rs:266-287`）已逐字段验证清干净（`raw_samples`/`fbank_cache`/`feat_cache`/`encoder_out_cache`/`alpha_cache`/`decoder_caches`/`num_processed_frames`/`all_token_ids`/`last_emitted_token`/`fresh_segment` 全归零/清空，`decoder_caches` 复用内存）。复用前提满足。

### 5.3 配套改动：`Box → Arc`（让 Arc 复用成立）

| 位置 | 改动 | 文件 |
|---|---|---|
| `StreamingRunner.engine` | `Box<dyn StreamingEngine>` → `Arc<dyn StreamingEngine>` | `asr-local/streaming_runner.rs:174` |
| `StreamingRunner::new` | 接 `Arc<dyn StreamingEngine>` | `asr-local/streaming_runner.rs:191` |
| `LocalPipelineEngine::from_session` | 接 `Arc<StreamingSession>` 而非 owned `StreamingSession` | `desktop/pipeline.rs:146` |

**cloud 不受影响**：`CloudPipelineEngine`（`coordinator.rs:765`）独立 impl `StreamingPipelineEngine`，不经 `StreamingRunner`/`StreamingSession`。

---

## 6. 数据流（desktop 录音）

```
启动:
  main: Arc::new(StreamingSessionManager::new())  // 空缓存，不预热（流式按需）
       → app.manage(State)                        // 对齐离线 main.rs:355/415

首次录音 (coordinator.rs:811 改造):
  session = manager.active_session(&config.asr_engine, &config.language)?
      // 未命中 → switch_model(new) 加载+入缓存 → 返回 Arc
  session.reset()
  LocalPipelineEngine::from_session(session, false)  // Arc clone 进 StreamingRunner

后续录音 (同模型):
  session = manager.active_session(...)  // 命中，Arc clone，不 new
  session.reset()                        // 毫秒级
  → 启动秒级 → 毫秒级

模型变更 (switch_asr_engine 命令联动):
  streaming_manager.switch_model(new_spec, lang)  // 加载新模型入缓存，设 active

录音结束 (Stage 转换):
  StreamingPipeline drop → Arc clone 释放 → manager 保留引擎
```

`active_session(spec, lang)` 懒加载语义（命中返回 Arc / 未命中 switch_model 再返回），让 coordinator 单行调用，无需关心首次/复用——对齐离线 `get_engine` 的取用风格。

---

## 7. 错误处理与边界

| 场景 | 处理 |
|---|---|
| switch_model 失败（模型缺失/加载错） | 返回 `Err` → coordinator 保留现有 `FALLBACK_STREAMING_SPEC` 降级链（`coordinator.rs:811-829` 不变） |
| reset | 无 `Result`（纯清零），不会失败 |
| reset 完整性风险 | Paraformer 已逐字段验证；**ZipformerCtc/Transducer 的底层 `StreamingZipformer::reset` 待实现时核实**（不干净则补全——复用正确性的硬前提） |
| 并发 | desktop 单录音串行；manager 内 `RwLock` 保护 cached/active；Arc clone 非并发使用 |
| server | **不接入**（per-connection `new` 不变，状态隔离 + 非大并发） |

---

## 8. 测试

- **manager 单测**：`active_session` 命中/未命中、重复调用返回同一 Arc（加载计数器断言「只 new 一次」）、模型切换 active 跟随。
- **回归**：`StreamingRunner` 现有测试（`streaming_runner.rs:331+`）改 Box→Arc 后调整 `FakeStreamingEngine` 构造仍绿；`from_session` 接 Arc。
- **desktop e2e**：连录两次，第二次启动延迟大降（日志/手测）。

---

## 9. 范围

**改**：
- `asr-local`：`streaming_engine` 新增 `StreamingSessionManager`、`streaming_runner` Box→Arc。
- `desktop`：`pipeline.rs` from_session、`coordinator` 创建时机、`main` 注入、`switch_asr_engine` 联动。

**不改**：server、cloud（独立路径）、离线 `AsrEngineManager`。

**YAGNI 不做**：动静字段拆 struct（ort `&mut` 下无并发收益）、server 池化、r2d2。

---

## 10. 待实现时确认

- `StreamingZipformer`/`StreamingZipformerTransducer` 的 `reset()` 完整性（不干净则补全）。
- `switch_asr_engine` 命令当前如何联动离线 `engine_manager`，照搬对流式 `streaming_manager`。
- `StreamingSessionManager` 是否需要缓存上限（流式模型种类少，可不设或宽松；离线 desktop 默认 2）。
