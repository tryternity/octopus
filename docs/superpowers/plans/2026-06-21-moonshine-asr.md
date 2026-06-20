# Moonshine ASR 引擎接入实施计划

> **For agentic workers:** REQUIRED SUB-SILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 接入 Moonshine ONNX ASR 模型（v1 格式，4 个 ONNX session），实现离线英语语音识别。

**Architecture:** 新建 `MoonshineEngine` 实现 `OfflineAsrEngine` trait，管理 4 个 ONNX session（preprocess → encode → uncached_decode → cached_decode 循环）。纯 ONNX 体系，无新依赖。category=`moonshine` 走 DB models 表配置。

**Tech Stack:** `ort`（ONNX Runtime）、`ndarray`、`anyhow`。模型来自 `csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8`（HF 缓存已就绪）。

---

## 文件结构

| 文件 | 职责 | 动作 |
|------|------|------|
| `crates/infra/src/db.rs` | `AsrSection` 结构 + `load_asr_config` 映射 | 修改 |
| `crates/asr/src/config.rs` | `EngineCategory` enum + 映射函数 | 修改 |
| `crates/asr/src/moonshine.rs` | `MoonshineEngine` 实现 | **新建** |
| `crates/asr/src/lib.rs` | 模块声明 | 修改 |
| `crates/asr/src/engine.rs` | `AsrEngineManager` 路由 | 修改 |
| `crates/cli/src/main.rs` | CLI 入口 | 修改 |

---

### Task 1: infra 层 — AsrSection 新增 moonshine 字段

**Files:**
- Modify: `crates/infra/src/db.rs:31-43`（AsrSection struct）
- Modify: `crates/infra/src/db.rs:416-424`（load_asr_config match）

- [ ] **Step 1: AsrSection 新增 moonshine 字段**

在 `crates/infra/src/db.rs` 的 `AsrSection` struct 中，`zipformer` 和 `aliyun` 之间新增：

```rust
    #[serde(default)]
    pub zipformer: Option<HashMap<String, ModelEntry>>,
    /// Moonshine 端侧 ASR（Useful Sensors）。provider='local' + category='moonshine' 路由入此。
    #[serde(default)]
    pub moonshine: Option<HashMap<String, ModelEntry>>,
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    #[serde(default)]
    pub aliyun: Option<HashMap<String, ModelEntry>>,
```

- [ ] **Step 2: load_asr_config 映射追加**

在 `crates/infra/src/db.rs:416` 的 match 中，`zipformer` 和 default 之间新增：

```rust
            (_, "zipformer") => &mut asr.zipformer,
            (_, "moonshine") => &mut asr.moonshine,
            _ => continue,
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-infra`
Expected: 编译成功

- [ ] **Step 4: 运行 infra 测试**

Run: `cargo test -p octopus-infra`
Expected: 全部通过

---

### Task 2: asr config 层 — EngineCategory + 映射

**Files:**
- Modify: `crates/asr/src/config.rs:124-132`（enum）
- Modify: `crates/asr/src/config.rs:138-147`（engine_category_from_str）
- Modify: `crates/asr/src/config.rs:234-244`（category_label）
- Modify: `crates/asr/src/config.rs:160-170`（all_sections）
- Modify: `crates/asr/src/config.rs:373-382`（pick_entry）

- [ ] **Step 1: enum 新增 Moonshine variant**

```rust
pub enum EngineCategory {
    Whisper,
    SenseVoice,
    Paraformer,
    Qwen3Asr,
    Zipformer,
    Moonshine,
    /// 阿里云云端 ASR（DashScope Fun-ASR 实时）。provider='aliyun' 路由入此。
    Aliyun,
}
```

- [ ] **Step 2: engine_category_from_str 映射**

```rust
        "zipformer" => Some(EngineCategory::Zipformer),
        "moonshine" => Some(EngineCategory::Moonshine),
        _ => None,
```

- [ ] **Step 3: category_label 映射**

```rust
        Zipformer => "zipformer",
        Moonshine => "moonshine",
        Aliyun => "Fun-ASR",
```

- [ ] **Step 4: all_sections 追加 moonshine**

```rust
    [
        (cfg.asr.whisper.as_ref(), EngineCategory::Whisper),
        (cfg.asr.sensevoice.as_ref(), EngineCategory::SenseVoice),
        (cfg.asr.paraformer.as_ref(), EngineCategory::Paraformer),
        (cfg.asr.qwen3_asr.as_ref(), EngineCategory::Qwen3Asr),
        (cfg.asr.zipformer.as_ref(), EngineCategory::Zipformer),
        (cfg.asr.moonshine.as_ref(), EngineCategory::Moonshine),
        (cfg.asr.aliyun.as_ref(), EngineCategory::Aliyun),
    ]
```

注意：数组维度从 `[..; 6]` 改为 `[..; 7]`。

- [ ] **Step 5: pick_entry 追加**

```rust
        EngineCategory::Zipformer => cfg.asr.zipformer.as_ref(),
        EngineCategory::Moonshine => cfg.asr.moonshine.as_ref(),
        EngineCategory::Aliyun => cfg.asr.aliyun.as_ref(),
```

- [ ] **Step 6: 编译验证**

Run: `cargo build -p octopus-asr`
Expected: 编译成功（moonshine module 尚未引用，纯 enum 变更）

- [ ] **Step 7: 测试验证**

Run: `cargo test -p octopus-asr -- --nocapture config`
Expected: config 相关测试全通过

---

### Task 3: moonshine.rs — MoonshineEngine 实现

**Files:**
- Create: `crates/asr/src/moonshine.rs`
- Modify: `crates/asr/src/lib.rs`

- [ ] **Step 1: lib.rs 声明模块**

在 `crates/asr/src/lib.rs` 追加（位置在 `pub mod whisper;` 附近）：

```rust
pub mod moonshine;
```

- [ ] **Step 2: 创建 moonshine.rs 骨架（tokens 加载 + new + struct）**

创建 `crates/asr/src/moonshine.rs`：

```rust
use anyhow::{Context, Result};
use ort::session::Session;
use std::collections::HashMap;

use crate::config;

/// Moonshine ASR 引擎 — 纯 ONNX 体系，4 session 流水线。
///
/// 模型来自 csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8（v1 格式）。
/// 推理流程：preprocess → encode → uncached_decode（首 token，初始化 KV cache）
///           → cached_decode 循环（后续 token，复用 KV cache）→ EOS 停止。
pub struct MoonshineEngine {
    preprocess_session: Session,
    encode_session: Session,
    uncached_decode_session: Session,
    cached_decode_session: Session,
    vocab: Vec<String>,
}

impl MoonshineEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)
            .context("Failed to resolve Moonshine model dir")?;

        // 4 个 ONNX session（v1 格式：固定文件名）
        let preprocess_path = hf_path.join("preprocess.onnx");
        let encode_path = hf_path.join("encode.int8.onnx");
        let uncached_path = hf_path.join("uncached_decode.int8.onnx");
        let cached_path = hf_path.join("cached_decode.int8.onnx");

        for (name, p) in [
            ("preprocess", &preprocess_path),
            ("encode", &encode_path),
            ("uncached_decode", &uncached_path),
            ("cached_decode", &cached_path),
        ] {
            if !p.exists() {
                anyhow::bail!("Moonshine {} not found at {}", name, p.display());
            }
        }

        let preprocess_session = config::apply_session_acceleration(
            ort::session::SessionBuilder::new()?
                .with_optimization_level(ort::session::GraphOptimizationLevel::Level3)?
                .with_intra_threads(1)?
                .with_model_from_file(&preprocess_path)?,
        )?;
        let encode_session = config::apply_session_acceleration(
            ort::session::SessionBuilder::new()?
                .with_optimization_level(ort::session::GraphOptimizationLevel::Level3)?
                .with_intra_threads(1)?
                .with_model_from_file(&encode_path)?,
        )?;
        let uncached_decode_session = config::apply_session_acceleration(
            ort::session::SessionBuilder::new()?
                .with_optimization_level(ort::session::GraphOptimizationLevel::Level3)?
                .with_intra_threads(1)?
                .with_model_from_file(&uncached_path)?,
        )?;
        let cached_decode_session = config::apply_session_acceleration(
            ort::session::SessionBuilder::new()?
                .with_optimization_level(ort::session::GraphOptimizationLevel::Level3)?
                .with_intra_threads(1)?
                .with_model_from_file(&cached_path)?,
        )?;

        // 加载 tokens.txt（格式：token_text\ttoken_id，32768 行）
        let vocab = load_tokens(&hf_path.join("tokens.txt"))?;
        if vocab.len() != 32768 {
            anyhow::bail!(
                "Moonshine vocab size mismatch: expected 32768, got {}",
                vocab.len()
            );
        }

        Ok(Self {
            preprocess_session,
            encode_session,
            uncached_decode_session,
            cached_decode_session,
            vocab,
        })
    }
}

/// 加载 tokens.txt：每行 "token_text\ttoken_id"，按 id 索引构建 vocab。
fn load_tokens(path: &std::path::Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read tokens.txt at {}", path.display()))?;
    let mut vocab: HashMap<i64, String> = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.rsplitn(2, '\t').collect();
        if parts.len() == 2 {
            let token_text = parts[1].to_string();
            let token_id: i64 = parts[0].parse()
                .with_context(|| format!("Invalid token id in tokens.txt: {}", parts[0]))?;
            vocab.insert(token_id, token_text);
        }
    }
    let max_id = vocab.keys().copied().max().unwrap_or(-1);
    let mut result = vec![String::new(); (max_id + 1) as usize];
    for (id, text) in vocab {
        result[id as usize] = text;
    }
    Ok(result)
}
```

- [ ] **Step 3: 编译骨架验证**

Run: `cargo build -p octopus-asr`
Expected: 编译成功（struct + new 骨架通过）

- [ ] **Step 4: 实现 transcribe + 3 个 run_* 辅助方法**

在 `MoonshineEngine` 的 `impl` 块中追加：

```rust
    /// 运行 preprocess：audio (1, N) → features (1, T, 416)
    fn run_preprocess(&self, samples: &[f32]) -> Result<ndarray::Array2<f32>> {
        let audio = ndarray::ArrayView2::from_shape(
            (1, samples.len()),
            samples,
        )?;
        let outputs = self.preprocess_session.run(ort::inputs! {
            "args_0" => audio?
        }?)?;
        // 输出是 (1, T, 416)，reshape 为 (T, 416) 便于后续
        let out = outputs["sequential"].try_extract_tensor::<f32>()?;
        let shape = out.0.iter().map(|&d| d as usize).collect::<Vec<_>>();
        Ok(ndarray::Array2::from_shape_vec(
            (shape[1], shape[2]),
            out.1.to_vec(),
        )?)
    }

    /// 运行 encode：features (1, T, 416) → encoder_out (1, T, 416)
    fn run_encode(&self, features: &ndarray::Array2<f32>, features_len: usize) -> Result<ndarray::Array3<f32>> {
        let (t, dim) = (features.nrows(), features.ncols());
        let features_3d = features.view().insert_axis(ndarray::Axis(0)); // (1, T, dim)
        let features_len_arr = [features_len as i32];
        let outputs = self.encode_session.run(ort::inputs! {
            "args_0" => features_3d?,
            "args_1" => ndarray::ArrayView1::from(&features_len_arr)?
        }?)?;
        let out = outputs["layer_normalization_16"].try_extract_tensor::<f32>()?;
        let shape = out.0.iter().map(|&d| d as usize).collect::<Vec<_>>();
        Ok(ndarray::Array3::from_shape_vec(
            (shape[0], shape[1], shape[2]),
            out.1.to_vec(),
        )?)
    }

    /// Greedy decode 循环
    fn greedy_decode(
        &self,
        encoder_out: &ndarray::Array3<f32>,
        features_len: i32,
    ) -> Result<Vec<i64>> {
        const BOS: i64 = 1;
        const EOS: i64 = 2;
        let audio_seconds = features_len as f32 * 384.0 / 16000.0;
        let max_len = (audio_seconds * 6.0) as i32 + 10;

        let enc_view = encoder_out.view();

        // 首 token: uncached_decode
        let token = ndarray::ArrayView2::from_shape((1, 1), &[BOS])?;
        let seq_len = [1i32];
        let uncached_out = self.uncached_decode_session.run(ort::inputs! {
            "args_0" => token?,
            "args_1" => enc_view?,
            "args_2" => ndarray::ArrayView1::from(&seq_len)?
        }?)?;

        // 提取 logits（index 0）+ KV cache（index 1..37）
        let outputs_vec: Vec<_> = uncached_out.into_iter().collect();
        let (logits_shape, logits_data) = outputs_vec[0].1.try_extract_tensor::<f32>()?;
        let vocab_size = logits_shape[2] as usize;
        let mut kv_caches: Vec<Vec<f32>> = Vec::with_capacity(36);
        let mut kv_shapes: Vec<(usize, usize, usize)> = Vec::with_capacity(36);
        for i in 1..37 {
            let (shape, data) = outputs_vec[i].1.try_extract_tensor::<f32>()?;
            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            kv_caches.push(data.to_vec());
            kv_shapes.push((dims[0], dims[1], dims[2]));
        }

        let mut result_tokens: Vec<i64> = Vec::new();
        let mut last_logits: Vec<f32> = logits_data.to_vec();

        for _ in 0..max_len {
            // argmax
            let next_token = last_logits[..vocab_size]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .map(|(i, _)| i as i64)
                .unwrap_or(EOS);

            if next_token == EOS {
                break;
            }
            result_tokens.push(next_token);

            // cached_decode
            let seq_len_val = (result_tokens.len() + 1) as i32;
            let token_arr = ndarray::ArrayView2::from_shape((1, 1), &[next_token as i32])?;
            let seq_len = [seq_len_val];
            let mut inputs = ort::inputs! {
                "args_0" => token_arr?,
                "args_1" => enc_view?,
                "args_2" => ndarray::ArrayView1::from(&seq_len)?
            }?;

            // 喂入 36 个 KV cache
            for (i, (cache, &(d0, d1, d2))) in kv_caches.iter().zip(kv_shapes.iter()).enumerate() {
                let cache_arr = ndarray::ArrayView3::from_shape((d0, d1, d2), cache)?;
                inputs.push((format!("args_{}", i + 3).into(), ort::value::TensorRef::from_array_view(cache_arr)?.into()));
            }

            let cached_out = self.cached_decode_session.run(inputs)?;

            // 更新 logits + cache
            let cached_vec: Vec<_> = cached_out.into_iter().collect();
            let (new_logits_shape, new_logits_data) = cached_vec[0].1.try_extract_tensor::<f32>()?;
            vocab_size = new_logits_shape[2] as usize;
            last_logits = new_logits_data.to_vec();
            for i in 0..36 {
                let (_, new_data) = cached_vec[i + 1].1.try_extract_tensor::<f32>()?;
                kv_caches[i] = new_data.to_vec();
            }
        }

        Ok(result_tokens)
    }
```

注意：以上代码中 `ort::inputs!` 返回的 SessionOutputs 的索引顺序——sherpa-onnx 用 output name 遍历，但 ort crate 的 SessionOutputs 可以按 index 遍历。实际实现时需确认 ort crate 2.0 的 API（`try_extract_tensor` 返回 `(&[i64], ArrayViewD<f32>)`）。

- [ ] **Step 5: 实现 OfflineAsrEngine trait + decode_tokens + 顶层 transcribe**

```rust
impl crate::engine::OfflineAsrEngine for MoonshineEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let features = self.run_preprocess(samples)?;
        let features_len = features.nrows() as i32;
        let encoder_out = self.run_encode(&features, features_len as usize)?;
        let token_ids = self.greedy_decode(&encoder_out, features_len)?;
        Ok(decode_moonshine_tokens(&token_ids, &self.vocab))
    }
}

/// Moonshine byte-level BPE 解码：直接拼接 vocab[token_id]，无需 BPE merge 处理
/// （merge 在 ONNX 模型内部完成，输出的 token_id 已经是最终文本 token）。
fn decode_moonshine_tokens(token_ids: &[i64], vocab: &[String]) -> String {
    let mut text = String::new();
    for &id in token_ids {
        let id = id as usize;
        if id < vocab.len() {
            text.push_str(&vocab[id]);
        }
    }
    text
}

/// 顶层 transcribe 入口（CLI 用）
pub fn transcribe(name: &str, samples: &[f32], language: &str) -> Result<String> {
    let cfg = config::load_config()?;
    let entry = config::pick_entry(&cfg, config::EngineCategory::Moonshine, name)
        .with_context(|| format!("Moonshine model '{}' not found in config", name))?;
    let engine = MoonshineEngine::new(entry)?;
    engine.transcribe(samples, language)
}
```

- [ ] **Step 6: 编译验证**

Run: `cargo build -p octopus-asr`
Expected: 编译成功。若有 ort API 不匹配，按编译错误调整（ort 2.0-rc API 可能有细节差异）。

- [ ] **Step 7: 运行现有 ASR 测试确认无回归**

Run: `cargo test -p octopus-asr --release`
Expected: 52+ tests passed（现有测试不受影响）

---

### Task 4: engine.rs 路由 + CLI 入口

**Files:**
- Modify: `crates/asr/src/engine.rs:69`（match 路由）
- Modify: `crates/cli/src/main.rs`（transcribe 入口）

- [ ] **Step 1: engine.rs import + match 路由**

在 `crates/asr/src/engine.rs` 的 import 段追加：

```rust
use crate::moonshine::MoonshineEngine;
```

在 `switch_model` 的 match 中（`Zipformer` 之前或之后）追加：

```rust
                config::EngineCategory::Moonshine => Arc::new(MoonshineEngine::new(entry)?),
```

- [ ] **Step 2: CLI 入口**

在 `crates/cli/src/main.rs` 找到 whisper transcribe 的调用位置，追加 Moonshine 分支：

```rust
        // 在 match category 或条件分支中
        config::EngineCategory::Moonshine => {
            octopus_asr::moonshine::transcribe(bare, samples, language)
        }
```

具体位置需查看现有 CLI 如何按 category 分发（可能有 `match` 或 `if` 链）。

- [ ] **Step 3: 编译全部**

Run: `cargo build --release -p octopus-asr -p octopus-cli`
Expected: 编译成功

- [ ] **Step 4: CLI 功能测试（真实模型）**

Run: `cargo run --release -p octopus-cli -- transcribe ~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-moonshine-base-en-int8/snapshots/*/test_wavs/*.wav --model moonshine-base-en`

Expected: 输出英语识别文本（具体取决于 test_wavs 内容）。

> 注：`moonshine-base-en` 是 DB models 表中的 model_name。如果 DB 尚无此条目，需先插入：
> ```sql
> INSERT INTO models (domain, provider, category, model_name, source, language, is_local, is_streaming, is_enabled, description)
> VALUES ('asr', 'local', 'moonshine', 'moonshine-base-en', 'csukuangfj/sherpa-onnx-moonshine-base-en-int8', 'en', 1, 0, 1, 'Moonshine Base EN (int8)');
> ```

- [ ] **Step 5: 提交**

```bash
git add crates/infra/src/db.rs crates/asr/src/config.rs crates/asr/src/moonshine.rs crates/asr/src/lib.rs crates/asr/src/engine.rs crates/cli/src/main.rs
git commit -m "feat(asr): 接入 Moonshine ONNX ASR 引擎（v1 格式，4 session 流水线）"
```

---

### Task 5: 单元测试

**Files:**
- Modify: `crates/asr/src/moonshine.rs`（追加 #[cfg(test)] mod tests）

- [ ] **Step 1: 编写真实模型测试**

在 `moonshine.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moonshine_base_real_model() {
        let cfg = config::load_config().expect("load_config failed");
        let entry = match config::pick_entry(&cfg, config::EngineCategory::Moonshine, "moonshine-base-en") {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] moonshine-base-en not in DB — skip real model test");
                return;
            }
        };
        let engine = MoonshineEngine::new(entry).expect("MoonshineEngine::new failed");

        // 用模型自带的 test_wavs
        let model_dir = config::resolve_model_dir(&entry.source).unwrap();
        let test_wav = model_dir.join("test_wavs");
        if !test_wav.exists() {
            eprintln!("[SKIP] no test_wavs dir");
            return;
        }

        let mut any_tested = false;
        for entry_fs in std::fs::read_dir(&test_wav).unwrap() {
            let path = entry_fs.unwrap().path();
            if path.extension().map_or(true, |e| e != "wav") {
                continue;
            }
            let (_sr, samples) = crate::audio::read_wav(&path).expect("read_wav failed");
            let text = engine.transcribe(&samples, "en").expect("transcribe failed");
            println!("[Moonshine] {:?}: {:?}", path.file_name().unwrap(), text);
            assert!(!text.is_empty(), "transcription should not be empty for {:?}", path);
            any_tested = true;
        }
        assert!(any_tested, "should have tested at least one wav");
    }

    #[test]
    fn test_load_tokens() {
        // 测试 tokens.txt 解析逻辑
        let cfg = config::load_config().expect("load_config failed");
        let entry = match config::pick_entry(&cfg, config::EngineCategory::Moonshine, "moonshine-base-en") {
            Some(e) => e,
            None => {
                eprintln!("[SKIP] moonshine-base-en not in DB");
                return;
            }
        };
        let model_dir = config::resolve_model_dir(&entry.source).unwrap();
        let vocab = load_tokens(&model_dir.join("tokens.txt")).expect("load_tokens failed");
        assert_eq!(vocab.len(), 32768);
        assert_eq!(vocab[0], "<unk>");
        assert_eq!(vocab[1], "<s>");
        assert_eq!(vocab[2], "</s>");
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p octopus-asr --release moonshine -- --nocapture`
Expected: test_moonshine_base_real_model 和 test_load_tokens 通过（需 DB 有 moonshine 记录 + HF 缓存有模型文件）

- [ ] **Step 3: 提交**

```bash
git add crates/asr/src/moonshine.rs
git commit -m "test(asr): Moonshine 真实模型单元测试"
```

---

## Self-Review

### Spec coverage
- [x] EngineCategory::Moonshine → Task 2
- [x] AsrSection.moonshine → Task 1
- [x] MoonshineEngine (4 session + decode loop + KV cache) → Task 3
- [x] AsrEngineManager 路由 → Task 4
- [x] CLI 入口 → Task 4
- [x] 验证（真实模型测试）→ Task 5

### Placeholder scan
- Task 3 Step 4 的 ort API 细节（SessionOutputs index 遍历 / try_extract_tensor 返回值）标注了"实际实现时需确认"——这是合理的，ort 2.0-rc API 可能有细节差异，需按编译错误调整
- Task 4 Step 2 的 CLI 入口"具体位置需查看"——已在代码中给出模板，位置根据现有代码确定

### Type consistency
- `MoonshineEngine::new(entry: &config::ModelEntry)` — 与 `WhisperEngine::new` / `ParaformerEngine::new` 签名一致
- `transcribe(&self, samples: &[f32], _language: &str) -> Result<String>` — 与 `OfflineAsrEngine` trait 一致
- `load_tokens(path: &Path) -> Result<Vec<String>>` — 在 Task 3 定义，Task 5 测试中使用
