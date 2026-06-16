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

模型配置唯一来源是 `models` 表。首次建库时自动执行 [`db.sql`](../crates/infra/src/db.sql)（`include_str!` 编译期嵌入），写入默认 8 个 ASR 引擎：

| category | name | source | is_local | is_enabled | is_streaming |
|---|---|---|---|---|---|
| zipformer | zipformer-small-ctc | `models/zipformer`（本地打包，**兜底引擎**） | 1 | 1 | 1 |
| zipformer | zipformer-multi | k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13 | 1 | 1 | 1 |
| zipformer | zipformer-ctc | csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30 | 1 | 1 | 1 |
| paraformer | paraformer-streaming | csukuangfj/sherpa-onnx-streaming-paraformer-zh | 1 | 1 | 1 |
| sensevoice | sherpa-onnx-sense-voice-funasr-nano-int8 | csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17 | 1 | 1 | 0 |
| qwen3-asr | qwen3-asr-0.6B | csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25 | 1 | 1 | 0 |
| qwen3-asr | qwen3-asr-1.7B | ilmina/qwen3-asr-1.7b-sherpa-onnx | 1 | 1 | 0 |
| whisper | whisper-small | onnx-community/whisper-small | 1 | 1 | 0 |

同时，首次建库时也会写入默认的 LLM 润色模型条目：

| domain | category (provider) | name (model) | source (base_url) | is_thinking | is_local | is_enabled | 说明 |
|---|---|---|---|---|---|---|---|
| llm | deepseek | deepseek-v4-flash | `https://api.deepseek.com/` | 1 | 0 | 1 | DeepSeek V4 Flash（思考模型） |
| llm | bigmodel | glm-4-flashx | `https://open.bigmodel.cn/api/paas/v4` | 0 | 0 | 1 | 智谱 GLM-4 FlashX（非思考） |
| llm | bigmodel | glm-4.5-flash | `https://open.bigmodel.cn/api/paas/v4` | 1 | 0 | 1 | 智谱 GLM-4.5 Flash（思考模型） |

> **`is_thinking` 字段**：标记该模型是否为思考（reasoning）模型。思考模型在润色等明确任务中若不关闭思考，`content` 可能为空（token 被 `reasoning_content` 耗尽）。置为 `1` 时程序自动发送关闭思考的参数——DeepSeek 用 `thinking: {type: "disabled"}`，BigModel 用 `enable_thinking: false`。

> **`is_local` 字段**：标记该模型是否为本地运行模型。`1` 表示本地模型（例如随应用打包或下载到本地的 ASR 模型），`0` 表示云端 API 模型。`domain + name + is_local` 构成唯一键。

> **`is_enabled` 字段**：标记该模型是否启用。`1` 表示启用，`0` 表示禁用。只有启用的模型才会被系统加载或供识别/润色使用。

> **`is_streaming` 字段**：标记 ASR 模型是否支持流式识别。`1` 表示流式（zipformer×3 / paraformer-streaming，走流式 partial），`0` 表示非流式（sensevoice / qwen3-asr / whisper，走 VAD 分段伪流式）。`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming`，数据驱动、不再按 category 硬编码。

> **LLM API Key 配置方式**：LLM 模型的所有参数（包括 Base URL 和 API Key）全部存储在 DB `models` 表中。其中 `source` 存储 API Base URL，`secret_key` 存储 API Key。你可以通过 SQLite 客户端手动将 API Key 填入 `models` 表对应条目的 `secret_key` 字段。

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

> **开发阶段 schema 变更**：直接修改 [`crates/infra/src/db.sql`](../crates/infra/src/db.sql)，然后删除 `~/.octopus/octopus.db` 并重启即可重新初始化。无迁移逻辑，开发期以此替代。

### model.json / history.txt 已废弃

`model.json`（旧模型配置）和 `history.txt`（旧识别历史）已在 DB 单一源重构中彻底删除，DB 是唯一来源。详见 [db-single-source 设计](superpowers/specs/2026-06-14-db-single-source-design.md)。

## config.yaml

应用行为配置，文件不存在时使用默认值。

> **⚠️ 迁移提示**：旧字段 `polish_enabled` 已废弃。请改用 `polish_mode`：`false` → `0`（关闭）；`true` + interval>0 → `2`（中间+最终润色）；`true` + interval=0 → `1`（仅最终润色）。旧字段被忽略，未配置 `polish_mode` 时润色默认关闭。

| 字段 | 类型 | 默认值 | 适用端 | 说明 |
|---|---|---|---|---|
| `microphone` | string | `""` | cli + desktop | 麦克风设备名（空 = 系统默认） |
| `asr_engine` | string | `""` | desktop + server | ASR 引擎选择，格式 `"PREFIX:NAME"`（见下方模型选择 spec）。空或匹配不到回退兜底 `local:zipformer-small-ctc`。显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，不走兜底。**desktop 悬停工具栏可在运行时切换**（`switch_asr_engine` 命令）：写 RuntimeConfig 镜像 + 持久化回 config.yaml，**下次录音生效**（Coordinator 在 Toggle 进入 Idle 时重读镜像并重建引擎） |
| `language` | string | `"auto"` | desktop | auto / zh / en / ja / ko |
| `engine_mode` | string | `"embedded"` | desktop | embedded / websocket / grpc |
| `remote_url` | string | `ws://127.0.0.1:3000/ws/stream` | desktop | websocket 模式远程地址 |
| `grpc_endpoint` | string | `http://127.0.0.1:50051` | desktop | grpc 模式端点 |
| `shortcut` | string | `CmdOrCtrl+Shift+Space` | desktop | 全局快捷键 |
| `paste_method` | string | `"clipboard"` | desktop | clipboard / direct / none |
| `write_to_clipboard` | bool | `true` | desktop | 粘贴完成后是否把识别结果写入剪贴板（方便他处再粘贴）；`false` 时三模式等同重构前现状（不碰/恢复原剪贴板）。详见 [transcript-model spec §6](superpowers/specs/2026-06-14-transcript-model-design.md) |
| `overlay_position` | string | `"top"` | desktop | top / bottom / none |
| `segment_duration` | f64 | `5.0` | desktop | VAD 伪流式：缓冲累积时长阈值（秒） |
| `segment_silence` | f64 | `500.0` | desktop | VAD 伪流式：静音触发识别阈值（毫秒） |
| `segment_overlap` | f64 | `200.0` | desktop | VAD 伪流式：相邻分段 overlap（毫秒） |
| `polish_mode` | int | `0` | desktop | LLM 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色。**desktop 悬停工具栏可在运行时切换**（`set_polish_mode` 命令）：写 RuntimeConfig 镜像 + 持久化回 config.yaml，**立即生效**（Coordinator 每个 tick 重读镜像并 `Transcript::set_mode`，下一次润色按新模式） |
| `polish_interval` | f64 | `5.0` | desktop | 中间润色最小间隔（秒），仅 `polish_mode=2` 生效；`<=0` 回退 `1.0s` |
| `pause_polish_threshold_ms` | f64 | `600` | desktop | 停顿触发中间润色的静音阈值（毫秒），仅 `polish_mode=2` 生效；**须 > 500**（Active Flush 500ms），否则润色先于尾音冲刷、快照缺尾音 |
| `polish_llm` | string | `"bigmodel:glm-4-flashx"` | desktop | 当前润色使用的 LLM 模型，格式 `"PREFIX:NAME"`（见下方模型选择 spec） |
| `asr_hardware_accelerated` | bool | `false` | desktop + cli | ASR 推理是否启用硬件加速（CUDA/DirectML/CoreML EP），失败自动回退 CPU；不影响 VAD（VAD 固定 CPU） |
| `asr_correct` | bool | `false` | cli + server + desktop | 是否对 ASR 输出做拼音映射 + bigram 转移概率的轻量纠错/热词校正；**自动跳过 Qwen3-ASR**（其自带标点且语义纠错强），仅作用于 Whisper/SenseVoice/Paraformer/Zipformer。详见 [architecture.md §ASR 纠错](../architecture.md) |
| `denoise_enabled` | bool | `true` | desktop | 麦克风音频送入 VAD/ASR 前是否经 RNNoise 环境降噪（`nnnoiseless`，纯 Rust 内置默认模型，48kHz→频带增益+OLA，GRU 状态跨帧保持）；初始化失败自动降级直通（warn），不阻断录音。详见 [architecture.md](../architecture.md) |

> **前缀划分**：`segment_*` 控制 VAD 分段，`polish_*` 控制润色行为（包括 `polish_mode`、`polish_interval` 和新字段 `polish_llm`），`asr_*`（`asr_engine`、`asr_hardware_accelerated`、`asr_correct`）控制 ASR 引擎选择 / 推理后端 / 输出后处理。`denoise_enabled`（前缀 `denoise_`）控制麦克风环境降噪（采集层前置，VAD/ASR 前）。`pause_polish_threshold_ms`（前缀 `pause_`）亦属润色行为——停顿触发中间润色的静音阈值。`write_to_clipboard` 属粘贴行为（与 `paste_method` 同组）。`microphone` 为 cli + desktop 跨端通用字段，其余为 desktop 行为参数。

### 模型选择 spec（`asr_engine` / `polish_llm` 统一格式）

`asr_engine` 和 `polish_llm` 都使用 `"PREFIX:NAME"` 格式从 DB `models` 表定位模型：

| 写法 | 含义 | 示例 |
|------|------|------|
| `"local:NAME"` | `is_local = true AND name = NAME` | `"local:zipformer-small-ctc"` |
| `"CATEGORY:NAME"` | `category = CATEGORY AND name = NAME` | `"bigmodel:glm-4-flashx"`、`"aliyun:deepseek-v4-flash"` |
| `"NAME"`（无冒号） | 等价 `"local:NAME"`——筛 `is_local = true` | `"zipformer-small-ctc"` |

- **裸名默认走 local**：不指定前缀时视为本地模型（`is_local = true`）。远程模型必须用 category 前缀显式指定。
- **`local` 是特殊前缀**（不对应 DB `category` 值），表示筛 `is_local = true` 的本地模型。ASR 本地引擎（whisper/sensevoice/paraformer/qwen3-asr/zipformer）通常用此前缀。
- 其他前缀是 DB `models.category` 列的精确匹配（如 `bigmodel`、`deepseek`、`aliyun`）。
- 区分 category 是因为不同 category 可能有同名模型（如 `deepseek` 和 `aliyun` 下都有 `deepseek-v4-flash`）。

### 引擎选择与兜底（resolve_active_engine）

引擎激活的**唯一真相是 `config.yaml.asr_engine`**（DB `models` 表不再有 `is_active` 列）。解析逻辑（`asr::config::resolve_active_engine`）：

| `asr_engine` 值 | 解析结果 |
|---|---|
| spec 匹配到 DB 模型（如 `local:qwen3-asr-0.6B`） | 用命中引擎 |
| 空 `""` 或缺失 | 回退兜底 `zipformer-small-ctc` |
| 非空但匹配不到任何模型 | 回退兜底 + warn 日志 |

**兜底级联**：优先从 DB `models` 表 `zipformer` section 查 `zipformer-small-ctc`（用户手编 `source` 仍生效）；DB 无该条目时硬构造（靠 `DEFAULT_ASR_MODEL_DIR` 本地打包路径，保证开箱可用）。

**优先级**：显式参数 > `config.yaml.asr_engine` > 兜底。
- cli `--model`、server 请求带 `engine` 字段、`AsrEngineManager.switch_model(spec)` 都支持 spec 格式，按 spec 匹配，**不走兜底**（匹配不到直接报错）。
- `resolve_active_engine` 仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。

### 完整示例

```yaml
# 麦克风（留空用系统默认）
microphone: ""

# ASR 引擎（格式 "PREFIX:NAME"，见上方「模型选择 spec」节）
# local: → is_local=true 的本地模型；CATEGORY: → 按 category 精确匹配
# 留空或匹配不到则回退兜底 local:zipformer-small-ctc
asr_engine: "local:qwen3-asr-0.6B"
language: "auto"

# 引擎接入模式
engine_mode: "embedded"          # embedded | websocket | grpc

# 桌面交互
shortcut: "CmdOrCtrl+Shift+Space"
paste_method: "clipboard"        # clipboard | direct | none
write_to_clipboard: true         # 粘贴后是否把识别结果留在剪贴板（false = 等同重构前现状）
overlay_position: "top"          # top | bottom | none

# VAD 伪流式分段（离线引擎）
segment_duration: 5.0            # 秒
segment_silence: 500.0           # 毫秒
segment_overlap: 200.0           # 毫秒

# LLM 润色（可选）
polish_mode: 0                   # 0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
polish_interval: 5.0             # 秒，仅 polish_mode=2 生效（中间润色最小间隔）
pause_polish_threshold_ms: 600   # 毫秒，仅 polish_mode=2 生效（停顿触发润色的静音阈值，须 > 500）
polish_llm: "bigmodel:glm-4-flashx"  # 润色模型，格式 "PREFIX:NAME"（见模型选择 spec）；provider/base_url/API Key 保存于 SQLite 的 models 表中
asr_hardware_accelerated: false  # true 启用 GPU/CoreML/DirectML 加速（失败回退 CPU）；VAD 不受影响
asr_correct: false               # true 对 ASR 输出做拼音+bigram 轻量纠错（自动跳过 Qwen3-ASR）
denoise_enabled: true            # false 关闭麦克风环境降噪（RNNoise）；初始化失败自动降级直通
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
