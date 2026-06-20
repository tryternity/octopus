# 流式 Paraformer fbank 特征提取修复

**日期**: 2026-06-21
**状态**: 已实现（待 e2e 验证）
**分支**: `feature/setting-ui2`

## 背景

流式 Paraformer 识别质量严重退化：输出文本出现大量 token 重复（如 `"thedayday"`、`"tomtomor"`、`"星星期三"`），且英文单词粘连无空格、中文停顿无逗号。

## 根因分析

通过逐层对比 sherpa-onnx（Python `sherpa_onnx` v1.13.2 + C++ `feature-window.cc`）源码，定位到 **fbank 特征提取** 层有 5 个缺陷：

### 缺陷 1: 缺少 DC offset removal
sherpa-onnx `FeatureExtractorConfig` 默认 `remove_dc_offset = true`——每帧 FFT 前减去帧均值。我们的 `compute_fbank()` 完全缺失此步骤。

### 缺陷 2: 缺少 pre-emphasis 滤波
sherpa-onnx 默认 `preemph_coeff = 0.97`——预加重滤波器 `y[i] = x[i] - 0.97 * x[i-1]`，提升高频能量补偿语音谱高频衰减。我们完全缺失。

### 缺陷 3: 窗口函数错误
流式 Paraformer 使用 **povey 窗** `(0.5 - 0.5*cos(2πi/(N-1)))^0.85`，而非 hamming 窗 `0.54 - 0.46*cos(...)`。povey = hanning^0.85，与 hamming 差异显著。

### 缺陷 4: mel 滤波器 high_freq 错误
sherpa-onnx 默认 `high_freq = -400`（即 Nyquist - 400 = **7600 Hz**），我们用了 **8000 Hz**（Nyquist），导致 mel 滤波器覆盖范围不一致。

### 缺陷 5: 流式架构 — 重叠 chunk 重复提取 fbank
原架构按音频 chunk 重复提取 fbank，相邻 chunk 有 1 帧（10ms）重叠但各自独立调用 `compute_fbank()`——pre-emphasis 状态（`x_prev`）无法跨 chunk 正确传递，导致重叠帧的 fbank 值不一致。

sherpa-onnx 采用**增量式**架构：`OnlineFbank` 线性追加音频样本，fbank 帧按序计算，pre-emphasis 状态自然跨所有帧传递。

## 修复方案

### 1. `compute_fbank()` 重构（`paraformer.rs`）

参数化窗口类型 + pre-emphasis 状态：

```rust
pub(crate) fn compute_fbank(
    samples: &[f32],
    window: &[f32],        // povey（流式）或 hamming（离线）
    preemph_coeff: f32,    // 0.97
    preemph_prev: &mut f32, // 跨帧状态
) -> Result<Array2<f32>>
```

帧处理流水线（对齐 knf `feature-window.cc`）：
```
帧样本提取 → DC offset removal（减帧均值）→ pre-emphasis（×0.97 跨帧状态）
→ povey/hamming 窗 → FFT → 功率谱 → mel 滤波器组 → log
```

### 2. povey 窗 + mel 滤波器修正

- 新增 `POVEY_WINDOW` static + `povey_window()` 函数
- `mel_filterbank_fbank()` 的 `high_freq` 从 8000 → 7600 Hz（`high_freq = -400`）
- mel 滤波器权重计算改为 mel 空间（此前已在上一轮修复）

### 3. 流式增量式 fbank 提取（`streaming_paraformer.rs`）

**完全重写**流式引擎的音频处理架构，从"按 chunk 提取"改为"线性追加 + 增量计算"：

| 原架构 | 新架构 |
|--------|--------|
| `sample_buffer: Vec<f32>`（原始样本） | `raw_samples: Vec<f32>`（× 32768 后样本） |
| 每 chunk 调 `compute_fbank(chunk_samples)` | `fbank_cache: Vec<f32>`（已计算的所有 fbank 帧） |
| pre-emphasis 状态每 chunk 重置 | `preemph_prev: f32` 跨所有帧正确传递 |
| chunk 间重叠帧 fbank 不一致 | 增量计算，无重复帧 |

数据流：
```
accept_samples(δsamples)
  → raw_samples.extend(δsamples × 32768)
  → compute_new_fbank_frames()    // 增量计算新帧，pre-emphasis 跨帧
  → while fbank_ready >= processed + CHUNK_SIZE:
      process_chunk_at(frame_start)  // 从 fbank_cache 切 CHUNK_SIZE 帧
      processed += CHUNK_SIZE - 1    // 1 帧重叠
```

`flush()` 补零到足够帧数后同样走 `process_chunk_at()`，最后一个 chunk force-fire CIF。

### 4. 英文单词空格 + chunk 间智能拼接

#### `decode_tokens` 重写（`paraformer.rs`）

对齐 sherpa-onnx `Convert()` 的空格逻辑：
- ASCII 词前加空格（非 subword 续接时）
- `@@` BPE 子词合并不加分隔
- 中英文边界（ASCII ↔ 非 ASCII）加空格

#### `smart_append` 辅助函数

chunk 边界拼接时自动检测 ASCII ↔ 非 ASCII 过渡并插入空格：
```rust
pub(crate) fn smart_append(existing: &mut String, new: &str) {
    // ASCII ↔ ASCII: 加空格
    // 中文 ↔ ASCII / ASCII ↔ 中文: 加空格
    // 中文 ↔ 中文: 不加空格
}
```

`StreamingParaformer::accept_samples` / `flush` 内部累积文本用 `smart_append`；
`StreamingSession::accept_samples` / `flush` 在 accumulated 与 delta 间用 `smart_append`。

### 5. VAD 停顿逗号即时反馈

`StreamingSession::flush(insert_comma: bool)` 新增参数：
- `insert_comma = true` 时，flush 产生的文本末尾**立即追加逗号**
- 此前逗号只在下一句话到来时才插入（`accept_samples` 的 `was_silent` 分支），停顿期间无标点反馈
- `coordinator.rs` 和 `server/main.rs` 的 flush 调用均传 `insert_comma = true`

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/asr/src/paraformer.rs` | `compute_fbank` 参数化 + DC offset + pre-emphasis + povey 窗 + mel high_freq + `decode_tokens` 重写 + `smart_append` |
| `crates/asr/src/streaming_paraformer.rs` | 增量式 fbank 架构重写（`raw_samples` + `fbank_cache` + `preemph_prev`）|
| `crates/asr/src/streaming_engine.rs` | `flush(insert_comma)` + `smart_append` 拼接 |
| `crates/desktop/src/coordinator.rs` | `flush(true)` 调用 |
| `crates/server/src/main.rs` | `flush(true)` 调用 |

## 验证

### 识别质量对比（test_wavs/0.wav）

| 版本 | 输出 |
|------|------|
| 修复前 | `昨天是mondaytodayisplease班二thedaydaytomtomorrow星星期三` |
| **修复后** | `昨天是 monday today day is 礼拜二 the day after tomorrow 是星期三` |
| sherpa-onnx 参考值 | `昨天是 monday today day is 礼拜二 the day after tomorrow 是星期` |

47 项单元测试全通过，server/cli/desktop release 构建成功。

## 后续优化（同分支追加）

### 6. BPE 跨 chunk 整体解码

**问题**：`value` 被切成 `val@@` + `ue` 两个 token，chunk 边界各自 decode 导致断词。

**修复**：`StreamingParaformer` 新增 `all_token_ids: Vec<i64>` 跨 chunk 累积所有有效 token ID，`accept_samples` / `flush` 整体调用 `decode_tokens(all_token_ids)` 返回完整 ASR 文本。`process_chunk_at` 只累积 token 不再逐 chunk 解码。

`StreamingSession` Paraformer 路径改为 `punct_prefix` + `committed_chars` 双字段逗号管理：静音点冻结当前 ASR 快照 + 插逗号（`committed_chars` 推进），后续新 delta（`full_asr` skip 已提交字符）拼在逗号后。

### 7. 热路径性能优化（零拷贝）

| 优化点 | 每次节省 | 方式 |
|--------|---------|------|
| decoder_caches 更新 | ~320KB 堆分配（16×512×10×4B） | `copy_from_slice` 复用预分配 Array3，维度变化才重分配 |
| encoder 特征构造 | ~45KB clone（10×560×4B） | `into_shape` 零拷贝 reshape 替代 `iter().cloned().collect()` |
| run_cif encoder 数据 | ~20-40KB 拷贝 | `as_slice().unwrap()` 直接拿 `&[f32]`，移除 `.to_vec()` |
| decoder input 键名 | 16× `format!()` | `cache_keys: Vec<String>` 预分配于 `new()` |

### 8. mask_alphas 越界防护

`mask_alphas` / `mask_alphas_left_only` 改为 `n = alphas.len().min(enc_len)` 再循环，消除 `alphas.len() < enc_len` 时的 panic 风险。

