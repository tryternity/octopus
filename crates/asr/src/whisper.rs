use anyhow::{Context, Result};
use ndarray::{Array2, Array3, ArrayD, IxDyn};
use once_cell::sync::Lazy;
use ort::session::Session;
use tokenizers::Tokenizer;

use crate::config;

// ── Whisper constants ──
const FFT_SIZE: usize = 400;
const HOP_LENGTH: usize = 160;
const N_MELS: usize = 80;
const SAMPLE_RATE: u32 = 16000;
const N_SAMPLES: usize = 30 * SAMPLE_RATE as usize;


static HANN_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| hann_window(FFT_SIZE));

// ── Mel spectrogram ──
fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos()))
        .collect()
}

fn compute_mel(audio: &[f32]) -> Result<Array3<f32>> {
    let mut padded = vec![0.0f32; N_SAMPLES];
    let copy_len = audio.len().min(N_SAMPLES);
    padded[..copy_len].copy_from_slice(&audio[..copy_len]);
    let n_frames = N_SAMPLES / HOP_LENGTH;
    let mut planner = rustfft::FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut mel_data = vec![0.0f32; N_MELS * n_frames];

    let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); FFT_SIZE];
    let n_freqs = FFT_SIZE / 2 + 1;

    // center=True reflect padding：frame t 覆盖 [t*hop - n_fft/2, t*hop + n_fft/2)
    // 与 OpenAI whisper.audio.log_mel_spectrogram 调用的 torch.stft 默认行为一致
    // （torch.stft 默认 center=True, pad_mode="reflect"，
    //  对音频两端各反射填充 n_fft/2=200 采样，使 frame 0 中心对齐 sample 0）
    let pad = FFT_SIZE / 2; // 200
    for fi in 0..n_frames {
        let start = (fi * HOP_LENGTH) as isize - pad as isize;
        for j in 0..FFT_SIZE {
            let idx = start + j as isize;
            // 反射填充（与 PyTorch pad_mode="reflect" 一致，边界样本不参与反射）：
            //   左越界 idx<0         → padded[-idx]              (sample -1→1, -2→2, ..., -200→200)
            //   右越界 idx>=N_SAMPLES → padded[2N - idx - 2]      (sample N→N-2, N+1→N-3, ...)
            let s = if idx < 0 {
                padded[(-idx) as usize]
            } else if (idx as usize) < N_SAMPLES {
                padded[idx as usize]
            } else {
                let over = idx as usize - N_SAMPLES;
                if N_SAMPLES >= over + 2 {
                    padded[N_SAMPLES - over - 2]
                } else {
                    0.0
                }
            };
            buf[j] = rustfft::num_complex::Complex::new(s * HANN_WINDOW[j], 0.0);
        }
        fft.process(&mut buf);

        // Pre-compute power spectrum to reduce redundant math in the filterbank loop
        let mut power_spectrum = [0.0f64; FFT_SIZE / 2 + 1];
        for k in 0..n_freqs {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

        for mi in 0..N_MELS {
            let mut sum = 0.0f64;
            let fb_row = &crate::whisper_mel_matrix::WHISPER_MEL_FILTERBANK[mi];
            for k in 0..n_freqs {
                sum += power_spectrum[k] * fb_row[k];
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

    fn new_from_decoder_output(dec_out: &ort::session::SessionOutputs, n_layers: usize) -> Result<Self> {
        let mut dk = Vec::with_capacity(n_layers);
        let mut dv = Vec::with_capacity(n_layers);
        let mut ek = Vec::with_capacity(n_layers);
        let mut ev = Vec::with_capacity(n_layers);
        for layer in 0..n_layers {
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

    fn update_decoder_kv(&mut self, dec_out: &ort::session::SessionOutputs, n_layers: usize) -> Result<()> {
        for layer in 0..n_layers {
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

/// Thread-safe, reusable engine for Whisper model
pub struct WhisperEngine {
    encoder: std::sync::Mutex<Session>,
    dec_init: std::sync::Mutex<Session>,
    dec_past: std::sync::Mutex<Session>,
    tokenizer: Tokenizer,
    past_key_names: Vec<(&'static str, &'static str, &'static str, &'static str)>,
    n_decoder_layers: usize,
    entry_language: String,
}

impl WhisperEngine {
    /// Create a new Whisper engine instance by loading models and tokenizer
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;
        let onnx_dir = config::find_onnx_dir(&hf_path);

        // Encoder
        let encoder_path = onnx_dir.join(if onnx_dir.join("encoder_model_int8.onnx").exists() {
            "encoder_model_int8.onnx"
        } else {
            "encoder_model.onnx"
        });
        let encoder = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&encoder_path)?;

        // Decoders（优先 int8 量化版本，与 encoder 一致）
        let dec_init_path = onnx_dir.join(if onnx_dir.join("decoder_model_int8.onnx").exists() {
            "decoder_model_int8.onnx"
        } else {
            "decoder_model.onnx"
        });
        let dec_past_path = onnx_dir.join(
            if onnx_dir.join("decoder_with_past_model_int8.onnx").exists() {
                "decoder_with_past_model_int8.onnx"
            } else {
                "decoder_with_past_model.onnx"
            },
        );
        let dec_init = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&dec_init_path)?;
        let dec_past = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&dec_past_path)?;

        // 从 dec_init session 输出数量推算 decoder 层数：
        // 输出 = 1(logits) + 4*n_layers(decoder_key/value + encoder_key/value)
        let n_decoder_layers = (dec_init.outputs().len() - 1) / 4;

        // Tokenizer
        let tk_path = hf_path.join("tokenizer.json");
        let tk_path = if tk_path.exists() {
            tk_path
        } else {
            let tk2 = hf_path.parent().unwrap_or(&hf_path).join("tokenizer.json");
            if tk2.exists() {
                tk2
            } else {
                anyhow::bail!("tokenizer.json not found in {}", hf_path.display())
            }
        };
        let tokenizer = Tokenizer::from_file(tk_path).map_err(|e| anyhow::anyhow!("Tokenizer: {}", e))?;

        let mut past_key_names = Vec::with_capacity(n_decoder_layers);
        for layer in 0..n_decoder_layers {
            let dk_name: &'static str = Box::leak(format!("past_key_values.{}.decoder.key", layer).into_boxed_str());
            let dv_name: &'static str = Box::leak(format!("past_key_values.{}.decoder.value", layer).into_boxed_str());
            let ek_name: &'static str = Box::leak(format!("past_key_values.{}.encoder.key", layer).into_boxed_str());
            let ev_name: &'static str = Box::leak(format!("past_key_values.{}.encoder.value", layer).into_boxed_str());
            past_key_names.push((dk_name, dv_name, ek_name, ev_name));
        }

        Ok(Self {
            encoder: std::sync::Mutex::new(encoder),
            dec_init: std::sync::Mutex::new(dec_init),
            dec_past: std::sync::Mutex::new(dec_past),
            tokenizer,
            past_key_names,
            n_decoder_layers,
            entry_language: entry.language.clone(),
        })
    }
}

impl crate::engine::OfflineAsrEngine for WhisperEngine {
    fn transcribe(&self, audio: &[f32], language: &str) -> Result<String> {
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
        let mut encoder = self.encoder.lock().unwrap();
        let enc_out = encoder.run(ort::inputs![mel_tensor])?;
        let (enc_shape, enc_data) = enc_out[0].try_extract_tensor::<f32>()?;
        let enc_dim: Vec<usize> = enc_shape.iter().map(|&d| d as usize).collect();
        let encoder_hidden = Array3::from_shape_vec(
            (enc_dim[0], enc_dim[1], enc_dim[2]),
            enc_data.to_vec(),
        )?;

        // Tokenizer & special tokens
        let sot: u32 = self.tokenizer
            .token_to_id("<|startoftranscript|>")
            .unwrap_or(50258);
        let transcribe_tok: u32 = self.tokenizer.token_to_id("<|transcribe|>").unwrap_or(50359);
        let no_ts: u32 = self.tokenizer.token_to_id("<|notimestamps|>").unwrap_or(50363);
        let eot: u32 = self.tokenizer.token_to_id("<|endoftext|>").unwrap_or(50257);

        // Build prompt tokens: <|SOT|> <|LANG|> <|transcribe|> <|notimestamps|>
        // 语言确定优先级：config.yaml language > DB models.language > auto-detect
        let mut lang_code = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };
        if lang_code == "auto" && !self.entry_language.is_empty() && self.entry_language != "auto" {
            lang_code = self.entry_language.clone();
        }
        eprintln!("[whisper] language: {}", lang_code);

        let mut dec_init = self.dec_init.lock().unwrap();

        // 构建 prompt tokens
        let mut tokens: Vec<i64> = vec![sot as i64];
        if lang_code != "auto" {
            let lang_tag = format!("<|{}|>", lang_code);
            if let Some(lang_id) = self.tokenizer.token_to_id(&lang_tag) {
                tokens.push(lang_id as i64);
            }
        } else {
            // auto-detect：先喂 [sot] 让模型预测语言 token（与 OpenAI whisper 一致）
            let detect_ids = Array2::from_shape_vec((1, 1), vec![sot as i64])?;
            let detect_out = dec_init.run(ort::inputs! {
                "input_ids" => ort::value::TensorRef::from_array_view(detect_ids.view())?,
                "encoder_hidden_states" => ort::value::TensorRef::from_array_view(encoder_hidden.view())?
            })?;
            let (det_shape, det_logits) = detect_out["logits"].try_extract_tensor::<f32>()?;
            let det_vocab = det_shape[2] as usize;
            let detected_lang = argmax_last_token(det_logits, 1, det_vocab);
            eprintln!("[whisper] auto-detected language token: {}", detected_lang);
            tokens.push(detected_lang as i64);
        }
        tokens.extend_from_slice(&[transcribe_tok as i64, no_ts as i64]);
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

        let mut kv = KvCache::new_from_decoder_output(&init_out, self.n_decoder_layers)?;

        // Autoregressive loop
        // 根据实际音频时长限制解码步数：compute_mel 会把音频 0 填充到 30s，
        // 若 VAD 只传入 2s 片段，剩余 28s 是静音，Whisper 在静音段不会预测 EOT
        // 反而开始幻听（重复最后一句话 / “谢谢观看”等）并跑满 448 步导致 RTF 暴增。
        // .en 模型平均生成 ~6 text tokens/秒，+10 为 prompt/safety 余量；
        // 30s 以上恢复 448 上限（实际上限由模型上下文决定）。
        let audio_seconds = audio.len() as f32 / SAMPLE_RATE as f32;
        let max_tokens = ((audio_seconds * 6.0) as usize + 10).min(448);
        eprintln!(
            "[whisper] audio {:.2}s → max_tokens={} (was 448)",
            audio_seconds, max_tokens
        );
        for _step in 1..max_tokens {
            let last_id = Array2::from_shape_vec((1, 1), vec![*tokens.last().unwrap()])?;

            let mut inputs = ort::inputs! {
                "input_ids" => ort::value::TensorRef::from_array_view(last_id.view())?
            };

            for layer in 0..self.n_decoder_layers {
                let (dk, dv, ek, ev) = self.past_key_names[layer];
                inputs.push((
                    dk.into(),
                    ort::value::TensorRef::from_array_view(kv.decoder_keys[layer].view())?.into(),
                ));
                inputs.push((
                    dv.into(),
                    ort::value::TensorRef::from_array_view(kv.decoder_values[layer].view())?.into(),
                ));
                inputs.push((
                    ek.into(),
                    ort::value::TensorRef::from_array_view(kv.encoder_keys[layer].view())?.into(),
                ));
                inputs.push((
                    ev.into(),
                    ort::value::TensorRef::from_array_view(kv.encoder_values[layer].view())?.into(),
                ));
            }

            let mut dec_past = self.dec_past.lock().unwrap();
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

            kv.update_decoder_kv(&dec_out, self.n_decoder_layers)?;

            if next_token == eot {
                break;
            }
            tokens.push(next_token as i64);
        }

        let text_ids: Vec<u32> = tokens[prompt_len..].iter().map(|&t| t as u32).collect();
        let text = self.tokenizer
            .decode(&text_ids, true)
            .map_err(|e| anyhow::anyhow!("Decode: {}", e))?;
        Ok(text)
    }
}

/// Transcribe audio using Whisper model
/// Input: 16kHz mono f32 samples, language code ("auto"/"zh"/"en"/...). Output: transcribed text.
pub fn transcribe(name: &str, audio: &[f32], language: &str) -> Result<String> {
    let cfg = config::load_config()?;
    let whisper_cfg = cfg
        .asr
        .whisper
        .as_ref()
        .context("No whisper models in config")?;
    let entry = whisper_cfg
        .get(name)
        .with_context(|| format!("whisper model '{}' not in DB", name))?;

    let engine = WhisperEngine::new(entry)?;
    crate::engine::transcribe_with_vad(&engine, audio, language)
}
