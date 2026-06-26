# ASR Pipeline 阶段3：server 迁移设计

> **关联总 spec**：`docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`（§3.4 / L119 / L68 / L93 / L206）
> **阶段定位**：阶段1（`asr::pipeline` / `transcribe_batch` / cli 已迁）+ 阶段2（desktop `StreamingRunner` / `PipelineEvent`，2a-2d 全 ff-merge main）已完成。**阶段3 = server 端迁移**，即总 spec §7「本次不迁 server」中「本次」=阶段1/2，阶段3 正是本设计补齐的 server 占位。
> **范围决策（用户 2026-06-25）**：流式 + 批处理**两端点都迁**；**不接 cloud**（server 仅 local `StreamingSession`）；WS 回推改 `TranscriptEvent` JSON（server 无外部客户端，破坏旧 `{text,final}` 协议可接受）。

---

## 1. 背景与现状

`crates/server/src/main.rs`（407 行单文件）暴露两条 ASR 路径，**均未对齐阶段1/2 的 asr helper**：

- **流式 `WS /ws/stream`**（`handle_ws` L221-367）：裸调 `StreamingSession::new(&engine)` + **手搓** `SileroVad` / `detect_silence_gap_local`（L175-219，512 chunk / 0.5 阈值 / 0.5s 静音）+ 手写 `accept_samples`/`flush`/`finish` 循环 + **手拼** `{text,final}` / `{error}` JSON。正是总 spec §2.2「流式绕过 trait，裸调 StreamingSession」反模式。
- **批处理 `POST /transcribe`**（`transcribe` L86）：走 `AsrEngineManager.transcribe(&samples, language)`（旧路径），**非**阶段1 的 `transcribe_batch`（cli 已用）。

阶段2 已在 desktop 验证 `StreamingRunner`（`crates/asr/src/streaming_runner.rs`）收编了 VAD 静音 + 标点触发 + engine accept/flush/finish 的纯 ASR 编排。server 阶段3 = 用这套验证过的抽象替换 server 的裸调与手搓，使 cli/desktop/server 三端统一走 asr helper。

## 2. 范围

**迁**：
- 流式 `/ws/stream` → `StreamingRunner`（消除手搓 VAD + 手拼 JSON + 裸 `StreamingSession`）
- 批处理 `/transcribe` → `AsrEngineManager::transcribe_batch` + `PipelineConfig`（对齐 cli：VAD 分段 + 纠错 + 简繁归一化）

**不接 cloud**：server 作 WS server 再串一层 cloud WSS（bytedance/tencent 等）角色怪异，复杂度高，YAGNI。server 仅 local `StreamingSession`。

**不动**（边界，见 §8）：
- **polish 不进 server**（总 spec §3.8：润色留端，server 不依赖 `octopus_llm`；server 只到 `TranscriptEvent`）
- **denoise/resample 不进 server**（总 spec §3.6 + §12 调整：紧耦合 cpal 采集，server 无 cpal；server 信任客户端发送的 16k PCM）

## 3. 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/server/src/pipeline.rs`（**新建**） | WS↔`StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化 | 新建 `WsStreamSession` + `event_to_json` |
| `crates/server/src/main.rs` | axum 路由 + WS/HTTP 胶水 | `handle_ws` / `transcribe` 迁移；**删** `detect_silence_gap_local`（手搓 VAD）、裸 `StreamingSession`、手拼 `{text,final}` |

`main.rs` 预计从 407 行瘦身到 ~280 行（删 ~130 行手搓 VAD + 手拼 JSON）。职责清晰：`pipeline.rs` = WS 与 runner 之间的桥接（纯逻辑，可单测）；`main.rs` = 路由与网络胶水。与 cli/desktop 各有 `pipeline.rs` 的三端结构一致。

## 4. 组件

### 4.1 `WsStreamSession`（`server/src/pipeline.rs`）

薄包 asr `StreamingRunner`，对外暴露 feed/finish/reset 三个 WS 流式所需操作。**不引入** desktop 的 `StreamingPipelineEngine` trait——该 trait 为 desktop local/cloud 多态设计，server 只有 local，直接薄包即可（YAGNI）。

```rust
// crates/server/src/pipeline.rs
use anyhow::Result;
use octopus_asr_local::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

/// WS 流式会话：薄包 asr `StreamingRunner`（含 VAD 预热 + accept/flush/finish + 纠错）。
/// 不含 polish / denoise（spec §3.8/§3.6：留端，server 不依赖 llm/cpal）。
pub struct WsStreamSession {
    runner: StreamingRunner,
}

impl WsStreamSession {
    /// 由已构造的流式引擎装箱传入（解耦 `StreamingSession`，便于测试注入 fake）。
    /// `correct` 来自 app_config.asr_correct（与批处理 PipelineConfig.correct 同源）。
    /// 失败（VAD 初始化）返 Err，由 handle_ws 回推 {type:error} 后 return。
    /// engine 名校验 + `StreamingSession::new(&engine)` 由 `handle_ws` 负责（见 §5）。
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new(engine, correct)?,
        })
    }

    /// 喂一帧已降噪 16k 样本，返回本帧事件流（0..n 个 TranscriptEvent）。
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        self.runner.push_samples(samples_16k)
    }

    /// 收尾：runner.finish() → Final（追加句号 + 简繁归一）。
    pub fn finish(&mut self) -> TranscriptEvent {
        self.runner.finish()
    }

    /// 重置（会话间复用前调用）。
    pub fn reset(&mut self) {
        self.runner.reset()
    }
}
```

### 4.2 `event_to_json`（`server/src/pipeline.rs`）

`TranscriptEvent` 仅 derive `Debug/Clone/PartialEq/Eq`（**无 Serialize**）。为不污染 asr crate（总 spec §3.1：asr = 零件库 + 端做桥接），WS JSON 序列化放 server 端，`match` 4 variant：

```rust
/// TranscriptEvent → server 私有 WS JSON（统一 {type,text}）。
/// 不动 asr crate（端做桥接，spec §3.1）。
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    let (ty, text) = match ev {
        TranscriptEvent::Partial(t) => ("partial", t),
        TranscriptEvent::Committed(t) => ("committed", t),
        TranscriptEvent::Final(t) => ("final", t),
        TranscriptEvent::Error(t) => ("error", t),
    };
    // text 内的 " / \ / 控制字符转义（与旧手拼路径一致，防破坏 JSON）
    format!(
        r#"{{"type":"{}","text":"{}"}}"#,
        ty,
        text.replace('\\', r"\\").replace('"', r#"\""#).replace('\n', r"\n")
    )
}
```

### 4.3 批处理（`server/src/main.rs::transcribe`）

把 `engine_manager.transcribe(&samples, language)` 换成：

```rust
let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config(language);
let text = state.engine_manager.transcribe_batch(&samples, &cfg)?;
```

`TranscribeResponse { text, duration_ms, rtf }` **格式不变**。

## 5. 数据流

**流式 `WS /ws/stream`**：
```
client → binary PCM(16k LE) ─▶ WsStreamSession.feed
                                  │ runner.push_samples（VAD 静音/标点 + accept/flush/finish 内部收编）
                                  ▼
                              Vec<TranscriptEvent>
                                  │ event_to_json
                                  ▼
client ◀── text {type:partial|committed|final|error, text} ── WS 回推

client → text "flush"  ─▶ WsStreamSession.finish ─▶ Final ─▶ 回推 ─▶ reset()
client → Close          ─▶ 退出循环
```
输入协议（binary f32 PCM + `"flush"` text + Close）**不变**。

**批处理 `POST /transcribe`**：
```
body PCM → read_wav_16k_from_bytes（或 raw f32 兜底）
        → engine_manager.transcribe_batch(&samples, &PipelineConfig::from_app_config(language))
        → TranscribeResponse { text, duration_ms, rtf }（格式不变）
```

## 6. 接口契约

**WS 输出（新协议）**——统一 `{type, text}`，对应 `TranscriptEvent` 4 variant：
```json
{"type":"partial","text":"..."}     // engine.accept_samples 增量（可能随后被改写）
{"type":"committed","text":"..."}   // 静音冲刷提交（runner 内部 0.5s 静音触发，插逗号）
{"type":"final","text":"..."}       // "flush" 命令收尾（追加句号 + 简繁归一）
{"type":"error","text":"..."}       // 单帧/建连错误（非致命）
```
破坏旧 `{text,final}` / `{error}` 协议——已确认 server 无外部客户端。

**WS 输入**：不变（binary f32 PCM 16k LE + text `"flush"` + Close）。

**批处理响应**：`TranscribeResponse { text, duration_ms, rtf }` 不变；仅内部换 `transcribe_batch`。

## 7. 错误处理

| 场景 | 处理 |
|---|---|
| 流式建连失败（未知 engine 名 / VAD 初始化） | `WsStreamSession::new` 返 `Err`；`handle_ws` 回推一条 `{type:error}` 后 return（同现状 L230-249） |
| 单帧处理错误 | `push_samples` 不返 Result，错误产 `TranscriptEvent::Error` variant；server 回推后**继续**（非致命，总 spec §9.1，与现状 `accept_samples` 错误回推一致） |
| 批处理失败 | `transcribe_batch` 返 `Result`，现有 500 错误路径不变 |
| 静音 flush | 从 server 手搓（`detect_silence_gap_local`）迁入 `StreamingRunner` 内部（`PUNCTUATION_SILENCE_THRESHOLD = 0.5`，与现状 `detect_silence_gap_local` 的 0.5s 一致），**行为不变** |

## 8. 删除项（零行为差异）

> **唯一预期差异——VAD preroll**（code review I-1）：新路径经 `StreamingRunner::new` 构造时 `preroll_vad`（喂 10 帧静音预热 Silero LSTM，搬自 `coordinator.rs`），旧 `detect_silence_gap_local` 无预热。效果是会话开头几帧 VAD 概率更稳定 → 标点触发时机更准（对齐 desktop 已验证行为），属**预期改善**，非 regression。另：accept/flush 错误路径更严格（旧 `_ => {}` 吞错，新区分 `Ok(None)` 静默 vs `Err → Error` 事件）——同样属改善。

- `detect_silence_gap_local`（~45 行手搓 VAD：512 chunk / 0.5 阈值 / 0.5s 静音）→ `StreamingRunner` 内部已收编等价逻辑
- 裸 `StreamingSession` + 手写 `accept_samples`/`flush`/`finish` 循环 → `WsStreamSession`
- 手拼 `{text,final}` / `{error}` JSON → `event_to_json`
- `handle_ws` 内本地 `silence_duration` / `flushed` 状态变量 → runner 内部状态

## 9. 测试策略

**单测**（`server/src/pipeline.rs` 纯逻辑，无需起 server）：
- `event_to_json`：4 variant 各一条断言（含 `text` 转义：`"` / `\` / `\n`）
- `WsStreamSession`（注入 `FakeStreamingEngine`，无需 VAD 模型）：feed 产 `Partial`（accept Some）/ 第二帧空（accept None）、finish 产 `Final`

**e2e**（起 server，回归）：
- WS：发 16k PCM → 验 `{type:...}` 事件序列（含静音后 `committed`、`flush` 后 `final`）
- HTTP `/transcribe`：验 `transcribe_batch` 结果与旧 `transcribe` 路径一致（相同音频产出相同文本）

## 10. 迁移映射（现有 → 新）

| 现有（server/main.rs） | 新位置 | 说明 |
|---|---|---|
| `StreamingSession::new(&engine)` 裸调 | `handle_ws` 构 `StreamingSession` → `WsStreamSession::new(Box::new(session), correct)` → `StreamingRunner::new` | engine 构造留 handle_ws，WsStreamSession 只收 `Box<dyn StreamingEngine>`（解耦 + 可注入 fake） |
| `detect_silence_gap_local` 手搓 VAD | 删除（`StreamingRunner` 内部 VAD） | 阈值一致 0.5s |
| `streaming_session.accept_samples/flush/finish` 手写循环 | `WsStreamSession::feed`/`finish` | 委托 runner |
| 手拼 `{text,final}`/`{error}` JSON | `event_to_json` | match TranscriptEvent |
| `engine_manager.transcribe(&samples, language)` | `engine_manager.transcribe_batch(&samples, &PipelineConfig::from_app_config(language))` | 对齐 cli |

## 11. 风险

- **VAD 阈值差异**：现状 `detect_silence_gap_local` 与 `StreamingRunner` 内部 VAD 均为 0.5s/0.5 阈值，但 chunk 判定细节（`speech_chunks>=2` 重置 vs runner 的 `silence_duration` 累积）可能有细微差异 → e2e 回归验证行为一致；若发现差异，以 runner 为准（desktop 已验证）。
- **WS 协议破坏**：已确认无外部客户端；若有未发现的调用方，需同步更新 → 低风险。
- **`correct` 参数来源**：流式 `WsStreamSession::new(engine, correct)` 与批处理 `PipelineConfig.correct` 均取自 `app_config.asr_correct`，保持一致。
