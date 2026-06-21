# 火山引擎豆包大模型流式 ASR 接入设计

> 对标文档：[双向流式模式（优化版本）](https://www.bytedance.com/docs/6561/1354869)
> 对标实现：`DashScopeStreamSession`（aliyun / dashscope feature）

## 1. 目标

接入火山引擎豆包大模型 ASR **双向流式模式（优化版本）** 作为第二个云端 ASR provider，与
现有阿里云 DashScope（`EngineCategory::Aliyun`）并列。用户申请 API Key 后填入 DB 即可使用。

### 非目标

- 不接入单向流式 / nostream 模式（双向流式优化版即可覆盖）
- 不接入小模型 V1 协议（仅接入大模型 `bigmodel_async`）
- 不做 TTS（仅 ASR）

## 2. 协议规格

### 2.1 Endpoint

```
wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async
```

固定 host，无 region/cluster 参数。资源路由通过 WS 握手 header `X-Api-Resource-Id` 指定。

### 2.2 认证（WS 握手 Headers）

新控制台 auth：
```
X-Api-Key: <api_key>
X-Api-Resource-Id: volc.bigasr.sauc.duration | volc.seedasr.sauc.duration
X-Api-Request-Id: <UUID>
X-Api-Sequence: -1
```

| 模型 | 计费 | Resource ID |
|---|---|---|
| Doubao ASR 1.0 | 时长 | `volc.bigasr.sauc.duration` |
| Doubao ASR 1.0 | 并发 | `volc.bigasr.sauc.concurrent` |
| Doubao ASR 2.0 | 时长 | `volc.seedasr.sauc.duration` |
| Doubao ASR 2.0 | 并发 | `volc.seedasr.sauc.concurrent` |

**约定**：Resource ID 存入 DB `source` 字段（如 `volc.bigasr.sauc.duration`），endpoint 固定。
API Key 存入 `secret_key`，与 aliyun 一致。

### 2.3 二进制帧协议（4B Header + Payload）

所有整数字段大端序。

**Header（4 字节）**：
```
Byte 0: [Protocol Version 4b=0001] [Header Size 4b=0001]  → 0x11
Byte 1: [Message Type 4b]       [Msg Type Flags 4b]
Byte 2: [Serialization 4b]      [Compression 4b]
Byte 3: [Reserved 8b=0x00]
```

**Message Types**：
| 值 | 常量 | 方向 | 含义 |
|---|---|---|---|
| 0x1 | FULL_CLIENT_REQUEST | C→S | 带 JSON config 的初始帧 |
| 0x2 | AUDIO_ONLY_REQUEST | C→S | 纯音频帧 |
| 0x9 | FULL_SERVER_RESPONSE | S→C | 带 JSON 结果的响应 |
| 0xF | ERROR_RESPONSE | S→C | 错误 |

**Message Type Flags**：
| 值 | 常量 | 含义 |
|---|---|---|
| 0x0 | NO_SEQUENCE | 无 sequence number |
| 0x1 | POS_SEQUENCE | 有 sequence number |
| 0x2 | NEG_SEQUENCE | 最后一帧（负包），无 seq |
| 0x3 | NEG_WITH_SEQUENCE | 最后一帧（负包），有 seq |

**Serialization**：0x0=NONE（纯音频）, 0x1=JSON
**Compression**：0x0=NONE, 0x1=GZIP

### 2.4 客户端发帧

**FULL_CLIENT_REQUEST（初始 config）**：
```
[Header: 0x11 0x11 0x11 0x00]    // ver=1, hdr=1, type=FULL_CLIENT_REQUEST, flags=NO_SEQ, ser=JSON, comp=GZIP
[Payload Size 4B BE]
[gzip(JSON)]
```

JSON config（minimal）：
```json
{
  "user": { "uid": "<随机>" },
  "audio": { "format": "pcm", "codec": "raw", "rate": 16000, "bits": 16, "channel": 1, "language": "zh-CN" },
  "request": { "model_name": "bigmodel", "enable_itn": true, "enable_punc": true, "enable_ddc": false, "show_utterances": true }
}
```

**AUDIO_ONLY_REQUEST（正常音频帧）**：
```
[Header: 0x11 0x20 0x01 0x00]    // type=AUDIO_ONLY, flags=NO_SEQ, comp=GZIP
[Payload Size 4B BE]
[gzip(raw_audio)]
```

**AUDIO_ONLY_REQUEST（最后一帧，EOF）**：
```
[Header: 0x11 0x22 0x01 0x00]    // type=AUDIO_ONLY, flags=NEG_SEQUENCE（负包=末帧）
[Payload Size 4B BE]
[gzip(raw_audio)]
```

### 2.5 服务端响应

**FULL_SERVER_RESPONSE**：
```
[Header: 0x11 0x91 0x11 0x00]    // type=FULL_SERVER_RESPONSE, flags=POS_SEQ, ser=JSON, comp=GZIP
[Sequence 4B BE]
[Payload Size 4B BE]
[gzip(JSON)]
```

末帧 flags=0x3（NEG_WITH_SEQUENCE）。

响应 JSON：
```json
{
  "result": {
    "text": "累积全文",
    "utterances": [{ "definite": true, "text": "此句已确定", "start_time": 0, "end_time": 1705 }]
  }
}
```

- `result.text`：累积全文
- `utterances[].definite=true`：此句已 finalize
- 末帧 flags=0x3：全部结束

**ERROR_RESPONSE**：
```
[Header: 0x11 0xF1 0x00 0x00]    // type=ERROR, flags=POS_SEQ
[Error Code 4B BE]
[Error Msg Size 4B BE]
[Error Msg UTF-8]
```

### 2.6 优化版（bigmodel_async）特性

- **事件驱动响应**：不是每包音频都回，仅在结果变化时回——降低 RTF 和尾延迟
- **两遍识别**（配合 `enable_punc` + `show_utterances`）：流式 partial + `definite=true` 最终句
- 与标准双向流式相同的二进制协议，仅响应行为不同

## 3. 架构设计

### 3.1 新增 EngineCategory::ByteDance

与 `Aliyun` 平级的云端 provider：

```rust
pub enum EngineCategory {
    // ... 现有 6 个本地 ...
    Aliyun,       // DashScope Fun-ASR
    ByteDance,   // 豆包大模型 bigmodel_async
}
```

- `provider='bytedance'` → 路由到 `ByteDance`
- 与 `Aliyun` 一样：`is_streaming=true`，但在桌面端走独立 `CloudStreaming` 路径
- `is_cloud_engine` 扩展：`ByteDance || Aliyun` 均判定为云端

### 3.2 infra 层：AsrSection 新增 bytedance 字段

```rust
pub struct AsrSection {
    // ... 现有 ...
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。
    pub aliyun: Option<HashMap<String, ModelEntry>>,
    /// 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）。
    #[serde(default)]
    pub bytedance: Option<HashMap<String, ModelEntry>>,
}
```

`all_sections` 维度从 7→8。

### 3.3 DB seed

```sql
('asr','bytedance','Doubao-ASR','doubao-asr-1.0-streaming','volc.bigasr.sauc.duration','zh','火山引擎豆包大模型 ASR 1.0 双向流式（bigmodel_async，DashScope-style key 填 secret_key）',0,0,1),
('asr','bytedance','Doubao-ASR-2.0','doubao-asr-2.0-streaming','volc.seedasr.sauc.duration','zh','火山引擎豆包大模型 ASR 2.0 双向流式（bigmodel_async，时长计费）',0,0,0);
```

- `provider='bytedance'`，`category='Doubao-ASR'`
- `source` = Resource ID（如 `volc.bigasr.sauc.duration`）
- `secret_key` 空（用户填 API Key）
- `is_streaming=1`（Doubao-ASR 1.0 默认开启）

### 3.4 ByteDanceStreamSession

镜像 `DashScopeStreamSession` 接口（`push_pcm` / `try_recv_text` / `finish` / `close_async`），
但内部实现完全不同——使用火山的二进制帧协议而非 DashScope 的 JSON 文本协议。

**关键模块**：`crates/desktop/src/bytedance_stream.rs`（feature gated `dashscope` 或新建 `bytedance` feature）

**设计决策**：复用 `dashscope` feature gate（而非新建 `bytedance` feature）。
原因：两个 provider 都是云端 WS 流式，feature 控制的是"是否编译云端流式路径"，两者语义一致。
避免新增 feature 增加 build matrix 复杂度。

**结构**：
```rust
pub struct ByteDanceStreamSession {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpsc::UnboundedReceiver<StreamEvent>,
}
```

接口与 `DashScopeStreamSession` 完全一致，coordinator 可用同一 `StreamEvent` enum。

### 3.5 coordinator 路由

`is_cloud_engine` 扩展：
```rust
fn is_cloud_engine(config: &AppConfig) -> bool {
    let cat = resolve_engine_category(&config.asr_engine);
    cat == Some(EngineCategory::Aliyun) || cat == Some(EngineCategory::ByteDance)
}
```

`resolve_cloud_config` 根据 category 分派到 DashScope 或 VolcEngine：
```rust
fn resolve_cloud_config(engine_spec: &str) -> Result<CloudProvider, String> {
    let cat = resolve_engine_category(engine_spec);
    match cat {
        Some(EngineCategory::Aliyun) => Ok(CloudProvider::Aliyun { ... }),
        Some(EngineCategory::ByteDance) => Ok(CloudProvider::ByteDance { ... }),
        _ => Err(...),
    }
}
```

`Stage::CloudStreaming.session` 字段改为 enum：
```rust
session: Option<CloudSession>,  // enum: Aliyun(DashScopeStreamSession) | ByteDance(ByteDanceStreamSession)
```

或更简单：trait object（但 `DashScopeStreamSession` 方法非 async-safe，trait 化需仔细）。
**采用 enum 方案**——显式分派，类型安全，避免动态分派。

### 3.6 配置（config.yaml / AppConfig）

用户通过设置 UI 选引擎（与 aliyun 一致），`AppConfig.asr_engine` 存
`bytedance:Doubao-ASR:doubao-asr-1.0-streaming` 格式（3-part spec）。
无需新增 AppConfig 字段——复用 `asr_engine` + `language`。

## 4. 不变量

1. **endpoint 固定**：`wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`，不通过 DB 配置
2. **Resource ID = DB source**：用户选模型时 source 字段即 Resource ID，直接用作 WS header
3. **secret_key = API Key**：与 aliyun 一致的 key 存储约定
4. **PCM 16kHz mono s16le**：与现有 coordinator 录音管线一致，无需重采样
5. **StreamEvent 共享**：两个 provider 都返回统一的 `StreamEvent`，coordinator 上层逻辑零改动

## 5. 降级路径

- **API Key 缺失**：`resolve_cloud_config` 返回 Err，coordinator 报错给用户（与 aliyun 一致）
- **WS 连接失败**：`ByteDanceStreamSession::open` 返回 Err，coordinator 回退 `is_speaking=false`
- **无 `dashscope` feature**：云端引擎不可用（`is_cloud_engine` 返回 false 时走本地 VadSegmented）

## 6. 与 Aliyun（DashScope）的关键差异

| 维度 | Aliyun | ByteDance |
|---|---|---|
| 协议 | JSON 文本帧 | 二进制帧（4B header + payload） |
| Endpoint | DB source 字段（wss://） | 固定 `openspeech.bytedance.com/api/v3/sauc/bigmodel_async` |
| Auth | `Authorization: bearer <key>` | `X-Api-Key: <key>` + Resource ID header |
| 音频编码 | 裸 PCM s16le bytes | gzip(PCM s16le) |
| 结果解析 | JSON 文本（run-task 协议） | gzip(JSON) 从二进制帧 payload |
| EOF 信号 | `finish-task` JSON | 末帧 flags=0x2（NEG_SEQUENCE） |
