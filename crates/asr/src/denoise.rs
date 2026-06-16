//! DeepFilterNet3 流式环境降噪（对齐 libDF / mellonella 参考实现）。
//!
//! 处理模型：penta2himajin/deepfilternet3-onnx/dfn3.onnx（带 GRU 状态的流式版）。
//! 数据流：48k 样本 → 每 480 样本(10ms)一帧 → STFT(Vorbis,n_fft=960) → 特征提取
//!       → conv_lookahead 环形缓冲 → onnx(spec,feat,GRU状态) → enhanced_spec → iSTFT + OLA → 48k 增强样本。
//!
//! 关键对齐点（vs libDF + mellonella）：
//! - Vorbis 窗（非 sqrt-Hann）：w[n] = sin(π/2·sin²(π(n+0.5)/N))
//! - ERB 公式分母 228.833 = 24.7×9.265（非 24.863）
//! - feat_erb：band 互相关功率 → dB → EMA 均值归一化 → /40 缩放
//! - feat_spec：前 96 bin 复数 → 单位归一化（EMA 跟踪幅度 |z|，除以 √state）
//! - conv_lookahead=2：spec[t] 配对 feat[t+2]，模型导出时已移除内部 lookahead

use std::collections::VecDeque;
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

/// conv_lookahead：模型导出时移除的内部 lookahead 帧数。
/// 调用方维护环形缓冲，将 spec[t] 配对 feat[t+CONV_LOOKAHEAD] 送入模型。
pub const CONV_LOOKAHEAD: usize = 2;

/// 特征归一化 EMA 平滑系数（τ=1.0s @48kHz/hop=480）。
/// alpha = exp(-hop/sr/τ) ≈ exp(-0.01) ≈ 0.99005
const NORM_ALPHA: f32 = 0.99;

/// 初始归一化状态（匹配 libDF MEAN_NORM_INIT / UNIT_NORM_INIT）。
const MEAN_NORM_INIT: [f32; 2] = [-60.0, -90.0];
const UNIT_NORM_INIT: [f32; 2] = [0.001, 0.0001];

// ── Vorbis 窗 ──────────────────────────────────────────────────────────────

/// Vorbis 窗：w[n] = sin(π/2 · sin²(π(n+0.5)/N))。
/// 分析窗 = 合成窗；50% overlap 下 w²[n]+w²[n+H]=1（COLA 完美重建）。
pub fn vorbis_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let inner = (std::f32::consts::PI * (i as f32 + 0.5) / n as f32).sin();
            (std::f32::consts::FRAC_PI_2 * inner * inner).sin()
        })
        .collect()
}

// ── STFT / iSTFT ────────────────────────────────────────────────────────────

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

// ── ERB 尺度（对齐 libDF freq2erb / erb2freq）─────────────────────────────

/// Glasberg-Moore ERB 尺度：频率(Hz) → ERB number。
/// f_erb = 9.265 · ln(1 + f / 228.833)  其中 228.833 = 24.7 × 9.265
fn freq_to_erb(freq: f32) -> f32 {
    9.265 * (1.0 + freq / 228.833).ln()
}

/// ERB number → 频率(Hz)。
fn erb_to_freq(erb: f32) -> f32 {
    228.833 * ((erb / 9.265).exp() - 1.0)
}

/// 生成 N_ERB 个 ERB 带宽度（覆盖 0..NBINS，对齐 libDF erb_fb）。
/// 按 ERB 尺度等分，边界量化到最近 FFT bin（round），确保连续覆盖全部 bin。
pub fn erb_widths() -> Vec<usize> {
    let nyquist = 24000.0;
    let erb_high = freq_to_erb(nyquist);
    let step = erb_high / N_ERB as f32;

    // 计算边界 bin（含首尾 0 和 NBINS）
    let mut bounds: Vec<usize> = Vec::with_capacity(N_ERB + 1);
    let mut last = 0;
    for i in 0..=N_ERB {
        let erb = i as f32 * step;
        let freq = erb_to_freq(erb);
        let bin = (freq / nyquist * (NBINS - 1) as f32).round() as usize;
        let bin = bin.min(NBINS - 1);
        if i == 0 {
            bounds.push(0);
            last = 0;
        } else if i == N_ERB {
            // 末尾强制到 NBINS（覆盖最后一个 bin）
            if NBINS > last {
                bounds.push(NBINS);
            }
        } else if bin > last {
            bounds.push(bin);
            last = bin;
        }
    }

    // 计算宽度
    let mut widths: Vec<usize> = Vec::with_capacity(N_ERB);
    for w in bounds.windows(2) {
        widths.push(w[1] - w[0]);
    }

    // 如果去重导致 band 数 < N_ERB，用宽度 1 补齐（极端低频近似）
    while widths.len() < N_ERB {
        widths.push(1);
    }

    // 如果 band 数 > N_ERB（不应发生），截断
    widths.truncate(N_ERB);

    widths
}

// ── 特征提取 + 归一化（对齐 libDF）─────────────────────────────────────────

/// ERB 带功率（自相关形式）：每个带内 (Σ|spec|²)² / width²。
/// 匹配 libDF compute_band_corr(spec, spec, widths) 的自相关结果。
fn compute_band_corr(spec: &[Complex<f32>], widths: &[usize]) -> Vec<f32> {
    let mut out = Vec::with_capacity(widths.len());
    let mut start = 0;
    for &width in widths {
        let sum: f32 = (0..width).map(|i| spec[start + i].norm_sqr()).sum();
        let w = width as f32;
        out.push((sum / w) * (sum / w)); // mean² = (Σ/width)²
        start += width;
    }
    out
}

/// band_mean_norm_erb：dB 转换 + EMA 均值消除 + /40 缩放。
/// 匹配 libDF：先 10·log10(1e-10 + x)，再 EMA 归一化，再 /40。
fn band_mean_norm_erb(xs: &mut [f32], state: &mut [f32], alpha: f32) {
    for (x, s) in xs.iter_mut().zip(state.iter_mut()) {
        // dB 转换
        *x = 10.0 * (1e-10 + *x).log10();
        // EMA 均值跟踪
        *s = *x * (1.0 - alpha) + *s * alpha;
        // 中心化 + 缩放
        *x = (*x - *s) / 40.0;
    }
}

/// band_unit_norm：复数 bin 的单位归一化。
/// EMA 跟踪 |z|（复幅度），除以 √state。
/// 匹配 libDF band_unit_norm。
fn band_unit_norm(xs: &mut [Complex<f32>], state: &mut [f32], alpha: f32) {
    for (x, s) in xs.iter_mut().zip(state.iter_mut()) {
        let mag = x.norm();
        *s = mag * (1.0 - alpha) + *s * alpha;
        let norm = s.sqrt().max(1e-10);
        *x /= norm;
    }
}

/// 线性插值生成初始归一化状态（匹配 mellonella init_state_lerp）。
fn init_state_lerp(init: [f32; 2], n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![init[0]];
    }
    (0..n)
        .map(|i| init[0] + (init[1] - init[0]) * i as f32 / (n - 1) as f32)
        .collect()
}

/// 前 96 bin 复数 → (re, im) 交错 flat（供 vec_to_5d 构造 ONNX 输入）。
fn interleave_complex(spec: &[Complex<f32>], n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(spec[i].re);
        out.push(spec[i].im);
    }
    out
}

// ── 降噪处理器 ──────────────────────────────────────────────────────────────

/// DeepFilterNet3 流式降噪处理器。
///
/// 状态语义（与 filter_vad 每段 reset 相反）：
/// - GRU 隐状态 + 归一化 EMA 状态跨帧保持（噪声估计是连续物理过程）
/// - conv_lookahead 环形缓冲跨 drain_samples 周期保持
/// - 仅新会话 `start()` 调 `reset()` 全部清零
pub struct DenoiseProcessor {
    session: Session,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    erb_widths: Vec<usize>,
    // 归一化 EMA 状态
    erb_norm_state: Vec<f32>, // [32]
    df_norm_state: Vec<f32>,  // [96]
    // GRU 隐状态（持久，跨帧）
    enc_h: Array3<f32>, // [1,1,256]
    erb_h: Array3<f32>, // [2,1,256]
    df_h: Array3<f32>,  // [2,1,256]
    // conv_lookahead 环形缓冲：spec[t] 配对 feat[t+CONV_LOOKAHEAD]
    spec_queue: VecDeque<Vec<f32>>,    // 扁平 re/im [NBINS*2]
    erb_feat_queue: VecDeque<Vec<f32>>, // 归一化后 feat_erb [N_ERB]
    df_feat_queue: VecDeque<Vec<f32>>,  // 归一化后 feat_spec [N_DF*2]
    // 流式增量缓冲
    in_buf: Vec<f32>,   // 48k 原始输入累积
    raw_prev: Vec<f32>, // 上一帧原始 HOP 样本（分析帧左上下文）
    out_buf: Vec<f32>,  // 已增强样本待输出
    ola_prev: Vec<f32>, // 上一帧 iSTFT 增强 N_FFT 样本（OLA 重叠）
}

impl DenoiseProcessor {
    /// 加载模型 + 初始化 DSP 常量 + 状态归零。
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()?.commit_from_file(model_path)?;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        Ok(Self {
            session,
            fft,
            ifft,
            window: vorbis_window(N_FFT),
            erb_widths: erb_widths(),
            erb_norm_state: init_state_lerp(MEAN_NORM_INIT, N_ERB),
            df_norm_state: init_state_lerp(UNIT_NORM_INIT, N_DF),
            enc_h: Array3::zeros((1, 1, 256)),
            erb_h: Array3::zeros((2, 1, 256)),
            df_h: Array3::zeros((2, 1, 256)),
            spec_queue: VecDeque::new(),
            erb_feat_queue: VecDeque::new(),
            df_feat_queue: VecDeque::new(),
            in_buf: Vec::new(),
            raw_prev: vec![0.0; HOP],
            out_buf: Vec::new(),
            ola_prev: vec![0.0; N_FFT],
        })
    }

    /// 全状态清零（录音会话边界调用）。
    pub fn reset(&mut self) {
        self.erb_norm_state = init_state_lerp(MEAN_NORM_INIT, N_ERB);
        self.df_norm_state = init_state_lerp(UNIT_NORM_INIT, N_DF);
        self.enc_h.fill(0.0);
        self.erb_h.fill(0.0);
        self.df_h.fill(0.0);
        self.spec_queue.clear();
        self.erb_feat_queue.clear();
        self.df_feat_queue.clear();
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
            self.push_frame(&new);
            self.drain_emit();
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填到 HOP 整数倍处理残留，再填 CONV_LOOKAHEAD 零特征帧排空队列。
    pub fn flush(&mut self) -> Vec<f32> {
        // 1. 处理 in_buf 残留音频（零填到 HOP）
        if !self.in_buf.is_empty() {
            if self.in_buf.len() < HOP {
                self.in_buf.resize(HOP, 0.0);
            }
            while self.in_buf.len() >= HOP {
                let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
                self.push_frame(&new);
                self.drain_emit();
            }
        }
        // 2. 填 CONV_LOOKAHEAD 零特征帧排空 lookahead 队列
        let zero_spec = vec![0.0f32; NBINS * 2];
        let zero_erb = vec![0.0f32; N_ERB];
        let zero_df = vec![0.0f32; N_DF * 2];
        for _ in 0..CONV_LOOKAHEAD {
            self.spec_queue.push_back(zero_spec.clone());
            self.erb_feat_queue.push_back(zero_erb.clone());
            self.df_feat_queue.push_back(zero_df.clone());
            self.drain_emit();
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 计算一帧的特征并推入 lookahead 队列。
    /// 归一化状态在此更新（因果，无 lookahead）。
    fn push_frame(&mut self, new_samples: &[f32]) {
        debug_assert_eq!(new_samples.len(), HOP);

        // 分析帧 = [raw_prev(上一帧原始 HOP)] + [new_samples] = N_FFT
        let mut frame = Vec::with_capacity(N_FFT);
        frame.extend_from_slice(&self.raw_prev);
        frame.extend_from_slice(new_samples);
        self.raw_prev.copy_from_slice(new_samples);

        // STFT
        let spec = stft_frame(&frame, &self.window, self.fft.as_ref());

        // feat_erb：band 功率 → dB → 均值归一化
        let mut band_pow = compute_band_corr(&spec, &self.erb_widths);
        band_mean_norm_erb(&mut band_pow, &mut self.erb_norm_state, NORM_ALPHA);

        // feat_spec：前 96 bin → 单位归一化
        let mut df_spec: Vec<Complex<f32>> = spec[..N_DF].to_vec();
        band_unit_norm(&mut df_spec, &mut self.df_norm_state, NORM_ALPHA);
        let df_flat = interleave_complex(&df_spec, N_DF);

        // 原始 spec 扁平化（未经归一化，供模型 "spec" 输入）
        let spec_flat = interleave_complex(&spec, NBINS);

        // 推入 lookahead 队列
        self.spec_queue.push_back(spec_flat);
        self.erb_feat_queue.push_back(band_pow);
        self.df_feat_queue.push_back(df_flat);
    }

    /// 排空 lookahead 队列：spec[t] 配对 feat[t+CONV_LOOKAHEAD] 送入模型。
    fn drain_emit(&mut self) {
        while self.spec_queue.len() > CONV_LOOKAHEAD {
            let spec_flat = self.spec_queue.pop_front().unwrap();
            let feat_erb = self.erb_feat_queue[CONV_LOOKAHEAD].clone();
            let feat_spec = self.df_feat_queue[CONV_LOOKAHEAD].clone();
            self.erb_feat_queue.pop_front();
            self.df_feat_queue.pop_front();

            self.run_model(&spec_flat, &feat_erb, &feat_spec);
        }
    }

    /// 用 ONNX 模型增强单帧频谱，iSTFT + OLA 输出 HOP 个增强样本。
    fn run_model(&mut self, spec_flat: &[f32], feat_erb: &[f32], feat_spec: &[f32]) {
        // 构造 ONNX 输入张量
        let spec_4d = vec_to_5d(spec_flat); // [1,1,1,481,2]
        let erb_in = vec_to_4d_flat(feat_erb); // [1,1,1,32]
        let fspec_in = vec_to_5d(feat_spec); // [1,1,1,96,2]

        let spec_t = TensorRef::from_array_view(spec_4d.view()).unwrap();
        let erb_t = TensorRef::from_array_view(erb_in.view()).unwrap();
        let fspec_t = TensorRef::from_array_view(fspec_in.view()).unwrap();
        let enc_t = TensorRef::from_array_view(self.enc_h.view()).unwrap();
        let erbh_t = TensorRef::from_array_view(self.erb_h.view()).unwrap();
        let dfh_t = TensorRef::from_array_view(self.df_h.view()).unwrap();

        // 推理；失败则 bypass（输出静音 HOP，GRU 状态保持，warn，不 panic）
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
                log::warn!("DenoiseProcessor 单帧推理失败，bypass: {e}");
                self.out_buf.extend(vec![0.0; HOP]);
                return;
            }
        };

        // 提取输出张量；任一失败 → bypass 静音
        let (enh_data, enc_data, erb_data, df_data) = {
            let enh = outputs["enhanced_spec"].try_extract_tensor::<f32>();
            let enc = outputs["new_enc_h"].try_extract_tensor::<f32>();
            let erb = outputs["new_erb_h"].try_extract_tensor::<f32>();
            let df = outputs["new_df_h"].try_extract_tensor::<f32>();
            match (enh, enc, erb, df) {
                (Ok(a), Ok(b), Ok(c), Ok(d)) => (a.1, b.1, c.1, d.1),
                (e1, e2, e3, e4) => {
                    let first_err = e1.err().or(e2.err()).or(e3.err()).or(e4.err());
                    log::warn!("DenoiseProcessor 输出提取失败，bypass: {:?}", first_err);
                    self.out_buf.extend(vec![0.0; HOP]);
                    return;
                }
            }
        };
        // 长度校验
        if enh_data.len() < NBINS * 2
            || enc_data.len() != 256
            || erb_data.len() != 512
            || df_data.len() != 512
        {
            log::warn!(
                "DenoiseProcessor 输出形状异常（enh={},enc={},erb={},df={}），bypass",
                enh_data.len(),
                enc_data.len(),
                erb_data.len(),
                df_data.len()
            );
            self.out_buf.extend(vec![0.0; HOP]);
            return;
        }

        // —— 全部成功，安全修改 self ——
        // 增强频谱 → 复数
        let enh_spec: Vec<Complex<f32>> = (0..NBINS)
            .map(|i| Complex::new(enh_data[2 * i], enh_data[2 * i + 1]))
            .collect();

        // 回写 GRU 状态
        if let Some(s) = self.enc_h.as_slice_mut() {
            s.copy_from_slice(&enc_data);
        } else {
            self.enc_h = Array3::from_shape_vec((1, 1, 256), enc_data.to_vec()).expect("enc_h");
        }
        if let Some(s) = self.erb_h.as_slice_mut() {
            s.copy_from_slice(&erb_data);
        } else {
            self.erb_h = Array3::from_shape_vec((2, 1, 256), erb_data.to_vec()).expect("erb_h");
        }
        if let Some(s) = self.df_h.as_slice_mut() {
            s.copy_from_slice(&df_data);
        } else {
            self.df_h = Array3::from_shape_vec((2, 1, 256), df_data.to_vec()).expect("df_h");
        }

        // iSTFT → 增强时域 N_FFT 样本
        let time = istft_frame(&enh_spec, self.ifft.as_ref(), &self.window);

        // OLA：本帧输出前 HOP = time[0..HOP] + 上一帧增强后半 ola_prev[HOP..N_FFT]
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[HOP + i];
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;
    }
}

// ── 张量辅助 ────────────────────────────────────────────────────────────────

/// 扁平 [re0,im0,...] → [1,1,1,n,2]。
fn vec_to_5d(v: &[f32]) -> ndarray::Array5<f32> {
    let n = v.len() / 2;
    let mut a = ndarray::Array5::zeros((1, 1, 1, n, 2));
    for i in 0..n {
        a[[0, 0, 0, i, 0]] = v[i * 2];
        a[[0, 0, 0, i, 1]] = v[i * 2 + 1];
    }
    a
}

/// feat_erb[n] → [1,1,1,n]。
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

    // ── Vorbis 窗 ──

    #[test]
    fn vorbis_satisfys_cola_at_50pct_overlap() {
        let w = vorbis_window(N_FFT);
        for i in 0..HOP {
            let sum = w[i] * w[i] + w[i + HOP] * w[i + HOP];
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "Vorbis COLA 失败 @ {}: w² + w²_shifted = {}",
                i,
                sum
            );
        }
    }

    #[test]
    fn stft_istft_reconstructs_with_high_snr() {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        let w = vorbis_window(N_FFT);

        // 0.5s 的 1kHz 正弦 @48k
        let n_total = 48000 / 2;
        let signal: Vec<f32> = (0..n_total)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.5)
            .collect();

        // 逐帧 STFT → iSTFT + OLA
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

        // 中段 SNR
        let lo = N_FFT;
        let hi = n_total - N_FFT;
        let mut sig_pow = 0.0;
        let mut noise_pow = 0.0;
        for i in lo..hi {
            sig_pow += signal[i] * signal[i];
            let e = recon[i] - signal[i];
            noise_pow += e * e;
        }
        let snr_db = 10.0 * (sig_pow / noise_pow).log10();
        eprintln!("STFT/iSTFT (Vorbis) 重建 SNR = {:.1}dB", snr_db);
        assert!(snr_db > 40.0, "重建 SNR 应 > 40dB，实际 {:.1}dB", snr_db);
    }

    // ── ERB ──

    #[test]
    fn erb_widths_correct_count_and_coverage() {
        let widths = erb_widths();
        assert_eq!(widths.len(), N_ERB, "应为 {} 个 ERB 带", N_ERB);
        let total: usize = widths.iter().sum();
        assert_eq!(total, NBINS, "ERB 带总宽应覆盖全部 {} bins", NBINS);
        // 每个带至少 1 bin
        for (i, &w) in widths.iter().enumerate() {
            assert!(w >= 1, "ERB 带 {} 宽度为 0", i);
        }
    }

    #[test]
    fn erb_formula_matches_libdf() {
        // 验证关键频率的 ERB 值（对比 libDF freq2erb）
        let assert_close = |freq: f32, expected: f32, msg: &str| {
            let actual = freq_to_erb(freq);
            assert!(
                (actual - expected).abs() < 0.01,
                "{}: freq_to_erb({}) = {}, expected {}",
                msg,
                freq,
                actual,
                expected
            );
        };
        assert_close(0.0, 0.0, "DC");
        assert_close(1000.0, 15.58, "1kHz");
        assert_close(24000.0, 43.19, "Nyquist");

        // 逆函数往返
        for &f in &[100.0, 500.0, 1000.0, 5000.0, 15000.0, 24000.0] {
            let erb = freq_to_erb(f);
            let back = erb_to_freq(erb);
            assert!((back - f).abs() < 0.1, "erb 往返失败: {} → {} → {}", f, erb, back);
        }
    }

    // ── 归一化 ──

    #[test]
    fn band_mean_norm_centers_around_zero() {
        let n = 32;
        let mut xs = vec![60.0f32; n]; // 线性能量
        let mut state = vec![-60.0f32; n]; // 初始 EMA 状态
        band_mean_norm_erb(&mut xs, &mut state, NORM_ALPHA);
        // x_db = 10*log10(60) ≈ 17.78
        // new_state = 17.78*0.01 + (-60)*0.99 ≈ -59.22
        // output = (17.78 - (-59.22))/40 ≈ 1.925
        assert!(xs[0] > 1.5 && xs[0] < 2.5, "首帧 ERB 归一化值异常: {}", xs[0]);
    }

    #[test]
    fn band_unit_norm_normalizes_complex() {
        let mut xs = vec![Complex::new(10.0, 0.0)]; // |z|=10
        let mut state = vec![0.001]; // 初始状态
        band_unit_norm(&mut xs, &mut state, NORM_ALPHA);
        // mag=10, state = 10*0.01 + 0.001*0.99 ≈ 0.101
        // norm = sqrt(0.101) ≈ 0.318
        // x /= 0.318 → x.re ≈ 31.4
        assert!(xs[0].re > 30.0 && xs[0].re < 33.0, "归一化值异常: {}", xs[0].re);
    }

    #[test]
    fn init_state_lerp_correct_values() {
        let erb = init_state_lerp(MEAN_NORM_INIT, N_ERB);
        assert_eq!(erb.len(), N_ERB);
        assert!((erb[0] - (-60.0)).abs() < 1e-5);
        assert!((erb[N_ERB - 1] - (-90.0)).abs() < 1e-5);

        let df = init_state_lerp(UNIT_NORM_INIT, N_DF);
        assert_eq!(df.len(), N_DF);
        assert!((df[0] - 0.001).abs() < 1e-7);
        assert!((df[N_DF - 1] - 0.0001).abs() < 1e-7);
    }

    // ── compute_band_corr ──

    #[test]
    fn compute_band_corr_autocorrelation() {
        // 全 1 频谱 → 每个带功率 = (Σ1²/width)² = 1.0
        let spec = vec![Complex::new(1.0, 0.0); NBINS];
        let widths = erb_widths();
        let pow = compute_band_corr(&spec, &widths);
        for (i, &p) in pow.iter().enumerate() {
            assert!((p - 1.0).abs() < 1e-5, "band {} 功率 = {} (应=1.0)", i, p);
        }
    }

    // ── 模型集成测试（需 dfn3.onnx）──

    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn processor_runs_and_updates_gru_state() {
        let path = crate::config::find_df3()
            .expect("dfn3.onnx 未下载，跑: hf download penta2himajin/deepfilternet3-onnx");
        let mut p = DenoiseProcessor::new(&path).unwrap();
        let enc_before = p.enc_h.clone();
        // 需 CONV_LOOKAHEAD+1=3 帧才能触发首次推理
        let frame = vec![0.0f32; HOP];
        for _ in 0..3 {
            let _ = p.process_samples(&frame);
        }
        assert_ne!(p.enc_h, enc_before, "GRU enc_h 应在推理后更新");
    }

    #[test]
    #[ignore]
    fn sample_conservation_input_equals_output_length() {
        let path = crate::config::find_df3().unwrap();
        let mut p = DenoiseProcessor::new(&path).unwrap();
        let n = 48000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        // conv_lookahead 引入 2 帧=960 样本延迟，flush 填 2 零帧补齐
        assert_eq!(out.len(), input.len(), "样本守恒：in={} out={}", input.len(), out.len());
    }

    #[test]
    #[ignore]
    fn streaming_incremental_equals_batch() {
        let path = crate::config::find_df3().unwrap();
        let n = 48000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();

        let mut p1 = DenoiseProcessor::new(&path).unwrap();
        let mut batch = p1.process_samples(&input);
        batch.extend(p1.flush());

        let mut p2 = DenoiseProcessor::new(&path).unwrap();
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

        assert_eq!(incr.len(), batch.len(), "长度不一致");
        let max_diff = incr.iter().zip(batch.iter()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        eprintln!("streaming max_diff = {:.3e}", max_diff);
        assert!(max_diff < 1e-4, "增量 vs 批处理不一致，max_diff={}", max_diff);
    }
}
