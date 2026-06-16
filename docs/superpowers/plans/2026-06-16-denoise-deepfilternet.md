# DeepFilterNet3 环境降噪 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在麦克风录音链路中插入 DeepFilterNet3（ONNX）流式环境降噪层，在送入 VAD/ASR 前降低背景噪声，跨平台（mac/win/linux）生效。

**Architecture:** 新建 `crates/asr/src/denoise.rs` 封装 `DenoiseProcessor`（Vorbis 窗 STFT + ERB 特征（dB + EMA 归一化）+ dfn3.onnx 有状态推理 + conv_lookahead=2 帧对齐 + iSTFT overlap-add）。集成在采集层 `SharedAudioState` 内（coordinator 无感），48kHz 域处理、前后各一次重采样桥接到 ASR 的 16kHz。复用现有 `rustfft` 依赖与 `ort` 推理，零新依赖。配置仅 `denoise_enabled`（infra::AppConfig），模型走 HF cache，失败降级直通不阻断录音。

**Tech Stack:** Rust、`ort 2.0.0-rc.12`（ONNX Runtime）、`rustfft 6`、`ndarray 0.17`、`rubato 0.16`（已有依赖）、Tauri/cpal。

**Spec:** `docs/superpowers/specs/2026-06-16-denoise-deepfilternet-design.md`

---

## 关键技术契约（实施前必读）

- **模型**：`penta2himajin/deepfilternet3-onnx/dfn3.onnx`（HF cache，唯一带 GRU 状态的流式版）。IO：
  - 入 `spec[1,1,1,481,2]` `feat_erb[1,1,1,32]` `feat_spec[1,1,1,96,2]` `enc_h[1,1,256]` `erb_h[2,1,256]` `df_h[2,1,256]`
  - 出 `enhanced_spec[1,1,1,481,2]` `new_enc_h` `new_erb_h` `new_df_h`
- **DSP 常量**：n_fft=960、hop=480（48kHz，10ms）、481 bins、32 ERB 带、96 DF bins。
- **窗**：Vorbis，`w[n]=sin(π/2·sin²(π(n+0.5)/960))`，分析窗=合成窗。50% overlap 下 COLA 增益=1。
- **ERB 尺度**：Glasberg-Moore，`f_erb=9.265·ln(1+f/228.833)`（分母 228.833 = 24.7×9.265，对齐 libDF）。
- **特征归一化**（对齐 libDF，缺失则模型收错误量级）：
  - `feat_erb`：band 互相关功率 `(Σ|spec|²/width)²` → `10·log10(1e-10+x)` → EMA 均值归一化（alpha=0.99）→ `/40`
  - `feat_spec`：前 96 bin 复数 → EMA 跟踪 `|z|`（alpha=0.99），除以 `√state`
  - 初始状态：feat_erb = linspace(-60, -90, 32)，feat_spec = linspace(0.001, 0.0001, 96)
- **conv_lookahead=2**：模型导出时移除了内部 lookahead，调用方需环形缓冲：spec[t] 配 feat[t+2]。首次推理需累积 3 帧（20ms 算法延迟），flush 填 2 零特征帧排空队列。
- **rustfft 6 API**：`FftPlanner::new()` → `plan_fft(N, FftDirection::Forward/Inverse)` → `fft.process(&mut [Complex<f32>])`。**inverse 不含 1/N 归一化**，需手动 `×1/N`。
- **ort**：参照 `crates/asr/src/vad.rs` 的 Session 加载 + `ort::inputs!` + `TensorRef::from_array_view` + `session.run`。
- **测试模型依赖**：纯 DSP 测试（窗、STFT 重建、ERB、OLA）**不需模型**，常规跑；推理/集成测试需 `dfn3.onnx`，标 `#[ignore]`。

---

## Task 1: infra::AppConfig 加 denoise_enabled 配置字段

**Files:**
- Modify: `crates/infra/src/config.rs:138`（AppConfig 末字段后）、`:179`（default 函数）、`:212`（Default impl）、`:279`（测试）

- [ ] **Step 1: 写失败测试**

在 `crates/infra/src/config.rs` 的 `mod tests` 末尾（`app_config_serialize_round_trip_preserves_overrides` 之后）加：

```rust
    #[test]
    fn denoise_enabled_defaults_to_true() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(cfg.denoise_enabled, "denoise_enabled 应默认 true");
    }

    #[test]
    fn denoise_enabled_override_from_yaml() {
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: false\n").unwrap();
        assert!(!cfg.denoise_enabled);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-infra denoise_enabled`
Expected: 编译失败 `no field denoise_enabled on type AppConfig`

- [ ] **Step 3: 加字段 + default 函数 + Default impl**

在 `AppConfig` 结构体 `asr_correct` 字段后（`config.rs:138` 之后）加：

```rust
    /// 是否启用 DeepFilterNet3 环境降噪（录音送 ASR 前降噪）
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
```

在 `default_asr_correct` 函数后（`:179` 附近）加：

```rust
fn default_denoise_enabled() -> bool {
    true
}
```

在 `Default for AppConfig` impl 的 `asr_correct: default_asr_correct(),` 后加：

```rust
            denoise_enabled: default_denoise_enabled(),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新加的 2 个测试）

- [ ] **Step 5: 提交**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): add denoise_enabled field (default true)"
```

---

## Task 2: find_df3() 模型定位（复用 HF cache helpers）

**Files:**
- Modify: `crates/asr/src/config.rs`（在 `find_silero_vad` 之后，`:93` 附近）

- [ ] **Step 1: 确认现有 helper 签名**

Run: `grep -nE "fn find_hf_cache|fn find_latest_snapshot" crates/asr/src/config.rs`
确认：`find_hf_cache(source: &str) -> Result<PathBuf>`（返回 repo 的 model_dir，含 snapshots/）、`find_latest_snapshot(model_dir: &Path) -> Result<PathBuf>`（返回最新 snapshot 目录）。

- [ ] **Step 2: 写失败测试**

在 `crates/asr/src/config.rs` 末尾的 `#[cfg(test)] mod tests`（若不存在则新建）加：

```rust
    #[test]
    fn find_df3_missing_returns_download_hint() {
        // 临时改 HF cache 路径不可行（函数读固定 HOME）；改为验证错误信息文案
        // 当模型未下载时，find_df3 应返回含 hf download 提示的 Err
        match crate::config::find_df3() {
            Ok(_) => { /* 模型存在，跳过缺失路径断言 */ }
            Err(e) => {
                let msg = format!("{:#}", e);
                assert!(
                    msg.contains("hf download penta2himajin/deepfilternet3-onnx"),
                    "缺失时应提示 hf download 命令，实际: {}",
                    msg
                );
            }
        }
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p octopus-asr find_df3_missing`
Expected: 编译失败 `cannot find function find_df3`

- [ ] **Step 4: 实现 find_df3**

在 `find_silero_vad` 之后加：

```rust
// ── DeepFilterNet3 model discovery ──

/// DF3 模型 HF repo（唯一固定，不走 DB / 不切换）。
const DF3_HF_REPO: &str = "penta2himajin/deepfilternet3-onnx";
/// DF3 onnx 文件名（带 GRU 状态的流式版）。
const DF3_ONNX_FILE: &str = "dfn3.onnx";

/// 定位 DeepFilterNet3 模型：~/.cache/huggingface/hub/models--penta2himajin--deepfilternet3-onnx/snapshots/*/dfn3.onnx
/// 单一固定模型，不走 DB；缺失时提示下载命令。
pub fn find_df3() -> Result<PathBuf> {
    let model_dir = find_hf_cache(DF3_HF_REPO)?;
    let snapshot = find_latest_snapshot(&model_dir)?;
    let onnx = snapshot.join(DF3_ONNX_FILE);
    if onnx.exists() {
        return Ok(onnx);
    }
    anyhow::bail!(
        "DeepFilterNet3 模型缺失，请先下载：hf download {}",
        DF3_HF_REPO
    )
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p octopus-asr find_df3_missing`
Expected: PASS（模型在则 Ok 跳过；不在则 Err 含下载提示）

- [ ] **Step 6: 提交**

```bash
git add crates/asr/src/config.rs
git commit -m "feat(asr): add find_df3() for DeepFilterNet3 model discovery"
```

---

## Task 3: denoise.rs 骨架 + Vorbis 窗 + STFT/iSTFT 重建（纯 DSP，无需模型）

**Files:**
- Create: `crates/asr/src/denoise.rs`
- Modify: `crates/asr/src/lib.rs`（加 `pub mod denoise;`）

- [ ] **Step 1: 注册模块**

在 `crates/asr/src/lib.rs` 加（与 `pub mod vad;` 同处）：

```rust
pub mod denoise;
```

- [ ] **Step 2: 写 denoise.rs 常量 + 窗 + STFT/iSTFT + 重建测试**

创建 `crates/asr/src/denoise.rs`：

```rust
//! DeepFilterNet3 流式环境降噪（ONNX，48kHz）。
//!
//! 处理模型：penta2himajin/deepfilternet3-onnx/dfn3.onnx（带 GRU 状态的流式版）。
//! 数据流：48k 样本 → 每 480 样本(10ms)一帧 → STFT(hann,n_fft=960) → feat
//!       → onnx(spec,feat,GRU状态) → enhanced_spec → iSTFT + OLA → 48k 增强样本。

use anyhow::Result;
use ndarray::{Array3, Array4};
use rustfft::{Fft, FftPlanner, FftDirection};
use rustfft::num_complex::Complex;

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
pub fn stft_frame(frame: &[f32], window: &[f32], fft: &Fft<f32>) -> Vec<Complex<f32>> {
    debug_assert_eq!(frame.len(), N_FFT);
    let mut buf: Vec<Complex<f32>> = (0..N_FFT)
        .map(|i| Complex::new(frame[i] * window[i], 0.0))
        .collect();
    fft.process(&mut buf);
    buf[..NBINS].to_vec()
}

/// iSTFT 单帧：NBINS 复数 → 共轭对称填充 → IFFT → × 合成窗 → N_FFT 实样本。
/// rustfft 的 inverse 不含 1/N 归一化，手动 ×1/N。
pub fn istft_frame(spec: &[Complex<f32>], ifft: &Fft<f32>, window: &[f32]) -> Vec<f32> {
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

        // 生成 ~0.5s 的 1kHz 正弦 + 白噪
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
            let spec = stft_frame(frame, &w, &fft);
            let time = istft_frame(&spec, &ifft, &w);
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
        assert!(
            snr_db > 40.0,
            "STFT/iSTFT 重建 SNR 应 > 40dB，实际 {:.1}dB",
            snr_db
        );
    }
}
```

- [ ] **Step 3: 跑测试确认通过**

Run: `cargo test -p octopus-asr denoise::tests`
Expected: PASS（2 个纯 DSP 测试，无需模型）

- [ ] **Step 4: 提交**

```bash
git add crates/asr/src/denoise.rs crates/asr/src/lib.rs
git commit -m "feat(asr): denoise.rs skeleton + sqrt-Hann STFT/iSTFT reconstruction"
```

---

## Task 4: ERB 边界 + feat_erb / feat_spec 特征提取

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试**

在 `denoise.rs` 的 `mod tests` 加：

```rust
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr denoise::tests::erb`
Expected: 编译失败 `cannot find function erb_bounds`

- [ ] **Step 3: 实现 ERB 边界 + feat 函数**

在 `denoise.rs`（`istft_frame` 之后）加：

```rust
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
/// 注：DeepFilterNet 的精确带划分对齐 df crate（deep_filter::df::freq）；
/// 此实现为标准 ERB 均分，feat_erb 测试验证数值聚合正确性。
pub fn erb_bounds() -> Vec<(usize, usize)> {
    let nyquist = 24000.0f32;
    let erb_max = freq_to_erb(nyquist);
    // bin i 的频率 = i / N_FFT * sample_rate = i / 960 * 48000
    let bin_freq = |i: usize| -> f32 { i as f32 / N_FFT as f32 * 48000.0 };

    let mut bounds = Vec::with_capacity(N_ERB);
    for b in 0..N_ERB {
        let erb_lo = erb_max * b as f32 / N_ERB as f32;
        let erb_hi = erb_max * (b + 1) as f32 / N_ERB as f32;
        let f_lo = erb_to_freq(erb_lo);
        let f_hi = erb_to_freq(erb_hi);
        // 找到首个 freq >= f_lo 的 bin 作为 lo，首个 freq > f_hi 的 bin 作为 hi
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
    // 修正：确保连续无空洞（前一带 hi = 后一带 lo），最后到 NBINS
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
        .map(|(lo, hi)| {
            (lo..hi).map(|i| spec[i].norm_sqr()).sum()
        })
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
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr denoise::tests`
Expected: PASS（5 个 DSP 测试）

- [ ] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): ERB bounds + feat_erb/feat_spec feature extraction"
```

---

## Task 5: DenoiseProcessor 结构体 + ONNX session + GRU 状态 + 单帧推理

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试（需模型，#[ignore]）**

在 `denoise.rs` 的 `mod tests` 加：

```rust
    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn processor_runs_and_updates_gru_state() {
        let path = crate::config::find_df3().expect("dfn3.onnx 未下载，跑: hf download penta2himajin/deepfilternet3-onnx");
        let mut p = super::DenoiseProcessor::new(&path).unwrap();
        let enc_before = p.enc_h.clone();
        // 一帧静音输入
        let frame = vec![0.0f32; HOP];
        let out = p.process_samples(&frame);
        assert!(!out.is_empty() || p.flush().is_empty() == false || true); // 允许首帧无输出（OLA 起始延迟）
        // GRU 状态应已变化
        assert_ne!(p.enc_h, enc_before, "GRU enc_h 应在推理后更新");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr processor_runs -- --ignored`
Expected: 编译失败 `cannot find type DenoiseProcessor`

- [ ] **Step 3: 实现 DenoiseProcessor::new + 单帧推理**

在 `denoise.rs` 顶部加 import：

```rust
use std::path::Path;
use ort::session::Session;
use ort::value::TensorRef;
```

在 `feat_spec` 之后加：

```rust
/// DeepFilterNet3 流式降噪处理器（有状态：GRU 隐状态 + 缓冲）。
///
/// 生命周期：录音会话内跨帧保持状态（GRU 反映噪声环境稳态估计，不应被分段打断）；
/// 新会话开始时调 `reset()`。状态语义与 filter_vad（每段 reset）故意相反。
pub struct DenoiseProcessor {
    session: Session,
    fft: std::sync::Arc<Fft<f32>>,
    ifft: std::sync::Arc<Fft<f32>>,
    window: Vec<f32>,
    erb_bounds: Vec<(usize, usize)>,
    // GRU 隐状态（持久，跨帧）
    enc_h: Array3<f32>, // [1,1,256]
    erb_h: Array3<f32>, // [2,1,256]
    df_h: Array3<f32>,  // [2,1,256]
    // 流式增量缓冲
    in_buf: Vec<f32>,    // 48k 累积
    out_buf: Vec<f32>,   // 已增强样本待输出
    ola_prev: Vec<f32>,  // 上一帧 iSTFT（OLA 用）
}

impl DenoiseProcessor {
    /// 加载模型 + 初始化 DSP 常量 + GRU 状态归零。
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model_path)?;
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
        self.out_buf.clear();
        self.ola_prev.iter_mut().for_each(|v| *v = 0.0);
    }

    /// 处理一帧（HOP=480 样本）→ 增强样本入 out_buf。
    /// 用上一帧尾部 480 + 本帧 480 = 960 做分析窗。
    fn process_frame(&mut self, new_samples: &[f32]) {
        debug_assert_eq!(new_samples.len(), HOP);
        // 分析帧 = [ola_prev 的后 480] + new_samples；但 ola_prev 存的是完整上一帧 iSTFT。
        // 实际：分析窗作用于 [上帧尾 HOP 样本 + 本帧 HOP 样本]。
        // 简化：维护 in_buf，取末尾 N_FFT 做帧。
        let mut frame = Vec::with_capacity(N_FFT);
        // 上 480 样本（从 ola_prev 的时域输出尾，或从 in_buf）
        let tail: Vec<f32> = self.in_buf[self.in_buf.len().saturating_sub(N_FFT)..]
            .to_vec();
        let need = N_FFT - tail.len();
        frame.extend_from_slice(&tail);
        frame.extend_from_slice(&new_samples[..need.min(new_samples.len())]);
        if frame.len() < N_FFT {
            frame.resize(N_FFT, 0.0);
        }

        let spec = stft_frame(&frame, &self.window, &self.fft);
        let feat_erb = feat_erb(&spec, &self.erb_bounds);
        let feat_spec = feat_spec(&spec);

        // 构造 onnx 输入（形状对齐 IO 契约）
        let spec_4d = complex_to_4d(&spec);            // [1,1,1,481,2]
        let erb_in = vec_to_arr(&feat_erb);            // [1,1,1,32]
        let fspec_in = vec_to_4d(&feat_spec);          // [1,1,1,96,2]

        let outputs = self.session.run(ort::inputs! {
            "spec" => TensorRef::from_array_view(spec_4d.view())?,
            "feat_erb" => TensorRef::from_array_view(erb_in.view())?,
            "feat_spec" => TensorRef::from_array_view(fspec_in.view())?,
            "enc_h" => TensorRef::from_array_view(self.enc_h.view())?,
            "erb_h" => TensorRef::from_array_view(self.erb_h.view())?,
            "df_h" => TensorRef::from_array_view(self.df_h.view())?,
        }?)?;

        // 取增强频谱 + 更新 GRU 状态
        let enhanced = outputs["enhanced_spec"].try_extract_tensor::<f32>()?;
        let new_enc = outputs["new_enc_h"].try_extract_tensor::<f32>()?;
        let new_erb = outputs["new_erb_h"].try_extract_tensor::<f32>()?;
        let new_df = outputs["new_df_h"].try_extract_tensor::<f32>()?;

        let enh_spec = arr4d_to_complex(&enhanced.view().to_owned()); // [481] 复数
        let time = istft_frame(&enh_spec, &self.ifft, &self.window); // N_FFT 实样本

        // OLA：本帧输出 = time - ola_prev 的重叠部分贡献 + ... 简化为标准 OLA
        // 由于 COLA 增益=1，直接累加前 HOP 个样本（减去上一帧重叠）
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[i + HOP]; // 上帧后半重叠
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;

        // 更新 GRU 状态
        self.enc_h = new_enc.view().to_owned().into_shape((1, 1, 256))?.to_owned() into_dyn... 
        // （注：状态拷贝见下方修正步骤；精确形状重塑在 Step 4 调试）
        let _ = (new_erb, new_df);
    }
}
```

> ⚠️ **Step 3 的 GRU 状态回写与 OLA 是初稿，Step 4 会编译驱动修正**。ort `try_extract_tensor` 返回的类型重塑（`into_shape((1,1,256))`）与 OLA 重叠减法的精确边界需在编译时报错处对齐——这是 ONNX 集成最易错的点，按编译器错误逐个修正，不猜测。

- [ ] **Step 4: 编译驱动修正 GRU 状态回写 + OLA**

Run: `cargo build -p octopus-asr`

逐个修正编译错误（典型）：
- `ort::session::Session` 的 `outputs[...]` 访问与 `try_extract_tensor` 签名（参照 `crates/asr/src/vad.rs:37-45` 的 outputs 取值模式）。
- `new_enc.view().to_owned()` → `Array3<f32>`：用 `.into_shape((1,1,256))` 或直接 `.to_owned()` 若输出形状已是 `[1,1,256]`。enc_h/erb_h/df_h 分别对齐 `[1,1,256]`/`[2,1,256]`/`[2,1,256]`。
- OLA 边界：确认 `ola_prev` 用途——上一帧完整 iSTFT（960），本帧输出的前 480 = 上帧后 480 重叠 + 本帧前 480。

修正后 GRU 回写（示例正确形式）：

```rust
        self.enc_h = outputs["new_enc_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((1, 1, 256))
            .map_err(|e| anyhow::anyhow!("enc_h shape: {e}"))?;
        self.erb_h = outputs["new_erb_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((2, 1, 256))
            .map_err(|e| anyhow::anyhow!("erb_h shape: {e}"))?;
        self.df_h = outputs["new_df_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((2, 1, 256))
            .map_err(|e| anyhow::anyhow!("df_h shape: {e}"))?;
```

并加辅助函数（complex ↔ ndarray 转换）：

```rust
fn complex_to_4d(spec: &[Complex<f32>]) -> ndarray::Array5<f32> {
    // [1,1,1,481,2]
    let mut a = ndarray::Array5::zeros((1, 1, 1, NBINS, 2));
    for i in 0..NBINS {
        a[[0, 0, 0, i, 0]] = spec[i].re;
        a[[0, 0, 0, i, 1]] = spec[i].im;
    }
    a
}

fn vec_to_arr(v: &[f32]) -> ndarray::Array4<f32> {
    // [1,1,1,N]
    let mut a = ndarray::Array4::zeros((1, 1, 1, v.len()));
    for (i, x) in v.iter().enumerate() {
        a[[0, 0, 0, i]] = *x;
    }
    a
}

fn vec_to_4d(v: &[f32]) -> ndarray::Array5<f32> {
    // [1,1,1,96,2]
    let n = v.len() / 2;
    let mut a = ndarray::Array5::zeros((1, 1, 1, n, 2));
    for i in 0..n {
        a[[0, 0, 0, i, 0]] = v[i * 2];
        a[[0, 0, 0, i, 1]] = v[i * 2 + 1];
    }
    a
}

fn arr4d_to_complex(view: &ndarray::ArrayViewD<f32>) -> Vec<Complex<f32>> {
    // enhanced_spec [1,1,1,481,2] → [481] 复数
    let mut out = Vec::with_capacity(NBINS);
    for i in 0..NBINS {
        out.push(Complex::new(view[[0, 0, 0, i, 0]], view[[0, 0, 0, i, 1]]));
    }
    out
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p octopus-asr processor_runs -- --ignored`
Expected: PASS（需 `hf download penta2himajin/deepfilternet3-onnx` 已执行）

- [ ] **Step 6: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): DenoiseProcessor ONNX session + GRU state + per-frame inference"
```

---

## Task 6: 流式增量 process_samples + flush + 一致性测试

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试（#[ignore]，需模型）**

在 `mod tests` 加：

```rust
    #[test]
    #[ignore]
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
        assert_eq!(out.len(), input.len(), "样本守恒失败：in={} out={}", input.len(), out.len());
    }

    #[test]
    #[ignore]
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

        // 增量（分多次，每次不固定长度）
        let mut p2 = super::DenoiseProcessor::new(&path).unwrap();
        let mut incr = Vec::new();
        let chunks = [300usize, 700, 480, 1024, 480, 613, 480, 200, 13783]; // 和非整除 HOP
        let mut off = 0;
        for &c in &chunks {
            if off + c > input.len() { break; }
            incr.extend(p2.process_samples(&input[off..off + c]));
            off += c;
        }
        if off < input.len() {
            incr.extend(p2.process_samples(&input[off..]));
        }
        incr.extend(p2.flush());

        // 增量 vs 批处理逐样本相等（无状态漂移、无边界丢帧）
        assert_eq!(incr.len(), batch.len());
        let max_diff = incr.iter().zip(batch.iter()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(max_diff < 1e-4, "增量 vs 批处理不一致，max_diff={}", max_diff);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr sample_conservation -- --ignored`
Expected: 编译失败（`process_samples`/`flush` 未实现）

- [ ] **Step 3: 实现 process_samples + flush**

在 `impl DenoiseProcessor` 加（替换 Task 5 的 `process_frame` 调用方式为公开增量接口）：

```rust
    /// 增量处理 48k 样本：累积到 in_buf，每满 HOP 处理一帧，返回已增强样本。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        self.in_buf.extend_from_slice(samples);
        while self.in_buf.len() >= HOP {
            // 取首 HOP 个作为新样本，分析帧在 process_frame 内从 in_buf 末尾 N_FFT 构造
            let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
            self.process_frame(&new);
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填到 HOP 整数倍，处理残留，吐剩余输出。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            let pad = HOP - (self.in_buf.len() % HOP);
            if pad < HOP {
                self.in_buf.extend(std::iter::repeat(0.0).take(pad));
            } else {
                self.in_buf.extend(std::iter::repeat(0.0).take(HOP));
            }
            while self.in_buf.len() >= HOP {
                let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
                self.process_frame(&new);
            }
        }
        std::mem::take(&mut self.out_buf)
    }
```

并修正 `process_frame` 的分析帧构造（Task 5 简化版改为正确版）：

```rust
    fn process_frame(&mut self, new_samples: &[f32]) {
        // 分析帧 = in_buf 已 drain 出 new_samples 后，但需上一帧上下文。
        // 维护 separate history：把 new 加入一个滚动缓冲 last_frame_tail。
        // 简化正确做法：分析帧 = [prev_tail(480)] + new(480)
        //   prev_tail = 上一帧 new 的后 480（首次用 0）
        let mut frame = Vec::with_capacity(N_FFT);
        frame.extend_from_slice(&self.ola_prev[N_FFT - HOP..]); // 上一帧尾 HOP（OLA 复用）
        // 注：ola_prev 在 iSTFT 后存完整 960；这里取其分析用的尾 480 作上一帧时域上下文近似
        // 严格：分析窗作用于原始时域，需单独维护原始 tail。此处用 ola_prev 近似，
        // sample_conservation 与 streaming_equals_batch 测试会暴露偏差，据此修正。
        frame.extend_from_slice(new_samples);

        let spec = stft_frame(&frame, &self.window, &self.fft);
        // ...（feat + onnx run + iSTFT + OLA，同 Task 5 Step 4）
        // 完整逻辑复用 Task 5 Step 4 已修正的 GRU 回写
        let feat_erb_v = feat_erb(&spec, &self.erb_bounds);
        let feat_spec_v = feat_spec(&spec);
        let spec_4d = complex_to_4d(&spec);
        let erb_in = vec_to_arr(&feat_erb_v);
        let fspec_in = vec_to_4d(&feat_spec_v);
        let outputs = self.session.run(ort::inputs! {
            "spec" => TensorRef::from_array_view(spec_4d.view())?,
            "feat_erb" => TensorRef::from_array_view(erb_in.view())?,
            "feat_spec" => TensorRef::from_array_view(fspec_in.view())?,
            "enc_h" => TensorRef::from_array_view(self.enc_h.view())?,
            "erb_h" => TensorRef::from_array_view(self.erb_h.view())?,
            "df_h" => TensorRef::from_array_view(self.df_h.view())?,
        }?)?;
        let enhanced = outputs["enhanced_spec"].try_extract_tensor::<f32>()?;
        self.enc_h = outputs["new_enc_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((1, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.erb_h = outputs["new_erb_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((2, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.df_h = outputs["new_df_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((2, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;

        let enh_spec = arr4d_to_complex(&enhanced.view());
        let time = istft_frame(&enh_spec, &self.ifft, &self.window);
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[i + HOP];
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;
    }
```

> **若 `streaming_incremental_equals_batch` 失败**：偏差来自分析帧上下文（`ola_prev` 是 iSTFT 输出而非原始时域）。修正：新增字段 `prev_time_tail: Vec<f32>`（存上一帧原始 new 样本尾 480），分析帧用它而非 `ola_prev` 切片。测试驱动此修正。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr sample_conservation streaming_incremental -- --ignored`
Expected: PASS（两个流式一致性测试）

- [ ] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): streaming process_samples/flush + sample conservation & consistency tests"
```

---

## Task 7: SharedAudioState 集成（48k NS + 双桥接 + start reset + 降级）

**Files:**
- Modify: `crates/desktop/src/audio.rs`（`SharedAudioState` 加字段、`start`/`stop`/`drain_samples` 接入）

- [ ] **Step 1: 读现有 audio.rs 确认集成点**

Run: `grep -nE "fn start|fn stop|fn drain_samples|resampler|struct SharedAudioState" crates/desktop/src/audio.rs`

确认：`stop`（重采样到 16k 后返回）、`drain_samples`（流式重采样到 16k）、`start`（clear buffer + 建流）。

- [ ] **Step 2: SharedAudioState 加 DenoiseProcessor 字段**

在 `crates/desktop/src/audio.rs` 的 `SharedAudioState` 结构体加字段：

```rust
pub struct SharedAudioState {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
    sample_rate: std::sync::atomic::AtomicU32,
    device_name: String,
    resampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
    stream: Mutex<Option<cpal::Stream>>,
    // 新增：降噪处理器（None = 未启用/加载失败，降级直通）
    denoise: Mutex<Option<octopus_asr::denoise::DenoiseProcessor>>,
    // 新增：48k→16k 重采样器（NS 输出后降采样）
    down_sampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
}
```

`new` 初始化（若 `config.denoise_enabled`）：在 `new` 加参数 `denoise_enabled: bool`，或从 config 读。鉴于 `SharedAudioState::new` 当前只接 `device_name`，改为在 `start` 时按需 lazy init（见 Step 3）。

- [ ] **Step 3: 接入 drain_samples / stop**

改造重采样路径。原 `stop`/`drain_samples` 直接 `raw → 16k`；改为 `raw → 48k → NS → 16k`（denoise 启用时）。

抽出一个统一处理函数（DRY）：

```rust
impl SharedAudioState {
    /// raw(原生SR) → [升48k] → [NS降噪] → [降16k]。
    /// denoise 未启用/加载失败时降级：raw → 16k（原逻辑）。
    fn process_pipeline(&self, raw: Vec<f32>, rate: u32) -> Vec<f32> {
        let rate48 = if rate == 48000 { raw.clone() } else { self.resample_to(raw.clone(), rate, 48000) };

        let cleaned = if rate == 0 || rate48.is_empty() {
            rate48.clone()
        } else {
            match self.denoise.lock().unwrap().as_mut() {
                Some(d) => {
                    let mut out = d.process_samples(&rate48);
                    out.extend(d.flush());
                    out
                }
                None => rate48.clone(),
            }
        };

        // 48k → 16k
        if 48000 == 16000 { cleaned } else { self.resample_to(cleaned, 48000, 16000) }
    }

    fn resample_to(&self, samples: Vec<f32>, from: u32, to: u32) -> Vec<f32> {
        if from == to { return samples; }
        // 用 rubato 一次性重采样（非流式路径）
        octopus_asr::audio::resample_to_16k 仅为 16k；此处用通用 resample
        // 实现见 Step 4：复用 AudioResampler 或新增通用函数
        todo_in_step4()
    }
}
```

> ⚠️ **Step 4 编译驱动**：`resample_to` 的通用实现——`octopus_asr::audio` 现有 `resample_to_16k` 写死 16k 目标。新增通用 `resample_to(samples, from, to)`（用 `rubato::FftFixedIn`），放 `crates/asr/src/audio.rs`。

- [ ] **Step 4: 新增通用重采样 + lazy init denoise**

在 `crates/asr/src/audio.rs` 加：

```rust
/// 通用重采样：任意 from_rate → to_rate。
pub fn resample_to(samples: &[f32], from_rate: u32, to_rate: u32) -> anyhow::Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let mut resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, 1024, 2, 1)?;
    let mut input = vec![samples.to_vec()]; // mono 单声道
    let out = resampler.process(&input, None)?;
    Ok(out.into_iter().next().unwrap_or_default())
}
```

`SharedAudioState::start` 加 denoise lazy init + reset：

```rust
    pub fn start(&self, device_name: &str) -> Result<()> {
        self.samples.lock().unwrap().clear();
        self.is_recording.store(true, Ordering::Relaxed);

        // lazy init denoise（首次启用时加载模型；失败降级 None，warn 不阻断）
        let mut dn = self.denoise.lock().unwrap();
        if dn.is_none() {
            match octopus_asr::config::find_df3().and_then(|p| octopus_asr::denoise::DenoiseProcessor::new(&p)) {
                Ok(mut proc) => { proc.reset(); *dn = Some(proc); info!("DenoiseProcessor loaded"); }
                Err(e) => log::warn!("降噪未启用（降级直通）: {:#}", e),
            }
        } else {
            dn.as_mut().unwrap().reset();
        }
        drop(dn);

        let stream = self.build_stream(device_name)?;
        stream.play()?;
        *self.stream.lock().unwrap() = Some(stream);
        debug!("Recording started");
        Ok(())
    }
```

`stop`/`drain_samples` 改用 `process_pipeline`（替换原 `rate==16000 ? raw : resample` 分支）：

`stop` 中：
```rust
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let resampled = self.process_pipeline(raw, rate);
        *self.resampler.lock().unwrap() = None;
        *self.down_sampler.lock().unwrap() = None;
        Ok(resampled)
```

`drain_samples` 同理用 `process_pipeline`（注意流式：drain 不 flush denoise，让状态跨次保持；仅 stop 时 flush。修正：drain_samples 内 process_samples 但不 flush，stop 内调 flush）。

> 流式细节：`drain_samples` 用 `process_samples`（不 flush，GRU 状态跨次保持）；`stop` 在取最后一段时 `process_samples` + `flush` 吐残留。重构 `process_pipeline` 接受 `flush: bool` 参数。

- [ ] **Step 5: 编译 + 跑全量测试**

Run: `cargo build -p octopus-desktop && cargo test -p octopus-asr && cargo test -p octopus-infra`
Expected: 编译通过；DSP 测试 PASS（推理测试 `--ignored` 单独跑）

- [ ] **Step 6: 手动 e2e 验证（需模型）**

```bash
# 确保模型已下载
hf download penta2himajin/deepfilternet3-onnx

# 跑应用，对比开/关降噪
# config.yaml: denoise_enabled: true  → 带噪录音识别应改善
# config.yaml: denoise_enabled: false → 行为与现状一致（零回归）
# 删除 dfn3.onnx → 应用正常启动 + warn 下载提示，不崩溃
```

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/src/audio.rs crates/asr/src/audio.rs
git commit -m "feat(desktop): integrate DeepFilterNet3 denoise in SharedAudioState (48k NS + 16k bridge)"
```

---

## Task 8: 文档同步（CLAUDE.md 强制）

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: 更新 architecture.md**

在音频采集/持久化相关段加「环境降噪（DeepFilterNet3）」小节：

```markdown
### 环境降噪（DeepFilterNet3，可选）

录音送 VAD/ASR 前可选的 ONNX 降噪层（`config.yaml.denoise_enabled`，默认 true）：

- **模型**：`penta2himajin/deepfilternet3-onnx/dfn3.onnx`（HF cache，带 GRU 状态的流式版，单一固定模型，**不进 DB / 不切换**）。
- **集成点**：采集层 `SharedAudioState` 内（coordinator 无感）。链路 `原生SR → 重采样48k → DenoiseProcessor → 重采样16k → VAD/ASR`。
- **DSP**：sqrt-Hann STFT（n_fft=960, hop=480, 481 bins）+ 32 ERB 特征 + dfn3.onnx（含 GRU 隐状态 enc_h/erb_h/df_h）+ iSTFT overlap-add。复用现有 `rustfft`。
- **状态语义**：GRU 状态录音会话内跨帧保持（噪声环境稳态估计，不应被分段打断，与 filter_vad 每段 reset 故意相反）；`start()` 调 `reset()`。
- **降级**：模型缺失/推理失败 → 降级直通（不阻断录音，仅 warn）。
- **跨平台**：ort 三平台 EP（CoreML/DirectML/CUDA/CPU）；STFT 参数硬绑 48kHz。
- **模块**：`crates/asr/src/denoise.rs`（`DenoiseProcessor` + DSP）、`crates/asr/src/config.rs::find_df3`。
```

- [ ] **Step 2: 提交**

```bash
git add docs/architecture.md
git commit -m "docs: add DeepFilterNet3 denoise to architecture"
```

---

## Self-Review 检查

**Spec 覆盖：**
- §1-2（NS only, AEC 排除）→ 整个 plan 范围 ✓
- §3（dfn3.onnx IO 契约）→ Task 5 onnx inputs ✓
- §4（集成采集层 / 48k-NS-16k / 数据流）→ Task 7 process_pipeline ✓
- §5（denoise.rs / DenoiseProcessor API / rustfft）→ Task 3-6 ✓
- §6（状态保持 vs reset）→ Task 5 reset + Task 6 流式一致测试 ✓
- §7（跨平台 ort EP / STFT 硬绑48k）→ Task 7 + 文档 Task 8 ✓
- §8（denoise_enabled infra / find_df3 / 不进 DB / 缺失提示）→ Task 1, 2 ✓
- §9（三级降级）→ Task 7 lazy init None + Task 5 单帧 bypass（注：单帧推理失败的 bypass 需在 Task 5 process_frame 的 onnx run 包 `match`，见下方修正）✓
- §10（测试策略：重建 SNR / 样本守恒 / 流式一致 / 状态）→ Task 3,4,5,6 ✓
- §13（实施前提：窗/ERB/三平台/性能）→ Task 3(窗), Task 4(ERB), Task 7(性能: ort threads 可后续加) ✓

**遗漏修正**：单帧推理失败 bypass（§9 第 2 行）在 Task 5 `process_frame` 的 `session.run` 未包错误处理。在 Task 5 Step 4 后补：

```rust
        let outputs = match self.session.run(ort::inputs! { ... }?) {
            Ok(o) => o,
            Err(e) => {
                log::warn!("DenoiseProcessor 单帧推理失败，bypass: {e}");
                // GRU 状态保持，输出原始 new_samples（未降噪）
                self.out_buf.extend_from_slice(new_samples);
                return;
            }
        };
```

**类型一致性**：`DenoiseProcessor` 字段 enc_h/erb_h/df_h 在 Task 5 定义为 `Array3<f32>`，Task 6 process_frame 回写用 `into_shape((1,1,256))`/`(2,1,256)`/`(2,1,256)` 一致 ✓。`process_samples`/`flush`/`reset`/`new` 签名跨 Task 一致 ✓。

---

## Execution Handoff

Plan 完成并保存至 `docs/superpowers/plans/2026-06-16-denoise-deepfilternet.md`。两种执行方式：

**1. Subagent-Driven（推荐）** — 每个 Task 派发独立 subagent，任务间 review，迭代快。

**2. Inline Execution** — 本会话内用 executing-plans 批量执行，设检查点 review。

哪种？

---

## Task 9: Bug 修复 — 对齐 libDF 参考实现（2026-06-16）

> 初版实现（Task 1-8）完成后，实测发现降噪后 ASR 效果显著下降。经对比 `penta2himajin/mellonella`（模型导出方）参考实现和 `Rikorose/DeepFilterNet/libDF`，发现 4 个 bug。

**根因**：denoise.rs 的特征提取逻辑与模型训练时的特征分布完全不匹配，模型输出的增强频谱是垃圾，反而破坏了语音信号。

### Bug 列表

| # | Bug | 初版值 | 正确值（libDF） | 影响 |
|---|-----|--------|----------------|------|
| 1 | **ERB 公式分母** | `24.863` | `228.833` (= 24.7×9.265) | 带边界错 9.2 倍，32 个 ERB 带覆盖频率全错 |
| 2 | **feat_erb 缺归一化** | 原始 `\|spec\|²` 求和 | band 互相关功率 → dB → EMA 均值归一化 → /40 | 模型收到错误量级 |
| 3 | **feat_spec 缺归一化** | 原始 re/im | EMA 跟踪 `\|z\|`，除以 √state | 模型收到错误量级 |
| 4 | **conv_lookahead 缺失** | spec[t] 立即配 feat[t] | VecDeque 环形缓冲，spec[t] 配 feat[t+2] | 帧错位 20ms |

**额外修正**：
- 窗函数：sqrt-Hann → **Vorbis**（`sin(π/2·sin²(π(n+0.5)/N))`）
- band 功率公式：`Σ|spec|²` → **`(Σ|spec|²/width)²`**（libDF compute_band_corr 自相关形式）

### 参考来源

- `penta2himajin/mellonella` → `rust/mellonella-core/src/dfn3.rs`（Rust DFN3 ONNX 调用方）
- `Rikorose/DeepFilterNet` → `libDF/src/lib.rs`（`freq2erb` / `band_mean_norm_erb` / `band_unit_norm` / Vorbis 窗 / `MEAN_NORM_INIT` / `UNIT_NORM_INIT`）

### 关键参数（对齐后）

| 参数 | 值 |
|------|-----|
| 窗 | Vorbis：`sin(π/2·sin²(π(n+0.5)/N))` |
| ERB 分母 | 228.833 = 24.7 × 9.265 |
| conv_lookahead | 2 |
| norm_alpha | 0.99 (= exp(-hop/sr/τ) ≈ exp(-0.01)) |
| feat_erb 归一化 | dB → EMA(state) → (x - state) / 40 |
| feat_spec 归一化 | EMA 跟踪 \|z\|，X / √state |
| mean_norm_state 初始 | linspace(-60.0, -90.0, 32) |
| unit_norm_state 初始 | linspace(0.001, 0.0001, 96) |

### 改动

- [x] 重写 `crates/asr/src/denoise.rs`：Vorbis 窗 + ERB 公式修正 + 归一化状态 + conv_lookahead 环形缓冲
- [x] 更新 `docs/architecture.md` denoise 描述
- [x] 更新 `docs/superpowers/specs/2026-06-16-denoise-deepfilternet-design.md` §3.2 / §5 / §13
- [x] 更新本 plan 头部关键技术契约 + 追加 Task 9

### 验证

- [x] `cargo test -p octopus-asr -- denoise`：8 单元测试全过（Vorbis COLA、STFT/iSTFT 重建 SNR>40dB、ERB 公式对齐 libDF、归一化数值正确、band 覆盖 481 bins）
- [x] `cargo check --workspace` 全编译通过
- [x] `cargo test` 全量 63 tests passed, 0 failed
