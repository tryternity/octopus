# VadSegmentedPipeline 双 VAD 伪流式设计规格

**日期**：2026-07-08
**范围**：`crates/desktop/src/pipeline.rs` 中 `VadSegmentedPipeline`——离线 ASR 引擎（Whisper / SenseVoice / SenseVoice-orig / Qwen3-ASR / FireRed / Moonshine）的伪流式封装：双 VAD + 切段 + spawn + 乱序回填。
**关联**：修复见 plans/`2026-07-07-vad-segmented-lstm-drift-fix.md`；数据流总览见 `docs/features/asr-engine.md`、`docs/features/coordinator.md`；架构定位见 `docs/architecture.md`。

---

## 1. 定位与适用边界

| Pipeline | 引擎 | 分段方式 | 本 spec？ |
|----------|------|----------|-----------|
| **VadSegmentedPipeline** | Whisper / SenseVoice 系列 / Qwen3-ASR / FireRed / Moonshine（离线，整段输入） | 双 VAD 段触发 | ✅ |
| StreamingPipeline / StreamingSession | Paraformer / Zipformer（真流式） | 引擎自带流式，无 detect_vad 分段 | ❌ |
| CloudPipelineEngine | 阿里云 / 字节 / 腾讯 / 百度 | detect_vad 仅起始门控，无分段 | ❌（detect_vad 复用 §3 helper） |

VadSegmentedPipeline 把「整段输入」的离线引擎包装成「边录边出字」的伪流式：录音 PCM 按 tick 喂入，VAD 检测段边界，每段切下后 spawn 一次离线识别，乱序回填到 Transcript。

---

## 2. 双 VAD 架构

两个独立的 Silero LSTM VAD 实例（模型 `~/.octopus/models/silero_vad_v4.onnx`，有状态 `h`/`c`）：

| 实例 | 字段 | 生命周期 | 用途 |
|------|------|----------|------|
| **detect_vad** | `detect_vad: SileroVad` | 流式有状态，跨 tick 续接；**切段后 reset+preroll**（§5 不变量①）；段内不 reset | 段触发：`compute_speech_chunks` 数语音帧，驱动 `has_speech` |
| **filter_vad** | `filter_vad: SileroVad` | **每段 reset**（`filter_speech_from_buffer` L322） | 段内语音提取：从切段缓冲抠出语音样本送识别 |

**为何双 VAD**：检测流需跨 tick 累积上下文（流式判定 has_speech），其 LSTM 状态会随段累积；过滤流必须每段从干净状态判定（等价旧代码每 buffer 新建 VAD）。分离两者，detect_vad 的累积不污染 filter_vad 的逐段过滤。

构造（`new` L363-384）：加载模型 → `detect_vad` preroll（L371）→ `filter_vad` 不 preroll（首段 `filter_speech_from_buffer` 先 reset 再算）。

---

## 3. 共享 VAD helper（coordinator vad-seg tick 与 cloud tick 共用）

| 符号 | 值 / 行为 |
|------|-----------|
| `VAD_SPEECH_THRESHOLD` | `0.5`（语音帧判定阈值，与 `streaming_runner` 一致） |
| `VAD_CHUNK_SIZE` | `512` 采样点（16k 下 32ms/帧） |
| `VAD_PREROLL_FRAMES` | `10`（预滚静音帧数，LSTM 预热） |
| `compute_speech_chunks(vad, samples)` | 512 样本/帧切片，`vad.compute` 得 prob≥0.5 计一帧；计算失败保守计一帧（L301） |
| `vad_preroll(vad)` | 喂 10 帧静音预热 LSTM，避免首几帧误判静音丢字 |
| `filter_speech_from_buffer(filter_vad, samples)` | **先 `reset()`** → `audio::filter_speech(samples, filter_vad, 480, 0.5)` 抠语音 |

---

## 4. run_tick 状态机（`pipeline.rs:428`）

每个录音 tick（约 100ms 音频）调用：

```
samples 非空：
  1. audio_buffer.extend(samples)
  2. speech_chunks = compute_speech_chunks(detect_vad, samples)
     ├─ >= 2 帧 → has_speech=true, silence_duration=0
     └─ < 2 帧 → silence_duration += 本 tick 时长
  3. silence_cut = has_speech && silence_ms >= segment_silence_ms   （来自 config.segment_silence）
     force_cut  = buffer_duration_s >= SEGMENT_DURATION_S(20s)       （不依赖 has_speech，§5 不变量②）
  4. if silence_cut || force_cut:
       send_buffer = overlap_tail ++ audio_buffer
       force_cut → overlap_tail = 末尾 SEGMENT_OVERLAP(200ms) 样本（衔接下段）
       silence_cut → overlap_tail.clear()
       audio_buffer.clear(); has_speech=false; silence_duration=0
       detect_vad.reset() + vad_preroll()        ← §5 不变量①（本次修复）
       speech_samples = filter_speech_from_buffer(filter_vad, send_buffer)
       非空 → segment_cut_this_tick=true; spawn_offline(speech_samples, seq)
（恒）drain_rx_and_consume(transcript) → 回填连续 seq，返回是否文本变化
```

**spawn + 乱序回填**：`spawn_offline`（L390）异步 `engine.transcribe` → 发 `SegmentResult{seq,text}` 到 mpsc tx。`drain_rx_and_consume`（L415）`try_recv` 至空 → `apply_segment_result` 按 seq 缓存到 `completed_results: HashMap<seq,String>` → `consume_completed_results_vad` 消费连续 seq 追加 transcript。乱序不丢、不阻塞。

**事件流**（`tick` L497，包 `run_tick`）：文本变化 → `[PersistRaw{vad_segmented}, Emit]`；段切（有语音）→ 追加 `[Polish{INFINITY}]`（段边界 silence 必过，触发停顿润色）；`speaking` 变化 → `[Speaking]`（`has_speech && silence_duration<0.3`）。

---

## 5. 关键不变量（本次修复确立，违反即回归「几段后不吐字」）

**① detect_vad 切段后必须 reset+preroll**
切段 = 一段语音结束、新段开始，是 LSTM 状态的安全重置点。reset+preroll 让 detect_vad 从干净状态检测下一段（与构造 L371 对称），**消除跨段累积漂移**。
> 根因（2026-07-07 bug）：detect_vad 会话内从不 reset → 几段后 LSTM 漂移 → 对真实语音持续 prob<0.5 → `compute_speech_chunks < 2` → `has_speech` 卡 false → `silence_cut`（`&&has_speech`）永不触发 → `audio_buffer` 无限堆积不吐字。重启录音 = drop pipeline + `new`（h/c=0+preroll）→ 恢复。

**② force_cut 解绑 has_speech**
`force_cut = buffer_duration_s >= SEGMENT_DURATION_S`，不带 `&& has_speech`。达 20s 上限必切，由 `filter_vad`（每段 reset、不漂移）独立兜底判定有无语音：检出则 spawn（双 VAD 保险），未检出则不 spawn 但 buffer 已清（防内存堆积）。`silence_cut` 保留 `&& has_speech`（原意：检测到停顿才切）。

**③ filter_vad 每段 reset**
`filter_speech_from_buffer` L322 每段先 `reset()`，LSTM 不跨段累积，是②的可信兜底来源。

**④ 会话级复用 `reset()` 当前未启用**
`reset()`（L535，`#[allow(dead_code)]`）清全部状态 + 双 VAD reset，供未来会话间复用 pipeline。当前 coordinator stage 切换时直接 drop pipeline 重建（不调 reset）。

---

## 6. 常量与配置

| 常量 / 配置 | 值 | 来源 |
|-------------|-----|------|
| `SEGMENT_DURATION_S` | `20.0`（force_cut 上限，秒） | `infra::consts` |
| `SEGMENT_OVERLAP_MS` | `200.0`（force_cut 衔接 overlap，毫秒） | `infra::consts` |
| `segment_silence_ms` | 来自 `config.segment_silence` | AppConfig |
| `VAD_SPEECH_THRESHOLD` / `VAD_CHUNK_SIZE` / `VAD_PREROLL_FRAMES` | `0.5` / `512` / `10` | 本文件 §3 硬编码 |

---

## 7. 边界

- **纯静音会话**：detect_vad 持续判无语音 → has_speech 保持 false → 20s 触发 force_cut → filter_vad reset 判无语音不 spawn → buffer 清空（无害，只防堆积）。
- **乱码风险**（理论）：detect_vad 失灵 + filter_vad 误判有语音 → spawn 噪声段。不变量①修复后 detect_vad 不漂移，②仅兜底；乱码优于「完全不吐字 + 内存爆涨」。
- **coordinator drop 重建已确认**：stop / discard / cancel / finalize 四路径全 drop pipeline、无 reset 复用，故不变量①在 pipeline.rs 内即可闭环，不动 coordinator。
