# Zipformer Transducer（RNN-T）引擎设计

## 背景

原 `ZipformerEngine` 仅支持 CTC 解码（单 session → log_probs → argmax）。新增两个 Transducer 模型需要 RNN-T 解码（encoder + decoder + joiner 三 session 架构）：

- `csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（154M, encoder_dim=512）
- `csukuangfj/sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30`（726M, encoder_dim=768）

## 设计决策

### 1. 重命名而非扩展

`ZipformerEngine` → `ZipformerCtcEngine`，新建 `ZipformerTransducerEngine`。原因：
- CTC 和 Transducer 解码控制流根本不同（CTC: 单 session argmax + blank/repeat skip；Transducer: 三 session RNN-T greedy decoding with inner emit loop）
- 符合代码库已有模式（每引擎一个 struct）
- 共享代码已提取为自由函数（`load_vocab`、`initial_encoder_states`、`decode_token_ids`）

### 2. 路由层检测（不新增 EngineCategory）

`EngineCategory::Zipformer` 分支内检测 `decoder.onnx` 存在性：
- 有 → `ZipformerTransducerEngine`
- 无 → `ZipformerCtcEngine`

### 3. RNN-T Greedy Decoding

遵循 sherpa-onnx 标准流式 greedy search 约定：
- `token_buf` 初始 `[-1, ..., -1, 0]`（长度 = context_size，末位 blank）
- 每个 encoder frame：`joiner(enc_frame, decoder_out) → logit → argmax`
- 非 blank：发射 token，滑动窗口更新，重跑 decoder
- blank：移到下一 encoder frame
- 内循环安全上限 20 次/frame

## 共享函数提取

| 函数 | 位置 | 用途 |
|---|---|---|
| `load_vocab(hf_path)` | zipformer.rs | tokens.txt → Vec<String> |
| `initial_encoder_states(session)` | zipformer.rs | 遍历 encoder inputs 创建零张量初始状态 |
| `decode_token_ids(vocab, is_bbpe, ids)` | zipformer.rs | token ID 序列 → 文本（BBPE + SentencePiece byte-fallback） |

## 引擎结构

```rust
pub struct ZipformerTransducerEngine {
    encoder_session: Mutex<Session>,
    decoder_session: Mutex<Session>,
    joiner_session: Mutex<Session>,
    chunk_len: usize,       // T=45（从 encoder metadata 读）
    chunk_shift: usize,     // decode_chunk_len=32
    context_size: usize,    // 2（从 decoder metadata 读）
    vocab: Vec<String>,
    is_bbpe: bool,
    initial_states: Vec<(String, StateValue)>,
    is_whisper: bool,       // 两新模型 feature=whisper
}
```

## 数据流

```
音频 → compute_whisper_features_linear → normalize → chunked encoder inference
                                                     ↓
                                              encoder_out [T', enc_dim]
                                                     ↓
                                    RNN-T greedy decoding (per frame):
                                      decoder(token_buf) → decoder_out
                                      joiner(enc_frame, decoder_out) → logit
                                      argmax → blank? next frame : emit + re-run decoder
                                                     ↓
                                              decode_token_ids → 文本
```

## 流式 Transducer 引擎（StreamingZipformerTransducer）

Transducer 模型（zh-int8 / xlarge）原生支持流式（`is_streaming=1`），故除离线引擎外还需流式引擎。

### 流式引擎分流

`StreamingSession::new`（`streaming_engine.rs`）检测模型目录下 `decoder.onnx` 存在性：
- **无 `decoder.onnx`** → `StreamingZipformer`（CTC，单 session log_probs argmax）
- **有 `decoder.onnx`** → `StreamingZipformerTransducer`（RNN-T，三 session greedy decoding）

两者实现 `ZipformerStreamOps` trait，`StreamingSession` 通过 trait 统一分发 `accept_samples` / `flush` / `finish` / `reset`，消除重复代码。

### 跨 chunk 持久状态

| 状态 | 说明 |
|---|---|
| `token_buf: Vec<i64>` | decoder 上下文窗口（长度 = context_size，默认 2），初始化 `[-1, ..., -1, 0]` |
| `emitted_ids: Vec<usize>` | 累积输出 token ID |
| `states: Vec<(String, StateValue)>` | encoder 缓存（cached_key/N、cached_val/N 等，与 CTC 相同） |

### 关键设计：new_from_entry

`StreamingSession::new` 经 `resolve_active_engine` 解析 entry 后，直接传 `entry` 给流式引擎的 `new_from_entry()`——而非传 bare_name 让引擎内部再查 DB。避免双重 DB 查找 + 可能选错 entry。

### run_chunk 两阶段借用

ort 2.0.0-rc.12 的 `SessionOutputs` 持有 session 的借用，调 decoder/joiner 前必须结束该借用。`run_chunk` 采用两阶段：
1. encoder session run → `SessionOutputs` → 提取 encoder_out 到 owned `Vec<f32>`（借用结束）
2. 用 owned 数据调 decoder/joiner session

### 流式 RNN-T 解码

每个 chunk 的 encoder_out 逐 frame 跑 joiner → argmax：
- **非 blank(0)**：发射 token、`token_buf` 滑动窗口更新、重跑 decoder 获取新 decoder_out
- **blank(0)**：移到下一 frame
- 内循环安全上限 20 次/frame（防理论无限循环）

## Whisper 特征归一化（3 个根因修复）

对比 sherpa-onnx 官方 C++ 实现，发现并修复 3 个导致流式 Transducer 质量差的根因：

### 根因 1：归一化公式错误

sherpa-onnx `NormalizeWhisperFeatures`（`math.cc`）：
```
mel = (max(log10(clamp(x, 1e-10)), max_v - 8.0) + 4.0) / 4.0
```
输出范围 ~0-2。我们的实现错误地用 `clamped - clamp_min`（输出范围 0-8，尺度差 4 倍），ONNX 模型输入分布不匹配。修正为 `(clamped + 4.0) / 4.0`。

### 根因 2：Transducer history 泄漏

`StreamingZipformerTransducer::process_chunks` 保留**全部未消费样本**作为 `history_samples`（可达上万样本），而非仅 1 帧（160 samples）。导致每次重算特征时归一化 max_v 剧烈跳变。修复为与 CTC 引擎一致的 1-frame history。

### 根因 3：流式归一化 scope

sherpa-onnx 做 **per-chunk 归一化**（每个 chunk 独立 normalize，配合增量特征计算）。此前误改为 pseudo-global（每次重算 history+buffer 全局归一化），但由于 history/buffer 内容每次不同，max_v 仍不稳定。回退为 per-chunk 归一化——与 sherpa-onnx 行为一致。

修复覆盖 CTC + Transducer 两套流式引擎的 `process_chunks` 和 `finish` 共四处 + `normalize_whisper_features` 函数本身。

