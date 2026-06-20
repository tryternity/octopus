use anyhow::{Context, Result};
use std::sync::Arc;

use ndarray::Array2;
use once_cell::sync::Lazy;
use ort::session::Session;

use crate::config;

// ── Fbank constants (identical to SenseVoice / Paraformer) ──
pub(crate) const FBANK_FFT_SIZE: usize = 512;
pub(crate) const FBANK_FRAME_LEN: usize = 400;
pub(crate) const FBANK_FRAME_SHIFT: usize = 160;
pub(crate) const FBANK_NUM_BINS: usize = 80;
pub(crate) const FBANK_SAMPLE_RATE: u32 = 16000;

// ── LFR (Low Frame Rate) stacking ──
pub(crate) const LFR_WINDOW_SIZE: usize = 7;
pub(crate) const LFR_WINDOW_SHIFT: usize = 6;

// 窗口函数：流式 Paraformer 使用 povey window (hanning^0.85)，离线使用 hamming
pub(crate) static POVEY_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| povey_window(FBANK_FRAME_LEN));
static HAMMING_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| hamming_window(FBANK_FRAME_LEN));
pub(crate) static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| mel_filterbank_fbank());
// 预规划的 512 点正向 FFT — 所有 fbank 提取共用，避免每次特征计算重复规划（堆分配 + twiddle 计算）。
// 对流式热路径（StreamingParaformer）尤为关键：每个 chunk 都会调用。
pub(crate) static FBANK_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>> = Lazy::new(|| {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    planner.plan_fft_forward(FBANK_FFT_SIZE)
});

// ── Decoder cache ──
const NUM_CACHE_LAYERS: usize = 16;
const CACHE_CHANNELS: usize = 512;
const CACHE_TIME: usize = 10;

// ── Special token IDs ──
const TOKEN_BLANK: i64 = 0;
const TOKEN_SOS: i64 = 1;
const TOKEN_EOS: i64 = 2;

// ── Public API ──

// ── Public API ──

/// Thread-safe, reusable engine for Paraformer model
pub struct ParaformerEngine {
    encoder_session: std::sync::Mutex<Session>,
    decoder_session: std::sync::Mutex<Session>,
    neg_mean: Vec<f32>,
    inv_stddev: Vec<f32>,
    vocab: Vec<String>,
}

impl ParaformerEngine {
    /// Create a new Paraformer engine instance by loading models and vocab
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;
        let prefer_int8 = true;

        // Discover encoder ONNX
        let encoder_path = if prefer_int8 {
            if hf_path.join("encoder.int8.onnx").exists() {
                hf_path.join("encoder.int8.onnx")
            } else if hf_path.join("encoder.onnx").exists() {
                hf_path.join("encoder.onnx")
            } else {
                anyhow::bail!("encoder.onnx / encoder.int8.onnx not found at {}", hf_path.display());
            }
        } else {
            if hf_path.join("encoder.onnx").exists() {
                hf_path.join("encoder.onnx")
            } else if hf_path.join("encoder.int8.onnx").exists() {
                hf_path.join("encoder.int8.onnx")
            } else {
                anyhow::bail!("encoder.onnx / encoder.int8.onnx not found at {}", hf_path.display());
            }
        };

        // Discover decoder ONNX
        let decoder_path = if prefer_int8 {
            if hf_path.join("decoder.int8.onnx").exists() {
                hf_path.join("decoder.int8.onnx")
            } else if hf_path.join("decoder.onnx").exists() {
                hf_path.join("decoder.onnx")
            } else {
                anyhow::bail!("decoder.onnx / decoder.int8.onnx not found at {}", hf_path.display());
            }
        } else {
            if hf_path.join("decoder.onnx").exists() {
                hf_path.join("decoder.onnx")
            } else if hf_path.join("decoder.int8.onnx").exists() {
                hf_path.join("decoder.int8.onnx")
            } else {
                anyhow::bail!("decoder.onnx / decoder.int8.onnx not found at {}", hf_path.display());
            }
        };

        let encoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&encoder_path)?;
        let decoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&decoder_path)?;

        // Read CMVN normalization from encoder metadata
        let (neg_mean, inv_stddev, _encoder_output_size) = extract_cmvn_from_metadata(&encoder_session)?;

        // Token decoding
        let tokens_path = hf_path.join("tokens.txt");
        let tokens_text = std::fs::read_to_string(&tokens_path)
            .with_context(|| format!("tokens.txt not found at {}", tokens_path.display()))?;

        let mut vocab = Vec::new();
        for line in tokens_text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((token, id_str)) = line.rsplit_once(' ') {
                if let Ok(id) = id_str.parse::<usize>() {
                    while vocab.len() <= id {
                        vocab.push(String::new());
                    }
                    vocab[id] = token.to_string();
                }
            }
        }

        Ok(Self {
            encoder_session: std::sync::Mutex::new(encoder_session),
            decoder_session: std::sync::Mutex::new(decoder_session),
            neg_mean,
            inv_stddev,
            vocab,
        })
    }
}

impl crate::engine::OfflineAsrEngine for ParaformerEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        let mut encoder_session = self.encoder_session.lock().unwrap();
        let mut decoder_session = self.decoder_session.lock().unwrap();

        // ── Feature extraction (fbank + LFR, same as SenseVoice) ──
        let mut features = compute_fbank_features(samples)?;
        let (n_frames, feat_dim) = (features.nrows(), features.ncols());

        // ── Apply CMVN normalization ──
        // inv_stddev 已在 extract_cmvn_from_metadata 中乘过 scale = sqrt(enc_output_size)，
        // 此处不再重复乘（此前重复乘导致特征放大 ~22.6 倍 → 乱码）
        for i in 0..n_frames {
            for j in 0..feat_dim {
                if j < self.neg_mean.len() && j < self.inv_stddev.len() {
                    features[[i, j]] = (features[[i, j]] + self.neg_mean[j]) * self.inv_stddev[j];
                }
            }
        }

        // ── Encoder ──
        let speech_vec = {
            let (v, _) = features.into_raw_vec_and_offset();
            v
        };
        let speech_tensor = ndarray::Array3::from_shape_vec((1, n_frames, feat_dim), speech_vec)?;
        let speech_lengths_data = [n_frames as i32];
        let speech_lengths = ndarray::ArrayView1::from(&speech_lengths_data);

        let enc_outputs = encoder_session.run(ort::inputs! {
            "speech" => ort::value::TensorRef::from_array_view(speech_tensor.view())?,
            "speech_lengths" => ort::value::TensorRef::from_array_view(speech_lengths)?
        })?;

        // Encoder outputs: enc [1, T', 512], enc_len [1], alphas [1, T']
        let (enc_shape, enc_data) = enc_outputs[0].try_extract_tensor::<f32>()?;
        let enc_dim: Vec<usize> = enc_shape.iter().map(|&d| d as usize).collect();
        if enc_dim.len() != 3 {
            anyhow::bail!("Unexpected encoder output rank: {:?}", enc_dim);
        }
        let enc_len_val = enc_dim[1];
        let enc_feat = enc_dim[2];

        let enc_tensor =
            ndarray::Array3::from_shape_vec((1, enc_len_val, enc_feat), enc_data.to_vec())?;

        // enc_len from encoder output (i32)
        let (_, enc_len_data) = enc_outputs[1].try_extract_tensor::<i32>()?;
        let enc_len_scalar = enc_len_data[0] as usize;

        let (_, alpha_data) = enc_outputs[2].try_extract_tensor::<f32>()?;
        let alphas: Vec<f32> = alpha_data.to_vec();

        // 零拷贝：直接借用 enc_tensor 的连续切片供 CIF 循环读取，避免 clone() 整段 encoder 输出。
        // 形状 [1, enc_len_val, enc_feat] 为标准行主序，slice(s![0, ..enc_len_scalar, ..]) 连续。
        let enc_slice = enc_tensor.slice(ndarray::s![0, ..enc_len_scalar, ..]);
        let enc_data: &[f32] = enc_slice.as_slice().unwrap();

        let mut acoustic_embedding: Vec<f32> = Vec::new();
        let mut initial_hidden: Vec<f32> = vec![0.0; enc_feat];
        let mut integrate: f32 = 0.0;
        let threshold: f32 = 1.0;

        for i in 0..enc_len_scalar {
            let this_alpha = alphas[i];
            if integrate + this_alpha < threshold {
                integrate += this_alpha;
                // accumulate weighted encoder output
                let enc_row = &enc_data[i * enc_feat..(i + 1) * enc_feat];
                for j in 0..enc_feat {
                    initial_hidden[j] += enc_row[j] * this_alpha;
                }
                continue;
            }

            // fire — threshold reached
            let remaining = threshold - integrate;
            let enc_row = &enc_data[i * enc_feat..(i + 1) * enc_feat];
            for j in 0..enc_feat {
                initial_hidden[j] += enc_row[j] * remaining;
            }
            acoustic_embedding.extend_from_slice(&initial_hidden);

            // start new integration with the remainder
            integrate += this_alpha - threshold;
            for j in 0..enc_feat {
                initial_hidden[j] = enc_row[j] * integrate;
            }
        }

        let num_tokens = acoustic_embedding.len() / enc_feat;

        if num_tokens == 0 {
            return Ok(String::new());
        }

        // ── Decoder ──
        let acoustic_tensor =
            ndarray::Array3::from_shape_vec((1, num_tokens, enc_feat), acoustic_embedding)?;
        let acoustic_len_data = [num_tokens as i32];
        let enc_len_for_dec_data = [enc_len_scalar as i32];
        let acoustic_len = ndarray::ArrayView1::from(&acoustic_len_data);
        let enc_len_for_dec = ndarray::ArrayView1::from(&enc_len_for_dec_data);

        let mut cache_inputs = ort::inputs! {
            "enc" => ort::value::TensorRef::from_array_view(enc_tensor.view())?,
            "enc_len" => ort::value::TensorRef::from_array_view(enc_len_for_dec)?,
            "acoustic_embeds" => ort::value::TensorRef::from_array_view(acoustic_tensor.view())?,
            "acoustic_embeds_len" => ort::value::TensorRef::from_array_view(acoustic_len)?
        };

        let caches: Vec<ndarray::Array3<f32>> = (0..NUM_CACHE_LAYERS)
            .map(|_| ndarray::Array3::<f32>::zeros((1, CACHE_CHANNELS, CACHE_TIME)))
            .collect();

        for i in 0..NUM_CACHE_LAYERS {
            cache_inputs.push((
                format!("in_cache_{}", i).into(),
                ort::value::TensorRef::from_array_view(caches[i].view())?.into(),
            ));
        }

        let dec_outputs = decoder_session.run(cache_inputs)?;

        // Extract sample_ids from output index 1
        let (_, ids_data) = dec_outputs[1].try_extract_tensor::<i64>()?;
        let all_sample_ids: Vec<i64> = ids_data.to_vec();

        let text = decode_tokens(&all_sample_ids, &self.vocab);
        Ok(text)
    }
}

/// Transcribe audio using Paraformer model (offline mode)
/// Input: 16kHz mono f32 samples. Output: transcribed text.
pub fn transcribe(name: &str, samples: &[f32], language: &str) -> Result<String> {
    let cfg = config::load_config()?;
    let para_cfg = cfg
        .asr
        .paraformer
        .as_ref()
        .context("No paraformer models in config")?;
    let entry = para_cfg
        .get(name)
        .with_context(|| format!("paraformer model '{}' not in DB", name))?;

    let engine = ParaformerEngine::new(entry)?;
    crate::engine::transcribe_with_vad(&engine, samples, language)
}

/// Extract CMVN normalization parameters from encoder ONNX model metadata.
/// Returns (neg_mean, inv_stddev, encoder_output_size).
/// The inv_stddev values are pre-scaled by sqrt(encoder_output_size).
pub(crate) fn extract_cmvn_from_metadata(session: &Session) -> Result<(Vec<f32>, Vec<f32>, usize)> {
    let metadata = session.metadata()?;

    let encoder_output_size: usize = metadata
        .custom("encoder_output_size")
        .ok_or_else(|| anyhow::anyhow!("Missing encoder_output_size metadata"))?
        .parse()?;

    let neg_mean_str = metadata
        .custom("neg_mean")
        .ok_or_else(|| anyhow::anyhow!("Missing neg_mean metadata"))?;
    let neg_mean: Vec<f32> = neg_mean_str
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()?;

    let inv_stddev_str = metadata
        .custom("inv_stddev")
        .ok_or_else(|| anyhow::anyhow!("Missing inv_stddev metadata"))?;
    let mut inv_stddev: Vec<f32> = inv_stddev_str
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()?;

    // Scale inv_stddev by sqrt(encoder_output_size), matching sherpa-onnx convention
    let scale = (encoder_output_size as f32).sqrt();
    for v in inv_stddev.iter_mut() {
        *v *= scale;
    }

    eprintln!(
        "[paraformer] CMVN: {} means, {} stddevs, enc_out={}",
        neg_mean.len(),
        inv_stddev.len(),
        encoder_output_size
    );

    Ok((neg_mean, inv_stddev, encoder_output_size))
}

/// Append `new` to `existing` with intelligent spacing at the boundary.
/// Used when concatenating decoded text from different streaming chunks.
/// - ASCII ↔ ASCII: add space
/// - Chinese ↔ ASCII or ASCII ↔ Chinese: add space
/// - Chinese ↔ Chinese: no space
pub(crate) fn smart_append(existing: &mut String, new: &str) {
    if new.is_empty() {
        return;
    }
    if existing.is_empty() {
        existing.push_str(new);
        return;
    }
    let last_byte = existing.as_bytes().last().copied().unwrap_or(0);
    let first_byte = new.as_bytes().first().copied().unwrap_or(0);
    let last_is_ascii = last_byte < 0x80;
    let first_is_ascii = first_byte < 0x80;
    // Add space if either side is ASCII (word boundary), except both Chinese
    // or either side already being a space (defensive against double spacing)
    if (last_is_ascii || first_is_ascii) && last_byte != 0x20 && first_byte != 0x20 {
        existing.push(' ');
    }
    existing.push_str(new);
}

/// Decode token IDs into text, with sherpa-onnx compatible spacing logic.
///
/// Spacing rules (matching sherpa-onnx Convert):
/// - `@@` suffix → BPE subword continuation, strip `@@` and merge without space
/// - ASCII token (not @@) → prepend space unless merging from previous subword
/// - Non-ASCII token (Chinese etc.) → no space; but prepend space if previous token was ASCII
pub(crate) fn decode_tokens(sample_ids: &[i64], vocab: &[String]) -> String {
    // Collect effective (token_id, token_string) pairs, skipping specials/blanks
    let tokens: Vec<&str> = sample_ids
        .iter()
        .filter_map(|&tid| {
            if tid == TOKEN_BLANK || tid == TOKEN_SOS || tid == TOKEN_EOS {
                return None;
            }
            let idx = tid as usize;
            if idx == 0 || idx >= vocab.len() || vocab[idx].is_empty() {
                return None;
            }
            Some(vocab[idx].as_str())
        })
        .collect();

    let mut text = String::new();
    let mut mergeable = false;

    for (i, token) in tokens.iter().enumerate() {
        let ends_with_at = token.ends_with("@@");

        if !ends_with_at {
            // Token does NOT end with @@ — it's a complete word/character
            let first_byte = token.as_bytes().first().copied().unwrap_or(0);
            if first_byte < 0x80 {
                // ASCII — prepend space unless merging from subword
                if mergeable {
                    mergeable = false;
                    text.push_str(token);
                } else {
                    text.push(' ');
                    text.push_str(token);
                }
            } else {
                // Non-ASCII (Chinese, Japanese, etc.)
                mergeable = false;
                if i > 0 {
                    // Prepend space if previous token was ASCII
                    let prev = tokens[i - 1];
                    let prev_byte = prev.as_bytes().first().copied().unwrap_or(0);
                    if prev_byte < 0x80 && !prev.ends_with("@@") {
                        text.push(' ');
                    }
                }
                text.push_str(token);
            }
        } else {
            // Token ends with @@ — BPE subword continuation
            let stem = &token[..token.len() - 2]; // strip "@@"
            if mergeable {
                // Continue merging
                text.push_str(stem);
            } else {
                // Start new subword chain
                text.push(' ');
                text.push_str(stem);
                mergeable = true;
            }
        }
    }

    text.trim().to_string()
}

// ── Fbank feature extraction (same as SenseVoice) ──

/// 离线 Paraformer fbank 特征提取（hamming window）
pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
    let fbank = compute_fbank(&scaled, &HAMMING_WINDOW, 0.97)?;
    let lfr = apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);
    Ok(lfr)
}

/// Fbank 特征提取（DC offset removal + pre-emphasis + windowing → FFT → mel → log）
///
/// `window` — povey window（流式）或 hamming window（离线）
/// `preemph_coeff` — 预加重系数，paraformer 用 0.97
///
/// Pre-emphasis 无需跨帧状态：帧重叠（shift=160 < len=400），上一帧末尾并非
/// 本帧 start-1，故直接从连续 `samples` 回溯 start-1 取准确前序样本。
pub(crate) fn compute_fbank(
    samples: &[f32],
    window: &[f32],
    preemph_coeff: f32,
) -> Result<Array2<f32>> {
    let n_frames = if samples.len() >= FBANK_FRAME_LEN {
        (samples.len() - FBANK_FRAME_LEN) / FBANK_FRAME_SHIFT + 1
    } else {
        1
    };

    let fft = &*FBANK_FFT;

    let n_freqs = FBANK_FFT_SIZE / 2 + 1;
    let mut fbank_data = vec![0.0f32; n_frames * FBANK_NUM_BINS];
    let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); FBANK_FFT_SIZE];

    let mut frame_buf = [0.0f32; FBANK_FRAME_LEN];

    for fi in 0..n_frames {
        let start = fi * FBANK_FRAME_SHIFT;

        // 1. 提取帧样本
        for j in 0..FBANK_FRAME_LEN {
            frame_buf[j] = if start + j < samples.len() {
                samples[start + j]
            } else {
                0.0
            };
        }

        // 2. DC offset removal（去直流）
        let mean: f32 = frame_buf.iter().sum::<f32>() / FBANK_FRAME_LEN as f32;
        for s in frame_buf.iter_mut() {
            *s -= mean;
        }

        // 3. Pre-emphasis（预加重）: y[i] = x[i] - preemph_coeff * x[i-1]
        //    帧重叠（shift=160 < len=400），上一帧末尾并非本帧 start-1。
        //    直接从连续缓冲回溯 start-1 取准确前序样本，无需跨帧状态。
        //    samples[start-1] 未去直流，减去本帧 mean 作近似（knf 行为）。
        let mut prev = if start > 0 {
            samples[start - 1] - mean
        } else {
            0.0
        };
        for i in 0..FBANK_FRAME_LEN {
            let cur = frame_buf[i];
            frame_buf[i] = cur - preemph_coeff * prev;
            prev = cur;
        }

        // 4. 加窗 + FFT
        for j in 0..FBANK_FFT_SIZE {
            let s = if j < FBANK_FRAME_LEN {
                frame_buf[j] * window[j]
            } else {
                0.0
            };
            buf[j] = rustfft::num_complex::Complex::new(s, 0.0);
        }
        fft.process(&mut buf);

        // 5. 功率谱
        let mut power_spectrum = [0.0f64; FBANK_FFT_SIZE / 2 + 1];
        for k in 0..n_freqs {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

        // 6. Mel 滤波器组 + log
        for mi in 0..FBANK_NUM_BINS {
            let mut sum = 0.0f64;
            let fb_row = &MEL_FILTERBANK[mi];
            for k in 0..n_freqs {
                sum += power_spectrum[k] * fb_row[k];
            }
            fbank_data[fi * FBANK_NUM_BINS + mi] = (sum as f32 + 1e-10).ln();
        }
    }

    Array2::from_shape_vec((n_frames, FBANK_NUM_BINS), fbank_data).map_err(Into::into)
}

pub(crate) fn apply_lfr(fbank: &Array2<f32>, window_size: usize, window_shift: usize) -> Array2<f32> {
    let (n_frames, feat_dim) = (fbank.nrows(), fbank.ncols());
    let n_lfr = if n_frames >= window_size {
        (n_frames - window_size) / window_shift + 1
    } else {
        1
    };
    let out_dim = feat_dim * window_size;

    let mut out = Array2::zeros((n_lfr, out_dim));
    for i in 0..n_lfr {
        let base = i * window_shift;
        for w in 0..window_size {
            let frame_idx = base + w;
            if frame_idx < n_frames {
                for d in 0..feat_dim {
                    out[[i, w * feat_dim + d]] = fbank[[frame_idx, d]];
                }
            }
        }
    }
    out
}

/// Povey window: (0.5 - 0.5*cos(2*pi*i/(N-1)))^0.85
/// knf feature-window.cc GetWindow() — 流式 Paraformer 默认使用此窗口
fn povey_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| {
            let a = 2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32;
            (0.5 - 0.5 * a.cos()).powf(0.85)
        })
        .collect()
}

fn hamming_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos())
        .collect()
}

fn mel_filterbank_fbank() -> Vec<Vec<f64>> {
    let n_freqs = FBANK_FFT_SIZE / 2 + 1;
    // sherpa-onnx 默认 high_freq = -400（即 Nyquist - 400 = 7600 Hz）
    let high_freq_param = -400.0f64;
    let fmax = if high_freq_param > 0.0 {
        high_freq_param
    } else {
        FBANK_SAMPLE_RATE as f64 / 2.0 + high_freq_param // 7600.0
    };
    let mel_low = hz_to_mel(20.0);
    let mel_high = hz_to_mel(fmax);

    // knf/kaldi 的 mel 滤波器在 **mel 空间** 均匀分布 (num_bins+2) 个点，
    // 权重斜率也在 mel 空间计算。此前在 Hz 空间计算权重导致 fbank 输出完全不同。
    let mel_delta = (mel_high - mel_low) / (FBANK_NUM_BINS as f64 + 1.0);
    let fft_bin_width = FBANK_SAMPLE_RATE as f64 / FBANK_FFT_SIZE as f64;

    let mut filters = vec![vec![0.0f64; n_freqs]; FBANK_NUM_BINS];
    for bin in 0..FBANK_NUM_BINS {
        let left_mel = mel_low + bin as f64 * mel_delta;
        let center_mel = mel_low + (bin as f64 + 1.0) * mel_delta;
        let right_mel = mel_low + (bin as f64 + 2.0) * mel_delta;

        for j in 0..n_freqs {
            let freq = fft_bin_width * j as f64;
            let mel = hz_to_mel(freq);
            if mel > left_mel && mel < right_mel {
                if mel <= center_mel {
                    filters[bin][j] = (mel - left_mel) / (center_mel - left_mel);
                } else {
                    filters[bin][j] = (right_mel - mel) / (right_mel - center_mel);
                }
            }
        }
    }
    filters
}

fn hz_to_mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}
