# 已归档设计规格（2026-06-23 ~ 2026-06-25）

> 以下功能均已实现并合并 main。交叉引用统一指向本归档文件内同名章节；对应 plans 见 `docs/superpowers/plans/2026-06-25-archived-plan.md`。

## 目录

| 主题 |
|---|
| asr-pipeline-design | ASR pipeline 架构重构设计 |
| asr-pipeline-stage2c2-design | Stage 2C2：coordinator 清理设计 |
| asr-server-stage3-design | Stage 3：server crate 设计 |
| cloud-asr-cli-design | 云端 ASR CLI 接入设计 |
| coordinator-cleanup-design | coordinator 状态机清理设计 |
| desktop-cloud-dedupe-design | desktop 云端引擎去重设计 |
| vad-segmented-rehome-design | VAD 分段引擎迁移设计 |

> **注**：clipboard-history-design 仍在活跃迭代，保留在独立文件 `2026-06-25-clipboard-history-design.md`。

---

---

## 2026-06-23-asr-pipeline-design

# ASR Pipeline 架构重构设计

> 2026-06-23 初版（brainstorming 产出）。
> **阶段1 已实施（2026-06-23）**：`asr::pipeline`（PipelineConfig + transcribe_batch）、`transcribe_with_vad` 委托、cli 走新 pipeline。流式 trait / StreamingRunner / desktop / server 分别在阶段2/3 落地（均已完成，见下）。
> **阶段2 已完成（2026-06-25，2a-2d 全 ff-merge main）**：phase 2（desktop 全量拆分）拆为 2a/2b/2c-1/2c-2/2c-3/2d——
> - **2a（已实施，ff-merge main）**：asr 流式基础设施 `StreamingRunner` + `StreamingEngine` trait + `TranscriptEvent`（plan `stage2a.md`）。
> - **2b（已实施，commit 5ab50e7/1d9e347，ff-merge main deac36b，e2e 基本通过 2026-06-24）**：desktop 本地流式迁移——`Stage::Streaming` 委托 `StreamingRunner`，`handle_streaming_tick` 消费 `TranscriptEvent`，stop 用 `finish_with_tail`；`StreamingPipeline` 抽象延后 2c（plan `stage2b.md`）。
> - **2c-1（已实施，commit 6106401/d2bf7dd/9a803a5，e2e 通过 2026-06-24，ff-merge main 9a803a5）**：`StreamingPipeline` 壳立（`desktop/pipeline.rs`）+ local ASR→set_full 迁入 pipeline；emit/DB/polish 留 coordinator（三路径共用 / 保持 set_full→DB→emit 顺序）；transcript 留 Stage。cloud/VadSegmented 不动（plan `stage2c1.md`）。
> - **2c-2（已合并 main 2026-06-24，commit f8cd395→9928f60，e2e 通过）**：cloud 接入——上层 trait `StreamingPipelineEngine`（`LocalPipelineEngine` 包 StreamingRunner / `CloudPipelineEngine` 持 CloudStreamHandle 各 impl）+ cloud `close_async` 留 coordinator（不可消除，`Stage::CloudClosing` + session_id 护栏保留）；`Stage::CloudStreaming` 合并进 `Stage::Streaming`，cloud tick 迁入 `CloudPipelineEngine`（`cloud_pipeline.rs`）。双 feature 编译/测试通过 + clippy 零新 warning；e2e 通过（本地+云端流式识别正常），已 ff-merge main。spec `2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design`，plan `2026-06-25-archived-plan.md#asr-pipeline-stage2c2`。
> - **2c-3（已 ff-merge main `a5630c4`，2026-06-25）**：VadSegmented（离线分段，`OfflineAsrEngine` async transcribe + seq 乱序回填）归位——`VadSegmentedPipeline` + `Pipeline` 上层 trait，删 `Command::TranscriptionDone`，WaitingCompletion 由 tick 线程空样本 drain rx 驱动；零行为差异，VadSegmented 全路径 e2e 通过。spec `2026-06-25-archived-spec.md#vad-segmented-rehome-design`。
> - **2d（已 ff-merge main `4acf340`，2026-06-25）**：coordinator 清理——emit/DB/polish 触发逻辑收敛进 pipeline 事件流（`PipelineEvent` + `Pipeline::tick` 签名 `bool→Vec` + `apply_pipeline_events`/`dispatch_tick` 统一路由），coordinator 退化为事件路由；transcript 留 Stage，stop 路径丢弃 tick 事件保零行为差异，三路径 e2e 通过。spec `2026-06-25-archived-spec.md#coordinator-cleanup-design`。
> **阶段3 已实施（2026-06-26）**：server 两端点迁 asr helper——流式 `/ws/stream` 用 `WsStreamSession`（薄包 `StreamingRunner`，删手搓 `detect_silence_gap_local` + 手拼 `{text,final}`），批处理 `/transcribe` 走 `transcribe_batch`（对齐 cli：VAD 分段 + 纠错 + 简繁）。WS 回推改 `TranscriptEvent` `{type,text}`。不接 cloud，polish/denoise 不进 server（§3.8/§3.6）。spec `2026-06-25-archived-spec.md#asr-server-stage3-design`，plan `2026-06-25-archived-plan.md#asr-server-stage3`。
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
`coordinator.rs:9` `use octopus_asr_local::streaming_engine::StreamingSession;`，`StreamingSession::new(&config.asr_engine)` 直接创建（:676）。流式/云端流式通过 `use_streaming` / `use_cloud_streaming` 标志在主循环分发（:615+），**不经过 TranscriptionEngine trait**。

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

---

## 2026-06-24-asr-pipeline-stage2c2-design

# ASR Pipeline 阶段 2c-2：云端流式接入 StreamingPipeline

> 2026-06-24 初版（brainstorming 产出）。
> **状态**：已合并 main（2026-06-24，T1-T4 + final review 共 7 commit `f8cd395`→`9928f60`，TDD + 双 feature 编译/测试通过 + clippy 零新 warning；e2e 通过——本地+云端流式识别正常，ff-merge main）。Approach 1：上层 trait `StreamingPipelineEngine`（`LocalPipelineEngine`/`CloudPipelineEngine`）+ cloud close 留 coordinator。plan `docs/superpowers/plans/2026-06-25-archived-plan.md#asr-pipeline-stage2c2`。
> **关联**：总 spec `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design` §3.4（阶段 2c-2）。
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
    vad: octopus_asr_local::vad::SileroVad,
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

---

## 2026-06-25-asr-server-stage3-design

# ASR Pipeline 阶段3：server 迁移设计

> **关联总 spec**：`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`（§3.4 / L119 / L68 / L93 / L206）
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

---

## 2026-06-25-cloud-asr-cli-design

# 云端 ASR 下沉：cli 批处理接入（`octopus-asr-cloud` crate）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：已实现且 e2e 通过（plan `docs/superpowers/plans/2026-06-25-archived-plan.md#cloud-asr-cli`，8 task 全完成；asr-cloud 30 单测绿、workspace check 0 error、新代码 clippy 0 warning；e2e 用户本地云端 key 验通过 2026-06-25）。
> **动机**：cli/server 转译音频文件应能选云端 ASR 引擎（DashScope/ByteDance/Tencent/Baidu），不必只靠本地 onnx。当前云端 ASR 全锁在 desktop crate（依赖 `tauri::async_runtime`），cli 够不到。
> **关联**：ASR pipeline 总 spec `2026-06-25-archived-spec.md#asr-pipeline-design`；2c-2 cloud 流式已合并 main（`fa2becc`）。
> **范围（本次）**：建 `octopus-asr-cloud` crate（WSS 协议层 + 批引擎）+ cli 接入。**不含**：desktop 复用（第二步，后续）、流式适配（留 desktop）、VadSegmented（2c-3）。

---

## 1. 背景与问题

ASR pipeline 重构的大愿景：asr 模块含一切 ASR 能力（含云端），desktop 只是壳。当前现实：

- **本地 ASR**（onnx：zipformer/whisper/qwen3 等）在 `octopus-asr-local`，cli/server/desktop 共用，走同步 `OfflineAsrEngine` trait。
- **云端 ASR**（4 provider WSS 流式）全在 `octopus-desktop`：`baidu_stream.rs`/`bytedance_stream.rs`/`aliyun_stream.rs`/`tencent_stream.rs` + `cloud_types.rs` + `cloud_pipeline.rs`。签名 `open(rt: &tauri::async_runtime::RuntimeHandle, ...)`——依赖 tauri runtime，cli/server 够不到。
- `asr` crate 是**纯同步**（无 tokio），被 cli/server/desktop 共用；`desktop` 才有 `tokio` + `tokio-tungstenite`（`cloud` feature）。

结果：cli 转译音频文件**只能本地 ASR**，无法选云端 API。这违背「asr 含一切 ASR」的愿景，也限制了 cli/server 的实用性。

## 2. 用户决策（brainstorming 2026-06-25）

1. **范围**：协议层 + 批处理下沉 asr 层；desktop 流式适配（`CloudPipelineEngine`）留 desktop。
2. **crate 结构**：新建 `octopus-asr-cloud`（依赖 asr，`asr` 保持纯同步零污染）。
3. **cli 配置**：复用 `AppConfig.asr.{provider}`（与 desktop 同源，不另建配置）。
4. **时机**：分两步——本次只 cli（cloud crate + 批引擎 + cli 接入，desktop 零改动、`*_stream.rs` 副本暂留）；后续第二步再让 desktop 删副本、改指 cloud 协议层。

## 3. 架构

### 3.1 crate 依赖图

```
octopus-asr-cloud ──→ octopus-asr-local        (impl OfflineAsrEngine trait)
                 ──→ octopus-infra       (ModelEntry, parse_model_spec, config 类型)
                 ──→ tokio, tokio-tungstenite(native-tls), uuid, base64, flate2, hmac, sha1

octopus-cli ──→ octopus-asr-local              (AsrEngineManager, pipeline, config)
            ──→ octopus-asr-cloud        (CloudBatchEngine, 云端分流)
            ──→ octopus-infra

octopus-desktop（本次不动）──→ 仍用自己的 *_stream.rs 副本
```

**依赖单向**：`asr ← cloud`，`asr` 不依赖 `cloud`（避免循环）。cli 同时依赖两者，在 cli 层做本地/云端分流。`asr` 保持纯同步、零 tokio。

### 3.2 三层分工

| 层 | crate | 形态 | 本次 |
|---|---|---|---|
| **协议层**（4 provider WSS） | `octopus-asr-cloud` | 纯 **async fn**（建连/鉴权/帧编解码/消息循环），**不自己 spawn** | ✅ 新建（从 desktop 复刻） |
| **批引擎** | `octopus-asr-cloud` | `CloudBatchEngine impl asr::OfflineAsrEngine`：整段音频→VAD 分段→每段推 WSS→拼接。同步，内部 `Runtime::new().block_on` | ✅ 新建 |
| **流式适配** | desktop | `CloudPipelineEngine`+`CloudStreamHandle`+coordinator 桥接 | ⏸ 不动（第二步复用协议层） |

### 3.3 runtime 方案（关键简化）

cloud 协议层是**纯 async fn 不 spawn** → 不依赖具体 runtime、不造 trait、不依赖 tauri：

- **批引擎**（cli/server）：内部 `tokio::runtime::Runtime::new().block_on(async { ... })`。cli 主线程非 tokio context，无嵌套 runtime 风险。
- **desktop**（第二步）：`tauri::async_runtime::spawn` 驱动 cloud 协议层 async fn，沿用现有同步/异步桥接。

cloud crate 只暴露 async 协议 fn + 同步批引擎，spawn 上下文由调用方定。无需 `AsyncRuntime` trait 或 Handle 注入。

## 4. `octopus-asr-cloud` crate

### 4.1 协议层（4 provider WSS，纯 async fn）

> **实施修正**（核对 desktop 源码后，详见 plan 顶部「据实修正」）：`open()` 保持**同步**签名（仅 `CloudStreamHandle::new()` + `tokio::spawn` + 返回 handle，不 await），唯一 async 收尾在 `CloudStreamHandle::close_async`；`CloudBatchEngine` 不自己 VAD 分段（`asr::pipeline::transcribe_segments` 自动分段 + CJK 连接）；`is_cloud_spec`/`from_spec` 用 `parse_model_spec` 的 **3-part provider 前缀**判云端（不查 DB），须 `provider:category:model_name` 三段 spec。

从 desktop `baidu_stream.rs`/`bytedance_stream.rs`/`aliyun_stream.rs`/`tencent_stream.rs` 复刻协议逻辑（建连、鉴权、二进制/JSON 帧编解码、WS 收发循环），改造为 **async fn**（去掉 `open()` 内部的 `tauri::async_runtime::spawn`，改为调用方驱动的 async fn）：

- `async fn open_<provider>(handle: tokio::runtime::Handle, config: ProviderConfig, pre_roll: &[f32]) -> Result<CloudStream>`
- `CloudStream`：暴露 `push_pcm(&self, samples)` / `finish(&self)` / `try_recv_event() -> Option<StreamEvent>` / `close_async()`（类型沿用 desktop `cloud_types.rs` 的 `PcmFrame`/`StreamEvent`/`CloudStreamHandle` 语义，迁入 cloud crate）。

**复刻原则**：协议字节级、鉴权算法、帧格式 1:1 照搬 desktop（零行为差异），仅把「同步 open + 内部 tauri spawn」重构为「async fn + 调用方 spawn」。

### 4.2 批引擎 `CloudBatchEngine`

```rust
pub struct CloudBatchEngine { /* provider config + tokio Handle */ }

impl OfflineAsrEngine for CloudBatchEngine {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        // 内部 Runtime::new().block_on：VAD 分段 → 每段一个 WSS session → 收 utterance → CJK 规则拼接
    }
    fn skip_corrector(&self) -> bool { /* 与 desktop 云端一致：云端已纠错，跳过本地 corrector？plan 确认 */ }
}
```

**音频策略**（plan 阶段最终确认）：复用 `asr::audio::segment_audio_vad` + `asr::vad::SileroVad` 把长音频分段，每段开一个云端 WSS session（短音频直连一个 session），收每段 `StreamEvent::Text`/`Finished`，按 CJK/非 CJK 规则拼接（复用 `asr::pipeline::transcribe_segments` 的连接逻辑或抽出共享）。这是「每段一个短 session」的 chunk 模式，适合批处理（无需维持长连接 onset/close 状态）。

### 4.3 provider 分发

复刻 desktop `cloud_pipeline.rs` 的分发：`EngineCategory`（Aliyun/Bytedance/Tencent/Baidu）+ `resolve_cloud_entry`/`resolve_<provider>_config`（从 `AppConfig.asr.<provider>` 查 `ModelEntry`，校验 `secret_key` 非空，返回 `(source, secret_key, model_name)`）。

cloud crate 暴露统一入口：
```rust
impl CloudBatchEngine {
    pub fn from_spec(spec: &str) -> Result<Self>;  // "aliyun:qwen-asr" → 解析 category + model_name → resolve config
}
```
`EngineCategory` + spec 解析逻辑在 cloud crate 定义（复用 infra `parse_model_spec` 拿 model_name；category 前缀解析新增）。desktop 第二步改用 cloud crate 的这套，消除重复。

## 5. cli 接入

`crates/cli/src/pipeline.rs::run` 改造为本地/云端分流：

```rust
pub fn run(model_spec: &str, language: &str, samples: &[f32]) -> Result<String> {
    let engine: Box<dyn OfflineAsrEngine> = if is_cloud_spec(model_spec) {
        Box::new(CloudBatchEngine::from_spec(model_spec)?)  // cli 直接构造云端引擎
    } else {
        let mgr = AsrEngineManager::new();
        mgr.switch_model(model_spec)?;                       // 本地 onnx
        mgr.into_active_engine()?                            // 取 Arc<dyn OfflineAsrEngine>（需加 getter）
    };
    let cfg = PipelineConfig::from_app_config(language);
    transcribe_batch(&*engine, samples, &cfg)                // 现有编排零改动
}
```

**依赖边界**：`AsrEngineManager`（asr crate）不支持云端（asr 不依赖 cloud）。分流在 cli 层完成，两端都产出 `dyn OfflineAsrEngine`，`transcribe_batch` 无感。

`AsrEngineManager` 需补一个公开 getter（取 `active_engine: Arc<dyn OfflineAsrEngine>`），供 cli 本地分支使用。

## 6. config 复用

云端 ASR 配置走 `octopus_asr_local::config::load_config().asr.{aliyun,bytedance,tencent,baidu}`（`Option<HashMap<String, ModelEntry>>`），与 desktop 完全同源。`ModelEntry`（`crates/infra/src/db.rs:13`）：`source`/`language`/`secret_key`/`is_local`/`is_enabled`/`is_streaming`/`description`。

各 provider 字段语义复刻 desktop 约定（`source`/`secret_key`/`model_name` 在不同 provider 复用为 endpoint/api_key/dev_pid 等，见 desktop `resolve_<provider>_config`）。cloud crate 依赖 infra 拿 `ModelEntry` 类型 + `parse_model_spec`。

## 7. 测试策略

- **协议层帧编解码**：纯函数（字节级编解码、鉴权串构造、Tencent HMAC-SHA1 签名、ByteDance gzip 帧）→ 单元测试，不连真实 WSS。
- **批引擎**：WSS 难单测，用 `#[ignore]` 真实 key 集成测试（同 desktop DashScope 模式），需用户本地 key 跑。
- **cli 分流**：单测 `is_cloud_spec` / spec 解析；端到端转译用真实 key 手动验（plan 列 e2e 清单）。

## 8. 不在范围

- **desktop 复用**（第二步）：删 desktop `*_stream.rs`/`cloud_types.rs` 协议副本，`CloudPipelineEngine` 改调 cloud crate 协议层。需云端流式 e2e 回归。
- **流式适配**（`CloudPipelineEngine`）：留 desktop，本次不动。
- **VadSegmented 归位**：2c-3，独立设计。
- **server 接入**：同 crate 自动可用（gRPC server 可构造 `CloudBatchEngine`），但本次不验证、不接入。

## 9. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 协议层临时两份（desktop 副本 + cloud crate），第二步前重复维护 | 接受临时重复；协议字节级稳定，改动概率低；第二步尽快合并 |
| 批引擎音频策略（chunk 模式 vs 长连接）未定 | plan 阶段读 desktop `engine_aliyun.rs`（chunk 模式）确认，复用最贴近批处理的形态 |
| 长音频云端超时/限长 | VAD 分段控制单 session 长度；超时分多 session |
| `asr` 不能依赖 `cloud` 的循环约束 | 分流放 cli 层，`asr` 只出 trait；依赖单向 |
| cli 二进制增大（拉 tokio+tungstenite） | cli 本就需 ASR 能力，云端是可选价值；cloud crate 仅 cli/server/desktop 按需依赖，不影响纯本地构建路径 |

## 10. 后续（非本次）

- **第二步**：desktop 复用 cloud 协议层（删副本、`CloudPipelineEngine` 改指 cloud crate），云端流式 e2e 回归。
- **2c-3**：VadSegmented 归位。
- **2d**：coordinator 清理。

---

## 2026-06-25-coordinator-cleanup-design

# 2d coordinator 清理（emit/DB/polish 触发逻辑收敛进 pipeline）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已实施并 ff-merge main（Task 1-4，2026-06-25）。双 feature 编译 0 error、clippy 无 2d 引入的 dead_code、workspace 测试除 2 pre-existing infra 失败外全绿；e2e 零行为差异回归通过（2026-06-25）。
> **动机**：ASR pipeline 重构阶段2（总 spec `2026-06-25-archived-spec.md#asr-pipeline-design`）已把三条 ASR 编排路径收进统一 `Pipeline` 角色——流式（2a/2b/2c-1：`StreamingPipeline` 壳）+ cloud（2c-2：`StreamingPipelineEngine` trait + `CloudPipelineEngine`）+ VadSegmented（2c-3：`VadSegmentedPipeline` + 删 `TranscriptionDone`）。但 `Pipeline::tick` 目前只返回 `changed: bool`，**emit/DB/polish 的触发逻辑仍散在 coordinator 三处**（`handle_streaming_tick` / `after_vad_tick` / WaitingCompletion 内联），每处重复 `if changed { DB + emit } + polish` 的变体，cloud 还多出「每 tick emit 预览 + 错误上报」特判。2d 把这些**触发逻辑**收敛进 pipeline 产事件，coordinator 退化为统一事件路由。
> **关联**：总 spec `2026-06-25-archived-spec.md#asr-pipeline-design`（§3.4/§9/§11）；2c-1 `2026-06-23-...`（StreamingPipeline 壳）；2c-2 `2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design`（StreamingPipelineEngine trait）；2c-3 `2026-06-25-archived-spec.md#vad-segmented-rehome-design`（VadSegmentedPipeline + Pipeline trait）。
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

---

## 2026-06-25-desktop-cloud-dedupe-design

# desktop 复用 cloud 协议层（消除协议层两份副本）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已合并 main（`6a4593e`，ff-merge）。Task 1-6 编译/测试/云端流式 e2e 全通过（2026-06-25 本地云端 key 验证）。
> **动机**：cloud-asr-cli（`octopus-asr-cloud` crate）落地后，4 provider WSS 协议层临时存在两份副本——`octopus-asr-cloud`（cli/server 用，去 tauri）与 `octopus-desktop`（流式适配用，依赖 tauri runtime）。本 spec 收口这份技术债：删 desktop 协议副本，desktop 改指 cloud crate，协议层单源。
> **关联**：`2026-06-25-archived-spec.md#cloud-asr-cli-design` §8/§10（明确"第二步"范围）；ASR pipeline 总 spec `2026-06-25-archived-spec.md#asr-pipeline-design`。
> **范围**：desktop 删 5 个协议副本 + 改造 `cloud_pipeline.rs`/`coordinator.rs` 改指 cloud crate + cloud crate 加测试构造器 + 云端流式 e2e 回归。**不含**：`engine_aliyun.rs`、VadSegmented 归位（2c-3）、coordinator 清理（2d）。

---

## 1. 背景

cloud-asr-cli 第一步（已合并 main `bb967be`）为 cli/server 批处理新建了 `octopus-asr-cloud` crate：4 provider（Aliyun/ByteDance/Tencent/Baidu）WSS 协议层 + `CloudBatchEngine`，从 desktop `*_stream.rs` 1:1 复刻，**唯一改造是把 `open()` 内部从 `tauri::async_runtime::RuntimeHandle` + `rt.spawn` 改成 `tokio::spawn`**（去 tauri）。

结果：协议层字节级一致的两份副本同时存在——

| | desktop 副本（tauri 版） | cloud crate（tokio 版） |
|---|---|---|
| `*_stream.rs::open()` 第一参 | `&tauri::async_runtime::RuntimeHandle` | 无 |
| spawn 方式 | `rt.spawn(...)` | `tokio::spawn(...)` |
| 调用上下文要求 | 任意线程（tauri runtime 全局） | **须在 tokio runtime context** |
| 行数 | `*_stream.rs` 569/470/380/280 + `cloud_types.rs` 146 | 同名文件行数几乎一致（1:1 复刻） |

任何协议层改动（鉴权串、帧格式）此刻都要改两处，是必须尽快收口的技术债。cloud crate 已稳定（30 单测绿 + cli/server/desktop 批处理 e2e 通过），是收口时机。

## 2. 用户决策（brainstorming 2026-06-25）

1. **范围**：删 desktop 协议副本（`*_stream.rs` × 4 + `cloud_types.rs`），desktop `CloudPipelineEngine` 改指 cloud crate 协议层。`CloudPipelineEngine` 本身（流式适配）留 desktop。
2. **D1 runtime 兼容（核心）**：方案 **B**——cloud crate 零改动，desktop 用 `tauri::async_runtime::block_on` 进入 tokio context 后调 cloud crate 的 `open_cloud_session`。
3. **D2 类型归属**：desktop 全栈改用 `octopus_asr_cloud::{CloudStreamHandle, StreamEvent}`；cloud crate 加 `#[doc(hidden)] pub fn new_for_test()` 供 desktop 测试构造预载 handle。
4. **`engine_aliyun.rs` 不动**：它是 `AliyunEngine`（chunk 模式离线引擎，经 `engine_dispatch.rs` 用），与 `*_stream.rs` 长连接协议是两套（`aliyun_stream.rs:9` 文档自述）。

## 3. D1：runtime 兼容（方案 B）

### 3.1 问题

cloud crate 的 `open_cloud_session`（`config.rs:81`）同步返回 `CloudStreamHandle`，但各 provider `open()` 内部 `tokio::spawn`（如 `aliyun_stream.rs:53`）——**须在 tokio runtime 上下文调用**。

desktop `CloudPipelineEngine::tick` 在 coordinator 主线程（`std::thread`，**非 tokio context**）同步调 `open_cloud_session`（`cloud_pipeline.rs:307`）。desktop 现行副本靠 `tauri::async_runtime::handle()`（tauri runtime 全局、任意线程可 spawn）绕过；cloud crate 去 tauri 后该能力消失，直接调 `tokio::spawn` 会 panic（no reactor running）。

### 3.2 方案 B：cloud crate 零改动，desktop block_on 进 context

```rust
// desktop cloud_pipeline.rs：open_cloud_session 改为瘦 wrapper（保留，tick 调用点零改动）
pub(super) fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle, String> {
    // cloud crate 的 open_cloud_session 内部 tokio::spawn，须在 tokio context。
    // coordinator 主线程非 tokio，用 tauri runtime 的 block_on 进入（tauri runtime 即 tokio）。
    tauri::async_runtime::block_on(async {
        octopus_asr_cloud::open_cloud_session(asr_engine, language, pre_roll)
    })
    .map_err(|e| e.to_string())
}
```

### 3.3 为什么安全

- `tauri::async_runtime` 底层即 tokio multi_thread runtime；`block_on` 进入时设置 current context，使同步 `open_cloud_session` 内部的 `tokio::spawn` 可用。
- `open_cloud_session` 内部只 `spawn` 一条 reader task + 返回 mpsc channel 构造的 `CloudStreamHandle`（不 `await` 建连），future 立即 ready，`block_on` 立即返回、不阻塞 coordinator。
- tokio `Runtime::block_on` 在非 runtime 线程调用安全（coordinator 线程非 worker、未嵌套在 runtime 内，无 "can't call block_on from within async context" panic 风险）。

### 3.4 不选方案 A 的理由

方案 A（cloud crate `open()` 加 `tokio::runtime::Handle` 参数）虽然把 context 约束显式化，但：要改刚稳定的 cloud crate 4 个 stream + config + batch 签名（牵连 cli/server）；desktop 端仍要 `block_on` 拿 handle（tauri 不直接暴露底层 tokio Handle），没省掉 block_on。净亏。

## 4. D2：类型归属 + 测试构造器

### 4.1 全栈改用 cloud crate 类型

desktop 删 `cloud_types.rs` 后，所有 `CloudStreamHandle`/`StreamEvent` 引用改自 `octopus_asr_cloud`：
- `cloud_pipeline.rs`：`use crate::cloud_types::{CloudStreamHandle, StreamEvent};` → `use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};`
- `coordinator.rs`：close 路径用到的 `CloudStreamHandle`/`StreamEvent` 的 `use` 源改 `octopus_asr_cloud`（`take_close_handle` 返回类型、`Stage::CloudClosing` 的 `close_async` 调用随之）。

`PcmFrame` 是 `pub(crate)`，desktop 经 `push_pcm`/`finish` 间接用，不直接引用，无需改。

### 4.2 测试构造器（cloud crate 增量）

desktop `cloud_pipeline.rs` 有 5 个 drain 测试调 `CloudStreamHandle::new()`（其中 2 个经 `handle_with_events` helper、3 个直接调）构造预载 `StreamEvent` 的 handle 来测 `drain_cloud_session`（另有 3 个纯函数测试 `onset_confirmed`/`should_send_finish`/`take_preroll` 不用 handle）。drain 逻辑留 desktop，这些测试必须能在 desktop 构造预载 handle。

约束：cloud crate 的 `CloudStreamHandle::new()` 是 `pub(crate)`，且返回类型含 `mpsc::UnboundedReceiver<PcmFrame>`，而 `PcmFrame` 是 `pub(crate)`——直接把 `new()` 改 `pub` 会编译失败（pub fn 不能返回含私有类型的签名）。

**解决**：cloud crate 加一个只暴露 `pub` 类型的测试构造器（不泄露 `pub(crate) PcmFrame`）：

```rust
impl CloudStreamHandle {
    /// 仅供测试：构造 handle + result 发送端（预载事件用）。不暴露 pcm_rx / PcmFrame。
    #[doc(hidden)]
    pub fn new_for_test() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (handle, _pcm_rx, result_tx) = Self::new();
        (handle, result_tx)
    }
}
```

cloud crate 自身测试仍用 `pub(crate) new()`；desktop 测试改用 `new_for_test()`。`PcmFrame` 封装不动。这是 cloud crate 纯测试支持增量，不动协议逻辑。

## 5. D3：删除 / 改造清单

### 5.1 desktop 侧

| 动作 | 对象 | 备注 |
|---|---|---|
| 删文件 | `aliyun_stream.rs`、`bytedance_stream.rs`、`tencent_stream.rs`、`baidu_stream.rs`、`cloud_types.rs` | 5 个，cloud crate 1:1 |
| 删 `mod` | `main.rs` 的 `mod aliyun_stream;`/`mod bytedance_stream;`/`mod tencent_stream;`/`mod baidu_stream;`/`mod cloud_types;` | 5 行；`mod cloud_pipeline;`/`mod engine_aliyun;` 保留 |
| 改造 `cloud_pipeline.rs` | 见 5.2 | |
| `coordinator.rs` | **零改动**（靠类型推断：`take_close_handle`→`close_async`，`CloudStreamHandle` 类型由 cloud_pipeline 返回类型推断，无需 `use`） | close 路径 |
| 改造 `pipeline.rs` | `StreamingPipelineEngine::take_close_handle` trait 默认 + `StreamingPipeline` 包装方法签名 `crate::cloud_types::CloudStreamHandle` → `octopus_asr_cloud::CloudStreamHandle`（trait 与 impl 类型须一致，否则 E0053） | **实施盲点修正** |
| 改造 `engine_aliyun.rs` | `is_qwen_realtime_endpoint` + `samples_to_pcm_s16le` re-export 改指 `octopus_asr_cloud`（chunk 模式复用 cloud 协议层工具） | **实施盲点修正** |
| `Cargo.toml` | `cloud` feature 加 `octopus-asr-cloud`；可能瘦身 `tokio-tungstenite`/`uuid`/`base64`/`flate2`/`hmac`/`sha1` | plan 阶段 grep `use` 核实，仅当 desktop 删副本后不再直接用才删 |

### 5.2 `cloud_pipeline.rs` 改造明细

| 区块 | 动作 |
|---|---|
| `use` | `crate::cloud_types::{CloudStreamHandle, StreamEvent}` → `octopus_asr_cloud::{CloudStreamHandle, StreamEvent}`；删 `use tauri::async_runtime::RuntimeHandle;` |
| `resolve_cloud_entry` + `resolve_aliyun_config` + `resolve_bytedance_config` + `resolve_tencent_config` + `resolve_baidu_config`（5 个 fn，113-177） | **删**（cloud crate `config.rs` 有等价物） |
| `open_cloud_session`（181-213） | 改 3.2 的 block_on 瘦 wrapper（删 `RuntimeHandle` + `crate::*_stream::open` 分发，改为单行调 `octopus_asr_cloud::open_cloud_session`） |
| `CloudPipelineEngine`（216-399） | **逻辑零改动**（`session: Option<CloudStreamHandle>` 字段类型随 use 源变；`tick`/`finish_with_tail`/`reset`/`take_close_handle`/`current_partial`/`silence_duration`/`is_cloud` 不变） |
| `drain_cloud_session`/`onset_confirmed`/`should_send_finish`/`take_preroll`（38-108） | **逻辑零改动**（match `StreamEvent` 随 use 源变） |
| tests（401-569，共 8 个：5 drain + 3 纯函数） | 所有调 `CloudStreamHandle::new()` 处（`handle_with_events` helper + 3 个直接调）→ `CloudStreamHandle::new_for_test()`；3 个纯函数测试零改动；其余断言不变 |

### 5.3 cloud crate 侧

加 `CloudStreamHandle::new_for_test()`（4.2）+ 暴露两个 engine_aliyun 复用的 helper：`is_qwen_realtime_endpoint`（`aliyun_stream.rs` `pub(crate)`→`pub`）、`samples_to_pcm_s16le`（`cloud_types.rs` `pub(crate)`→`pub`）+ `lib.rs` 顶层 re-export `CloudStreamHandle`/`StreamEvent`/`samples_to_pcm_s16le`。协议逻辑（鉴权/帧/会话状态机）**零改动**。

### 5.4 依赖边界

```
octopus-desktop ──(cloud feature)──→ octopus-asr-cloud ──→ octopus-asr-local + octopus-infra
```

单向，无循环。`asr` 不依赖 `cloud`（cloud-asr-cli 第一步已确立）。desktop 仅在 `cloud` feature 开启时依赖 cloud crate。

## 6. 不在范围

- **`engine_aliyun.rs`**：`AliyunEngine`（chunk 模式离线引擎，`engine_dispatch.rs` 用）——实施时发现它**复用了** `aliyun_stream::is_qwen_realtime_endpoint` + `cloud_types::samples_to_pcm_s16le`（brainstorming 盲点，原以为零改动）。实际改指 `octopus_asr_cloud`（chunk 模式也复用 cloud 协议层工具，进一步消除重复）。
- **VadSegmented 归位（2c-3）**：独立设计。
- **coordinator 清理（2d）**：emit/DB/polish + transcript 全收敛进 pipeline。
- **cli/server 接入**：cloud-asr-cli 第一步已完成（批处理用 cloud crate），本次不改 cli/server。
- **cloud 协议层逻辑改动**：零行为差异搬迁，不动鉴权/帧格式/会话状态机。

## 7. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 方案 B 的 `block_on` 在 coordinator 主线程有"隐式须 context"约束 | 注释标明；`block_on` 包同步 fn 是进入 context 的标准手段，open 只 spawn+返回 channel handle 不阻塞；plan 含 e2e 回归验证 |
| cloud crate 加 `new_for_test()` 是封装小让步 | `#[doc(hidden)]` + 不暴露 `PcmFrame`（返回类型只含 `pub StreamEvent`）；仅测试用，非生产路径 |
| `Cargo.toml` cloud feature 瘦身可能误删仍被直接使用的 dep | plan 阶段 grep desktop src 确认每个 dep 的直接 `use`，仅删确认无直接引用的 |
| 删副本后 desktop cloud 编译/行为回归 | 双 feature 编译（cloud on/off）+ cloud_pipeline 8 测试 + cloud crate 31 测试 + 云端流式 e2e 回归（用户本地 key） |
| cloud crate 协议层与 desktop 副本字节级一致的前提 | cloud-asr-cli 第一步已验证 1:1 复刻（30 单测 + cli/server/desktop 批处理 e2e 通过）；改指后 e2e 回归再次确认 |

## 8. 验证清单

- [x] desktop `cargo build --features cloud` + `cargo build`（cloud off）双 feature 编译 0 error
- [x] desktop `cargo clippy --features cloud --all-targets` 本次新代码（cloud_pipeline/pipeline/engine_aliyun 改造 + cloud crate 可见性/re-export）0 新 warning。注：cloud crate 协议层（`*_stream.rs`）与 desktop（coordinator/transcript 等）有第一步遗留的预存 warning，非本次引入，不在范围
- [x] `cloud_pipeline.rs` 全部测试绿（8 个：5 个 drain 测试改用 `new_for_test` + 3 个纯函数测试零改动）
- [x] cloud crate 31 测试不变（加 `new_for_test` 不破坏）
- [x] `cargo check --workspace --all-targets` 0 error
- [x] **云端流式 e2e 回归**：desktop `--features cloud`，用户本地云端 key，本地流式 + 云端流式识别均正常（onset/partial/finish/close 全路径，2026-06-25 验证通过）

---

## 2026-06-25-vad-segmented-rehome-design

# 2c-3 VadSegmented 归位（统一 pipeline 角色）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已实施并 ff-merge main。Task 1-6（commit `cde1100`）双 feature 编译 0 error、新代码 clippy 0 新 warning；workspace 测试除 2 个 pre-existing infra 失败（`seed_then_load_round_trips` / `list_all_local_asr_models_includes_disabled`，seed `c796cbc` 重写后断言过时，与本次无关——2c-3 未触碰 `crates/infra/`）外全绿；VadSegmented 全路径 e2e 验证通过（2026-06-25）。
> **动机**：ASR pipeline 重构阶段2（spec `2026-06-25-archived-spec.md#asr-pipeline-design`）已收编流式（2a/2b/2c-1：`StreamingPipeline` 壳）+ cloud（2c-2：`StreamingPipelineEngine` trait + `CloudPipelineEngine`）。VadSegmented（非流式引擎的 VAD 分段伪流式）是阶段2 最后一块未归位的编排，散在 `coordinator.rs`（`handle_vad_segmented_tick` + `Stage::VadSegmented`/`WaitingCompletion` 两处 `TranscriptionDone` 乱序回填 handler）。本 spec 把它收进统一 `Pipeline` 角色，为 2d（coordinator 清理）铺路。
> **关联**：总 spec `2026-06-25-archived-spec.md#asr-pipeline-design`（§3.4 / §9 迁移映射）；2c-1 spec `2026-06-23-...`（`StreamingPipeline` 壳）；2c-2 spec `2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design`（`StreamingPipelineEngine` trait）。
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

