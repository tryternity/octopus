# Third-Party Licenses

本文件集中声明 octopus 发行物所包含或调用的第三方组件及其许可证，便于合规审查与 attribution 管理。

> **收录原则**：只收录「随 octopus 分发（打包进 binary / DMG / 内嵌资源）」或「运行时由 octopus 调用但未随 app 分发（用户机器外部二进制）」的组件。Rust crate 的传递依赖（数百个）由 `cargo license` 工具按需生成，不在本文件逐一列举。

---

## 1. octopus 自身许可

**octopus 当前未发布开源许可证。** 仓库根目录无 `LICENSE` 文件，Cargo.toml 无 `license` 字段，法律上等同于 "All rights reserved"（专有/闭源）。

未来若决定开源，将在本节明确许可证。

---

## 2. 随 app 分发的内嵌资源

以下是编译期通过 `include_bytes!` / `include_str!` 内嵌进 Rust binary 的第三方数据，会随 DMG 打包：

### 2.1 Silero VAD（语音活动检测模型）

| 字段 | 值 |
| --- | --- |
| **文件** | `crates/asr-local/models/silero_vad_v4.onnx` |
| **用途** | ASR 流式引擎的 VAD（区分语音/静音）|
| **内嵌位置** | `crates/asr-local/src/vad.rs:74`（`include_bytes!("../models/silero_vad_v4.onnx")`）|
| **来源** | <https://github.com/snakers4/silero-models> |
| **许可证** | MIT（[silero-models LICENSE](https://github.com/snakers4/silero-models/blob/master/LICENSE)）|
| **版权声明** | Copyright (c) 2020-2024 Silero Authors |

### 2.2 简繁转换数据

| 字段 | 值 |
| --- | --- |
| **文件** | `crates/asr-local/data/t2s.txt` / `s2t.txt` |
| **用途** | ASR 输出的简繁归一化 |
| **内嵌位置** | `crates/asr-local/src/hans.rs:18,20` |
| **来源** | 中文简繁转换词表（社区维护）|
| **许可证** | 待补（需确认数据上游来源）|

### 2.3 纠错 unigram/bigram

| 字段 | 值 |
| --- | --- |
| **文件** | `crates/asr-local/src/corrector_data/unigram.txt.gz` / `bigram.txt.gz` |
| **用途** | ASR 输出文本纠错（同音字/易错词）|
| **内嵌位置** | `crates/asr-local/src/corrector.rs:13,14` |
| **许可证** | 待补（需确认数据上游来源）|

---

## 3. 编译期链接的 Rust 依赖（关键 / 非 MIT-Apache）

> 完整依赖树清单（含数百个传递依赖）请用 `cargo license` 生成：
> ```bash
> cargo install cargo-license
> cargo license --all-features --avoid-dev-deps --avoid-build-deps
> ```

以下是 octopus 依赖的关键 crate，标注其许可证类型（仅列非 MIT/Apache-2.0 默认的，或值得特别说明的）：

### 3.1 Rust crates（MIT / Apache-2.0，按惯例归入此类不单独列）

绝大多数依赖（`tokio` / `serde` / `reqwest` / `anyhow` / `tauri` 全家桶 / `ort` 等）为 MIT 或 Apache-2.0 许可，与 octopus 当前 "All rights reserved" 兼容。

### 3.2 DeepFilterNet（深度滤波降噪，git fork）

| 字段 | 值 |
| --- | --- |
| **crate** | `deep_filter`（git 依赖）|
| **来源** | <https://github.com/tryternity/DeepFilterNet.git> tag `v0.5.6` |
| **用途** | `octopus-asr-local` 的音频降噪（DeepFilterNet 模型推理）|
| **许可证** | Apache-2.0 OR MIT（[LICENSE-APACHE](https://github.com/tryternity/DeepFilterNet/blob/main/LICENSE-APACHE) / [LICENSE-MIT](https://github.com/tryternity/DeepFilterNet/blob/main/LICENSE-MIT)）|

### 3.3 ONNX Runtime（`ort` crate）

| 字段 | 值 |
| --- | --- |
| **crate** | `ort` v2.0.0-rc.12 |
| **用途** | ASR/OCR/PaddleOCR 的 ONNX 模型推理引擎 |
| **许可证** | MIT OR Apache-2.0（crate 层）；底层 ONNX Runtime C++ 库为 MIT（[onnxruntime LICENSE](https://github.com/microsoft/onnxruntime/blob/main/LICENSE)）|

### 3.4 简体中文分词（jieba-rs）

| 字段 | 值 |
| --- | --- |
| **crate** | `jieba-rs` |
| **用途** | ASR 文本后处理（中文分词，热词挖掘）|
| **许可证** | MIT |

---

## 4. 运行时调用的外部二进制（不随 app 分发）

octopus 在运行时通过 `tokio::process::Command` 调用以下系统二进制，**不随 DMG 打包**。这些是用户机器上的外部程序，octopus 不分发它们的代码，仅在功能需要时检测并调用：

### 4.1 yt-dlp（视频音频下载）

| 字段 | 值 |
| --- | --- |
| **调用方** | `octopus-dlp` crate（`crates/dlp/src/main.rs`）|
| **获取方式** | 优先系统 PATH；缺失时从 GitHub releases 自动下载到 `~/.octopus/bin/yt-dlp` |
| **用途** | 从网络 URL 下载音视频流，供 ASR 转写 |
| **许可证** | Unlicense（[yt-dlp LICENSE](https://github.com/yt-dlp/yt-dlp/blob/master/LICENCE)）|
| **隔离论证** | 见 [`crates/dlp/docs/architecture.md` §1](crates/dlp/docs/architecture.md#1-许可证合规性设计-gplv3-隔离)（注：yt-dlp 实际是 Unlicense 非 GPLv3，架构文档表述需更新）|

> ⚠️ **架构文档勘误**：`crates/dlp/docs/architecture.md:9` 称 yt-dlp 为 GPLv3，但 yt-dlp 实际是 Unlicense（公有领域）。此外 octopus-dlp crate **并未** link `boul2gom/yt-dlp` Rust crate（dlp Cargo.toml 无此依赖），而是直接 spawn 系统 `yt-dlp` 二进制。物理进程隔离论证仍成立（Unlicense 比 GPLv3 更宽松），但文档表述需修正。

### 4.2 ffmpeg（音视频转码）

| 字段 | 值 |
| --- | --- |
| **调用方** | `octopus-dlp`（音视频转码）；**未来** `octopus-record`（F15 字幕抽音轨）|
| **获取方式** | 优先系统 PATH；其次 `~/.octopus/bin/ffmpeg`（dlp 下载缓存）；缺失时打印平台安装指导 |
| **用途** | 音视频格式转换、抽音轨、转码 |
| **许可证** | LGPL 2.1+（默认配置）或 GPL 2+（启用 --enable-gpl 等）；ffmpeg 二进制本身的许可取决于其编译配置 |
| **隔离论证** | 同 yt-dlp，物理进程隔离 + Mere Aggregation 边界 |

### 4.3 sherpa-onnx（可选，ASR 参考实现）

| 字段 | 值 |
| --- | --- |
| **用途** | 开发期对比验证 ASR 输出（不进生产构建）|
| **许可证** | Apache-2.0（[sherpa-onnx LICENSE](https://github.com/k2-fsa/sherpa-onnx/blob/master/LICENSE)）|

---

## 5. 前端依赖（`crates/desktop/frontend/`）

> 完整清单见 `crates/desktop/frontend/package.json`。以下是关键组件：

| 依赖 | 用途 | 许可证 |
| --- | --- | --- |
| **React 19** | UI 框架 | MIT |
| **Tauri 2 plugins**（api / plugin-dialog / plugin-opener）| IPC | MIT/Apache-2.0 |
| **CodeMirror 6** | 文本编辑器（compact editor）| MIT |
| **Radix UI** | 无样式基础组件 | MIT |
| **Tailwind CSS v4** | 原子化 CSS | MIT |
| **lucide-react** | 图标库 | ISC |

---

## 6. 字体

octopus 当前不内嵌字体（使用系统字体）。未来若内嵌 Web 字体，将在此节声明。

---

## 7. 待补录（录屏功能 vendor 时）

> 本节是录屏功能（D-Swift 选型）的占位，录屏 helper vendor 进来后填充：

### 7.1 openscreen ScreenCaptureKit Helper（待 vendor）

| 字段 | 值 |
| --- | --- |
| **来源** | <https://github.com/EtienneLescot/openscreen>（`electron/native/screencapturekit/`）|
| **用途** | macOS 屏幕录制 helper 子进程（SCK + AVAssetWriter）|
| **打包方式** | 编译为 `octopus-sck-helper` 二进制，放 `Contents/Resources/binaries/`（见 [录屏功能文档 §2.1](docs/superpowers/specs/2026-07-25-screen-record-features.md#f1--helper-二进制获取与打包)）|
| **许可证** | MIT（[openscreen LICENSE](https://github.com/EtienneLescot/openscreen/blob/main/LICENSE)）|
| **版权声明** | Copyright (c) 2025 Siddharth Vaddem |
| **修改声明** | octopus vendor 后修改点见 `crates/desktop/native/screencapturekit-helper/README.md`（待建）|
| **隔离论证** | 复用 [`crates/dlp/docs/architecture.md` §1](crates/dlp/docs/architecture.md) 的物理进程隔离 + Mere Aggregation 边界（openscreen MIT 比 yt-dlp 更宽松）|

### 7.2 openscreen Cursor Helper（待 vendor，P3 阶段）

| 字段 | 值 |
| --- | --- |
| **来源** | 同上（`electron/native/screencapturekit/Sources/OpenScreenMacOSCursorHelper/`）|
| **许可证** | MIT |

---

## 维护指引

- **何时更新本文件**：
  - 新增非 MIT/Apache-2.0 的依赖（Rust crate / npm 包 / 外部二进制）
  - 新增内嵌资源（`include_bytes!` / `include_str!`）
  - 新增随 app 分发的二进制（helper / sidecar）
  - 新增字体或图片资源
- **如何查证**：
  - Rust 依赖许可证：`cargo install cargo-license && cargo license --all-features`
  - npm 依赖许可证：`cd crates/desktop/frontend && npx license-checker --summary`
  - 内嵌资源：`rg "include_bytes!|include_str!" crates/*/src`
  - 外部二进制调用：`rg "Command::new|tokio::process::Command" crates/`
- **新增条目模板**：见上文各小节的表格格式，至少包含「文件/来源/用途/许可证/版权声明」5 个字段
