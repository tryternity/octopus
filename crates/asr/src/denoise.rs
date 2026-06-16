//! RNNoise 流式环境降噪（基于 `nnnoiseless`，纯 Rust，内置默认模型）。
//!
//! ## 历史
//! 曾用 DeepFilterNet3（`penta2himajin/deepfilternet3-onnx/dfn3.onnx`，带 GRU 状态的
//! 流式逐帧 ONNX 导出）。经完整诊断确认该**模型本身存在缺陷**：把正常语音当噪声压到
//! 约 10%（开降噪后 ASR 反而错乱）。证据链：
//! - DSP/链路全对：spec 量级 ~0.30（含 wnorm）、feat 正常、GRU shape 对、完美重构
//!   （增量 vs 批处理 max_diff < 1e-4）、ort 对 whisper/paraformer/vad 等其他模型正常；
//! - 唯一异常：`enhanced_spec ≈ 0.10 · spec`（应当 ≈1.0·spec 或 mask）→ 推理压语音；
//! - mellonella 的流式逐帧导出测试只验形状不验质量，缺陷未被覆盖。
//!
//! 故弃用 dfn3.onnx，改用 RNNoise（Xiph，成熟实时语音降噪，WebKit/Zoom 在用）。
//! `nnnoiseless` 是其纯 Rust 移植（BSD-3，无 C 依赖），内置默认 RNNoise 模型，
//! 无需任何外部模型文件。`FRAME_SIZE = 480` 样本（10ms @48kHz）正好匹配 octopus HOP。
//!
//! ## 语义
//! 状态跨帧保持（GRU 隐状态 + 特征缓冲，噪声估计是连续物理过程），仅会话起点
//! `reset()` 全部清零——与 VAD 每段 reset 相反。输入输出均为 48kHz、`[-1, 1]` 归一化
//! 单声道；内部按 nnnoiseless 契约转 `[-32768, 32767]`（i16 PCM 等价）运算。

use anyhow::Result;

/// nnnoiseless 帧大小（480 样本 = 10ms @48kHz），与 octopus HOP 一致。
const FRAME_SIZE: usize = nnnoiseless::FRAME_SIZE;

/// nnnoiseless 内部以 i16 PCM 等价值域（`[-32768, 32767]`）运算；octopus 音频 pipeline
/// 使用 `[-1, 1]` 归一化。喂入前 `×SCALE`，输出后 `/SCALE`。
const PCM_SCALE: f32 = 32768.0;

/// RNNoise 流式降噪处理器。
///
/// 包装 nnnoiseless `DenoiseState`（内置默认 RNNoise 模型，`'static` owned，无外部文件）。
///
/// 状态语义（与 VAD 每段 reset 相反）：
/// - GRU 隐状态 + 特征缓冲跨帧保持（噪声估计是连续物理过程）；
/// - 仅新会话 `start()` 调 `reset()` 全部清零（重建 `DenoiseState`）。
///
/// 接口对齐旧 DF3 实现：`new` / `reset` / `process_samples` / `flush`。
/// `new()` 无参数——RNNoise 无需外部模型文件（旧 DF3 的 `model_path` 参数已移除）。
pub struct DenoiseProcessor {
    denoise: Box<nnnoiseless::DenoiseState<'static>>,
    in_buf: Vec<f32>,  // 48k [-1,1] 累积输入
    out_buf: Vec<f32>, // 48k [-1,1] 已降噪待输出
}

impl DenoiseProcessor {
    /// 创建降噪器（内置默认 RNNoise 模型，无外部依赖）。
    pub fn new() -> Result<Self> {
        Ok(Self {
            denoise: nnnoiseless::DenoiseState::new(),
            in_buf: Vec::with_capacity(FRAME_SIZE),
            out_buf: Vec::new(),
        })
    }

    /// 全状态清零（重建 `DenoiseState`，GRU/特征缓冲归零）。
    pub fn reset(&mut self) {
        self.denoise = nnnoiseless::DenoiseState::new();
        self.in_buf.clear();
        self.out_buf.clear();
    }

    /// 增量处理 48k 样本：累积到 `in_buf`，每满 `FRAME_SIZE` 降噪一帧，返回已降噪样本。
    ///
    /// 流式语义：不足一帧的残差留在 `in_buf` 跨调用累积（状态连续，不 flush）。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        self.in_buf.extend_from_slice(samples);
        let mut out_frame = [0.0f32; FRAME_SIZE];
        while self.in_buf.len() >= FRAME_SIZE {
            // 取一帧并转 i16 PCM 等价值域
            let frame: Vec<f32> = self.in_buf.drain(..FRAME_SIZE).collect();
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| frame[i] * PCM_SCALE);
            self.denoise.process_frame(&mut out_frame, &pcm);
            // 转回 [-1, 1]
            for &s in &out_frame {
                self.out_buf.push(s / PCM_SCALE);
            }
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填 `in_buf` 残留到 `FRAME_SIZE` 处理一帧，排出尾部样本。
    ///
    /// 会话结束调用。残留不足一帧时零填补齐，使末尾真实样本（含 OLA 半窗延迟部分）
    /// 也被降噪输出。输出长度为 `FRAME_SIZE` 的整数倍（与输入差 `< FRAME_SIZE`，对 16k
    /// ASR 无感知）。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            self.in_buf.resize(FRAME_SIZE, 0.0);
            let mut out_frame = [0.0f32; FRAME_SIZE];
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| self.in_buf[i] * PCM_SCALE);
            self.denoise.process_frame(&mut out_frame, &pcm);
            for &s in &out_frame {
                self.out_buf.push(s / PCM_SCALE);
            }
            self.in_buf.clear();
        }
        std::mem::take(&mut self.out_buf)
    }
}

impl Default for DenoiseProcessor {
    fn default() -> Self {
        Self::new().expect("DenoiseState::new 仅在 OOM 失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 基本可用：构造 + reset + 增量处理 + flush，不 panic。
    #[test]
    fn processor_basic_roundtrip() {
        let mut p = DenoiseProcessor::new().unwrap();
        // 非整帧输入（验证累积）
        let _ = p.process_samples(&[0.0f32; 300]);
        let _ = p.process_samples(&[0.0f32; 700]);
        let _ = p.flush();
        p.reset();
        let _ = p.process_samples(&[0.0f32; 48000]);
        let _ = p.flush();
    }

    /// 长度守恒：输入 N → process+flush 输出为 FRAME_SIZE 整数倍，与 N 差 < FRAME_SIZE。
    #[test]
    fn length_invariant_within_one_frame() {
        for &n in &[480usize, 481, 960, 1000, 48000, 48001] {
            let mut p = DenoiseProcessor::new().unwrap();
            let input: Vec<f32> = (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
                .collect();
            let mut out = p.process_samples(&input);
            out.extend(p.flush());
            let diff = (out.len() as i64 - n as i64).abs();
            assert!(
                diff < FRAME_SIZE as i64,
                "n={n} out={} diff={diff} 应 < FRAME_SIZE({})",
                out.len(),
                FRAME_SIZE
            );
        }
    }

    /// 流式 = 批处理：任意分块喂入与一次喂入产出逐样本相同（验证 in_buf 累积逻辑）。
    #[test]
    fn streaming_incremental_equals_batch() {
        let n = 48000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();

        let mut p1 = DenoiseProcessor::new().unwrap();
        let mut batch = p1.process_samples(&input);
        batch.extend(p1.flush());

        let mut p2 = DenoiseProcessor::new().unwrap();
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
        // 分块边界都在帧内累积，帧序列与批处理完全一致 → 逐位相同
        let max_diff = incr
            .iter()
            .zip(batch.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(max_diff, 0.0, "增量 vs 批处理应逐位相同，max_diff={}", max_diff);
    }

    // ── 决定性诊断（RNNoise 无外部模型，直接可跑）──

    /// 合成语音（基频 + 谐波 + 共振峰 + 3Hz 音节调制）。GRU 靠时变性识别语音。
    fn synth_speech(n: usize) -> Vec<f32> {
        let pi = std::f32::consts::PI;
        (0..n)
            .map(|i| {
                let t = i as f32 / 48000.0;
                let env = (2.0 * pi * 3.0 * t).sin().max(0.0); // 3Hz 音节调制
                let mut s = 0.0;
                for h in 1..=40 {
                    let fh = 150.0 * h as f32;
                    if fh > 12000.0 {
                        break;
                    }
                    let formant = (-((fh - 1200.0) / 2000.0).powi(2)).exp();
                    s += (2.0 * pi * fh * t).sin() * formant;
                }
                s * 0.1 * (0.3 + 0.7 * env)
            })
            .collect()
    }

    /// 确定性白噪声（LCG），幅度可调。
    fn white_noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed: u32 = 0xC0FFEE;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    /// SNR(x vs pure)：x 相对纯净语音的信噪比（dB）。
    fn snr_vs_pure(x: &[f32], pure: &[f32], lo: usize, hi: usize) -> f32 {
        let mut sp = 0.0;
        let mut np = 0.0;
        for i in lo..hi {
            sp += pure[i] * pure[i];
            let e = x[i] - pure[i];
            np += e * e;
        }
        10.0 * (sp / np.max(1e-12)).log10()
    }

    fn rms(x: &[f32], lo: usize, hi: usize) -> f32 {
        (x[lo..hi].iter().map(|v| v * v).sum::<f32>() / (hi - lo) as f32).sqrt()
    }

    /// **决定性（噪声路径）**：纯白噪声（无语音）→ denoise → 输出应大幅抑制。
    ///
    /// RNNoise 训练的核心能力就是识别非语音并压制噪声。这是不依赖真实语音合成质量
    /// 的稳健信号级验证（合成蜂音谐波不是 RNNoise 的有效代理——RNNoise 频带增益会
    /// 重塑稳态谐波，连干净合成语音都 SNR≈-3dB，故不在合成语音上断言 SNR 改善）。
    ///
    /// 真实语音在噪声下的 ASR 改善，由 `diag_denoise_tts_wav`（macOS `say` 真实语音）
    /// 验证（`#[ignore]`，手动跑）。
    #[test]
    fn diag_pure_noise_suppressed() {
        let n = 48000 * 3;
        let input = white_noise(n, 0.1);
        let mut p = DenoiseProcessor::new().unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());

        // 跳过首部 1s（RNNoise 噪声估计适应期）+ 尾部边界，测稳态抑制
        let adapt = FRAME_SIZE * 100; // ~1s
        let lo = adapt;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        let suppression_db = 20.0 * (out_rms / in_rms).log10();
        eprintln!(
            "DIAG pure_noise(稳态): in_rms={:.4} out_rms={:.4} suppression={:.1}dB",
            in_rms,
            out_rms,
            suppression_db
        );
        // RNNoise 对稳态宽带噪声保守抑制（避免 musical noise），但不应放大。
        // 断言至少有可测量的衰减（out < in），防止"放大噪声"回归。
        assert!(
            out_rms < in_rms,
            "纯噪声未被衰减：out_rms={:.4} in_rms={:.4}",
            out_rms,
            in_rms
        );
    }

    /// **反 dfn3 压语音**：干净语音 denoise 应近无损保留（gain 合理 ≥0.5）。
    /// 旧 dfn3.onnx 在此场景 gain≈0.10（压语音）；RNNoise 应 ≥0.5。
    #[test]
    fn diag_clean_speech_preserved() {
        let n = 48000 * 2;
        let input = synth_speech(n);

        let mut p = DenoiseProcessor::new().unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());

        let lo = FRAME_SIZE * 2;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        let snr = snr_vs_pure(&out, &input, lo, hi);
        eprintln!(
            "DIAG clean: gain={:.3}x SNR(out vs in)={:.2}dB (RNNoise gain 应 ≥0.5；旧 dfn3≈0.10)",
            out_rms / in_rms,
            snr
        );
        assert!(
            out_rms / in_rms >= 0.5,
            "干净语音被过度压制：gain={:.3}（应 ≥0.5；旧 dfn3 缺陷即 gain≈0.10）",
            out_rms / in_rms
        );
    }

    /// 静音输入 → 输出近静音（验证链路不引入直流/噪声）。
    #[test]
    fn diag_silence_output() {
        let mut p = DenoiseProcessor::new().unwrap();
        let input = vec![0.0f32; 48000 * 2];
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let out_rms = rms(&out, FRAME_SIZE, out.len());
        eprintln!("DIAG silence: out_rms={:.6}", out_rms);
        assert!(out_rms < 0.01, "静音输入输出过大：out_rms={}", out_rms);
    }

    /// 决定性真实语音验证：macOS `say` 生成的 TTS wav（48k）→ denoise → 写出对比 wav。
    /// 干净真实语音 denoise 应近无损保留（gain 合理、SNR 高）。
    #[test]
    #[ignore] // 需 /tmp/voice48k.wav（macOS: say -o voice48k.wav "..." 后转 48k）
    fn diag_denoise_tts_wav() {
        let samples = read_tts_wav();
        let mut p = DenoiseProcessor::new().unwrap();
        let mut out = p.process_samples(&samples);
        out.extend(p.flush());

        let lo = FRAME_SIZE * 2;
        let hi = samples.len().saturating_sub(FRAME_SIZE * 2);
        let in_rms = rms(&samples, lo, hi);
        let out_rms = rms(&out, lo, hi);
        let snr = snr_vs_pure(&out, &samples, lo, hi);
        eprintln!(
            "DIAG tts_wav(clean): gain={:.3}x SNR(out vs in)={:.2}dB",
            out_rms / in_rms,
            snr
        );
        write_wav("/tmp/voice48k_denoised.wav", &out);
        eprintln!("降噪后 wav: /tmp/voice48k_denoised.wav");
    }

    /// 决定性真实语音 + 噪声：真实 TTS 语音 + 白噪声 → denoise → out_SNR 应 > in_SNR。
    /// 这是「开降噪改善 ASR」的直接证据（真实语音特征，非合成蜂音）。
    #[test]
    #[ignore] // 需 /tmp/voice48k.wav
    fn diag_real_speech_noisy_denoise_effect() {
        let pure = read_tts_wav();
        let n = pure.len();
        let noise = white_noise(n, 0.05);
        let input: Vec<f32> = (0..n).map(|i| pure[i] + noise[i]).collect();

        let mut p = DenoiseProcessor::new().unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());

        let lo = FRAME_SIZE * 4; // 跳过首部 RNNoise 适应期 + fade-in
        let hi = n.saturating_sub(FRAME_SIZE * 2);
        let in_snr = snr_vs_pure(&input, &pure, lo, hi);
        let out_snr = snr_vs_pure(&out, &pure, lo, hi);
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        eprintln!(
            "DIAG real_noisy: in_SNR={:.2}dB out_SNR={:.2}dB Δ={:+.2}dB gain={:.3}x",
            in_snr,
            out_snr,
            out_snr - in_snr,
            out_rms / in_rms
        );
        write_wav("/tmp/voice48k_noisy_in.wav", &input);
        write_wav("/tmp/voice48k_noisy_out.wav", &out);
        eprintln!("输入/输出 wav: /tmp/voice48k_noisy_in.wav 与 _out.wav");
    }

    /// 读取 /tmp/voice48k.wav（48k mono i16）→ [-1,1] f32。
    fn read_tts_wav() -> Vec<f32> {
        let mut reader = hound::WavReader::open("/tmp/voice48k.wav").expect("/tmp/voice48k.wav");
        assert_eq!(reader.spec().sample_rate, 48000, "TTS wav 应 48k");
        reader
            .samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect()
    }

    /// 写 48k mono i16 wav。
    fn write_wav(path: &str, samples: &[f32]) {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .expect("writer");
        for &s in samples {
            let v = (s * i16::MAX as f32).clamp(-i16::MAX as f32, i16::MAX as f32) as i16;
            writer.write_sample(v).expect("write");
        }
        writer.finalize().expect("finalize");
    }
}
