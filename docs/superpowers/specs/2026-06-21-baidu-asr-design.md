# 百度智能云实时语音识别接入设计

> 文档：https://ai.baidu.com/ai-doc/SPEECH/jlbxejt2i
> 对标实现：AliyunStreamSession / ByteDanceStreamSession / TencentStreamSession

## 功能概述

接入百度智能云实时语音识别 WebSocket API，作为第四个云端 ASR provider。鉴权信息在 START 帧 JSON 中直接传入（appid + appkey），协议简洁。

## 协议要点

### Endpoint

固定 `wss://vop.baidu.com/realtime_asr?sn=<UUID>`

- `sn`：用户自定义的请求标识（UUID 即可），用于排查日志

### 帧类型

| 帧 | Opcode | 内容 |
|---|---|---|
| START（开始） | Text | `{"type":"START","data":{...}}` |
| 音频数据 | **Binary** | 原始 PCM s16le（无头、无压缩） |
| FINISH（结束） | Text | `{"type":"FINISH"}` |
| CANCEL（取消） | Text | `{"type":"CANCEL"}` |

### START 帧 data 参数

| 参数 | 必填 | 说明 |
|---|---|---|
| `appid` | 是 | AppID（int） |
| `appkey` | 是 | API Key |
| `dev_pid` | 是 | 语种模型（int，推荐 15372 中文加强标点） |
| `cuid` | 是 | 设备唯一标识（统计 UV，不影响识别） |
| `format` | 是 | 固定 `"pcm"` |
| `sample` | 是 | 固定 `16000` |
| `user` | 可选 | 多方言模型（dev_pid=15376）必填 |

### 音频发送

- **Binary 帧**：原始 PCM s16le，**无头、无压缩**
- **每帧**：160ms = 5120 字节（范围 20-200ms）
- **间隔**：建议实时（160ms），最长不超过 5s（否则超时断开）

### 响应格式（Text JSON）

| 字段 | 说明 |
|---|---|
| `err_no` | 0=正常，非 0=错误 |
| `err_msg` | 错误描述 |
| `type` | `MID_TEXT`（临时结果）/ `FIN_TEXT`（最终结果）/ `HEARTBEAT`（心跳） |
| `result` | 识别文本 |
| `start_time` / `end_time` | 句时间戳（仅 FIN_TEXT） |

### 结束信号

客户端发 `{"type":"FINISH"}` → 服务端完成识别后自行关闭连接。

### dev_pid 取值

| PID | 模型 | 标点 |
|---|---|---|
| 1537 | 中文普通话 | 弱标点 |
| **15372** | 中文普通话 | **加强标点（推荐）** |
| 15376 | 中文多方言 | 弱标点（需 user 参数） |
| 1737 | 英语 | 无标点 |
| 17372 | 英语 | 加强标点 |

## DB 映射

| DB 字段 | 百度含义 | 示例 |
|---|---|---|
| `source` | **AppID** | `105xxx17` |
| `secret_key` | **API Key**（appkey） | `UA4oPSxxxxkGOuFbb6` |
| `model_name` | **dev_pid**（字符串形式） | `15372` |

> Endpoint 固定，不存 DB。百度实时识别不使用 access_token / SecretKey，鉴权全在 START 帧。

## 与其他三个 provider 的差异

| 维度 | Aliyun | ByteDance | Tencent | **Baidu** |
|---|---|---|---|---|
| 鉴权 | Bearer header | X-Api-Key header | URL HMAC-SHA1 | **START 帧 appid+appkey** |
| 初始化 | run-task JSON | FULL_CLIENT_REQUEST binary | URL 参数 | **START 帧 JSON** |
| 音频帧 | Raw PCM / base64 | gzip(PCM) | Raw PCM | **Raw PCM binary** |
| 响应 | JSON text | Binary+gzip(JSON) | JSON text | **JSON text** |
| 临时结果 | result-generated | result.text | slice_type=0/1 | **MID_TEXT** |
| 最终结果 | task-finished | flags=0x3 | final=1 | **FIN_TEXT** |
| 结束信号 | finish-task JSON | 末帧 flags=0x2 | `{"type":"end"}` | **`{"type":"FINISH"}`** |

## 架构设计

### EngineCategory::Baidu

- `crates/asr/src/config.rs`：新增 `Baidu` 变体
- `resolve_category`：`provider == "baidu"` → `Some(Baidu)`
- `is_streaming_engine`：排除 Baidu（与其他三个云 provider 一致）
- `coordinator::is_cloud_engine`：追加 `Some(Baidu)`

### BaiduStreamSession

- 文件：`crates/desktop/src/baidu_stream.rs`
- 接口与其他三个完全一致：`open` / `push_pcm` / `finish` / `try_recv_text` / `close_async`
- 复用 `aliyun_stream::{PcmFrame, StreamEvent}`
- 无额外 cargo 依赖（无 HMAC/gzip/base64 需求）

### CloudSession enum

`crates/desktop/src/cloud_session.rs` 新增 `Baidu(BaiduStreamSession)` 变体。

### 文本累积策略

- `Vec<String>` 存所有 FIN_TEXT 的 result（按顺序拼接）
- `current_partial` 存当前 MID_TEXT 的 result
- `StreamEvent::Text` = `fin_texts.join("") + current_partial`
- FINISH 发送后服务端关闭连接 → `StreamEvent::Finished`

### dev_pid 处理

DB `model_name` 列存 dev_pid 字符串（如 `"15372"`），`open()` 时解析为 `i64` 填入 START 帧 `data.dev_pid`。
