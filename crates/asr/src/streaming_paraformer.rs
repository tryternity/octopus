//! Streaming Paraformer ASR — chunk-by-chunk inference with stateful CIF + decoder caches.
//!
//! Based on sherpa-onnx's `online-recognizer-paraformer-impl.h`.

use anyhow::{Context, Result};
use ort::session::Session;

use crate::config;
use crate::paraformer::{
    apply_lfr, compute_fbank, decode_tokens, extract_cmvn_from_metadata,
    FBANK_FRAME_LEN, FBANK_FRAME_SHIFT, FBANK_NUM_BINS,
    LFR_WINDOW_SHIFT, LFR_WINDOW_SIZE,
};

// ── Streaming chunk parameters (from sherpa-onnx) ──
const CHUNK_SIZE: usize = 61; // fbank frames per chunk (~0.61s)
const LEFT_CHUNK_SIZE: usize = 5; // left context overlap in LFR frames
const RIGHT_CHUNK_SIZE: usize = 3; // right context overlap in LFR frames

// ── Samples needed for one full chunk ──
// First fbank frame needs FBANK_FRAME_LEN samples, each subsequent frame adds FBANK_FRAME_SHIFT.
const CHUNK_SAMPLES: usize = FBANK_FRAME_LEN + (CHUNK_SIZE - 1) * FBANK_FRAME_SHIFT; // 10,000



/// Streaming Paraformer engine — maintains state across chunks.
pub struct StreamingParaformer {
    // ONNX sessions
    encoder_session: Session,
    decoder_session: Session,

    // Model metadata
    neg_mean: Vec<f32>,
    inv_stddev: Vec<f32>,
    encoder_output_size: usize, // 512
    feat_dim: usize,            // 560 (= FBANK_NUM_BINS * LFR_WINDOW_SIZE)
    decoder_num_blocks: usize,  // 16
    decoder_kernel_size: usize, // 11 → cache_time = 10
    vocab: Vec<String>,

    // Streaming state (carried across chunks)
    sample_buffer: Vec<f32>,                      // accumulated raw 16kHz samples
    feat_cache: Vec<f32>,                         // [8 * 560] overlap buffer
    encoder_out_cache: Vec<f32>,                  // [512] CIF hidden accumulator
    alpha_cache: f32,                             // CIF integrate accumulator
    decoder_caches: Vec<ndarray::Array3<f32>>,    // 16 × [1, 512, cache_time]
    num_processed_frames: i32,                    // fbank frame counter (in LFR space)
}

impl StreamingParaformer {
    /// Create a new streaming engine for the given model name (e.g. "paraformer-streaming").
    /// Loads ONNX sessions and vocabulary; initializes all state to zeros.
    pub fn new(engine_name: &str) -> Result<Self> {
        let cfg = config::load_config()?;

        let para_cfg = cfg
            .asr
            .paraformer
            .as_ref()
            .context("No paraformer models in config")?;
        let entry = if let Some(e) = para_cfg.get(engine_name) {
            e
        } else {
            para_cfg
                .iter()
                .next()
                .map(|(_, v)| v)
                .context("No paraformer model entries")?
        };

        let hf_path = config::resolve_model_dir(&entry.source)?;

        let prefer_int8 = true;

        let encoder_path = discover_onnx(&hf_path, "encoder", prefer_int8)?;
        let decoder_path = discover_onnx(&hf_path, "decoder", prefer_int8)?;

        let encoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&encoder_path)?;
        let decoder_session = crate::config::apply_session_acceleration(Session::builder()?)?.commit_from_file(&decoder_path)?;

        // Extract metadata — read everything before moving sessions into the struct
        let (neg_mean, inv_stddev, encoder_output_size) =
            extract_cmvn_from_metadata(&encoder_session)?;

        // Read decoder_num_blocks and decoder_kernel_size from encoder metadata
        let metadata = encoder_session.metadata()?;
        let decoder_num_blocks_str = metadata
            .custom("decoder_num_blocks")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "16".into());
        let decoder_kernel_size_str = metadata
            .custom("decoder_kernel_size")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "11".into());
        // Drop metadata before moving encoder_session
        drop(metadata);

        let decoder_num_blocks: usize = decoder_num_blocks_str.parse().unwrap_or(16);
        let decoder_kernel_size: usize = decoder_kernel_size_str.parse().unwrap_or(11);

        let cache_time = decoder_kernel_size - 1; // 10
        let feat_dim = FBANK_NUM_BINS * LFR_WINDOW_SIZE; // 560

        // Load vocabulary
        let vocab = load_vocab(&hf_path)?;

        // Initialize decoder caches
        let decoder_caches = (0..decoder_num_blocks)
            .map(|_| ndarray::Array3::<f32>::zeros((1, encoder_output_size, cache_time)))
            .collect();

        Ok(Self {
            encoder_session,
            decoder_session,
            neg_mean,
            inv_stddev,
            encoder_output_size,
            feat_dim,
            decoder_num_blocks,
            decoder_kernel_size,
            vocab,
            sample_buffer: Vec::new(),
            feat_cache: vec![0.0; (LEFT_CHUNK_SIZE + RIGHT_CHUNK_SIZE) * feat_dim],
            encoder_out_cache: vec![0.0; encoder_output_size],
            alpha_cache: 0.0,
            decoder_caches,
            num_processed_frames: 0,
        })
    }

    /// Feed audio samples (16kHz mono f32) into the engine.
    /// Returns `Some(text)` if the chunk produced recognition results, `None` otherwise.
    /// Call this repeatedly as audio arrives (~600ms chunks).
    pub fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>> {
        self.sample_buffer.extend_from_slice(samples);

        // Process as many full chunks as available
        let mut accumulated_text = String::new();
        while self.sample_buffer.len() >= CHUNK_SAMPLES {
            let chunk_samples: Vec<f32> = self.sample_buffer.drain(..CHUNK_SAMPLES).collect();
            if let Some(text) = self.process_chunk(&chunk_samples)? {
                accumulated_text.push_str(&text);
            }
        }

        if accumulated_text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(accumulated_text))
        }
    }

    /// Flush any remaining buffered audio. Call when recording stops.
    /// Pads short final chunk with zeros to CHUNK_SIZE fbank frames.
    pub fn finish(&mut self) -> Result<String> {
        if self.sample_buffer.is_empty() {
            return Ok(String::new());
        }

        // Process whatever is left as a final (possibly short) chunk
        let remaining = std::mem::take(&mut self.sample_buffer);
        let result = self.process_final_chunk(&remaining)?;
        Ok(result.unwrap_or_default())
    }

    /// Active flush: pad the current sample buffer with zeros to CHUNK_SAMPLES
    /// to force processing of the lookahead / right context of the tail speech frames.
    pub fn flush(&mut self) -> Result<Option<String>> {
        let needed = CHUNK_SAMPLES.saturating_sub(self.sample_buffer.len());
        if needed > 0 {
            self.sample_buffer.resize(CHUNK_SAMPLES, 0.0);
        }

        let mut accumulated_text = String::new();
        while self.sample_buffer.len() >= CHUNK_SAMPLES {
            let chunk_samples: Vec<f32> = self.sample_buffer.drain(..CHUNK_SAMPLES).collect();
            if let Some(text) = self.process_chunk(&chunk_samples)? {
                accumulated_text.push_str(&text);
            }
        }

        if accumulated_text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(accumulated_text))
        }
    }


    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.sample_buffer.clear();
        self.feat_cache.fill(0.0);
        self.encoder_out_cache.fill(0.0);
        self.alpha_cache = 0.0;
        let cache_time = self.decoder_kernel_size - 1;
        for cache in &mut self.decoder_caches {
            *cache = ndarray::Array3::<f32>::zeros((1, self.encoder_output_size, cache_time));
        }
        self.num_processed_frames = 0;
    }

    /// Process a full chunk (exactly CHUNK_SAMPLES samples).
    fn process_chunk(&mut self, samples: &[f32]) -> Result<Option<String>> {
        // 1. Fbank → LFR → CMVN
        let mut features = self.extract_features(samples)?;

        // 2. Positional encoding
        self.apply_positional_encoding(&mut features);

        // 3. Prepend feat_cache, update feat_cache
        let combined = self.apply_feat_overlap(features)?;

        // 4. Encoder
        let (enc_tensor, enc_len_scalar, alphas) = self.run_encoder(&combined)?;

        // 5. Zero overlap alphas
        let alphas = self.mask_alphas(alphas, enc_len_scalar);

        // 6. Stateful CIF
        let acoustic = self.run_cif(&enc_tensor, enc_len_scalar, &alphas)?;
        if acoustic.is_empty() {
            // Advance frame counter
            self.num_processed_frames += (CHUNK_SIZE - 1) as i32;
            return Ok(None);
        }
        let num_tokens = acoustic.len() / self.encoder_output_size;

        // 7. Stateful decoder
        let sample_ids = self.run_decoder(&enc_tensor, enc_len_scalar, &acoustic, num_tokens)?;

        // 8. Decode tokens
        let text = decode_tokens(&sample_ids, &self.vocab);

        // 9. Advance frame counter (overlap by 1 fbank frame)
        self.num_processed_frames += (CHUNK_SIZE - 1) as i32;

        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// Process a final (possibly short) chunk. Pads to CHUNK_SIZE fbank frames.
    fn process_final_chunk(&mut self, samples: &[f32]) -> Result<Option<String>> {
        // Pad to at least CHUNK_SAMPLES so we get CHUNK_SIZE fbank frames
        let mut padded = samples.to_vec();
        if padded.len() < CHUNK_SAMPLES {
            padded.resize(CHUNK_SAMPLES, 0.0);
        }
        self.process_chunk(&padded)
    }

    /// Fbank → LFR → CMVN normalization
    fn extract_features(&self, samples: &[f32]) -> Result<ndarray::Array2<f32>> {
        let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
        let fbank = compute_fbank(&scaled)?;

        // Pad fbank to CHUNK_SIZE frames if short
        let n_frames = fbank.nrows();
        let fbank = if n_frames < CHUNK_SIZE {
            let mut padded = ndarray::Array2::zeros((CHUNK_SIZE, FBANK_NUM_BINS));
            padded.slice_mut(ndarray::s![..n_frames, ..]).assign(&fbank);
            padded
        } else {
            fbank
        };

        let lfr = apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);

        // CMVN normalization (same as offline)
        let (n_rows, n_cols) = (lfr.nrows(), lfr.ncols());
        let mut features = lfr;
        for i in 0..n_rows {
            for j in 0..n_cols {
                if j < self.neg_mean.len() && j < self.inv_stddev.len() {
                    features[[i, j]] = (features[[i, j]] + self.neg_mean[j]) * self.inv_stddev[j];
                }
            }
        }

        Ok(features)
    }

    /// Apply sinusoidal positional encoding with t_offset from num_processed_frames.
    /// This is critical for streaming — without it the model produces garbage after the first chunk.
    fn apply_positional_encoding(&self, features: &mut ndarray::Array2<f32>) {
        let half_dim = self.feat_dim / 2;
        // k_scale = ln(10000) / (half_dim - 1)
        let k_scale = 10000.0f32.ln() / (half_dim - 1) as f32;
        let t_offset = self.num_processed_frames as f32 / LFR_WINDOW_SHIFT as f32;

        let n_frames = features.nrows();
        for t in 0..n_frames {
            let pos = (t as f32 + 1.0 + t_offset) as f64;
            for d in 0..half_dim {
                let inv_timescale = pos * ((d as f64) * k_scale as f64).exp();
                features[[t, d]] += inv_timescale.sin() as f32;
                features[[t, d + half_dim]] += inv_timescale.cos() as f32;
            }
        }
    }

    /// Prepend feat_cache to features, save last (left+right) frames back.
    /// Returns combined features [cache_rows + chunk_rows, feat_dim].
    fn apply_feat_overlap(
        &mut self,
        features: ndarray::Array2<f32>,
    ) -> Result<ndarray::Array2<f32>> {
        let n_chunk = features.nrows();
        let cache_rows = LEFT_CHUNK_SIZE + RIGHT_CHUNK_SIZE; // 8

        // Reshape feat_cache into [cache_rows, feat_dim]
        let cache_arr = ndarray::Array2::from_shape_vec(
            (cache_rows, self.feat_dim),
            self.feat_cache.clone(),
        )?;

        // Concatenate: [cache | chunk]
        let mut combined = ndarray::Array2::zeros((cache_rows + n_chunk, self.feat_dim));
        combined
            .slice_mut(ndarray::s![..cache_rows, ..])
            .assign(&cache_arr);
        combined
            .slice_mut(ndarray::s![cache_rows.., ..])
            .assign(&features);

        // Save last (left+right) rows back to feat_cache
        let total_rows = combined.nrows();
        let save_start = total_rows.saturating_sub(cache_rows);
        let new_cache: Vec<f32> = combined
            .slice(ndarray::s![save_start..total_rows, ..])
            .iter()
            .cloned()
            .collect();
        self.feat_cache = new_cache;

        Ok(combined)
    }

    /// Run the encoder ONNX session.
    fn run_encoder(
        &mut self,
        features: &ndarray::Array2<f32>,
    ) -> Result<(ndarray::Array3<f32>, usize, Vec<f32>)> {
        let (n_rows, n_cols) = (features.nrows(), features.ncols());
        let flat: Vec<f32> = features.iter().cloned().collect();
        let speech_tensor =
            ndarray::Array3::from_shape_vec((1, n_rows, n_cols), flat)?;
        let speech_lengths = ndarray::Array1::from_vec(vec![n_rows as i32]);

        let outputs = self.encoder_session.run(ort::inputs! {
            "speech" => ort::value::TensorRef::from_array_view(speech_tensor.view())?,
            "speech_lengths" => ort::value::TensorRef::from_array_view(speech_lengths.view())?
        })?;

        // enc [1, T', 512]
        let (enc_shape, enc_data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: Vec<usize> = enc_shape.iter().map(|&d| d as usize).collect();
        let enc_len = dims[1];
        let enc_feat = dims[2];
        let enc_tensor =
            ndarray::Array3::from_shape_vec((1, enc_len, enc_feat), enc_data.to_vec())?;

        // enc_len [1]
        let (_, enc_len_data) = outputs[1].try_extract_tensor::<i32>()?;
        let enc_len_scalar = enc_len_data[0] as usize;

        // alphas [1, T']
        let (_, alpha_data) = outputs[2].try_extract_tensor::<f32>()?;
        let alphas: Vec<f32> = alpha_data.to_vec();

        Ok((enc_tensor, enc_len_scalar, alphas))
    }

    /// Zero out alphas in the left/right overlap regions.
    fn mask_alphas(&self, mut alphas: Vec<f32>, enc_len: usize) -> Vec<f32> {
        // Zero left context (first LEFT_CHUNK_SIZE frames)
        for i in 0..LEFT_CHUNK_SIZE.min(enc_len) {
            alphas[i] = 0.0;
        }
        // Zero right context (last RIGHT_CHUNK_SIZE frames)
        let right_start = enc_len.saturating_sub(RIGHT_CHUNK_SIZE);
        for i in right_start..enc_len {
            alphas[i] = 0.0;
        }
        alphas
    }

    /// Stateful CIF (Continuous Integrate-and-Fire).
    /// Uses self.encoder_out_cache and self.alpha_cache as persistent state.
    fn run_cif(
        &mut self,
        enc_tensor: &ndarray::Array3<f32>,
        enc_len: usize,
        alphas: &[f32],
    ) -> Result<Vec<f32>> {
        let enc_data = enc_tensor
            .slice(ndarray::s![0, ..enc_len, ..])
            .as_slice()
            .unwrap()
            .to_vec();

        let mut acoustic: Vec<f32> = Vec::new();
        let mut integrate = self.alpha_cache;
        let threshold: f32 = 1.0;
        let feat = self.encoder_output_size;

        for i in 0..enc_len {
            let this_alpha = alphas[i];
            if this_alpha <= 0.0 {
                continue;
            }

            if integrate + this_alpha < threshold {
                integrate += this_alpha;
                let enc_row = &enc_data[i * feat..(i + 1) * feat];
                for j in 0..feat {
                    self.encoder_out_cache[j] += enc_row[j] * this_alpha;
                }
                continue;
            }

            // Fire — threshold reached
            let remaining = threshold - integrate;
            let enc_row = &enc_data[i * feat..(i + 1) * feat];
            for j in 0..feat {
                self.encoder_out_cache[j] += enc_row[j] * remaining;
            }
            acoustic.extend_from_slice(&self.encoder_out_cache);

            // Start new integration with remainder
            integrate += this_alpha - threshold;
            for j in 0..feat {
                self.encoder_out_cache[j] = enc_row[j] * integrate;
            }
        }

        // Save state
        self.alpha_cache = integrate;

        Ok(acoustic)
    }

    /// Stateful decoder — updates self.decoder_caches.
    fn run_decoder(
        &mut self,
        enc_tensor: &ndarray::Array3<f32>,
        enc_len: usize,
        acoustic: &[f32],
        num_tokens: usize,
    ) -> Result<Vec<i64>> {
        let acoustic_tensor = ndarray::Array3::from_shape_vec(
            (1, num_tokens, self.encoder_output_size),
            acoustic.to_vec(),
        )?;
        let acoustic_len = ndarray::Array1::from_vec(vec![num_tokens as i32]);
        let enc_len_arr = ndarray::Array1::from_vec(vec![enc_len as i32]);

        let mut inputs = ort::inputs! {
            "enc" => ort::value::TensorRef::from_array_view(enc_tensor.view())?,
            "enc_len" => ort::value::TensorRef::from_array_view(enc_len_arr.view())?,
            "acoustic_embeds" => ort::value::TensorRef::from_array_view(acoustic_tensor.view())?,
            "acoustic_embeds_len" => ort::value::TensorRef::from_array_view(acoustic_len.view())?
        };

        // Feed current decoder caches as inputs
        for i in 0..self.decoder_num_blocks {
            inputs.push((
                format!("in_cache_{}", i).into(),
                ort::value::TensorRef::from_array_view(self.decoder_caches[i].view())?.into(),
            ));
        }

        let outputs = self.decoder_session.run(inputs)?;

        // sample_ids from output index 1
        let (_, ids_data) = outputs[1].try_extract_tensor::<i64>()?;
        let sample_ids: Vec<i64> = ids_data.to_vec();

        // Update decoder caches from outputs (out_cache_0..out_cache_15 start at output index 2)
        for i in 0..self.decoder_num_blocks {
            let out_idx = 2 + i;
            let (shape, data) = outputs[out_idx].try_extract_tensor::<f32>()?;
            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            self.decoder_caches[i] =
                ndarray::Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())?;
        }

        Ok(sample_ids)
    }
}

// ── Helpers ──

fn discover_onnx(hf_path: &std::path::Path, name: &str, prefer_int8: bool) -> Result<std::path::PathBuf> {
    if prefer_int8 {
        let int8 = hf_path.join(format!("{}.int8.onnx", name));
        let fp32 = hf_path.join(format!("{}.onnx", name));
        if int8.exists() {
            Ok(int8)
        } else if fp32.exists() {
            Ok(fp32)
        } else {
            anyhow::bail!("{}.onnx / {}.int8.onnx not found at {}", name, name, hf_path.display())
        }
    } else {
        let fp32 = hf_path.join(format!("{}.onnx", name));
        let int8 = hf_path.join(format!("{}.int8.onnx", name));
        if fp32.exists() {
            Ok(fp32)
        } else if int8.exists() {
            Ok(int8)
        } else {
            anyhow::bail!("{}.onnx / {}.int8.onnx not found at {}", name, name, hf_path.display())
        }
    }
}

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
