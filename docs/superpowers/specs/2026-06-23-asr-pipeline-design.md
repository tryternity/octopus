# ASR Pipeline 架构重构设计

> 2026-06-23 初版（brainstorming 产出）。
> **阶段1 已实施（2026-06-23）**：`asr::pipeline`（PipelineConfig + transcribe_batch）、`transcribe_with_vad` 委托、cli 走新 pipeline。流式 trait / StreamingRunner / desktop / server 留阶段2/3。
> **阶段2 进行中（2026-06-23）**：phase 2（desktop 全量拆分）拆为 2a/2b/2c-1/2c-2/2c-3/2d——
> - **2a（已实施，ff-merge main）**：asr 流式基础设施 `StreamingRunner` + `StreamingEngine` trait + `TranscriptEvent`（plan `stage2a.md`）。
> - **2b（已实施，commit 5ab50e7/1d9e347，ff-merge main deac36b，e2e 基本通过 2026-06-24）**：desktop 本地流式迁移——`Stage::Streaming` 委托 `StreamingRunner`，`handle_streaming_tick` 消费 `TranscriptEvent`，stop 用 `finish_with_tail`；`StreamingPipeline` 抽象延后 2c（plan `stage2b.md`）。
> - **2c-1（已实施，commit 6106401/d2bf7dd/9a803a5，e2e 通过 2026-06-24，ff-merge main 9a803a5）**：`StreamingPipeline` 壳立（`desktop/pipeline.rs`）+ local ASR→set_full 迁入 pipeline；emit/DB/polish 留 coordinator（三路径共用 / 保持 set_full→DB→emit 顺序）；transcript 留 Stage。cloud/VadSegmented 不动（plan `stage2c1.md`）。
> - **2c-2（设计已定 2026-06-24，待 plan）**：cloud 接入——上层 trait `StreamingPipelineEngine`（`LocalPipelineEngine` 包 StreamingRunner / `CloudPipelineEngine` 持 CloudStreamHandle 各 impl）+ cloud `close_async` 留 coordinator（不可消除，`Stage::CloudClosing` + session_id 护栏保留）；`Stage::CloudStreaming` 合并进 `Stage::Streaming`，cloud tick 迁入 `CloudPipelineEngine`。spec `2026-06-24-asr-pipeline-stage2c2-design.md`。
> - **2c-3（待）**：VadSegmented（离线分段，`OfflineAsrEngine` async transcribe + seq 乱序回填）归位——语义模型不同（非流式分段），单独设计。
> - **2d（待）**：coordinator 清理——emit/DB/polish + transcript 全收敛进 `StreamingPipeline`，coordinator 退化为路由。
> **设计调整（用户 2026-06-23 决策，覆盖 §3.3/§3.6 字面）**：denoise + resample **留 `desktop/audio.rs` 不迁入 StreamingRunner**（denoise 紧耦合 cpal 采集，`DenoiseProcessor`/`AudioResampler` 类型本就在 asr，`audio.rs` 仅调用方）；`StreamingRunner` 输入即 `drain_samples()` 的已降噪 16k 样本，只收编 VAD 静音+标点+engine accept/flush/finish 纯 ASR 编排。`AudioSource` trait 延后到 2b，流式纠错 hook 预留但默认关（§9.4 待核实）。
> 目标：把现在散落在 `desktop/coordinator.rs` 主循环、`desktop/audio.rs` 录制层、`asr/engine.rs::transcribe_with_vad`、`asr/streaming_engine.rs::StreamingSession` 的隐式编排，收敛成**显式的 pipeline 角色** + **asr 提供可复用零件与无端编排 helper**。
> 工作分支：`worktree-model-mgmt-ui`（后续可开独立 worktree 实施）。

## 1. 背景与定位

当前 ASR 流程的编排是**隐式且分散**的：

- **降噪**在 `desktop/audio.rs` 录制回调（`denoise.process_samples(&s48k)`），混在采集层。
- **流式/批处理/云端三路分发**埋在 `desktop/coordinator.rs` 一个大主循环里（`use_streaming` / `use_cloud_streaming` 标志 + if-else）。
- **批处理编排**（VAD 分段→逐段 transcribe→连接→纠错）在 `asr/engine.rs::transcribe_with_vad`。
- **流式编排**在 `asr/streaming_engine.rs::StreamingSession` + coordinator 循环裸调。
- **纠错**（`LightCorrector`）只在批处理末端、默认关；**流式无纠错钩子**。
- **润色**状态机在 `desktop/transcript.rs::Transcript`，执行调 `octopus_llm::polish`。

问题：没有一个「pipeline」角色能让人一眼看清「音频从哪进、经过哪些 stage、从哪出」；流式/批处理/云端三套路径各自演化；asr 与 desktop 职责糊在一起（coordinator 既管 Tauri 事件又管 ASR 编排）。

**本次重构定位**：

- **asr = 零件库 + trait + 无端编排 helper**。提供可复用的纯计算组件（denoise/vad/engine/corrector）和两个**不知道端**的编排 helper（`transcribe_batch` / `StreamingRunner`）。**不依赖 cpal、不依赖 llm crate**。
- **desktop / cli / server 各自有一个显式的 pipeline 角色**：用 asr 零件 + helper 串成端到端流程，加端特有胶水（Tauri emit / stdout / WS）。
- **音频采集（cpal）留 desktop**——只有 desktop 有麦克风，cpal 不进 asr。

## 2. 现状（已探明）

### 2.1 TranscriptionEngine trait 只有批处理接口
`crates/desktop/src/engine.rs:6`：
```rust
pub trait TranscriptionEngine: Send + Sync {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String>;
    async fn health_check(&self) -> bool;
}
```
`transcribe` 吃整段 16k samples 返回 String——**纯批处理，无流式方法**。实现：`EmbeddedEngine`（local，内部调 asr）、`WsRemoteEngine`、`GrpcRemoteEngine`。`build_local_engine`（main.rs:354）按 `engine_mode` 选实现。

### 2.2 流式绕过 trait，coordinator 裸调 StreamingSession
`coordinator.rs:9` `use octopus_asr::streaming_engine::StreamingSession;`，`StreamingSession::new(&config.asr_engine)` 直接创建（:676）。流式/云端流式通过 `use_streaming` / `use_cloud_streaming` 标志在主循环分发（:615+），**不经过 TranscriptionEngine trait**。

### 2.3 润色执行已在 llm crate
desktop 调 `octopus_llm::polish(preserved, to_polish, &llm_config)`（coordinator.rs:1092/1919），`desktop/config.rs::llm_config()` 构造 `CompatibleLlmConfig`。润色**状态机**（`polished`/`raw_len`/`polish_pending`/`polish_snapshot_len`/`PolishMode`）在 `desktop/transcript.rs::Transcript`。

### 2.4 降噪在 desktop 录制层
`desktop/audio.rs` 录制回调 `denoise.process_samples(&s48k)`——**48k 上降噪**。RNNoise/DF3 都是 **48kHz 训练**，frame 尺寸绑死 48k（10ms=480 样本），不能在 16k 上跑。

### 2.5 批处理编排 + 纠错
`asr/engine.rs:151 transcribe_with_vad`：短音频（<480k samples）直连 transcribe；长音频 silero VAD `segment_audio_vad` 分段→逐段 transcribe→连接。纠错在 :236 接入（`app_cfg.asr_correct && !engine.skip_corrector() && !is_english` 时调 `LightCorrector`）。`skip_corrector()`：qwen3 返回 true（自带纠错），moonshine 英文跳过。

## 3. 设计

### 3.1 crate 边界

| crate | 职责 | 依赖 cpal? | 依赖 llm? |
|---|---|---|---|
| **asr** | 纯计算零件（denoise/vad/engine/corrector）+ trait（`StreamingEngine`/`OfflineEngine`/`AudioSource`/`TranscriptEvent`）+ 无端 helper（`transcribe_batch`/`StreamingRunner`）+ ngram 预留位 | **否** | **否** |
| **desktop** | `MicSource`（cpal）+ 显式 `StreamingPipeline`（持有 `StreamingRunner`）+ Transcript 润色状态机 + Tauri 壳 | 是 | 是 |
| **cli** | 读 wav + 显式 BatchPipeline（调 `transcribe_batch`）+ 可选润色 + stdout | 否 | 是（可选） |
| **server** | WS source + 显式 pipeline（`StreamingRunner`）+ WS 回推 | 否 | 视需求 |

### 3.2 核心 trait（定义在 asr）

```rust
/// 批处理引擎：整段 16k samples → 文本。收编现有 TranscriptionEngine.transcribe。
pub trait OfflineEngine: Send + Sync {
    async fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;
    async fn health_check(&self) -> bool;
    fn skip_corrector(&self) -> bool;
}

/// 流式引擎：推 frame → 增量文本。现有 StreamingSession 包装 / cloud WS client 实现。
pub trait StreamingEngine: Send + Sync {
    fn push_frame(&mut self, frame: &[f32]) -> Result<()>;
    /// 静音点/flush：返回定稿片段，可能插入标点。
    fn commit(&mut self) -> Result<Option<String>>;
    fn flush(&mut self) -> Result<String>;
}

/// 流式音频源（mic / WS）。文件不抽象成 trait——BatchPipeline 直接吃 &[f32]。
pub trait AudioSource: Send {
    fn next_frame(&mut self) -> Option<Vec<f32>>;  // 48k frame（mic）
}

/// pipeline 输出事件。各端桥接：desktop→Tauri emit、cli→stdout、server→WS。
pub enum TranscriptEvent {
    Partial(String),    // 流式增量
    Committed(String),  // 定稿片段
    Final(String),      // 整句/批处理完成
    Error(String),
}
```

### 3.3 asr 无端编排 helper（不碰 Tauri/stdout/WS）

- **`transcribe_batch(engine: &dyn OfflineEngine, samples: &[f32], cfg: &PipelineConfig) -> impl Iterator<Item<TranscriptEvent>`**
  收编 `transcribe_with_vad`：VAD 分段→逐段 transcribe→连接→（`cfg.correct` 时纠错）→末尾发 `Final`。**不知道端**。

- **`StreamingRunner`**
  收编 coordinator 流式循环：吃 **48k frame（`Vec<f32>`，来自 `AudioSource`）** → `denoise(48k) → resample(16k) → 流式 VAD → streaming engine →（commit/flush 时 `cfg.correct` 纠错）` → emit `Partial`/`Committed`/`Final`。持有 `StreamingEngine` + `DenoiseProcessor`。**补流式纠错钩子**（现状无）。

两者输出 `TranscriptEvent`，润色**不在** helper（留端，见 3.8）。

### 3.4 各端显式 pipeline

- **`desktop/src/pipeline.rs`** — `StreamingPipeline { source: MicSource, runner: StreamingRunner, cfg: PipelineConfig }`：
  MicSource（cpal，48k frame）→ StreamingRunner → TranscriptEvent 流 → Transcript 润色状态机（调 `octopus_llm::polish`）→ Tauri emit + overlay + 快捷键。收编 coordinator 主循环的流式/云端分发（分发 = 选 StreamingEngine 实现：local `StreamingSession` / cloud WS）。

- **`cli/src/pipeline.rs`** — 读 wav（`read_wav_16k`）→ `asr::transcribe_batch` →（可选 `octopus_llm::polish`）→ stdout。

- **`server/src/pipeline.rs`** — WS source（实现 `AudioSource`）→ `StreamingRunner` → WS 回推 `TranscriptEvent`。

### 3.5 PipelineConfig（6 维度映射）

入口维度（1/2）**不进 config**——desktop 永远流式、cli 文件永远批处理，由调用端构造时选 pipeline 类型。维度 4（润色）也**不进 asr config**——润色留端（见 3.8），由端按其 `polish_mode`（desktop `Transcript`/`runtime_config` 已有）调 `octopus_llm::polish`。config 只管 ASR stage 开关（维度 3/5/6 + 降噪/语言），**流式与批处理共用一份**：

```rust
struct PipelineConfig {
    backend: AsrBackend,     // 维度3：Local(ResolvedEngine) | Cloud(CloudSpec)
    correct: bool,           // 维度5：asr_correct（默认 false）
    ngram: bool,             // 维度6：默认 false（未实现，预留，见 3.7）
    denoise: DenoiseMode,    // 降噪（Off/Rnnoise/Df3）
    language: String,
    simplify: bool,          // 简繁
}
```

**engine 正交分解**（维度 3 × 流式/批处理）：

|  | 本地 | 云端 |
|---|---|---|
| **流式** | `StreamingEngine` = StreamingSession 包装 | `StreamingEngine` = WS client（火山/腾讯/百度/阿里） |
| **批处理** | `OfflineEngine` = OfflineAsrEngine | `OfflineEngine` = 同步 ASR API（若有） |

`StreamingPipeline` 持 `Box<dyn StreamingEngine>`、BatchPipeline 持 `Box<dyn OfflineEngine>`；`cfg.backend` 决定实例化 local 还是 cloud 实现。**pipeline 主体对本地/云端无感**。

### 3.6 降噪采样率顺序（关键）

RNNoise/DF3 是 48k 训练，**必须在 48k 降噪**（反过来 16k denoise 不 work——窗长/hop 对不上、高频已丢）。正确顺序：**48k 采集 → 48k denoise → 16k resample → ASR**。

- **StreamingRunner**（mic 流式）：`MicSource` 输出 **48k frame**，runner 内部 `denoise(48k) → resample(16k) → vad → engine`。denoise 状态机归 runner。
- **transcribe_batch**（文件）：cli 经 `read_wav_16k` 提供 16k samples。⚠️ 文件路径的 denoise 是**待核实项**——现状 `audio.rs` denoise 是 mic 48k 专用，文件批处理路径是否/如何 denoise 需迁移时核实（文件若 48k 可 denoise 再 resample；若已 16k 则受限于 48k 模型无法直接 denoise）。

### 3.7 ngram（disabled 开发，预留接入点）

现状**未实现**。`cfg.ngram` 默认 false。pipeline 骨架预留一个 ngram stage 位置（engine 解码后、corrector 前），默认跳过；后续实现只需填该 stage + 翻开关，不动骨架。

### 3.8 polisher 留端，asr 不碰 llm

润色**不进 asr helper**。`StreamingRunner`/`transcribe_batch` 输出到「corrected 文本」（`TranscriptEvent`）为止；润色由端 pipeline 在 `Committed`/`Final` 时按 `polish_mode` 调 `octopus_llm::polish`（**已存在，无需新抽**）。理由：① asr 不依赖 llm crate，更干净；② 润色状态机（节流、`polished`/`raw`/`edited` 分层）是 desktop `Transcript` 的成熟职责，留端；③ cli/server 直接调同一个 `octopus_llm::polish` 复用。流式中间润色（`MidAndFinal` 节流）归端 StreamingPipeline（与 emit 节奏绑定）。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| `asr` 新增 | `OfflineEngine`/`StreamingEngine`/`AudioSource`/`TranscriptEvent` trait + `PipelineConfig` + `transcribe_batch` helper + `StreamingRunner` + ngram stage 预留位 |
| `asr/engine.rs::transcribe_with_vad` | 收编进 `transcribe_batch`（保留行为，纠错参数化） |
| `asr/streaming_engine.rs::StreamingSession` | 包进 `StreamingRunner`（补流式纠错） |
| `desktop TranscriptionEngine` trait | 迁入 asr 成 `OfflineEngine`（EmbeddedEngine/WsRemoteEngine/GrpcRemoteEngine 跟随或重定向） |
| `desktop/coordinator.rs` | 主循环的流式/批处理/云端分发逻辑 → 拆入 `StreamingRunner`（asr）+ `StreamingPipeline`（desktop）；coordinator 退化为 pipeline 驱动 + Tauri 命令路由 |
| `desktop/audio.rs` | denoise 调用移入 `StreamingRunner`；audio.rs 只保留 cpal 采集 + 输出 48k frame |
| `desktop/transcript.rs::Transcript` | **保留**（润色状态机留端） |
| 润色执行 `octopus_llm::polish` | **不改**（各端复用） |
| cloud 引擎 | 现有 cloud 调用点（coordinator `use_cloud_streaming`）→ `StreamingEngine` 的 cloud 实现（feature-gated，位置迁移时定） |

## 5. 数据流

**desktop mic（本地，开润色+纠错）**：
```
MicSource(cpal, 48k) ─frames─▶ StreamingRunner {
    denoise(48k) → resample(16k) → 流式 VAD → StreamingEngine(Local StreamingSession)
    → commit/flush 时 corrector(cfg.correct)
} ─TranscriptEvent─▶ StreamingPipeline → Transcript(润色, octopus_llm::polish) → Tauri emit
```
换云端：`backend: Cloud(spec)`，`StreamingEngine` 换 cloud WS，其余不动。

**cli 文件**：
```
read_wav_16k ─&[f32]─▶ transcribe_batch(OfflineEngine, cfg) ─Final─▶ (可选 polish) → stdout
```

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| 引擎创建失败 | 沿用现状 fallback（StreamingSession 失败→默认引擎，再失败→降级批处理） |
| 单帧处理/网络错误 | emit `TranscriptEvent::Error`，各端决定是否中断/重试 |
| denoise/resample 失败 | runner 返回 Error 事件，跳过该帧继续（不致命） |
| 纠错/润色失败 | 纠错失败保留原文；润色失败保留 raw（沿用 Transcript 既有 `polish_failed_keeps_polished`） |

## 7. 范围边界（不做）

- **cpal 不进 asr**：音频采集留 desktop（`MicSource` 在 desktop）。
- **润色不进 asr**：asr 不依赖 llm crate，润色留端。
- **不统一 Stage trait**：流式/批处理语义不同，不强抽统一 `Stage`；用两个 helper + 共享零件。
- **ngram 不实现**：仅预留 stage 位置，默认 disabled。
- **不重写润色状态机**：Transcript 留 desktop 原样。
- **本次不迁 server**：server pipeline 列为架构占位，实施时可后置（cli/desktop 优先）。

## 8. 测试策略

- **`transcribe_batch` 单测**：收编自 `transcribe_with_vad`，沿用其测试 + 补 `cfg.correct` 开关用例。
- **`StreamingRunner` 单测**：注入 `FakeAudioSource`（产固定 frame）+ `FakeStreamingEngine`，验证 denoise→resample→commit→纠错→TranscriptEvent 序列。流式纠错是新增点，重点测。
- **trait 拆分单测**：`OfflineEngine`/`StreamingEngine` 各 fake 实现，验证 pipeline 持有类型正确。
- **denoise 采样率**：断言 runner 内 denoise 在 48k、ASR 输入 16k（用 fake source 控制采样率）。
- 端 pipeline / Tauri / WS 集成无自动化，靠 `cargo check --workspace --all-targets` + clippy + 手动。

## 9. 迁移映射（现有代码 → 新位置）

| 现状 | 归到 | 动作 |
|---|---|---|
| `asr/engine.rs::transcribe_with_vad` | asr `transcribe_batch` | 整理 + 纠错参数化 |
| `asr/streaming_engine.rs::StreamingSession` | asr `StreamingRunner` | 包装 + 补流式纠错 |
| `desktop/audio.rs` denoise 调用 | asr `StreamingRunner` 内部第一级 | desktop 退成「只采集 48k PCM」 |
| `desktop/coordinator.rs` 主循环分发 | desktop `pipeline.rs`（StreamingPipeline）+ asr `StreamingRunner` | 分发 = 选 engine 实现，不再是 if-else 循环 |
| `desktop TranscriptionEngine` trait | asr `OfflineEngine` | 搬 trait + 实现 |
| `desktop/transcript.rs::Transcript` | desktop（保留） | 润色状态机不动 |
| `LightCorrector`（仅批处理、默认关） | cfg.correct，batch + streaming 都接 | streaming 补 flush 纠错 |
| cloud（`use_cloud_streaming`） | asr `StreamingEngine` cloud 实现 | feature-gated，位置迁移时定 |

## 10. 待核实 / 风险

- **文件路径 denoise**：现状 denoise 是 mic 48k 专用；`transcribe_batch`（文件）的 denoise 行为需迁移时核实（见 3.6）。
- **cloud 引擎实现位置**：现有 cloud（dashscope 等 feature）的代码组织，迁移时确认归 asr feature-gated 还是独立。
- **TranscriptionEngine 搬迁影响面**：trait 搬入 asr 后，`build_local_engine`、EmbeddedEngine 等跟随；需核 remote-ws/remote-grpc feature 的 engine 实现是否顺势归 asr（remote 引擎本质上也是 OfflineEngine 的远程实现）。
- **流式纠错语义**：补 flush 纠错后，需确认与 Transcript 的 raw/polished 分层不冲突（纠错改 raw，润色基于 raw/polished）。
- **迁移节奏**：本次是大改（trait 搬迁 + helper 收编 + 三端 pipeline），建议分阶段——先 asr helper + cli BatchPipeline（最简，验证骨架）→ desktop StreamingPipeline（含 cpal 边界 + Transcript 接线）→ server（可后置）。
