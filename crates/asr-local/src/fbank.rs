//! 80-bin log-fbank 特征提取 + LFR 堆叠（共享设施）。
//!
//! 原服务于 SenseVoice（sherpa nano 简化版，已移除——见
//! `docs/removed-sensevoice-sherpa-nano.md`），现被 [`crate::sensevoice_orig`]
//! （fbank+LFR→560 维）与 [`crate::firered`]（纯 fbank，无 LFR）复用。

use anyhow::Result;
use ndarray::Array2;
use once_cell::sync::Lazy;

use crate::feature;
use crate::paraformer::FBANK_FFT;

// ── Fbank constants (matching kaldi_native_fbank defaults) ──
const FBANK_FFT_SIZE: usize = 512;
const FBANK_FRAME_LEN: usize = 400;
const FBANK_FRAME_SHIFT: usize = 160;
const FBANK_NUM_BINS: usize = 80;
const FBANK_SAMPLE_RATE: u32 = 16000;

// ── LFR (Low Frame Rate) stacking ──
const LFR_WINDOW_SIZE: usize = 7;
const LFR_WINDOW_SHIFT: usize = 6;

static HAMMING_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| feature::hamming_window(FBANK_FRAME_LEN));
static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| {
    // C1 修复：改用 mel 空间 filterbank 权重（对齐 paraformer / kaldi_native_fbank）
    feature::mel_filterbank(FBANK_NUM_BINS, FBANK_FFT_SIZE, FBANK_SAMPLE_RATE, FBANK_SAMPLE_RATE as f64 / 2.0)
});

/// 80-bin log-fbank + LFR(m=7/n=6) → [T,560]（原版 SenseVoice 用）。
pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
    let fbank = compute_fbank(&scaled, 0.97)?;
    let lfr = feature::apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);
    Ok(lfr)
}

/// 纯 80-bin log-fbank（frame_len=400 / shift=160 / hamming 窗，无 LFR）。FireRed 等用。
///
/// `preemph_coeff` — 预加重系数。SenseVoice 传 0.97（对齐 kaldi_native_fbank 默认）；
/// FireRed 训练 preemph 配置未确认，传 0.0 保持旧行为（仅跳过 pre-emphasis）。
///
/// DC offset removal 始终执行（knf 默认 `remove_dc_offset=true`；fbank 常量注释自称
/// "matching kaldi_native_fbank defaults"，此前缺此步是遗漏）。
///
/// 2026-07-09 审查修复：此前缺 DC offset removal + pre-emphasis，与 paraformer.rs /
/// kaldi_native_fbank 默认不一致——SenseVoice 的 am.mvn 基于含这两步的特征统计，
/// 推理缺它们 → 特征分布偏移 → 真实音频乱码（合成音频落在模型鲁棒区侥幸通过）。
pub(crate) fn compute_fbank(samples: &[f32], preemph_coeff: f32) -> Result<Array2<f32>> {
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

        // 2. DC offset removal（去直流）— knf 默认 remove_dc_offset=true
        let mean: f32 = frame_buf.iter().sum::<f32>() / FBANK_FRAME_LEN as f32;
        for s in frame_buf.iter_mut() {
            *s -= mean;
        }

        // 3. Pre-emphasis（预加重）: y[i] = x[i] - preemph_coeff * x[i-1]
        //    帧重叠（shift=160 < len=400），上一帧末尾并非本帧 start-1。
        //    直接从连续缓冲回溯 start-1 取准确前序样本，无需跨帧状态。
        //    samples[start-1] 未去直流，减去本帧 mean 作近似（knf 行为，对齐 paraformer.rs:503）。
        if preemph_coeff != 0.0 {
            let mut prev = if start > 0 {
                samples[start - 1] - mean
            } else {
                0.0
            };
            for val in frame_buf.iter_mut().take(FBANK_FRAME_LEN) {
                let cur = *val;
                *val = cur - preemph_coeff * prev;
                prev = cur;
            }
        }

        // 4. 加窗 + FFT
        for j in 0..FBANK_FFT_SIZE {
            let s = if j < FBANK_FRAME_LEN {
                frame_buf[j] * HAMMING_WINDOW[j]
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

// apply_lfr 已抽取至 feature.rs

// hamming_window / mel_filterbank_fbank / apply_lfr 已抽取至 feature.rs（C1 修复）
