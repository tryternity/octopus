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
- Task 3 Step 4 的 ort API 细节（SessionOutputs index 遍历 / try_extract_tensor 返回值）标注了"实际实现时需确认"——已确认并解决，见下方「实施偏差」
- Task 4 Step 2 的 CLI 入口"具体位置需查看"——已在 cli/main.rs 的 `do_transcribe` match 中添加

### 实施偏差（实际实现 vs 计划伪代码）

以下偏差在实现过程中发现并解决，最终代码以实际实现为准：

1. **ort 2.0-rc API**：
   - 计划：`SessionBuilder::new().with_optimization_level(Level3).with_intra_threads(1).with_model_from_file(path)`
   - 实际：`Session::builder()?.commit_from_file(path)` + `apply_session_acceleration(builder)?`
   - 原因：ort 2.0-rc.12 的实际 API 与伪代码不同，需匹配 codebase 已有模式（whisper.rs/paraformer.rs）

2. **Session 需要 Mutex 包裹**：
   - 计划：struct 字段直接用 `Session`
   - 实际：`Mutex<Session>`（`Session::run` 在 ort 2.x 接受 `&mut self`）
   - 与 paraformer.rs:48-49 模式一致

3. **KV cache 数量动态获取**：
   - 计划/spec：36 个（18 层 × K,V）
   - 实际：`num_caches = uncached_out.len() - 1`（base 模型实际 32 个 = 16 层 × K,V）
   - 原因：spec 的层数推断有误；运行时动态获取更健壮，适配不同模型大小

4. **decode_moonshine_tokens 空格处理**：
   - 计划：直接拼接 vocab[id]，无需处理
   - 实际：增加 `▁` (U+2581) → 空格替换 + `trim_start()`
   - 原因：Moonshine tokens.txt 使用 SentencePiece 编码，`▁` 是词首/空格标记

5. **CLI transcribe 使用 VAD 分段**：
   - 计划：`engine.transcribe(samples, language)`
   - 实际：`crate::engine::transcribe_with_vad(&engine, samples, language)`
   - 原因：与 whisper.rs/paraformer.rs CLI 一致，长音频自动 VAD 分段

6. **max_len 公式**：
   - 计划：`audio_seconds * 6 + 10`
   - 实际：`features_len * 384 / 16000 * 6`（无 +10，与 sherpa-onnx 一致）

7. **测试使用 `read_wav_16k`**：
   - 计划：`crate::audio::read_wav(&path)` 返回 `(_sr, samples)`
   - 实际：`crate::audio::read_wav_16k(path_str)` 返回 `Vec<f32>`（实际 API）

### 合并后修复（session 后 follow-up）

Moonshine 5 task 完成并合并后，在测试 whisper 系列模型时发现两个 pre-existing bug，一并修复：

8. **whisper dec_init int8 优先**（`whisper.rs`）：
   - bug：encoder 和 dec_past 都有 int8 优先判断，但 dec_init 硬编码加载 fp32 的 `decoder_model.onnx`（586MB）
   - 修复：dec_init 也优先 `decoder_model_int8.onnx`（149MB）
   - 效果：whisper-small 实际加载 88+149+135 = 372MB（vs 原 88+586+135 = 809MB）

9. **whisper N_DECODER_LAYERS / D_MODEL 动态化**（`whisper.rs`）：
   - bug：`N_DECODER_LAYERS=12` / `D_MODEL=768` / `ENCODER_LEN=1500` 三个常量硬编码，只适配 small（12层）
   - 症状：tiny（4层）/ base（6层）模型 KV cache 提取循环越界 → `out of bounds indexing`
   - 修复：层数从 `dec_init.outputs().len()` 推算 `(n-1)/4`；encoder 输出维度从实际 shape 读取
   - 效果：tiny/base/small 均可加载推理（不再崩溃；识别质量取决于模型容量）

10. **db.sql 新增 moonshine seed**（`infra/db.sql`）：
    - 新增 `moonshine-base-en` + `moonshine-tiny-en` 两条 seed 记录
    - 修复 `init_sql_is_idempotent` / `seed_then_load_round_trips` 两个过时的测试断言（行数 + zipformer 条数）

11. **whisper auto-language-detect 两步式实现**（`whisper.rs`）：
    - bug：`language="auto"` 时跳过语言 token，prompt 变为 `[sot, transcribe, no_ts]`（3个）而非标准 `[sot, lang, transcribe, no_ts]`（4个），positional embedding 错位 → 输出乱码 / EOT
    - 修复：auto 时先喂 `[sot]` 让模型预测语言 token，再拼完整 4-token prompt 跑 dec_init（与 OpenAI whisper 一致）
    - 当前 DB 里 whisper 模型均为 `.en` + `language=en`，config `language=auto` 由 DB 兜底不走 auto-detect；此 bug 在添加多语言 whisper 模型时才暴露

12. **whisper 短音频提早结束机制**（`whisper.rs`，外部 review 发现）：
    - bug：`compute_mel` 把音频 0 填充到固定 30s，若 VAD 只传入 2s 片段，剩余 28s 全是静音；原解码循环硬编码 `max_tokens=448`，只靠 EOT 终止，但 Whisper 在长静音段往往不预测 EOT 反而开始幻听（重复最后一句话 / “谢谢观看”等），既产生转录噪声又把本应秒级结束的短音频拖到完整 448 步，RTF 暴增
    - 修复：按实际音频时长动态计算上限 `max_tokens = (audio_seconds × 6 + 10).min(448)`，.en 模型平均生成 ~6 text tokens/秒，+10 为 prompt/safety 余量，30s 以上恢复 448 上限
    - 验证：6.62s 测试音频 max_tokens 49 步即终止，输出与参考文本完全一致，无幻听无截断
    - 局限：6 tokens/秒 是 .en 模型的经验值；若未来加入多语言 / 中文 whisper，密集中文可达 ~8-10 tokens/秒，届时需调高系数

13. **whisper Mel 频谱 center=True reflect 填充**（`whisper.rs`，外部 review 发现）：
    - bug：OpenAI `log_mel_spectrogram` 调用 `torch.stft(audio, N_FFT, HOP_LENGTH, window, return_complex=True)` 未显式传 `center`，依赖 PyTorch 默认 `center=True, pad_mode="reflect"`——即两端各反射填充 `n_fft/2=200` 采样，使 frame 0 中心对齐 sample 0。原 `compute_mel` 直接从 sample 0 开始加窗（`center=False` 语义），导致整个 Mel 谱时间轴偏移 12.5ms，降低首音节识别准确率
    - 修复：frame t 改为覆盖 `[t×hop - n_fft/2, t×hop + n_fft/2)`，左/右越界样本按 PyTorch `pad_mode="reflect"` 反射（边界样本不参与反射）：左越界 `idx<0 → padded[-idx]`，右越界 `idx>=N → padded[2N - idx - 2]`
    - 验证：6.62s 测试音频输出仍与参考文本完全一致；mel stats 微变（min -0.8487→-0.8476, max 1.1513→1.1524, mean -0.6792→-0.6763）证明特征确实改变；54 个 ASR 测试全部通过
    - 注：sherpa-onnx 使用 Kaldi 风格加窗（`start = t×hop + hop/2 - win/2`），与 librosa/PyTorch center=True 相差 5ms，但两者都比原 `center=False` 实现更接近训练分布；此处采用 OpenAI 官方实现（librosa 风格）

14. **whisper Large v3 / Turbo mel 维度防御性检查**（`whisper.rs`，外部 review 发现）：
    - 现状：`N_MELS=80` 硬编码 + `WHISPER_MEL_FILTERBANK` 是 `[[f64; 201]; 80]` 静态常量；Large v3 / Turbo 使用 128 mel bins，当前引擎无法支持
    - 为何是防御检查而非完整支持：完整支持 128 mel 属于"新功能 / 架构调整"（需 25,728 个 f64 常量 + N_MELS 动态化 + filterbank 重构），按 AGENTS.md 应走完整 superpowers 工作流（brainstorming → spec → plan）；DB seed 仅 whisper-small.en（v2，80 mel），whisper-tiny/base 经实测识别质量不可用（tiny 3/3 全空、base 1/3 可用）故不入 seed，HF 缓存无 Large v3/Turbo——非 active bug
    - 防御：`WhisperEngine::new` 加载 encoder 后读取其 mel 输入 shape（`[batch, n_mels, n_audio_ctx]`），若 `dims[1] != 80` 立即 fail 给出明确错误消息（"仅支持 v1/v2，Large v3/Turbo 用 128 mel，请用 whisper-small"），避免后续 `encoder.run()` 踩 ONNX shape mismatch 崩溃
    - 验证：whisper-small 加载/转录正常通过（80 mel 不触发检查）；54 个 ASR 测试全部通过

15. **whisper 特殊 token 查询改强制 fail**（`whisper.rs`，外部 review 发现）：
    - 现状：`unwrap_or(50XXX)` fallback 值取自 multilingual 模型，但各 Whisper 变体的特殊 token ID 不同（.en 模型整体偏移 -1：`.en` sot=50257/transcribe=50358/no_ts=50362/eot=50256；multilingual sot=50258/transcribe=50359/no_ts=50363/eot=50257）。若 tokenizer 查询失败，静默 fallback 会注入错误 ID（对 .en 是错的）导致模型行为失控且极难排查
    - 核实：当前 3 个 .en 模型的 tokenizer 都包含这些 special tokens，`token_to_id()` 实际返回 `Some(正确ID)`，unwrap_or 分支从未被触发——**非 active bug，是潜在隐患**。但审计方向成立：fallback 值确实不适用于 .en 词表
    - 修复：改为 `ok_or_else(bail!)` 强制查询——若 tokenizer 缺少任一特殊 token 立即报错（"tokenizer 缺少 <|xxx|> token"），让真实问题暴露而非静默腐烂
    - 验证：whisper-small/base/tiny 均正常加载并转录；6.62s 测试音频输出仍与参考文本完全一致；54 个 ASR 测试全部通过

16. **whisper encoder/dec_init 互斥锁生命周期优化**（`whisper.rs`，外部 review 发现）：
    - 现状：`encoder` 和 `dec_init` 的 MutexGuard 绑定为函数级局部变量，会一直持有锁到 `transcribe` 函数结束——包括漫长的 `dec_past` 自回归循环（~0.26s），期间并发线程无法使用 encoder/dec_init
    - 数据所有权核实：`encoder.run()` 输出通过 `to_vec()` 深拷贝到 owned `Array3 encoder_hidden`；`dec_init.run()` 输出通过 `extract_kv` 的 `to_vec()` 深拷贝到 owned `ArrayD kv`——两者提取后 session 不再被引用，锁可以安全释放
    - 修复：encoder 用 `{}` 块限定 guard 生命周期；dec_init 在 kv 提取后显式 `drop(init_out)` + `drop(dec_init)`（需先 drop init_out 因 SessionOutputs 借用 dec_init）
    - 效果（并发场景）：线程 A 跑 decode 循环时，线程 B 可并行跑 encoder forward（0.43s，占 63%），实现流水线并发
    - 验证：54 个 ASR 测试全部通过；whisper-small 转录输出不变

### Type consistency
- `MoonshineEngine::new(entry: &config::ModelEntry)` — 与 `WhisperEngine::new` / `ParaformerEngine::new` 签名一致
- `transcribe(&self, samples: &[f32], _language: &str) -> Result<String>` — 与 `OfflineAsrEngine` trait 一致
- `load_tokens(path: &Path) -> Result<Vec<String>>` — 在 Task 3 定义，Task 5 测试中使用
