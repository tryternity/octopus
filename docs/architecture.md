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
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶） |

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

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/asr/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`；cli/server/desktop 共用）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count / duration_ms）
- **过程增量入库（schema v3）**：`transcriptions.id` = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入，去 `AUTOINCREMENT`），兼任主键 / 业务 key / 开始时间戳；`duration_ms = finalize_now_ms - id`。入库时机分散到识别过程各事件：首次有 ASR 文本 → `INSERT`（`insert_transcription_at_id`）；分段 / 流式 partial → `UPDATE raw_text`（`update_raw_text`）；停顿润色完成 → `UPDATE polished_text`（`update_polished`）；停止 → `finalize`（含 `duration_ms`，`finalize_transcription`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。v2→v3 migration DROP 重建（旧数据无所谓）。详见 [spec](superpowers/specs/2026-06-14-transcript-model-design.md)
- `models` 表：模型目录（**唯一来源**，首次建库时 `seed_default_models` 写入默认引擎集；v2 schema 无 `is_active` 列——引擎激活改由 `config.yaml.asr_engine` 决定，见「模型管理」）
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
- **应用行为配置** `config.yaml` → `infra::config::AppConfig`（`octopus_infra::config::load_config()`，18 字段：麦克风/引擎选择/分段/润色/LLM 等）。schema 统一定义在 infra，asr/desktop/cli 共享。
- **DB 模型目录** `~/.octopus/octopus.db` `models` 表 → `asr::config::AsrConfig`（`octopus_asr::config::load_config()`，首次 `db::ensure_db()` 自动建表 + seed，读后缓存到 `OnceLock`）。
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-transcript-model-design.md)。

**引擎选择（单一真相 = `config.yaml.asr_engine`）：**
- `models` 表不再有 `is_active` 列（v1→v2 migration 自动 `DROP COLUMN`，见 db.rs `init_schema`）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：DB name 精确匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。
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

## 技术栈

- **推理引擎**: ONNX Runtime（通过 ort crate）
- **音频处理**: cpal（录音）、rubato（重采样）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
