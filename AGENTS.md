# AGENTS.md — octopus 代码库指南

## 项目概述

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，使用 Rust 编写。提供 CLI、HTTP Server、Tauri 桌面应用三种使用方式，并集成了 LLM 文本润色能力。

**架构、功能、技术细节以 [`docs/`](docs/) 下文档为唯一真相源**——本文件只描述项目结构和开发流程，避免重复维护导致文档滞后。

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

## 开发流程（文档驱动）

**一切以文档为基础继续开发，保持文档与代码同步。** 架构、功能、技术细节以 [`docs/`](docs/) 下文档为唯一真相源——遇到「代码怎么实现」「架构怎么组织」「流程怎么走」的问题，**先查文档，不猜代码**；文档没覆盖或描述过时，先修文档再改代码。

### 大需求（新功能 / 架构调整 / 接口变化）

必须完整经过 superpowers 工作流，**不得跳步**：

1. **brainstorming** — 用 `superpowers:brainstorming` skill 充分探讨需求、用户意图、设计取舍，确认方向后再动手
2. **写 spec** — 在 `docs/superpowers/specs/YYYY-MM-DD-<feature>-design.md` 记录设计（功能、架构、接口、不变量、降级路径）
3. **写执行计划** — 在 `docs/superpowers/plans/YYYY-MM-DD-<feature>.md` 分解任务（每任务含文件、变更点、验证命令）
4. **实现代码** — 按计划逐任务实现，每任务后跑验证命令
5. **review plan（强制）** — 实现完成后必须回看 plan，把实际偏差、新增决策、删除/合并的子任务回写到 plan。**plan 是「实施记录」而非「一次性待办」**，最终必须反映实际实现

### 小需求（bug fix / 参数调整 / 文案修改）

实现前 **review 相关 spec 和 plan**：
- 找到对应 spec / plan，检查设计描述是否仍然成立
- 及时修改文档中过时的地方（参数变了、阈值变了、流程变了）
- 没有对应 spec 的小改动，至少更新 `docs/architecture.md` 相关章节

### 文档同步（强制）

代码变更完成后（或同时）必须同步文档：
- 架构概览：[`docs/architecture.md`](docs/architecture.md) — 最权威的结构文档，任何架构 / 流程 / 模块变化都要更新
- 规格文档：`docs/superpowers/specs/` — 功能设计、架构、接口
- 实施计划：`docs/superpowers/plans/` — 实施步骤、任务分解

### 混淆即讨论

如果代码实现与文档描述出现冲突，或文档描述含糊不清导致多种解读：
- **及时提出讨论**，不要自行假设继续推进
- 讨论澄清后回写到对应文档，避免下次重复混淆

## 文档体系

```
docs/
├── architecture.md          # 架构概览（最权威的结构文档）
├── api.md                    # Server HTTP/WS API
├── configuration.md          # 配置指南
├── asr_archiveture_opt.md    # ASR 引擎架构重构总结
└── superpowers/
    ├── specs/                 # 功能设计规格（按日期，大需求必备）
    └── plans/                 # 实施计划（按日期，大需求必备）
```

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

## config 目录

`config/` 是指向 `~/.octopus/` 的软链接，这是实际运行配置目录（不在 git 仓库内，无密钥泄露风险）。

对 `config/` 下文件的读写操作，必须使用绝对路径 `~/.octopus/`（即 `/Users/wudarui/.octopus/`）进行，不要通过 `config/` 相对路径访问：
- 读：`~/.octopus/config.yaml`、`~/.octopus/record.txt` 等
- 写：直接写 `~/.octopus/` 下对应文件
- 原因：`config/` 经符号链接访问时，自动安全分类器无法判断目标在仓库外，可能误判为"向仓库提交密钥"而拦截；用绝对路径 `~/.octopus/` 可避免误拦。

## 重要 Gotchas

### Zipformer Whisper 特征归一化（已踩 3 次坑，勿再改错）

Transducer 系列（`zh-int8-2025-06-30` / `zh-xlarge-int8-2025-06-30`）和 `zipformer-ctc` 使用 whisper 特征（ONNX metadata `feature=whisper` → `is_whisper=true`）。`normalize_whisper_features` 有 3 个关键约束，全部来自 sherpa-onnx C++ 源码（`sherpa-onnx/csrc/math.cc::NormalizeWhisperFeatures`），**修改前务必先读参考实现**：

1. **公式不可变**：最后一步 `(clamped + 4.0) / 4.0`（范围~0-2）。曾错误写成 `clamped - clamp_min`（范围 0-8，尺度差 4 倍）→ ONNX 模型输入分布不匹配 → 输出乱码。

2. **流式必须 per-chunk 归一化**：每个 chunk 切片后**独立** normalize，不是对整段特征全局归一化。sherpa-onnx 的 `online-recognizer-transducer-impl.h` 就是 per-chunk 调 `NormalizeFeatures`。曾误改为 pseudo-global（每次重算 history+buffer 全局归一化），方向完全错误——`history_samples` 每 tick 内容不同导致 max_v 跨 tick 不稳定。

3. **Transducer `history_samples` 仅保留最后 1 帧**（`Z_FRAME_SHIFT` = 160 samples），与 CTC 引擎一致。曾错误保留全部未消费样本（可达上万），导致每次重算特征时归一化 max_v 剧烈跳变 + $O(N^2)$ 性能崩坏。

**诊断方法**：如果流式 Transducer 输出乱码（"回 月 因 同"式重复 token），对照 sherpa-onnx 命令行输出验证——同一段音频，如果 sherpa-onnx 正常但我们的乱码，必定是上述 3 点之一。

### Paraformer Fbank 特征提取（5 个必做步骤，缺一即乱码）

流式 Paraformer 的 fbank 特征提取必须与 sherpa-onnx `kaldi-native-fbank` 完全一致，否则输出 token 重复（`thedayday`/`tomtomor`）或英文粘连。**5 个步骤缺一不可**：

1. **DC offset removal**（`remove_dc_offset=true`）— 每帧 FFT 前减帧均值
2. **Pre-emphasis**（`preemph_coeff=0.97`）— `y[i]=x[i]-0.97*x[i-1]`。**无跨帧状态**：帧重叠（shift=160 < len=400），上一帧末尾并非本帧 start-1，直接从连续缓冲回溯 `samples[start-1]`（减去本帧 mean 近似去直流），无需 `preemph_prev` 字段
3. **Povey 窗**（流式 Paraformer）— `(0.5-0.5cos(2πi/(N-1)))^0.85`，**非 hamming**
4. **Mel 滤波器 high_freq=7600 Hz**（`high_freq=-400`，即 Nyquist-400），**非 8000 Hz**
5. **增量式 fbank 提取**（流式）— 音频线性追加到 `raw_samples`、fbank 帧按序增量计算到 `fbank_cache`。不可按 chunk 重复提取（重叠帧重复计算 + 边界问题）

离线 Paraformer 用 **hamming 窗**，流式用 **povey 窗**。`compute_fbank(samples, window, preemph_coeff)` 参数化窗口，两者共享同一实现，**pre-emphasis 均无状态**（直接回溯连续缓冲 `samples[start-1]`）。

另：`decode_tokens` 遵循 sherpa-onnx `Convert()` 空格逻辑——ASCII 词前加空格、`@@` BPE 合并；`smart_append()` 在 chunk 边界检测 ASCII↔非 ASCII 插入空格。流式引擎累积 `all_token_ids` 跨 chunk 整体 `decode_tokens`（非逐 chunk 解码），避免 BPE 续接断裂（`val@@`+`ue` 被切成 `val`/`ue`）。`StreamingSession` Paraformer 用 `punct_prefix` + `committed_chars` 管理逗号分句。

**热路径性能**：decoder_caches 用 `copy_from_slice` 复用预分配内存（省 ~320KB/chunk），encoder 输入 `into_shape` 零拷贝（省 ~45KB），CIF 用 `as_slice()` 引用（省 ~20-40KB），decoder 键名预分配 `cache_keys`（省 16× format!）。

**诊断方法**：`cargo test -p octopus-asr --lib streaming_paraformer::tests::test_streaming_paraformer_real_model -- --nocapture`，对比输出与 sherpa-onnx 参考值 `"昨天是 monday today day is 礼拜二 the day after tomorrow 是星期"`。详见 [spec](docs/superpowers/specs/2026-06-21-paraformer-fbank-feature-extraction-fix.md)。
