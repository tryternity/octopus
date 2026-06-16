//! DeepFilterNet3 流式环境降噪（ONNX，48kHz）。
//!
//! 处理模型：penta2himajin/deepfilternet3-onnx/dfn3.onnx（带 GRU 状态的流式版）。
//! 数据流：48k 样本 → 每 480 样本(10ms)一帧 → STFT(hann,n_fft=960) → feat
//!       → onnx(spec,feat,GRU状态) → enhanced_spec → iSTFT + OLA → 48k 增强样本。

use rustfft::num_complex::Complex;
use rustfft::Fft;

/// FFT 参数（DeepFilterNet3 契约，绑定 48kHz）。
pub const N_FFT: usize = 960;
pub const HOP: usize = 480;
pub const NBINS: usize = N_FFT / 2 + 1; // 481
pub const N_ERB: usize = 32;
pub const N_DF: usize = 96; // DF 滤波作用的 bin 数（feat_spec 维度）

/// sqrt-Hann 窗：w[n] = sqrt(0.5 - 0.5·cos(2πn/N))。分析窗 = 合成窗。
/// 50% overlap（hop=N/2）下 w² 跨 hop 求和 = 1（COLA 完美重建，增益=1）。
pub fn sqrt_hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let hann = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            hann.sqrt()
        })
        .collect()
}

/// STFT 单帧：实信号 × 窗 → FFT → 取前 NBINS 复数 bin。
pub fn stft_frame(frame: &[f32], window: &[f32], fft: &dyn Fft<f32>) -> Vec<Complex<f32>> {
    debug_assert_eq!(frame.len(), N_FFT);
    let mut buf: Vec<Complex<f32>> = (0..N_FFT)
        .map(|i| Complex::new(frame[i] * window[i], 0.0))
        .collect();
    fft.process(&mut buf);
    buf[..NBINS].to_vec()
}

/// iSTFT 单帧：NBINS 复数 → 共轭对称填充 → IFFT → × 合成窗 → N_FFT 实样本。
/// rustfft 的 inverse 不含 1/N 归一化，手动 ×1/N。
pub fn istft_frame(spec: &[Complex<f32>], ifft: &dyn Fft<f32>, window: &[f32]) -> Vec<f32> {
    debug_assert_eq!(spec.len(), NBINS);
    let mut buf = vec![Complex::new(0.0, 0.0); N_FFT];
    for i in 0..NBINS {
        buf[i] = spec[i];
    }
    // 共轭对称填充（实信号的 FFT 性质）
    for i in 1..(N_FFT - NBINS + 1) {
        buf[N_FFT - i] = spec[i].conj();
    }
    ifft.process(&mut buf);
    let scale = 1.0 / N_FFT as f32;
    (0..N_FFT).map(|i| buf[i].re * scale * window[i]).collect()
}

/// Glasberg-Moore ERB 尺度：频率(Hz) → ERB number。
/// f_erb = 9.265 · ln(1 + f / 24.863)
fn freq_to_erb(freq: f32) -> f32 {
    9.265 * (1.0 + freq / 24.863).ln()
}

/// ERB number → 频率(Hz)（反函数）。
fn erb_to_freq(erb: f32) -> f32 {
    24.863 * ((erb / 9.265).exp() - 1.0)
}

/// 生成 32 个 ERB 带对 481 个 bin 的 [lo, hi) 边界。
/// 覆盖 0..24000Hz（48kHz Nyquist），按 ERB 尺度均分。
/// 注：DeepFilterNet 的精确带划分对齐 df crate；此实现为标准 ERB 均分近似。
pub fn erb_bounds() -> Vec<(usize, usize)> {
    let nyquist = 24000.0f32;
    let erb_max = freq_to_erb(nyquist);
    let bin_freq = |i: usize| -> f32 { i as f32 / N_FFT as f32 * 48000.0 };

    let mut bounds = Vec::with_capacity(N_ERB);
    for b in 0..N_ERB {
        let erb_lo = erb_max * b as f32 / N_ERB as f32;
        let erb_hi = erb_max * (b + 1) as f32 / N_ERB as f32;
        let f_lo = erb_to_freq(erb_lo);
        let f_hi = erb_to_freq(erb_hi);
        let mut lo = 0;
        while lo < NBINS && bin_freq(lo) < f_lo {
            lo += 1;
        }
        let mut hi = lo;
        while hi < NBINS && bin_freq(hi) <= f_hi {
            hi += 1;
        }
        bounds.push((lo, hi.max(lo + 1)));
    }
    // 连续性修正：前一带 hi = 后一带 lo，最后到 NBINS
    if bounds[0].0 > 0 {
        bounds[0].0 = 0;
    }
    for w in 0..N_ERB.saturating_sub(1) {
        bounds[w].1 = bounds[w + 1].0;
    }
    bounds[N_ERB - 1].1 = NBINS;
    bounds
}

/// feat_erb[32]：每个 ERB 带的能量（|spec|² 之和）。
pub fn feat_erb(spec: &[Complex<f32>], bounds: &[(usize, usize)]) -> Vec<f32> {
    bounds
        .iter()
        .map(|(lo, hi)| (*lo..*hi).map(|i| spec[i].norm_sqr()).sum())
        .collect()
}

/// feat_spec[96·2]：前 96 个 bin 的复数 (re, im) 交错。
pub fn feat_spec(spec: &[Complex<f32>]) -> Vec<f32> {
    let mut out = Vec::with_capacity(N_DF * 2);
    for i in 0..N_DF {
        out.push(spec[i].re);
        out.push(spec[i].im);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{FftDirection, FftPlanner};

    #[test]
    fn sqrt_hann_satisfys_cola_at_50pct_overlap() {
        // 相邻两帧的 w²（=hann）之和应为常数 1.0（COLA 完美重建条件）
        let w = sqrt_hann_window(N_FFT);
        for i in 0..HOP {
            let sum = w[i] * w[i] + w[i + HOP] * w[i + HOP];
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "COLA 失败 @ {}: w²+hann_shifted = {}",
                i,
                sum
            );
        }
    }

    #[test]
    fn stft_istft_reconstructs_with_high_snr() {
        // 纯 DSP 重建（不经模型）：长信号逐帧 STFT→iSTFT+OLA，中段应高 SNR 还原
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        let w = sqrt_hann_window(N_FFT);

        // 生成 ~0.5s 的 1kHz 正弦
        let n_total = 48000 * 1 / 2; // 0.5s @48k
        let mut signal = Vec::with_capacity(n_total);
        for i in 0..n_total {
            let t = i as f32 / 48000.0;
            signal.push((2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.5);
        }

        // 逐帧 STFT→iSTFT + OLA 重建
        let mut recon = vec![0.0f32; n_total + N_FFT];
        let n_frames = (n_total - N_FFT) / HOP + 1;
        for f in 0..n_frames {
            let start = f * HOP;
            let frame = &signal[start..start + N_FFT];
            let spec = stft_frame(frame, &w, &*fft);
            let time = istft_frame(&spec, &*ifft, &w);
            for j in 0..N_FFT {
                recon[start + j] += time[j];
            }
        }

        // 中段（避开边界）计算 SNR
        let lo = N_FFT;
        let hi = n_total - N_FFT;
        let mut signal_power = 0.0;
        let mut noise_power = 0.0;
        for i in lo..hi {
            signal_power += signal[i] * signal[i];
            let e = recon[i] - signal[i];
            noise_power += e * e;
        }
        let snr_db = 10.0 * (signal_power / noise_power).log10();
        eprintln!("STFT/iSTFT 重建 SNR = {:.1}dB", snr_db);
        assert!(
            snr_db > 40.0,
            "STFT/iSTFT 重建 SNR 应 > 40dB，实际 {:.1}dB",
            snr_db
        );
    }

    #[test]
    fn erb_bounds_cover_all_bins_and_correct_count() {
        let bounds = erb_bounds();
        assert_eq!(bounds.len(), N_ERB, "应为 32 个 ERB 带");
        // 第 0 带从 bin 0 开始，最后一带到 NBINS(481) 结束，无间断无重叠
        assert_eq!(bounds[0].0, 0);
        assert_eq!(bounds[N_ERB - 1].1, NBINS);
        for w in bounds.windows(2) {
            assert_eq!(w[0].1, w[1].0, "ERB 带应连续");
        }
    }

    #[test]
    fn feat_erb_aggregates_bin_energy() {
        // DC(bin0)=大能量，其余=0 → feat_erb[0] 应 > 0，其余 ≈ 0
        let mut spec = vec![Complex::new(0.0, 0.0); NBINS];
        spec[0] = Complex::new(1.0, 0.0);
        let bounds = erb_bounds();
        let erb = feat_erb(&spec, &bounds);
        assert_eq!(erb.len(), N_ERB);
        assert!(erb[0] > 0.99 && erb[0] < 1.01);
        for v in &erb[1..] {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn feat_spec_packs_first_96_bins_complex() {
        let mut spec = vec![Complex::new(0.0, 0.0); NBINS];
        for i in 0..N_DF {
            spec[i] = Complex::new(i as f32, (i as f32) * 0.5);
        }
        let fs = feat_spec(&spec);
        assert_eq!(fs.len(), N_DF * 2);
        // 前 96 bin 的 (re, im) 交错
        assert_eq!(fs[0], 0.0); // bin0 re
        assert_eq!(fs[1], 0.0); // bin0 im
        assert_eq!(fs[2], 1.0); // bin1 re
        assert_eq!(fs[3], 0.5); // bin1 im
    }
}
