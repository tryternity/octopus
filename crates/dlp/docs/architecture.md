# octopus-dlp 架构与实现方案

本文档描述了 `octopus-dlp` Crate 的技术方案与系统架构。该模块旨在通过输入网络视频/音频 URL，分离并提取音频，并通过管道（Pipe）向主程序进程流式传输 16kHz 单声道 `f32le` 原始 PCM 数据，从而交付给核心 ASR 引擎完成语音识别。

---

## 1. 许可证合规性设计 (GPLv3 隔离)

本模块使用的底层下载器 `boul2gom/yt-dlp` Crate 采用 **GNU GPL v3.0** 开源许可证。为了防止 GPLv3 许可证向 `octopus` 项目的其他闭源/专有模块（如 `octopus-asr` 核心推理模块、`octopus-desktop` 桌面端应用等）发生传染，我们采用了**物理进程隔离**的架构设计：

### 1.1. 边界划分
1.  **`octopus-dlp` (独立 CLI 进程，开源 GPLv3)**：
    *   这是一个独立的二进制命令行程序（`octopus-dlp`）。
    *   直接作为 `boul2gom/yt-dlp` 的 Rust 封装层，处理多平台依赖（`yt-dlp`、`ffmpeg`）的自动检测与下载。
    *   **由于不链接任何商业或闭源的 ASR 及 UI 代码，该 Crate 开源不会对项目的核心知识产权造成损害**。
2.  **`octopus-desktop` / `octopus-cli` (主进程，保留原有闭源/商业许可)**：
    *   通过操作系统的 `fork-exec` 机制异步拉起 `octopus-dlp` 子进程。
    *   双方通过**标准输出管道 (Stdout Pipe)** 进行单向通信。
    *   根据 FSF 的官方合规性解释，通过管道、命令行参数以及标准输入输出进行数据交换属于**单纯聚合 (Mere Aggregation)**，不构成派生作品，因此 GPL 许可证**不会传染**到主进程。
3.  **`octopus-asr` (核心推理库，完全闭源/受保护)**：
    *   由主进程本地直接调用，与 `octopus-dlp` 无任何直接代码链接或编译依赖。

### 1.2. 编译期与运行期依赖解耦
为了彻底隔绝代码层面的依赖和合规风险，我们对 `octopus-cli` 和 `octopus-dlp` 进行了编译期与运行期的完全解耦：
1. **编译期（Cargo 依赖树）无关联**：
   在 `crates/cli/Cargo.toml` 中，我们**没有**在 `[dependencies]` 声明对 `octopus-dlp` 的任何依赖。因此，在 Rust 编译器编译 ASR 和 CLI 代码时，`dlp` 模块的代码和底层 GPLv3 的 `boul2gom/yt-dlp` 库绝不会被编译、链接到主程序中，这阻断了代码层面的静态/动态链接传染。
2. **运行期（Runtime 子进程管道）松耦合**：
   主程序在运行时，通过操作系统的进程启动接口（如 `tokio::process::Command`）拉起外部的可执行程序 `octopus-dlp`。双方仅通过标准的标准输出（Stdout）和标准错误（Stderr）管道进行数据流动交互。这种松耦合符合 GPL 协议的“单纯聚合 (Mere Aggregation)”边界，保证了主程序闭源的独立性。

---

## 2. 管道式通信机制 (Pipe Communication)

为了规避传统的磁盘临时 WAV 文件读写开销并提升处理效率，`octopus-dlp` 与主进程采用基于 `Stdio` 管道的流式传输。

### 2.1. 数据流向

```
[ octopus-desktop (主进程) ]
      │
      ├─ 1. 异步启动 ──> [ octopus-dlp (子进程) ]
      │                        │
      │                        ├─ 2. 调用 yt-dlp ──> 下载流媒体原始音频至临时目录
      │                        │
      │                        └─ 3. 调用 ffmpeg ──> 读取临时音频文件并解码转码
      │                                                │
      │                                                └─ (通过 stdout 流式输出 raw f32le)
      │                                                      │
      ├─ 4. 从 ChildStdout 实时拉取 raw f32 字节流 <─────────┘
      │
      └─ 5. 本地运行 ASR 推理 (octopus-asr) ──> 输出文本
```

### 2.2. 数据流格式说明：Raw `f32le` PCM
传输音频流时不使用带文件头的 `.wav` 格式，而是直接使用原始脉冲编码调制流（Raw PCM）：
*   **采样率 (Sample Rate)**：`16000 Hz`
*   **通道数 (Channels)**：`1` (Mono 单声道)
*   **格式 (Format)**：`f32le` (32-bit Float Little Endian，每个采样占 4 字节)

**采用此格式的优势**：
1.  **零编解码开销**：主进程每读取 4 个字节，直接通过 `f32::from_le_bytes` 即可还原为 ASR 推理可以直接消费的 `f32` 采样，免去 `i16 -> f32` 的浮点转换与重采样计算。
2.  **规避流式 WAV 局限**：WAV 文件头包含固定的数据长度信息（在流开始前无法预知），使用 Raw PCM 能够完美支持单向无长度指示 of 无限流式传输。

---

## 3. CLI 接口设计 (`octopus-dlp`)

`octopus-dlp` 提供简洁的命令行接口，输入 URL，支持流式输出或直接保存到本地指定文件。

### 3.1. 命令格式
```bash
octopus-dlp <URL> [-o/--output [<FILE>]] [--unclear]
```

### 3.2. 参数说明
*   `<URL>`：要转码解析的网络视频/音频链接。
*   `-o, --output [<FILE>]`：（可选）指定输出文件路径。
    *   若指定该参数且给定了具体文件名，音频数据直接写入该 WAV 文件，`stdout` 不输出任何音频内容。这非常利于开发调试或本地音频持久化。
    *   若指定该参数但**没有指定具体的文件名/路径（留空）**，系统将自动解析为 `~/.octopus/tmp/<md5(url)>.wav`，实现自动保存。
    *   若不指定该参数，默认流式写入 `stdout` 管道。
*   `--unclear`：（可选）不删除下载的流媒体视频文件。第二次运行相近的 URL 时，若检测到本地缓存已存在该文件，则自动跳过下载（Skipping download）以避免重复下载。缓存文件名规则为 `~/.octopus/tmp/<md5(url)>.<后缀>`。

### 3.3. 输出设计
*   **标准输出 (Stdout)**：当且仅当未指定 `-o` 时，为纯净的二进制原始 `f32le` PCM 采样流。没有日志或其他非音频数据，确保主进程可以盲读。
*   **标准错误 (Stderr)**：日志、下载进度、报错信息以及视频的元数据信息（以结构化 JSON 形式输出到 stderr 的首行，以便主进程提取标题和时长）。
    *   *元数据 JSON 格式示例*：
        ```json
        {"title": "Bilibili视频标题", "duration": 128.5, "author": "UP主名称"}
        ```

---

## 4. Crate 核心实现步骤

### 4.1. [NEW] 创建 `crates/dlp`
在项目根目录中新建 `crates/dlp/`，在主项目的 `Cargo.toml` 工作区成员中加入该路径。

### 4.2. `crates/dlp/Cargo.toml` 依赖声明
```toml
[package]
name = "octopus-dlp"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json", "stream"] }
futures-util = "0.3"
tempfile = "3"
md5 = "0.7"
```

### 4.3. 依赖自动下载与检测逻辑
*   **yt-dlp 自动管理**：利用 `yt-dlp` Crate 提供的能力（或我们自研的轻量级下载组件），在 `~/.octopus/bin/` 自动下载并更新 `yt-dlp` 可执行二进制。
*   **ffmpeg 检测**：检查系统 PATH 与 `~/.octopus/bin/ffmpeg`。若无，在 `stderr` 打印友好的平台安装指导并退出。

### 4.4. 临时下载与流式输出实现
1.  计算 URL 的 MD5 值（`url_md5`）。
2.  子进程通过 `yt-dlp` 获取元数据并输出 JSON 到 `stderr`。
3.  若指定了 `--unclear` 且文件 `~/.octopus/tmp/<url_md5>.<ext>` 已存在，跳过下载阶段。
4.  否则，调用 `yt-dlp` 异步下载音视频轨道（`-f ba/b`）并保存为 `~/.octopus/tmp/<url_md5>.<ext>`。
5.  启动 `ffmpeg` 异步转码：
    *   若指定了 `-o/--output`，则强制使用 `-f wav` 输出一个包含标准 RIFF 头部的可直接播放音频文件到指定路径。
    *   若未指定 `-o`，则强制使用 `-f f32le` 输出原始 `pcm_f32le` 数据流至 stdout：
        ```bash
        ffmpeg -y -i <temp_file> -f f32le -ar 16000 -ac 1 -c:a pcm_f32le -
        ```
6.  转码完毕后，除非指定了 `--unclear`，否则自动异步清理下载的原始流媒体视频/音频文件。

---

## 5. 主程序消费逻辑示例

主应用通过管道执行并读取流式数据的典型写法：

```rust
use tokio::process::Command;
use std::process::Stdio;
use tokio::io::{BufReader, AsyncBufReadExt, AsyncReadExt};

async fn extract_and_transcribe_url(url: &str) -> Result<()> {
    let mut child = Command::new("octopus-dlp")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) // 用以接收日志与 JSON 元数据
        .spawn()?;

    let mut stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // 1. 在另一个任务中异步读取 stderr 获取视频信息
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        if let Ok(Some(first_line)) = reader.next_line().await {
            // 第一行是元数据 JSON
            if let Ok(meta) = serde_json::from_str::<VideoMeta>(&first_line) {
                println!("正在转译视频: {} (时长: {}秒)", meta.title, meta.duration);
            }
        }
        // 之后的行是进度日志
        while let Ok(Some(line)) = reader.next_line().await {
            eprintln!("[dlp log] {}", line);
        }
    });

    // 2. 流式读取 stdout 的 f32le 音频数据
    let mut samples = Vec::new();
    let mut chunk = [0u8; 1024 * 4]; // 必须 4 字节对齐
    while let Ok(n) = stdout.read(&mut chunk).await {
        if n == 0 { break; }
        for raw_sample in chunk[..n].chunks_exact(4) {
            let sample = f32::from_le_bytes(raw_sample.try_into().unwrap());
            samples.push(sample);
        }
    }

    // 3. 等待进程退出并确保清理完毕
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("视频下载/音频分离提取失败");
    }

    // 4. 调用闭源的 ASR 引擎识别
    let text = my_closed_source_asr.transcribe(&samples)?;
    println!("转译文本: {}", text);
    Ok(())
}

---

## 6. ASR 语音转译与长音频 VAD 分段机制 (ASR & VAD Segmentation)

当 `octopus-cli` 接收到大文件或者流式管道流出的较长音频时，需要防止 Transformer 模型在推理时因序列过长产生显存/内存溢出（OOM）或者因注意力机制发散产生虚假复读。

### 6.1. VAD 自动分段策略
在 `octopus-asr` 核心推理模块中，实现了统一的离线语音转译封装函数 `transcribe_with_vad`：
1.  **分段阈值**：当输入的 16kHz f32 单声道音频序列长度超过 `30秒`（即 480,000 个采样点）时，自动启用 `Silero VAD v4` 进行分段。
2.  **停顿检测与切割**：
    *   利用 VAD 逐帧（30ms/480点）检测音频。
    *   当遇到持续 `500ms` 以上的静音/换气点时，在此处将音频截断。
    *   为防止单词跨段切割并限制单次计算复杂度，当单个片段累积长度超过 `25秒` 时强制从最近的静音区截断。
3.  **串行转译与文本清洗**：
    *   将分段后的若干语音切片送入底层的 ASR 引擎（如 `SenseVoice`、`Whisper`、`Paraformer` 等）中进行推理。
    *   如果推理出的分段文本包含 `<|nospeech|>` 特殊事件标记，自动将其过滤。
4.  **智能拼接排版**：
    *   在合并相邻语音片段 of 文本时，根据边界字符的语系自适应：若边界两端均为 CJK（中日韩）字符，则无缝拼接不注入空格；若包含英文/西文字符，则自动插入空格，保证排版格式完全贴合人类自然语言习惯。
```
