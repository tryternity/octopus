use anyhow::{Context, Result};
use ndarray::{Array2, ArrayD};
use ort::session::Session;
use ort::value::{Tensor, TensorElementType};
use std::collections::HashMap;
use once_cell::sync::Lazy;

#[derive(Clone)]
pub(crate) enum StateValue {
    F32(ArrayD<f32>),
    I64(ArrayD<i64>),
}

use crate::config;

// ── BBPE Mapping Table ──
pub static BBPE_TABLE: Lazy<HashMap<&'static str, u8>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("Ā", 0);
    m.insert("ā", 1);
    m.insert("Ă", 2);
    m.insert("ă", 3);
    m.insert("Ą", 4);
    m.insert("ą", 5);
    m.insert("Ć", 6);
    m.insert("ć", 7);
    m.insert("Ĉ", 8);
    m.insert("ĉ", 9);
    m.insert("Ċ", 10);
    m.insert("ċ", 11);
    m.insert("Č", 12);
    m.insert("č", 13);
    m.insert("Ď", 14);
    m.insert("ď", 15);
    m.insert("Đ", 16);
    m.insert("đ", 17);
    m.insert("Ē", 18);
    m.insert("ē", 19);
    m.insert("Ĕ", 20);
    m.insert("ĕ", 21);
    m.insert("Ė", 22);
    m.insert("ė", 23);
    m.insert("Ę", 24);
    m.insert("ę", 25);
    m.insert("Ě", 26);
    m.insert("ě", 27);
    m.insert("Ĝ", 28);
    m.insert("ĝ", 29);
    m.insert("Ğ", 30);
    m.insert("ğ", 31);
    m.insert(" ", 32);
    m.insert("!", 33);
    m.insert("#", 35);
    m.insert("$", 36);
    m.insert("%", 37);
    m.insert("&", 38);
    m.insert("'", 39);
    m.insert("(", 40);
    m.insert(")", 41);
    m.insert("*", 42);
    m.insert("+", 43);
    m.insert(",", 44);
    m.insert("-", 45);
    m.insert(".", 46);
    m.insert("/", 47);
    m.insert("0", 48);
    m.insert("1", 49);
    m.insert("2", 50);
    m.insert("3", 51);
    m.insert("4", 52);
    m.insert("5", 53);
    m.insert("6", 54);
    m.insert("7", 55);
    m.insert("8", 56);
    m.insert("9", 57);
    m.insert(":", 58);
    m.insert(";", 59);
    m.insert("<", 60);
    m.insert("=", 61);
    m.insert(">", 62);
    m.insert("?", 63);
    m.insert("@", 64);
    m.insert("A", 65);
    m.insert("B", 66);
    m.insert("C", 67);
    m.insert("D", 68);
    m.insert("E", 69);
    m.insert("F", 70);
    m.insert("G", 71);
    m.insert("H", 72);
    m.insert("I", 73);
    m.insert("J", 74);
    m.insert("K", 75);
    m.insert("L", 76);
    m.insert("M", 77);
    m.insert("N", 78);
    m.insert("O", 79);
    m.insert("P", 80);
    m.insert("Q", 81);
    m.insert("R", 82);
    m.insert("S", 83);
    m.insert("T", 84);
    m.insert("U", 85);
    m.insert("V", 86);
    m.insert("W", 87);
    m.insert("X", 88);
    m.insert("Y", 89);
    m.insert("Z", 90);
    m.insert("[", 91);
    m.insert("\\", 92);
    m.insert("]", 93);
    m.insert("^", 94);
    m.insert("_", 95);
    m.insert("`", 96);
    m.insert("a", 97);
    m.insert("b", 98);
    m.insert("c", 99);
    m.insert("d", 100);
    m.insert("e", 101);
    m.insert("f", 102);
    m.insert("g", 103);
    m.insert("h", 104);
    m.insert("i", 105);
    m.insert("j", 106);
    m.insert("k", 107);
    m.insert("l", 108);
    m.insert("m", 109);
    m.insert("n", 110);
    m.insert("o", 111);
    m.insert("p", 112);
    m.insert("q", 113);
    m.insert("r", 114);
    m.insert("s", 115);
    m.insert("t", 116);
    m.insert("u", 117);
    m.insert("v", 118);
    m.insert("w", 119);
    m.insert("x", 120);
    m.insert("y", 121);
    m.insert("z", 122);
    m.insert("{", 123);
    m.insert("|", 124);
    m.insert("}", 125);
    m.insert("~", 126);
    m.insert("Ġ", 127);
    m.insert("ġ", 128);
    m.insert("Ģ", 129);
    m.insert("ģ", 130);
    m.insert("Ĥ", 131);
    m.insert("ĥ", 132);
    m.insert("Ħ", 133);
    m.insert("ħ", 134);
    m.insert("Ĩ", 135);
    m.insert("ĩ", 136);
    m.insert("Ī", 137);
    m.insert("ī", 138);
    m.insert("Ĭ", 139);
    m.insert("ĭ", 140);
    m.insert("Į", 141);
    m.insert("į", 142);
    m.insert("İ", 143);
    m.insert("ı", 144);
    m.insert("Ĵ", 145);
    m.insert("ĵ", 146);
    m.insert("Ķ", 147);
    m.insert("ķ", 148);
    m.insert("ĸ", 149);
    m.insert("Ĺ", 150);
    m.insert("ĺ", 151);
    m.insert("Ļ", 152);
    m.insert("ļ", 153);
    m.insert("Ľ", 154);
    m.insert("ľ", 155);
    m.insert("Ł", 156);
    m.insert("ł", 157);
    m.insert("Ń", 158);
    m.insert("ń", 159);
    m.insert("Ņ", 160);
    m.insert("ņ", 161);
    m.insert("Ň", 162);
    m.insert("ň", 163);
    m.insert("Ŋ", 164);
    m.insert("ŋ", 165);
    m.insert("Ō", 166);
    m.insert("ō", 167);
    m.insert("Ŏ", 168);
    m.insert("ŏ", 169);
    m.insert("Ő", 170);
    m.insert("ő", 171);
    m.insert("Œ", 172);
    m.insert("œ", 173);
    m.insert("Ŕ", 174);
    m.insert("ŕ", 175);
    m.insert("Ŗ", 176);
    m.insert("ŗ", 177);
    m.insert("Ř", 178);
    m.insert("ř", 179);
    m.insert("Ś", 180);
    m.insert("ś", 181);
    m.insert("Ŝ", 182);
    m.insert("ŝ", 183);
    m.insert("Ş", 184);
    m.insert("ş", 185);
    m.insert("Š", 186);
    m.insert("š", 187);
    m.insert("Ţ", 188);
    m.insert("ţ", 189);
    m.insert("Ť", 190);
    m.insert("ť", 191);
    m.insert("Ŧ", 192);
    m.insert("ŧ", 193);
    m.insert("Ũ", 194);
    m.insert("ũ", 195);
    m.insert("Ū", 196);
    m.insert("ū", 197);
    m.insert("Ŭ", 198);
    m.insert("ŭ", 199);
    m.insert("Ů", 200);
    m.insert("ů", 201);
    m.insert("Ű", 202);
    m.insert("ű", 203);
    m.insert("Ų", 204);
    m.insert("ų", 205);
    m.insert("Ŵ", 206);
    m.insert("ŵ", 207);
    m.insert("Ŷ", 208);
    m.insert("ŷ", 209);
    m.insert("Ÿ", 210);
    m.insert("Ź", 211);
    m.insert("ź", 212);
    m.insert("Ż", 213);
    m.insert("ż", 214);
    m.insert("Ž", 215);
    m.insert("ž", 216);
    m.insert("ƀ", 217);
    m.insert("Ɓ", 218);
    m.insert("Ƃ", 219);
    m.insert("ƃ", 220);
    m.insert("Ƅ", 221);
    m.insert("ƅ", 222);
    m.insert("Ɔ", 223);
    m.insert("Ƈ", 224);
    m.insert("ƈ", 225);
    m.insert("Ɖ", 226);
    m.insert("Ɗ", 227);
    m.insert("Ƌ", 228);
    m.insert("ƌ", 229);
    m.insert("ƍ", 230);
    m.insert("Ǝ", 231);
    m.insert("Ə", 232);
    m.insert("Ɛ", 233);
    m.insert("Ƒ", 234);
    m.insert("ƒ", 235);
    m.insert("Ɠ", 236);
    m.insert("Ɣ", 237);
    m.insert("ƕ", 238);
    m.insert("Ɩ", 239);
    m.insert("Ɨ", 240);
    m.insert("Ƙ", 241);
    m.insert("ƙ", 242);
    m.insert("ƚ", 243);
    m.insert("ƛ", 244);
    m.insert("Ɯ", 245);
    m.insert("Ɲ", 246);
    m.insert("ƞ", 247);
    m.insert("Ɵ", 248);
    m.insert("Ơ", 249);
    m.insert("ơ", 250);
    m.insert("Ƣ", 251);
    m.insert("ƣ", 252);
    m.insert("Ƥ", 253);
    m.insert("ƥ", 254);
    m.insert("Ʀ", 255);
    m.insert("⁇", 32);
    m
});

// ── Fbank constants (matching standard Kaldi Native Fbank defaults) ──
pub(crate) const Z_FFT_SIZE: usize = 512;
pub(crate) const Z_FRAME_LEN: usize = 400;
pub(crate) const Z_FRAME_SHIFT: usize = 160;
pub(crate) const Z_NUM_BINS: usize = 80;
pub(crate) const Z_SAMPLE_RATE: u32 = 16000;

// ── Zipformer CTC blank ──
pub(crate) const ZIPFORMER_BLANK_ID: usize = 0;

// ── Public API ──

/// Transcribe audio using Zipformer model
/// Input: 16kHz mono f32 samples. Output: transcribed text.
pub fn transcribe(samples: &[f32], _language: &str) -> Result<String> {
    let cfg = config::load_config()?;
    let zip_cfg = cfg
        .asr
        .zipformer
        .as_ref()
        .context("No zipformer models in config")?;

    let entry = if let Some(e) = zip_cfg.get(&cfg.asr.active) {
        e
    } else {
        zip_cfg
            .iter()
            .next()
            .map(|(_, v)| v)
            .context("No zipformer model entries")?
    };

    let hf_path = config::find_hf_cache(&entry.source)?;
    let model_path = if hf_path.join("model.onnx").exists() {
        hf_path.join("model.onnx")
    } else if hf_path.join("model.int8.onnx").exists() {
        hf_path.join("model.int8.onnx")
    } else {
        anyhow::bail!(
            "model.onnx / model.int8.onnx not found at {}",
            hf_path.display()
        );
    };

    let mut session = Session::builder()?.commit_from_file(&model_path)?;

    // Read chunk parameters from model metadata (T, decode_chunk_len)
    let metadata = session.metadata()?;
    let chunk_len: usize = metadata
        .custom("T")
        .and_then(|s| s.parse().ok())
        .unwrap_or(77);
    let chunk_shift: usize = metadata
        .custom("decode_chunk_len")
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    drop(metadata);

    // Compute Fbank features (DC offset removal, pre-emphasis, povey window)
    let my_feats = compute_fbank_features(samples)?;
    let n_frames = my_feats.nrows();

    // Setup initial states by inspecting session inputs
    let mut states: Vec<(String, StateValue)> = Vec::new();
    for input in session.inputs() {
        let name = input.name();
        if name == "x" {
            continue;
        }
        if let Some(shape) = input.dtype().tensor_shape() {
            let dims: Vec<usize> = shape
                .iter()
                .map(|&d| if d <= 0 { 1 } else { d as usize })
                .collect();
            let is_int64 = match input.dtype().tensor_type() {
                Some(TensorElementType::Int64) => true,
                _ => false,
            };
            if is_int64 {
                let arr = ArrayD::<i64>::zeros(dims);
                states.push((name.to_string(), StateValue::I64(arr)));
            } else {
                let arr = ArrayD::<f32>::zeros(dims);
                states.push((name.to_string(), StateValue::F32(arr)));
            }
        }
    }

    // Decoding results
    let mut token_ids = Vec::new();
    let mut prev_id = -1;

    // Pad features with last frame values if we run out of frames
    let mut padded_feats = Array2::<f32>::zeros((n_frames + chunk_len, Z_NUM_BINS));
    for i in 0..n_frames {
        for j in 0..Z_NUM_BINS {
            padded_feats[[i, j]] = my_feats[[i, j]];
        }
    }
    for i in n_frames..(n_frames + chunk_len) {
        let last_idx = if n_frames > 0 { n_frames - 1 } else { 0 };
        for j in 0..Z_NUM_BINS {
            padded_feats[[i, j]] = my_feats[[last_idx, j]];
        }
    }

    // Load tokens mapping
    let tokens_path = hf_path.join("tokens.txt");
    let tokens_text = std::fs::read_to_string(&tokens_path)
        .with_context(|| format!("tokens.txt not found at {}", tokens_path.display()))?;

    let mut vocab: Vec<String> = Vec::new();
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

    // Check if symbol table contains byte BPE characters to determine mode
    let mut is_bbpe = false;
    for tok in &vocab {
        if tok.chars().any(|c| c as u32 > 0xc6 && c != '▁') {
            // Note: split logic uses standard BBPE range check
            // if any token is composed entirely of BBPE chars
        }
        // Simplified check: if <blk>, <sos/eos>, <unk> exist and there are BPE tokens
        if tok.starts_with('▁') {
            is_bbpe = true;
        }
    }

    // Chunked inference
    let mut frame_idx = 0;
    while frame_idx < n_frames {
        let mut chunk = Array2::<f32>::zeros((chunk_len, Z_NUM_BINS));
        for i in 0..chunk_len {
            for j in 0..Z_NUM_BINS {
                chunk[[i, j]] = padded_feats[[frame_idx + i, j]];
            }
        }
        let (chunk_vec, _) = chunk.into_raw_vec_and_offset();
        let chunk_input = ndarray::Array3::from_shape_vec(
            (1, chunk_len, Z_NUM_BINS),
            chunk_vec,
        )?;

        let x_tensor = ort::value::TensorRef::from_array_view(chunk_input.view())?;

        // Prepare input map for this step
        let mut inputs = ort::inputs! {
            "x" => x_tensor
        };

        // Create temporary tensors for the inputs of this run step
        let mut state_tensors = Vec::new();
        for (name, val) in &states {
            let t = match val {
                StateValue::F32(arr) => Tensor::from_array(arr.clone())?.into_dyn(),
                StateValue::I64(arr) => Tensor::from_array(arr.clone())?.into_dyn(),
            };
            state_tensors.push((name.clone(), t));
        }

        for (name, t) in &state_tensors {
            inputs.push((name.as_str().into(), t.into()));
        }

        // Run model forward
        let outputs = session.run(inputs)?;

        // The first output is log_probs [1, num_out_frames, vocab_dim]
        let (log_probs_shape, log_probs_data) = outputs[0].try_extract_tensor::<f32>()?;
        let num_out_frames = log_probs_shape[1] as usize;
        let vocab_dim = log_probs_shape[2] as usize;

        // Carry states forward
        for (name, val) in states.iter_mut() {
            let out_name = format!("new_{}", name);
            if let Some(new_val) = outputs.get(out_name.as_str()) {
                match val {
                    StateValue::F32(arr) => {
                        let (shape, data) = new_val.try_extract_tensor::<f32>()?;
                        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
                        *arr = ndarray::ArrayD::from_shape_vec(dims, data.to_vec())?;
                    }
                    StateValue::I64(arr) => {
                        let (shape, data) = new_val.try_extract_tensor::<i64>()?;
                        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
                        *arr = ndarray::ArrayD::from_shape_vec(dims, data.to_vec())?;
                    }
                }
            }
        }

        // Decode CTC frames
        for t in 0..num_out_frames {
            let offset = t * vocab_dim;
            let frame_logits = &log_probs_data[offset..offset + vocab_dim];
            let best_id = frame_logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0);
            if best_id != ZIPFORMER_BLANK_ID && best_id as isize != prev_id {
                token_ids.push(best_id);
            }
            prev_id = best_id as isize;
        }

        frame_idx += chunk_shift;
    }

    // Decode tokens to text
    let mut decoded = String::new();
    if is_bbpe {
        let mut raw_token_string = String::new();
        for &tid in &token_ids {
            if tid < vocab.len() {
                raw_token_string.push_str(&vocab[tid]);
            }
        }
        decoded = decode_byte_bpe(&raw_token_string);
    } else {
        // Standard token decoding
        for &tid in &token_ids {
            if tid < vocab.len() {
                let token = &vocab[tid];
                if token.starts_with('▁') {
                    if !decoded.is_empty() {
                        decoded.push(' ');
                    }
                    decoded.push_str(&token[3..]); // Strip BPE space marker ▁ (3 bytes)
                } else {
                    decoded.push_str(token);
                }
            }
        }
    }

    Ok(decoded.trim().to_string())
}

// ── Decode Byte BPE mapping ──

pub(crate) fn decode_byte_bpe(text: &str) -> String {
    let mut ans = Vec::new();
    for char_val in text.chars() {
        let char_str = char_val.to_string();
        if char_str == "▁" {
            if !ans.is_empty() {
                let last_byte = *ans.last().unwrap();
                if last_byte != b' ' && last_byte >= 32 && last_byte <= 126 {
                    ans.push(b' ');
                }
            }
        } else if let Some(&byte_val) = BBPE_TABLE.get(char_str.as_str()) {
            ans.push(byte_val);
        } else if char_str.len() == 1 {
            let b = char_val as u32;
            if b >= 32 && b <= 126 {
                ans.push(b as u8);
            }
        }
    }
    String::from_utf8_lossy(&ans).into_owned()
}

// ── Kaldi Fbank features extraction ──

pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = (samples.len() + Z_FRAME_SHIFT / 2) / Z_FRAME_SHIFT;
    let n_frames = n_frames.max(1);

    let window = povey_window(Z_FRAME_LEN);
    let mel_fb = mel_filterbank_fbank();

    let mut planner = rustfft::FftPlanner::new();
    let fft = planner.plan_fft_forward(Z_FFT_SIZE);

    let n_freqs = Z_FFT_SIZE / 2 + 1;
    let mut fbank_data = vec![0.0f32; n_frames * Z_NUM_BINS];

    for fi in 0..n_frames {
        let midpoint = Z_FRAME_SHIFT * fi + Z_FRAME_SHIFT / 2;
        let wave_start = midpoint as isize - (Z_FRAME_LEN as isize) / 2;

        let mut frame = vec![0.0f32; Z_FRAME_LEN];
        let wave_dim = samples.len() as isize;
        for s in 0..Z_FRAME_LEN {
            let mut s_in_wave = s as isize + wave_start;
            while s_in_wave < 0 || s_in_wave >= wave_dim {
                if s_in_wave < 0 {
                    s_in_wave = -s_in_wave - 1;
                } else {
                    s_in_wave = 2 * wave_dim - 1 - s_in_wave;
                }
            }
            frame[s] = samples[s_in_wave as usize];
        }

        // 1. Remove DC offset
        let sum: f32 = frame.iter().sum();
        let mean = sum / Z_FRAME_LEN as f32;
        for val in frame.iter_mut() {
            *val -= mean;
        }

        // 2. Preemphasize
        let mut preemph = vec![0.0f32; Z_FRAME_LEN];
        for i in (1..Z_FRAME_LEN).rev() {
            preemph[i] = frame[i] - 0.97 * frame[i - 1];
        }
        preemph[0] = frame[0] - 0.97 * frame[0];

        // 3. Apply window
        let mut buf: Vec<rustfft::num_complex::Complex<f32>> = (0..Z_FFT_SIZE)
            .map(|j| {
                let s = if j < Z_FRAME_LEN {
                    preemph[j] * window[j]
                } else {
                    0.0
                };
                rustfft::num_complex::Complex::new(s, 0.0)
            })
            .collect();
        fft.process(&mut buf);

        for mi in 0..Z_NUM_BINS {
            let mut sum = 0.0f64;
            for k in 0..n_freqs {
                let power =
                    buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
                sum += power * mel_fb[mi][k];
            }
            fbank_data[fi * Z_NUM_BINS + mi] = (sum as f32 + 1.1920929e-7).ln();
        }
    }

    Array2::from_shape_vec((n_frames, Z_NUM_BINS), fbank_data).map_err(Into::into)
}

fn povey_window(size: usize) -> Vec<f32> {
    let a = 2.0 * std::f64::consts::PI / (size - 1) as f64;
    (0..size)
        .map(|i| (0.5 - 0.5 * (a * i as f64).cos()).powf(0.85) as f32)
        .collect()
}

pub(crate) fn mel_filterbank_fbank() -> Vec<Vec<f64>> {
    let n_freqs = Z_FFT_SIZE / 2 + 1;
    let fmax = Z_SAMPLE_RATE as f64 / 2.0;
    let mel_min = hz_to_mel(20.0);
    let mel_max = hz_to_mel(fmax);

    let hz_points: Vec<f64> = (0..=Z_NUM_BINS + 1)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (Z_NUM_BINS + 1) as f64))
        .collect();

    let fft_freqs: Vec<f64> = (0..n_freqs)
        .map(|i| Z_SAMPLE_RATE as f64 * i as f64 / Z_FFT_SIZE as f64)
        .collect();

    let mut filters = vec![vec![0.0f64; n_freqs]; Z_NUM_BINS];
    for i in 0..Z_NUM_BINS {
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

pub(crate) fn hz_to_mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

pub(crate) fn mel_to_hz(mel: f64) -> f64 {
    700.0 * ((mel / 1127.0).exp() - 1.0)
}
