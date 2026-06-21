# 腾讯云 ASR 实时语音识别接入设计

> 文档：https://cloud.tencent.com/document/product/1093/48982
> 对标实现：ByteDanceStreamSession / AliyunStreamSession（Stage::CloudStreaming 路径）

## 功能概述

接入腾讯云实时语音识别 WebSocket API，作为第三个云端 ASR provider（与 Aliyun、ByteDance 并列）。采用签名鉴权（HMAC-SHA1），WebSocket 文本帧响应 JSON 结果。

## 协议要点

### Endpoint

固定 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`

- `<appid>` 替换为用户 AppID（URL 路径段）
- `{params}` 为查询参数串（含签名）

### 鉴权（签名生成）

三步：

1. **拼接签名原文**：除 `signature` 外的所有参数按**字典序**排序，拼接为
   `asr.cloud.tencent.com/asr/v2/<appid>?key1=value1&key2=value2&...`
2. **HMAC-SHA1 + Base64**：`signature_raw = Base64(HMAC-SHA1(sign_str, SecretKey))`
3. **URL 编码**：`signature = urlencode(signature_raw)`（必须编码 `+`、`=`、`/` 等特殊字符）

最终 URL = `wss://...?{sorted_params}&signature={encoded_signature}`

### 必填握手参数

| 参数 | 说明 |
|---|---|
| `secretid` | 腾讯云 SecretID |
| `timestamp` | 当前 UNIX 时间戳（秒） |
| `expired` | 签名过期时间戳（秒），须 > timestamp |
| `nonce` | 随机正整数（≤10 位） |
| `engine_model_type` | 引擎模型（如 `16k_zh`、`16k_zh_en`） |
| `voice_id` | 音频流 UUID（每次连接重新生成） |
| `signature` | 签名 |

可选参数：`voice_format=1`（PCM）、`needvad=1`、`filter_punc=1`、`vad_silence_time=1000`

### 音频发送

- **WebSocket Binary 帧**：原始 PCM s16le 字节，**无额外头**
- **速率**：200ms 音频 = 6400 字节（16k），1:1 实时率
- 发送过快或间隔 >6s 会被服务端断开

### 响应格式（Text 帧 JSON）

顶层字段：
- `code`：0=正常，非 0=错误（错误码表见官方文档）
- `message`：错误描述
- `final`：1 = 全部识别结束（连接将断开）
- `result`：识别结果对象

`result` 字段：
- `slice_type`：0=开始，1=识别中（非稳态），2=识别结束（稳态）
- `index`：句序号（从 0 递增）
- `voice_text_str`：文本

### 结束信号

客户端发 Text 帧 `{"type":"end"}` → 服务端返回 `final=1` → 断开连接。

## DB 映射

需要 3 个鉴权信息：AppID、SecretID、SecretKey。

| DB 字段 | 腾讯含义 | 示例 |
|---|---|---|
| `source` | `{appid}:{secretid}` 复合字段 | `1259221234:AKIDxxxxxxxxxxxxx` |
| `secret_key` | SecretKey（HMAC 签名密钥） | `yyyyyyyyyyyyyy` |
| `model_name` | DB 内标识（= engine_model_type） | `16k_zh`、`16k_zh_en` |

> Endpoint 固定，不存 DB。`source` 用冒号分隔 AppID 和 SecretID（与 model spec 的 3-part 冒号不冲突——DB `source` 列是自由文本）。

## 与 Aliyun / ByteDance 的差异

| 维度 | Aliyun | ByteDance | **Tencent** |
|---|---|---|---|
| 鉴权 | Bearer token | X-Api-Key header | **URL 签名（HMAC-SHA1）** |
| 音频帧 | Raw PCM / base64 | gzip(PCM) | **Raw PCM（binary frame）** |
| 响应 | JSON text | Binary + gzip(JSON) | **JSON text** |
| 结束信号 | finish-task JSON | 末帧 flags=0x2 | **`{"type":"end"}` text** |
| Endpoint 来源 | DB source | 固定 | **固定 + appid 路径段** |
| 额外依赖 | — | flate2 | **hmac + sha1** |

## 架构设计

### EngineCategory::Tencent

- `crates/asr/src/config.rs`：新增 `Tencent` 变体
- `resolve_category`：`provider == "tencent"` → `Some(Tencent)`
- `is_streaming_engine`：排除 Tencent（与 Aliyun/ByteDance 一致）
- `coordinator::is_cloud_engine`：`matches!(cat, Some(Aliyun) | Some(ByteDance) | Some(Tencent))`

### TencentStreamSession

- 文件：`crates/desktop/src/tencent_stream.rs`
- 接口与 `AliyunStreamSession` / `ByteDanceStreamSession` 完全一致：
  `open` / `push_pcm` / `finish` / `try_recv_text` / `close_async`
- 复用 `aliyun_stream::{PcmFrame, StreamEvent}`

### CloudSession enum

`crates/desktop/src/cloud_session.rs` 新增 `Tencent(TencentStreamSession)` 变体，方法分派。

### 文本累积策略

腾讯返回分句增量结果（`slice_type=0/1/2`），需自行累积：
- `slice_type=2`（稳态）→ 存入 `BTreeMap<index, text>`
- `slice_type=0/1`（非稳态）→ 临时 partial
- 发给 coordinator 的 `StreamEvent::Text` = `stable_segments.join("") + current_partial`
- `final=1` → `StreamEvent::Text(stable)` then `StreamEvent::Finished`

## 降级与安全

- 无 API Key 时 DB `secret_key` 为空 → `resolve_tencent_config` 返回明确错误
- 签名过期（timestamp 偏差大）→ 服务端返回 code=4002，session 报 Failed
- 速率超限 → 服务端返回 code=4000 并断开
