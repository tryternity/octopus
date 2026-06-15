# 设计文档：流式 ASR 尾音主动冲刷（Active Flush）

> 解决流式识别的「尾音憋字」——最后一个字（如「等一下」的「下」）被憋在引擎缓冲区里，直到用户再次说话才吐出，且因静音判定被误插逗号（「，下」）。

## 0. 背景

octopus-desktop 的流式模式（Paraformer / Zipformer）以 ~600ms tick 驱动 `accept_samples` 增量识别。实测发现：

- 说「等一下」时，先识别出「等一」，尾字「下」**不立即出现**；
- 停顿数秒后再次说话，「下」才随新语音被「挤」出来；
- 由于此时协调器已判定发生过 >0.5s 静音，在「下」前**误插逗号** → 「，下」。

该滞后**超过 VAD 静音阈值**，属于引擎层的固有滞后，非网络/调度延迟。

## 1. 问题分析

### 1.1 流式引擎为何憋字

| 机制 | 说明 |
|------|------|
| Conformer 右上下文（lookahead） | 流式 Conformer 需要向后看若干帧才能对齐当前帧的输出；尾音处于 chunk 边界时，缺少后续帧 → 输出被挂起 |
| CIF 门限累加器（Paraformer） | CIF 以 alpha 权重累加，静音期 alpha≈0，累加值达不到触发阈值 1.0 → 尾音 token 无法发射 |
| 状态化推理 | 引擎保存 encoder/decoder/CIF 缓存；尾音「卡」在这些缓存里，只有下一轮新音频带来正向权重时才被「挤出」 |

### 1.2 伪流式（VadSegmented）为何无此问题

离线引擎（SenseVoice / Whisper / Qwen3-ASR）为**无状态**整段推理：协调器在检测到 ≥0.5s 静音后切断音频，把整段送引擎一次性转录。双向注意力同时看到首尾，尾音随当前分段立即返回，不存在「卡在缓存」的物理条件。

> 因此 **Active Flush 只作用于流式 `Streaming` 阶段，`VadSegmented` 不涉及**。

## 2. 目标与约束

### 2.1 功能范围

| 功能 | 说明 |
|------|------|
| 静音期主动冲刷 | 累积静音 ≥0.5s 时，向引擎补零强制对齐右上下文 / 触发 CIF，把憋住的尾音即时吐出 |
| 非破坏性 | flush 不重置引擎状态（缓存连续），后续真实音频照常识别 |
| 每静音段一次 | `flushed` 标志保证一个静音段只冲刷一次，避免重复触发 |
| 尾音不带逗号 | flush 走独立路径，**不经过** `accept_samples` 的静音插逗号逻辑 |
| 恢复说话后可再次冲刷 | 重新说话（静音清零）时重置 `flushed`，下个静音段可再次 flush |

### 2.2 不做

| 不做 | 原因 |
|------|------|
| VadSegmented 阶段的 flush | 离线整段识别天然无憋字（见 §1.2） |
| flush 结果的标点处理 | 尾音属于当前句尾，不应加逗号；标点由 `accept_samples` 在恢复说话时统一处理 |
| 调整 VAD 静音阈值 | 阈值复用既有 `PUNCTUATION_SILENCE_THRESHOLD`（0.5s）|

## 3. 机制设计

三层协作：引擎层补零冲刷 → 累积层无逗号合并 → 协调器状态机驱动。

### 3.1 引擎层：非破坏性 active flush

#### Paraformer（`crates/asr/src/streaming_paraformer.rs`）

```rust
/// Active flush: pad the current sample buffer with zeros to CHUNK_SAMPLES
/// to force processing of the lookahead / right context of the tail speech frames.
pub fn flush(&mut self) -> Result<Option<String>> {
    let needed = CHUNK_SAMPLES.saturating_sub(self.sample_buffer.len());
    if needed > 0 {
        self.sample_buffer.resize(CHUNK_SAMPLES, 0.0);
    }
    let mut accumulated_text = String::new();
    while self.sample_buffer.len() >= CHUNK_SAMPLES {
        let chunk_samples: Vec<f32> = self.sample_buffer.drain(..CHUNK_SAMPLES).collect();
        if let Some(text) = self.process_chunk(&chunk_samples)? {
            accumulated_text.push_str(&text);
        }
    }
    // 返回本次冲刷的增量文本
}
```

- 补零到 `CHUNK_SAMPLES`（10000 样本 ≈ 0.61s）→ 用 `process_chunk`（**非** `process_final_chunk`）处理，保留 `feat_cache` / `alpha_cache` / `decoder_caches` 连续性；
- 补的零帧提供右上下文对齐 + 推动 CIF 累加器过阈 → 尾音 token 发射；
- drain 掉补零，不污染后续真实音频。

#### Zipformer（`crates/asr/src/streaming_zipformer.rs`）

```rust
/// Active flush: pad the current sample buffer with enough zeros
/// to force processing of the lookahead / right context of any remaining audio.
pub fn flush(&mut self) -> Result<Option<String>> {
    let h_frames = self.history_samples.len() / Z_FRAME_SHIFT;
    let required_total_samples = (h_frames + self.chunk_len + 1) * Z_FRAME_SHIFT;
    let current_total_samples = self.history_samples.len() + self.sample_buffer.len();
    if current_total_samples < required_total_samples {
        let needed = required_total_samples - current_total_samples;
        self.sample_buffer.resize(self.sample_buffer.len() + needed, 0.0);
    }
    self.process_chunks()
}
```

- 补零到 `(h_frames + chunk_len + 1) * Z_FRAME_SHIFT`，正好让 `process_chunks` 的就绪守卫 `h_frames + chunk_len >= feats.nrows()` 被绕过、放行恰好 **1 个 chunk**；
- 静音零填充符合「静音期」语义（对比 `finish()` 录音结束时复制最后一帧特征，两者场景不同，策略各异）。

### 3.2 累积层：flush 不插逗号（`crates/desktop/src/streaming_engine.rs`）

`StreamingSession` 统一对外返回**累积全文**。**Paraformer** 的 flush 把尾音增量追加到 `accumulated`，刻意不插逗号：

```rust
pub fn flush(&self) -> Result<Option<String>> {
    match self {
        Self::Paraformer { engine, accumulated } => {
            let mut eng = engine.lock().unwrap();
            match eng.flush()? {
                Some(delta) => {
                    let mut acc = accumulated.lock().unwrap();
                    acc.push_str(&delta);   // ← 注意：不插逗号
                    Ok(Some(acc.clone()))
                }
                None => Ok(None),
            }
        }
        Self::Zipformer { engine, accumulated } => { /* 见 §3.4：分段由 accept_samples 的 finish+reset 完成 */ }
    }
}
```

**关键差异（Paraformer）**：`accept_samples` 在「上轮静音 + 本轮有新文本」时会插入逗号（line 62-64）；`flush` **刻意省略**此逻辑——尾音是当前句的结尾，不应被当作新句起点的逗号分隔。这是修复「，下」误逗号的直接手段。

> **Zipformer 不同**（见 §3.4）：不靠 flush 吐尾音，而靠 VAD 静音时 `finish`+`reset` 显式分段。其 flush 分支虽存在（match 完整性），但静音 tick 下引擎已被 `accept_samples` 的 reset 清空，flush 多返回空——对 Zipformer 基本是 no-op。

### 3.3 Coordinator：`flushed` 标志状态机（`crates/desktop/src/coordinator.rs`）

`Stage::Streaming` 新增字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `flushed` | `bool` | 是否已对当前静音段进行过主动冲刷；恢复说话时重置为 `false` |

`handle_streaming_tick` 核心时序：

```rust
// ① VAD 检测，更新 silence_duration
let was_silent = detect_silence_gap(vad, &samples, silence_duration);

// ② 恢复说话 → 重置 flushed，允许下个静音段再次冲刷
if *silence_duration == 0.0 {
    *flushed = false;
}

// ③ 正常增量识别（was_silent 控制逗号）
match engine.accept_samples(&samples, was_silent) { ... }

// ④ 累积静音 ≥ 阈值且未冲刷 → 主动 flush 吐尾音（无逗号）
if *silence_duration >= PUNCTUATION_SILENCE_THRESHOLD && !*flushed {
    match engine.flush() { ... }
    *flushed = true;
}

// ⑤ 检查润色（flush 产生的新文本被 polish 基准正确计入）
check_and_trigger_polish(...);
```

### 3.4 Zipformer VAD 驱动分段（`accept_samples` 的 finish+reset，区别于 Paraformer flush）

> **新增（2026-06-15，方案 A）**：`StreamingSession::Zipformer` 由直接持有 `Mutex` 重构为 `{ engine, accumulated }`（与 Paraformer 对称），并实现基于 VAD 的 `finish`+`reset` 分段。本节描述该机制，与上文 Paraformer 的 flush 策略对照。

Zipformer 流式采用与 Paraformer **不同的分段策略**：不依赖 flush 补零吐尾音，而是在 VAD 判定静音时主动 `finish`+`reset` 把当前段归档、清空引擎状态，下一句从干净状态重新识别。

**动机**：Paraformer 的 flush（§3.1）保留引擎状态补零吐尾音，修复「尾音憋字」。但 Zipformer 状态化推理在长录音下 receptive field / cache 持续累积，分段边界易模糊、句间粘连。方案 A 选择**显式斩断**——静音即句界，`finish` 归档当前段 + `reset` 清状态，下段独立识别。

**机制**（`accept_samples` 的 Zipformer 分支）：

```rust
Self::Zipformer { engine, accumulated } => {
    let mut eng = engine.lock().unwrap();

    // ① 静音 → 斩断：finish 归档当前段到 accumulated，reset 清引擎状态
    if was_silent {
        let segment_text = eng.finish()?;
        let trimmed = segment_text.trim();
        if !trimmed.is_empty() {
            let mut acc = accumulated.lock().unwrap();
            if !acc.is_empty() { acc.push('，'); }   // 段间逗号
            acc.push_str(trimmed);
        }
        eng.reset();
    }

    // ② 当前段识别：返回 accumulated + 当前段拼接（段间逗号）
    match eng.accept_samples(samples)? {
        Some(current_segment) => { /* format!("{}，{}", accumulated, current_segment) */ }
        None => { /* 返回 accumulated（若有）或 None */ }
    }
}
```

要点：
- **连续静音不重复归档**：第 2+ 轮静音 tick 引擎已 reset，`finish()` 返回空 → `trimmed.is_empty()` 跳过 push，不重复插逗号。
- **段间逗号**：归档 `push('，')` + 显示 `format!("{}，{}")`，保证「段1，段2，…，当前段」。Zipformer CTC 输出无标点，无双逗号风险。
- **生命周期**：`finish()`（录音结束）归档末段 + `append_final_punctuation` 补句号；`reset()` 清引擎 + accumulated。

**flush 对 Zipformer 的角色（基本 no-op）**：coordinator 在 silence≥0.5s 时无差别调 `engine.flush()`（§3.3 ④）。对 Zipformer，同 tick `accept_samples(was_silent=true)` 已 `finish`+`reset`，reset 后 `eng.flush()` 补零无真实音频 → 多返回空。故 Zipformer 尾音由 `finish`（分段时）处理，**不靠 flush**；flush 分支仅为 match 完整性存在。

**Paraformer vs Zipformer 策略对比**：

| 维度 | Paraformer | Zipformer |
|------|-----------|-----------|
| 分段手段 | flush 补零吐尾音（状态连续） | was_silent 时 finish+reset（显式斩断） |
| accumulated 语义 | 连续全文，flush delta 直接追加（不插逗号） | 分段归档，段间逗号分隔 |
| 尾音修复 | flush（§3.1） | finish（分段归档时吐出） |
| 静音期 coordinator flush | 有效（吐尾音） | 基本 no-op（引擎已 reset） |

## 4. 时序推演（验证修复）

> 以下为 **Paraformer** 的 flush 时序（验证「，下」误逗号修复）。Zipformer 走 `finish`+`reset` 分段（§3.4），无独立 flush 路径、不依赖 `flushed` 标志。

用户说「等一下」→ 停 2s → 再说「你好」（Paraformer）：

| tick | 事件 | silence_duration | 动作 | accumulated_text |
|------|------|------------------|------|------------------|
| N | 说「等一」 | 0 | accept → 增量 | 等一 |
| N+1 | 停顿 | 0.6s | flush 触发（≥0.5 且 !flushed）→ 吐「下」，flushed=true | 等一下 |
| N+2 | 持续静音 | 1.2s | was_silent 但无新文本；flushed=true 跳过 | 等一下 |
| N+3 | 说「你好」 | →0 | flushed 重置；accept(was_silent=true) 插逗号 + 增量 | 等一下，你好 |

- 尾音「下」在停顿时即时出现、**无逗号** ✓
- 「你好」前正确插入逗号 ✓
- `flushed` 每个静音段恰好触发一次、恢复说话后正确重置 ✓

## 5. 副作用与权衡

| 项 | 说明 | 取舍 |
|----|------|------|
| Paraformer 虚拟帧位置编码偏移 | 每次 flush 让 `num_processed_frames` 前进 ~60 帧（≈0.61s），抬升后续真实音频的 positional encoding `t_offset` | 每静音段一次，影响轻微；若需严格零副作用，可记录注入帧数在后续位置编码扣除。当前可接受 |
| Zipformer history 含补零 | `process_chunks` 把末尾 `Z_FRAME_SHIFT` 样本存为 history，flush 后 history 可能是零样本，轻微影响下段 fbank 前缀 | 仅补约 1 chunk 量，影响轻微 |
| flush 失败 | 仅 `warn` 日志，`accumulated_text` 不变 | 与既有错误处理策略一致，识别完整性不受影响 |

## 6. 常量

| 常量 | 值 | 位置 | 说明 |
|------|-----|------|------|
| `PUNCTUATION_SILENCE_THRESHOLD` | `0.5`（秒）| `coordinator.rs` | 触发 flush 与插入逗号共用的静音阈值 |
| `CHUNK_SAMPLES` | `10000`（≈0.61s）| `streaming_paraformer.rs` | Paraformer 补零目标长度 |
| `STREAMING_TICK_INTERVAL_MS` | `600` | `coordinator.rs` | 流式 tick 间隔，决定 flush 检查频率 |

## 7. 验证

1. `cargo build --package octopus-desktop --features embedded`
2. 配置流式引擎（paraformer-streaming / zipformer）
3. 按快捷键 → 说「等一下」→ 停顿 2s → 观察「下」是否**即时**出现且**无逗号**
4. 继续说「你好」→ 确认「你好」前有逗号 → 「等一下，你好」
5. 测试连续多段静音（说一句→停→再说→停），确认每段 `flushed` 正确重置、尾音每段都即时吐出
6. 切换为 Zipformer 流式引擎重复 3-5：确认静音时 `finish`+`reset` 分段归档、段间逗号、连续静音不重复归档（机制见 §3.4，与 Paraformer flush 不同）
