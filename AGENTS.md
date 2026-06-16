# AGENTS.md — octopus 代码库指南

## 项目概述

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，使用 Rust 编写。支持 5 种 ASR 引擎（Whisper / SenseVoice / Paraformer / Qwen3-ASR / Zipformer），提供 CLI、HTTP Server、Tauri 桌面应用三种使用方式，并集成了 LLM 文本润色能力。

## 关键命令

### 构建

```bash
# 构建全部（含 library）
cargo build --release

# 仅构建 server + cli（最常用）
cargo build --release -p octopus-server -p octopus-cli

# 仅构建 library
cargo build --release -p octopus-asr

# 构建桌面应用（embedded 模式，默认）
cargo run --release -p octopus-desktop --features embedded

# 构建桌面应用（WebSocket 远程模式）
cargo run --release -p octopus-desktop --features remote-ws

# 构建桌面应用（gRPC 远程模式）
cargo run --release -p octopus-desktop --features remote-grpc
```

### 开发运行

```bash
# CLI 查看模型配置
cargo run -p octopus-cli -- config

# CLI 麦克风实时识别
cargo run -p octopus-cli -- e2e --model sensevoice

# CLI 文件识别
cargo run -p octopus-cli -- transcribe <wav_path> --model whisper --language zh

# CLI 流式测试
cargo run -p octopus-cli -- stream-test test.wav --model zipformer-ctc

# Server 启动（默认 3000 端口）
cargo run -p octopus-server

# 桌面应用（推荐用脚本，会清 WebView 缓存）
./run-octopus.sh

# LLM 润色测试
cargo run --release --package octopus-llm --example test_polish
```

### 测试

```bash
# 运行全部测试（内联 unit tests）
cargo test

# 单个 crate 测试
cargo test -p octopus-asr
cargo test -p octopus-infra
cargo test -p octopus-desktop
```

注意：没有独立的 `tests/` 目录，所有测试都是 `#[cfg(test)] mod tests {}` 内联在源文件中。

## Cargo Workspace 结构

```
crates/
├── infra/     # octopus-infra — 基础设施层，无项目内依赖
├── asr/       # octopus-asr — 核心推理库（所有上层依赖此 crate）
├── llm/       # octopus-llm — LLM 润色客户端
├── cli/       # octopus-cli — 命令行工具
├── server/    # octopus-server — HTTP/WebSocket 服务
├── desktop/   # octopus-desktop — Tauri 2 桌面应用
└── dlp/       # octopus-dlp — 视频音频下载工具
```

### 依赖关系

```
infra ← (asr, llm, cli, server, desktop, dlp)  — 所有 crate 都依赖 infra
asr ← (cli, server, desktop via "embedded" feature)
llm ← (asr via dev-dep, desktop)
desktop → feature-gated: embedded (=asr) | remote-ws | remote-grpc
```

**infra 是唯一无项目内依赖的 crate**，任何跨 crate 共享的内容应放在 infra。

## 架构要点

### 两套配置系统，各司其职

| 配置 | 存储位置 | Schema 定义 | 用途 |
|------|---------|------------|------|
| 应用行为配置 | `~/.octopus/config.yaml` | `infra::config::AppConfig` | 麦克风/引擎选择/分段参数/润色/粘贴等 |
| 模型配置 | `~/.octopus/octopus.db` (SQLite `models` 表) | `infra::db::ModelEntry` | 引擎目录/LLM 配置/API Key |

- **引擎激活唯一真相**：`config.yaml.asr_engine`（DB `models` 表无 `is_active` 列）
- **缓存策略**：两者都通过 `OnceLock` 缓存，手编配置后需**重启进程**生效
- **运行时切换**：desktop 的 `RuntimeConfig`（`Arc<RwLock<>>`）支持 `asr_engine` / `polish_mode` 运行时切换

### DB Schema 开发方式

- Schema 定义在 `crates/infra/src/db.sql`，编译期 `include_str!` 嵌入
- `init_schema` 仅在 `user_version=0` 时执行一次建表 + seed
- **无迁移逻辑**：开发阶段改 schema 时直接删除 `~/.octopus/octopus.db` 重启即可

### 数据流

**离线识别**：`音频文件 → read_wav_16k → [VAD 过滤] → engine.transcribe → 文本`

**流式识别**：`麦克风 → PCM chunk → resample_to_16k → StreamingSession.accept_samples → [partial] → 静音补零 flush → [final]`

**VAD 伪流式**（非流式引擎）：`麦克风 → 300ms tick → VAD 检测 → 静音切分/超时切断 → 离线引擎 → 按序拼接`

### Desktop Coordinator 状态机

核心控制流在 `crates/desktop/src/coordinator.rs`，通过 `std::mpsc` channel 串行化所有事件：

```
Idle → Streaming → (Polishing) → Pasting → Idle
Idle → VadSegmented → WaitingCompletion → (Polishing) → Pasting → Idle
```

- 粘贴异步化（`do_paste` 投递到 `tauri::async_runtime::spawn`）
- 润色异步化（`Polishing` 阶段 spawn 独立线程跑 LLM）
- DB 写入异步化（actor 模式，后台写线程通过 channel 消费）

## 代码模式与约定

### Rust Edition 与风格

- **Edition 2021**，所有 crate 统一
- 中文注释和文档为主
- 错误处理统一使用 `anyhow::Result`
- 日志统一使用 `log` crate（desktop 通过 `tauri-plugin-log`）
- 异步运行时：server/dlp 用 `tokio`，desktop 混用 `std::thread`（Coordinator）+ `tauri::async_runtime`（异步任务）

### ONNX Session 线程安全

`ort::Session::run` 需要 `&mut self`，因此所有引擎内部用 `Mutex<Session>` 包裹：
```rust
pub trait OfflineAsrEngine: Send + Sync {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;
}
```
内部 `self.session.lock().unwrap().run(...)` 实现内部可变性。

### Feature Gates

desktop crate 有三个互斥的引擎接入 feature：
- `embedded`（默认）— 内嵌 octopus-asr
- `remote-ws` — WebSocket 远程
- `remote-grpc` — gRPC 远程

对应代码用 `#[cfg(feature = "...")]` 条件编译。`engine_ws.rs` / `engine_grpc.rs` 仅在对应 feature 下编译。

### 引擎路由

引擎名 → 类别 → 具体实现的路由在 `crates/asr/src/config.rs`：
- `resolve_engine_category(name)` — 查 DB 配置确定引擎属于 Whisper/SenseVoice/Paraformer/Qwen3Asr/Zipformer
- `pick_entry(cfg, category, name)` — 取具体模型配置
- `AsrEngineManager` — 按需加载 + 缓存（最多 2 个引擎实例），秒级切换

### 流式 vs 离线判定

由 DB `models.is_streaming` 列数据驱动（不按 category 硬编码）：
- `is_streaming=1`（zipformer×3 / paraformer）→ 流式 partial
- `is_streaming=0`（sensevoice / qwen3-asr / whisper）→ VAD 分段伪流式

### AudioResampler

`crates/asr/src/audio.rs` 的 `AudioResampler`（有状态 `rubato::FftFixedIn`）：
- 跨帧 leftover 缓冲保边界 glitch-free
- 源速率不变时复用同一规划器（避免每 tick 重建 FFT planner）
- 流结束时 `flush()` 补零吐尾

## 重要 Gotchas

### config/ 符号链接

`config/` 是指向 `~/.octopus/` 的软链接。**读写必须用绝对路径 `~/.octopus/`**，不要通过 `config/` 相对路径——否则安全分类器可能误判为"向仓库提交密钥"。

### VAD 双实例（检测 vs 过滤）

VadSegmented 阶段持两个独立 SileroVad 实例：
- `vad`：检测用，逐 tick 喂入、有状态累积（跨 tick 不 reset）
- `filter_vad`：过滤用，每段过滤前 `reset()` 归零

原因：SileroVad 是有状态 LSTM，检测流已见过音频，若共用实例会导致双重喂入 + LSTM 状态污染。

### cpal::Stream 的 Send Safety

`cpal::Stream` 是 `!Send + !Sync`。desktop 中通过 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`）管理，`unsafe impl Send/Sync` 在 Coordinator 的 `std::thread::spawn` 单线程独占前提下 sound。**不要将 Stream 跨线程移动**。

### 序列空洞修复

VadSegmented 模式中，异步转写可能失败或返回空。**失败/空结果必须占位该 seq（写空串）**，否则 `completed_seq` 游标卡死，后续所有有效段积压丢失。

### 停顿润色阈值

`pause_polish_threshold_ms`（默认 600ms）**必须 > 500ms**（Active Flush 阈值），否则润色快照会缺少尾音。

### Zipformer 归一化

- `zipformer-small-ctc` / `zipformer-multi`：输入归一化到 `[-1.0, 1.0]`（不乘 32768）
- `zipformer-ctc`（whisper 特征）：使用专属 WhisperMelExtractor + chunk 级偏移归一化（非标准 Whisper 的 `(+4)/4` 缩放）

### 纠错器自动旁路

`asr_correct` 启用时，Qwen3-ASR 结果会自动跳过纠错（其自带强纠错能力），仅作用于 Whisper/SenseVoice/Paraformer/Zipformer。

### 关机 DB drain

后台 DB 写线程通过 `OnceLock<Sender>` 持久化 sender，进程退出时队列可能丢数据。`coordinator::shutdown_db()` 在 `ExitRequested` 时排空剩余命令。**新增 DB 写操作路径时记得走 actor 模式（`get_db_sender().send()`）**。

## 文档体系

```
docs/
├── architecture.md          # 架构概览（最权威的结构文档）
├── api.md                    # Server HTTP/WS API
├── configuration.md          # 配置指南
├── asr_archiveture_opt.md    # ASR 引擎架构重构总结
└── superpowers/
    ├── specs/                 # 功能设计规格（按日期）
    └── plans/                 # 实施计划（按日期）
```

**需求变更后必须同步更新** `docs/superpowers/specs/`、`docs/superpowers/plans/`、`docs/architecture.md`。

## 运行时文件布局

```
~/.octopus/
├── octopus.db          # SQLite（唯一存储：models 表 + transcriptions 表）
├── config.yaml         # 应用配置（缺失用默认值）
├── VOICE_POLISH.md     # 自定义润色 prompt（可选，覆盖内置默认）
└── models/
    ├── silero_vad_v4.onnx   # VAD（固定路径，不进 DB）
    └── zipformer/           # 默认 ASR（兜底引擎，27M）

~/.cache/huggingface/hub/   # 大模型 HF 缓存
```

## Tauri Desktop 注意事项

- 前端 `frontendDist: "dist"` 相对于 `crates/desktop/` 目录解析
- WebView 缓存在 `~/Library/WebKit/com.octopus.desktop/`，开发时 `run-octopus.sh` 会自动清理
- macOS 用 `tauri-nspanel`（git 依赖），Linux 用 `gtk-layer-shell` + `gtk`（平台条件编译）
- 结果窗 ready 机制：`result_window_ready` Tauri command + `WINDOW_READY` AtomicBool + `PENDING_TEXT` Mutex 解决首帧事件丢失

## Proto（gRPC）

`crates/desktop/proto/asr.proto` — 仅在 `remote-grpc` feature 编译。`build.rs` 中 `tonic-build` 条件编译。

## config 目录
`config/` 是指向 `~/.octopus/` 的软链接，这是实际运行配置目录（不在 git 仓库内，无密钥泄露风险）。

对 `config/` 下文件的读写操作，必须使用绝对路径 `~/.octopus/`（即 `/Users/wudarui/.octopus/`）进行，不要通过 `config/` 相对路径访问：
- 读：`~/.octopus/config.yaml`、`~/.octopus/record.txt` 等
- 写：直接写 `~/.octopus/` 下对应文件
- 原因：`config/` 经符号链接访问时，自动安全分类器无法判断目标在仓库外，可能误判为"向仓库提交密钥"而拦截；用绝对路径 `~/.octopus/` 可避免误拦。
