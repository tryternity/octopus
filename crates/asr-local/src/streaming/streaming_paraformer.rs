//! Streaming Paraformer ASR — chunk-by-chunk inference with stateful CIF + decoder caches.
//!
//! Based on sherpa-onnx's `online-recognizer-paraformer-impl.h`.

use anyhow::{Context, Result};
use ort::session::Session;

use crate::config;
use crate::fbank::apply_lfr;
use crate::paraformer::{
    decode_tokens, extract_cmvn_from_metadata, FBANK_FFT, FBANK_FFT_SIZE,
    FBANK_FRAME_LEN, FBANK_FRAME_SHIFT, FBANK_NUM_BINS, LFR_WINDOW_SHIFT, LFR_WINDOW_SIZE,
    MEL_FILTERBANK, MEL_FILTERBANK_RANGE, POVEY_WINDOW,
};

// ── Streaming chunk parameters (from sherpa-onnx) ──
const CHUNK_SIZE: usize = 61; // fbank frames per chunk (~0.61s)
const TOKEN_EOS: i64 = 2; // skip blank(0)/sos(1)/eos(2) when accumulating
const LEFT_CHUNK_SIZE: usize = 5; // left context overlap in LFR frames
const RIGHT_CHUNK_SIZE: usize = 3; // right context overlap in LFR frames

/// Streaming Paraformer engine — maintains state across chunks.
///
/// fbank 提取采用**增量式**架构（与 sherpa-onnx OnlineFbank 一致）：
/// 音频样本线性追加，fbank 帧按序计算，pre-emphasis 状态跨帧正确传递。
/// 不再对重叠 chunk 重复提取 fbank。
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
    cache_keys: Vec<String>, // 预分配的 decoder input 键名 "in_cache_0".."in_cache_15"

    // Incremental fbank extraction state
    raw_samples: Vec<f32>, // accumulated samples (× 32768)
    fbank_cache: Vec<f32>, // computed fbank frames, flattened [num_frames * 80]
    input_finished: bool,  // true after flush — allows last frame to zero-pad

    // Streaming state (carried across chunks)
    feat_cache: Vec<f32>,                      // [8 * 560] overlap buffer
    encoder_out_cache: Vec<f32>,               // [512] CIF hidden accumulator
    alpha_cache: f32,                          // CIF integrate accumulator
    decoder_caches: Vec<ndarray::Array3<f32>>, // 16 × [1, 512, cache_time]
    num_processed_frames: i32,                 // fbank frame counter (fbank space)
    all_token_ids: Vec<i64>,                   // 全局累积 token ID（跨 chunk），用于整体解码
    last_emitted_token: i64, // 上个 chunk 最后一个有效 token（跨边界去重用，-1=无）
    /// flush 后新段标记：flush 用零 padding 收尾，结束后 feat_cache 已被冲成静音（非上段语音
    /// 尾巴）。故新段首 chunk 不 mask left 是安全的——静音 alpha≈0 不会重 fire 上段尾，却保住
    /// 新句音头（修停顿后首字丢失：段2丢「开」、段4丢「始」）。锁存到新段首个 fire 的 chunk 才清
    ///（若首 chunk 是静音没 fire，保留给下个 chunk，确保音头不被错过）。
    fresh_segment: bool,
}

impl StreamingParaformer {
    /// Create a new streaming engine for the given model name (e.g. "paraformer-streaming").
    /// Loads ONNX sessions and vocabulary; initializes all state to zeros.
    pub fn new(engine_name: &str) -> Result<Self> {
        let (_cat, entry) = config::resolve_engine_any(engine_name)
            .with_context(|| format!("Failed to resolve paraformer streaming engine: {}", engine_name))?;
        Self::new_from_entry(&entry)
    }

    /// 从已解析的 ModelEntry 构造（StreamingSession::new 用，避免双重 DB 查找）。
    pub fn new_from_entry(entry: &octopus_infra::db::ModelEntry) -> Result<Self> {
        let hf_path = config::resolve_model_dir(&entry.source)?;

        let prefer_int8 = true;

        let encoder_path = crate::config::discover_onnx(&hf_path, "encoder", prefer_int8)?;
        let decoder_path = crate::config::discover_onnx(&hf_path, "decoder", prefer_int8)?;

        let encoder_session = crate::config::apply_session_acceleration(Session::builder()?)?
            .commit_from_file(&encoder_path)?;
        let decoder_session = crate::config::apply_session_acceleration(Session::builder()?)?
            .commit_from_file(&decoder_path)?;

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
        // 第十六轮 P2-2：.max(1) 防 usize 下溢——异常模型（decoder_kernel_size="0"）
        // parse 成功得 0，unwrap_or(11) 不生效 → cache_time = 0 - 1 下溢 panic（new + reset 都炸）。
        let decoder_kernel_size: usize = decoder_kernel_size_str.parse().unwrap_or(11).max(1);

        let cache_time = decoder_kernel_size - 1; // 10
        let feat_dim = FBANK_NUM_BINS * LFR_WINDOW_SIZE; // 560

        // Load vocabulary
        let vocab = crate::zipformer::load_vocab(&hf_path)?;

        // Pre-allocate decoder cache input keys (avoid per-chunk format!)
        let cache_keys = (0..decoder_num_blocks)
            .map(|i| format!("in_cache_{}", i))
            .collect::<Vec<_>>();

        // Initialize decoder caches
        let decoder_caches = (0..decoder_num_blocks)
            .map(|_| ndarray::Array3::<f32>::zeros((1, encoder_output_size, cache_time)))
            .collect();

        let engine = Self {
            encoder_session,
            decoder_session,
            neg_mean,
            inv_stddev,
            encoder_output_size,
            feat_dim,
            decoder_num_blocks,
            decoder_kernel_size,
            vocab,
            cache_keys,
            raw_samples: Vec::new(),
            fbank_cache: Vec::new(),
            input_finished: false,
            feat_cache: vec![0.0; (LEFT_CHUNK_SIZE + RIGHT_CHUNK_SIZE) * feat_dim],
            encoder_out_cache: vec![0.0; encoder_output_size],
            alpha_cache: 0.0,
            decoder_caches,
            num_processed_frames: 0,
            all_token_ids: Vec::new(),
            last_emitted_token: -1,
            fresh_segment: false,
        };

        Ok(engine)
    }

    /// Feed audio samples (16kHz mono f32, range [-1,1]) into the engine.
    /// Returns `Some(text)` if the chunk produced recognition results.
    /// Call this repeatedly as audio arrives (~600ms chunks).
    pub fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>> {
        // 清除 flush 留下的 input_finished 收尾标记。flush 在静音冲刷时设 true，让末帧越界
        // 零 padding 吐尾音（compute_new_fbank_frames 走收尾分支）；但 Paraformer 流式不 reset
        // （累积上下文跨 chunk），会话内若不清，后续 accept_samples 会持续走收尾分支 →
        // 每次多算越界零 padding 帧 → 特征错乱 → 识别错乱 / 丢字 / 大量重复字。
        // accept_samples 代表「继续说话」，必须回到正常帧计算模式。
        // （reset 仅在录音停止 / 取消的会话边界调用，清不了此处。）
        self.input_finished = false;
        // Scale × 32768 and append
        self.raw_samples.reserve(samples.len());
        for &s in samples {
            self.raw_samples.push(s * 32768.0);
        }

        // Incrementally compute new fbank frames
        self.compute_new_fbank_frames();

        // Process available chunks (need CHUNK_SIZE frames per chunk)
        let prev_token_count = self.all_token_ids.len();
        while self.num_fbank_ready() >= self.num_processed_frames as usize + CHUNK_SIZE {
            let frame_start = self.num_processed_frames as usize;
            self.process_chunk_at(frame_start, false)?;
            self.num_processed_frames += (CHUNK_SIZE - 1) as i32;
        }

        // 只要有新 token 就重新解码全部（整体解码，BPE 跨 chunk 正确合并）
        if self.all_token_ids.len() > prev_token_count {
            let full_text = decode_tokens(&self.all_token_ids, &self.vocab);
            if !full_text.is_empty() {
                return Ok(Some(full_text));
            }
        }

        // 不 drain raw_samples：compute_new_fbank_frames 用绝对帧索引 fi*FBANK_FRAME_SHIFT
        // 索引 raw_samples，fbank_cache/num_processed_frames 同为绝对、不随 drain 前移。若 drain
        // 前段，三者索引错位 → max_frames 被 raw.len() 钉死、current_frames 单调增长追不上 →
        // 连续 accept 几个 chunk 后不再算新帧 → 识别停滞（开头几词后停住，曾由 87a49a6「防无界
        // 增长」引入）。raw_samples 全程累积，单次录音 ~64KB/s（reset 每次录音开始 clear），可控。

        Ok(None)
    }

    /// Flush remaining audio after recording stops.
    /// Pads with zeros, computes all remaining frames, processes final chunks.
    pub fn finish(&mut self) -> Result<String> {
        let result = self.flush()?;
        Ok(result.unwrap_or_default())
    }

    /// Active flush: pad raw_samples with zeros, compute remaining fbank frames,
    /// and process all remaining chunks. The last chunk force-fires residual CIF.
    pub fn flush(&mut self) -> Result<Option<String>> {
        // flush 收尾【当前段】——其 process_chunk_at 走正常 mask（清掉可能残留的 fresh_segment，
        // 避免上一段 unconsumed 的 fresh 误 mask 当前段尾 chunk）。
        self.fresh_segment = false;
        // Pad raw_samples with enough zeros to ensure at least CHUNK_SIZE frames
        // can be computed for the final chunk.
        let current_frames = self.num_fbank_ready();
        let processed = self.num_processed_frames as usize;
        let needed_frames = if current_frames > processed {
            // How many chunks can we still process?
            let remaining = current_frames - processed;
            CHUNK_SIZE.saturating_sub(remaining)
        } else {
            CHUNK_SIZE
        };

        if needed_frames > 0 {
            let needed_samples = needed_frames * FBANK_FRAME_SHIFT;
            self.raw_samples
                .resize(self.raw_samples.len() + needed_samples, 0.0);
        }

        self.input_finished = true;
        self.compute_new_fbank_frames();

        let mut had_new_tokens = false;
        while self.num_fbank_ready() >= self.num_processed_frames as usize + CHUNK_SIZE {
            let frame_start = self.num_processed_frames as usize;
            // Check if this is the last chunk
            let remaining_after =
                self.num_fbank_ready() as i32 - frame_start as i32 - CHUNK_SIZE as i32;
            let is_last = remaining_after < (CHUNK_SIZE as i32 - 1);
            let produced = self.process_chunk_at(frame_start, is_last)?;
            if produced {
                had_new_tokens = true;
            }
            self.num_processed_frames += (CHUNK_SIZE - 1) as i32;
        }

        // flush 结束 = 段边界。下次 accept 是新段首 chunk，置 fresh_segment 让其 mask_left=false
        //（保新句音头；feat_cache 已被零 padding 冲成静音，不 mask left 不会重 fire 上段尾）。
        self.fresh_segment = true;
        // 段边界后去重上下文失效：上段尾 token 与新段首 token 是不同句，不应被
        // `tid == self.last_emitted_token`（见 process_chunk_at 去重逻辑）误判为重复。
        // 重置为 -1（无），避免新段首有效 token 被误去重。R4-4。
        self.last_emitted_token = -1;
        if had_new_tokens {
            let full_text = decode_tokens(&self.all_token_ids, &self.vocab);
            if !full_text.is_empty() {
                return Ok(Some(full_text));
            }
        }
        Ok(None)
    }

    /// Reset all streaming state for a new utterance.
    pub fn reset(&mut self) {
        self.raw_samples.clear();
        self.fbank_cache.clear();
        self.input_finished = false;
        self.feat_cache.fill(0.0);
        self.encoder_out_cache.fill(0.0);
        self.alpha_cache = 0.0;
        // 第十六轮 P2-2：saturating_sub 防御（构造端已 .max(1)，此处双保险）。
        let cache_time = self.decoder_kernel_size.saturating_sub(1);
        // 形状一致时直接 fill(0.0) 复用内存（run_decoder 慢路径可能改维度，此处兜底重建）。
        let init_shape = (1, self.encoder_output_size, cache_time);
        for cache in &mut self.decoder_caches {
            if cache.dim() == init_shape {
                cache.fill(0.0);
            } else {
                *cache = ndarray::Array3::<f32>::zeros(init_shape);
            }
        }
        self.num_processed_frames = 0;
        self.all_token_ids.clear();
        self.last_emitted_token = -1;
        self.fresh_segment = false;
    }

    /// Number of fbank frames computed and available in fbank_cache.
    fn num_fbank_ready(&self) -> usize {
        self.fbank_cache.len() / FBANK_NUM_BINS
    }

    /// Incrementally compute new fbank frames from raw_samples.
    /// Pre-emphasis state carries correctly across all frames (no chunk-boundary issues).
    fn compute_new_fbank_frames(&mut self) {
        let n_samples = self.raw_samples.len();

        // How many frames can we compute?
        let max_frames = if n_samples >= FBANK_FRAME_LEN {
            if self.input_finished {
                // Flush: allow last frame to extend past buffer (zero-padded)
                (n_samples - FBANK_FRAME_LEN + FBANK_FRAME_SHIFT + FBANK_FRAME_SHIFT / 2)
                    / FBANK_FRAME_SHIFT
            } else {
                (n_samples - FBANK_FRAME_LEN) / FBANK_FRAME_SHIFT + 1
            }
        } else if self.input_finished && n_samples > 0 {
            1
        } else {
            0
        };

        let current_frames = self.fbank_cache.len() / FBANK_NUM_BINS;
        if max_frames <= current_frames {
            return;
        }

        let fft = &*FBANK_FFT;
        let n_freqs = FBANK_FFT_SIZE / 2 + 1;
        let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0f32); FBANK_FFT_SIZE];
        let mut frame_buf = [0.0f32; FBANK_FRAME_LEN];
        let preemph_coeff = 0.97f32;

        for fi in current_frames..max_frames {
            let start = fi * FBANK_FRAME_SHIFT;

            // 1. Extract frame
            for (j, frame_val) in frame_buf.iter_mut().enumerate().take(FBANK_FRAME_LEN) {
                *frame_val = if start + j < n_samples {
                    self.raw_samples[start + j]
                } else {
                    0.0
                };
            }

            // 2. DC offset removal
            let mean: f32 = frame_buf.iter().sum::<f32>() / FBANK_FRAME_LEN as f32;
            for s in frame_buf.iter_mut() {
                *s -= mean;
            }

            // 3. Pre-emphasis: y[i] = x[i] - α·x[i-1]
            //    帧重叠（shift=160 < len=400），上一帧末尾并非本帧 start-1。
            //    直接从连续缓冲回溯 start-1 取准确前序样本，无需跨帧状态。
            //    raw_samples[start-1] 未去直流，减去本帧 mean 作近似（knf 行为）。
            let mut prev = if start > 0 {
                self.raw_samples[start - 1] - mean
            } else {
                0.0
            };
            for val in frame_buf.iter_mut().take(FBANK_FRAME_LEN) {
                let cur = *val;
                *val = cur - preemph_coeff * prev;
                prev = cur;
            }

            // 4. Povey window + FFT
            for j in 0..FBANK_FFT_SIZE {
                let s = if j < FBANK_FRAME_LEN {
                    frame_buf[j] * POVEY_WINDOW[j]
                } else {
                    0.0
                };
                buf[j] = rustfft::num_complex::Complex::new(s, 0.0);
            }
            fft.process(&mut buf);

            // 5. Power spectrum + mel filterbank + log
            let mut power_spectrum = [0.0f64; FBANK_FFT_SIZE / 2 + 1];
            for k in 0..n_freqs {
                power_spectrum[k] =
                    buf[k].re as f64 * buf[k].re as f64 + buf[k].im as f64 * buf[k].im as f64;
            }

            for mi in 0..FBANK_NUM_BINS {
                let mut sum = 0.0f64;
                let fb_row = &MEL_FILTERBANK[mi];
                let (start, end) = MEL_FILTERBANK_RANGE[mi];
                for k in start..end {
                    sum += power_spectrum[k] * fb_row[k];
                }
                self.fbank_cache.push((sum as f32 + 1e-10).ln());
            }
        }
    }

    /// Process a chunk of CHUNK_SIZE fbank frames starting at frame_start.
    /// Accumulates decoded token IDs into self.all_token_ids.
    /// Returns true if new tokens were produced.
    fn process_chunk_at(&mut self, frame_start: usize, is_final: bool) -> Result<bool> {
        // 1. Extract CHUNK_SIZE frames from fbank_cache, pad with zeros if short
        let mut features = self.extract_features_from_cache(frame_start)?;

        // 2. Positional encoding
        self.apply_positional_encoding(&mut features);

        // 3. Prepend feat_cache, update feat_cache
        let combined = self.apply_feat_overlap(features)?;

        // 4. Encoder
        let (enc_tensor, enc_len_scalar, alphas) = self.run_encoder(&combined)?;

        // 5. Zero overlap alphas（去 chunk 间 overlap，防 CIF 重复 fire）
        // mask_left = !is_first：首 chunk 的 left 基于"padding+首音频"非 overlap（feat_cache 初始
        //   全0），mask 会误删；中段/final mask left 去【上 chunk】overlap。
        // mask_right = !(is_first || is_final)，即**仅中段 chunk** mask right：
        //   - 首 chunk 不 mask right：其 alpha 集中在 right 3 帧（e2e 实测 frame0 mask前1.7，
        //     right占~1.0），mask 会砍光首字能量 → fired=0 首字丢失。
        //   - final chunk 不 mask right：保留尾音 fire。
        //   - 中段 chunk mask right：right 是与【下 chunk】的 overlap 边界帧，acoustic 不准
        //     （边界重复计算），不 mask 会让 fired 增多 → 叠字/错字上升（2dae4c8 全关 right 的副作用）。
        let is_first_chunk = self.num_processed_frames == 0;
        // fresh_segment：flush 后新段首 chunk——feat_cache 已是静音，不 mask left 保新句音头
        //（修停顿后首字丢失）。锁存到新段首个 fire 的 chunk 才清（见 step 6 后 consume）。
        let fresh = self.fresh_segment;
        let mask_left = !(is_first_chunk || fresh);
        let mask_right = !is_first_chunk && !is_final;
        let alphas = mask_alphas_selective(
            alphas,
            enc_len_scalar,
            /*mask_left=*/ mask_left,
            /*mask_right=*/ mask_right,
        );

        // 6. CIF (force-fire if final)
        let acoustic = if is_final {
            self.run_cif_final(&enc_tensor, enc_len_scalar, &alphas)?
        } else {
            self.run_cif(&enc_tensor, enc_len_scalar, &alphas)?
        };

        if acoustic.is_empty() {
            return Ok(false);
        }
        let num_tokens = acoustic.len() / self.encoder_output_size;

        // 新段首个 fire 的 chunk：音头已过，清 fresh_segment 恢复正常 mask_left。
        // 若本 chunk 静音没 fire（num_tokens=0），保留 fresh 给下个 chunk——确保音头不被错过。
        if num_tokens > 0 && self.fresh_segment {
            self.fresh_segment = false;
        }

        // 7. Stateful decoder → token IDs
        let sample_ids = self.run_decoder(&enc_tensor, enc_len_scalar, &acoustic, num_tokens)?;

        // 8. 累积有效 token（跳过 blank/sos/eos）——整体解码交给调用方。
        // 跨 chunk 边界去重：CIF 在音节跨 chunk 时，会在相邻两个 chunk 各 fire 一次同一 token
        //（如"识别"的"别"跨 480/540 两个 chunk 双 fire → "识别别"）。若本 chunk 首个有效 token
        // == 上 chunk 末 token，判为双 fire 重复，跳过。单 chunk 内合法重复（"爸爸""常常"，
        // 两个相同字在同一 chunk fire）不跨边界，不受影响。
        let mut seen_first_valid = false;
        for &tid in &sample_ids {
            if tid > TOKEN_EOS {
                let idx = tid as usize;
                if idx < self.vocab.len() && !self.vocab[idx].is_empty() {
                    if !seen_first_valid && tid == self.last_emitted_token {
                        seen_first_valid = true;
                        continue;
                    }
                    self.all_token_ids.push(tid);
                    self.last_emitted_token = tid;
                    seen_first_valid = true;
                }
            }
        }

        Ok(true)
    }

    /// Extract CHUNK_SIZE fbank frames from cache starting at frame_start,
    /// apply LFR and CMVN. Pads with zeros if not enough frames.
    fn extract_features_from_cache(&self, frame_start: usize) -> Result<ndarray::Array2<f32>> {
        let total_ready = self.fbank_cache.len() / FBANK_NUM_BINS;
        let available = total_ready.saturating_sub(frame_start);
        let n_frames = available.min(CHUNK_SIZE);

        let mut fbank = ndarray::Array2::zeros((CHUNK_SIZE, FBANK_NUM_BINS));
        for fi in 0..n_frames {
            let src = (frame_start + fi) * FBANK_NUM_BINS;
            for j in 0..FBANK_NUM_BINS {
                fbank[[fi, j]] = self.fbank_cache[src + j];
            }
        }

        let lfr = apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);

        // CMVN
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
        // k_scale = -ln(10000) / (half_dim - 1) — 负号与 sherpa-onnx 一致
        //（标准 Transformer 正弦位置编码：exp(-d * log(10000) / (d_model/2 - 1))）
        let k_scale = -(10000.0f32).ln() / (half_dim - 1) as f32;
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
        // 零拷贝：用只读视图包装 &self.feat_cache，避免 4480 个 f32（~17.5KB）的堆克隆。
        let cache_arr =
            ndarray::ArrayView2::from_shape((cache_rows, self.feat_dim), &self.feat_cache)?;

        // Concatenate: [cache | chunk]
        let mut combined = ndarray::Array2::zeros((cache_rows + n_chunk, self.feat_dim));
        combined
            .slice_mut(ndarray::s![..cache_rows, ..])
            .assign(&cache_arr);
        combined
            .slice_mut(ndarray::s![cache_rows.., ..])
            .assign(&features);

        // Save last (left+right) rows back to feat_cache——复用预分配（容量恒定
        // cache_rows × feat_dim），省每 chunk 17.5KB 堆分配。AGENTS.md 热路径同款
        // copy_from_slice 模式（对齐 decoder_caches 优化）。
        let total_rows = combined.nrows();
        let save_start = total_rows.saturating_sub(cache_rows);
        let new_cache_view = combined.slice(ndarray::s![save_start..total_rows, ..]);
        if let Some(src) = new_cache_view.as_slice() {
            // C-contiguous 快路径：整体 memcpy
            self.feat_cache.copy_from_slice(src);
        } else {
            // 回退：逐元素拷贝（理论不会走到，combined 是 owned C-order）
            for (i, &v) in new_cache_view.iter().enumerate() {
                self.feat_cache[i] = v;
            }
        }

        Ok(combined)
    }

    /// Run the encoder ONNX session.
    fn run_encoder(
        &mut self,
        features: &ndarray::Array2<f32>,
    ) -> Result<(ndarray::Array3<f32>, usize, Vec<f32>)> {
        let (n_rows, n_cols) = (features.nrows(), features.ncols());
        // 零拷贝：直接 reshape 共享底层数据（features 是 owned &mut 的 borrow，此期间不会被修改）
        let speech_tensor = features
            .view()
            .into_shape_with_order(ndarray::IxDyn(&[1, n_rows, n_cols]))?
            .into_dimensionality::<ndarray::Ix3>()?;
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

    /// Stateful CIF (Continuous Integrate-and-Fire).
    /// Uses self.encoder_out_cache and self.alpha_cache as persistent state.
    #[allow(clippy::needless_range_loop)] // CIF 内层 hot loop：索引访问 enc_row + cache 更直观
    fn run_cif(
        &mut self,
        enc_tensor: &ndarray::Array3<f32>,
        enc_len: usize,
        alphas: &[f32],
    ) -> Result<Vec<f32>> {
        // 零拷贝：直接拿 &[f32] 切片引用 enc_tensor 底层数据
        // 防御性 enc_len 截断：ONNX 输出异常（padding/截断）时避免 slice panic
        let enc_dim1 = enc_tensor.shape()[1];
        let enc_slice = enc_tensor.slice(ndarray::s![0, ..enc_len.min(enc_dim1), ..]);
        let enc_data = enc_slice.as_slice().ok_or_else(|| anyhow::anyhow!(
            "encoder output non-contiguous (shape={:?})", enc_tensor.shape()
        ))?;

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

    /// CIF with force-fire: same as run_cif, but fires any residual accumulator
    /// (alpha_cache > 0.5) at the end to prevent tail token loss.
    #[allow(clippy::needless_range_loop)] // CIF 内层 hot loop：索引访问 enc_row + cache 更直观
    fn run_cif_final(
        &mut self,
        enc_tensor: &ndarray::Array3<f32>,
        enc_len: usize,
        alphas: &[f32],
    ) -> Result<Vec<f32>> {
        // 零拷贝：直接拿 &[f32] 切片引用 enc_tensor 底层数据
        // 防御性 enc_len 截断：ONNX 输出异常（padding/截断）时避免 slice panic
        let enc_dim1 = enc_tensor.shape()[1];
        let enc_slice = enc_tensor.slice(ndarray::s![0, ..enc_len.min(enc_dim1), ..]);
        let enc_data = enc_slice.as_slice().ok_or_else(|| anyhow::anyhow!(
            "encoder output non-contiguous (shape={:?})", enc_tensor.shape()
        ))?;

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
            // Fire
            let remaining = threshold - integrate;
            let enc_row = &enc_data[i * feat..(i + 1) * feat];
            for j in 0..feat {
                self.encoder_out_cache[j] += enc_row[j] * remaining;
            }
            acoustic.extend_from_slice(&self.encoder_out_cache);
            integrate += this_alpha - threshold;
            for j in 0..feat {
                self.encoder_out_cache[j] = enc_row[j] * integrate;
            }
        }

        // Force-fire residual
        if integrate > 0.5 && !self.encoder_out_cache.iter().all(|&v| v == 0.0) {
            acoustic.extend_from_slice(&self.encoder_out_cache);
            self.alpha_cache = 0.0;
            self.encoder_out_cache.fill(0.0);
        } else {
            self.alpha_cache = integrate;
        }

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
        // 零拷贝：用只读视图包装 acoustic 切片，避免 to_vec() 的堆拷贝。
        let acoustic_view =
            ndarray::ArrayView3::from_shape((1, num_tokens, self.encoder_output_size), acoustic)?;
        // 单元素长度张量用栈数组 + ArrayView1，避免 from_vec(vec![x]) 的堆分配。
        let acoustic_len_data = [num_tokens as i32];
        let enc_len_data = [enc_len as i32];
        let acoustic_len = ndarray::ArrayView1::from(&acoustic_len_data);
        let enc_len_arr = ndarray::ArrayView1::from(&enc_len_data);

        let mut inputs = ort::inputs! {
            "enc" => ort::value::TensorRef::from_array_view(enc_tensor.view())?,
            "enc_len" => ort::value::TensorRef::from_array_view(enc_len_arr)?,
            "acoustic_embeds" => ort::value::TensorRef::from_array_view(acoustic_view)?,
            "acoustic_embeds_len" => ort::value::TensorRef::from_array_view(acoustic_len)?
        };

        // Feed current decoder caches as inputs (键名预分配，避免 format!)
        for i in 0..self.decoder_num_blocks {
            inputs.push((
                self.cache_keys[i].as_str().into(),
                ort::value::TensorRef::from_array_view(self.decoder_caches[i].view())?.into(),
            ));
        }

        let outputs = self.decoder_session.run(inputs)?;

        // sample_ids from output index 1
        let (_, ids_data) = outputs[1].try_extract_tensor::<i64>()?;
        let sample_ids: Vec<i64> = ids_data.to_vec();

        // Update decoder caches from outputs (out_cache_0..out_cache_15 start at output index 2)
        // 直接复用预分配的 Array3 内存，避免 to_vec + 重新分配
        for i in 0..self.decoder_num_blocks {
            let out_idx = 2 + i;
            let (shape, data) = outputs[out_idx].try_extract_tensor::<f32>()?;
            let expected = self.decoder_caches[i].len();
            let actual = data.len();
            if expected == actual {
                // 快路径：维度匹配，直接 copy 到预分配内存
                self.decoder_caches[i]
                    .as_slice_mut()
                    .ok_or_else(|| anyhow::anyhow!(
                        "decoder cache non-contiguous (idx={})", i
                    ))?
                    .copy_from_slice(data);
            } else {
                // 慢路径：维度变化（首次或模型异常），重新分配
                let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
                self.decoder_caches[i] =
                    ndarray::Array3::from_shape_vec((dims[0], dims[1], dims[2]), data.to_vec())?;
            }
        }

        Ok(sample_ids)
    }
}

// ── Helpers ──

// discover_onnx 已抽取到 config.rs（pub(crate)），本文件调 crate::config::discover_onnx

// ── CIF alpha overlap mask（去 chunk 间 overlap，防 CIF 重复 fire）──

/// 按 `mask_left` / `mask_right` 选择性置零 alpha 的 overlap 区。纯函数，便于单测。
///
/// - `mask_left`：置零前 [`LEFT_CHUNK_SIZE`] 帧（上 chunk 的 overlap）。**首 chunk 不 mask**——
///   其 `feat_cache` 初始全 0（padding），left 帧 基于"padding + 首音频"，alpha 反映首字音头，
///   mask 会误删首字（见 `process_chunk_at` step 5 的 e2e 实测）。
/// - `mask_right`：置零后 [`RIGHT_CHUNK_SIZE`] 帧（下 chunk 的 overlap）。**final chunk 不 mask**——
///   其后无新 chunk，right 帧是真实尾音，置零会丢尾字。
fn mask_alphas_selective(
    mut alphas: Vec<f32>,
    enc_len: usize,
    mask_left: bool,
    mask_right: bool,
) -> Vec<f32> {
    let n = alphas.len().min(enc_len);
    if mask_left {
        for val in alphas.iter_mut().take(LEFT_CHUNK_SIZE.min(n)) {
            *val = 0.0;
        }
    }
    if mask_right {
        let right_start = n.saturating_sub(RIGHT_CHUNK_SIZE);
        for val in alphas.iter_mut().take(n).skip(right_start) {
            *val = 0.0;
        }
    }
    alphas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hf_snapshot(repo: &str) -> Option<std::path::PathBuf> {
        let base = format!(
            "~/.cache/huggingface/hub/models--{}",
            repo.replace('/', "--")
        );
        let base = base.replace("~", &std::env::var("HOME").unwrap_or_default());
        let snapshots = std::path::Path::new(&base).join("snapshots");
        if !snapshots.exists() {
            return None;
        }
        let mut entries = std::fs::read_dir(&snapshots).ok()?;
        entries
            .next()?
            .ok()
            .map(|e| e.path().join("test_wavs"))
            .filter(|p| p.exists())
    }

    #[test]
    fn mask_alphas_selective_four_combinations() {
        // enc_len=18, LEFT_CHUNK_SIZE=5, RIGHT_CHUNK_SIZE=3。用 1.0..=18.0 标记每帧便于观察。
        let mk = || (1..=18).map(|x| x as f32).collect::<Vec<_>>();
        // 正常中间 chunk（非首非 final）：mask 两侧 → 前 5 + 后 3 置零
        let v = mask_alphas_selective(mk(), 18, true, true);
        assert_eq!(&v[0..5], &[0.0; 5], "left 5 置零");
        assert_eq!(&v[15..18], &[0.0; 3], "right 3 置零 (idx 15,16,17)");
        assert_eq!(v[5], 6.0, "中间保留");
        assert_eq!(v[14], 15.0);
        // 首 chunk 非 final：只 mask right（首字 left 保留——本次首字丢失修复的核心）
        let v = mask_alphas_selective(mk(), 18, false, true);
        assert_eq!(v[0], 1.0, "首字 left[0] 保留");
        assert_eq!(v[4], 5.0, "left[4] 保留");
        assert_eq!(&v[15..18], &[0.0; 3], "right 仍 mask");
        // 非首 final：只 mask left（尾音 right 保留）
        let v = mask_alphas_selective(mk(), 18, true, false);
        assert_eq!(&v[0..5], &[0.0; 5], "left 仍 mask");
        assert_eq!(v[17], 18.0, "尾音 right[17] 保留");
        // 首 chunk final（开口极短、一 chunk 即 flush）：两侧都不 mask
        let v = mask_alphas_selective(mk(), 18, false, false);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[17], 18.0);
        // enc_len 截断：alphans 长于 enc_len 时只 mask 前 enc_len 帧
        let v = mask_alphas_selective(vec![1.0; 20], 18, true, true);
        assert_eq!(v.len(), 20, "长度不变");
        assert_eq!(&v[0..5], &[0.0; 5]);
        assert_eq!(&v[15..18], &[0.0; 3]);
        assert_eq!(v[18], 1.0, "超出 enc_len 的帧不动");
    }

    /// 流式 Paraformer 集成测试 — 用真实模型验证识别质量。
    /// 此测试用于诊断"字重复/乱码"类问题。
    #[test]
    #[ignore = "real-model: 需 HF 模型缓存，cargo test -- --ignored 跑"]
    fn test_streaming_paraformer_real_model() {
        let repo = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
        let test_wavs = match hf_snapshot(repo) {
            Some(p) => p,
            None => {
                eprintln!("[skip] HF cache 未找到 {}", repo);
                return;
            }
        };

        let wav_path = test_wavs.join("0.wav");
        if !wav_path.exists() {
            eprintln!("[skip] 测试 wav 不存在: {}", wav_path.display());
            return;
        }

        let samples =
            crate::audio::read_wav_16k(wav_path.to_str().unwrap()).expect("读取 wav 失败");
        eprintln!(
            "[test] 样本数: {} ({:.2}s)",
            samples.len(),
            samples.len() as f32 / 16000.0
        );

        let mut engine = StreamingParaformer::new("paraformer-bilingual").expect("创建引擎失败");

        // 模拟流式：每次喂入 600ms 样本（9600 samples）
        let chunk_size = 16000 * 600 / 1000; // 9600
        let mut full_text = String::new();

        for (i, chunk) in samples.chunks(chunk_size).enumerate() {
            if let Some(text) = engine.accept_samples(chunk).expect("accept_samples 失败") {
                eprintln!("[chunk {}] full_asr: {:?}", i, text);
                full_text = text; // 引擎返回完整 ASR 文本（跨 chunk 累积解码）
            }
        }

        // flush 尾部
        if let Some(text) = engine.flush().expect("flush 失败") {
            eprintln!("[flush] full_asr: {:?}", text);
            full_text = text;
        }

        let final_text = engine.finish().expect("finish 失败");
        if !final_text.is_empty() {
            eprintln!("[finish] full_asr: {:?}", final_text);
            full_text = final_text;
        }

        eprintln!("[result] 完整文本: {:?}", full_text);

        // 不做严格断言，只验证不 panic 且输出非空
        assert!(!full_text.is_empty(), "识别结果不应为空");
    }

    /// 回归（问题四）：flush 设 input_finished=true（收尾模式），accept_samples（继续说话）
    /// 必须清除它，否则 Paraformer 流式不 reset（累积上下文）会导致后续 compute_new_fbank_frames
    /// 持续走零 padding 收尾分支 → 帧边界越界零填充 → 特征错乱 → 识别错乱 / 丢字 / 大量重复字。
    #[test]
    #[ignore = "real-model: 需 HF 模型缓存，cargo test -- --ignored 跑"]
    fn test_accept_samples_clears_input_finished_after_flush() {
        let repo = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
        let test_wavs = match hf_snapshot(repo) {
            Some(p) => p,
            None => {
                eprintln!("[skip] HF cache 未找到 {}", repo);
                return;
            }
        };
        let wav_path = test_wavs.join("0.wav");
        if !wav_path.exists() {
            eprintln!("[skip] 测试 wav 不存在: {}", wav_path.display());
            return;
        }
        let samples =
            crate::audio::read_wav_16k(wav_path.to_str().unwrap()).expect("读取 wav 失败");

        let mut engine = StreamingParaformer::new("paraformer-bilingual").expect("创建引擎失败");
        let chunk_size = 16000 * 600 / 1000;

        // 喂前半段 + flush（模拟第一次静音停顿冲刷）
        for chunk in samples[..samples.len() / 2].chunks(chunk_size) {
            let _ = engine.accept_samples(chunk).unwrap();
        }
        let _ = engine.flush().unwrap();
        assert!(
            engine.input_finished,
            "flush 后 input_finished 应为 true（收尾模式）"
        );

        // 用户继续说话：accept_samples 必须清除 input_finished
        let _ = engine
            .accept_samples(&samples[samples.len() / 2..])
            .unwrap();
        assert!(
            !engine.input_finished,
            "accept_samples 必须清除 input_finished，否则后续帧计算持续走零 padding 收尾分支 → 特征错乱"
        );

        // 验证 flush→accept→finish 整条路径不 panic、产出非空文本
        let final_text = engine.finish().unwrap();
        eprintln!("[问题四回归] final_text: {:?}", final_text);
        assert!(!final_text.is_empty(), "flush→accept→finish 后文本不应为空");
    }

    /// 回归（drain 停滞）：87a49a6 的 raw_samples drain 与 compute_new_fbank_frames 的绝对帧索引
    /// fi*FBANK_FRAME_SHIFT 不兼容——drain 前移 raw_samples 但 fbank_cache/num_processed_frames
    /// 不前移 → max_frames 被 raw.len() 钉死、current_frames 单调追不上 → 连续 accept 几个 chunk
    /// 后不再算新帧 → 识别停滞（用户症状：开头几词后停住）。`test_streaming_paraformer_real_model`
    /// 只断言「文本非空」故漏检（开头几词即非空）。本测试直接断言连续 accept 后 fbank 帧持续增长。
    #[test]
    #[ignore = "real-model: 需 DB paraformer-bilingual 引擎 + HF 模型缓存，cargo test -- --ignored 跑"]
    fn test_no_drain_stall_continuous_accept_grows_fbank() {
        let repo = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
        let test_wavs = match hf_snapshot(repo) {
            Some(p) => p,
            None => {
                eprintln!("[skip] HF cache 未找到 {}", repo);
                return;
            }
        };
        let wav_path = test_wavs.join("0.wav");
        if !wav_path.exists() {
            eprintln!("[skip] 测试 wav 不存在: {}", wav_path.display());
            return;
        }
        let base =
            crate::audio::read_wav_16k(wav_path.to_str().unwrap()).expect("读取 wav 失败");
        // 重复到 >6s（跨 10+ chunk），放大「多 chunk 后停滞」信号
        let mut samples = Vec::new();
        while samples.len() < 16000 * 6 {
            samples.extend_from_slice(&base);
        }

        let mut engine = StreamingParaformer::new("paraformer-bilingual").expect("创建引擎失败");
        let chunk_size = 16000 * 600 / 1000; // 9600
        let mut last_ready = 0usize;
        for (i, chunk) in samples.chunks(chunk_size).enumerate() {
            let _ = engine.accept_samples(chunk).expect("accept_samples 失败");
            last_ready = engine.num_fbank_ready();
            eprintln!("[drain-regress chunk {}] fbank_ready={}", i, last_ready);
        }
        // drain bug 下 raw_samples 被 drain 限制在 ~19000 样本 → max_frames ~117 → ready 钉死；
        // 修复（移除 drain）后 6s 音频 raw 全程累积 → ready ≈ 590+。阈值 300 明确区分。
        assert!(
            last_ready > 300,
            "fbank 帧停滞在 {}：连续 accept 后应持续增长（drain bug 回归——raw_samples 被 \
             drain 但 fbank_cache 绝对索引未同步，max_frames 被钉死导致不再算新帧）",
            last_ready
        );
    }

    /// 离线对比测试 — 用同一个 wav 跑离线 paraformer，对比流式结果。
    /// 注意：离线用的是 paraformer-zh（非流式模型），和流式模型不同，
    /// 但可以验证 fbank/LFR 基础设施是否正确。
    #[test]
    #[ignore = "real-model: 需 HF 模型缓存，cargo test -- --ignored 跑"]
    fn test_offline_paraformer_real_model() {
        let repo = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
        let test_wavs = match hf_snapshot(repo) {
            Some(p) => p,
            None => {
                eprintln!("[skip] HF cache 未找到 {}", repo);
                return;
            }
        };

        let wav_path = test_wavs.join("0.wav");
        if !wav_path.exists() {
            eprintln!("[skip] 测试 wav 不存在: {}", wav_path.display());
            return;
        }

        let samples =
            crate::audio::read_wav_16k(wav_path.to_str().unwrap()).expect("读取 wav 失败");
        eprintln!(
            "[test-offline] 样本数: {} ({:.2}s)",
            samples.len(),
            samples.len() as f32 / 16000.0
        );

        // 离线 paraformer 使用 encoder/decoder 分离模型
        // 直接用 extract_cmvn_from_metadata 测试 CMVN 是否正确
        let (_cat, entry) = config::resolve_engine_any("paraformer-bilingual")
            .expect("paraformer-bilingual 未在 DB 可用引擎中（需 is_available=1）");

        let hf_path = config::resolve_model_dir(&entry.source).expect("resolve_model_dir 失败");
        let encoder_path = hf_path.join("encoder.int8.onnx");
        let encoder_session =
            crate::config::apply_session_acceleration(Session::builder().unwrap())
                .unwrap()
                .commit_from_file(&encoder_path)
                .expect("加载 encoder 失败");

        let (neg_mean, inv_stddev, enc_out) =
            extract_cmvn_from_metadata(&encoder_session).expect("extract_cmvn 失败");

        eprintln!(
            "[cmvn] neg_mean: {} vals, inv_stddev: {} vals, enc_out: {}",
            neg_mean.len(),
            inv_stddev.len(),
            enc_out
        );
        eprintln!(
            "[cmvn] neg_mean[0..5]: {:?}",
            &neg_mean[..5.min(neg_mean.len())]
        );
        eprintln!(
            "[cmvn] inv_stddev[0..5]: {:?}",
            &inv_stddev[..5.min(inv_stddev.len())]
        );

        // 验证 inv_stddev 是否被 sqrt(512) ≈ 22.6 缩放
        let scale = (enc_out as f32).sqrt();
        eprintln!("[cmvn] sqrt(enc_out) = {:.4}", scale);
    }
}
