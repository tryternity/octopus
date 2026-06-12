use anyhow::{Context, Result};
use ndarray::{Array2, Array3, ArrayD, IxDyn};
use ort::session::Session;
use std::path::PathBuf;
use tokenizers::Tokenizer;

use crate::config;

// ── Whisper constants ──
const FFT_SIZE: usize = 400;
const HOP_LENGTH: usize = 160;
const N_MELS: usize = 80;
const SAMPLE_RATE: u32 = 16000;
const N_SAMPLES: usize = 30 * SAMPLE_RATE as usize;
const N_DECODER_LAYERS: usize = 12;
const ENCODER_LEN: usize = 1500;
const D_MODEL: usize = 768;

// ── Model discovery via config ──
fn find_whisper_onnx_dir() -> Result<PathBuf> {
    let cfg = config::load_config()?;
    let whisper_cfg = cfg
        .asr
        .whisper
        .as_ref()
        .context("No whisper models in config")?;
    let (_, entry) = whisper_cfg
        .iter()
        .next()
        .context("No whisper model entries")?;
    let hf_path = config::find_hf_cache(&entry.source)?;
    Ok(config::find_onnx_dir(&hf_path))
}

fn find_tokenizer() -> Result<PathBuf> {
    let cfg = config::load_config()?;
    let whisper_cfg = cfg
        .asr
        .whisper
        .as_ref()
        .context("No whisper models in config")?;
    let (_, entry) = whisper_cfg
        .iter()
        .next()
        .context("No whisper model entries")?;
    let hf_path = config::find_hf_cache(&entry.source)?;
    let tk = hf_path.join("tokenizer.json");
    if tk.exists() {
        return Ok(tk);
    }
    let tk2 = hf_path.parent().unwrap_or(&hf_path).join("tokenizer.json");
    if tk2.exists() {
        return Ok(tk2);
    }
    anyhow::bail!("tokenizer.json not found in {}", hf_path.display())
}

// ── Mel spectrogram ──
fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos()))
        .collect()
}

fn mel_filterbank() -> Vec<Vec<f64>> {
    let n_freqs = FFT_SIZE / 2 + 1;
    let fmax = SAMPLE_RATE as f64 / 2.0;
    let mel_min = 2595.0f64 * (1.0f64).log10();
    let mel_max = 2595.0 * (1.0 + fmax / 700.0).log10();
    let hz_points: Vec<f64> = (0..=N_MELS + 1)
        .map(|i| {
            700.0
                * (10.0f64.powf(
                    (mel_min + (mel_max - mel_min) * i as f64 / (N_MELS + 1) as f64) / 2595.0,
                ) - 1.0)
        })
        .collect();
    let fft_freqs: Vec<f64> = (0..n_freqs)
        .map(|i| SAMPLE_RATE as f64 * i as f64 / FFT_SIZE as f64)
        .collect();
    let mut filters = vec![vec![0.0f64; n_freqs]; N_MELS];
    for i in 0..N_MELS {
        let (fl, _fc, fr) = (hz_points[i], hz_points[i + 1], hz_points[i + 2]);
        let fc = hz_points[i + 1];
        for j in 0..n_freqs {
            if fft_freqs[j] >= fl && fft_freqs[j] <= fc && fc > fl {
                filters[i][j] = (fft_freqs[j] - fl) / (fc - fl);
            } else if fft_freqs[j] > fc && fft_freqs[j] <= fr && fr > fc {
                filters[i][j] = (fr - fft_freqs[j]) / (fr - fc);
            }
        }
        let enorm = 2.0 / (hz_points[i + 2] - hz_points[i]);
        for j in 0..n_freqs {
            filters[i][j] *= enorm;
        }
    }
    filters
}

fn compute_mel(audio: &[f32]) -> Result<Array3<f32>> {
    let mut padded = vec![0.0f32; N_SAMPLES];
    let copy_len = audio.len().min(N_SAMPLES);
    padded[..copy_len].copy_from_slice(&audio[..copy_len]);
    let window = hann_window(FFT_SIZE);
    let n_frames = N_SAMPLES / HOP_LENGTH;
    let mel_fb = mel_filterbank();
    let mut planner = rustfft::FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut mel_data = vec![0.0f32; N_MELS * n_frames];
    for fi in 0..n_frames {
        let start = fi * HOP_LENGTH;
        let mut buf: Vec<rustfft::num_complex::Complex<f32>> = (0..FFT_SIZE)
            .map(|j| {
                let s = if start + j < N_SAMPLES {
                    padded[start + j]
                } else {
                    0.0
                };
                rustfft::num_complex::Complex::new(s * window[j], 0.0)
            })
            .collect();
        fft.process(&mut buf);
        for mi in 0..N_MELS {
            let mut sum = 0.0f64;
            for (k, c) in buf[..FFT_SIZE / 2 + 1].iter().enumerate() {
                sum += (c.re as f64 * c.re as f64 + c.im as f64 * c.im as f64) * mel_fb[mi][k];
            }
            mel_data[mi * n_frames + fi] = sum as f32;
        }
    }

    // Whisper normalization: log10, clamp, shift+scale
    let max_val = mel_data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    for v in mel_data.iter_mut() {
        *v = (*v + 1e-10).log10();
        *v = v.max(max_val.log10() - 8.0);
        *v = (*v + 4.0) / 4.0;
    }

    Array3::from_shape_vec((1, N_MELS, n_frames), mel_data).map_err(Into::into)
}

// ── KV Cache ──
struct KvCache {
    decoder_keys: Vec<ArrayD<f32>>,
    decoder_values: Vec<ArrayD<f32>>,
    encoder_keys: Vec<ArrayD<f32>>,
    encoder_values: Vec<ArrayD<f32>>,
}

impl KvCache {
    fn extract_kv(
        dec_out: &ort::session::SessionOutputs,
        layer: usize,
    ) -> Result<(ArrayD<f32>, ArrayD<f32>, ArrayD<f32>, ArrayD<f32>)> {
        let dk_name = format!("present.{}.decoder.key", layer);
        let dv_name = format!("present.{}.decoder.value", layer);
        let ek_name = format!("present.{}.encoder.key", layer);
        let ev_name = format!("present.{}.encoder.value", layer);
        let (shape, data) = dec_out[dk_name.as_str()].try_extract_tensor::<f32>()?;
        let dk = ArrayD::from_shape_vec(
            IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
            data.to_vec(),
        )?;
        let (shape, data) = dec_out[dv_name.as_str()].try_extract_tensor::<f32>()?;
        let dv = ArrayD::from_shape_vec(
            IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
            data.to_vec(),
        )?;
        let (shape, data) = dec_out[ek_name.as_str()].try_extract_tensor::<f32>()?;
        let ek = ArrayD::from_shape_vec(
            IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
            data.to_vec(),
        )?;
        let (shape, data) = dec_out[ev_name.as_str()].try_extract_tensor::<f32>()?;
        let ev = ArrayD::from_shape_vec(
            IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
            data.to_vec(),
        )?;
        Ok((dk, dv, ek, ev))
    }

    fn new_from_decoder_output(dec_out: &ort::session::SessionOutputs) -> Result<Self> {
        let mut dk = Vec::with_capacity(N_DECODER_LAYERS);
        let mut dv = Vec::with_capacity(N_DECODER_LAYERS);
        let mut ek = Vec::with_capacity(N_DECODER_LAYERS);
        let mut ev = Vec::with_capacity(N_DECODER_LAYERS);
        for layer in 0..N_DECODER_LAYERS {
            let (d_k, d_v, e_k, e_v) = Self::extract_kv(dec_out, layer)?;
            dk.push(d_k);
            dv.push(d_v);
            ek.push(e_k);
            ev.push(e_v);
        }
        Ok(Self {
            decoder_keys: dk,
            decoder_values: dv,
            encoder_keys: ek,
            encoder_values: ev,
        })
    }

    fn update_decoder_kv(&mut self, dec_out: &ort::session::SessionOutputs) -> Result<()> {
        for layer in 0..N_DECODER_LAYERS {
            let dk_name = format!("present.{}.decoder.key", layer);
            let dv_name = format!("present.{}.decoder.value", layer);
            let (shape, data) = dec_out[dk_name.as_str()].try_extract_tensor::<f32>()?;
            self.decoder_keys[layer] = ArrayD::from_shape_vec(
                IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
                data.to_vec(),
            )?;
            let (shape, data) = dec_out[dv_name.as_str()].try_extract_tensor::<f32>()?;
            self.decoder_values[layer] = ArrayD::from_shape_vec(
                IxDyn(&shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
                data.to_vec(),
            )?;
        }
        Ok(())
    }
}

fn argmax_last_token(logits_data: &[f32], n_tokens: usize, vocab: usize) -> u32 {
    let offset = (n_tokens - 1) * vocab;
    let mut best = 0u32;
    let mut best_score = f32::NEG_INFINITY;
    for (i, &s) in logits_data[offset..offset + vocab].iter().enumerate() {
        if s > best_score {
            best_score = s;
            best = i as u32;
        }
    }
    best
}

// ── Public API ──

/// Transcribe audio using Whisper model
/// Input: 16kHz mono f32 samples, language code ("auto"/"zh"/"en"/...). Output: transcribed text.
pub fn transcribe(audio: &[f32], language: &str) -> Result<String> {
    let onnx_dir = find_whisper_onnx_dir()?;

    // Encoder
    let encoder_path = onnx_dir.join(if onnx_dir.join("encoder_model_int8.onnx").exists() {
        "encoder_model_int8.onnx"
    } else {
        "encoder_model.onnx"
    });
    let mut encoder = Session::builder()?.commit_from_file(&encoder_path)?;

    // Mel spectrogram
    let mel = compute_mel(audio)?;
    eprintln!("[whisper] mel shape: {:?}", mel.shape());
    {
        let mel_slice = mel.as_slice().unwrap();
        let mel_max = mel_slice.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mel_min = mel_slice.iter().cloned().fold(f32::INFINITY, f32::min);
        let mel_mean: f32 = mel_slice.iter().sum::<f32>() / mel_slice.len() as f32;
        eprintln!(
            "[whisper] mel stats: min={:.4} max={:.4} mean={:.4}",
            mel_min, mel_max, mel_mean
        );
    }
    let mel_tensor = ort::value::TensorRef::from_array_view(mel.view())?;

    // Encoder forward
    let enc_out = encoder.run(ort::inputs![mel_tensor])?;
    let (_s, enc_data) = enc_out[0].try_extract_tensor::<f32>()?;
    let encoder_hidden = Array3::from_shape_vec((1, ENCODER_LEN, D_MODEL), enc_data.to_vec())?;

    // Two decoders
    let dec_init_path = onnx_dir.join("decoder_model.onnx");
    let dec_past_path = onnx_dir.join(
        if onnx_dir.join("decoder_with_past_model_int8.onnx").exists() {
            "decoder_with_past_model_int8.onnx"
        } else {
            "decoder_with_past_model.onnx"
        },
    );
    let mut dec_init = Session::builder()?.commit_from_file(&dec_init_path)?;
    let mut dec_past = Session::builder()?.commit_from_file(&dec_past_path)?;

    // Tokenizer & special tokens
    let tokenizer =
        Tokenizer::from_file(find_tokenizer()?).map_err(|e| anyhow::anyhow!("Tokenizer: {}", e))?;
    let sot: u32 = tokenizer
        .token_to_id("<|startoftranscript|>")
        .unwrap_or(50258);
    let transcribe: u32 = tokenizer.token_to_id("<|transcribe|>").unwrap_or(50359);
    let no_ts: u32 = tokenizer.token_to_id("<|notimestamps|>").unwrap_or(50363);
    let eot: u32 = tokenizer.token_to_id("<|endoftext|>").unwrap_or(50257);

    // Build prompt tokens: <|SOT|> [<|LANG|>] <|transcribe|> <|notimestamps|>
    let mut tokens: Vec<i64> = vec![sot as i64];
    let lang_code = if language.is_empty() {
        "auto".into()
    } else {
        language.to_string()
    };
    eprintln!("[whisper] language: {}", lang_code);
    if lang_code != "auto" {
        let lang_tag = format!("<|{}|>", lang_code);
        if let Some(lang_id) = tokenizer.token_to_id(&lang_tag) {
            tokens.push(lang_id as i64);
        }
    }
    tokens.extend_from_slice(&[transcribe as i64, no_ts as i64]);
    let prompt_len = tokens.len();
    eprintln!("[whisper] prompt tokens: {:?}", tokens);

    // Step 0: initial decoder (no past KV)
    let input_ids = Array2::from_shape_vec((1, tokens.len()), tokens.clone())?;
    let init_out = dec_init.run(ort::inputs! {
        "input_ids" => ort::value::TensorRef::from_array_view(input_ids.view())?,
        "encoder_hidden_states" => ort::value::TensorRef::from_array_view(encoder_hidden.view())?
    })?;

    let (logits_shape, logits_data) = init_out["logits"].try_extract_tensor::<f32>()?;
    let vocab = logits_shape[2] as usize;
    let next_token = argmax_last_token(logits_data, tokens.len(), vocab);
    eprintln!("[whisper] first token: {} (eot={})", next_token, eot);

    if next_token == eot {
        eprintln!("[whisper] EOT on first token, returning empty");
        return Ok(String::new());
    }
    tokens.push(next_token as i64);

    let mut kv = KvCache::new_from_decoder_output(&init_out)?;

    // Autoregressive loop
    let max_tokens = 448;
    for _step in 1..max_tokens {
        let last_id = Array2::from_shape_vec((1, 1), vec![*tokens.last().unwrap()])?;

        let mut inputs = ort::inputs! {
            "input_ids" => ort::value::TensorRef::from_array_view(last_id.view())?
        };

        for layer in 0..N_DECODER_LAYERS {
            inputs.push((
                format!("past_key_values.{}.decoder.key", layer).into(),
                ort::value::TensorRef::from_array_view(kv.decoder_keys[layer].view())?.into(),
            ));
            inputs.push((
                format!("past_key_values.{}.decoder.value", layer).into(),
                ort::value::TensorRef::from_array_view(kv.decoder_values[layer].view())?.into(),
            ));
            inputs.push((
                format!("past_key_values.{}.encoder.key", layer).into(),
                ort::value::TensorRef::from_array_view(kv.encoder_keys[layer].view())?.into(),
            ));
            inputs.push((
                format!("past_key_values.{}.encoder.value", layer).into(),
                ort::value::TensorRef::from_array_view(kv.encoder_values[layer].view())?.into(),
            ));
        }

        let dec_out = dec_past.run(inputs)?;

        let (logits_shape, logits_data) = dec_out["logits"].try_extract_tensor::<f32>()?;
        let next_token = if logits_shape[1] == 1 {
            let mut best = 0u32;
            let mut best_s = f32::NEG_INFINITY;
            for (i, &s) in logits_data.iter().enumerate() {
                if s > best_s {
                    best_s = s;
                    best = i as u32;
                }
            }
            best
        } else {
            argmax_last_token(
                logits_data,
                logits_shape[1] as usize,
                logits_shape[2] as usize,
            )
        };

        kv.update_decoder_kv(&dec_out)?;

        if next_token == eot {
            break;
        }
        tokens.push(next_token as i64);
    }

    let text_ids: Vec<u32> = tokens[prompt_len..].iter().map(|&t| t as u32).collect();
    let text = tokenizer
        .decode(&text_ids, true)
        .map_err(|e| anyhow::anyhow!("Decode: {}", e))?;
    Ok(text)
}
