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
