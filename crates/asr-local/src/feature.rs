//! 共享特征提取设施：mel filterbank、LFR 堆叠、窗口函数。
//!
//! 抽取自 paraformer.rs（正确实现）+ fbank.rs（待修）+ zipformer.rs（待修）。
//! 统一使用 mel 空间计算 filterbank 权重（对齐 kaldi_native_fbank）。
//! 修复 C1：fbank.rs / zipformer.rs 此前在 Hz 空间算权重，与 paraformer 的
//! mel 空间实现不一致，影响特征正确性。

use ndarray::Array2;

// ── Mel scale conversions ──

pub fn hz_to_mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

#[allow(dead_code)]
pub fn mel_to_hz(mel: f64) -> f64 {
    700.0 * ((mel / 1127.0).exp() - 1.0)
}

// ── Mel filterbank (mel-space weights, 对齐 kaldi_native_fbank) ──

/// 在 mel 空间均匀分布 (num_bins+2) 个点，权重斜率也在 mel 空间计算。
///
/// `high_freq > 0` 直接用作最大频率；`high_freq <= 0` 视为 Nyquist + high_freq
/// （如 -400 → 7600 Hz，paraformer 默认）。
pub fn mel_filterbank(
    num_bins: usize,
    fft_size: usize,
    sample_rate: u32,
    high_freq: f64,
) -> Vec<Vec<f64>> {
    let n_freqs = fft_size / 2 + 1;
    let nyquist = sample_rate as f64 / 2.0;
    let fmax = if high_freq > 0.0 {
        high_freq
    } else {
        nyquist + high_freq
    };
    let mel_low = hz_to_mel(20.0);
    let mel_high = hz_to_mel(fmax);
    let mel_delta = (mel_high - mel_low) / (num_bins as f64 + 1.0);
    let fft_bin_width = sample_rate as f64 / fft_size as f64;

    let mut filters = vec![vec![0.0f64; n_freqs]; num_bins];
    for bin in 0..num_bins {
        let left_mel = mel_low + bin as f64 * mel_delta;
        let center_mel = mel_low + (bin as f64 + 1.0) * mel_delta;
        let right_mel = mel_low + (bin as f64 + 2.0) * mel_delta;

        for j in 0..n_freqs {
            let freq = fft_bin_width * j as f64;
            let mel = hz_to_mel(freq);
            if mel > left_mel && mel < right_mel {
                if mel <= center_mel {
                    filters[bin][j] = (mel - left_mel) / (center_mel - left_mel);
                } else {
                    filters[bin][j] = (right_mel - mel) / (right_mel - center_mel);
                }
            }
        }
    }
    filters
}

// ── LFR (Low Frame Rate) stacking ──

/// LFR 堆叠：将相邻 window_size 帧拼接为单帧，步进 window_shift。
/// 不足部分零填充。公式：n_lfr = (n_frames - window_size) / shift + 1（与原实现一致）。
pub fn apply_lfr(fbank: &Array2<f32>, window_size: usize, window_shift: usize) -> Array2<f32> {
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

// ── Window functions ──

/// Hamming 窗：0.54 - 0.46*cos(2πi/(N-1))
pub fn hamming_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos())
        .collect()
}

/// Povey 窗：(0.5 - 0.5*cos(2πi/(N-1)))^0.85
/// knf feature-window.cc GetWindow() — 流式 Paraformer 默认使用此窗口
pub fn povey_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| {
            let a = 2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32;
            (0.5 - 0.5 * a.cos()).powf(0.85)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hz_mel_roundtrip() {
        for &hz in &[100.0_f64, 1000.0, 4000.0, 8000.0] {
            let mel = hz_to_mel(hz);
            let back = mel_to_hz(mel);
            assert!((back - hz).abs() < 1e-6, "roundtrip {} Hz failed: {}", hz, back);
        }
    }

    #[test]
    fn test_mel_filterbank_mel_space_weights() {
        let fb = mel_filterbank(80, 512, 16000, 7600.0);
        assert_eq!(fb.len(), 80);
        assert_eq!(fb[0].len(), 257);
        // 低频和高频 bin 都应有非零权重
        let sum0: f64 = fb[0].iter().sum();
        assert!(sum0 > 0.0, "bin 0 权重和应 > 0");
        let sum79: f64 = fb[79].iter().sum();
        assert!(sum79 > 0.0, "bin 79 权重和应 > 0");
        // 三角形 filter 顶点权重 <= 1.0
        for bin in &fb {
            let max_w = bin.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            assert!(max_w <= 1.0 + 1e-10, "三角形 filter 权重 <= 1.0");
        }
    }

    #[test]
    fn test_mel_filterbank_high_freq_negative() {
        // high_freq=-400 → fmax=7600（paraformer 默认）
        let fb_a = mel_filterbank(80, 512, 16000, -400.0);
        let fb_b = mel_filterbank(80, 512, 16000, 7600.0);
        // 两种指定方式应产生相同结果
        assert_eq!(fb_a, fb_b);
    }

    #[test]
    fn test_apply_lfr_shapes() {
        let fbank = Array2::ones((13, 80));
        let out = apply_lfr(&fbank, 7, 6);
        assert_eq!(out.ncols(), 560);
        // (13-7)/6+1 = 2
        assert_eq!(out.nrows(), 2);
    }

    #[test]
    fn test_apply_lfr_short_input() {
        let fbank = Array2::ones((3, 80));
        let out = apply_lfr(&fbank, 7, 6);
        // n_frames < window_size → 1
        assert_eq!(out.nrows(), 1);
        assert_eq!(out.ncols(), 560);
    }
}
