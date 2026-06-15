# 架构概览

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，支持多种 ASR 引擎和多种使用方式。

## 项目结构

```
octopus/
├── crates/
│   ├── infra/       # 基础设施 (octopus-infra) — 常量 + octopus_config_home，无项目内依赖
│   ├── asr/         # 核心推理库 (octopus-asr) — 含 db.rs（SQLite：模型配置+识别历史）
│   ├── llm/         # LLM 润色 (octopus-llm)
│   ├── cli/         # 命令行工具 (octopus-cli)
│   ├── server/      # HTTP/WebSocket 服务 (octopus-server)
│   ├── desktop/     # Tauri 桌面应用 (octopus-desktop)
│   └── dlp/         # 模型下载工具 (octopus-dlp)
├── docs/            # 文档
└── usage.md         # 快速使用指南
```

## 模块说明

### octopus-infra（基础设施）

无项目内依赖的最底层 crate，承载跨 crate 共享的基础设施：`consts`（固定路径常量：VAD 模型 / 默认 ASR 模型目录 / 润色 prompt 文件名）+ `paths`（`octopus_config_home()` 返回 `~/.octopus`，三端统一不再各自定义）+ `config`（`AppConfig`——config.yaml 的**统一 schema** 与 `load_config()` 读取，asr/desktop/cli 共享，多余字段对各端无害）。未来加时间工具等。任何项目 crate 都可依赖它。

### octopus-asr（核心推理库）

ASR 推理的核心库，所有上层组件都依赖它。

| 模块 | 说明 |
|------|------|
| `config` | DB 模型配置加载（`AsrConfig`）、模型发现、引擎路由、全局默认引擎兜底解析（`resolve_active_engine`） |
| `audio` | WAV 读取、重采样（→16kHz）、VAD 语音过滤 |
| `vad` | Silero VAD 语音活动检测 |
| `whisper` | Whisper 离线识别 |
| `sensevoice` | SenseVoice 离线识别 |
| `paraformer` | Paraformer 离线识别 |
| `qwen3_asr` | Qwen3-ASR 离线识别 |
| `zipformer` | Zipformer 离线识别 |
| `streaming_paraformer` | Paraformer 流式识别 |
| `streaming_zipformer` | Zipformer 流式识别 |
| `corrector` | 基于拼音映射和 Bigram 转移概率的轻量级中文拼音纠错与热词校正 |


**数据流（离线）：**
```
音频文件/WAV → read_wav_16k → [VAD 过滤] → 引擎.transcribe → 文本
```

**数据流（流式）：**
```
麦克风 → PCM chunk → resample_to_16k → 引擎.accept_samples → [partial]
                                    └─ 静音≥0.5s → 引擎.flush（补零吐尾音，无逗号）→ [partial]
                                                              → engine.finish → [final]
```

### octopus-cli（命令行工具）

通过 clap 提供 5 个子命令：

| 命令 | 说明 |
|------|------|
| `devices` | 列出可用麦克风 |
| `config` | 显示模型发现信息 |
| `transcribe` | WAV 文件离线识别 |
| `e2e` | 麦克风实时识别（离线/流式） |
| `stream-test` | WAV 文件流式识别测试 |

### octopus-server（HTTP 服务）

基于 Axum 的 Web 服务，提供 REST 和 WebSocket 接口。

```
Client ──HTTP POST──→ /transcribe ──→ octopus-asr ──→ JSON 响应
Client ──WebSocket──→ /ws/stream  ──→ VAD + ASR   ──→ 流式 JSON
```

### octopus-desktop（桌面应用）

基于 Tauri 2 的桌面应用，支持系统托盘、全局快捷键、悬浮窗、流式识别。

**识别模式：**

| 模式 | 引擎 | 说明 |
|------|------|------|
| 流式 | Paraformer, Zipformer | 边说边识别，600ms tick 驱动 |
| 离线 | SenseVoice, Whisper, Qwen3-ASR | VAD 分段伪流式，300ms tick 驱动，阈值可配置 |

**窗口管理：**

| 窗口 | 用途 |
|------|------|
| `recording_overlay` | 录音/识别状态提示（离线模式） |
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶）。顶部悬停工具栏：鼠标移入展开（窗口高度 100→132px），移出收起；4 个工具——设置 / 润色模式切换 / ASR 引擎切换 / LLM 模型切换（前二者+ASR 已接通，设置与 LLM 模型为占位） |

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → Pasting
- **VAD 分段切分策略**（`handle_vad_segmented_tick`）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 500ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `segment_duration`（默认 20s）仍未静音 → 强制切断，**保留末尾 `segment_overlap`（200ms）作下一段 overlap**（语句被硬切，需重叠保连贯）
  - 每段经 `filter_speech_from_buffer` 过滤静音后送离线识别，按 `seq` 有序拼接
- **Transcript 文本状态机**：识别文本状态由 `Transcript` 结构（`crates/desktop/src/transcript.rs`）统一管理——内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 `raw`（停顿快照，润色基准）/ `increase`（停顿后增量），避免维护三份字符串。`Stage::Streaming` / `VadSegmented` / `WaitingCompletion` 各持 `transcript: Transcript` 字段，文本流经 Transcript 方法（`set_full` / `append_segment` / `display_text` / `db_text`）。`Stage::Pasting` 仍为结构变体，持 `id` + `raw_text` + `polished_text` + `polish_status` + `engine` + `engine_mode`（由停止时从 Transcript 取出构造）。详见 [spec](superpowers/specs/2026-06-14-transcript-model-design.md)
- **停顿驱动润色**：流式 / 伪流式统一——静音 ≥ `pause_polish_threshold_ms`（默认 600ms，可配置）/ 伪流式段边界完成时，把当前完整 ASR 快照（`snapshot_for_polish()` = `raw + increase`）送 LLM 全量润色（mode=2 only），**不重置流式引擎**（只读快照送 LLM，引擎状态原样保留）。修复了流式中间润色 P0（partial 全量覆盖 polished）。默认 600ms > Active Flush 500ms（用户配置需保持 > 500ms，否则润色先于尾音冲刷、快照缺尾音），润色在 tick 流程最末执行，快照可靠
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时向引擎补零，强制对齐右上下文 / 触发 CIF，把憋住的尾音即时吐出；走独立路径不插逗号，每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-13-streaming-tail-flush-design.md)
- **运行时配置子系统（RuntimeConfig）**：工具栏可运行时切换 `asr_engine` / `polish_mode`，无需重启。`runtime_config.rs` 提供 `SharedRuntimeConfig`（`Arc<RwLock<RuntimeConfig>>`，挂 `tauri::State`）作为这两个字段的**可变运行时镜像**，与启动只读的 `AppConfig` 快照互补。4 个 Tauri 命令（`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode`）读写镜像 + best-effort 持久化回 `~/.octopus/config.yaml`（写盘失败仅 `warn`，本次仍生效、重启回退）。Coordinator 闭包持镜像句柄，**在 Toggle 进入 `Idle` 时重读 `asr_engine` 重建引擎**（下次录音生效）；**每个 tick 重读 `polish_mode` 并 `Transcript::set_mode`**（立即生效，下一次润色按新模式）。详见 [spec](superpowers/specs/2026-06-15-result-window-toolbar-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`；asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop 共用）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count / duration_ms）
- **过程增量入库（schema v3）**：`transcriptions.id` = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入，去 `AUTOINCREMENT`），兼任主键 / 业务 key / 开始时间戳；`duration_ms = finalize_now_ms - id`。入库时机分散到识别过程各事件：首次有 ASR 文本 → `INSERT`（`insert_transcription_at_id`）；分段 / 流式 partial → `UPDATE raw_text`（`update_raw_text`）；停顿润色完成 → `UPDATE polished_text`（`update_polished`）；停止 → `finalize`（含 `duration_ms`，`finalize_transcription`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。v2→v3 migration DROP 重建（旧数据无所谓）。详见 [spec](superpowers/specs/2026-06-14-transcript-model-design.md)
- `models` 表：模型目录（**唯一来源**，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed 默认引擎集；含 `is_local` / `is_enabled` / `is_streaming` 列——`load_models_at` 仅读 `domain='asr' AND is_enabled=1`，`domain='llm'` 经 `load_llm_model` 读；引擎激活由 `config.yaml.asr_engine` 决定，无 `is_active` 列，见「模型管理」）
- `model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由 `check_and_trigger_polish` 在停顿点触发（流式静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界），把 `Transcript.snapshot_for_polish()`（完整 ASR）送 LLM 全量润色，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）；最终润色在 `Stage::Pasting` 入口（`start_pasting`）。详见 [设计](superpowers/specs/2026-06-14-transcript-model-design.md)。
- 停止空文本边界：Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音），`start_pasting` 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `overlay::hide_overlay` + `tray → Idle` 三类 UI 反馈（缺一则"正在聆听…"框残留）。详见 [设计 §4.5](superpowers/specs/2026-06-12-squid-desktop-design-v2.md)。

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务

## 模型管理

模型配置**唯一来源**是 `~/.octopus/octopus.db` 的 `models` 表。小模型（VAD + 默认 ASR）随应用打包到固定路径，开箱即用；大模型按需从 HuggingFace 下载到缓存。

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（models 表 + transcriptions 表，唯一存储）
├── config.yaml         # 应用配置（麦克风/引擎选择/分段参数等）
└── models/             # 随应用打包的小模型（固定路径）
    ├── silero_vad_v4.onnx   # VAD（1.8M，find_silero_vad 固定加载）
    └── zipformer/           # 默认 ASR（27M，model.int8.onnx + tokens.txt）

~/.cache/huggingface/hub/   # 大模型 HF 缓存（whisper/sensevoice/qwen3/paraformer 等，按需下载）
```

**模型目录解析（`config::resolve_model_dir`）** —— source 字段双模式：
- 本地相对路径（如 `models/zipformer`）→ `~/.octopus/<source>`（随应用打包的小模型）
- HF repo 名（如 `onnx-community/whisper-small`）→ `~/.cache/huggingface/hub/`（大模型缓存）

**两份配置，各司其职：**
- **应用行为配置** `config.yaml` → `infra::config::AppConfig`（`octopus_infra::config::load_config()`，22 字段：麦克风/引擎选择/分段/润色/LLM/粘贴/硬件加速/ASR 纠错等）。schema 统一定义在 infra，asr/desktop/cli 共享。
- **DB 模型目录** `~/.octopus/octopus.db` `models` 表 → `asr::config::AsrConfig`（`octopus_asr::config::load_config()`，首次 `db::ensure_db()` 自动建表 + seed，读后缓存到 `OnceLock`）。
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-transcript-model-design.md)。

**引擎选择（单一真相 = `config.yaml.asr_engine`）：**
- `models` 表无 `is_active` 列（开发期 schema 变更采用删库重初始化——见 `crates/infra/src/db.sql` 注释；`init_schema` 仅 `user_version=0→1` 一次性建表 + seed，不做 migration）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：DB name 精确匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。
- **流式判定数据驱动**：是否走流式识别由 `models.is_streaming` 列决定——`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming`（seed：zipformer×3 + paraformer = 流式；whisper / sensevoice / qwen3-asr×2 = 非流式），不再按 category 硬编码匹配。Coordinator 的 `use_streaming` 据此在 Toggle 进入 `Idle`（切引擎 / 切模式）时重算——流式引擎走流式 partial，非流式引擎自动回退 VAD 分段伪流式。
- 显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，按 name 精确匹配、**不走兜底**（匹配不到直接报错）。
- VAD 模型固定路径（`find_silero_vad` 直接返回 `~/.octopus/models/silero_vad_v4.onnx`），不进 DB、不读配置。
- **手编 `models` 表 / `config.yaml` 需重启进程生效**（`OnceLock` 缓存，运行中不可热更新）。

## 支持的 ASR 引擎

| 引擎 | 类型 | 特点 |
|------|------|------|
| Whisper | 离线 | 多语言，auto 检测 |
| SenseVoice | 离线 | 快速，自动语言检测 |
| Paraformer | 离线/流式 | 中文优化 |
| Qwen3-ASR | 离线 | 大模型能力 |
| Zipformer | 离线/流式 | 轻量级 CTC |

## 拼音纠错与热词校正 (ASR Corrector)

为了在不引入重型深度学习模型（如 MacBERT 等动辄几百 MB 的模型）的前提下，实现极致轻量的纠错与专有名词（热词）校正，项目实现了一套基于 **“拼音映射 + 长度归一化 Bigram 转移概率”** 的轻量级后处理纠错引擎。

### 核心特性
- **纯静态与轻量化**：纠错所需的 unigram 词表与 bigram 共现表（各精简至高频的前 40,000 条，压缩后约 450KB）直接通过 `include_bytes!` 静态嵌入二进制中，无需额外网络下载，运行时解压，额外内存占用约 30MB。
- **配置开关控制**：由 `config.yaml` 中的 `asr_correct` 字段控制（默认 `false`）。
- **智能排除**：由于 Qwen3-ASR (0.6B/1.7B) 模型本身输出带有标点且语义纠错能力强，纠错引擎会自动跳过对 Qwen3-ASR 结果的处理，仅应用于 Whisper、SenseVoice、Paraformer 和 Zipformer。

### 纠错算法逻辑
1. **滑窗候选召回 (Sliding Window)**：使用 2 字和 3 字的字符滑窗扫描识别出的文本，通过拼音库计算滑窗文本的拼音，并在此拼音的 $O(1)$ 模糊拼音倒排索引（支持南方口音混淆，如 `zh/ch/sh` <-> `z/c/s`、`in/en` <-> `ing/eng`、`n` <-> `l` 等）中召回**相同字符长度**的同音/近音候选词。
2. **长度归一化打分 (Length Normalization)**：利用未登录词（typo）容易被 `jieba` 拆碎分词的特性，评估替换后的句子，并使用 **“句子总 log 概率 / 分词后 Token 数量”** 对句子的语言模型得分进行归一化，彻底消除倾向于更短分词结果的长度偏置。
3. **基于 Jieba 字典的自适应惩罚**：
   - 如果原滑窗词是 Jieba 字典中的已登录词（即 `jieba.cut().len() == 1`，说明它是合法的词，如 `"坐上"`），系统施加极高的修改惩罚（`-1.5`）以保护正确表述不被误改；
   - 如果原滑窗词是未登录词（typo，如 `"以经"` 被 Jieba 拆分为 `"以"` 和 `"经"`），则修改惩罚降低（`-0.2`）以积极纠错。

## ASR 硬件加速与自动降级机制 (ASR Hardware Acceleration & Fallback)

为了最大化利用用户本机的 GPU 资源加速语音识别，同时避免因显卡驱动或算子不支持导致应用程序崩溃，系统在 `octopus-asr` 核心引擎中实现了一套手自动一体的硬件加速及平滑降级机制。

### 核心特性
- **手动控制开关**：在 `config.yaml` 中提供 `asr_hardware_accelerated` 字段（`bool` 类型，默认 `false`）。用户如果不需要加速，或者大模型加速不稳定时，可随时降级回退到纯 CPU 推理。
- **多平台加速后端支持**：通过 `ort` (ONNX Runtime) crate 的硬件加速接口，自动支持多平台主流 EP (Execution Provider) 注册：
  - **macOS**: 自动尝试使用 `CoreML` 执行提供商进行加速。
  - **Windows/Linux**: 自动尝试使用 `CUDA` 和 `DirectML` 进行 GPU 加速。
- **平滑降级机制 (CPU Fallback)**：
  在初始化推理 Session 时，若检测到 `asr_hardware_accelerated: true`，SessionBuilder 会动态尝试注册对应的 GPU EP。如果系统驱动不兼容、加速库文件缺失，或模型自身包含硬件加速器不支持的特殊算子（例如 `Qwen3-ASR` 由于含有复杂动态 Shape 算子，在部分平台的 CoreML 加速启动时会被拦截限制），构建器会捕获该错误、打印 Warning 日志，并**自动且无缝地重构出一个纯 CPU 的 Session**，保证语音识别服务不发生闪退或中断。
- **VAD 免加速策略**：
  由于 VAD (Silero VAD) 模型的体积极其微小 (1.8MB)，且对实时性要求极高。将其调度至 GPU 进行加速所产生的显存交互与上下文切换开销（Latency Overhead）远超加速本身带来的收益。因此，**VAD 推理固定运行在 CPU 端**，完全不受 `asr_hardware_accelerated` 字段的影响。

## 技术栈

- **推理引擎**: ONNX Runtime（通过 ort crate）；可选硬件加速——CUDA/DirectML/CoreML execution provider（由 `config.yaml.asr_hardware_accelerated` 控制，默认 `false`，注册失败自动回退 CPU），VAD 不受影响（固定 CPU）。config 经 `APP_CONFIG` OnceLock 缓存避免每次 session 构建重复读 yaml。详见 [spec](superpowers/specs/2026-06-15-asr-hardware-acceleration-design.md)
- **音频处理**: cpal（录音）、rubato（重采样）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
