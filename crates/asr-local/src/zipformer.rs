use anyhow::{Context, Result};
use ndarray::{Array2, ArrayD};
use ort::session::Session;
use ort::value::{Tensor, TensorElementType};
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;

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
    m.insert("\"", 34);
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

/// P0-8（2026-07-21）：whisper mel filterbank 稀疏化——预计算每行非零 [start, end)。
/// WHISPER_MEL_FILTERBANK 是 [[f64; 201]; 80] 固定数组（硬编码自 OpenAI whisper 模型），
/// 转 Vec<Vec<f64>> 后复用 mel_filterbank_ranges 计算稀疏区间。
static WHISPER_MEL_FILTERBANK_RANGE: Lazy<Vec<(usize, usize)>> = Lazy::new(|| {
    let as_vec: Vec<Vec<f64>> = crate::whisper_mel_matrix::WHISPER_MEL_FILTERBANK
        .iter()
        .map(|row| row.to_vec())
        .collect();
    crate::feature::mel_filterbank_ranges(&as_vec)
});

static POVEY_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| crate::feature::povey_window(Z_FRAME_LEN));
static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| {
    // C1 修复：改用 mel 空间 filterbank 权重（对齐 paraformer / kaldi_native_fbank）
    crate::feature::mel_filterbank(Z_NUM_BINS, Z_FFT_SIZE, Z_SAMPLE_RATE, Z_SAMPLE_RATE as f64 / 2.0)
});
/// P0-8（2026-07-21）：mel filterbank 稀疏化——预计算每行非零 [start, end) 区间。
static MEL_FILTERBANK_RANGE: Lazy<Vec<(usize, usize)>> =
    Lazy::new(|| crate::feature::mel_filterbank_ranges(&MEL_FILTERBANK));

// ── Fbank constants (matching standard Kaldi Native Fbank defaults) ──
pub(crate) const Z_FFT_SIZE: usize = 512;
pub(crate) const Z_FRAME_LEN: usize = 400;
pub(crate) const Z_FRAME_SHIFT: usize = 160;
pub(crate) const Z_NUM_BINS: usize = 80;
pub(crate) const Z_SAMPLE_RATE: u32 = 16000;

// 预规划的 512 点正向 FFT — fbank 提取共用，避免每次重复规划（堆分配 + twiddle 计算）。
// 流式热路径（Zipformer process_chunks 每 accept_samples 调 compute_fbank_features）尤为关键。
// 对齐 paraformer::FBANK_FFT 模式。
static Z_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>> = Lazy::new(|| {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    planner.plan_fft_forward(Z_FFT_SIZE)
});

// ── Zipformer CTC blank ──
pub(crate) const ZIPFORMER_BLANK_ID: usize = 0;

// ── Public API ──

/// 加载 tokens.txt 为 vocab 数组（id → token 字符串）。
/// 格式：每行 `<token> <id>`，id 从 0 开始。
pub(crate) fn load_vocab(hf_path: &std::path::Path) -> Result<Vec<String>> {
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
    Ok(vocab)
}

/// 遍历 session inputs，为所有非 "x" 的输入创建零张量初始状态。
/// 适用于 Zipformer encoder（CTC 和 Transducer 结构相同）。
pub(crate) fn initial_encoder_states(session: &Session) -> Vec<(String, StateValue)> {
    let mut initial_states: Vec<(String, StateValue)> = Vec::new();
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
            let is_int64 = matches!(input.dtype().tensor_type(), Some(TensorElementType::Int64));
            if is_int64 {
                let arr = ArrayD::<i64>::zeros(dims);
                initial_states.push((name.to_string(), StateValue::I64(arr)));
            } else {
                let arr = ArrayD::<f32>::zeros(dims);
                initial_states.push((name.to_string(), StateValue::F32(arr)));
            }
        }
    }
    initial_states
}

pub struct ZipformerCtcEngine {
    session: parking_lot::Mutex<Session>,
    chunk_len: usize,
    chunk_shift: usize,
    vocab: Vec<String>,
    is_bbpe: bool,
    initial_states: Vec<(String, StateValue)>,
    is_whisper: bool,
}

pub(crate) fn is_vocab_bbpe(vocab: &[String]) -> bool {
    let mut has_bbpe_marker = false;
    for tok in vocab {
        if tok.starts_with('<') && tok.ends_with('>') {
            continue;
        }
        if tok.starts_with('▁') {
            has_bbpe_marker = true;
        }
        for c in tok.chars() {
            if c == '▁' {
                continue;
            }
            if !c.is_ascii() {
                let c_str = c.to_string();
                if !BBPE_TABLE.contains_key(c_str.as_str()) {
                    return false;
                }
            }
        }
    }
    has_bbpe_marker
}

pub(crate) fn discover_streaming_zipformer_onnx(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    // 1. Standard names
    for name in ["model.int8.onnx", "model.onnx"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Scan directory for ONNX files
    let entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("Cannot read dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "onnx")
                .unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        anyhow::bail!(
            "No .onnx files found at {}. Expected model.onnx or encoder-*.onnx",
            dir.display()
        );
    }

    // Prefer encoder file (transducer models: encoder, decoder, joiner)
    // Prefer int8 version first
    if let Some(e) = entries.iter().find(|e| {
        let binding = e.file_name();
        let name = binding.to_string_lossy();
        name.starts_with("encoder") && name.contains("int8")
    }) {
        return Ok(e.path());
    }
    // Any encoder file
    if let Some(e) = entries.iter().find(|e| {
        let binding = e.file_name();
        binding.to_string_lossy().starts_with("encoder")
    }) {
        return Ok(e.path());
    }

    // 3. Fall back: prefer int8, then any
    if let Some(e) = entries
        .iter()
        .find(|e| e.file_name().to_string_lossy().contains("int8"))
    {
        return Ok(e.path());
    }

    entries
        .into_iter()
        .next()
        .map(|e| e.path())
        .context("No .onnx files found")
}

impl ZipformerCtcEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;
        let model_path = discover_streaming_zipformer_onnx(&hf_path)?;

        let session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&model_path)?;

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
        let is_whisper = metadata
            .custom("feature")
            .map(|s| s == "whisper")
            .unwrap_or(false);
        drop(metadata);

        // Setup initial states by inspecting session inputs
        let initial_states = initial_encoder_states(&session);

        // Load tokens mapping
        let vocab = load_vocab(&hf_path)?;

        // Check if symbol table contains byte BPE characters to determine mode
        let is_bbpe = is_vocab_bbpe(&vocab);


        Ok(Self {
            session: parking_lot::Mutex::new(session),
            chunk_len,
            chunk_shift,
            vocab,
            is_bbpe,
            initial_states,
            is_whisper,
        })
    }
}

impl crate::engine::OfflineAsrEngine for ZipformerCtcEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        let mut session = self.session.lock();
        let mut my_feats = if self.is_whisper {
            compute_whisper_features_linear(samples)?
        } else {
            compute_fbank_features(samples)?
        };
        // 离线路径：对整段特征做全局归一化再分 chunk 送 encoder。
        // 与流式路径（per-chunk 归一化）不同——离线一次性处理整段音频，
        // 全局 max_v 稳定；且这些模型本质是流式模型，sherpa-onnx 离线
        // Transducer impl 对 whisper 特征不做归一化（仅 NeMo CTC 做 CMVN），
        // 我们的 chunk 循环模拟需要归一化才能正常工作。
        if self.is_whisper {
            normalize_whisper_features(&mut my_feats);
        }
        let n_frames = my_feats.nrows();

        let mut states = self.initial_states.clone();

        // Decoding results
        let mut token_ids = Vec::new();
        let mut prev_id = -1;

        // Pad features with last frame values if we run out of frames
        let mut padded_feats = Array2::<f32>::zeros((n_frames + self.chunk_len, Z_NUM_BINS));
        for i in 0..n_frames {
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[i, j]];
            }
        }
        for i in n_frames..(n_frames + self.chunk_len) {
            let last_idx = if n_frames > 0 { n_frames - 1 } else { 0 };
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[last_idx, j]];
            }
        }

        // Chunked inference
        let mut frame_idx = 0;
        while frame_idx < n_frames {
            let mut chunk = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
            for i in 0..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = padded_feats[[frame_idx + i, j]];
                }
            }



            let (chunk_vec, _) = chunk.into_raw_vec_and_offset();
            let chunk_input = ndarray::Array3::from_shape_vec(
                (1, self.chunk_len, Z_NUM_BINS),
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

            frame_idx += self.chunk_shift;
        }

        // Decode tokens to text
        let decoded = decode_token_ids(&self.vocab, self.is_bbpe, &token_ids);

        Ok(decoded.trim().to_string())
    }
}

/// 将 token id 序列解码为文本（CTC / Transducer 共用）。
/// 支持 BBPE 和 SentencePiece byte-fallback 两种模式。
pub(crate) fn decode_token_ids(vocab: &[String], is_bbpe: bool, token_ids: &[usize]) -> String {
    if is_bbpe {
        let mut raw_token_string = String::new();
        for &tid in token_ids {
            if tid < vocab.len() {
                raw_token_string.push_str(&vocab[tid]);
            }
        }
        decode_byte_bpe(&raw_token_string, false)
    } else {
        // Check for BPE with byte fallback (standard SentencePiece behavior)
        let mut is_bpe_with_byte_fallback = false;
        let mut id_for_0x00 = 0;

        let pos_00 = vocab.iter().position(|t| t == "<0x00>");
        let pos_ff = vocab.iter().position(|t| t == "<0xFF>");
        if let (Some(p00), Some(pff)) = (pos_00, pos_ff) {
            if pff > p00 && pff - p00 == 255 {
                is_bpe_with_byte_fallback = true;
                id_for_0x00 = p00;
            }
        }

        let mut bytes: Vec<u8> = Vec::new();

        for &tid in token_ids {
            if tid >= vocab.len() {
                continue;
            }
            let mut token = vocab[tid].clone();

            // Decode BPE with byte fallback (translates to raw byte)
            if is_bpe_with_byte_fallback
                && token.len() == 6
                && token.starts_with("<0x")
                && token.ends_with('>')
                && tid >= id_for_0x00 && tid <= id_for_0x00 + 255 {
                    if let Ok(hex_val) = u8::from_str_radix(&token[3..5], 16) {
                        if hex_val == (tid - id_for_0x00) as u8 {
                            bytes.push(hex_val);
                            continue;
                        }
                    }
                }

            // For BPE-based models, we replace ▁ (U+2581, utf8 \xe2\x96\x81) with a space " "
            if token.len() >= 3 && token.starts_with('▁') {
                token = format!(" {}", &token[3..]);
            }

            bytes.extend_from_slice(token.as_bytes());
        }

        clean_decode_utf8(&bytes, false)
    }
}

// ── Zipformer Transducer (RNN-T) Engine ──

/// RNN-T Transducer 引擎：encoder + decoder + joiner 三 session 架构。
/// encoder 与 CTC 版完全相同（cached_* 状态管理），但输出 encoder_out
/// 而非 log_probs；decoder 无状态（输入最近 context_size 个 token），
/// joiner 融合 encoder frame + decoder_out → logit，greedy argmax 解码。
pub struct ZipformerTransducerEngine {
    encoder_session: parking_lot::Mutex<Session>,
    decoder_session: parking_lot::Mutex<Session>,
    joiner_session: parking_lot::Mutex<Session>,
    chunk_len: usize,
    chunk_shift: usize,
    context_size: usize,
    vocab: Vec<String>,
    is_bbpe: bool,
    initial_states: Vec<(String, StateValue)>,
    is_whisper: bool,
}

impl ZipformerTransducerEngine {
    pub fn new(entry: &config::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;

        let encoder_path = discover_streaming_zipformer_onnx(&hf_path)?;
        let decoder_path = hf_path.join("decoder.onnx");
        let joiner_path = {
            // 优先 joiner.int8.onnx，其次 joiner.onnx
            let int8 = hf_path.join("joiner.int8.onnx");
            if int8.exists() {
                int8
            } else {
                hf_path.join("joiner.onnx")
            }
        };

        if !decoder_path.exists() {
            anyhow::bail!(
                "decoder.onnx not found at {} — not a Transducer model",
                decoder_path.display()
            );
        }
        if !joiner_path.exists() {
            anyhow::bail!(
                "joiner.onnx not found at {} — not a Transducer model",
                joiner_path.display()
            );
        }

        let encoder_session =
            crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&encoder_path)?;
        let decoder_session =
            crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&decoder_path)?;
        let joiner_session =
            crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&joiner_path)?;

        // encoder metadata: T, decode_chunk_len, feature
        let metadata = encoder_session.metadata()?;
        let chunk_len: usize = metadata
            .custom("T")
            .and_then(|s| s.parse().ok())
            .unwrap_or(77);
        let chunk_shift: usize = metadata
            .custom("decode_chunk_len")
            .and_then(|s| s.parse().ok())
            .unwrap_or(64);
        let is_whisper = metadata
            .custom("feature")
            .map(|s| s == "whisper")
            .unwrap_or(false);
        drop(metadata);

        // encoder_dim: 从 encoder 输出 shape 最后一维读（如 512 / 768）
        let mut encoder_dim: usize = 0;
        for output in encoder_session.outputs() {
            if output.name() == "encoder_out" {
                if let Some(shape) = output.dtype().tensor_shape() {
                    // shape = [N, T', enc_dim]
                    if let Some(last) = shape.last() {
                        encoder_dim = if *last <= 0 { 0 } else { *last as usize };
                    }
                }
                break;
            }
        }
        // 兜底：从 joiner 输入读
        if encoder_dim == 0 {
            for input in joiner_session.inputs() {
                if input.name() == "encoder_out" {
                    if let Some(shape) = input.dtype().tensor_shape() {
                        if let Some(last) = shape.last() {
                            encoder_dim = if *last <= 0 { 512 } else { *last as usize };
                        }
                    }
                    break;
                }
            }
        }
        if encoder_dim == 0 {
            encoder_dim = 512;
            log::warn!("无法从模型 shape 读出 encoder_dim，使用默认值 512");
        }

        // context_size: 从 decoder metadata 读（默认 2）
        let context_size = decoder_session
            .metadata()
            .ok()
            .and_then(|m| m.custom("context_size"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(2);

        log::info!(
            "ZipformerTransducer: encoder_dim={}, context_size={}, chunk_len={}, chunk_shift={}, is_whisper={}",
            encoder_dim, context_size, chunk_len, chunk_shift, is_whisper
        );

        // encoder 初始状态（同 CTC）
        let initial_states = initial_encoder_states(&encoder_session);

        // vocab
        let vocab = load_vocab(&hf_path)?;
        let is_bbpe = is_vocab_bbpe(&vocab);

        Ok(Self {
            encoder_session: parking_lot::Mutex::new(encoder_session),
            decoder_session: parking_lot::Mutex::new(decoder_session),
            joiner_session: parking_lot::Mutex::new(joiner_session),
            chunk_len,
            chunk_shift,
            context_size,
            vocab,
            is_bbpe,
            initial_states,
            is_whisper,
        })
    }

    /// 运行 decoder：输入 token 窗口 [context_size] → decoder_out [enc_dim]
    fn run_decoder(&self, token_window: &[i64]) -> Result<Vec<f32>> {
        let y = ndarray::Array2::from_shape_vec((1, token_window.len()), token_window.to_vec())?;
        let y_tensor = ort::value::TensorRef::from_array_view(y.view())?;
        let mut session = self.decoder_session.lock();
        let outputs = session.run(ort::inputs! { "y" => y_tensor })?;
        let (_shape, data) = outputs["decoder_out"].try_extract_tensor::<f32>()?;
        Ok(data.to_vec())
    }

    /// 运行 joiner：encoder_frame [enc_dim] + decoder_out [enc_dim] → logit [vocab_size]
    fn run_joiner(&self, enc_frame: &[f32], dec_out: &[f32]) -> Result<Vec<f32>> {
        let enc = ndarray::Array2::from_shape_vec((1, enc_frame.len()), enc_frame.to_vec())?;
        let dec = ndarray::Array2::from_shape_vec((1, dec_out.len()), dec_out.to_vec())?;
        let enc_t = ort::value::TensorRef::from_array_view(enc.view())?;
        let dec_t = ort::value::TensorRef::from_array_view(dec.view())?;
        let mut session = self.joiner_session.lock();
        let outputs = session.run(ort::inputs! {
            "encoder_out" => enc_t,
            "decoder_out" => dec_t,
        })?;
        let (_shape, data) = outputs["logit"].try_extract_tensor::<f32>()?;
        Ok(data.to_vec())
    }
}

impl crate::engine::OfflineAsrEngine for ZipformerTransducerEngine {
    fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
        let mut enc_session = self.encoder_session.lock();

        // 特征提取
        let mut my_feats = if self.is_whisper {
            compute_whisper_features_linear(samples)?
        } else {
            compute_fbank_features(samples)?
        };
        // 离线路径全局归一化（同 CTC 注释，详见 ZipformerCtcEngine::transcribe）
        if self.is_whisper {
            normalize_whisper_features(&mut my_feats);
        }
        let n_frames = my_feats.nrows();

        // encoder state
        let mut states = self.initial_states.clone();

        // RNN-T token 缓冲：初始 [-1, ..., -1, 0]，长度 = context_size
        let mut token_buf: Vec<i64> = vec![-1; self.context_size];
        if let Some(last) = token_buf.last_mut() {
            *last = 0; // blank
        }

        // 输出 token ids（不含 context padding）
        let mut emitted_ids: Vec<usize> = Vec::new();

        // Pad features（同 CTC：尾部补最后一帧）
        let mut padded_feats = Array2::<f32>::zeros((n_frames + self.chunk_len, Z_NUM_BINS));
        for i in 0..n_frames {
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[i, j]];
            }
        }
        for i in n_frames..(n_frames + self.chunk_len) {
            let last_idx = if n_frames > 0 { n_frames - 1 } else { 0 };
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[last_idx, j]];
            }
        }

        // Chunked encoder 推理 + RNN-T greedy decoding
        let mut frame_idx = 0;
        while frame_idx < n_frames {
            // 构造 chunk 输入
            let mut chunk = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
            for i in 0..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = padded_feats[[frame_idx + i, j]];
                }
            }
            let (chunk_vec, _) = chunk.into_raw_vec_and_offset();
            let chunk_input =
                ndarray::Array3::from_shape_vec((1, self.chunk_len, Z_NUM_BINS), chunk_vec)?;
            let x_tensor = ort::value::TensorRef::from_array_view(chunk_input.view())?;

            // 构建 encoder inputs（x + 所有状态）
            let mut inputs = ort::inputs! { "x" => x_tensor };
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

            // encoder forward
            let outputs = enc_session.run(inputs)?;

            // encoder_out: [1, T', enc_dim]
            let (enc_shape, enc_data) = outputs["encoder_out"].try_extract_tensor::<f32>()?;
            let num_enc_frames = enc_shape[1] as usize;
            let enc_dim = enc_shape[2] as usize;

            // 更新 encoder states（同 CTC：new_<name> 输出回写）
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

            // 用当前 token_buf 跑一次 decoder → decoder_out [1, enc_dim]
            let mut current_dec = self.run_decoder(&token_buf)?;

            // 对每个 encoder frame 做 RNN-T greedy decoding
            for t in 0..num_enc_frames {
                let enc_offset = t * enc_dim;
                let enc_frame = &enc_data[enc_offset..enc_offset + enc_dim];

                // inner emit loop：同一 encoder frame 上可能连续发射多个 token
                let mut safety = 0usize;
                loop {
                    let logit = self.run_joiner(enc_frame, &current_dec)?;
                    let best_id = logit
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(i, _)| i)
                        .unwrap_or(0);

                    if best_id == 0 {
                        // blank → 移到下一 encoder frame
                        break;
                    }

                    // 发射 token，更新 token_buf（滑动窗口）
                    emitted_ids.push(best_id);
                    token_buf.push(best_id as i64);
                    if token_buf.len() > self.context_size {
                        token_buf.remove(0);
                    }
                    // 重跑 decoder（新的 token 上下文）
                    current_dec = self.run_decoder(&token_buf)?;

                    safety += 1;
                    if safety >= 20 {
                        log::warn!("RNN-T inner loop safety break at frame {}", t);
                        break;
                    }
                }
            }

            frame_idx += self.chunk_shift;
        }

        // Decode tokens to text
        let decoded = decode_token_ids(&self.vocab, self.is_bbpe, &emitted_ids);
        Ok(decoded.trim().to_string())
    }
}

// ── Decode Byte BPE mapping ──

pub(crate) fn clean_decode_utf8(bytes: &[u8], is_streaming: bool) -> String {
    let mut decode_slice = bytes;

    if is_streaming && !bytes.is_empty() {
        let mut last_start_idx = None;
        for i in (0..bytes.len()).rev() {
            if (bytes[i] & 0xC0) != 0x80 {
                last_start_idx = Some(i);
                break;
            }
        }

        if let Some(start_idx) = last_start_idx {
            let start_byte = bytes[start_idx];
            let required_len = if (start_byte & 0x80) == 0 {
                1
            } else if (start_byte & 0xE0) == 0xC0 {
                2
            } else if (start_byte & 0xF0) == 0xE0 {
                3
            } else if (start_byte & 0xF8) == 0xF0 {
                4
            } else {
                1
            };

            let actual_len = bytes.len() - start_idx;
            if actual_len < required_len {
                decode_slice = &bytes[..start_idx];
            }
        }
    }

    let decoded = String::from_utf8_lossy(decode_slice);
    decoded.replace('\u{FFFD}', "")
}

pub(crate) fn decode_byte_bpe(text: &str, is_streaming: bool) -> String {
    let mut ans = Vec::new();
    for char_val in text.chars() {
        let char_str = char_val.to_string();
        if char_str == "▁" {
            if !ans.is_empty() {
                let last_byte = *ans.last().unwrap();
                if last_byte > b' ' && last_byte <= 126 {
                    ans.push(b' ');
                }
            }
        } else if let Some(&byte_val) = BBPE_TABLE.get(char_str.as_str()) {
            ans.push(byte_val);
        } else if char_str.len() == 1 {
            let b = char_val as u32;
            if (32..=126).contains(&b) {
                ans.push(b as u8);
            }
        }
    }
    clean_decode_utf8(&ans, is_streaming)
}


// ── Whisper Feature Constants and Extractor ──

static WHISPER_HANN_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| whisper_hann_window(400));

// 预规划的 400 点正向 FFT — Whisper 特征提取（compute_whisper_features_linear）共用，
// 与 Z_FFT 同理：流式热路径避免每次 accept_samples 重复规划。
static WHISPER_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>> = Lazy::new(|| {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    planner.plan_fft_forward(400)
});

fn whisper_hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos()))
        .collect()
}

pub(crate) fn compute_whisper_features_linear(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = (samples.len() + Z_FRAME_SHIFT / 2) / Z_FRAME_SHIFT;
    let n_frames = n_frames.max(1);

    let fft = &*WHISPER_FFT;

    let mut fbank_data = vec![0.0f32; n_frames * Z_NUM_BINS];

    for fi in 0..n_frames {
        let midpoint = Z_FRAME_SHIFT * fi + Z_FRAME_SHIFT / 2;
        let wave_start = midpoint as isize - (Z_FRAME_LEN as isize) / 2;

        let mut frame = vec![0.0f32; Z_FRAME_LEN];
        let wave_dim = samples.len() as isize;
        for (s, frame_val) in frame.iter_mut().enumerate().take(Z_FRAME_LEN) {
            let mut s_in_wave = s as isize + wave_start;
            while s_in_wave < 0 || s_in_wave >= wave_dim {
                if s_in_wave < 0 {
                    s_in_wave = -s_in_wave - 1;
                } else {
                    s_in_wave = 2 * wave_dim - 1 - s_in_wave;
                }
            }
            *frame_val = samples[s_in_wave as usize];
        }

        // Apply Hann window
        let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); 400];
        for j in 0..400 {
            buf[j] = rustfft::num_complex::Complex::new(frame[j] * WHISPER_HANN_WINDOW[j], 0.0);
        }
        fft.process(&mut buf);

        // Pre-compute power spectrum
        let mut power_spectrum = [0.0f64; 201];
        for k in 0..201 {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

        for mi in 0..Z_NUM_BINS {
            let mut sum = 0.0f64;
            let fb_row = &crate::whisper_mel_matrix::WHISPER_MEL_FILTERBANK[mi];
            let (start, end) = WHISPER_MEL_FILTERBANK_RANGE[mi];
            for k in start..end {
                sum += power_spectrum[k] * fb_row[k];
            }
            fbank_data[fi * Z_NUM_BINS + mi] = sum as f32;
        }
    }

    Array2::from_shape_vec((n_frames, Z_NUM_BINS), fbank_data).map_err(Into::into)
}

/// Whisper 特征归一化——公式与 sherpa-onnx `NormalizeWhisperFeatures`（math.cc）完全一致。
///
/// **勿改公式**：曾错误用 `clamped - clamp_min`（范围 0-8）代替 `(clamped + 4) / 4`（范围~0-2），
/// 尺度差 4 倍导致 ONNX 模型输入分布不匹配、输出乱码。
///
/// **调用方式**：流式引擎必须 **per-chunk** 调用（每个 chunk 切片后独立 normalize），
/// 不是对整段特征全局归一化。参考 sherpa-onnx online-recognizer-transducer-impl.h。
pub(crate) fn normalize_whisper_features(chunk: &mut Array2<f32>) {
    let nrows = chunk.nrows();
    let ncols = chunk.ncols();

    // 1. log_spec = torch.clamp(features, min=1e-10).log10()
    for i in 0..nrows {
        for j in 0..ncols {
            chunk[[i, j]] = chunk[[i, j]].max(1e-10f32).log10();
        }
    }

    // 2. Find max_v
    let mut max_v = f32::NEG_INFINITY;
    for i in 0..nrows {
        for j in 0..ncols {
            if chunk[[i, j]] > max_v {
                max_v = chunk[[i, j]];
            }
        }
    }

    // 3. clamp to max_v - 8.0, then (x + 4.0) / 4.0 — 与 sherpa-onnx NormalizeWhisperFeatures 一致
    let clamp_min = max_v - 8.0f32;
    for i in 0..nrows {
        for j in 0..ncols {
            let clamped = chunk[[i, j]].max(clamp_min);
            chunk[[i, j]] = (clamped + 4.0f32) / 4.0f32;
        }
    }
}

// ── Kaldi Fbank features extraction ──


pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = (samples.len() + Z_FRAME_SHIFT / 2) / Z_FRAME_SHIFT;
    let n_frames = n_frames.max(1);

    let fft = &*Z_FFT;

    let n_freqs = Z_FFT_SIZE / 2 + 1;
    let mut fbank_data = vec![0.0f32; n_frames * Z_NUM_BINS];

    for fi in 0..n_frames {
        let midpoint = Z_FRAME_SHIFT * fi + Z_FRAME_SHIFT / 2;
        let wave_start = midpoint as isize - (Z_FRAME_LEN as isize) / 2;

        let mut frame = vec![0.0f32; Z_FRAME_LEN];
        let wave_dim = samples.len() as isize;
        for (s, frame_val) in frame.iter_mut().enumerate().take(Z_FRAME_LEN) {
            let mut s_in_wave = s as isize + wave_start;
            while s_in_wave < 0 || s_in_wave >= wave_dim {
                if s_in_wave < 0 {
                    s_in_wave = -s_in_wave - 1;
                } else {
                    s_in_wave = 2 * wave_dim - 1 - s_in_wave;
                }
            }
            *frame_val = samples[s_in_wave as usize];
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
        let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); Z_FFT_SIZE];
        for j in 0..Z_FFT_SIZE {
            let s = if j < Z_FRAME_LEN {
                preemph[j] * POVEY_WINDOW[j]
            } else {
                0.0
            };
            buf[j] = rustfft::num_complex::Complex::new(s, 0.0);
        }
        fft.process(&mut buf);

        // Pre-compute power spectrum to avoid redundant calculations in the filterbank loop
        let mut power_spectrum = [0.0f64; Z_FFT_SIZE / 2 + 1];
        for k in 0..n_freqs {
            power_spectrum[k] = buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
        }

        for mi in 0..Z_NUM_BINS {
            let mut sum = 0.0f64;
            let fb_row = &MEL_FILTERBANK[mi];
            let (start, end) = MEL_FILTERBANK_RANGE[mi];
            for k in start..end {
                sum += power_spectrum[k] * fb_row[k];
            }
            fbank_data[fi * Z_NUM_BINS + mi] = (sum as f32 + 1.1920929e-7).ln();
        }
    }

    Array2::from_shape_vec((n_frames, Z_NUM_BINS), fbank_data).map_err(Into::into)
}

// povey_window / mel_filterbank_fbank / hz_to_mel / mel_to_hz 已抽取至 feature.rs（C1 修复统一）

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::hf_snapshot;
    use crate::engine::OfflineAsrEngine;

    /// BBPE_TABLE 应对 byte 0-255 全部显式映射。decode_byte_bpe 的 len()==1 兜底
    /// 只覆盖 ASCII 32-126；byte 0-31（控制字符，无法作 str 键，用 chr(N+0x100)）
    /// 与 127-255 必须靠表。补全前 byte 34 (`"`) 唯一缺失（靠兜底"恰好正确"），
    /// 此测试锁死表完整性，防未来再漏任何 byte。
    #[test]
    fn bbpe_table_covers_all_bytes() {
        let covered: std::collections::HashSet<u8> = BBPE_TABLE.values().copied().collect();
        let missing: Vec<u8> = (0u8..=255).filter(|b| !covered.contains(b)).collect();
        assert!(
            missing.is_empty(),
            "BBPE_TABLE 缺少 byte 映射: {:?}（每个 byte 都应有显式键，不依赖 decode 兜底）",
            missing
        );
    }

    /// `"` (byte 34) 即使删掉表项，decode 兜底（ASCII 32-126）也应正确映射——
    /// 双保险：表显式映射 + 兜底 safety net。
    #[test]
    fn decode_byte_bpe_handles_quote() {
        assert_eq!(decode_byte_bpe("\"", false), "\"");
    }

    #[test]
    fn test_clean_decode_utf8() {
        // Test normal ascii
        assert_eq!(clean_decode_utf8(b"hello", true), "hello");
        assert_eq!(clean_decode_utf8(b"hello", false), "hello");

        // Test normal Chinese character "张" (3 bytes: E5 BC A0)
        let zhang = "张";
        let zhang_bytes = zhang.as_bytes();
        assert_eq!(zhang_bytes, &[0xE5, 0xBC, 0xA0]);
        assert_eq!(clean_decode_utf8(zhang_bytes, true), "张");
        assert_eq!(clean_decode_utf8(zhang_bytes, false), "张");

        // Test incomplete trailing Chinese character in streaming mode
        // 1 byte incomplete (E5)
        assert_eq!(clean_decode_utf8(&[0xE5], true), "");
        // 2 bytes incomplete (E5 BC)
        assert_eq!(clean_decode_utf8(&[0xE5, 0xBC], true), "");

        // Test incomplete trailing Chinese character in non-streaming mode (should drop the replacement character)
        assert_eq!(clean_decode_utf8(&[0xE5], false), "");
        assert_eq!(clean_decode_utf8(&[0xE5, 0xBC], false), "");

        // Test mixed text with incomplete character at the end in streaming mode
        // "hello张" with last character incomplete
        let mut mixed = b"hello".to_vec();
        mixed.push(0xE5);
        assert_eq!(clean_decode_utf8(&mixed, true), "hello");
        mixed.push(0xBC);
        assert_eq!(clean_decode_utf8(&mixed, true), "hello");
        mixed.push(0xA0);
        assert_eq!(clean_decode_utf8(&mixed, true), "hello张");

        // Test mixed text with corrupted character in the middle
        // E.g., "hello" + invalid sequence + "world"
        let mut corrupted = b"hello".to_vec();
        corrupted.extend_from_slice(&[0xE5, 0xBC]); // invalid/incomplete middle character
        corrupted.extend_from_slice(b"world");
        // Replacement characters generated in the middle should be removed
        assert_eq!(clean_decode_utf8(&corrupted, true), "helloworld");
        assert_eq!(clean_decode_utf8(&corrupted, false), "helloworld");
    }

    #[test]
    fn test_zipformer_ctc_offline_debug() {
        let snapshot = hf_snapshot("models--k2-fsa--sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13");
        let wav_path = match snapshot {
            Some(s) => s.join("test_wavs/DEV_T0000000000.wav"),
            None => { eprintln!("Skipping: HF snapshot not found"); return; }
        };
        if !wav_path.exists() { eprintln!("Skipping: {} not found", wav_path.display()); return; }
        let samples = crate::audio::read_wav_16k(wav_path.to_str().unwrap()).unwrap();
        
        let cfg = config::load_config().unwrap();
        let zip_cfg = cfg.asr.zipformer.as_ref().unwrap();
        let entry = zip_cfg.get("zipformer-ctc")
            .or_else(|| zip_cfg.get("zipformer-small-ctc"))
            .unwrap();
        let engine = ZipformerCtcEngine::new(entry).unwrap();

        println!("\n--- Debugging Offline zipformer-ctc ---");
        println!("is_whisper: {}, chunk_len: {}, chunk_shift: {}", engine.is_whisper, engine.chunk_len, engine.chunk_shift);
        let mut session = engine.session.lock();
        let my_feats = if engine.is_whisper {
            let mut feats = compute_whisper_features_linear(&samples).unwrap();
            normalize_whisper_features(&mut feats);
            feats
        } else {
            compute_fbank_features(&samples).unwrap()
        };
        let n_frames = my_feats.nrows();
        let mut states = engine.initial_states.clone();
        let mut token_ids = Vec::new();
        let mut prev_id = -1;

        let mut padded_feats = ndarray::Array2::<f32>::zeros((n_frames + engine.chunk_len, Z_NUM_BINS));
        for i in 0..n_frames {
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[i, j]];
            }
        }
        for i in n_frames..(n_frames + engine.chunk_len) {
            let last_idx = if n_frames > 0 { n_frames - 1 } else { 0 };
            for j in 0..Z_NUM_BINS {
                padded_feats[[i, j]] = my_feats[[last_idx, j]];
            }
        }

        let mut frame_idx = 0;
        let mut chunk_idx = 0;
        while frame_idx < n_frames {
            let mut chunk = ndarray::Array2::<f32>::zeros((engine.chunk_len, Z_NUM_BINS));
            for i in 0..engine.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = padded_feats[[frame_idx + i, j]];
                }
            }

            if engine.is_whisper
                && frame_idx == 96 {
                    let first_20: Vec<f32> = (0..20).map(|j| chunk[[4, j]]).collect();
                    println!("Frame 100 first 20 values in Rust: {:?}", first_20);
                }

            let (chunk_vec, _) = chunk.into_raw_vec_and_offset();
            let chunk_input = ndarray::Array3::from_shape_vec(
                (1, engine.chunk_len, Z_NUM_BINS),
                chunk_vec,
            ).unwrap();

            let x_tensor = ort::value::TensorRef::from_array_view(chunk_input.view()).unwrap();
            let mut inputs = ort::inputs! {
                "x" => x_tensor
            };

            let mut state_tensors = Vec::new();
            for (name, val) in &states {
                let t = match val {
                    StateValue::F32(arr) => Tensor::from_array(arr.clone()).unwrap().into_dyn(),
                    StateValue::I64(arr) => Tensor::from_array(arr.clone()).unwrap().into_dyn(),
                };
                state_tensors.push((name.clone(), t));
            }
            for (name, t) in &state_tensors {
                inputs.push((name.as_str().into(), t.into()));
            }

            let outputs = session.run(inputs).unwrap();
            let (log_probs_shape, log_probs_data) = outputs[0].try_extract_tensor::<f32>().unwrap();
            let num_out_frames = log_probs_shape[1] as usize;
            let vocab_dim = log_probs_shape[2] as usize;

            for (name, val) in states.iter_mut() {
                let out_name = format!("new_{}", name);
                if let Some(new_val) = outputs.get(out_name.as_str()) {
                    match val {
                        StateValue::F32(arr) => {
                            let (shape, data) = new_val.try_extract_tensor::<f32>().unwrap();
                            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
                            *arr = ndarray::ArrayD::from_shape_vec(dims, data.to_vec()).unwrap();
                        }
                        StateValue::I64(arr) => {
                            let (shape, data) = new_val.try_extract_tensor::<i64>().unwrap();
                            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
                            *arr = ndarray::ArrayD::from_shape_vec(dims, data.to_vec()).unwrap();
                        }
                    }
                }
            }

            let mut chunk_best_ids = Vec::new();
            for t in 0..num_out_frames {
                let offset = t * vocab_dim;
                let frame_logits = &log_probs_data[offset..offset + vocab_dim];
                let best_id = frame_logits
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                chunk_best_ids.push(best_id);
                if best_id != ZIPFORMER_BLANK_ID && best_id as isize != prev_id {
                    token_ids.push(best_id);
                }
                prev_id = best_id as isize;
            }

            println!("  Chunk {}: best_ids = {:?}", chunk_idx, chunk_best_ids);
            
            frame_idx += engine.chunk_shift;
            chunk_idx += 1;
        }
    }

    #[test]
    fn test_zipformer_transducer_offline() {
        let zh_int8 = match hf_snapshot("models--csukuangfj--sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30") {
            Some(p) => p,
            None => { eprintln!("Skipping: HF snapshot not found"); return; }
        };
        let wav_path = zh_int8.join("test_wavs/0.wav");
        if !wav_path.exists() {
            eprintln!("Skipping transducer test: {} not found", wav_path.display());
            return;
        }

        let samples = crate::audio::read_wav_16k(wav_path.to_str().unwrap()).unwrap();

        let entry = config::ModelEntry {
            source: zh_int8.to_string_lossy().to_string(),
            language: "zh".to_string(),
            secret_key: String::new(),
            source_type: 1,
            is_enabled: true,
                is_available: true,
            is_streaming: true,
            description: "test".to_string(),
        };

        let engine = ZipformerTransducerEngine::new(&entry).unwrap();
        let text = engine.transcribe(&samples, "zh").unwrap();
        println!("\n--- Zipformer Transducer (zh-int8) Result ---");
        println!("  text = {:?}", text);
        assert!(!text.is_empty(), "transducer should produce non-empty output");
    }

    #[test]
    fn test_zipformer_transducer_xlarge() {
        let xlarge = match hf_snapshot("models--csukuangfj--sherpa-onnx-streaming-zipformer-zh-xlarge-int8-2025-06-30") {
            Some(p) => p,
            None => { eprintln!("Skipping: HF snapshot not found"); return; }
        };
        let wav_path = xlarge.join("test_wavs/0.wav");
        if !wav_path.exists() {
            eprintln!("Skipping xlarge test: {} not found", wav_path.display());
            return;
        }

        let samples = crate::audio::read_wav_16k(wav_path.to_str().unwrap()).unwrap();

        let entry = config::ModelEntry {
            source: xlarge.to_string_lossy().to_string(),
            language: "zh".to_string(),
            secret_key: String::new(),
            source_type: 1,
            is_enabled: true,
                is_available: true,
            is_streaming: true,
            description: "test".to_string(),
        };

        let engine = ZipformerTransducerEngine::new(&entry).unwrap();
        let text = engine.transcribe(&samples, "zh").unwrap();
        println!("\n--- Zipformer Transducer (xlarge) Result ---");
        println!("  text = {:?}", text);
        assert!(!text.is_empty(), "transducer xlarge should produce non-empty output");
    }
}

