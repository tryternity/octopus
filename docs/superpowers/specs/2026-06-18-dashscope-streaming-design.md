# DashScope 云端流式 ASR（VAD-gated per-utterance streaming）

> Date: 2026-06-18
> 状态：✅ 已实现（2026-06-18）— Tasks 1-6 完成；cargo check + 33 tests pass（embedded+dashscope）

## 1. 背景与目标

当前 DashScope FunASR 走 **chunk 模式**（`engine_dashscope.rs`）：每段 VAD 切分后开一条新 WS，跑完整 duplex 协议，收最终结果。问题：
- 每次切段有 ~200-300ms 建连 RTT 延迟
- 无 partial 结果（用户看不到实时字幕）
- 每段开新连接开销大

**目标**：新增 **VAD-gated per-utterance streaming** 模式——VAD 检测到语音 onset → 开一条长连接 WSS，持续推 PCM 收 partial；静音 ≥ `pause_polish_threshold_ms`（700ms）→ 断开 WSS。下一段语音开新 WSS + 100ms pre-roll。

## 2. 架构设计

### 2.1 新 Stage：`CloudStreaming`

```
audio.drain_samples → VAD 检测 → 语音？
  ├─ 否（静音中）→ 更新 pre_roll_buffer + silence_duration
  │    └─ silence ≥ 700ms 且有活跃 WSS → close WSS + 拼接最终文本 + 触发润色
  └─ 是（语音中）→ 更新 pre_roll_buffer + silence_duration=0
       ├─ 无活跃 WSS（onset）→ open WSS + 推 pre-roll 100ms + push 当前 PCM
       └─ 有活跃 WSS（持续）→ push 当前 PCM + try_recv partial 更新 UI
```

### 2.2 `DashScopeStreamSession`（`dashscope_stream.rs`）

有状态 WS 会话句柄，持有两条 tokio task：

| Task | 职责 |
|---|---|
| sender task | 从 PCM channel 收样本 → `ws.send(binary)` |
| reader task | `ws.next()` → result channel 发 partial/final 文本 |

coordinator 通过同步接口操作：
- `open()` → 建连 + run-task + pre-roll
- `push_pcm(&[f32])` → 非阻塞推 PCM
- `try_recv_text()` → 非阻塞取 partial（`Option<StreamEvent>`）
- `close()` → 发 finish-task + 阻塞等最终结果

### 2.3 Pre-roll 滚动缓冲区

- **容量**：`CLOUD_PREROLL_BUFFER_SAMPLES` = 3200（200ms @ 16kHz）
- **语义**：每 tick drain 的音频追加到尾部，超容量弹出头部（滚动窗口）
- **onset 时**：取最后 1600 samples（100ms）作为 pre-roll 推入 WSS
- **200ms 而非 100ms**：VAD 有 ~64ms 检测延迟（≥2 chunks），实际语音起点比确认点早

### 2.4 `max_sentence_silence` 参数

`run-task` 设 `max_sentence_silence: 600`（比客户端 700ms 断开阈值短 100ms）：
- 服务端 600ms 时触发 `sentence_end=true` → 出完整句
- 客户端 700ms 时 `finish-task` → 收 `task-finished`
- 保证客户端断开前服务端已出完整结果

## 3. 模式路由

```
Toggle → Idle：
  is_cloud_engine(config) → Stage::CloudStreaming
  else if is_streaming_engine → Stage::Streaming（本地流式）
  else → Stage::VadSegmented（本地分块）
```

`is_cloud_engine` 判定：`resolve_engine_category(config.asr_engine) == EngineCategory::Aliyun`。

CloudStreaming 优先于本地 streaming——云端引擎 `is_streaming=1` 但不能走 `StreamingSession::new`（只支持 Paraformer/Zipformer）。

## 4. 三个静音阈值的角色

| 阈值 | 默认值 | 路径 | 作用 |
|---|---|---|---|
| `segment_silence` | 300ms | VadSegmented only | 段内静音切分（逗号级） |
| `PUNCTUATION_SILENCE_THRESHOLD` | 500ms | Streaming only | Active Flush（尾音冲刷） |
| `pause_polish_threshold_ms` | 700ms | CloudStreaming only | WSS 断开 + 停顿润色 |

CloudStreaming 不读 `segment_silence`——DashScope 服务端自带 VAD 做断句标点。

## 5. 与现有代码的关系

| 组件 | 变化 |
|---|---|
| `dashscope_stream.rs`（新建） | `DashScopeStreamSession` + 后台 task |
| `coordinator.rs` | 新增 `Stage::CloudStreaming` + `Command::CloudStreamingTick` + `handle_cloud_streaming_tick` |
| `engine_dashscope.rs` | 复用 `samples_to_pcm_s16le`；chunk 模式保留作 fallback |
| `main.rs` | 注册 `dashscope_stream` 模块 |

## 6. Toggle 停止时的收尾

Toggle（停止录音）从 `CloudStreaming` → `WaitingCompletion`：
1. 停 tick 线程
2. `audio.stop()` 排空剩余音频
3. 如果有活跃 WSS：推剩余 PCM → close WSS → 拿最终文本 → 拼接到 transcript
4. 进入 `Pasting`（直接粘贴，无 active_count 等待）
