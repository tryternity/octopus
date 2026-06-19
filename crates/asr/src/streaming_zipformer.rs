//! Streaming Zipformer ASR — chunk-by-chunk CTC inference with stateful caches.
//!
//! Unlike the offline `transcribe()` which processes all audio at once,
//! this module exposes a `StreamingZipformer` struct that accepts audio
//! incrementally and produces partial recognition results after each chunk.

use anyhow::{Context, Result};
use ndarray::{Array2, ArrayD};
use ort::session::Session;
use ort::value::{Tensor, TensorElementType};

use crate::config;
use crate::zipformer::{
    clean_decode_utf8, compute_fbank_features, decode_byte_bpe,
    discover_streaming_zipformer_onnx, is_vocab_bbpe, StateValue, Z_FRAME_SHIFT,
    Z_NUM_BINS, ZIPFORMER_BLANK_ID,
};

/// Streaming Zipformer engine — maintains state across chunks.
pub struct StreamingZipformer {
    session: Session,
    vocab: Vec<String>,
    is_bbpe: bool,
    is_whisper: bool,

    // Chunk parameters (read from model metadata)
    chunk_len: usize,   // T (e.g. 77 or 45)
    chunk_shift: usize,  // decode_chunk_len (e.g. 64 or 32)

    // Streaming state
    sample_buffer: Vec<f32>,
    history_samples: Vec<f32>,
    states: Vec<(String, StateValue)>,
    token_ids: Vec<usize>,
    prev_id: isize,
}

impl StreamingZipformer {
    /// Create a new streaming engine for the given model name (e.g. "zipformer-small-ctc").
    pub fn new(engine_name: &str) -> Result<Self> {
        let cfg = config::load_config()?;

        // DB zipformer section 查找；section 缺失时用本地打包兜底（DEFAULT_ASR_MODEL_DIR），
        // 与 config::fallback_engine 一致——兜底引擎 zipformer-small-ctc 随应用打包，
        // DB 缺条目（旧 schema / 人为删除）时仍可用
        let entry_owned;
        let entry = if let Some(zip_cfg) = cfg.asr.zipformer.as_ref() {
            if let Some(e) = zip_cfg.get(engine_name) {
                e
            } else {
                entry_owned = zip_cfg
                    .iter()
                    .next()
                    .map(|(_, v)| v.clone())
                    .context("No zipformer model entries")?;
                &entry_owned
            }
        } else {
            // DB 无 zipformer section：硬构造兜底（本地打包路径）
            entry_owned = octopus_infra::db::ModelEntry {
                source: octopus_infra::consts::DEFAULT_ASR_MODEL_DIR.to_string(),
                language: "zh".to_string(),
                description: String::new(),
                secret_key: String::new(),
                is_local: true,
                is_enabled: true,
                is_streaming: true,
            };
            &entry_owned
        };

        let hf_path = config::resolve_model_dir(&entry.source)?;
        let model_path = discover_streaming_zipformer_onnx(&hf_path)?;

        let session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&model_path)?;

        // Read chunk parameters from model metadata
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

        // Initialize states from ONNX input shapes
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
                let is_int64 = matches!(
                    input.dtype().tensor_type(),
                    Some(TensorElementType::Int64)
                );
                if is_int64 {
                    states.push((name.to_string(), StateValue::I64(ArrayD::<i64>::zeros(dims))));
                } else {
                    states.push((name.to_string(), StateValue::F32(ArrayD::<f32>::zeros(dims))));
                }
            }
        }

        // Load vocabulary
        let vocab = load_vocab(&hf_path)?;
        let is_bbpe = is_vocab_bbpe(&vocab);

        Ok(Self {
            session,
            vocab,
            is_bbpe,
            is_whisper,
            chunk_len,
            chunk_shift,
            sample_buffer: Vec::new(),
            history_samples: Vec::new(),
            states,
            token_ids: Vec::new(),
            prev_id: -1,
        })
    }

    /// Feed audio samples (16kHz mono f32) into the engine.
    /// Returns `Some(text)` with partial (accumulated) result if new tokens were produced.
    pub fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>> {
        self.sample_buffer.extend_from_slice(samples);
        self.process_chunks()
    }

    /// Flush any remaining buffered audio. Call when recording stops.
    pub fn finish(&mut self) -> Result<String> {
        let samples = std::mem::take(&mut self.sample_buffer);
        if samples.is_empty() && self.history_samples.is_empty() {
            return Ok(self.decode_tokens(false));
        }

        let mut input_samples = Vec::with_capacity(self.history_samples.len() + samples.len());
        input_samples.extend_from_slice(&self.history_samples);
        input_samples.extend_from_slice(&samples);

        let feats = if self.is_whisper {
            crate::zipformer::compute_whisper_features_linear(&input_samples)?
        } else {
            compute_fbank_features(&input_samples)?
        };
        let h_frames = self.history_samples.len() / Z_FRAME_SHIFT;
        let n_frames = feats.nrows().saturating_sub(h_frames);

        if n_frames == 0 {
            self.history_samples.clear();
            return Ok(self.decode_tokens(false));
        }

        // Run extra chunks of padding to fully flush the model's receptive field / right context
        let num_extra_chunks = 3;
        let pad_len = self.chunk_len + num_extra_chunks * self.chunk_shift;
        let mut padded = Array2::<f32>::zeros((feats.nrows() + pad_len, Z_NUM_BINS));
        for i in 0..feats.nrows() {
            for j in 0..Z_NUM_BINS {
                padded[[i, j]] = feats[[i, j]];
            }
        }
        let last = if feats.nrows() > 0 { feats.nrows() - 1 } else { 0 };
        for i in feats.nrows()..(feats.nrows() + pad_len) {
            for j in 0..Z_NUM_BINS {
                padded[[i, j]] = feats[[last, j]];
            }
        }

        let mut frame_idx = 0;
        let limit = n_frames + num_extra_chunks * self.chunk_shift;
        while frame_idx < limit {
            let mut chunk = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
            for i in 0..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = padded[[frame_idx + h_frames + i, j]];
                }
            }

            if self.is_whisper {
                crate::zipformer::normalize_whisper_features(&mut chunk);
            }

            self.run_chunk(&chunk)?;
            frame_idx += self.chunk_shift;
        }

        self.history_samples.clear();
        Ok(self.decode_tokens(false))
    }

    /// Active flush: pad the current sample buffer with enough zeros
    /// to force processing of the lookahead / right context of any remaining audio.
    pub fn flush(&mut self) -> Result<Option<String>> {
        let h_frames = self.history_samples.len() / Z_FRAME_SHIFT;
        let required_total_samples = (h_frames + self.chunk_len + 1) * Z_FRAME_SHIFT;
        let current_total_samples = self.history_samples.len() + self.sample_buffer.len();

        if current_total_samples < required_total_samples {
            let needed = required_total_samples - current_total_samples;
            self.sample_buffer.resize(self.sample_buffer.len() + needed, 0.0);
        }

        self.process_chunks()
    }


    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.sample_buffer.clear();
        self.history_samples.clear();
        self.token_ids.clear();
        self.prev_id = -1;
        // Reset states to zeros
        for (_, val) in &mut self.states {
            match val {
                StateValue::F32(arr) => *arr = ArrayD::<f32>::zeros(arr.shape()),
                StateValue::I64(arr) => *arr = ArrayD::<i64>::zeros(arr.shape()),
            }
        }
    }

    /// Process as many full chunks as the sample buffer allows.
    fn process_chunks(&mut self) -> Result<Option<String>> {
        if self.sample_buffer.is_empty() {
            return self.decoded_current();
        }

        let mut input_samples = Vec::with_capacity(self.history_samples.len() + self.sample_buffer.len());
        input_samples.extend_from_slice(&self.history_samples);
        input_samples.extend_from_slice(&self.sample_buffer);

        let feats = if self.is_whisper {
            crate::zipformer::compute_whisper_features_linear(&input_samples)?
        } else {
            compute_fbank_features(&input_samples)?
        };

        let h_frames = self.history_samples.len() / Z_FRAME_SHIFT;

        // We only process chunks where we have enough real audio frames.
        // The condition for chunk `frame_idx` to have all real frames is:
        // `frame_idx + h_frames + chunk_len < feats.nrows()` (matching C++ sherpa-onnx `IsReady`).
        // If the first chunk is not ready, we return Ok(None) to wait for more audio.
        if h_frames + self.chunk_len >= feats.nrows() {
            return self.decoded_current();
        }

        // Process chunks
        let mut frame_idx = 0;
        while frame_idx + h_frames + self.chunk_len < feats.nrows() {
            let mut chunk = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
            for i in 0..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = feats[[frame_idx + h_frames + i, j]];
                }
            }

            if self.is_whisper {
                crate::zipformer::normalize_whisper_features(&mut chunk);
            }

            self.run_chunk(&chunk)?;
            frame_idx += self.chunk_shift;
        }

        // Consume the processed samples
        let consumed_samples = frame_idx * Z_FRAME_SHIFT;

        // Save the last 160 samples of the consumed audio as history
        let h_len = self.history_samples.len();
        let consumed_limit = (h_len + consumed_samples).min(input_samples.len());
        if consumed_limit >= Z_FRAME_SHIFT {
            self.history_samples = input_samples[consumed_limit - Z_FRAME_SHIFT .. consumed_limit].to_vec();
        } else if !input_samples.is_empty() {
            self.history_samples = input_samples[input_samples.len().saturating_sub(Z_FRAME_SHIFT)..].to_vec();
        }

        if consumed_samples < self.sample_buffer.len() {
            self.sample_buffer = self.sample_buffer[consumed_samples..].to_vec();
        } else {
            self.sample_buffer.clear();
        }

        // 始终返回当前累积段文本（见 decoded_current 说明）。
        self.decoded_current()
    }

    /// Run one chunk through the model. Returns true if new tokens were produced.
    fn run_chunk(&mut self, chunk: &Array2<f32>) -> Result<bool> {
        let (chunk_vec, _) = chunk.clone().into_raw_vec_and_offset();
        let chunk_input = ndarray::Array3::from_shape_vec(
            (1, self.chunk_len, Z_NUM_BINS),
            chunk_vec,
        )?;

        let x_tensor = ort::value::TensorRef::from_array_view(chunk_input.view())?;

        let mut inputs = ort::inputs! {
            "x" => x_tensor
        };

        // Feed current states
        let mut state_tensors = Vec::new();
        for (name, val) in &self.states {
            let t = match val {
                StateValue::F32(arr) => Tensor::from_array(arr.clone())?.into_dyn(),
                StateValue::I64(arr) => Tensor::from_array(arr.clone())?.into_dyn(),
            };
            state_tensors.push((name.clone(), t));
        }
        for (name, t) in &state_tensors {
            inputs.push((name.as_str().into(), t.into()));
        }

        let outputs = self.session.run(inputs)?;

        // Extract log_probs [1, num_out_frames, vocab_dim]
        let (log_probs_shape, log_probs_data) = outputs[0].try_extract_tensor::<f32>()?;
        let num_out_frames = log_probs_shape[1] as usize;
        let vocab_dim = log_probs_shape[2] as usize;

        // Update states from new_* outputs
        for (name, val) in self.states.iter_mut() {
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

        // Greedy CTC decoding
        let mut produced = false;
        for t in 0..num_out_frames {
            let offset = t * vocab_dim;
            let frame_logits = &log_probs_data[offset..offset + vocab_dim];
            let best_id = frame_logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0);
            if best_id != ZIPFORMER_BLANK_ID && best_id as isize != self.prev_id {
                self.token_ids.push(best_id);
                produced = true;
            }
            self.prev_id = best_id as isize;
        }

        Ok(produced)
    }

    /// Decode accumulated token_ids into text.
    fn decode_tokens(&self, is_streaming: bool) -> String {
        if self.token_ids.is_empty() {
            return String::new();
        }

        if self.is_bbpe {
            let mut raw = String::new();
            for &tid in &self.token_ids {
                if tid < self.vocab.len() {
                    raw.push_str(&self.vocab[tid]);
                }
            }
            decode_byte_bpe(&raw, is_streaming)
        } else {
            // Check for BPE with byte fallback (standard SentencePiece behavior)
            let mut is_bpe_with_byte_fallback = false;
            let mut id_for_0x00 = 0;

            let pos_00 = self.vocab.iter().position(|t| t == "<0x00>");
            let pos_ff = self.vocab.iter().position(|t| t == "<0xFF>");
            if let (Some(p00), Some(pff)) = (pos_00, pos_ff) {
                if pff > p00 && pff - p00 == 255 {
                    is_bpe_with_byte_fallback = true;
                    id_for_0x00 = p00;
                }
            }

            let mut bytes: Vec<u8> = Vec::new();

            for &tid in &self.token_ids {
                if tid >= self.vocab.len() {
                    continue;
                }
                let mut token = self.vocab[tid].clone();

                // Decode BPE with byte fallback (translates to raw byte)
                if is_bpe_with_byte_fallback
                    && token.len() == 6
                    && token.starts_with("<0x")
                    && token.ends_with('>')
                {
                    if tid >= id_for_0x00 && tid <= id_for_0x00 + 255 {
                        if let Ok(hex_val) = u8::from_str_radix(&token[3..5], 16) {
                            if hex_val == (tid - id_for_0x00) as u8 {
                                bytes.push(hex_val);
                                continue;
                            }
                        }
                    }
                }

                // For BPE-based models, we replace ▁ (U+2581, utf8 \xe2\x96\x81) with a space " "
                if token.len() >= 3 && token.starts_with('▁') {
                    token = format!(" {}", &token[3..]);
                }

                bytes.extend_from_slice(token.as_bytes());
            }

            let decoded = clean_decode_utf8(&bytes, is_streaming);
            decoded.trim().to_string()
        }
    }

    /// 当前已识别的段文本（token_ids 非空时 Some，空则 None）。
    /// process_chunks 各早退路径统一用它，避免「样本不够凑 chunk 时返回 None、上层丢失
    /// current_segment 只回 accumulated」导致的长短态逐帧交替闪烁。
    fn decoded_current(&self) -> Result<Option<String>> {
        let t = self.decode_tokens(true);
        Ok(if t.is_empty() { None } else { Some(t) })
    }
}

// ── Helpers ──

fn load_vocab(hf_path: &std::path::Path) -> Result<Vec<String>> {
    let tokens_path = hf_path.join("tokens.txt");
    let text = std::fs::read_to_string(&tokens_path)
        .with_context(|| format!("tokens.txt not found at {}", tokens_path.display()))?;

    let mut vocab: Vec<String> = Vec::new();
    for line in text.lines() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_zipformer_ctc() {
        let wav_path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join(".cache/huggingface/hub/models--k2-fsa--sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13/snapshots/cfa1a89c049cd0c48fb9e46a49c84b58744daec5/test_wavs/DEV_T0000000000.wav");
        let samples = crate::audio::read_wav_16k(wav_path.to_str().unwrap()).unwrap();

        println!("\n--- Testing Streaming zipformer-ctc ---");
        let mut engine = StreamingZipformer::new("zipformer-ctc").unwrap();
        let chunk_size = 10000;
        for chunk in samples.chunks(chunk_size) {
            if let Some(text) = engine.accept_samples(chunk).unwrap() {
                println!("Partial: {}", text);
            }
        }
        let final_text = engine.finish().unwrap();
        println!("Final: {}", final_text);
        assert!(!final_text.is_empty(), "Transcribed text should not be empty");
    }

    #[test]
    fn test_streaming_zipformer_multi() {
        let wav_path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join(".cache/huggingface/hub/models--k2-fsa--sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13/snapshots/cfa1a89c049cd0c48fb9e46a49c84b58744daec5/test_wavs/DEV_T0000000000.wav");
        let samples = crate::audio::read_wav_16k(wav_path.to_str().unwrap()).unwrap();

        println!("\n--- Testing Streaming zipformer-multi ---");
        let mut engine = StreamingZipformer::new("zipformer-multi").unwrap();
        let chunk_size = 10000;
        for chunk in samples.chunks(chunk_size) {
            if let Some(text) = engine.accept_samples(chunk).unwrap() {
                println!("Partial: {}", text);
            }
        }
        let final_text = engine.finish().unwrap();
        println!("Final: {}", final_text);
        assert!(!final_text.is_empty(), "Transcribed text should not be empty");
    }
}
