//! Qwen3-ASR offline inference (conv_frontend → encoder → autoregressive decoder)
//!
//! Model: csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25
//!
//! Pipeline:
//!   1. Mel features (128 bins, Whisper-style log-mel spectrogram)
//!   2. conv_frontend.onnx: [B, T, 128] → [B, T', 896]
//!   3. encoder.int8.onnx: [B, T', 896] + mask → [B, T', 1024]
//!   4. decoder.int8.onnx: autoregressive LLM with KV-cache deltas (28 layers)
//!   5. BPE decode using Qwen2 tokenizer (vocab.json + merges.txt)

use anyhow::{Context, Result};
use ndarray::{Array2, Array3, ArrayView3, ArrayView4};
use once_cell::sync::Lazy;
use ort::session::Session;

use crate::config;

// ── Mel constants (128 bins, Whisper-style) ──
const MEL_FFT_SIZE: usize = 400;
const MEL_FRAME_LEN: usize = 400;
const MEL_FRAME_SHIFT: usize = 160;
const MEL_NUM_BINS: usize = 128;
const MEL_SAMPLE_RATE: u32 = 16000;

// ── Decoder constants ──
const NUM_DECODER_LAYERS: usize = 28;
const NUM_KV_HEADS: usize = 8;
const HEAD_DIM: usize = 128;
const MAX_NEW_TOKENS: usize = 512;
/// 连续相同 token 数阈值：达到即判定 repetition loop，提前终止解码。
/// autoregressive ASR 在噪声/边界音频上易陷入重复（如连续吐几百个「你」）；
/// 正常文本极少连续 N 个相同 token，此阈值安全且能避免跑满 MAX_NEW_TOKENS 拖慢 RTF。
const REPETITION_LIMIT: usize = 8;

// ── Special token IDs (from tokenizer_config.json added_tokens_decoder) ──
const EOS_TOKEN_ID: i64 = 151645; // <|im_end|>

static HANN_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| hann_window(MEL_FRAME_LEN));
static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| mel_filterbank());

// ── Public API ──

/// Thread-safe, reusable engine for Qwen3-ASR model
pub struct Qwen3AsrEngine {
    conv_session: std::sync::Mutex<Session>,
    encoder_session: std::sync::Mutex<Session>,
    decoder_session: std::sync::Mutex<Session>,
    tokenizer: tokenizers::Tokenizer,
    entry: config::ModelEntry,
    cache_names: Vec<(&'static str, &'static str)>,
}

impl Qwen3AsrEngine {
    /// Create a new Qwen3-ASR engine instance by loading models and tokenizer
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;
        let prefer_int8 = true;

        // Discover ONNX files
        let conv_path = discover_onnx(&hf_path, "conv_frontend", prefer_int8)?;
        let encoder_path = discover_onnx(&hf_path, "encoder", prefer_int8)?;
        let decoder_path = discover_onnx(&hf_path, "decoder", prefer_int8)?;

        // Load ONNX sessions
        let conv_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&conv_path)?;
        let encoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&encoder_path)?;
        let decoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&decoder_path)?;

        // Load tokenizer from tokenizer/ subdirectory
        let tokenizer_dir = hf_path.join("tokenizer");
        let tokenizer = load_tokenizer(&tokenizer_dir)?;

        let mut cache_names = Vec::with_capacity(NUM_DECODER_LAYERS);
        for i in 0..NUM_DECODER_LAYERS {
            let key_name: &'static str = Box::leak(format!("cache_key_{}", i).into_boxed_str());
            let value_name: &'static str = Box::leak(format!("cache_value_{}", i).into_boxed_str());
            cache_names.push((key_name, value_name));
        }

        Ok(Self {
            conv_session: std::sync::Mutex::new(conv_session),
            encoder_session: std::sync::Mutex::new(encoder_session),
            decoder_session: std::sync::Mutex::new(decoder_session),
            tokenizer,
            entry: entry.clone(),
            cache_names,
        })
    }
}

impl crate::engine::OfflineAsrEngine for Qwen3AsrEngine {
    fn is_qwen3(&self) -> bool {
        true
    }

    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        // Resolve language：auto 且条目未配具体语言时保持 auto（不限制语言，支持多语言/中英混合）；
        // 原 auto→zh 硬编码会导致中英混合时英文丢失（Qwen3-ASR 最佳实践即不指定 language，自动检测）。
        let lang = if language == "auto" {
            if self.entry.language.is_empty() || self.entry.language == "auto" {
                "auto"
            } else {
                &self.entry.language
            }
        } else {
            language
        };

        // Lock sessions for mutability (Session::run requires mutable borrow)
        let mut conv_session = self.conv_session.lock().unwrap();
        let mut encoder_session = self.encoder_session.lock().unwrap();
        let mut decoder_session = self.decoder_session.lock().unwrap();

        // ── Step 1: Mel features (128 bins) ──
        let mut mel = compute_mel_features(samples)?;
        let (n_frames, mel_dim) = (mel.nrows(), mel.ncols());

        // Whisper-style normalization: per-frame mean/std
        normalize_whisper_features(&mut mel);

        // mel is [n_frames, 128], conv_frontend expects [B, n_frames, 128]
        let (mel_vec, _) = mel.into_raw_vec_and_offset();
        let mel_input = ndarray::Array3::from_shape_vec((1, n_frames, mel_dim), mel_vec)?;

        // ── Step 2: Conv frontend ──
        let conv_outputs = conv_session.run(ort::inputs! {
            "input_features" => ort::value::TensorRef::from_array_view(mel_input.view())?
        })?;

        let (conv_shape, conv_data) = conv_outputs[0].try_extract_tensor::<f32>()?;
        let conv_dims: Vec<usize> = conv_shape.iter().map(|&d| d as usize).collect();

        // conv_output is [1, T', 896]
        let conv_tensor = ArrayView3::from_shape(
            (conv_dims[0], conv_dims[1], conv_dims[2]),
            &*conv_data,
        )?;

        // Build token mask using FeatToAudioTokensLen (matching sherpa-onnx)
        let conv_num_frames = conv_dims[1];
        let expected_audio_tokens = feat_to_audio_tokens_len(n_frames, 100);
        let valid_frames = expected_audio_tokens.min(conv_num_frames);

        let mut mask_vec = vec![false; conv_num_frames];
        for i in 0..valid_frames {
            mask_vec[i] = true;
        }
        let tok_mask = ndarray::Array2::from_shape_vec((1, conv_num_frames), mask_vec)?;

        // ── Step 3: Encoder ──
        let enc_outputs = encoder_session.run(ort::inputs! {
            "input_features" => ort::value::TensorRef::from_array_view(conv_tensor.view())?,
            "feature_attention_mask" => ort::value::TensorRef::from_array_view(tok_mask.view())?
        })?;

        let (enc_shape, enc_data) = enc_outputs[0].try_extract_tensor::<f32>()?;
        let enc_dims: Vec<usize> = enc_shape.iter().map(|&d| d as usize).collect();

        let audio_features_view = ArrayView3::from_shape(
            (enc_dims[0], enc_dims[1], enc_dims[2]),
            &*enc_data,
        )?;

        // Trim trailing silent padding from audio features
        let (audio_features, trimmed_len) = trim_audio_features(audio_features_view);
        // Update audio_token_len to min of valid_frames and trimmed_len (matching sherpa-onnx)
        let audio_token_len = valid_frames.min(trimmed_len);

        // ── Step 4: Build prompt tokens ──
        let input_ids = build_prompt_ids(&self.tokenizer, audio_token_len, lang)?;

        let s0 = input_ids.len();

        // Dynamic max total length
        let max_total_len = 2048.max(s0 + MAX_NEW_TOKENS);

        // ── Step 5: Autoregressive decoder loop ──
        // Initialize KV caches: [1, max_total_len, num_kv_heads, head_dim]
        let mut caches: Vec<ndarray::Array4<f32>> = (0..NUM_DECODER_LAYERS)
            .flat_map(|_| {
                let k = ndarray::Array4::<f32>::zeros((1, max_total_len, NUM_KV_HEADS, HEAD_DIM));
                let v = ndarray::Array4::<f32>::zeros((1, max_total_len, NUM_KV_HEADS, HEAD_DIM));
                vec![k, v]
            })
            .collect();

        // Run prompt through decoder (prefill)
        let prompt_ids_arr = ndarray::Array2::from_shape_vec((1, s0), input_ids)?;
        let prompt_attn = ndarray::Array2::from_shape_vec((1, s0), vec![1i64; s0])?;
        let prompt_cache_pos: ndarray::Array1<i64> = (0..s0 as i64).collect();

        let logit_vec = run_decoder_step(
            &mut *decoder_session,
            &prompt_ids_arr,
            &audio_features,
            &prompt_attn,
            &prompt_cache_pos,
            0,
            &mut caches,
            max_total_len,
            &self.cache_names,
        )?;

        let mut generated_ids = Vec::new();
        let first_id = argmax(&logit_vec);
        generated_ids.push(first_id);

        let mut cur_len = s0;
        let mut active = first_id != EOS_TOKEN_ID;

        // Autoregressive loop
        while generated_ids.len() < MAX_NEW_TOKENS {
            if !active {
                break;
            }

            if cur_len >= max_total_len {
                break;
            }

            let step_ids = ndarray::Array2::from_shape_vec(
                (1, 1),
                vec![generated_ids.last().copied().unwrap_or(EOS_TOKEN_ID)],
            )?;
            let step_attn = ndarray::Array2::from_shape_vec((1, 1), vec![1i64])?;
            let step_pos = ndarray::Array1::from_vec(vec![cur_len as i64]);

            let logit_vec = run_decoder_step(
                &mut *decoder_session,
                &step_ids,
                &audio_features,
                &step_attn,
                &step_pos,
                cur_len,
                &mut caches,
                max_total_len,
                &self.cache_names,
            )?;

            cur_len += 1;

            let next_id = argmax(&logit_vec);
            generated_ids.push(next_id);

            // 重复 token 早停：连续相同 token 达阈值 → repetition loop，提前终止
            // （噪声/边界音频触发，如连续吐几百个「你」；正常文本不会连续 8 个相同 token）
            if generated_ids.len() >= REPETITION_LIMIT
                && generated_ids[generated_ids.len() - REPETITION_LIMIT..]
                    .iter()
                    .all(|&id| id == next_id)
            {
                log::warn!(
                    "qwen3-asr: repetition loop detected (token {} ×{}), stopping early",
                    next_id,
                    REPETITION_LIMIT
                );
                break;
            }

            if next_id == EOS_TOKEN_ID {
                active = false;
            }
        }

        // ── Step 6: Decode tokens to text ──
        let text = decode_tokens(&self.tokenizer, &generated_ids);

        Ok(text)
    }
}

/// Transcribe audio using Qwen3-ASR model
/// Input: 16kHz mono f32 samples. Output: transcribed text.
pub fn transcribe(name: &str, samples: &[f32], language: &str) -> Result<String> {
    let cfg = config::load_config()?;

    // Find qwen3-asr model entry
    let qwen_cfg = cfg
        .asr
        .qwen3_asr
        .as_ref()
        .context("No qwen3-asr models in config")?;
    let entry = qwen_cfg
        .get(name)
        .with_context(|| format!("qwen3-asr model '{}' not in DB", name))?;

    let engine = Qwen3AsrEngine::new(entry)?;
    crate::engine::transcribe_with_vad(&engine, samples, language)
}



// ── Tokenizer helpers ──

/// Load BPE tokenizer from vocab.json + merges.txt with ByteLevel pre-tokenizer/decoder.
/// Uses a padding strategy to align special token IDs with their expected values in the model.
fn load_tokenizer(dir: &std::path::Path) -> Result<tokenizers::Tokenizer> {
    let vocab_path = dir.join("vocab.json");
    let merges_path = dir.join("merges.txt");

    let bpe = tokenizers::models::bpe::BPE::from_file(
        vocab_path.to_string_lossy().as_ref(),
        merges_path.to_string_lossy().as_ref(),
    )
    .build()
    .map_err(|e| anyhow::anyhow!("Failed to build BPE from files: {}", e))?;

    let mut tokenizer = tokenizers::Tokenizer::new(bpe);

    // GPT-2 / Qwen2 uses ByteLevel pre-tokenizer and decoder
    let byte_level = tokenizers::pre_tokenizers::byte_level::ByteLevel::new(false, true, true);
    tokenizer.with_pre_tokenizer(Some(byte_level));
    tokenizer.with_decoder(Some(tokenizers::decoders::byte_level::ByteLevel::new(
        false, true, true,
    )));

    // Align special tokens to their exact expected IDs by adding dummy tokens sequentially.
    let special_tokens_with_target = vec![
        ("<|endoftext|>", 151643),
        ("<|im_start|>", 151644),
        ("<|im_end|>", 151645),
        ("<|audio_start|>", 151669),
        ("<|audio_end|>", 151670),
        ("<|audio_pad|>", 151676),
        ("<asr_text>", 151704),
    ];

    let mut current_id = 151643;
    for (tok, target_id) in special_tokens_with_target {
        while current_id < target_id {
            let dummy = tokenizers::AddedToken::from(format!("<dummy_{}>", current_id), true);
            tokenizer.add_special_tokens(&[dummy]);
            current_id += 1;
        }
        let added = tokenizers::AddedToken::from(tok.to_string(), true);
        tokenizer.add_special_tokens(&[added]);
        current_id += 1;
    }

    Ok(tokenizer)
}

/// Decode generated token IDs to text using the tokenizer, skipping special tokens.
/// Cleans prompt prefix if present.
fn decode_tokens(tokenizer: &tokenizers::Tokenizer, ids: &[i64]) -> String {
    const ASR_TEXT_TOKEN_ID: i64 = 151704;

    let mut cleaned_ids: Vec<u32> = ids.iter().map(|&id| id as u32).collect();
    if !ids.is_empty() {
        let prefix_window = 16.min(ids.len());
        if let Some(pos) = ids[..prefix_window]
            .iter()
            .position(|&id| id == ASR_TEXT_TOKEN_ID)
        {
            if pos > 0 {
                let prefix_ids: Vec<u32> = ids[..=pos].iter().map(|&id| id as u32).collect();
                if let Ok(prefix_text) = tokenizer.decode(&prefix_ids, false) {
                    if prefix_text.starts_with("language ") && prefix_text.ends_with("<asr_text>") {
                        cleaned_ids = ids[pos + 1..].iter().map(|&id| id as u32).collect();
                    }
                }
            }
        }
    }

    tokenizer
        .decode(&cleaned_ids, true)
        .unwrap_or_else(|e| format!("[decode error: {}]", e))
}

// ── ONNX discovery ──

fn discover_onnx(
    base: &std::path::Path,
    name: &str,
    prefer_int8: bool,
) -> Result<std::path::PathBuf> {
    let int8 = base.join(format!("{}.int8.onnx", name));
    let fp32 = base.join(format!("{}.onnx", name));

    if prefer_int8 {
        if int8.exists() {
            Ok(int8)
        } else if fp32.exists() {
            Ok(fp32)
        } else {
            anyhow::bail!(
                "{}.onnx / {}.int8.onnx not found at {}",
                name,
                name,
                base.display()
            )
        }
    } else {
        if fp32.exists() {
            Ok(fp32)
        } else if int8.exists() {
            Ok(int8)
        } else {
            anyhow::bail!(
                "{}.onnx / {}.int8.onnx not found at {}",
                name,
                name,
                base.display()
            )
        }
    }
}

// ── Prompt building ──

/// Build prompt token IDs for Qwen3-ASR.
fn build_prompt_ids(
    tokenizer: &tokenizers::Tokenizer,
    audio_token_len: usize,
    language: &str,
) -> Result<Vec<i64>> {
    const IM_START: i64 = 151644;
    const IM_END: i64 = 151645;
    const AUDIO_START: i64 = 151669;
    const AUDIO_END: i64 = 151670;
    const AUDIO_PAD: i64 = 151676;
    const ASR_TEXT: i64 = 151704;

    let encode_text = |text: &str| -> Result<Vec<i64>> {
        let enc = tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!("Failed to tokenize '{}': {}", text, e))?;
        Ok(enc.get_ids().iter().map(|&id| id as i64).collect())
    };

    let sys_ids = encode_text("system\n")?;
    let nl_ids = encode_text("\n")?;
    let user_ids = encode_text("user\n")?;
    let asst_ids = encode_text("assistant\n")?;

    let mut ids = Vec::new();

    // Before audio: <|im_start|>system\n<|im_end|>\n<|im_start|>user\n<|audio_start|>
    ids.push(IM_START);
    ids.extend_from_slice(&sys_ids);
    ids.push(IM_END);
    ids.extend_from_slice(&nl_ids);
    ids.push(IM_START);
    ids.extend_from_slice(&user_ids);
    ids.push(AUDIO_START);

    // Audio placeholders: <|audio_pad|> × audio_token_len
    for _ in 0..audio_token_len {
        ids.push(AUDIO_PAD);
    }

    // After audio: <|audio_end|><|im_end|>\n<|im_start|>assistant\n
    ids.push(AUDIO_END);
    ids.push(IM_END);
    ids.extend_from_slice(&nl_ids);
    ids.push(IM_START);
    ids.extend_from_slice(&asst_ids);

    // Language prefix：指定具体语言时注入 `language <lang>`；
    // `auto`/空时不注入（模型自动检测语言，支持多语言与中英混合）。
    // <asr_text> 是生成起始标记，始终注入（原实现空字符串时会连带跳过它，是 bug）。
    if !language.is_empty() && language != "auto" {
        let lang_text = format!("language {}", language);
        let lang_ids = encode_text(&lang_text)?;
        ids.extend(lang_ids);
    }
    ids.push(ASR_TEXT);

    Ok(ids)
}

// ── Decoder step ──

/// Run a single decoder step (prefill or generate) and return logits for the last token
fn run_decoder_step(
    decoder: &mut Session,
    input_ids: &ndarray::Array2<i64>,
    audio_features: &ndarray::Array3<f32>,
    attention_mask: &ndarray::Array2<i64>,
    cache_position: &ndarray::Array1<i64>,
    cur_len: usize,
    caches: &mut Vec<ndarray::Array4<f32>>,
    max_total_len: usize,
    cache_names: &[(&'static str, &'static str)],
) -> Result<Vec<f32>> {
    let s = input_ids.shape()[1]; // sequence length of this step

    let mut inputs = ort::inputs! {
        "input_ids" => ort::value::TensorRef::from_array_view(input_ids.view())?,
        "audio_features" => ort::value::TensorRef::from_array_view(audio_features.view())?,
        "attention_mask" => ort::value::TensorRef::from_array_view(attention_mask.view())?,
        "cache_position" => ort::value::TensorRef::from_array_view(cache_position.view())?
    };

    // Add KV cache inputs for all 28 layers
    for i in 0..NUM_DECODER_LAYERS {
        let (key_name, value_name) = cache_names[i];
        inputs.push((
            key_name.into(),
            ort::value::TensorRef::from_array_view(caches[2 * i].view())?.into(),
        ));
        inputs.push((
            value_name.into(),
            ort::value::TensorRef::from_array_view(caches[2 * i + 1].view())?.into(),
        ));
    }

    let outputs = decoder.run(inputs)?;

    // Update caches with KV deltas
    for i in 0..NUM_DECODER_LAYERS {
        let key_out_idx = 1 + 2 * i;
        let val_out_idx = 2 + 2 * i;

        if key_out_idx < outputs.len() && val_out_idx < outputs.len() {
            // key_delta
            if let Ok((kd_shape, kd_data)) = outputs[key_out_idx].try_extract_tensor::<f32>() {
                let kd: Vec<usize> = kd_shape.iter().map(|&d| d as usize).collect();
                if kd.len() == 4 && kd[1] == s {
                    if let Ok(delta) = ArrayView4::from_shape(
                        (kd[0], kd[1], kd[2], kd[3]),
                        &*kd_data,
                    ) {
                        if cur_len + s <= max_total_len {
                            let mut slice = caches[2 * i].slice_mut(ndarray::s![
                                ..,
                                cur_len..cur_len + s,
                                ..,
                                ..
                            ]);
                            slice.assign(&delta);
                        }
                    }
                }
            }

            // value_delta
            if let Ok((vd_shape, vd_data)) = outputs[val_out_idx].try_extract_tensor::<f32>() {
                let vd: Vec<usize> = vd_shape.iter().map(|&d| d as usize).collect();
                if vd.len() == 4 && vd[1] == s {
                    if let Ok(delta) = ArrayView4::from_shape(
                        (vd[0], vd[1], vd[2], vd[3]),
                        &*vd_data,
                    ) {
                        if cur_len + s <= max_total_len {
                            let mut slice = caches[2 * i + 1].slice_mut(ndarray::s![
                                ..,
                                cur_len..cur_len + s,
                                ..,
                                ..
                            ]);
                            slice.assign(&delta);
                        }
                    }
                }
            }
        }
    }

    // Extract logits and return only the last token's logits
    let (logits_shape, logits_data) = outputs[0].try_extract_tensor::<f32>()?;
    let seq_len = logits_shape[1] as usize;
    let vocab_size = logits_shape[2] as usize;
    let last_token_logits = &logits_data[(seq_len - 1) * vocab_size .. seq_len * vocab_size];
    Ok(last_token_logits.to_vec())
}

/// Argmax over a slice of f32 values
fn argmax(values: &[f32]) -> i64 {
    let mut best_idx = 0usize;
    let mut best_val = values[0];
    for (i, &v) in values.iter().enumerate().skip(1) {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx as i64
}

// ── Whisper-style mel normalization ──

/// Normalize mel features per-frame (Whisper-style): subtract mean, divide by std.
/// This matches sherpa-onnx's `NormalizeWhisperFeatures`.
fn normalize_whisper_features(mel: &mut Array2<f32>) {
    if let Some(slice) = mel.as_slice_mut() {
        let mut max_val = f32::NEG_INFINITY;
        for v in slice.iter_mut() {
            let log_v = v.max(1e-10f32).log10();
            if log_v > max_val {
                max_val = log_v;
            }
            *v = log_v;
        }
        let max_v = max_val - 8.0f32;
        for v in slice.iter_mut() {
            *v = (v.max(max_v) + 4.0f32) / 4.0f32;
        }
    } else {
        // Fallback for non-contiguous arrays
        mel.mapv_inplace(|v| v.max(1e-10f32).log10());
        let mut max_val = f32::NEG_INFINITY;
        for &v in mel.iter() {
            if v > max_val {
                max_val = v;
            }
        }
        let max_v = max_val - 8.0f32;
        mel.mapv_inplace(|v| (v.max(max_v) + 4.0f32) / 4.0f32);
    }
}

// ── Audio token length computation ──

/// Compute expected audio token length from mel feature frames.
///
/// The Qwen3-ASR conv frontend processes mel frames in chunks of `chunk_size` (100).
/// Each chunk goes through 3 stride-2 convolutions, yielding 13 audio tokens per full chunk.
/// Remainder frames use a different formula (`aftercnn`).
///
/// Matches sherpa-onnx `FeatToAudioTokensLen`.
fn feat_to_audio_tokens_len(feat_frames: usize, chunk_size: usize) -> usize {
    if feat_frames == 0 || chunk_size == 0 {
        return 0;
    }

    let conv_out_len_3x_stride2 = |n: usize| -> usize {
        let x = (n + 1) / 2;
        let x = (x + 1) / 2;
        (x + 1) / 2
    };

    let aftercnn = |mut x: usize| -> usize {
        if x == 0 {
            return 0;
        }
        x = (x - 1) / 2 + 1;
        x = (x - 1) / 2 + 1;
        (x - 1) / 2 + 1
    };

    let full = feat_frames / chunk_size;
    let rem = feat_frames % chunk_size;
    let tn = conv_out_len_3x_stride2(chunk_size);
    let mut out = full * tn;
    if rem > 0 {
        out += aftercnn(rem);
    }
    out
}

// ── Mel feature extraction (128-bin, Whisper-style) ──

/// Compute 128-bin log-mel spectrogram features from 16kHz f32 samples
fn compute_mel_features(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = (samples.len() + MEL_FRAME_SHIFT / 2) / MEL_FRAME_SHIFT;
    let n_frames = n_frames.max(1);

    let mut planner = rustfft::FftPlanner::new();
    let fft = planner.plan_fft_forward(MEL_FFT_SIZE);

    let n_freqs = MEL_FFT_SIZE / 2 + 1;
    let mut mel_data = vec![0.0f32; n_frames * MEL_NUM_BINS];

    let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); MEL_FFT_SIZE];

    for fi in 0..n_frames {
        let midpoint = MEL_FRAME_SHIFT * fi + MEL_FRAME_SHIFT / 2;
        let wave_start = midpoint as isize - (MEL_FRAME_LEN as isize) / 2;

        for j in 0..MEL_FFT_SIZE {
            let mut s_in_wave = wave_start + j as isize;
            if s_in_wave < 0 || s_in_wave >= samples.len() as isize {
                while s_in_wave < 0 || s_in_wave >= samples.len() as isize {
                    if s_in_wave < 0 {
                        s_in_wave = -s_in_wave - 1;
                    } else {
                        s_in_wave = 2 * samples.len() as isize - 1 - s_in_wave;
                    }
                }
            }
            let s = if j < MEL_FRAME_LEN {
                samples[s_in_wave as usize] * HANN_WINDOW[j]
            } else {
                0.0
            };
            buf[j] = rustfft::num_complex::Complex::new(s, 0.0);
        }
        fft.process(&mut buf);

        // Pre-compute power spectrum to avoid redundant calculations in the filterbank loop
        let mut power_spectrum = [0.0f64; MEL_FFT_SIZE / 2 + 1];
        for k in 0..n_freqs {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

        for mi in 0..MEL_NUM_BINS {
            let mut sum = 0.0f64;
            let fb_row = &MEL_FILTERBANK[mi];
            for k in 0..n_freqs {
                sum += power_spectrum[k] * fb_row[k];
            }
            mel_data[fi * MEL_NUM_BINS + mi] = sum as f32;
        }
    }

    Array2::from_shape_vec((n_frames, MEL_NUM_BINS), mel_data).map_err(Into::into)
}

fn hann_window(size: usize) -> Vec<f32> {
    let a = 2.0 * std::f64::consts::PI / size as f64;
    (0..size)
        .map(|i| (0.5 - 0.5 * (a * i as f64).cos()) as f32)
        .collect()
}

fn hz_to_mel_slaney(freq: f64) -> f64 {
    if freq <= 1000.0 {
        freq * 3.0 / 200.0
    } else {
        15.0 + 14.545078505785561 * (freq / 1000.0).ln()
    }
}

fn mel_to_hz_slaney(mel: f64) -> f64 {
    if mel <= 15.0 {
        200.0 / 3.0 * mel
    } else {
        1000.0 * ((mel - 15.0) * 0.06875177742094911).exp()
    }
}

fn mel_filterbank() -> Vec<Vec<f64>> {
    let num_bins = MEL_NUM_BINS; // 128
    let sample_freq = MEL_SAMPLE_RATE as f64; // 16000
    let window_length_padded = MEL_FFT_SIZE; // 400
    let num_fft_bins = window_length_padded / 2; // 200
    let nyquist = 0.5 * sample_freq; // 8000
    let fft_bin_width = sample_freq / window_length_padded as f64; // 40.0

    let low_freq = 0.0;
    let high_freq = nyquist; // 8000.0

    let mel_low_freq = hz_to_mel_slaney(low_freq);
    let mel_high_freq = hz_to_mel_slaney(high_freq);
    let mel_freq_delta = (mel_high_freq - mel_low_freq) / (num_bins + 1) as f64;

    let mut filters = vec![vec![0.0f64; num_fft_bins + 1]; num_bins];

    for bin in 0..num_bins {
        let left_mel = mel_low_freq + bin as f64 * mel_freq_delta;
        let center_mel = mel_low_freq + (bin + 1) as f64 * mel_freq_delta;
        let right_mel = mel_low_freq + (bin + 2) as f64 * mel_freq_delta;

        let left_hz = mel_to_hz_slaney(left_mel);
        let center_hz = mel_to_hz_slaney(center_mel);
        let right_hz = mel_to_hz_slaney(right_mel);

        for i in 0..=num_fft_bins {
            let hz = fft_bin_width * i as f64;
            if hz > left_hz && hz < right_hz {
                let mut weight = if hz <= center_hz {
                    (hz - left_hz) / (center_hz - left_hz)
                } else {
                    (right_hz - hz) / (right_hz - center_hz)
                };
                // Slaney normalization
                weight *= 2.0 / (right_hz - left_hz);
                filters[bin][i] = weight;
            }
        }
    }
    filters
}

/// Trim trailing silent padding from encoder hidden states.
/// Matches sherpa-onnx TrimAudioFeatures.
fn trim_audio_features(audio_features: ArrayView3<'_, f32>) -> (Array3<f32>, usize) {
    let shape = audio_features.shape();
    let a = shape[1];
    let h = shape[2];

    let mut a_valid = 0;
    let eps = 1e-6f32;

    for idx in (0..a).rev() {
        let mut max_energy = 0.0f32;
        for j in 0..h {
            let v = audio_features[[0, idx, j]].abs();
            if v > max_energy {
                max_energy = v;
            }
        }
        if max_energy > eps {
            a_valid = idx + 1;
            break;
        }
    }

    if a_valid == 0 {
        return (audio_features.to_owned(), 0);
    }
    if a_valid == a {
        return (audio_features.to_owned(), a);
    }

    let sliced = audio_features
        .slice(ndarray::s![.., ..a_valid, ..])
        .to_owned();
    (sliced, a_valid)
}
