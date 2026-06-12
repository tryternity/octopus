# octopus - ASR Inference Toolkit

基于 ort (ONNX Runtime) 的语音识别工具集，支持 Whisper、SenseVoice、Paraformer、Qwen3-ASR、Zipformer 五种引擎。

## 编译打包

```bash
# 编译 server 和 CLI（release 模式）
cargo build --release -p octopus-server -p octopus-cli

# 编译全部（含 library）
cargo build --release

# 仅编译 library
cargo build --release -p octopus-asr

# 桌面测试
cargo run --release -p octopus-desktop --features embedded
```

编译产物位于 `target/release/`：

| 文件 | 说明 |
|------|------|
| `octopus-server` | HTTP + WebSocket 服务 |
| `octopus-cli` | 命令行工具 |

## CLI 使用

```bash
# 列出可用麦克风
octopus-cli devices

# 查看模型配置信息
octopus-cli config

# WAV 文件识别（默认 SenseVoice，自动语言检测）
octopus-cli transcribe <wav_path>

# WAV 文件识别（指定引擎和语言）
octopus-cli transcribe <wav_path> --model whisper --language zh
octopus-cli transcribe <wav_path> --model sensevoice
octopus-cli transcribe <wav_path> --model paraformer
octopus-cli transcribe <wav_path> --model qwen3-asr-0.6B
octopus-cli transcribe <wav_path> --model zipformer-ctc

# 全流程：麦克风 → VAD → ASR → 文本（按回车停止录音）
octopus-cli e2e
octopus-cli e2e --model whisper --language zh

# 流式测试：WAV 文件分块送入流式引擎
octopus-cli stream-test <wav_path> --model paraformer-streaming
octopus-cli stream-test <wav_path> --model zipformer-ctc
```

### 参数说明

| 参数 | 说明 | 默认值 |
|------|------|--------|
| `--model <engine>` | ASR 引擎（见下表） | `sensevoice` |
| `--language <lang>` | 语言：`auto`、`zh`、`en`、`ja` 等 | `auto` |

### 支持的引擎

| 引擎名 | 类型 | 说明 |
|--------|------|------|
| `sensevoice` | 离线 | 快速，自动语言检测（默认） |
| `whisper` | 离线 | 多语言，auto 检测（短音频建议指定语言） |
| `paraformer` / `paraformer-streaming` | 离线/流式 | 中文优化 |
| `qwen3-asr-0.6B` | 离线 | 大模型能力 |
| `zipformer-ctc` / `zipformer-small-ctc` / `zipformer-multi` | 离线/流式 | 轻量级 CTC |

> **提示：** Whisper 的 auto 语言检测对短音频可能不准确，建议中文场景用 `--language zh`。SenseVoice 不受此参数影响（自动检测）。

## Server 使用

```bash
# 启动服务（默认端口 3000）
octopus-server

# 指定端口和地址
octopus-server --port 8080 --host 127.0.0.1

# 通过环境变量配置
OCTOPUS_PORT=8080 OCTOPUS_HOST=127.0.0.1 octopus-server
```

### API 接口

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | 健康检查 |
| GET | `/models` | 查看当前模型信息 |
| POST | `/transcribe` | 音频文件识别 |
| WS | `/ws/stream` | 实时流式识别 |

### 示例

```bash
# 健康检查
curl http://localhost:3000/health

# 查看模型
curl http://localhost:3000/models

# WAV 文件识别（默认引擎）
curl -X POST http://localhost:3000/transcribe \
  --data-binary @test.wav

# 指定引擎和语言
curl -X POST "http://localhost:3000/transcribe?engine=whisper&language=zh" \
  --data-binary @test.wav

# 返回格式
# {"text": "识别结果", "duration_ms": 3200, "rtf": 2.1}
```

### WebSocket 流式识别

连接 `ws://localhost:3000/ws/stream`，支持通过 query 参数指定引擎：

```
ws://localhost:3000/ws/stream?engine=sensevoice&language=auto
```

发送 f32 PCM（16kHz little-endian）音频块：

- 每积累 ~1 秒音频自动执行 VAD + ASR，返回识别结果
- 发送 `"flush"` 文本消息强制处理缓冲区剩余音频
- 返回格式：`{"text": "...", "final": true}`

## 配置文件

- `~/.octopus/model.json` — 模型路径配置（VAD、所有 ASR 引擎）
- `~/.octopus/config.yaml` — 应用设置（麦克风选择等）
- VAD 模型回退路径：`~/.octopus/models/silero_vad_v4.onnx`

详细配置说明见 [docs/configuration.md](docs/configuration.md)。

## 快速运行（开发模式）

```bash
cd octopus

# CLI
cargo run -p octopus-cli -- config
cargo run -p octopus-cli -- e2e --model sensevoice
cargo run -p octopus-cli -- e2e --model whisper --language zh
cargo run -p octopus-cli -- stream-test test.wav --model zipformer-ctc

# Server
cargo run -p octopus-server
cargo run -p octopus-server -- --port 8080
```
