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
    compute_fbank_features, decode_byte_bpe, StateValue,
    Z_FRAME_SHIFT, Z_NUM_BINS, ZIPFORMER_BLANK_ID,
};

/// Streaming Zipformer engine — maintains state across chunks.
pub struct StreamingZipformer {
    session: Session,
    vocab: Vec<String>,
    is_bbpe: bool,

    // Chunk parameters (read from model metadata)
    chunk_len: usize,   // T (e.g. 77 or 45)
    chunk_shift: usize,  // decode_chunk_len (e.g. 64 or 32)

    // Streaming state
    sample_buffer: Vec<f32>,
    states: Vec<(String, StateValue)>,
    token_ids: Vec<usize>,
    prev_id: isize,
}

impl StreamingZipformer {
    /// Create a new streaming engine for the given model name (e.g. "zipformer-small-ctc").
    pub fn new(engine_name: &str) -> Result<Self> {
        let cfg = config::load_config()?;
        let zip_cfg = cfg
            .asr
            .zipformer
            .as_ref()
            .context("No zipformer models in config")?;

        let entry = if let Some(e) = zip_cfg.get(engine_name) {
            e
        } else {
            zip_cfg
                .iter()
                .next()
                .map(|(_, v)| v)
                .context("No zipformer model entries")?
        };

        let hf_path = config::find_hf_cache(&entry.source)?;
        let model_path = if hf_path.join("model.int8.onnx").exists() {
            hf_path.join("model.int8.onnx")
        } else if hf_path.join("model.onnx").exists() {
            hf_path.join("model.onnx")
        } else {
            anyhow::bail!(
                "model.onnx / model.int8.onnx not found at {}",
                hf_path.display()
            );
        };

        let session = Session::builder()?.commit_from_file(&model_path)?;

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
        let is_bbpe = vocab.iter().any(|tok| tok.starts_with('▁'));

        Ok(Self {
            session,
            vocab,
            is_bbpe,
            chunk_len,
            chunk_shift,
            sample_buffer: Vec::new(),
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
        if self.sample_buffer.is_empty() {
            return Ok(self.decode_tokens());
        }

        // Pad remaining samples to produce at least chunk_len fbank frames
        let samples = std::mem::take(&mut self.sample_buffer);
        let feats = compute_fbank_features(&samples)?;
        let n_frames = feats.nrows();

        if n_frames == 0 {
            return Ok(self.decode_tokens());
        }

        // Pad to chunk_len
        let mut padded = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
        for i in 0..self.chunk_len.min(n_frames) {
            for j in 0..Z_NUM_BINS {
                padded[[i, j]] = feats[[i, j]];
            }
        }
        // Repeat last frame for padding
        if n_frames < self.chunk_len {
            let last = if n_frames > 0 { n_frames - 1 } else { 0 };
            for i in n_frames..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    padded[[i, j]] = feats[[last, j]];
                }
            }
        }

        self.run_chunk(&padded)?;
        Ok(self.decode_tokens())
    }

    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.sample_buffer.clear();
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
        // Compute fbank for all buffered samples
        if self.sample_buffer.is_empty() {
            return Ok(None);
        }

        let feats = compute_fbank_features(&self.sample_buffer)?;
        let n_frames = feats.nrows();

        if n_frames < self.chunk_len {
            // Not enough frames for a chunk yet
            return Ok(None);
        }

        // Pad features for safe chunk extraction
        let mut padded = Array2::<f32>::zeros((n_frames + self.chunk_len, Z_NUM_BINS));
        for i in 0..n_frames {
            for j in 0..Z_NUM_BINS {
                padded[[i, j]] = feats[[i, j]];
            }
        }
        let last = if n_frames > 0 { n_frames - 1 } else { 0 };
        for i in n_frames..(n_frames + self.chunk_len) {
            for j in 0..Z_NUM_BINS {
                padded[[i, j]] = feats[[last, j]];
            }
        }

        // Process chunks
        let mut frame_idx = 0;
        let mut produced_any = false;
        while frame_idx + self.chunk_len <= n_frames + self.chunk_len && frame_idx < n_frames {
            let mut chunk = Array2::<f32>::zeros((self.chunk_len, Z_NUM_BINS));
            for i in 0..self.chunk_len {
                for j in 0..Z_NUM_BINS {
                    chunk[[i, j]] = padded[[frame_idx + i, j]];
                }
            }

            if self.run_chunk(&chunk)? {
                produced_any = true;
            }
            frame_idx += self.chunk_shift;
        }

        // Consume the processed samples
        // Samples needed for processed frames: approximately frame_idx * Z_FRAME_SHIFT
        let consumed_samples = frame_idx * Z_FRAME_SHIFT;
        if consumed_samples < self.sample_buffer.len() {
            self.sample_buffer = self.sample_buffer[consumed_samples..].to_vec();
        } else {
            self.sample_buffer.clear();
        }

        if produced_any {
            Ok(Some(self.decode_tokens()))
        } else {
            Ok(None)
        }
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
    fn decode_tokens(&self) -> String {
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
            decode_byte_bpe(&raw)
        } else {
            let mut decoded = String::new();
            for &tid in &self.token_ids {
                if tid < self.vocab.len() {
                    let token = &self.vocab[tid];
                    if token.starts_with('▁') {
                        if !decoded.is_empty() {
                            decoded.push(' ');
                        }
                        decoded.push_str(&token[3..]); // Strip ▁ (3 bytes)
                    } else {
                        decoded.push_str(token);
                    }
                }
            }
            decoded.trim().to_string()
        }
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
