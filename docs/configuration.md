# 配置指南

octopus 配置分两部分：

- **模型配置**：`~/.octopus/octopus.db` 的 `models` 表（SQLite，唯一来源）
- **应用配置**：`~/.octopus/octopus.db` 的 `app_config` 表（SQLite，v3+ 替代旧 config.yaml）

首次启动自动建库 + 写入默认引擎 + 应用配置，开箱即用。

## 目录结构

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite：models 表 + transcriptions 表 + app_config 表（唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（首次启动自动生成，可安全删除）
├── VOICE_POLISH.md     # 润色 system prompt 自定义覆盖（可选）
└── models/             # 随应用打包的小模型（固定路径）
    ├── silero_vad_v4.onnx   # VAD（固定加载，不进 DB）
    └── zipformer/           # 默认 ASR（model.int8.onnx + tokens.txt）

~/.cache/huggingface/hub/   # 大模型 HF 缓存（whisper/sensevoice/qwen3/paraformer 等，按需下载）
```

## 模型配置（octopus.db）

模型配置唯一来源是 `models` 表。首次建库时自动执行 [`db.sql`](../crates/infra/src/db.sql)（`include_str!` 编译期嵌入），写入默认 ASR 引擎与 LLM 润色模型。

### models 表 schema

| 列 | 含义 |
|---|---|
| `domain` | `asr` / `llm` |
| `provider` | **vendor / 运行位置**——`local`（本地）/ `aliyun`（阿里云 DashScope）/ `bytedance`（字节跳动火山引擎）/ `tencent`（腾讯云）/ `baidu`（百度智能云）/ `deepseek` / `bigmodel` |
| `category` | ASR=引擎族（`zipformer`/`whisper`/`sensevoice`/`paraformer`/`qwen3-asr`/`Fun-ASR`）；LLM=模型系列（`qwen`/`glm`/`deepseek`） |
| `model_name` | 具体模型标识（精确匹配） |
| `source` | ASR=本地路径 / HF repo / 云 WS 端点；LLM=API Base URL |
| `secret_key` | 远程 API Key（本地模型留空） |
| `language` / `description` | 语种 / 描述 |
| `is_local` | 是否本地模型（`provider='local'` ⟺ `is_local=1`，二者并存） |
| `is_thinking` | LLM 专用：是否思考（reasoning）模型 |
| `is_streaming` | ASR 是否支持流式 |
| `is_enabled` | 是否启用（`0` 禁用、`1` 启用） |

**唯一键**：`UNIQUE(domain, provider, category, model_name)`——允许同名模型跨 provider 共存（如 `deepseek-v4-flash` 在 deepseek 直连与 aliyun 代管下各一行）。

### provider × category taxonomy

| `provider` | ASR（`category` = 引擎族） | LLM（`category` = 模型系列） |
|---|---|---|
| `local` | `zipformer` / `whisper` / `sensevoice` / `paraformer` / `qwen3-asr` | —（暂无本地 LLM） |
| `aliyun` | `Fun-ASR`（FunASR Realtime WS） | `qwen`（DashScope OpenAI 兼容端点） / `deepseek`（经 DashScope 代管） |
| `bytedance` | `Doubao-ASR`（豆包大模型 ASR 双向流式） | — |
| `tencent` | `Tencent-ASR`（腾讯云实时语音识别） | — |
| `baidu` | `Baidu-ASR`（百度智能云实时语音识别） | — |
| `deepseek` | — | `deepseek`（直连 api.deepseek.com） |
| `bigmodel` | — | `glm`（智谱开放平台） |

### 默认 ASR seed（8 local + 1 aliyun + 2 bytedance + 2 tencent + 1 baidu）

| provider | category | model_name | source | is_local | is_enabled | is_streaming |
|---|---|---|---|---|---|---|
| local | zipformer | zipformer-small-ctc | `models/zipformer`（本地打包，**兜底引擎**） | 1 | 1 | 1 |
| local | zipformer | zipformer-multi | k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13 | 1 | 0 | 1 |
| local | zipformer | zipformer-ctc | csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30 | 1 | 0 | 1 |
| local | zipformer | zipformer-zh-transducer | csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30 | 1 | 0 | 1 |
| local | zipformer | zipformer-xlarge-transducer | csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30 | 1 | 0 | 1 |
| local | paraformer | paraformer-streaming | csukuangfj/sherpa-onnx-streaming-paraformer-zh | 1 | 0 | 1 |
| local | sensevoice | sherpa-onnx-sense-voice-funasr-nano-int8 | csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17 | 1 | 0 | 0 |
| local | qwen3-asr | qwen3-asr-0.6B | csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25 | 1 | 0 | 0 |
| local | qwen3-asr | qwen3-asr-1.7B | ilmina/qwen3-asr-1.7b-sherpa-onnx | 1 | 0 | 0 |
| local | whisper | whisper-small | onnx-community/whisper-small.en | 1 | 0 | 0 |
| aliyun | Fun-ASR | fun-asr-2025-11-07 | `wss://dashscope.aliyuncs.com/api-ws/v1/inference` | 0 | 0 | 0 |
| bytedance | Doubao-ASR | doubao-asr-1.0-streaming | `volc.bigasr.sauc.duration` | 0 | 0 | 1 |
| bytedance | Doubao-ASR | doubao-asr-2.0-streaming | `volc.seedasr.sauc.duration` | 0 | 0 | 0 |
| tencent | Tencent-ASR | 16k_zh | `{appid}:{secretid}` | 0 | 0 | 1 |
| tencent | Tencent-ASR-Multi | 16k_zh_en | `{appid}:{secretid}` | 0 | 0 | 0 |
| baidu | Baidu-ASR | 15372 | `{appid}` | 0 | 0 | 1 |

> **bytedance `source` 字段**是 Resource ID（如 `volc.bigasr.sauc.duration`），不是 endpoint URL。
> **tencent `source` 字段**是 `{appid}:{secretid}` 复合格式（冒号分隔），`secret_key` 填 SecretKey（签名密钥）。endpoint 固定为 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`。
> **baidu `source` 字段**是 AppID（纯数字），`secret_key` 填 API Key（appkey），`model_name` 是 dev_pid（如 `15372`）。endpoint 固定为 `wss://vop.baidu.com/realtime_asr`。

### 默认 LLM seed（含 aliyun qwen / deepseek 经 DashScope）

| provider | category | model_name | source (base_url) | is_thinking | is_enabled | 说明 |
|---|---|---|---|---|---|---|
| deepseek | deepseek | deepseek-v4-flash | `https://api.deepseek.com/` | 1 | 0 | DeepSeek V4 Flash（思考模型） |
| aliyun | deepseek | deepseek-v4-flash | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 1 | 0 | DeepSeek V4 Flash 经 DashScope（思考模型） |
| bigmodel | glm | glm-4-flashx | `https://open.bigmodel.cn/api/paas/v4` | 0 | 0 | 智谱 GLM-4 FlashX（非思考） |
| bigmodel | glm | glm-4.5-flash | `https://open.bigmodel.cn/api/paas/v4` | 1 | 0 | 智谱 GLM-4.5 Flash（思考模型） |
| aliyun | qwen | qwen-plus | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 0 | 0 | Qwen Plus（非思考） |
| aliyun | qwen | qwen-turbo | `https://dashscope.aliyuncs.com/compatible-mode/v1` | 0 | 0 | Qwen Turbo（非思考，快） |

> **`provider` 字段**：vendor / 运行位置维度，与 `category`（引擎族/模型系列）正交。`local` 表示随应用打包或下载到本地，`aliyun` 表示经阿里云 DashScope 云端调用，`bytedance` 表示经火山引擎豆包云端调用。决定引擎路由（`provider='aliyun'` → `EngineCategory::Aliyun` → `AliyunEngine`；`provider='bytedance'` → `EngineCategory::ByteDance` → `Stage::CloudStreaming`）。

> **`is_local` 字段**：标记是否本地运行。`provider='local'` ⟺ `is_local=1`（二者并存：`is_local` 供本地过滤，`provider` 用于 vendor 路由）。

> **`is_thinking` 字段**：标记该模型是否为思考（reasoning）模型。思考模型在润色等明确任务中若不关闭思考，`content` 可能为空（token 被 `reasoning_content` 耗尽）。置为 `1` 时程序自动发送关闭思考的参数——DeepSeek 用 `thinking: {type: "disabled"}`，BigModel 用 `enable_thinking: false`。

> **`is_enabled` 字段**：标记是否启用。`1` 表示启用，`0` 表示禁用。只有启用的模型才会被系统加载或供识别/润色使用。阿里云 qwen / Fun-ASR seed 默认 `is_enabled=0`，用户填 API Key 后改为 `1` 启用。

> **`is_streaming` 字段**：标记 ASR 模型是否支持流式识别。`1` 表示流式（zipformer CTC×3 + Transducer×2 / paraformer-streaming，走本地流式 partial），`0` 表示非流式（sensevoice / qwen3-asr / whisper / aliyun Fun-ASR，走 VAD 分段伪流式）。`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming`，数据驱动、不再按 category 硬编码。

> **远程 API Key 配置方式（`secret_key`）**：LLM / 云端 ASR 的所有参数（包括 Base URL / WS 端点 和 API Key）全部存储在 DB `models` 表。`source` 存端点 URL，`secret_key` 存 API Key。可通过 SQLite 客户端手动填入（具体填法见下方「阿里云云端 API」小节）。

> **不再有 `is_active` 列**：引擎激活改由 `app_config.asr_engine` 决定（见下方「引擎选择与兜底」）。`zipformer-small-ctc` 是兜底引擎——`asr_engine` 为空或匹配不到任何模型时，自动回退到它（靠本地打包路径，开箱可用）。

### 阿里云云端 API 接入

通过 `provider='aliyun'` 接入两个阿里云 DashScope（百炼）云端能力。LLM 走 OpenAI 兼容端点（**零代码**，`llm/client.rs` 不改），ASR 走 FunASR Realtime WebSocket。

#### 1. 填 DashScope API Key（`secret_key`）

DashScope API Key 暂无 UI 入口，需手动 sqlite3 填入对应模型行的 `secret_key` 字段：

```bash
# ASR（FunASR Realtime WS）
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-你的dashscope-key', is_enabled=1 WHERE domain='asr' AND model_name='fun-asr-2025-11-07';"

# LLM（DashScope OpenAI 兼容）
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-你的dashscope-key', is_enabled=1 WHERE domain='llm' AND model_name='qwen-plus';"
```

填后需**重启进程**生效（`OnceLock` 缓存，运行中不热更新）。

#### 2. LLM 润色（零代码）

seed 已含 `aliyun:qwen:qwen-plus` / `qwen-turbo`（DashScope OpenAI 兼容端点 `https://dashscope.aliyuncs.com/compatible-mode/v1`）。填 Key + `is_enabled=1` 后，配置：

```yaml
polish_llm: "aliyun:qwen:qwen-plus"
```

即走 DashScope OpenAI 兼容路径（与 deepseek/bigmodel 同一 `polish()` 代码）。

#### 3. ASR 识别（FunASR Realtime WS）

seed 已含 `aliyun:Fun-ASR:fun-asr-2025-11-07`（WS 端点 `wss://dashscope.aliyuncs.com/api-ws/v1/inference`，`is_streaming=0` 走桌面分块路径）。填 Key + `is_enabled=1` 后，配置：

```yaml
asr_engine: "aliyun:Fun-ASR:fun-asr-2025-11-07"
```

**云引擎路由**：启动时 `resolve_active_engine(&config.asr_engine)` 按 `provider='aliyun'` 解析为 `EngineCategory::Aliyun` → 建 `AliyunEngine`；`provider='bytedance'` 解析为 `EngineCategory::ByteDance` → 直接走 `Stage::CloudStreaming`（无独立 engine）；否则按 `engine_mode`（embedded/websocket/grpc）走本地引擎。云 ↔ 本地切换改 `app_config.asr_engine` 后**重启**生效（engine 实例启动时固定）。

> **注意**：`engine_category_from_str` 对 `"aliyun"` / `"bytedance"` 均返回 `None`——云 provider 不靠 `category` 字符串识别，而由 `resolve_category(provider, category)` 按 provider 分支识别（不进 5 个本地族字符串映射）。

#### 4. 启用 `cloud` cargo feature

云 ASR 引擎（`AliyunEngine` + 各 provider 的 `*_stream::open`）在 `cloud` feature 后，默认不开（与 `remote-ws` / `remote-grpc` 一致）：

```bash
cargo run -p octopus-desktop --features cloud
```

### 字节跳动豆包 ASR 接入

通过 `provider='bytedance'` 接入火山引擎豆包大模型 ASR（双向流式优化版 `bigmodel_async`）。与阿里云不同：**二进制帧协议**（4B header + gzip payload，非 JSON 文本帧）、**固定 endpoint**（不来自 DB `source` 字段）、**Resource ID 鉴权**（`X-Api-Key` + `X-Api-Resource-Id` 握手 headers）。

#### DB 字段映射

| DB 字段 | 豆包含义 | 示例 |
|---|---|---|
| `source` | **Resource ID**（作为 `X-Api-Resource-Id` 握手 header） | `volc.bigasr.sauc.duration` |
| `secret_key` | **API Key**（作为 `X-Api-Key` 握手 header） | 你的火山引擎 API Key |
| `model_name` | 模型标识（seed 内固定，用于 DB 查找） | `doubao-asr-1.0-streaming` |

> Endpoint 固定为 `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`，**不存 DB**。

#### 1. 填 API Key

```bash
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='你的火山-api-key', is_enabled=1 WHERE domain='asr' AND model_name='doubao-asr-1.0-streaming';"
```

#### 2. 配置引擎

```yaml
asr_engine: "bytedance:Doubao-ASR:doubao-asr-1.0-streaming"
```

seed 还含 `doubao-asr-2.0-streaming`（`volc.seedasr.sauc.duration`），二选一即可。

#### 3. feature 依赖

与阿里云共用 `cloud` feature（控制 WS 流式编译），无需额外 feature flag。

### 腾讯云 ASR 接入

通过 `provider='tencent'` 接入腾讯云实时语音识别 WebSocket API。与阿里云/字节跳动不同：**URL 签名鉴权（HMAC-SHA1）**、**原始 PCM binary 帧**、**JSON 文本响应**、**`{"type":"end"}` 结束信号**。

#### DB 字段映射

需要 3 个鉴权信息：AppID、SecretID、SecretKey。

| DB 字段 | 腾讯含义 | 示例 |
|---|---|---|
| `source` | **`{appid}:{secretid}` 复合字段**（冒号分隔） | `1259221234:AKIDxxxxxxxx` |
| `secret_key` | **SecretKey**（HMAC-SHA1 签名密钥） | `yyyyyyyyyy` |
| `model_name` | engine_model_type（直接作为 URL 参数） | `16k_zh`、`16k_zh_en` |

> Endpoint 固定为 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`，**不存 DB**。

#### 1. 填 AppID + SecretID + SecretKey

```bash
# source 格式 = appid:secretid
sqlite3 ~/.octopus/octopus.db "UPDATE models SET source='你的appid:你的secretid', secret_key='你的secretkey', is_enabled=1 WHERE domain='asr' AND model_name='16k_zh';"
```

#### 2. 配置引擎

```yaml
asr_engine: "tencent:Tencent-ASR:16k_zh"
```

seed 还含 `16k_zh_en`（普方英大模型，支持中英+31种方言），二选一即可。

#### 3. feature 依赖

与阿里云/字节跳动共用 `cloud` feature，无需额外 feature flag。

### 百度智能云 ASR 接入

通过 `provider='baidu'` 接入百度智能云实时语音识别 WebSocket API。与腾讯云不同：**START 帧 JSON 鉴权**（appid+appkey 在 `data` 内，无 HMAC 签名）、**Raw PCM binary 帧**、**JSON text 响应**（`MID_TEXT` 临时 / `FIN_TEXT` 稳态）。

#### DB 字段映射

| DB 字段 | 百度含义 | 示例 |
|---|---|---|
| `source` | **AppID**（纯数字） | `1050000017` |
| `secret_key` | **API Key**（appkey） | `UA4oPSxxxxkGOuFbb6` |
| `model_name` | **dev_pid**（语种模型 ID） | `15372`（中文加强标点）、`17372`（英文加强标点） |

> Endpoint 固定为 `wss://vop.baidu.com/realtime_asr?sn=<UUID>`，**不存 DB**。
> 百度实时识别不使用 access_token / SecretKey，鉴权全在 START 帧。

#### 1. 填 AppID + API Key

```bash
sqlite3 ~/.octopus/octopus.db "UPDATE models SET source='你的AppID', secret_key='你的APIKey', is_enabled=1 WHERE domain='asr' AND model_name='15372';"
```

#### 2. 配置引擎

```yaml
asr_engine: "baidu:Baidu-ASR:15372"
```

`model_name` 是 dev_pid，常用取值：`15372`（中文加强标点）、`15376`（中文多方言）、`17372`（英文加强标点）。

#### 3. feature 依赖

与其他云端 provider 共用 `cloud` feature，无需额外 feature flag。

#### 5. schema 变更：删库重建（dev 阶段）

`models` 表 schema 变更（加 `provider` 列、`name`→`model_name`、唯一键改 4 字段）后，开发期直接删库重新初始化（不写 ALTER 迁移）：

```bash
rm -f ~/.octopus/octopus.db
# 下次启动 ensure_db 重建新 schema + seed
```

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
| HF repo 名 | `~/.cache/huggingface/hub/` | `onnx-community/whisper-small.en` → HF 缓存 |

### 手编 DB

`models` 表可手动编辑（增删模型条目），但**需重启进程生效**——`asr::load_config()` 首次读出后缓存到 `OnceLock`，运行中不热更新。引擎激活改由 `app_config.asr_engine` 决定（`models` 表不再有 `is_active` 列）。**唯一键** `UNIQUE(domain, provider, category, model_name)` 允许跨 provider 同名模型共存（如 deepseek-v4-flash 在 deepseek 直连与 aliyun 代管下各一行）。

> **开发阶段 schema 变更**：直接修改 [`crates/infra/src/db.sql`](../crates/infra/src/db.sql)，然后删除 `~/.octopus/octopus.db` 并重启即可重新初始化。无迁移逻辑，开发期以此替代。

### model.json / history.txt 已废弃

`model.json`（旧模型配置）和 `history.txt`（旧识别历史）已在 DB 单一源重构中彻底删除，DB 是唯一来源。详见 [db-single-source 设计](superpowers/specs/2026-06-14-archived-design.md)。

## 应用配置（app_config 表）

应用行为配置，v3+ 统一存储在 `~/.octopus/octopus.db` 的 `app_config` 表（key-value TEXT）。首次启动由 db.sql seed 默认值；旧 `config.yaml` 自动迁移到 DB 后重命名为 `.bak`。

**两种编辑方式**：
1. **GUI 设置窗口**（推荐）：桌面应用工具栏点击「设置」按钮或托盘菜单「设置...」打开独立设置窗口——系统设置页提供表单化编辑（toggle/select/number input），修改即时写回 DB `app_config` 表 + RuntimeConfig。21 个可配置字段均有类型校验和生效时间提示（立即 / 下次录音 / 重启）。
2. **手动编辑**：直接用 sqlite3 编辑 `~/.octopus/octopus.db` 的 `app_config` 表，需重启进程生效（`OnceLock` 缓存）。

> **⚠️ 迁移提示**：旧 `config.yaml` 首次启动 v3 版本时自动导入 DB 并重命名为 `config.yaml.bak`。旧字段 `polish_enabled` / `shortcut` / `polish_interval` 在迁移时自动转换为 `polish_mode` / `asr_shortcut` / `polish_min_interval`。

| 字段 | 类型 | 默认值 | 适用端 | 说明 |
|---|---|---|---|---|
| `microphone` | string | `""` | cli + desktop | 麦克风设备名（空 = 系统默认） |
| `asr_engine` | string | `""` | desktop + server | ASR 引擎选择，格式 `"{provider}:{category}:{model_name}"`（见下方「模型选择 spec」）。示例：`"local:zipformer:zipformer-small-ctc"`、`"aliyun:Fun-ASR:fun-asr-2025-11-07"`、`"bytedance:Doubao-ASR:doubao-asr-1.0-streaming"`、`"tencent:Tencent-ASR:16k_zh"`、`"baidu:Baidu-ASR:15372"`。空或匹配不到回退兜底 `local:zipformer:zipformer-small-ctc`。显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，不走兜底。**desktop 悬停工具栏可在运行时切换**（`switch_asr_engine` 命令）：写 RuntimeConfig + 持久化回 DB `app_config` 表，**下次录音生效**（Coordinator 在 Toggle 进入 Idle 时重读镜像并重建引擎）。`provider='aliyun'` / `provider='bytedance'` / `provider='tencent'` / `provider='baidu'` 路由云端引擎（需开 `aliyun` feature），改 `provider` 后**重启**生效 |
| `language` | string | `"auto"` | desktop | auto / zh / en / ja / ko |
| `engine_mode` | string | `"embedded"` | desktop | embedded / websocket / grpc |
| `remote_url` | string | `ws://127.0.0.1:3000/ws/stream` | desktop | websocket 模式远程地址 |
| `grpc_endpoint` | string | `http://127.0.0.1:50051` | desktop | grpc 模式端点 |
| `asr_shortcut` | string | `CmdOrCtrl+Shift+Space` | desktop | 全局 ASR 激活/关闭快捷键（Tauri Accelerator 格式）。GUI 设置页可配（快捷键捕获按钮 + `check_shortcut` 冲突检测 + 热重载）。旧字段名 `shortcut` 经 serde alias 向后兼容 |
| `paste_method` | string | `"clipboard"` | desktop | clipboard / direct / none |
| `write_to_clipboard` | bool | `true` | desktop | 粘贴完成后是否把识别结果写入剪贴板（方便他处再粘贴）；`false` 时三模式等同重构前现状（不碰/恢复原剪贴板）。详见 [transcript-model spec §6](superpowers/specs/2026-06-14-archived-design.md) |
| `overlay_position` | string | `"top"` | desktop | top / bottom / none。**已废弃**（2026-06-21 审查修复）：`recording_overlay` 窗口及 `overlay.rs` 模块已整体删除，UI 统一到 `result_window`。字段保留于 config 结构（避免 DB schema 迁移），但无任何使用方 |
| `segment_silence` | f64 | `400.0` | desktop | VAD 伪流式：句间停顿阈值（毫秒），起过此值的停顿触发切句识别 |
| `polish_mode` | int | `0` | desktop | LLM 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色。**desktop 悬停工具栏可在运行时切换**（`set_polish_mode` 命令）：写 RuntimeConfig + 持久化回 DB，**立即生效**（Coordinator 每个 tick 重读镜像并 `Transcript::set_mode`，下一次润色按新模式） |
| `polish_min_interval` | f64 | `5.0` | desktop | 中间润色最小间隔（秒，节流用），仅 `polish_mode=2` 生效；`<=0` 回退 `1.0s`。旧名 `polish_interval` 迁移时自动重命名 |
| `pause_polish_threshold_ms` | f64 | `600` | desktop | 停顿触发中间润色的静音阈值（毫秒），仅 `polish_mode=2` 生效；**须 >= 600**（须大于句间停顿最大值 600ms），否则润色先于尾音冲刷、快照缺尾音。GUI 设置页改为下拉（600~1000ms 五档），label 名为「润色停顿阈值」 |
| `polish_llm` | string | `"bigmodel:glm:glm-4-flashx"` | desktop | 当前润色使用的 LLM 模型，格式 `"{provider}:{category}:{model_name}"`（见下方「模型选择 spec」）。示例：`"bigmodel:glm:glm-4-flashx"`、`"aliyun:qwen:qwen-plus"`、`"deepseek:deepseek:deepseek-v4-flash"`。**留空 `""` = 不选择模型（不润色）**；该模型在 DB 找不到时，工具栏回退「不选择模型」并图标置灰（见 [toolbar spec §16.4](superpowers/specs/2026-06-16-archived-design.md)） |
| `asr_hardware_accelerated` | bool | `false` | desktop + cli | ASR 推理是否启用硬件加速（CUDA/DirectML/CoreML EP），失败自动回退 CPU；不影响 VAD（VAD 固定 CPU） |
| `asr_correct` | bool | `false` | cli + server + desktop | 是否对 ASR 输出做拼音映射 + bigram 转移概率的轻量纠错/热词校正；**自动跳过 Qwen3-ASR**（其自带标点且语义纠错强），仅作用于 Whisper/SenseVoice/Paraformer/Zipformer。详见 [architecture.md §ASR 纠错](../architecture.md) |
| `denoise_mode` | u8 | `1` | desktop | 环境降噪模式：`0`=关闭（直通）、`1`=RNNoise（`nnnoiseless`，默认，纯 Rust 内置默认模型，48kHz→频带增益+OLA，GRU 状态跨帧保持）、`2`=DeepFilterNet3（libDF v0.5.6 + tract 0.19，48kHz 全频带，编译期内嵌 ~7.9MB 模型，质量最佳）。降噪为可插拔后端（`FrameDenoise` trait），由 mode 选后端；亦可由工具栏运行时切换（`set_denoise_mode` 命令）并持久化回 DB `app_config` 表。初始化/推理失败自动降级直通（warn），不阻断录音。详见 [architecture.md](../architecture.md) |
| `output_simplified` | bool | `true` | desktop | ASR 输出字形归一化：`true`→简体（繁→简），`false`→繁体（简→繁）。基于开放词典网 CC-BY 3.0 单字对照表（编译期嵌入），在 ASR 输出后做单字级字形转换（不转地域用词）。解决 Qwen3-ASR `auto` 模式输出繁体的问题。详见 [architecture.md](../architecture.md) |
| `hide_toolbar` | bool | `true` | desktop | 结果展示区工具栏显隐模式：`true`→鼠标移入显示、移出隐藏（默认）；`false`→工具栏始终显示（窗口高度保持展开态 132px） |
| `edit_shortcut` | string | `"Cmd+Enter"` | desktop | 结果展示区编辑 toggle 快捷键——**进入与保存（退出）编辑都用此键**（与 ✏️ 按钮同语义，Tauri Accelerator 格式，窗口内、仅结果窗聚焦时生效）。GUI 设置页可配（快捷键捕获按钮，不需冲突检测——仅窗口内 keydown 判定）。曾用双击进入（WKWebView `dblclick` 难触发而弃用）；曾拆分「Cmd+E 进 / Cmd+Enter 存」，因两者均窗口内 keydown（非全局、不 hijack 系统）已统一为单键 toggle |

> **前缀划分**：`segment_*` 控制 VAD 分段，`polish_*` 控制润色行为（包括 `polish_mode`、`polish_interval` 和新字段 `polish_llm`），`asr_*`（`asr_engine`、`asr_hardware_accelerated`、`asr_correct`）控制 ASR 引擎选择 / 推理后端 / 输出后处理。`denoise_mode`（前缀 `denoise_`）控制麦克风环境降噪（采集层前置，VAD/ASR 前）。`pause_polish_threshold_ms`（前缀 `pause_`）亦属润色行为——停顿触发中间润色的静音阈值。`write_to_clipboard` 属粘贴行为（与 `paste_method` 同组）。`microphone` 为 cli + desktop 跨端通用字段，其余为 desktop 行为参数。

### 模型选择 spec（`asr_engine` / `polish_llm` 统一 3-part 格式）

`asr_engine` 和 `polish_llm` 都使用 `"{provider}:{category}:{model_name}"` 三段格式从 DB `models` 表唯一定位模型（按 `WHERE domain=? AND provider=? AND category=? AND model_name=?` 四字段精确匹配）：

| 写法 | 含义 | 示例 |
|------|------|------|
| `"{provider}:{category}:{model_name}"`（3 段） | 4 字段精确匹配，跨 provider/category 区分同名模型 | `"local:zipformer:zipformer-small-ctc"`、`"aliyun:Fun-ASR:fun-asr-2025-11-07"`、`"bytedance:Doubao-ASR:doubao-asr-1.0-streaming"`、`"tencent:Tencent-ASR:16k_zh"`、`"baidu:Baidu-ASR:15372"`、`"aliyun:qwen:qwen-plus"`、`"deepseek:deepseek:deepseek-v4-flash"`、`"bigmodel:glm:glm-4-flashx"` |
| `"{model_name}"`（裸名，无冒号） | **仅全局默认 fallback 路径用**——跨 provider/category 按 `model_name` 搜，优先 `provider='local'` | `"zipformer-small-ctc"` |
| `"{x}:{y}"`（旧 2-part 1 冒号） | 视为非法格式，记录 warn + 按裸名兜底（迁移期用户更新配置；删库重建后 seed 已是 3-part） | 旧 `"bigmodel:glm-4-flashx"` → warn + 裸名搜索 |

- **区分 provider 与 category**：因为不同 provider 下可有同名模型（`deepseek-v4-flash` 在 deepseek 直连与 aliyun 代管下各一行），不同 category 也可有同名模型。
- **`provider='aliyun'` / `provider='bytedance'` / `provider='tencent'` / `provider='baidu'` 路由云端**：解析时若 `provider='aliyun'` → `EngineCategory::Aliyun` → 云引擎 `AliyunEngine`（ASR）；`bytedance` → `ByteDance` → `Stage::CloudStreaming`（豆包流式）；`tencent` → `Tencent` → `Stage::CloudStreaming`（腾讯流式）；`baidu` → `Baidu` → `Stage::CloudStreaming`（百度流式）；否则按 `category` 映射到本地引擎族。
- **DB 查询统一 4 字段精确匹配**：`parse_model_spec` 解析后，`load_models_at` / `load_llm_model` / `resolve_engine_in_config` 均按 `provider + category + model_name` 查询。

### 引擎选择与兜底（resolve_active_engine）

引擎激活的**唯一真相是 `app_config.asr_engine`**（DB `models` 表不再有 `is_active` 列）。解析逻辑（`asr::config::resolve_active_engine`）：

| `asr_engine` 值 | 解析结果 |
|---|---|
| 3-part spec 匹配到 DB 模型（如 `local:qwen3-asr:qwen3-asr-0.6B`） | 用命中引擎 |
| 空 `""` 或缺失 | 回退兜底 `zipformer-small-ctc` |
| 非空但匹配不到任何模型 | 回退兜底 + warn 日志 |

**兜底级联**：优先从 DB `models` 表 `zipformer` section 查 `zipformer-small-ctc`（用户手编 `source` 仍生效）；DB 无该条目时硬构造（靠 `DEFAULT_ASR_MODEL_DIR` 本地打包路径，保证开箱可用）。

**优先级**：显式参数 > `app_config.asr_engine` > 兜底。
- cli `--model`、server 请求带 `engine` 字段、`AsrEngineManager.switch_model(spec)` 都支持 spec 格式，按 spec 匹配，**不走兜底**（匹配不到直接报错）。
- `resolve_active_engine` 仅服务「全局默认」（server 启动 preheat、请求未带 engine 时）。

### 完整示例

```yaml
# 麦克风（留空用系统默认）
microphone: ""

# ASR 引擎（格式 "{provider}:{category}:{model_name}"，见上方「模型选择 spec」节）
# local:zipformer:zipformer-small-ctc → 本地引擎；aliyun:Fun-ASR:fun-asr-2025-11-07 → 云端 DashScope
# 留空或匹配不到则回退兜底 local:zipformer:zipformer-small-ctc
asr_engine: "local:qwen3-asr:qwen3-asr-0.6B"
language: "auto"

# 引擎接入模式
engine_mode: "embedded"          # embedded | websocket | grpc

# 桌面交互
asr_shortcut: "CmdOrCtrl+Shift+Space"  # 全局 ASR 激活/关闭快捷键（旧字段名 shortcut 仍兼容）
paste_method: "clipboard"        # clipboard | direct | none
write_to_clipboard: true         # 粘贴后是否把识别结果留在剪贴板（false = 等同重构前现状）
overlay_position: "top"          # top | bottom | none

# VAD 伪流式分段（离线引擎）
segment_silence: 400.0           # 句间停顿阈值（毫秒）

# LLM 润色（可选）
polish_mode: 0                   # 0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
polish_min_interval: 5.0         # 秒，仅 polish_mode=2 生效（中间润色最小间隔；旧名 polish_interval 迁移时自动重命名）
pause_polish_threshold_ms: 600   # 毫秒，仅 polish_mode=2 生效（停顿触发润色的静音阈值，须 >= 600）
polish_llm: "bigmodel:glm:glm-4-flashx"  # 润色模型，格式 "{provider}:{category}:{model_name}"（见模型选择 spec）；provider/base_url/API Key 保存于 SQLite 的 models 表中
asr_hardware_accelerated: false  # true 启用 GPU/CoreML/DirectML 加速（失败回退 CPU）；VAD 不受影响
asr_correct: false               # true 对 ASR 输出做拼音+bigram 轻量纠错（自动跳过 Qwen3-ASR）
denoise_mode: 1                  # 环境降噪：0=关闭直通 / 1=RNNoise（默认）/ 2=DeepFilterNet3（48kHz 全频带，~7.9MB 模型）；亦可工具栏运行时切换（set_denoise_mode）
output_simplified: true          # ASR 输出字形：true=简体（繁→简），false=繁体（简→繁）
hide_toolbar: true               # 结果窗工具栏：true=hover 显隐（默认），false=始终显示
edit_shortcut: "Cmd+Enter"       # 编辑 toggle 快捷键（窗口内，进入/保存都用此键）
```

### 结果展示区编辑

录音过程中可随时修正识别/润色文本：
- **进入编辑**：按 `edit_shortcut`（默认 `Cmd+Enter`，窗口内），或点工具栏 ✏️ 编辑按钮。
- **编辑期间 ASR 硬暂停**（音频丢弃），改完恢复。
- **退出编辑**：再按 `edit_shortcut`（与进入同键，toggle 语义），或点工具栏 ✏️→💾 按钮保存。（曾用「完成编辑」按钮 + 固定 `Cmd+Enter`，前者已删、后者已统一为 `edit_shortcut`。）
- 编辑后的文本作为后续展示与润色基准；新识别文本追加其上；停止粘贴时保留编辑。
- 编辑后再触发润色时，仅润色新增部分、保留已编辑（润色结果折回）。
- 未编辑时行为与旧版完全一致。

## 模型下载

随应用打包的小模型（VAD + 默认 zipformer）无需下载。其他大模型用 `huggingface-cli` 按需下载到 HF 缓存：

```bash
# 安装 HF CLI
pip install huggingface_hub

# 下载（source 字段即 HF repo 名）
huggingface-cli download onnx-community/whisper-small.en
huggingface-cli download csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17
huggingface-cli download csukuangfj/sherpa-onnx-streaming-paraformer-zh
# Zipformer Transducer（RNN-T，三 session：encoder + decoder + joiner）
huggingface-cli download csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30
huggingface-cli download csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30
```

下载后存入 `~/.cache/huggingface/hub/`，DB `models` 表中 `source` 为 HF repo 名的引擎会经 `resolve_model_dir` 自动定位到对应缓存。
