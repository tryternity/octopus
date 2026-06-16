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
}
