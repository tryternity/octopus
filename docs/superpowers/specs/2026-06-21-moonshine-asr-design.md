# Moonshine ASR 引擎接入设计

**日期**: 2026-06-21
**状态**: 设计完成，待实现
**分支**: `feature/setting-ui2`

## 背景

项目需要接入 [Moonshine](https://github.com/moonshine-ai/moonshine) 语音识别模型——Useful Sensors 开发的端侧 ASR，专为低延迟优化。已通过 `csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8` 下载到 HF 缓存。

与 Whisper 相比：模型更小（tiny 26M / base 58M）、无需 30s padding、macOS arm64 上延迟更低（tiny ~34ms vs whisper ~277ms）。

## 模型架构

Moonshine 是**纯 ONNX 体系**的 encoder-decoder Transformer，与项目现有 `ort` 依赖完全契合，无需引入新框架。

### 4 个 ONNX session（v1 格式）

| Session | 输入 | 输出 | 作用 |
|---------|------|------|------|
| `preprocess.onnx` | `audio (1, N)` f32 | `features (1, T, 416)` | 学习型 conv 前端（替代手写 Mel），下采样率 384× |
| `encode.int8.onnx` | `features (1, T, 416)` + `features_len (1,)` i32 | `encoder_out (1, T, 416)` | Transformer encoder |
| `uncached_decode.int8.onnx` | `token (1, L)` i32 + `encoder_out` + `seq_len (1,)` i32 | `logits (1, 1, 32768)` + **36 个 KV cache 张量** `(1, 8, 52)` | 首个 token 解码（初始化 KV cache） |
| `cached_decode.int8.onnx` | `token (1, L)` + `encoder_out` + `seq_len` + **36 个 KV cache 张量** | `logits (1, 1, 32768)` + 36 个新 KV cache 张量 | 后续 token 解码（复用 KV cache） |

- **36 个 cache 张量** = 18 层 × (K, V)，每个 shape `(1, 8, 52)`
- **vocab 32768**：byte-level BPE（与 Llama 1/2 兼容），`tokens.txt` 格式（token_text + tab + token_id）
- **特殊 token**：`<unk>=0`, `<s>=1`(BOS), `</s>=2`(EOS)

### Decode 循环（sherpa-onnx `offline-moonshine-greedy-search-decoder.cc` 参考）

```
BOS(1) → uncached_decode → logits + 36 cache
         ↓ argmax → token_0
         (EOS? stop)
token_0 → cached_decode(prev_cache) → logits + 36 new cache
         ↓ argmax → token_1
         (EOS? stop)
... 循环至 EOS(2) 或 max_len
```

`max_len = audio_seconds * 6`（语音每秒约 6 个 token 上限）。

### 文件布局

```
~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-moonshine-base-en-int8/snapshots/<hash>/
├── preprocess.onnx
├── encode.int8.onnx
├── uncached_decode.int8.onnx
├── cached_decode.int8.onnx
├── tokens.txt              # 32768 行，格式: "token_text\ttoken_id"
└── test_wavs/
```

## 设计

### 1. 新增 `EngineCategory::Moonshine`

**`crates/asr/src/config.rs`**：

```rust
pub enum EngineCategory {
    Whisper,
    SenseVoice,
    Paraformer,
    Qwen3Asr,
    Zipformer,
    Moonshine,  // ← 新增
    Aliyun,
}
```

映射函数（4 处）：
- `engine_category_from_str`: `"moonshine" => Some(Moonshine)`
- `category_label`: `Moonshine => "moonshine"`
- `all_sections`: 新增 `(cfg.asr.moonshine.as_ref(), Moonshine)`
- `pick_entry`: `Moonshine => cfg.asr.moonshine.as_ref()`

### 2. 新增 `AsrSection.moonshine` 字段

**`crates/infra/src/db.rs`**：

```rust
pub struct AsrSection {
    pub whisper: Option<HashMap<String, ModelEntry>>,
    // ... existing ...
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,  // ← 新增
    pub aliyun: Option<HashMap<String, ModelEntry>>,
}
```

`load_asr_config` 的 category 映射追加 `(_, "moonshine") => &mut asr.moonshine`。

### 3. 新建 `crates/asr/src/moonshine.rs`

实现 `OfflineAsrEngine` trait。

```rust
pub struct MoonshineEngine {
    preprocess_session: Session,
    encode_session: Session,
    uncached_decode_session: Session,
    cached_decode_session: Session,
    vocab: Vec<String>,           // tokens.txt 加载
    // Session 是 Send+Sync（ort 保证），无需 Mutex 包裹
}
```

**`new(entry: &ModelEntry)`**：
1. `resolve_model_dir(&entry.source)` 定位 HF 缓存目录
2. 加载 4 个 ONNX session（preprocess / encode / uncached_decode / cached_decode）
3. 加载 `tokens.txt` 为 `Vec<String>`（32768 项）

**`transcribe(&self, samples: &[f32], _language: &str) -> Result<String>`**：

```rust
fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
    // 1. preprocess: audio (1, N) → features (1, T, 416)
    let features = self.run_preprocess(samples)?;
    let features_len = features.shape()[1] as i32;

    // 2. encode: features → encoder_out (1, T, 416)
    let encoder_out = self.run_encode(features, features_len)?;

    // 3. decode loop (greedy)
    let max_len = (samples.len() as f32 / 16000.0 * 6.0) as i32;
    let token_ids = self.greedy_decode(&encoder_out, features_len, max_len)?;

    // 4. tokens → text
    Ok(decode_tokens(&token_ids, &self.vocab))
}
```

#### greedy_decode 内部逻辑

```
token = [1]  // BOS
seq_len = [1]

// 首 token: uncached_decode
(logits, kv_caches) = uncached_decode(token, encoder_out, seq_len)

loop:
    next_token = argmax(logits)
    if next_token == EOS(2): break
    tokens.push(next_token)
    seq_len += 1

    // 后续 token: cached_decode
    (logits, kv_caches) = cached_decode([next_token], encoder_out, seq_len, kv_caches)
```

#### KV cache 管理

`kv_caches: Vec<Array3<f32>>`（36 个张量），在 `greedy_decode` 内部维护：
- uncached_decode 输出 index 1..37（跳过 index 0 logits）→ 初始化 cache
- cached_decode 输入 3..39（token + encoder_out + seq_len + 36 cache）→ 输出 index 1..37 → 更新 cache

### 4. `AsrEngineManager` 路由

**`crates/asr/src/engine.rs:69`** match 追加：

```rust
config::EngineCategory::Moonshine => Arc::new(MoonshineEngine::new(entry)?),
```

### 5. CLI 入口

**`crates/cli/src/main.rs`** 追加 Moonshine 分支（类似 whisper 的 `transcribe` 路径）。

## 不涉及

- **流式识别**：Moonshine v1 是 offline 模型（v2 有 streaming 但当前使用 v1）
- **CoreML/Metal 加速**：preprocess/encode/decode 已 INT8 量化，`apply_session_acceleration` 自动适用
- **多语言**：当前模型为 `en` only（Moonshine 有其他语言版本但不在本次范围）
- **VAD 分段**：长音频走现有 `transcribe_with_vad`（`engine.rs:134`），与 Whisper 路径一致

## 验证

- 单元测试：加载 `sherpa-onnx-moonshine-base-en-int8`，对 `test_wavs/` 内置样本识别，对比 sherpa-onnx 输出
- CLI 测试：`cargo run -p octopus-cli -- transcribe <wav> --model moonshine-base-en`
- 现有引擎回归：whisper / paraformer / zipformer 测试不受影响

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/asr/src/moonshine.rs` | **新建**：MoonshineEngine 实现 |
| `crates/asr/src/lib.rs` | `pub mod moonshine;` |
| `crates/asr/src/config.rs` | `EngineCategory::Moonshine` + 4 处映射 |
| `crates/asr/src/engine.rs` | match 路由 |
| `crates/infra/src/db.rs` | `AsrSection.moonshine` + `load_asr_config` 映射 |
| `crates/cli/src/main.rs` | CLI transcribe 入口 |
