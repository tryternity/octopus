# 配置指南

## 目录结构

```
~/.octopus/
├── model.json          # 模型配置（必需）
├── config.yaml         # 应用设置（可选）
└── models/
    └── silero_vad_v4.onnx  # VAD 模型（回退路径）
```

## model.json

定义所有 ASR 引擎的模型来源，每个引擎对应一个 HuggingFace 仓库。

### 示例

```json
{
  "vad": {
    "active": "silero-v4",
    "silero": {
      "silero-v4": {
        "source": "snakers4/silero-vad"
      }
    }
  },
  "asr": {
    "active": "sensevoice",
    "whisper": {
      "whisper-small": {
        "source": "onnx-community/whisper-small",
        "language": "en",
        "description": "Whisper small model"
      }
    },
    "sensevoice": {
      "sherpa-onnx-sense-voice-funasr-nano-int8": {
        "source": "csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17",
        "description": "SenseVoice nano int8"
      }
    },
    "paraformer": {
      "paraformer-streaming": {
        "source": "csukuangfj/sherpa-onnx-streaming-paraformer-zh",
        "description": "Streaming Paraformer 中文"
      }
    },
    "qwen3-asr": {
      "qwen3-asr-0.6B": {
        "source": "csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
        "description": "Qwen3-ASR 0.6B int8"
      }
    },
    "zipformer": {
      "zipformer-ctc": {
        "source": "csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30",
        "description": "Streaming Zipformer CTC int8"
      }
    }
  }
}
```

### 字段说明

#### 顶层

| 字段 | 类型 | 说明 |
|------|------|------|
| `vad` | object | VAD 配置（可选） |
| `asr` | object | ASR 引擎配置（必需） |

#### vad

| 字段 | 类型 | 说明 |
|------|------|------|
| `active` | string | 当前使用的 VAD 模型名（留空则跳过，使用回退路径） |
| `silero` | object | Silero VAD 模型映射，key 为模型名 |

#### asr

| 字段 | 类型 | 说明 |
|------|------|------|
| `active` | string | 默认 ASR 引擎名 |
| `whisper` | object | Whisper 模型映射（可选） |
| `sensevoice` | object | SenseVoice 模型映射（可选） |
| `paraformer` | object | Paraformer 模型映射（可选） |
| `qwen3-asr` | object | Qwen3-ASR 模型映射（可选） |
| `zipformer` | object | Zipformer 模型映射（可选） |

#### 模型条目（ModelEntry）

| 字段 | 类型 | 说明 |
|------|------|------|
| `source` | string | HuggingFace 仓库路径（如 `onnx-community/whisper-small`） |
| `language` | string | 默认语言（可选） |
| `description` | string | 描述（可选） |
| `quantization` | string | 量化偏好：`int8`（默认）或 `fp32`（可选） |

## config.yaml

应用级别设置，文件不存在时使用默认值。

### 示例

```yaml
microphone: "MacBook Pro Microphone"
```

### 字段说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `microphone` | string | `""` | 指定麦克风设备名（留空使用系统默认） |

## 模型下载

使用 `huggingface-cli` 下载模型到本地缓存：

```bash
# 安装 HF CLI
pip install huggingface_hub

# 下载模型
huggingface-cli download onnx-community/whisper-small
huggingface-cli download csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17
huggingface-cli download csukuangfj/sherpa-onnx-streaming-paraformer-zh
```

下载后模型自动存入 `~/.cache/huggingface/hub/`，`model.json` 中的 `source` 字段会自动定位到对应缓存路径。
