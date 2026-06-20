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

## 流式 Whisper 特征全局归一化（关键约束）

`normalize_whisper_features`（log10 → 全局 max_v clamp to `max_v - 8` → shift）是**全局操作**，依赖整段特征的 max_v 做统一缩放。

- **离线引擎**：整段音频一次处理，天然全局归一化，正确。
- **流式引擎**：`process_chunks` / `finish` 必须在整段可用特征（`history_samples + buffer`）上**一次性**归一化，再按 chunk 切片送入 encoder。**不可 per-chunk 归一化**——每 ~45 帧单独 normalize 时，静音 chunk 的 max_v 极小、语音 chunk 极大，归一化尺度在 chunk 间剧烈跳变，encoder 输入分布不一致 → 输出乱码（"回 月 因 同"式重复 token）。

修复覆盖 CTC（`StreamingZipformer`）与 Transducer（`StreamingZipformerTransducer`）两套流式引擎的 `process_chunks` 和 `finish` 共四处。CTC 的 `zipformer-small-ctc` 走 fbank 特征（`is_whisper=false`）不受影响，但代码路径统一以便未来 whisper-CTC 模型即插即用。

