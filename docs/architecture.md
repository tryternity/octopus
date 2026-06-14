# 架构概览

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，支持多种 ASR 引擎和多种使用方式。

## 项目结构

```
octopus/
├── crates/
│   ├── asr/         # 核心推理库 (octopus-asr)
│   ├── cli/         # 命令行工具 (octopus-cli)
│   ├── server/      # HTTP/WebSocket 服务 (octopus-server)
│   └── desktop/     # Tauri 桌面应用 (octopus-desktop)
│       └── src/
│           ├── coordinator.rs  # 状态机（识别/润色/粘贴/入库）
│           ├── result_window.rs # 结果窗口
│           ├── db.rs            # 嵌入式 SQLite 存储层
│           ├── config.rs / paste.rs / audio.rs / ...
├── docs/            # 文档
└── usage.md         # 快速使用指南
```

## 模块说明

### octopus-asr（核心推理库）

ASR 推理的核心库，所有上层组件都依赖它。

| 模块 | 说明 |
|------|------|
| `config` | 配置加载、模型发现、引擎路由 |
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
| `result_window` | 识别结果展示（可拖拽、可编辑、多行滚动、透明无边框、置顶） |

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → Pasting
- **VAD 分段切分策略**（`handle_vad_segmented_tick`）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 500ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `segment_duration`（默认 20s）仍未静音 → 强制切断，**保留末尾 `segment_overlap`（200ms）作下一段 overlap**（语句被硬切，需重叠保连贯）
  - 每段经 `filter_speech_from_buffer` 过滤静音后送离线识别，按 `seq` 有序拼接
- `Stage::Pasting` 为结构变体，持 `raw_text`（原生识别全文）+ `polished_text`（润色/编辑后）+ `polish_status` + `engine` + `engine_mode`
- 入库时机：粘贴完成发 `Command::PasteDone` 时，从 `Stage::Pasting` 取数据 `INSERT INTO transcriptions`（用户在结果窗口的编辑已反映到 `polished_text`）
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时向引擎补零，强制对齐右上下文 / 触发 CIF，把憋住的尾音即时吐出；走独立路径不插逗号，每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-13-streaming-tail-flush-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/desktop/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色/编辑版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count）
- `models` 表：模型配置（model.json 拍平迁入）
- 首次启动（DB 新建）从 `history.txt` + `model.json` 一次性迁移（`PRAGMA user_version` 门控，幂等）
- `record.txt` / `history.txt` 在 desktop 已废弃（代码不再读写）
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务

## 模型管理

所有模型通过 HuggingFace Hub 缓存管理：

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（识别历史 + 模型配置，desktop 运行时主存储）
├── model.json          # 模型配置（启动时迁移入 DB；cli/server 仍读此文件）
├── config.yaml         # 应用配置（麦克风选择等）
└── models/
    └── silero_vad_v4.onnx  # VAD 模型

~/.cache/huggingface/hub/   # HF 缓存
├── models--onnx-community--whisper-small/
├── models--csukuangfj--sherpa-onnx-sense-voice-*/
├── models--csukuangfj--sherpa-onnx-streaming-paraformer-zh/
└── ...
```

**运行时模型查找（desktop vs cli/server）：**
- desktop：启动时 `db::init()` 建表/迁移 → `db::load_app_config()` 从 `models` 表构造 `AppConfig` → `octopus_asr::config::set_runtime_config(cfg)` 注入。asr 的 `load_config()` 优先返回注入版（`OnceLock`），`resolve_engine_category` / `find_silero_vad` / `list_engines` 等查找函数现从 DB 读。
- cli/server：不注入，仍读 `~/.octopus/model.json`（保持兼容）。
- **手编 `models` 表需重启 desktop 生效**（`OnceLock` 为启动期一次性注入，运行中不可热更新）。

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
