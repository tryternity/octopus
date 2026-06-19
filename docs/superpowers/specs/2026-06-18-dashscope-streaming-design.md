# DashScope 云端流式 ASR（VAD-gated per-utterance streaming）

> Date: 2026-06-18（2026-06-19 更新：Qwen-ASR Realtime 协议 + 非阻塞 finish + partial 分离）
> 状态：✅ 已实现（2026-06-19）— 3 套云端协议 + 流式 bug 修复完成；48 desktop + 29 infra tests pass

## 1. 背景与目标

DashScope 云端实时 ASR 支持 **3 套接口**（共用 DashScope API Key），通过 endpoint 路径自动分发：

| 接口 | endpoint | 协议 | DB model_name |
|---|---|---|---|
| Fun-ASR | `/api-ws/v1/inference` | 任务型（run-task/finish-task） | `fun-asr-realtime` |
| Paraformer | `/api-ws/v1/inference` | 任务型（与 Fun-ASR 共用） | `paraformer-realtime-v2` |
| Qwen-ASR | `/api-ws/v1/realtime` | OpenAI Realtime 风格 | `qwen3-asr-flash-realtime` |

**目标**：VAD-gated per-utterance streaming——VAD 检测到语音 onset → 开一条长连接 WSS，持续推 PCM 收 partial；静音 ≥ `pause_polish_threshold_ms` → 发 finish 信号（**非阻塞**），后续 tick drain 最终结果。

## 2. 架构设计

### 2.1 Stage：`CloudStreaming`

```
audio.drain_samples → VAD 检测 → 语音？
  ├─ 否（静音中）→ 更新 pre_roll_buffer + silence_duration + speech_confirm_count=0
  │    └─ 有活跃 WSS + 静音 ≥ threshold → session.finish()（非阻塞）→ is_closing=true
  └─ 是（语音中）→ speech_confirm_count++ + silence_duration=0
       ├─ 无活跃 WSS + 连续 2 tick 确认 → open WSS + 推 pre-roll 100ms + push PCM
       └─ 有活跃 WSS（持续）→ push PCM + drain events:
            ├─ StreamEvent::Text(partial) → current_partial = partial（不碰 transcript）
            └─ StreamEvent::Finished → transcript.append_segment(current_partial) → drop session
```

**partial 与 transcript 分离**（关键设计决策）：
- `current_partial`：当前 session 的实时 partial 预览（UI 显示 transcript + partial）
- `transcript`：已提交的历史文本，只在 `Finished` 事件时 append
- 这解决了 partial 覆盖历史文本 + close 结果重复 append 的 bug

### 2.2 `DashScopeStreamSession`（`dashscope_stream.rs`）

有状态 WS 会话句柄，coordinator 通过同步接口操作：

| 方法 | 语义 | 协议 |
|---|---|---|
| `open()` | 建连 + 初始化（run-task / session.update）+ pre-roll | 自动分发 |
| `push_pcm(&[f32])` | 非阻塞推 PCM（二进制 / base64） | 自动分发 |
| `finish()` | **非阻塞**发 finish 信号（finish-task / session.finish） | 自动分发 |
| `try_recv_text()` | 非阻塞取 partial（`Option<StreamEvent>`） | — |
| `close()` | **阻塞**发 finish + 等最终结果（仅 Toggle 停止用，**8s 保底超时**防 WS 挂起冻死 Toggle 停止路径） | 自动分发 |

**非阻塞 finish**（关键修复）：tick handler 用 `finish()` 而非 `close()`——`close()` 的 `block_on` 会冻结 coordinator 线程（曾导致 UI 冻结 20 秒），`finish()` 只发信号，结果通过后续 tick 的 `try_recv_text()` 异步获取。

### 2.3 三套协议自动分发

`is_qwen_realtime_endpoint(endpoint)` 按 URL 路径分流：
- 含 `/v1/realtime` → Qwen-ASR Realtime 协议
- 否则 → Fun-ASR/Paraformer 任务型协议

**Fun-ASR / Paraformer**（`run_ws_session`）：
- 二进制 PCM 帧（s16le）
- run-task / finish-task / result-generated / task-finished
- **句边界检测用 `sentence_id` + `sentence_end`**（非靠 text 变空）：
  - `sentence_id` 变化 = 新句，提交前一句到 `committed`
  - `sentence_end=true` = 最终结果，立即提交
  - `heartbeat=true` 跳过心跳包

**Qwen-ASR Realtime**（`run_qwen_realtime_session`）：
- base64 PCM via `input_audio_buffer.append`（文本帧）
- session.update（server_vad 模式，silence_duration_ms=600）
- partial = `conversation.item.input_audio_transcription.text`（text + stash 拼接）
- final = `conversation.item.input_audio_transcription.completed`（transcript 字段）
- 结束 = `session.finish` → `session.finished`

### 2.4 onset 抗噪

连续 2 个 tick（~200ms）检测到语音才开 WSS（`speech_confirm_count >= 2`），消除单次噪声脉冲导致的空 session 误触发。

## 3. Toggle 停止时的收尾

Toggle（停止录音）从 `CloudStreaming` → `Pasting`：
1. 停 tick 线程
2. `audio.stop()` 排空剩余音频
3. 如果有活跃 WSS：`close()`（**阻塞**，Toggle 路径可用）→ 拿最终文本 → `set_full`
4. 提交 `current_partial`（如有未提交的 partial）
5. 进入 `Pasting`（直接粘贴）
