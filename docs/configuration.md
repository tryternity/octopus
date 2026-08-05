# 配置指南

octopus 配置分两部分：

- **模型配置**：`~/.octopus/octopus.db` 的 `models` 表（SQLite，唯一来源）
- **应用配置**：`~/.octopus/octopus.db` 的 `app_config` 表（SQLite，v3+ 替代旧 config.yaml）

首次启动自动建库 + 写入默认引擎 + 应用配置，开箱即用。

## 目录结构

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite：models + transcriptions + app_config + prompts 表（唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（首次启动自动生成，可安全删除）
└── models/             # 随应用打包的小模型（固定路径）
    ├── vad.onnx   # VAD 覆盖（可选——通用名，放任意 VAD 模型覆盖内嵌 silero_vad_v6；不进 DB）
    └── zipformer/           # 默认 ASR（model.int8.onnx + tokens.txt）

~/.cache/huggingface/hub/   # 大模型 HF 缓存（whisper/sensevoice-orig/qwen3/paraformer/firered 等，按需下载）
```

## 模型配置（octopus.db）

模型配置唯一来源是 `models` 表。首次建库时自动执行 [`schema.sql`](../crates/infra/resources/sql/schema.sql)（`include_str!` 编译期嵌入），写入默认 ASR 引擎与 LLM 润色模型。

### models 表 schema

| 列 | 含义 |
|---|---|
| `domain` | `asr` / `llm` |
| `provider` | **vendor / 运行位置**——`local`（本地）/ `aliyun`（阿里云 DashScope）/ `bytedance`（字节跳动火山引擎）/ `tencent`（腾讯云）/ `baidu`（百度智能云）/ `deepseek` / `bigmodel` |
| `category` | ASR=引擎族（`zipformer`/`whisper`/`sensevoice-orig`/`paraformer`/`qwen3-asr`/`moonshine`/`firered`/`Fun-ASR`）；LLM=模型系列（`qwen`/`glm`/`deepseek`） |
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
| `local` | `zipformer` / `whisper` / `sensevoice-orig` / `paraformer` / `qwen3-asr` / `moonshine` / `firered` | —（暂无本地 LLM） |
| `aliyun` | `Fun-ASR`（FunASR Realtime WS） | `qwen`（DashScope OpenAI 兼容端点） / `deepseek`（经 DashScope 代管） |
| `bytedance` | `Doubao-ASR`（豆包大模型 ASR 双向流式） | — |
| `tencent` | `Tencent-ASR`（腾讯云实时语音识别） | — |
| `baidu` | `Baidu-ASR`（百度智能云实时语音识别） | — |
| `deepseek` | — | `deepseek`（直连 api.deepseek.com） |
| `bigmodel` | — | `glm`（智谱开放平台） |

### 默认 ASR seed

完整权威 seed 在 [`crates/infra/resources/sql/schema.sql`](../crates/infra/resources/sql/schema.sql)（编译期 `include_str!` 嵌入，首次建库写入）——**以它为准，下表仅作概览**。当前 ASR seed 共 21 行：13 local + 8 云端（aliyun 3 / bytedance 2 / tencent 2 / baidu 1）。

| provider | category | 代表 model_name | 说明 |
|---|---|---|---|
| local | zipformer | `zipformer` / `zipformer-large` | 流式 CTC（`zipformer-small` 为 builtin **兜底引擎** source_type=0，seed + 首次启动下载） |
| local | paraformer | `paraformer-{streaming,zh,bilingual,multi-zh}` | 流式 ×4 |
| local | sensevoice-orig | `sensevoice-orig-small`（`WisemeAI/sensevoice-small-quant`） | 原版 FunASR 4 输入，**is_available=1**（随包就绪），`skip_corrector` |
| local | firered | `firered-asr2`（`VidraAI/FireRedASR2-onnx`） | FireRedASR2-AED CTC，**is_available=1**（随包就绪） |
| local | qwen3-asr | `qwen3-asr-{0.6B,1.7B}` | 非流式，`skip_corrector` |
| local | moonshine | `moonshine-{base,tiny}-en` | 非流式 en-only |
| local | whisper | `whisper-small`（`onnx-community/whisper-small.en`） | 非流式 en-only |
| aliyun | `Fun-ASR` / `Paraformer-Realtime` / `Qwen-ASR` | 见 db.sql | 云端 WS 流式（DashScope key） |
| bytedance | `Doubao-ASR` / `Doubao-ASR-2.0` | `doubao-asr-{1.0,2.0}-streaming` | 云端 bigmodel_async 流式 |
| tencent | `Tencent-ASR` / `Tencent-ASR-Multi` | `16k_zh` / `16k_zh_en` | 云端 HMAC-SHA1 流式 |
| baidu | `Baidu-ASR` | `15372` | 云端 START 帧鉴权流式 |

> sherpa nano 简化版 SenseVoice（旧 category=`sensevoice`）已移除，仅留原版 `sensevoice-orig`，见 [removed-sensevoice-sherpa-nano.md](removed-sensevoice-sherpa-nano.md)。

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

> **`provider` 字段**：vendor / 运行位置维度，与 `category`（引擎族/模型系列）正交。`local` 表示随应用打包或下载到本地，`aliyun` 表示经阿里云 DashScope 云端调用，`bytedance` 表示经火山引擎豆包云端调用。决定引擎路由（`provider='aliyun'` → `EngineCategory::Aliyun` → `AliyunEngine`；`provider='bytedance'` → `EngineCategory::ByteDance` → `CloudPipelineEngine`，`Stage::Streaming` cloud 分支）。

> **`is_local` 字段**：标记是否本地运行。`provider='local'` ⟺ `is_local=1`（二者并存：`is_local` 供本地过滤，`provider` 用于 vendor 路由）。

> **`is_thinking` 字段**：标记该模型是否为思考（reasoning）模型。思考模型在润色等明确任务中若不关闭思考，`content` 可能为空（token 被 `reasoning_content` 耗尽）。置为 `1` 时程序自动发送关闭思考的参数——DeepSeek 用 `thinking: {type: "disabled"}`，BigModel 用 `enable_thinking: false`。

> **`is_enabled` 字段**：标记是否启用。`1` 表示启用，`0` 表示禁用。只有启用的模型才会被系统加载或供识别/润色使用。阿里云 qwen / Fun-ASR seed 默认 `is_enabled=0`，用户填 API Key 后改为 `1` 启用。

> **`is_streaming` 字段**：标记 ASR 模型是否支持流式识别。`1` 表示流式（zipformer×2 + paraformer×4，走本地流式 partial），`0` 表示非流式（sensevoice-orig / firered / qwen3-asr / moonshine / whisper / aliyun Fun-ASR，走 VAD 分段伪流式）。`is_streaming_engine()` = `resolve_active_engine("asr").entry.is_streaming`（Task 2 后无参，读激活引擎），数据驱动、不再按 category 硬编码。

> **远程 API Key 配置方式（`secret_key`）**：LLM / 云端 ASR 的所有参数（包括 Base URL / WS 端点 和 API Key）全部存储在 DB `models` 表。`source` 存端点 URL，`secret_key` 存 API Key。可通过 SQLite 客户端手动填入（具体填法见下方「阿里云云端 API」小节）。

> **引擎激活由 DB `is_enabled` 决定（2026-07-17 重构后）**：`models` 表 `is_enabled=1` 表激活（每域仅 1 个），`is_available=1` 表可用（文件就绪）。`zipformer-small` 是兜底引擎（source_type=0 builtin）——ASR 域无激活模型时自动回退到它，首次启动若文件缺失弹下载窗自动下载。详见下方「模型激活」节。

### 阿里云云端 API 接入

通过 `provider='aliyun'` 接入两个阿里云 DashScope（百炼）云端能力。LLM 走 OpenAI 兼容端点（**零代码**，`llm/client.rs` 不改），ASR 走 FunASR Realtime WebSocket。

#### 1. 填 DashScope API Key（`secret_key`）

DashScope API Key 暂无 UI 入口，需手动 sqlite3 填入对应模型行的 `secret_key` 字段：

```bash
# ASR（FunASR Realtime WS）
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-你的dashscope-key', is_available=1 WHERE domain='asr' AND model_name='fun-asr-2025-11-07';"

# LLM（DashScope OpenAI 兼容）
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-你的dashscope-key', is_available=1 WHERE domain='llm' AND model_name='qwen-plus';"
```

填后需**重启进程**生效（`OnceLock` 缓存，运行中不热更新）。

#### 2. LLM 润色（零代码）

seed 已含 `aliyun:qwen:qwen-plus` / `qwen-turbo`（DashScope OpenAI 兼容端点 `https://dashscope.aliyuncs.com/compatible-mode/v1`）。填 Key 后在设置页「模型管理 → 文本模型」激活 `aliyun:qwen:qwen-plus`（`switch_active_model("llm", id)`，存 DB `is_enabled=1`），即走 DashScope OpenAI 兼容路径（与 deepseek/bigmodel 同一 `polish()` 代码）。

#### 3. ASR 识别（FunASR Realtime WS）

seed 已含 `aliyun:Fun-ASR:fun-asr-2025-11-07`（WS 端点 `wss://dashscope.aliyuncs.com/api-ws/v1/inference`，`is_streaming=0` 走桌面分块路径）。填 Key 后在设置页「模型管理 → 语音识别」激活该模型（`switch_active_model("asr", id)`）。

**云引擎路由**：启动时 `resolve_active_engine("asr")` 按 `provider='aliyun'` 解析为 `EngineCategory::Aliyun` → 建 `AliyunEngine`；`provider='bytedance'` 解析为 `EngineCategory::ByteDance` → 直接走 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支，无独立 engine）；否则按 `engine_mode`（embedded/websocket/grpc）走本地引擎。云 ↔ 本地切换经设置页 `switch_active_model` 后**下次录音生效**（`reload_active_engine` 刷新 ACTIVE_ENGINES 缓存）。

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
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='你的火山-api-key', is_available=1 WHERE domain='asr' AND model_name='doubao-asr-1.0-streaming';"
```

#### 2. 配置引擎

在设置页「模型管理 → 语音识别」激活 `bytedance:Doubao-ASR:doubao-asr-1.0-streaming`（`switch_active_model("asr", id)`，存 DB `is_enabled=1`）。

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
sqlite3 ~/.octopus/octopus.db "UPDATE models SET source='你的appid:你的secretid', secret_key='你的secretkey', is_available=1 WHERE domain='asr' AND model_name='16k_zh';"
```

#### 2. 配置引擎

在设置页「模型管理 → 语音识别」激活 `tencent:Tencent-ASR:16k_zh`（`switch_active_model("asr", id)`，存 DB `is_enabled=1`）。

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
sqlite3 ~/.octopus/octopus.db "UPDATE models SET source='你的AppID', secret_key='你的APIKey', is_available=1 WHERE domain='asr' AND model_name='15372';"
```

填完 `is_available=1`（可用）后，再到设置页激活（`is_enabled=1`）。

#### 2. 配置引擎

在设置页「模型管理 → 语音识别」激活 `baidu:Baidu-ASR:15372`（`switch_active_model("asr", id)`，存 DB `is_enabled=1`）。

`model_name` 是 dev_pid，常用取值：`15372`（中文加强标点）、`15376`（中文多方言）、`17372`（英文加强标点）。

#### 3. feature 依赖

与其他云端 provider 共用 `cloud` feature，无需额外 feature flag。

#### 5. schema 变更：删库重建（dev 阶段）

`models` 表 schema 变更（加 `provider` 列、`name`→`model_name`、唯一键改 4 字段）后，开发期直接删库重新初始化（不写 ALTER 迁移）：

```bash
rm -f ~/.octopus/octopus.db
# 下次启动 ensure_db 重建新 schema + seed
```

**VAD 不进表**：内嵌 `silero_vad_v6.onnx`（2026-08-04 从 v4 升级，随应用打包，`include_bytes!`）；磁盘 `~/.octopus/models/vad.onnx` 存在时覆盖（通用名，可放任意 VAD 模型，见 `VAD_OVERRIDE_PATH`）。

查看当前 DB 中的引擎：

```bash
octopus-cli config
```

### 模型目录解析（resolve_model_dir）

`source` 字段双模式：

| source 形态 | 解析结果 | 示例 |
|---|---|---|
| 本地相对路径 | `~/.octopus/models/<source>` | `asr/zipformer-small` → `~/.octopus/models/asr/zipformer-small` |
| HF repo 名 | `~/.cache/huggingface/hub/` | `onnx-community/whisper-small.en` → HF 缓存 |

### 手编 DB

`models` 表可手动编辑（增删模型条目），但**需重启进程生效**——`asr::load_config()` 首次读出后缓存到 `OnceLock`，运行中不热更新。引擎激活由 DB `is_enabled` 决定（每域仅 1 个=1，经设置页 `switch_active_model` 切换）。**唯一键** `UNIQUE(domain, provider, category, model_name)` 允许跨 provider 同名模型共存（如 deepseek-v4-flash 在 deepseek 直连与 aliyun 代管下各一行）。

> **开发阶段 schema 变更**：直接修改 [`crates/infra/resources/sql/schema.sql`](../crates/infra/resources/sql/schema.sql)，然后删除 `~/.octopus/octopus.db` 并重启即可重新初始化。无迁移逻辑，开发期以此替代。

### model.json / history.txt 已废弃

`model.json`（旧模型配置）和 `history.txt`（旧识别历史）已在 DB 单一源重构中彻底删除，DB 是唯一来源。详见 [db-single-source 设计](superpowers/specs/2026-06-14-archived-design.md)。

## 应用配置（app_config 表）

应用行为配置，v3+ 统一存储在 `~/.octopus/octopus.db` 的 `app_config` 表（key-value TEXT）。首次启动由 db.sql seed 默认值；旧 `config.yaml` 自动迁移到 DB 后重命名为 `.bak`。

**两种编辑方式**：
1. **GUI 设置窗口**（推荐）：桌面应用工具栏点击「设置」按钮或托盘菜单「设置...」打开独立设置窗口——系统设置页提供表单化编辑（toggle/select/number input），修改即时写回 DB `app_config` 表 + RuntimeConfig。29 个可配置字段均有类型校验和生效时间提示（立即 / 下次录音 / 重启）。
2. **手动编辑**：直接用 sqlite3 编辑 `~/.octopus/octopus.db` 的 `app_config` 表，需重启进程生效（`OnceLock` 缓存）。

> **⚠️ 迁移提示**：旧 `config.yaml` 首次启动 v3 版本时自动导入 DB 并重命名为 `config.yaml.bak`。旧字段 `polish_enabled` / `shortcut` / `polish_interval` 在迁移时自动转换为 `polish_mode` / `asr_shortcut` / `polish_min_interval`。

| 字段 | 类型 | 默认值 | 适用端 | 说明 |
|---|---|---|---|---|
| `microphone` | string | `""` | cli + desktop | 麦克风设备名（空 = 系统默认） |

> **⚠️ 模型激活字段已移除（2026-07-17 模型激活重构）**：`asr_engine` / `polish_llm` / `ocr_model` / `translate_engine` 4 个字段已从 `app_config` 删除。激活态统一存 DB `models.is_enabled`（每域仅 1 个=1），经设置页 `switch_active_model(domain, id)` 命令切换。详见下方「模型激活」节。
| `language` | string | `"auto"` | desktop | auto / zh / en / ja / ko |
| `engine_mode` | string | `"embedded"` | desktop | embedded / websocket / grpc |
| `remote_url` | string | `ws://127.0.0.1:3000/ws/stream` | desktop | websocket 模式远程地址 |
| `grpc_endpoint` | string | `http://127.0.0.1:50051` | desktop | grpc 模式端点 |
| `asr_shortcut` | string | `OptRight` | desktop | 单键三模式触发键（handy-keys 名：OptRight/CmdRight/CtrlRight/ShiftRight/Fn）。长按=PTT / 双击=toggle / 短按=hands-free。GUI 设置页 dropdown 5 选 1 + 热重载（unregister_ptt/register_ptt）。值不合法 fallback OptRight。**2026-08-01 从 Tauri 加速键（Alt+A）升级为单键名** |
| `paste_method` | string | `"clipboard"` | desktop | clipboard / direct / none |
| `write_to_clipboard` | bool | `true` | desktop | 粘贴完成后是否把识别结果写入剪贴板（方便他处再粘贴）；`false` 时三模式等同重构前现状（不碰/恢复原剪贴板）。详见 [transcript-model spec §6](superpowers/specs/2026-06-14-archived-design.md) |
| `overlay_position` | string | `"top"` | desktop | top / bottom / none。**已废弃**（2026-06-21 审查修复）：`recording_overlay` 窗口及 `overlay.rs` 模块已整体删除，UI 统一到 `result_window`。字段保留于 config 结构（避免 DB schema 迁移），但无任何使用方 |
| `segment_silence` | f64 | `400.0` | desktop | VAD 伪流式：句间停顿阈值（毫秒），起过此值的停顿触发切句识别 |
| `polish_mode` | int | `0` | desktop | LLM 润色模式：0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色。**desktop 悬停工具栏可在运行时切换**（`set_polish_mode` 命令）：写 RuntimeConfig + 持久化回 DB，**立即生效**（Coordinator 每个 tick 重读镜像并 `Transcript::set_mode`，下一次润色按新模式） |
| `polish_min_interval` | f64 | `5.0` | desktop | 中间润色最小间隔（秒，节流用），仅 `polish_mode=2` 生效；`<=0` 回退 `1.0s`。旧名 `polish_interval` 迁移时自动重命名 |
| `pause_polish_threshold_ms` | f64 | `600` | desktop | 停顿触发中间润色的静音阈值（毫秒），仅 `polish_mode=2` 生效；**须 >= 600**（须大于句间停顿最大值 600ms），否则润色先于尾音冲刷、快照缺尾音。GUI 设置页改为下拉（600~1000ms 五档），label 名为「润色停顿阈值」 |
| `asr_hardware_accelerated` | bool | `false` | desktop + cli | ASR 推理是否启用硬件加速（CUDA/DirectML/CoreML EP），失败自动回退 CPU；不影响 VAD（VAD 固定 CPU） |
| `asr_correct` | bool | `true` | cli + server + desktop | 是否对 ASR 输出做拼音映射 + bigram 转移概率的轻量纠错/热词校正；**自动跳过 Qwen3-ASR**（其自带标点且语义纠错强），仅作用于 Whisper/SenseVoice/Paraformer/Zipformer。2026-08-01 默认改 `true`（加了热词即生效，无热词 corrector no-op 零过纠）。详见 [architecture.md §ASR 纠错](../architecture.md) |
| `denoise_mode` | u8 | `1` | desktop | 环境降噪模式：`0`=关闭（直通）、`1`=RNNoise（`nnnoiseless`，默认，纯 Rust 内置默认模型，48kHz→频带增益+OLA，GRU 状态跨帧保持）、`2`=DeepFilterNet3（libDF v0.5.6 + tract 0.19，48kHz 全频带，编译期内嵌 ~7.9MB 模型，质量最佳）。降噪为可插拔后端（`FrameDenoise` trait），由 mode 选后端；亦可由工具栏运行时切换（`set_denoise_mode` 命令）并持久化回 DB `app_config` 表。初始化/推理失败自动降级直通（warn），不阻断录音。详见 [architecture.md](../architecture.md) |
| `output_simplified` | bool | `true` | desktop | ASR 输出字形归一化：`true`→简体（繁→简），`false`→繁体（简→繁）。基于开放词典网 CC-BY 3.0 单字对照表（编译期嵌入），在 ASR 输出后做单字级字形转换（不转地域用词）。解决 Qwen3-ASR `auto` 模式输出繁体的问题。详见 [architecture.md](../architecture.md) |
| `hide_toolbar` | bool | `true` | desktop | 结果展示区工具栏显隐模式：`true`→鼠标移入显示、移出隐藏（默认）；`false`→工具栏始终显示（窗口高度保持展开态 132px） |
| `edit_shortcut` | string | `"CmdOrCtrl+Enter"` | desktop | 结果展示区编辑 toggle 快捷键——**进入与保存（退出）编辑都用此键**（与 ✏️ 按钮同语义，Tauri Accelerator 格式，窗口内、仅结果窗聚焦时生效）。**跨平台**：`CmdOrCtrl` 在 macOS=⌘、Win/Linux=Ctrl（前端 `parseShortcut` 按 `e.metaKey||e.ctrlKey` 判定）；旧默认 `Cmd+Enter` 仅匹配 macOS、Win/Linux 下 Ctrl+Enter 失效——**DB v15→v16 迁移**自动把 `Cmd+Enter` 升级为 `CmdOrCtrl+Enter`（仅动等于旧默认的行，保留用户自定义值）。GUI 设置页可配（快捷键捕获按钮，不需冲突检测——仅窗口内 keydown 判定）。曾用双击进入（WKWebView `dblclick` 难触发而弃用）；曾拆分「Cmd+E 进 / Cmd+Enter 存」，因两者均窗口内 keydown（非全局、不 hijack 系统）已统一为单键 toggle |
| `edit_global_shortcut` | string | `"CmdOrCtrl+Shift+E"` | desktop | 全局编辑快捷键——任意应用聚焦时唤起结果窗并进入/保存编辑（toggle，复用窗口内编辑语义）。与 `edit_shortcut`（窗口内、仅结果窗聚焦时生效）并存。GUI 设置页可配 + 热重载 |
| `clipboard_shortcut` | string | `"CmdOrCtrl+Shift+D"` | desktop | 剪贴板历史浮窗全局快捷键（Tauri Accelerator 格式）。GUI 设置页可配 + 热重载 |
| `paste_stack_shortcut` | string | `"CmdOrCtrl+Shift+V"` | desktop | 粘贴队列出栈全局快捷键——任意应用聚焦时按此键弹出栈底条目并粘贴到前台应用（`pop_and_paste`）。与 `clipboard_shortcut`（打开浮窗）正交，可同时注册。Tauri Accelerator 格式，GUI 设置页可配 + 热重载。详见 [paste-stack spec](superpowers/specs/archived/2026-08-05-paste-stack-design.md) |
| `clipboard_max_items` | int | `1000` | desktop | 剪贴板最大保留条数（不含收藏，超出自动清理） |
| `clipboard_max_age_days` | int | `30` | desktop | 剪贴板自动清理天数（超过此天数的非收藏记录自动删除） |
| `screenshot_shortcut` | string | `"Alt+S"` | desktop | 截图全局快捷键（框选 → 标注 → 入剪贴板历史）。详见 [screenshot 设计](superpowers/specs/2026-06-28-archived-specs.md)。GUI 设置页可配 + 热重载 |
| `screenshot_watermark_text` | string | `""` | desktop | 截图水印文字，空=不加水印。工具栏水印按钮 + 设置页均可配。导出时 `drawWatermark` 叠加（不进 annotations 数组，独立全局层）。热重载 |
| `screenshot_watermark_position` | string | `"bottom-right"` | desktop | 截图水印 9 格位置：`top-left`/`top-center`/`top-right`/`middle-left`/`middle-center`/`middle-right`/`bottom-left`/`bottom-center`/`bottom-right`。热重载 |
| `screenshot_watermark_opacity` | f32 | `0.3` | desktop | 截图水印透明度 0.0-1.0。热重载 |
| `screenshot_watermark_font_size` | u32 | `24` | desktop | 截图水印字号（逻辑像素）。热重载 |
| `download_mirror` | string | `""` | cli + desktop | HF 模型下载镜像 host（如 `https://hf-mirror.com`），空 = 官方源 huggingface.co。cli `download --mirror` 可临时覆盖 |
| `active_polish_prompt` | string | `"1"` | desktop | 激活的润色 prompt id（`prompts` 表 `id` 字段，字符串形式存储）。默认 `"1"` 指向系统内置 prompt。设置窗口 prompt 管理页可切换（`set_active_prompt` 命令即时生效，下次润色用新 prompt）。详见 [prompts 表](#prompts-表-润色提示词管理) |

> **前缀划分**：`segment_*` 控制 VAD 分段，`polish_*` 控制润色行为（`polish_mode`、`polish_min_interval`、`pause_polish_threshold_ms`），`asr_*`（`asr_hardware_accelerated`、`asr_correct`）控制推理后端 / 输出后处理。**模型激活**（asr_engine / polish_llm / ocr_model / translate_engine）已从 AppConfig 移除，改存 DB `models.is_enabled`——详见下方「模型激活」节。`denoise_mode`（前缀 `denoise_`）控制麦克风环境降噪（采集层前置，VAD/ASR 前）。`write_to_clipboard` 属粘贴行为（与 `paste_method` 同组）。`microphone` 为 cli + desktop 跨端通用字段，其余为 desktop 行为参数。`active_polish_prompt` 属润色行为（与 `polish_*` 同组，但存独立 key，由 `db::load_active_prompt_id()` 读，不入 `AppConfig` struct）。

> **快捷键字段**（`asr_shortcut`（单键名，dropdown）/ `edit_shortcut` / `edit_global_shortcut` / `clipboard_shortcut` / `paste_stack_shortcut` / `screenshot_shortcut`）GUI 设置页可配 + 热重载。`asr_shortcut` 是 handy-keys 单键名（非 Tauri Accelerator），其余为 Tauri Accelerator 格式。`clipboard_*`（`clipboard_shortcut` / `paste_stack_shortcut` / `clipboard_max_items` / `clipboard_max_age_days` / `clipboard_enabled`）控制剪贴板历史（浮窗快捷键 + 队列粘贴快捷键 + 容量/清理 + 是否监听）。`clipboard_enabled`（默认 `true`）是否启用剪贴板历史监听——已纳入 `AppConfig`，设置页「交互」开关 + 浮窗 title bar 快捷按钮可配，热重载生效（运行时翻转 watcher flag，无需重启）；列表项**双击默认粘贴**（固定行为，不可配）。`paste_stack_shortcut`（默认 Cmd+Shift+V）控制粘贴队列出栈粘贴。`screenshot_shortcut` 控制截图触发。`download_mirror` 控制模型下载镜像源。

### 模型激活（2026-07-17 重构后）

模型激活态**不再存 `app_config`**（原 `asr_engine` / `polish_llm` / `ocr_model` / `translate_engine` 4 字段已删除）。激活态统一存 DB `models.is_enabled`（每域仅 1 个=1），4 域（asr/llm/ocr/translate）共用同一套机制：

| 操作 | 入口 | 实现 |
|------|------|------|
| **查询激活** | 代码内 `resolve_active_engine(domain)` | DB `WHERE domain=? AND is_enabled=1 AND is_available=1 LIMIT 1`，结果缓存在 `ACTIVE_ENGINES` 内存 |
| **切换激活** | 设置页模型管理 → 点击「激活」 | Tauri 命令 `switch_active_model(domain, id)` → DB `UPDATE SET is_enabled=IIF(id=?,1,0) WHERE domain=? AND is_available=1`（单语句原子刷新）→ `reload_active_engine` 重载缓存 |
| **ASR 兜底** | ASR 域无激活时 | 自动回退 `zipformer-small`（builtin source_type=0，首次启动下载）；其余域无激活返回 Err |

- **`is_available` vs `is_enabled`**：`is_available=1` = 文件就绪/配置完整（可被选择，同域可多个）；`is_enabled=1` = 当前激活（每域仅 1 个）。下载模型后置 `is_available=1`，激活模型时置 `is_enabled=1`。
- **CLI `--model` 显式路径**：cli 多模型场景用 `resolve_engine_any(spec)` 查 DB 任意可用 ASR（不限激活），支持 3-part spec `"{provider}:{category}:{model_name}"` 或裸名。
- **旧 DB 升级**：v36→v37 迁移自动完成（`UPDATE is_available=is_enabled` 迁语义 + `UPDATE is_enabled=0` 重置激活 + 删 app_config 4 字段），用户无需手工 SQL。

### 完整示例

```yaml
# 麦克风（留空用系统默认）
microphone: ""

# 注意：asr_engine / polish_llm / ocr_model / translate_engine 已移除（2026-07-17）
# 模型激活改在设置页「模型管理」GUI 操作，存 DB models.is_enabled，不再经 config.yaml
language: "auto"

# 引擎接入模式
engine_mode: "embedded"          # embedded | websocket | grpc

# 桌面交互
asr_shortcut: "OptRight"  # 单键三模式触发键（handy-keys 名：OptRight/CmdRight/CtrlRight/ShiftRight/Fn）
paste_method: "clipboard"        # clipboard | direct | none
write_to_clipboard: true         # 粘贴后是否把识别结果留在剪贴板（false = 等同重构前现状）

# VAD 伪流式分段（离线引擎）
segment_silence: 400.0           # 句间停顿阈值（毫秒）

# LLM 润色（polish_mode 控制是否润色；润色用哪个模型由设置页激活，存 DB is_enabled）
polish_mode: 0                   # 0=关闭 / 1=仅最终润色 / 2=中间润色+最终润色
polish_min_interval: 5.0         # 秒，仅 polish_mode=2 生效（中间润色最小间隔；旧名 polish_interval 迁移时自动重命名）
pause_polish_threshold_ms: 600   # 毫秒，仅 polish_mode=2 生效（停顿触发润色的静音阈值，须 >= 600）
asr_hardware_accelerated: false  # true 启用 GPU/CoreML/DirectML 加速（失败回退 CPU）；VAD 不受影响
asr_correct: true                # true 对 ASR 输出做拼音+bigram 轻量纠错（自动跳过 Qwen3-ASR）
denoise_mode: 1                  # 环境降噪：0=关闭直通 / 1=RNNoise（默认）/ 2=DeepFilterNet3（48kHz 全频带，~7.9MB 模型）；亦可工具栏运行时切换（set_denoise_mode）
output_simplified: true          # ASR 输出字形：true=简体（繁→简），false=繁体（简→繁）
hide_toolbar: true               # 结果窗工具栏：true=hover 显隐（默认），false=始终显示
edit_shortcut: "CmdOrCtrl+Enter"  # 编辑 toggle 快捷键（窗口内，进入/保存都用此键；CmdOrCtrl 跨平台=⌘/Ctrl）
edit_global_shortcut: "CmdOrCtrl+Shift+E"  # 全局编辑（跨应用唤起结果窗 + toggle）

# 剪贴板历史浮窗
clipboard_shortcut: "CmdOrCtrl+Shift+D"  # 剪贴板历史浮窗快捷键
clipboard_max_items: 1000         # 最大保留条数（不含收藏）
clipboard_max_age_days: 30        # 自动清理天数（不含收藏）

# 截图
screenshot_shortcut: "Alt+S"      # 截图快捷键（框选 → 标注 → 入剪贴板历史）

# 下载（OCR 模型激活同 ASR/LLM，由设置页 switch_active_model 切换，存 DB is_enabled）
download_mirror: ""               # HF 下载镜像（空 = 官方源），cli download --mirror 可临时覆盖
```

### 结果展示区编辑

录音过程中可随时修正识别/润色文本：
- **进入编辑**：按 `edit_shortcut`（默认 `Cmd+Enter`，窗口内），或点工具栏 ✏️ 编辑按钮。
- **编辑期间 ASR 硬暂停**（音频丢弃），改完恢复。
- **退出编辑**：再按 `edit_shortcut`（与进入同键，toggle 语义），或点工具栏 ✏️→💾 按钮保存。（曾用「完成编辑」按钮 + 固定 `Cmd+Enter`，前者已删、后者已统一为 `edit_shortcut`。）
- 编辑后的文本作为后续展示与润色基准；新识别文本追加其上；停止粘贴时保留编辑。
- 编辑后再触发润色时，仅润色新增部分、保留已编辑（润色结果折回）。
- 未编辑时行为与旧版完全一致。

## prompts 表（润色提示词管理）

润色提示词由 DB `prompts` 表管理（替代旧单文件 `~/.octopus/VOICE_POLISH.md`，已删除）。用户可维护多条润色 prompt，激活其一。

### schema

| 列 | 含义 |
|---|---|
| `id` | INTEGER PK AUTOINCREMENT——系统主键，用户不可编辑，`app_config.active_polish_prompt` 引用此字段 |
| `title` | TEXT——用户可读别名，**允许重复**（用户自行区分即可） |
| `category` | TEXT——用途分类，当前固定 `voice_text_polish`（语音文本润色） |
| `content` | TEXT——**文件名引用**（v50，不含 `.md`），运行时 `read_prompt_file(content)` 读 `~/.octopus/.sync/prompts/polish/<content>.md`；存「风格规则」部分（不含 edited 标记规则） |
| `description` | TEXT——用户可读描述 |
| `is_system` | INTEGER——`1`=系统内置（**2026-07-19 起可编辑不可删**），`0`=用户自建 |
| `created_at` / `updated_at` | TEXT——时间戳 |

### seed

3 条系统内置（2026-08-01 重构，旧 6 模板→3 模板，详见 [spec](superpowers/specs/2026-08-01-polish-prompt-templates-design.md)）：

```sql
INSERT OR IGNORE INTO prompts (id, title, category, content, description, is_system) VALUES
    (1, '润色-忠实校对',   'voice_text_polish', 'faithful',     '只纠错不改意…（系统内置）', 1),
    (2, '润色-意图整理',   'voice_text_polish', 'user-intent',  '清洗噪声+结构化…（系统内置）', 1),
    (3, '润色-口语化',     'voice_text_polish', 'app-casual',   '保留口语味，聊天标点…（系统内置）', 1);
```

每个 seed md 文件内嵌 few-shot 示例（faithful 3 例 / user-intent 2 例 / app-casual 3 例），演示 `[]` edited 标记输入 → 去括号纯文本输出。旧模板（default-polish / advanced-polish / sayit-*）移到 `crates/infra/seeds/prompts/history/` 保留对比，不再 seed。

### Prompt 组装

`content` 只存「风格规则」部分。润色时由 `llm::prompt::build_system_prompt(content)` 强制拼接 `EDITED_MARKER_RULE`（2026-08-01 重构：替代旧 `INCREMENTAL_RULE`——`[]` edited 标记规则，代码常量，用户不可见/不可改）：

```
system_prompt = content + "\n" + EDITED_MARKER_RULE
# EDITED_MARKER_RULE = "文本中 [方括号] 标记的词语是用户手动修正过的，请信任这些用词，并在润色全文时以其为语境参考。输出时去掉方括号标记，仅输出纯文本。"
```

edited 段（用户手动修正）在 `regions_prompt` / `user_prompt` 中用 `[方括号]` 内联标记拼回全文，全文连贯发给 LLM（不再用 region 标记法把文本切碎）。

### 运行时切换

设置窗口 prompt 管理页提供 7 个 Tauri 命令：`list_prompts` / `get_active_prompt` / `set_active_prompt` / `create_prompt` / `update_prompt` / `delete_prompt` / `restore_prompt_from_seed`。切换 active prompt 即时生效（`set_system_prompt` 写 `RwLock<String>`），下次润色用新 prompt；进行中的润色不受影响。

### 降级

- `active_polish_prompt` 指向不存在的 id → fallback 到 `id=1` + warn 日志 + 自动修正 app_config
- DB 读 prompt 失败 → fallback 到空 content（仅 edited 标记规则）+ warn 日志

## 模型下载

VAD 模型（silero_vad_v6.onnx 1.23MB，2026-08-04 从 v4 升级）内嵌进二进制（`include_bytes!`），无需下载。默认 ASR 兜底引擎（zipformer 27M）计划改为首次启动自动下载。其他大模型用 `huggingface-cli` 按需下载到 HF 缓存：

```bash
# 安装 HF CLI
pip install huggingface_hub

# 下载（source 字段即 HF repo 名）
huggingface-cli download onnx-community/whisper-small.en
huggingface-cli download WisemeAI/sensevoice-small-quant     # sensevoice-orig（原版 FunASR）
huggingface-cli download VidraAI/FireRedASR2-onnx            # firered（FireRedASR2 CTC）
huggingface-cli download csukuangfj/sherpa-onnx-streaming-paraformer-zh
# Zipformer Transducer（RNN-T，三 session：encoder + decoder + joiner）
huggingface-cli download csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30
huggingface-cli download csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30
```

下载后存入 `~/.cache/huggingface/hub/`，DB `models` 表中 `source` 为 HF repo 名的引擎会经 `resolve_model_dir` 自动定位到对应缓存。
