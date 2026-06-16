//! DeepFilterNet3 流式环境降噪（ONNX，48kHz）。
//!
//! 处理模型：penta2himajin/deepfilternet3-onnx/dfn3.onnx（带 GRU 状态的流式版）。
//! 数据流：48k 样本 → 每 480 样本(10ms)一帧 → STFT(hann,n_fft=960) → feat
//!       → onnx(spec,feat,GRU状态) → enhanced_spec → iSTFT + OLA → 48k 增强样本。

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use ndarray::Array3;
use ort::session::Session;
use ort::value::TensorRef;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftDirection, FftPlanner};

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

/// DeepFilterNet3 流式降噪处理器（有状态：GRU 隐状态 + 缓冲）。
///
/// 生命周期：录音会话内跨帧保持状态（GRU 反映噪声环境稳态估计，不应被分段打断）；
/// 新会话开始时调 `reset()`。状态语义与 filter_vad（每段 reset）故意相反。
pub struct DenoiseProcessor {
    session: Session,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    erb_bounds: Vec<(usize, usize)>,
    // GRU 隐状态（持久，跨帧）
    enc_h: Array3<f32>, // [1,1,256]
    erb_h: Array3<f32>, // [2,1,256]
    df_h: Array3<f32>,  // [2,1,256]
    // 流式增量缓冲
    in_buf: Vec<f32>, // 48k 原始输入累积（每次 process_samples 喂入，每满 HOP 取一帧）
    raw_prev: Vec<f32>, // 上一帧的原始 HOP 样本（分析帧左上下文，**原始时域**，非增强）
    out_buf: Vec<f32>,  // 已增强样本待输出
    ola_prev: Vec<f32>, // 上一帧 iSTFT 增强 N_FFT 样本（OLA 重叠用，**增强时域**）
}

impl DenoiseProcessor {
    /// 加载模型 + 初始化 DSP 常量 + GRU 状态归零。
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()?.commit_from_file(model_path)?;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        Ok(Self {
            session,
            fft,
            ifft,
            window: sqrt_hann_window(N_FFT),
            erb_bounds: erb_bounds(),
            enc_h: Array3::zeros((1, 1, 256)),
            erb_h: Array3::zeros((2, 1, 256)),
            df_h: Array3::zeros((2, 1, 256)),
            in_buf: Vec::new(),
            raw_prev: vec![0.0; HOP],
            out_buf: Vec::new(),
            ola_prev: vec![0.0; N_FFT],
        })
    }

    /// GRU + 缓冲清零（录音会话边界调用）。
    pub fn reset(&mut self) {
        self.enc_h.fill(0.0);
        self.erb_h.fill(0.0);
        self.df_h.fill(0.0);
        self.in_buf.clear();
        self.raw_prev.iter_mut().for_each(|v| *v = 0.0);
        self.out_buf.clear();
        self.ola_prev.iter_mut().for_each(|v| *v = 0.0);
    }

    /// 增量处理 48k 样本：累积到 in_buf，每满 HOP 处理一帧，返回已增强样本。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        self.in_buf.extend_from_slice(samples);
        while self.in_buf.len() >= HOP {
            let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
            self.process_one_frame(&new);
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填到 HOP 整数倍，处理残留，吐剩余输出。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            if self.in_buf.len() < HOP {
                self.in_buf.resize(HOP, 0.0);
            }
            while self.in_buf.len() >= HOP {
                let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
                self.process_one_frame(&new);
            }
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 处理一帧（new_samples 长度 = HOP）→ 增强样本（HOP 个）入 out_buf。
    fn process_one_frame(&mut self, new_samples: &[f32]) {
        debug_assert_eq!(new_samples.len(), HOP);

        // 分析帧 = [raw_prev(上一帧原始 HOP)] + [new_samples(本帧原始 HOP)] = N_FFT
        let mut frame = Vec::with_capacity(N_FFT);
        frame.extend_from_slice(&self.raw_prev);
        frame.extend_from_slice(new_samples);
        // 更新 raw_prev 为本帧原始样本（供下一帧分析）
        self.raw_prev.copy_from_slice(new_samples);

        // STFT + 特征
        let spec = stft_frame(&frame, &self.window, self.fft.as_ref());
        let feat_erb_v = feat_erb(&spec, &self.erb_bounds);
        let feat_spec_v = feat_spec(&spec);

        // 构造 onnx 输入张量（形状严格对齐 IO 契约）
        let spec_4d = complex_to_5d(&spec); // [1,1,1,481,2]
        let erb_in = vec_to_4d_flat(&feat_erb_v); // [1,1,1,32]
        let fspec_in = vec_to_5d(&feat_spec_v); // [1,1,1,96,2]

        let spec_t = TensorRef::from_array_view(spec_4d.view()).unwrap();
        let erb_t = TensorRef::from_array_view(erb_in.view()).unwrap();
        let fspec_t = TensorRef::from_array_view(fspec_in.view()).unwrap();
        let enc_t = TensorRef::from_array_view(self.enc_h.view()).unwrap();
        let erbh_t = TensorRef::from_array_view(self.erb_h.view()).unwrap();
        let dfh_t = TensorRef::from_array_view(self.df_h.view()).unwrap();

        // 推理；失败则 bypass（输出原始 new_samples，GRU 状态保持，warn，不 panic）
        let outputs = match self.session.run(ort::inputs! {
            "spec" => spec_t,
            "feat_erb" => erb_t,
            "feat_spec" => fspec_t,
            "enc_h" => enc_t,
            "erb_h" => erbh_t,
            "df_h" => dfh_t,
        }) {
            Ok(o) => o,
            Err(e) => {
                log::warn!("DenoiseProcessor 单帧推理失败，bypass 原始样本: {e}");
                self.out_buf.extend_from_slice(new_samples);
                return;
            }
        };

        // 提取增强频谱（扁平 [re0,im0,re1,im1,...]，长 481*2）
        let (_es, enh_data) = outputs["enhanced_spec"]
            .try_extract_tensor::<f32>()
            .expect("extract enhanced_spec");
        let enh_spec: Vec<Complex<f32>> = (0..NBINS)
            .map(|i| Complex::new(enh_data[2 * i], enh_data[2 * i + 1]))
            .collect();

        // 回写 GRU 状态（照抄 vad.rs 的 as_slice_mut + copy_from_slice 模式）
        let (_s, enc_data) = outputs["new_enc_h"]
            .try_extract_tensor::<f32>()
            .expect("extract new_enc_h");
        if let Some(s) = self.enc_h.as_slice_mut() {
            s.copy_from_slice(&enc_data);
        } else {
            self.enc_h = Array3::from_shape_vec((1, 1, 256), enc_data.to_vec()).expect("enc_h reshape");
        }
        let (_s, erb_data) = outputs["new_erb_h"]
            .try_extract_tensor::<f32>()
            .expect("extract new_erb_h");
        if let Some(s) = self.erb_h.as_slice_mut() {
            s.copy_from_slice(&erb_data);
        } else {
            self.erb_h = Array3::from_shape_vec((2, 1, 256), erb_data.to_vec()).expect("erb_h reshape");
        }
        let (_s, df_data) = outputs["new_df_h"]
            .try_extract_tensor::<f32>()
            .expect("extract new_df_h");
        if let Some(s) = self.df_h.as_slice_mut() {
            s.copy_from_slice(&df_data);
        } else {
            self.df_h = Array3::from_shape_vec((2, 1, 256), df_data.to_vec()).expect("df_h reshape");
        }

        // iSTFT → 增强时域 N_FFT 样本
        let time = istft_frame(&enh_spec, self.ifft.as_ref(), &self.window);

        // OLA：本帧输出前 HOP = time[0..HOP] + 上一帧增强后半 ola_prev[HOP..N_FFT]
        // （50% overlap，sqrt-Hann COLA 增益=1）
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[HOP + i];
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;
    }
}

/// spec[481] 复数 → [1,1,1,481,2] 实数组（re,im 交错）。
fn complex_to_5d(spec: &[Complex<f32>]) -> ndarray::Array5<f32> {
    let mut a = ndarray::Array5::zeros((1, 1, 1, NBINS, 2));
    for i in 0..NBINS {
        a[[0, 0, 0, i, 0]] = spec[i].re;
        a[[0, 0, 0, i, 1]] = spec[i].im;
    }
    a
}

/// feat_spec 长度 N_DF*2 的交错 (re,im) → [1,1,1,96,2]。
fn vec_to_5d(v: &[f32]) -> ndarray::Array5<f32> {
    let n = v.len() / 2;
    let mut a = ndarray::Array5::zeros((1, 1, 1, n, 2));
    for i in 0..n {
        a[[0, 0, 0, i, 0]] = v[i * 2];
        a[[0, 0, 0, i, 1]] = v[i * 2 + 1];
    }
    a
}

/// feat_erb[32] → [1,1,1,32]。
fn vec_to_4d_flat(v: &[f32]) -> ndarray::Array4<f32> {
    let mut a = ndarray::Array4::zeros((1, 1, 1, v.len()));
    for (i, x) in v.iter().enumerate() {
        a[[0, 0, 0, i]] = *x;
    }
    a
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

    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn processor_runs_and_updates_gru_state() {
        let path = crate::config::find_df3()
            .expect("dfn3.onnx 未下载，跑: hf download penta2himajin/deepfilternet3-onnx");
        let mut p = super::DenoiseProcessor::new(&path).unwrap();
        let enc_before = p.enc_h.clone();
        // 两帧静音输入（累积到 HOP 触发一帧）
        let frame = vec![0.0f32; HOP];
        let _ = p.process_samples(&frame);
        let _ = p.process_samples(&frame);
        // GRU enc_h 应在推理后变化
        assert_ne!(p.enc_h, enc_before, "GRU enc_h 应在推理后更新");
    }

    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn sample_conservation_input_equals_output_length() {
        let path = crate::config::find_df3().unwrap();
        let mut p = super::DenoiseProcessor::new(&path).unwrap();
        let n = 48000; // 1s @48k
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        // 样本守恒：输出长度 == 输入长度（OLA 不丢不增，尾部 flush 吐残留）
        assert_eq!(
            out.len(),
            input.len(),
            "样本守恒失败：in={} out={}",
            input.len(),
            out.len()
        );
    }

    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn streaming_incremental_equals_batch() {
        let path = crate::config::find_df3().unwrap();
        let n = 48000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();

        // 批处理（一次性）
        let mut p1 = super::DenoiseProcessor::new(&path).unwrap();
        let mut batch = p1.process_samples(&input);
        batch.extend(p1.flush());

        // 增量（分多次，每次不固定长度，含非整除 HOP 的块）
        let mut p2 = super::DenoiseProcessor::new(&path).unwrap();
        let mut incr = Vec::new();
        let chunks = [300usize, 700, 480, 1024, 480, 613, 480, 200, 13783];
        let mut off = 0;
        for &c in &chunks {
            if off + c > input.len() {
                break;
            }
            incr.extend(p2.process_samples(&input[off..off + c]));
            off += c;
        }
        if off < input.len() {
            incr.extend(p2.process_samples(&input[off..]));
        }
        incr.extend(p2.flush());

        // 增量 vs 批处理：长度相等 + 逐样本最大差 < 1e-4（无状态漂移、无边界丢帧）
        assert_eq!(
            incr.len(),
            batch.len(),
            "长度不一致：incr={} batch={}",
            incr.len(),
            batch.len()
        );
        let max_diff = incr
            .iter()
            .zip(batch.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        eprintln!("streaming max_diff = {:.3e}", max_diff);
        assert!(
            max_diff < 1e-4,
            "增量 vs 批处理不一致，max_diff={}",
            max_diff
        );
    }
}
