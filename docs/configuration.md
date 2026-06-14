# 配置指南

octopus 配置分两部分：

- **模型配置**：`~/.octopus/octopus.db` 的 `models` 表（SQLite，唯一来源）
- **应用配置**：`~/.octopus/config.yaml`（行为参数，可选）

首次启动自动建库 + 写入默认引擎，开箱即用。

## 目录结构

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite：models 表（模型配置）+ transcriptions 表（识别历史）
├── config.yaml         # 应用配置（可选，缺失用默认值）
├── VOICE_POLISH.md     # 润色 system prompt 自定义覆盖（可选）
└── models/             # 随应用打包的小模型（固定路径）
    ├── silero_vad_v4.onnx   # VAD（固定加载，不进 DB）
    └── zipformer/           # 默认 ASR（model.int8.onnx + tokens.txt）

~/.cache/huggingface/hub/   # 大模型 HF 缓存（whisper/sensevoice/qwen3/paraformer 等，按需下载）
```

## 模型配置（octopus.db）

模型配置唯一来源是 `models` 表。首次建库时 `seed_default_models` 写入默认 7 引擎：

| category | name | source |
|---|---|---|
| zipformer | zipformer-small-ctc | `models/zipformer`（本地打包，**兜底引擎**） |
| zipformer | zipformer-multi | k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13 |
| zipformer | zipformer-ctc | csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30 |
| paraformer | paraformer-streaming | csukuangfj/sherpa-onnx-streaming-paraformer-zh |
| sensevoice | sherpa-onnx-sense-voice-funasr-nano-int8 | csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17 |
| qwen3-asr | qwen3-asr-0.6B | csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25 |
| whisper | whisper-small | onnx-community/whisper-small |

> **不再有 `is_active` 列**：引擎激活改由 `config.yaml.asr_engine` 决定（见下方「引擎选择与兜底」）。`zipformer-small-ctc` 是兜底引擎——`asr_engine` 为空或匹配不到任何模型时，自动回退到它（靠本地打包路径，开箱可用）。

**VAD 不进表**：固定路径 `~/.octopus/models/silero_vad_v4.onnx`，随应用打包。

查看当前 DB 中的引擎：

```bash
octopus-cli config
```

### 模型目录解析（resolve_model_dir）

`source` 字段双模式：

| source 形态 | 解析结果 | 示例 |
|---|---|---|
| 本地相对路径 | `~/.octopus/<source>` | `models/zipformer` → `~/.octopus/models/zipformer` |
| HF repo 名 | `~/.cache/huggingface/hub/` | `onnx-community/whisper-small` → HF 缓存 |

### 手编 DB

`models` 表可手动编辑（增删模型条目），但**需重启进程生效**——`asr::load_config()` 首次读出后缓存到 `OnceLock`，运行中不热更新。引擎激活改由 `config.yaml.asr_engine` 决定（`models` 表不再有 `is_active` 列）。

### model.json / history.txt 已废弃

`model.json`（旧模型配置）和 `history.txt`（旧识别历史）已在 DB 单一源重构中彻底删除，DB 是唯一来源。详见 [db-single-source 设计](superpowers/specs/2026-06-14-db-single-source-design.md)。

## config.yaml

应用行为配置，文件不存在时使用默认值。

| 字段 | 类型 | 默认值 | 适用端 | 说明 |
|---|---|---|---|---|
| `microphone` | string | `""` | cli + desktop | 麦克风设备名（空 = 系统默认） |
| `asr_engine` | string | `""` | desktop + server | 引擎 name，按 DB `models` 表 `name` 精确匹配；空或匹配不到回退兜底 `zipformer-small-ctc`（用 `octopus-cli config` 查看可选值）。显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，不走兜底 |
| `language` | string | `"auto"` | desktop | auto / zh / en / ja / ko |
| `engine_mode` | string | `"embedded"` | desktop | embedded / websocket / grpc |
| `remote_url` | string | `ws://127.0.0.1:3000/ws/stream` | desktop | websocket 模式远程地址 |
| `grpc_endpoint` | string | `http://127.0.0.1:50051` | desktop | grpc 模式端点 |
| `shortcut` | string | `CmdOrCtrl+Shift+Space` | desktop | 全局快捷键 |
| `paste_method` | string | `"clipboard"` | desktop | clipboard / direct / none |
| `overlay_position` | string | `"top"` | desktop | top / bottom / none |
| `segment_duration` | f64 | `5.0` | desktop | VAD 伪流式：缓冲累积时长阈值（秒） |
| `segment_silence` | f64 | `500.0` | desktop | VAD 伪流式：静音触发识别阈值（毫秒） |
| `segment_overlap` | f64 | `200.0` | desktop | VAD 伪流式：相邻分段 overlap（毫秒） |
| `polish_enabled` | bool | `false` | desktop | LLM 润色总开关 |
| `polish_interval` | f64 | `5.0` | desktop | 中间润色间隔（秒），0 = 仅最终润色 |
| `llm_provider` | string | `""` | desktop | openai / deepseek / 自定义 |
| `llm_model` | string | `"gpt-4o-mini"` | desktop | 模型名 |
| `llm_base_url` | string | `https://api.openai.com/v1` | desktop | API base URL |
| `llm_secret_key` | string | `""` | desktop | API Key（空则润色不生效） |

> **前缀划分**：`segment_*` 控制 VAD 分段，`polish_*` 控制润色行为，`llm_*` 描述 LLM 连接（可被未来其他 LLM 用途复用）。`microphone` 为 cli + desktop 跨端通用字段，其余为 desktop 行为参数。

### 引擎选择与兜底（resolve_active_engine）

引擎激活的**唯一真相是 `config.yaml.asr_engine`**（DB `models` 表不再有 `is_active` 列）。解析逻辑（`asr::config::resolve_active_engine`）：

| `asr_engine` 值 | 解析结果 |
|---|---|
| 精确匹配到 DB `name`（如 `qwen3-asr-0.6B`） | 用命中引擎 |
| 空 `""` 或缺失 | 回退兜底 `zipformer-small-ctc` |
| 非空但匹配不到任何模型 | 回退兜底 + warn 日志 |

**兜底级联**：优先从 DB `models` 表 `zipformer` section 查 `zipformer-small-ctc`（用户手编 `source` 仍生效）；DB 无该条目时硬构造（靠 `DEFAULT_ASR_MODEL_DIR` 本地打包路径，保证开箱可用）。

**优先级**：显式参数 > `config.yaml.asr_engine` > 兜底。
- cli `--model`、server 请求带 `engine` 字段、`AsrEngineManager.switch_model(name)` 都是显式 name 路径，按 DB name 精确匹配，**不走兜底**（匹配不到直接报错）。
- `resolve_active_engine` 仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。

### 完整示例

```yaml
# 麦克风（留空用系统默认）
microphone: ""

# ASR 引擎（DB models 表中的 name，用 octopus-cli config 查看可选值）
# 精确匹配 DB name；留空或匹配不到则回退兜底 zipformer-small-ctc
asr_engine: "qwen3-asr-0.6B"
language: "auto"

# 引擎接入模式
engine_mode: "embedded"          # embedded | websocket | grpc

# 桌面交互
shortcut: "CmdOrCtrl+Shift+Space"
paste_method: "clipboard"        # clipboard | direct | none
overlay_position: "top"          # top | bottom | none

# VAD 伪流式分段（离线引擎）
segment_duration: 5.0            # 秒
segment_silence: 500.0           # 毫秒
segment_overlap: 200.0           # 毫秒

# LLM 润色（可选）
polish_enabled: false
polish_interval: 5.0             # 秒，0 = 仅最终润色
llm_provider: "deepseek"
llm_model: "deepseek-chat"
llm_base_url: "https://api.deepseek.com/v1"
llm_secret_key: ""               # 填入你的 API Key
```

## 模型下载

随应用打包的小模型（VAD + 默认 zipformer）无需下载。其他大模型用 `huggingface-cli` 按需下载到 HF 缓存：

```bash
# 安装 HF CLI
pip install huggingface_hub

# 下载（source 字段即 HF repo 名）
huggingface-cli download onnx-community/whisper-small
huggingface-cli download csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17
huggingface-cli download csukuangfj/sherpa-onnx-streaming-paraformer-zh
```

下载后存入 `~/.cache/huggingface/hub/`，DB `models` 表中 `source` 为 HF repo 名的引擎会经 `resolve_model_dir` 自动定位到对应缓存。
