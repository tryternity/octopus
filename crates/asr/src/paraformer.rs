use anyhow::{Context, Result};
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

static HAMMING_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| hamming_window(FBANK_FRAME_LEN));
static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| mel_filterbank_fbank());

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
    encoder_output_size: usize,
    vocab: Vec<String>,
}

impl ParaformerEngine {
    /// Create a new Paraformer engine instance by loading models and vocab
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;
        let prefer_int8 = entry.quantization != "fp32";

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
        let (neg_mean, inv_stddev, encoder_output_size) = extract_cmvn_from_metadata(&encoder_session)?;

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
            encoder_output_size,
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
        let scale = (self.encoder_output_size as f32).sqrt();
        for i in 0..n_frames {
            for j in 0..feat_dim {
                if j < self.neg_mean.len() && j < self.inv_stddev.len() {
                    features[[i, j]] = (features[[i, j]] + self.neg_mean[j]) * self.inv_stddev[j] * scale;
                }
            }
        }

        // ── Encoder ──
        let speech_vec = {
            let (v, _) = features.into_raw_vec_and_offset();
            v
        };
        let speech_tensor = ndarray::Array3::from_shape_vec((1, n_frames, feat_dim), speech_vec)?;
        let speech_lengths = ndarray::Array1::from_vec(vec![n_frames as i32]);

        let enc_outputs = encoder_session.run(ort::inputs! {
            "speech" => ort::value::TensorRef::from_array_view(speech_tensor.view())?,
            "speech_lengths" => ort::value::TensorRef::from_array_view(speech_lengths.view())?
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

        let enc_data = {
            let (v, _) = enc_tensor.clone().into_raw_vec_and_offset();
            v
        };

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
        let acoustic_len = ndarray::Array1::from_vec(vec![num_tokens as i32]);
        let enc_len_for_dec = ndarray::Array1::from_vec(vec![enc_len_scalar as i32]);

        let mut cache_inputs = ort::inputs! {
            "enc" => ort::value::TensorRef::from_array_view(enc_tensor.view())?,
            "enc_len" => ort::value::TensorRef::from_array_view(enc_len_for_dec.view())?,
            "acoustic_embeds" => ort::value::TensorRef::from_array_view(acoustic_tensor.view())?,
            "acoustic_embeds_len" => ort::value::TensorRef::from_array_view(acoustic_len.view())?
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

/// Decode token IDs into text, handling @@ BPE continuation markers
pub(crate) fn decode_tokens(sample_ids: &[i64], vocab: &[String]) -> String {
    let mut text_parts: Vec<String> = Vec::new();
    let mut i = 0;

    while i < sample_ids.len() {
        let tid = sample_ids[i];

        // Skip special tokens
        if tid == TOKEN_BLANK || tid == TOKEN_SOS || tid == TOKEN_EOS {
            i += 1;
            continue;
        }

        let idx = tid as usize;
        if idx == 0 || idx >= vocab.len() || vocab[idx].is_empty() {
            i += 1;
            continue;
        }

        let token = &vocab[idx];
        if token.ends_with("@@") {
            // BPE continuation: accumulate until non-@@ token
            let mut merged = String::new();
            merged.push_str(&token[..token.len() - 2]);
            i += 1;
            while i < sample_ids.len() {
                let next_tid = sample_ids[i];
                if next_tid == TOKEN_BLANK || next_tid == TOKEN_SOS || next_tid == TOKEN_EOS {
                    i += 1;
                    continue;
                }
                let next_idx = next_tid as usize;
                if next_idx == 0 || next_idx >= vocab.len() || vocab[next_idx].is_empty() {
                    i += 1;
                    continue;
                }
                let next_token = &vocab[next_idx];
                if next_token.ends_with("@@") {
                    merged.push_str(&next_token[..next_token.len() - 2]);
                } else {
                    merged.push_str(next_token);
                    i += 1;
                    break;
                }
                i += 1;
            }
            text_parts.push(merged);
        } else {
            text_parts.push(token.clone());
            i += 1;
        }
    }

    text_parts.join("")
}

// ── Fbank feature extraction (same as SenseVoice) ──

pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
    let fbank = compute_fbank(&scaled)?;
    let lfr = apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);
    Ok(lfr)
}

pub(crate) fn compute_fbank(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = if samples.len() >= FBANK_FRAME_LEN {
        (samples.len() - FBANK_FRAME_LEN) / FBANK_FRAME_SHIFT + 1
    } else {
        1
    };

    let mut planner = rustfft::FftPlanner::new();
    let fft = planner.plan_fft_forward(FBANK_FFT_SIZE);

    let n_freqs = FBANK_FFT_SIZE / 2 + 1;
    let mut fbank_data = vec![0.0f32; n_frames * FBANK_NUM_BINS];

    let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); FBANK_FFT_SIZE];

    for fi in 0..n_frames {
        let start = fi * FBANK_FRAME_SHIFT;

        for j in 0..FBANK_FFT_SIZE {
            let s = if start + j < samples.len() && j < FBANK_FRAME_LEN {
                samples[start + j] * HAMMING_WINDOW[j]
            } else {
                0.0
            };
            buf[j] = rustfft::num_complex::Complex::new(s, 0.0);
        }
        fft.process(&mut buf);

        // Pre-compute power spectrum to avoid redundant calculations in the filterbank loop
        let mut power_spectrum = [0.0f64; FBANK_FFT_SIZE / 2 + 1];
        for k in 0..n_freqs {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

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

fn hamming_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos())
        .collect()
}

fn mel_filterbank_fbank() -> Vec<Vec<f64>> {
    let n_freqs = FBANK_FFT_SIZE / 2 + 1;
    let fmax = FBANK_SAMPLE_RATE as f64 / 2.0;
    let mel_min = hz_to_mel(20.0);
    let mel_max = hz_to_mel(fmax);

    let hz_points: Vec<f64> = (0..=FBANK_NUM_BINS + 1)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (FBANK_NUM_BINS + 1) as f64))
        .collect();

    let fft_freqs: Vec<f64> = (0..n_freqs)
        .map(|i| FBANK_SAMPLE_RATE as f64 * i as f64 / FBANK_FFT_SIZE as f64)
        .collect();

    let mut filters = vec![vec![0.0f64; n_freqs]; FBANK_NUM_BINS];
    for i in 0..FBANK_NUM_BINS {
        let (fl, fc, fr) = (hz_points[i], hz_points[i + 1], hz_points[i + 2]);
        for j in 0..n_freqs {
            if fft_freqs[j] >= fl && fft_freqs[j] <= fc && fc > fl {
                filters[i][j] = (fft_freqs[j] - fl) / (fc - fl);
            } else if fft_freqs[j] > fc && fft_freqs[j] <= fr && fr > fc {
                filters[i][j] = (fr - fft_freqs[j]) / (fr - fc);
            }
        }
    }
    filters
}

fn hz_to_mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (mel / 1127.0).exp() - 700.0
}
