//! 80-bin log-fbank 特征提取 + LFR 堆叠（共享设施）。
//!
//! 原服务于 SenseVoice（sherpa nano 简化版，已移除——见
//! `docs/removed-sensevoice-sherpa-nano.md`），现被 [`crate::sensevoice_orig`]
//! （fbank+LFR→560 维）与 [`crate::firered`]（纯 fbank，无 LFR）复用。

use anyhow::Result;
use ndarray::Array2;
use once_cell::sync::Lazy;

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

static HAMMING_WINDOW: Lazy<Vec<f32>> = Lazy::new(|| hamming_window(FBANK_FRAME_LEN));
static MEL_FILTERBANK: Lazy<Vec<Vec<f64>>> = Lazy::new(|| mel_filterbank_fbank());

/// 80-bin log-fbank + LFR(m=7/n=6) → [T,560]（原版 SenseVoice 用）。
pub(crate) fn compute_fbank_features(samples: &[f32]) -> Result<Array2<f32>> {
    let scaled: Vec<f32> = samples.iter().map(|&s| s * 32768.0).collect();
    let fbank = compute_fbank(&scaled)?;
    let lfr = apply_lfr(&fbank, LFR_WINDOW_SIZE, LFR_WINDOW_SHIFT);
    Ok(lfr)
}

/// 纯 80-bin log-fbank（frame_len=400 / shift=160 / hamming 窗，无 LFR）。FireRed 等用。
pub(crate) fn compute_fbank(samples: &[f32]) -> Result<Array2<f32>> {
    let n_frames = if samples.len() >= FBANK_FRAME_LEN {
        (samples.len() - FBANK_FRAME_LEN) / FBANK_FRAME_SHIFT + 1
    } else {
        1
    };

    let fft = &*FBANK_FFT;

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

fn apply_lfr(fbank: &Array2<f32>, window_size: usize, window_shift: usize) -> Array2<f32> {
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
