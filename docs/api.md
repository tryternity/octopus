# API 参考

octopus-server 提供的 HTTP 和 WebSocket 接口。

## HTTP 接口

### GET /health

健康检查。

**响应：**

```json
{"status": "ok"}
```

### GET /models

查看当前模型配置信息。

**响应：**

```json
{
  "asr_engine": "sensevoice-orig",
  "vad_model": "/path/to/silero_vad_v6.onnx"
}
```

### POST /transcribe

上传音频进行语音识别。

**请求：**

- **Content-Type**: `application/octet-stream` 或不设置
- **Body**: WAV 文件二进制数据，或原始 f32 PCM（16kHz little-endian）
- **Query 参数**:

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `engine` | string | `sensevoice-orig` | ASR 引擎：`whisper`、`sensevoice-orig`、`paraformer`、`qwen3-asr`、`zipformer`、`moonshine`、`firered` |
| `language` | string | `auto` | 语言：`auto`、`zh`、`en`、`ja` 等 |

**响应（成功）：**

```json
{
  "text": "你好世界",
  "duration_ms": 3200,
  "rtf": 2.1
}
```

**响应（音频为空）：**

```
HTTP 400 Bad Request
{"text": "No audio data", "duration_ms": 0, "rtf": 0.0}
```

**响应（推理失败）：**

```
HTTP 500 Internal Server Error
{"text": "Error: ...", "duration_ms": 3200, "rtf": 0.0}
```

**示例：**

```bash
# 默认引擎（SenseVoice）
curl -X POST http://localhost:3000/transcribe \
  --data-binary @test.wav

# 指定 Whisper 引擎
curl -X POST "http://localhost:3000/transcribe?engine=whisper&language=zh" \
  --data-binary @test.wav
```

## WebSocket 接口

### WS /ws/stream

实时流式语音识别。

**连接：**

```javascript
const ws = new WebSocket("ws://localhost:3000/ws/stream");
```

**发送音频：**

发送 f32 PCM 二进制数据（16kHz little-endian），每块建议 1000~16000 个采样点（约 0.0625s~1s）。

```javascript
// 发送 f32 PCM 音频块
ws.send(float32Array.buffer);
```

**发送控制消息：**

| 文本消息 | 说明 |
|----------|------|
| `"flush"` | 强制处理缓冲区中剩余音频 |

**接收消息：**

```json
{"text": "识别结果文本", "final": true}
```

**工作流程：**

1. 客户端持续发送 f32 PCM 音频块
2. 服务端每积累 ~1 秒音频（16000 采样点）后，执行 VAD 检测
3. 检测到语音时，进行 ASR 识别并返回结果
4. 发送 `"flush"` 可强制处理缓冲区中未达到 1 秒阈值的音频

## 启动参数

```bash
octopus-server [选项]
```

| 选项 | 环境变量 | 默认值 | 说明 |
|------|----------|--------|------|
| `--port` | `OCTOPUS_PORT` | `3000` | 监听端口 |
| `--host` | `OCTOPUS_HOST` | `0.0.0.0` | 监听地址 |
