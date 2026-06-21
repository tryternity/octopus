use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::TensorRef;

use crate::config;

/// Moonshine KV cache tensor 数量（18 层 × K,V）。
const NUM_KV_CACHES: usize = 36;

/// Moonshine ASR 引擎 — 纯 ONNX 体系，4 session 流水线。
///
/// 模型来自 `csukuangfj/sherpa-onnx-moonshine-{base,tiny}-en-int8`（v1 格式）。
/// 推理流程：preprocess → encode → uncached_decode（首 token，初始化 KV cache）
///           → cached_decode 循环（后续 token，复用 KV cache）→ EOS 停止。
///
/// Decode 循环逻辑参考 sherpa-onnx `offline-moonshine-greedy-search-decoder.cc`：
/// BOS(1) → uncached_decode → logits + 36 cache
/// 循环: argmax → EOS(2) 则停 → cached_decode(token, cache) → logits + 新 cache
pub struct MoonshineEngine {
    preprocess_session: Mutex<Session>,
    encode_session: Mutex<Session>,
    uncached_decode_session: Mutex<Session>,
    cached_decode_session: Mutex<Session>,
    vocab: Vec<String>,
}

impl MoonshineEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let model_dir = config::resolve_model_dir(&entry.source)
            .context("解析 Moonshine 模型目录失败")?;

        let preprocess_path = model_dir.join("preprocess.onnx");
        let encode_path = model_dir.join("encode.int8.onnx");
        let uncached_path = model_dir.join("uncached_decode.int8.onnx");
        let cached_path = model_dir.join("cached_decode.int8.onnx");

        for (name, p) in [
            ("preprocess", &preprocess_path),
            ("encode", &encode_path),
            ("uncached_decode", &uncached_path),
            ("cached_decode", &cached_path),
        ] {
            if !p.exists() {
                anyhow::bail!("Moonshine {} 未找到: {}", name, p.display());
            }
        }

        let make_session = |path: &std::path::Path| -> Result<Session> {
            Ok(config::apply_session_acceleration(Session::builder()?)?
                .commit_from_file(path)?)
        };

        let vocab = load_tokens(&model_dir.join("tokens.txt"))?;
        if vocab.len() != 32768 {
            anyhow::bail!(
                "Moonshine vocab 大小不匹配: 期望 32768, 实际 {}",
                vocab.len()
            );
        }

        Ok(Self {
            preprocess_session: Mutex::new(make_session(&preprocess_path)?),
            encode_session: Mutex::new(make_session(&encode_path)?),
            uncached_decode_session: Mutex::new(make_session(&uncached_path)?),
            cached_decode_session: Mutex::new(make_session(&cached_path)?),
            vocab,
        })
    }

    /// 运行 preprocess：audio (1, N) → features (T, 416)。
    fn run_preprocess(&self, samples: &[f32]) -> Result<ndarray::Array2<f32>> {
        let audio = ndarray::ArrayView2::from_shape((1, samples.len()), samples)?;
        let mut session = self.preprocess_session.lock().unwrap();
        let outputs = session.run(ort::inputs! {
            "args_0" => TensorRef::from_array_view(audio)?
        })?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        // 输出 (1, T, 416) → 去掉 batch 维 (T, 416) 便于 encode 复用
        ndarray::Array2::from_shape_vec((dims[1], dims[2]), data.to_vec())
            .context("preprocess 输出 reshape 失败")
    }

    /// 运行 encode：features (1, T, 416) + len → encoder_out (1, T, 416)。
    fn run_encode(
        &self,
        features: &ndarray::Array2<f32>,
        features_len: usize,
    ) -> Result<ndarray::Array3<f32>> {
        let features_3d = features.view().insert_axis(ndarray::Axis(0));
        let len_arr = [features_len as i32];
        let len_view = ndarray::ArrayView1::from(&len_arr);
        let mut session = self.encode_session.lock().unwrap();
        let outputs = session.run(ort::inputs! {
            "args_0" => TensorRef::from_array_view(features_3d)?,
            "args_1" => TensorRef::from_array_view(len_view)?
        })?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        ndarray::Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())
            .context("encode 输出 reshape 失败")
    }

    /// Greedy decode 循环。参考 sherpa-onnx `offline-moonshine-greedy-search-decoder.cc`。
    fn greedy_decode(
        &self,
        encoder_out: &ndarray::Array3<f32>,
        features_len: i32,
    ) -> Result<Vec<i64>> {
        const BOS: i32 = 1;
        const EOS: i64 = 2;
        // 与 sherpa-onnx 一致：encoder_frames * 384 / 16000 * 6
        let max_len = (features_len as f32 * 384.0 / 16000.0 * 6.0) as usize;

        let enc_view = encoder_out.view();

        // ── 首 token（BOS）: uncached_decode ──
        let token = [BOS];
        let token_view = ndarray::ArrayView2::from_shape((1, 1), &token)?;
        let seq_len = [1i32];
        let seq_len_view = ndarray::ArrayView1::from(&seq_len);

        let mut uncached_session = self.uncached_decode_session.lock().unwrap();
        let uncached_out = uncached_session.run(ort::inputs! {
            "args_0" => TensorRef::from_array_view(token_view)?,
            "args_1" => TensorRef::from_array_view(enc_view)?,
            "args_2" => TensorRef::from_array_view(seq_len_view)?
        })?;

        // logits (index 0) + 36 KV caches (index 1..=36)
        let (logits_shape, logits_data) = uncached_out[0].try_extract_tensor::<f32>()?;
        let vocab_size = logits_shape[2] as usize;
        let mut last_logits: Vec<f32> = logits_data.to_vec();

        let mut state_shapes: Vec<Vec<usize>> = Vec::with_capacity(NUM_KV_CACHES);
        let mut state_data: Vec<Vec<f32>> = Vec::with_capacity(NUM_KV_CACHES);
        for i in 1..=NUM_KV_CACHES {
            let (shape, data) = uncached_out[i].try_extract_tensor::<f32>()?;
            state_shapes.push(shape.iter().map(|&d| d as usize).collect());
            state_data.push(data.to_vec());
        }

        // ── 后续 tokens: cached_decode 循环 ──
        let mut result_tokens: Vec<i64> = Vec::new();
        let mut seq_len_val: i32 = 1;
        let mut cached_session = self.cached_decode_session.lock().unwrap();

        for _ in 0..max_len {
            // argmax over last_logits[..vocab_size]
            let next_token = last_logits[..vocab_size]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i as i64)
                .unwrap_or(EOS);

            if next_token == EOS {
                break;
            }
            result_tokens.push(next_token);
            seq_len_val += 1;

            let token = [next_token as i32];
            let token_view = ndarray::ArrayView2::from_shape((1, 1), &token)?;
            let seq_len = [seq_len_val];
            let seq_len_view = ndarray::ArrayView1::from(&seq_len);

            let mut inputs = ort::inputs! {
                "args_0" => TensorRef::from_array_view(token_view)?,
                "args_1" => TensorRef::from_array_view(enc_view)?,
                "args_2" => TensorRef::from_array_view(seq_len_view)?
            };
            for (i, (shape, data)) in state_shapes.iter().zip(state_data.iter()).enumerate() {
                let view = ndarray::ArrayViewD::from_shape(shape.as_slice(), data.as_slice())?;
                inputs.push((
                    format!("args_{}", i + 3).into(),
                    TensorRef::from_array_view(view)?.into(),
                ));
            }

            let cached_out = cached_session.run(inputs)?;

            // 更新 logits + states
            let (_, new_logits) = cached_out[0].try_extract_tensor::<f32>()?;
            last_logits = new_logits.to_vec();

            for i in 0..NUM_KV_CACHES {
                let (shape, data) = cached_out[i + 1].try_extract_tensor::<f32>()?;
                state_shapes[i] = shape.iter().map(|&d| d as usize).collect();
                state_data[i] = data.to_vec();
            }
        }

        Ok(result_tokens)
    }
}

impl crate::engine::OfflineAsrEngine for MoonshineEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let features = self.run_preprocess(samples)?;
        let features_len = features.nrows();
        if features_len == 0 {
            return Ok(String::new());
        }
        let encoder_out = self.run_encode(&features, features_len)?;
        let token_ids = self.greedy_decode(&encoder_out, features_len as i32)?;
        Ok(decode_moonshine_tokens(&token_ids, &self.vocab))
    }
}

/// 加载 tokens.txt：每行 "token_text\ttoken_id"，按 id 索引构建 vocab。
fn load_tokens(path: &std::path::Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取 tokens.txt 失败: {}", path.display()))?;
    let mut vocab: HashMap<i64, String> = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.rsplitn(2, '\t').collect();
        if parts.len() == 2 {
            let token_id: i64 = parts[0]
                .parse()
                .with_context(|| format!("tokens.txt 无效 token id: {}", parts[0]))?;
            vocab.insert(token_id, parts[1].to_string());
        }
    }
    let max_id = vocab.keys().copied().max().unwrap_or(-1);
    let mut result = vec![String::new(); (max_id + 1) as usize];
    for (id, text) in vocab {
        result[id as usize] = text;
    }
    Ok(result)
}

/// Moonshine byte-level BPE 解码：直接拼接 vocab[token_id]。
/// （BPE merge 在 ONNX 模型内部完成，输出的 token_id 已是最终文本 token。）
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

/// CLI 顶层 transcribe 入口。
pub fn transcribe(name: &str, samples: &[f32], language: &str) -> Result<String> {
    let cfg = config::load_config()?;
    let entry = config::pick_entry(&cfg, config::EngineCategory::Moonshine, name)
        .with_context(|| format!("Moonshine 模型 '{}' 未在配置中找到", name))?;
    let engine = MoonshineEngine::new(entry)?;
    crate::engine::transcribe_with_vad(&engine, samples, language)
}
