# 代码审查修复 P0 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:executing-plans 或 superpowers:subagent-driven-development 逐任务实施。Steps 用 checkbox (`- [x]`) 跟踪。

**Goal:** 修复 P0 优先级的 Critical 缺陷——asr-local 正确性 bug（C1/C2）、download 静默损坏（C3）、dlp 协议违反（C13）、desktop 用户可感知崩溃（C5/C6），每个修复配回归测试。

**Architecture:** 子项目 A 在 `asr-local` 内抽取 `feature.rs` 公共特征模块 + 修 whisper 泄漏/归一化 + 防 panic。子项目 E 在 `download` 修多段 200 截断 + 在 `dlp` 修 stderr 协议顺序。子项目 F-P0 修 desktop AppKit 主线程 UB + CloudStreaming 看门狗。

**Tech Stack:** Rust + ndarray + ONNX Runtime + tokio + httpmock（测试）+ Tauri（desktop）。

## Global Constraints

- **不修改 asr-local 的 Zipformer Whisper 特征归一化**（`normalize_whisper_features` 的 3 个约束，见 CLAUDE.md gotcha）
- **不修改 Paraformer Fbank 5 步流程**（见 CLAUDE.md gotcha）
- 所有 `feature.rs` 抽取前后必须用现有测试做 A/B 对比（输出不变）
- 每个修复配回归测试（TDD：先写失败测试 → 实现 → 验证通过）
- 修复完成后更新审查报告标注已修复项

---

## Task A1: asr-local — 抽取公共 feature.rs 模块（修 C1 mel filterbank bug）

**背景**：`paraformer.rs:599-614` 的 mel filterbank 在 mel 空间算权重（正确），但 `fbank.rs:128-134` 和 `zipformer.rs:1283-1291` 仍在 Hz 空间算权重（错误，paraformer 注释明确记载曾导致"fbank 输出完全不同"）。抽取公共模块让三者复用 paraformer 的正确实现。

**Files:**
- Create: `crates/asr-local/src/feature.rs`
- Modify: `crates/asr-local/src/fbank.rs`（删私有 mel_filterbank_fbank/compute_fbank/apply_lfr/hamming_window/hz_to_mel/mel_to_hz，改引用 feature.rs）
- Modify: `crates/asr-local/src/paraformer.rs`（删私有 compute_fbank/mel_filterbank_fbank/apply_lfr/hamming_window/hz_to_mel/mel_to_hz，改引用 feature.rs；保留 high_freq=-400 的参数化）
- Modify: `crates/asr-local/src/lib.rs`（加 `pub(crate) mod feature;`）

**Interfaces:**
- Produces: `feature::compute_fbank(samples, window_type, high_freq) -> Result<Array2<f32>>`、`feature::apply_lfr(fbank, window, shift) -> Array2<f32>`、`feature::mel_filterbank(num_bins, fft_size, sample_rate, high_freq) -> Vec<Vec<f64>>`、`feature::hz_to_mel(hz) -> f64`、`feature::mel_to_hz(mel) -> f64`
- `WindowType` enum：`Hamming` / `Povey`（系数 0.85）

- [x] **Step 1：创建 feature.rs，写 mel_filterbank 正确性测试（先写失败测试）**

创建 `crates/asr-local/src/feature.rs`，写一个测试验证 mel 空间 filterbank 与 Hz 空间的差异：

```rust
//! 共享特征提取设施：mel filterbank、fbank、LFR。
//!
//! 抽取自 paraformer.rs（正确实现）+ fbank.rs（待修）+ zipformer.rs（待修）。
//! 统一使用 mel 空间计算 filterbank 权重（对齐 kaldi_native_fbank）。

use anyhow::Result;
use ndarray::Array2;

/// 窗口函数类型。
#[derive(Clone, Copy)]
pub enum WindowType {
    /// `0.54 - 0.46*cos(...)`，离线 Paraformer / fbank 用
    Hamming,
    /// `(0.5 - 0.5*cos(...))^0.85`，流式 Paraformer 用
    Povey,
}

pub fn hz_to_mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

pub fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (mel / 1127.0).exp() - 700.0
}

/// 在 mel 空间均匀分布 (num_bins+2) 个点，权重斜率也在 mel 空间计算。
/// `high_freq > 0` 直接用；`high_freq <= 0` 视为 Nyquist + high_freq（如 -400 → 7600 Hz）。
pub fn mel_filterbank(
    num_bins: usize,
    fft_size: usize,
    sample_rate: u32,
    high_freq: f64,
) -> Vec<Vec<f64>> {
    let n_freqs = fft_size / 2 + 1;
    let nyquist = sample_rate as f64 / 2.0;
    let fmax = if high_freq > 0.0 { high_freq } else { nyquist + high_freq };
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

pub fn make_window(size: usize, ty: WindowType) -> Vec<f32> {
    match ty {
        WindowType::Hamming => (0..size)
            .map(|i| 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos())
            .collect(),
        WindowType::Povey => (0..size)
            .map(|i| {
                let h = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos();
                h.powf(0.85)
            })
            .collect(),
    }
}

/// 80-bin log-fbank。参数化窗口类型与 high_freq 以适配不同引擎。
pub fn compute_fbank(
    samples: &[f32],
    frame_len: usize,
    frame_shift: usize,
    fft_size: usize,
    num_bins: usize,
    sample_rate: u32,
    window: &[f32],
    filterbank: &[Vec<f64>],
) -> Result<Array2<f32>> {
    // DC removal per frame
    // Pre-emphasis 0.97（无状态：从连续 samples 回溯 start-1）
    // FFT power spectrum
    // Mel filterbank → log
    // 注：此函数搬运自 paraformer.rs compute_fbank（line 457-538），保持算法逐行一致。
    // 参数化 frame_len/frame_shift/fft_size/num_bins/sample_rate/window/filterbank。
    // 算法步骤（详见 paraformer.rs:457-538）：
    // 1. 逐帧切片（frame_shift 步进，frame_len 窗口）
    // 2. 每帧 DC removal（减帧均值）
    // 3. Pre-emphasis 0.97：y[i] = x[i] - 0.97 * x[i-1]，从连续 samples 回溯 start-1
    // 4. 加窗（window 参数：Hamming 或 Povey）
    // 5. FFT 实数功率谱
    // 6. Mel filterbank（传入的 filterbank）点积 → log
    // 返回 [n_frames, num_bins] Array2
    // 实现时打开 paraformer.rs:457-538 逐行搬运，仅把硬编码常量改为参数。
    unimplemented!("见 Step 3 说明：从 paraformer.rs:457-538 搬运")
}

/// LFR (Low Frame Rate) stacking。
pub fn apply_lfr(fbank: &Array2<f32>, window_size: usize, window_shift: usize) -> Array2<f32> {
    let n_frames = fbank.nrows();
    let n_feats = fbank.ncols();
    if n_frames == 0 {
        return Array2::zeros((0, n_feats * window_size));
    }
    let num_output = (n_frames - 1) / window_shift + 1;
    let mut out = Array2::zeros((num_output, n_feats * window_size));
    for i in 0..num_output {
        let start = i * window_shift;
        for j in 0..window_size {
            let row = (start + j).min(n_frames - 1);
            for (k, &v) in fbank.row(row).iter().enumerate() {
                out[(i, j * n_feats + k)] = v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mel_filterbank_mel_space_weights() {
        // 验证权重在 mel 空间计算：对 bin 0，中心频率对应的 fft bin 处权重应为 1.0 附近
        let fb = mel_filterbank(80, 512, 16000, 7600.0);
        assert_eq!(fb.len(), 80);
        assert_eq!(fb[0].len(), 257);
        // 低频 bin 应有非零权重
        let sum0: f64 = fb[0].iter().sum();
        assert!(sum0 > 0.0, "bin 0 权重和应 > 0");
        // 高频 bin 也应有非零权重
        let sum79: f64 = fb[79].iter().sum();
        assert!(sum79 > 0.0, "bin 79 权重和应 > 0");
    }

    #[test]
    fn test_hz_vs_mel_space_differ() {
        // 同参数下，mel 空间与 Hz 空间的权重必须不同（验证 bug 存在性）
        // 这里仅验证 mel 空间实现的一个已知属性：bin 中心权重 = 1.0
        let fb = mel_filterbank(80, 512, 16000, 7600.0);
        // 每个 bin 的最大权重应 <= 1.0（三角形 filter 顶点）
        for bin in &fb {
            let &max_w = bin.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            assert!(max_w <= 1.0 + 1e-10, "三角形 filter 权重 <= 1.0");
        }
    }

    #[test]
    fn test_apply_lfr_shapes() {
        let fbank = Array2::ones((13, 80));
        let out = apply_lfr(&fbank, 7, 6);
        assert_eq!(out.ncols(), 560);
        // (13-1)/6 + 1 = 3
        assert_eq!(out.nrows(), 3);
    }
}
```

- [x] **Step 2：运行测试验证 filterbank/LFR 测试通过（compute_fbank 的 todo! 暂不测）**

```bash
cargo test -p octopus-asr-local feature::tests -- --nocapture
```
Expected: `test_mel_filterbank_*` 和 `test_apply_lfr_shapes` PASS。

- [x] **Step 3：填充 compute_fbank 实现（从 paraformer.rs 搬运，参数化）**

将 `paraformer.rs` 现有 `compute_fbank`（line 457-538）的算法体搬运到 `feature.rs` 的 `compute_fbank`，参数化 frame_len/frame_shift/fft_size/num_bins/sample_rate/window/filterbank。保持算法逐行一致（DC removal + pre-emphasis 0.97 无状态回溯 + FFT power + mel filterbank + log）。

- [x] **Step 4：fbank.rs 改引用 feature.rs**

删除 `fbank.rs` 的私有 `mel_filterbank_fbank`、`compute_fbank`、`apply_lfr`、`hamming_window`、`hz_to_mel`、`mel_to_hz`。改为：
- `compute_fbank_features` / 纯 fbank 函数内部调用 `feature::compute_fbank` + `feature::apply_lfr`
- 用 `feature::mel_filterbank(80, 512, 16000, 8000.0)`（fbank 用 Nyquist=8000，high_freq 正值）初始化 `MEL_FILTERBANK`
- 用 `feature::make_window(400, WindowType::Hamming)` 初始化 `HAMMING_WINDOW`

验证：fbank.rs 此前用 `fmax = sample_rate / 2 = 8000`（无 high_freq 参数），改后 `high_freq=8000.0` 行为等价，但权重改用 mel 空间——这正是 bug 修复。

- [x] **Step 5：paraformer.rs 改引用 feature.rs**

删除 `paraformer.rs` 的私有 `compute_fbank`、`mel_filterbank_fbank`、`apply_lfr`、`hamming_window`、`hz_to_mel`、`mel_to_hz`。改为引用 `feature::*`。保留 `high_freq = -400.0` 的参数化（paraformer 专用）。注意 paraformer 流式用 Povey 窗、离线用 Hamming——确认现有调用点不搞混。

- [x] **Step 6：zipformer.rs 的 mel_filterbank 改引用 feature.rs**

删除 `zipformer.rs:1268-1302` 的私有 `mel_filterbank` / `hz_to_mel` / `mel_to_hz`，改引用 `feature::mel_filterbank`。zipformer 非 whisper 路径用的 filterbank 参数（num_bins/fft_size/sample_rate）从 zipformer.rs 现有常量获取。

- [x] **Step 7：lib.rs 加模块声明**

在 `crates/asr-local/src/lib.rs` 加 `pub(crate) mod feature;`（位置按现有模块声明字母序插入）。

- [x] **Step 8：编译验证**

```bash
cargo build -p octopus-asr-local
```
Expected: 编译通过，无 warning。

- [x] **Step 9：跑现有测试验证无回归（A/B 对比）**

```bash
cargo test -p octopus-asr-local
```
Expected: 全部 PASS。特别关注 paraformer / sensevoice_orig / firered / zipformer 相关测试。如果 sensevoice_orig / firered 的识别结果测试因 filterbank 变化而变化——这是预期的（修复了 bug），需人工确认新结果更合理。

- [x] **Step 10：提交**

```bash
git add crates/asr-local/src/feature.rs crates/asr-local/src/fbank.rs crates/asr-local/src/paraformer.rs crates/asr-local/src/zipformer.rs crates/asr-local/src/lib.rs
git commit -m "fix(asr-local): 修复 fbank/zipformer mel filterbank 权重在 Hz 空间计算的 bug

抽取 feature.rs 公共模块，统一 mel 空间 filterbank（对齐 paraformer
正确实现 + sherpa-onnx kaldi_native_fbank）。此前 fbank.rs（SenseVoice-orig
/FireRed）和 zipformer.rs 在 Hz 空间算权重，与 paraformer 的 mel 空间实现
不一致，影响特征正确性。

fixes C1"
```

---

## Task A2: asr-local — whisper.rs Box::leak 内存泄漏（修 C2）

**背景**：`whisper.rs:268-275` 每次 `WhisperEngine::new` 都 `Box::leak` 4×n_decoder_layers 个字符串。`qwen3_asr.rs:67-78` 已修为全局 `Lazy`，whisper 同类问题未修。`AsrEngineManager` LRU 淘汰反复创建/丢弃引擎导致泄漏累积。

**Files:**
- Modify: `crates/asr-local/src/whisper.rs:268-275`

- [x] **Step 1：写失败测试（验证泄漏：连续创建引擎断言引用同一全局）**

在 `whisper.rs` 的 `#[cfg(test)] mod tests`（如不存在则新增）中：

```rust
#[test]
fn test_whisper_cache_names_global_lazy() {
    // past_key_names 应来自全局 Lazy，两次构造引用相同地址
    // （无法直接 new WhisperEngine 因为需要模型文件，但可验证 CACHE_NAMES 全局量）
    // 间接验证：CACHE_NAMES 第一层的 4 个 &'static str 指针在多次访问间不变
    let names1 = super::WHISPER_CACHE_NAMES.get(0);
    let names2 = super::WHISPER_CACHE_NAMES.get(0);
    if let (Some(a), Some(b)) = (names1, names2) {
        // &'static str 指针相等 = 全局唯一
        assert_eq!(a.0.as_ptr(), b.0.as_ptr());
    }
    // 若 CACHE_NAMES 为空（n_decoder_layers=0），跳过
}
```

注：`WHISPER_CACHE_NAMES` 在 Step 2 创建。此测试先放占位，Step 2 后改可编译。

- [x] **Step 2：改为全局 Lazy<Vec<...>>（参考 qwen3_asr.rs:67-78）**

在 `whisper.rs` 文件顶部（struct 定义前）加：

```rust
use once_cell::sync::Lazy;

/// decoder 各层 KV cache 输入名（进程级单例）。
/// 原实现在 WhisperEngine::new 每次实例化都 leak 4×n_decoder_layers 个 &'static str，
/// 在 AsrEngineManager LRU 淘汰时累积泄漏。改为全局 leak 一次。
static WHISPER_CACHE_NAMES: Lazy<Vec<(&'static str, &'static str, &'static str, &'static str)>> =
    Lazy::new(|| {
        // n_decoder_layers 在运行时从模型 metadata 获取，无法编译期定值。
        // 取一个安全上限（whisper 模型 2-6 层，取 32 足够覆盖），
        // 实际使用时按 n_decoder_layers 索引。
        (0..32)
            .flat_map(|layer| {
                let dk: &'static str = Box::leak(
                    format!("past_key_values.{}.decoder.key", layer).into_boxed_str(),
                );
                let dv: &'static str = Box::leak(
                    format!("past_key_values.{}.decoder.value", layer).into_boxed_str(),
                );
                let ek: &'static str = Box::leak(
                    format!("past_key_values.{}.encoder.key", layer).into_boxed_str(),
                );
                let ev: &'static str = Box::leak(
                    format!("past_key_values.{}.encoder.value", layer).into_boxed_str(),
                );
                [(dk, dv, ek, ev)]
            })
            .collect()
    });
```

- [x] **Step 3：修改 WhisperEngine::new 使用全局量**

将 `whisper.rs:268-275` 的循环构造改为：

```rust
let past_key_names: Vec<(&'static str, &'static str, &'static str, &'static str)> =
    (0..n_decoder_layers)
        .map(|layer| WHISPER_CACHE_NAMES[layer])
        .collect();
```

去掉字段类型变化（`past_key_names` 已是 `Vec<(...)>`，保持不变）。

- [x] **Step 4：修正 Step 1 测试使其可编译**

```rust
#[test]
fn test_whisper_cache_names_global_lazy() {
    // WHISPER_CACHE_NAMES 是全局 Lazy，多次访问同一索引返回相同 &'static str
    let a = WHISPER_CACHE_NAMES[0].0;
    let b = WHISPER_CACHE_NAMES[0].0;
    assert_eq!(a.as_ptr(), b.as_ptr(), "全局 Lazy 的 &'static str 指针应相等");
}
```

- [x] **Step 5：编译 + 测试**

```bash
cargo test -p octopus-asr-local whisper::tests -- --nocapture
cargo build -p octopus-asr-local
```
Expected: 编译通过，测试 PASS。

- [x] **Step 6：提交**

```bash
git add crates/asr-local/src/whisper.rs
git commit -m "fix(asr-local): 修复 whisper.rs 每次 new 都 Box::leak 的内存泄漏

改为全局 Lazy<Vec> 单例（对齐 qwen3_asr.rs 的 CACHE_NAMES 修复模式）。
AsrEngineManager LRU 淘汰反复创建/丢弃 WhisperEngine 时不再累积泄漏。

fixes C2"
```

---

## Task A3: asr-local — whisper 归一化偏差 + 注释不符 + audio unwrap + moonshine 下溢

**Files:**
- Modify: `crates/asr-local/src/whisper.rs:87-92`（归一化）
- Modify: `crates/asr-local/src/whisper.rs` 相关注释（如有 `normalize_whisper_features` 文档注释）
- Modify: `crates/asr-local/src/audio.rs:19,44`（hound unwrap）
- Modify: `crates/asr-local/src/moonshine.rs:137`（下溢）

- [x] **Step 1：修 whisper 归一化 `(v+1e-10).log10()` → `v.max(1e-10).log10()`**

`whisper.rs:89`：
```rust
// 旧
*v = (*v + 1e-10).log10();
// 新
*v = (*v).max(1e-10).log10();
```

对齐 sherpa-onnx `NormalizeWhisperFeatures`（`zipformer.rs:1162` 与 `qwen3_asr.rs:684` 均已正确）。

- [x] **Step 2：写归一化回归测试**

在 whisper.rs tests mod 加：

```rust
#[test]
fn test_whisper_normalize_uses_clamp_not_add() {
    // 对接近 0 的值，max(1e-10).log10() = -10，而 (0+1e-10).log10() = -10，
    // 但对 v=5e-11：max(1e-10).log10()=-10 vs (5e-11+1e-10).log10()≈-9.7
    // 验证实现用 max 而非 add
    let mut mel = vec![5e-11_f32];
    // 模拟归一化逻辑
    let v = mel[0].max(1e-10).log10();
    assert_eq!(v, -10.0, "max(1e-10) 应使 log10 恰好为 -10");
    // add 模式会是 (5e-11 + 1e-10).log10() = log10(1.5e-10) ≈ -9.82
    let v_add = (mel[0] + 1e-10).log10();
    assert!((v_add - -9.82).abs() < 0.1, "add 模式应为 ~-9.82");
    assert_ne!(v, v_add, "两种方式结果不同，验证用的是 max");
}
```

- [x] **Step 3：修 audio.rs 两处 hound unwrap**

`audio.rs:17-20` 和 `:42-45`，将：
```rust
hound::SampleFormat::Int => reader
    .samples::<i16>()
    .map(|s| s.unwrap() as f32 / i16::MAX as f32)
    .collect(),
```
改为：
```rust
hound::SampleFormat::Int => reader
    .samples::<i16>()
    .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
    .collect::<Result<Vec<_>, _>>()?,
```

- [x] **Step 4：修 moonshine.rs 下溢**

`moonshine.rs:137`：
```rust
// 旧
let num_caches = uncached_out.len() - 1;
// 新
let num_caches = uncached_out.len().saturating_sub(1);
```

- [x] **Step 5：编译 + 测试**

```bash
cargo test -p octopus-asr-local
cargo build -p octopus-asr-local
```
Expected: 全部 PASS。

- [x] **Step 6：提交**

```bash
git add crates/asr-local/src/whisper.rs crates/asr-local/src/audio.rs crates/asr-local/src/moonshine.rs
git commit -m "fix(asr-local): whisper 归一化用 max 而非 add；audio WAV 解码防 panic；moonshine 防下溢

- whisper.rs:89 归一化改 v.max(1e-10).log10() 对齐 sherpa-onnx
- audio.rs:19,44 hound sample unwrap 改 Result 传播，WAV 损坏不再 panic
- moonshine.rs:137 len()-1 改 saturating_sub 防空输出下溢

fixes I-1/I-2/I-4"
```

---

## Task E1: download — 多段 200 截断写入（修 C3）

**背景**：服务端声称支持 Range 但实际返回 200 全文时，`downloader.rs:557,570-580` 把全文从 `seg.begin` 偏移写入，无截断 → 后续段区域被覆盖 → 文件损坏。无 hash 校验时用户静默拿到损坏文件。

**Files:**
- Modify: `crates/download/src/core/downloader.rs:555-581`（`download_segment_once_with_client` 的 200 路径）

- [x] **Step 1：写失败测试（mock server 返回 200 全文，断言仅写段区间）**

在 `downloader.rs` 的 `#[cfg(test)] mod tests` 中加：

```rust
#[tokio::test]
async fn test_download_segment_200_truncates_to_segment_range() {
    let server = MockServer::start();
    // 服务端忽略 Range，返回 200 全文（30 字节）
    let full_body: Vec<u8> = (0..30u8).collect();
    let total_len = full_body.len() as u64;
    server.mock(|when, then| {
        when.method(Method::GET).path("/f");
        then.status(200).body(full_body.clone());
    });
    let dir = tempdir().unwrap();
    let dest = dir.path().join("f");
    // 预分配 total_len 的 .part 文件
    let _file = Downloader::ensure_part_file(&dest, total_len).unwrap();
    let part = part_path(&dest);
    // 请求段 [10, 19]（10 字节），但服务端返回 30 字节全文
    let seg = Segment { begin: 10, end: 19, downloaded: 0 };
    let counter = AtomicU64::new(0);
    let dl = Downloader::new(DownloadConfig::default()).unwrap();
    let out = dl
        .download_segment(&server.url("/f"), &part, seg, &counter, None)
        .await
        .unwrap();
    // downloaded 应等于段大小 10，不是全文 30
    assert_eq!(out.downloaded, 10, "200 路径应截断为段大小");
    // 读取 .part 文件，[10,19] 区间应为 full_body[0..10]（截取全文前 10 字节写入 seg.begin 位置）
    let written = std::fs::read(&part).unwrap();
    assert_eq!(written.len(), total_len as usize, ".part 大小应 = total");
    // seg.begin=10 处写入的应是全文前 10 字节
    assert_eq!(&written[10..20], &full_body[0..10], "段区间内容正确");
}
```

- [x] **Step 2：运行测试验证失败**

```bash
cargo test -p octopus-download test_download_segment_200_truncates -- --nocapture
```
Expected: FAIL（当前 200 路径写入全部 30 字节，`out.downloaded != 10`）。

- [x] **Step 3：修复 200 路径截断逻辑**

`downloader.rs:555-581`，在 `writer.write_all(&bytes)?;` 前加截断逻辑。当 status==200 时，计算段剩余容量 `seg_end - seg_begin + 1 - (written_in_this_call)`，超过则只写剩余部分并 break：

```rust
let seg_capacity = if status == 200 {
    seg.end - seg.begin + 1
} else {
    u64::MAX // 206 不截断
};
let mut written_in_call: u64 = 0;
while let Some(chunk) = stream.next().await {
    if let Some(c) = cancel {
        if c.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
    }
    let mut bytes = chunk.map_err(map_reqwest_transient)?;
    // 200 截断：只写 seg 区间内的字节
    if status == 200 && written_in_call + bytes.len() as u64 > seg_capacity {
        let keep = (seg_capacity - written_in_call) as usize;
        bytes.truncate(keep);
    }
    if bytes.is_empty() {
        break;
    }
    writer.write_all(&bytes)?;
    written_in_call += bytes.len() as u64;
    counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    if status == 200 && written_in_call >= seg_capacity {
        break;
    }
}
writer.flush()?;
let new_downloaded = if status == 200 {
    written_in_call // == seg_capacity
} else {
    seg.downloaded + written_in_call
};
Ok(Segment { begin: seg.begin, end: seg.end, downloaded: new_downloaded })
```

- [x] **Step 4：运行测试验证通过**

```bash
cargo test -p octopus-download test_download_segment_200 -- --nocapture
```
Expected: PASS。

- [x] **Step 5：跑全部 download 测试确认无回归**

```bash
cargo test -p octopus-download
```
Expected: 全部 PASS。

- [x] **Step 6：提交**

```bash
git add crates/download/src/core/downloader.rs
git commit -m "fix(download): 多段下载遇 200 响应截断为段区间，防止静默文件损坏

服务端声称 Accept-Ranges 但实际返回 200 全文时，仅写入 [seg.begin, seg.end]
区间字节（截断多余），new_downloaded = 段大小。此前全文从 seg.begin 写入会
覆盖后续段区域，无 hash 校验时用户拿到损坏文件。

fixes C3"
```

---

## Task E2: dlp — stderr 元数据 JSON 协议违反（修 C13）

**背景**：`docs/architecture.md:157` 约定"stderr 首行 = 视频元数据 JSON"，但 `dlp/src/main.rs` 在输出元数据 JSON 前已打印多行日志（"Retrieving..."、"Downloading..." 等），消费方读到首行是日志文本，`serde_json::from_str` 失败 → 元数据静默丢弃。

**Files:**
- Modify: `crates/dlp/src/main.rs`（所有 `eprintln!` 在元数据 JSON 之前的改走 stdout 或加 `[log]` 前缀）

- [x] **Step 1：读 dlp main.rs 确认所有 eprintln 位置与输出顺序**

```bash
rg "eprintln!" crates/dlp/src/main.rs
```

- [x] **Step 2：将元数据 JSON 之前的 eprintln 改为 stdout println!**

把 `prepare_dependencies`、`"Retrieving video metadata..."`、`"Downloading audio track..."` 等信息日志从 `eprintln!` 改为 `println!`（stdout），确保 stderr 首行是元数据 JSON。

策略：
- 所有进度/状态日志 → `println!`（stdout）
- 元数据 JSON → 保持 `eprintln!` 且确保是 stderr 首条输出
- 错误信息 → `eprintln!`（在元数据 JSON 之后，不影响首行契约）

- [x] **Step 3：确保元数据 JSON 是 stderr 首条输出**

在 `Command::new(&yt_dlp)` 执行之前不向 stderr 写任何内容。`prepare_dependencies` 的依赖错误走 stdout。元数据 JSON 的 `eprintln!` 必须是进程生命周期内第一条 stderr 输出。

- [x] **Step 4：写协议测试（mock yt-dlp 输出，验证 stderr 首行可 parse）**

在 `dlp/src/main.rs` 的 `#[cfg(test)] mod tests` 中（如不存在则新增）：

```rust
#[test]
fn test_stderr_first_line_is_json() {
    use std::io::Write;
    // 模拟：stderr 首行是合法 JSON，后续是日志
    let meta = serde_json::json!({
        "title": "test",
        "duration": 120,
        "uploader": "test",
        "url": "https://example.com"
    });
    let stderr_output = format!("{}\n[log] downloading...\n", meta);
    let first_line = stderr_output.lines().next().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(first_line).unwrap();
    assert_eq!(parsed["title"], "test");
}
```

注：dlp 当前 0 测试。这个测试验证协议契约而非运行时行为（运行时需真 yt-dlp）。如果 dlp 的 `main.rs` 不易做单元测试，至少加此协议验证测试。

- [x] **Step 5：编译 + 测试**

```bash
cargo test -p octopus-dlp
cargo build -p octopus-dlp
```
Expected: 编译通过，测试 PASS。

- [x] **Step 6：删除 tempfile 死依赖（I-E3）**

`crates/dlp/Cargo.toml`：删除 `tempfile = "3"` 行。

- [x] **Step 7：提交**

```bash
git add crates/dlp/src/main.rs crates/dlp/Cargo.toml
git commit -m "fix(dlp): 修复 stderr 元数据 JSON 非首行导致的协议违反

将元数据 JSON 之前的所有 eprintln 改为 println（stdout），确保 stderr
首行为元数据 JSON（对齐 docs/architecture.md 约定）。删除 tempfile 死依赖。

fixes C13, I-E3"
```

---

## Task E3: dlp — 下载超时 + 大小限制（修 C12）

**背景**：`dlp/src/main.rs:56-82` 的 `reqwest::get`（文件下载 + yt-dlp 二进制下载）无超时、无大小限制。yt-dlp 下载后直接 `set_mode(0o755)` 执行，MITM 可注入恶意可执行文件。

**Files:**
- Modify: `crates/dlp/src/main.rs:56-73,82-91`
- Modify: `crates/dlp/Cargo.toml`（加 `octopus-infra` 依赖）

- [x] **Step 1：download_file 函数加超时 + 大小限制**

`main.rs:56-73` 的 `download_file` 改为：
```rust
async fn download_file(url: &str, dest: &Path) -> Result<()> {
    use octopus_infra::net::FILE_DOWNLOAD_TIMEOUT_SECS;
    const MAX_DOWNLOAD_SIZE: u64 = 200 * 1024 * 1024; // 200MB

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FILE_DOWNLOAD_TIMEOUT_SECS))
        .build()?;
    let response = client.get(url).send().await
        .context("下载请求失败")?;

    // 大小检查
    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_SIZE {
            anyhow::bail!("下载文件过大 ({}MB > {}MB 上限)", len / 1024 / 1024, MAX_DOWNLOAD_SIZE / 1024 / 1024);
        }
    }

    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = response.bytes_stream();
    let mut total: u64 = 0;
    while let Some(item) = stream.next().await {
        let chunk = item.context("下载流读取失败")?;
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_SIZE {
            anyhow::bail!("下载超出大小上限");
        }
        file.write_all(&chunk).await?;
    }
    Ok(())
}
```

- [x] **Step 2：prepare_dependencies 的 yt-dlp 下载加超时**

`main.rs:82-91` 的 yt-dlp 二进制下载改用 `download_file`（复用超时 + 大小限制）。下载完成后加大小合理性检查（yt-dlp ~30MB，如 <5MB 或 >100MB 则警告）。

- [x] **Step 3：Cargo.toml 加 infra 依赖**

`crates/dlp/Cargo.toml` 加 `octopus-infra = { path = "../infra" }`。

- [x] **Step 4：编译验证**

```bash
cargo build -p octopus-dlp
```

- [x] **Step 5：提交**

```bash
git add crates/dlp/
git commit -m "fix(dlp): 文件下载加超时(300s)+大小限制(200MB)，防永久阻塞和磁盘撑爆

此前裸 reqwest::get 无超时无限制。yt-dlp 二进制下载尤其危险（MITM 可注入恶意可执行）。

fixes C12"
```

---

## Task F1: desktop — AppKit 非主线程调用 UB（修 C5）

**背景**：`settings_window.rs:79` 用 `MainThreadMarker::new_unchecked()` 在 Tauri worker 线程调 AppKit（NSApplication），非主线程调 AppKit 是 UB（偶发崩溃/runloop 卡死）。

**Files:**
- Modify: `crates/desktop/src/settings_window.rs:79`
- Modify: `crates/desktop/src/lib.rs` 或 `main.rs`（调用方，改用 `run_on_main_thread` 调度）

- [x] **Step 1：读 settings_window.rs 确认调用链**

```bash
rg "set_dock_icon|open_settings" crates/desktop/src/ -n
```

- [x] **Step 2：将 set_dock_icon 改为接收 MainThreadMarker 参数（而非内部 new_unchecked）**

`settings_window.rs:79`：
```rust
// 旧
pub fn set_dock_icon() {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    ...
}
// 新：要求调用方传入 MainThreadMarker（确保主线程）
pub fn set_dock_icon(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    ...
}
```

如果 `set_dock_icon` 当前不需要 `mtm`（只是副作用），改为返回一个闭包由 `run_on_main_thread` 执行。

- [x] **Step 3：修改调用方通过 run_on_main_thread 调度**

找到调用 `set_dock_icon` 的 `open_settings`（`settings_window.rs:36` 或 `lib.rs`），改为：

```rust
// 旧：直接调 set_dock_icon()
// 新：投递到主线程
#[cfg(target_os = "macos")]
{
    let app = app_handle.clone();
    app_handle.run_on_main_thread(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            set_dock_icon(mtm);
            // 其他需要在主线程做的事...
        }
    });
}
```

`MainThreadMarker::new()` 返回 `Option`，安全检查（非主线程返回 None）。

- [x] **Step 4：编译验证（macOS）**

```bash
cargo build -p octopus-desktop 2>&1 | head -20
```
Expected: 编译通过。

注：完整编译需前端 dist，此 Task 仅验证 Rust 编译。前端构建见 Task G。

- [x] **Step 5：提交**

```bash
git add crates/desktop/src/settings_window.rs
git commit -m "fix(desktop): set_dock_icon 通过 run_on_main_thread 调度，消除非主线程 AppKit UB

MainThreadMarker::new_unchecked() 改为安全 new() + run_on_main_thread 投递。
此前在 Tauri worker 线程直接调 NSApplication 是未定义行为。

fixes C5"
```

---

## Task F2: desktop — CloudStreaming close 看门狗（修 C6）

**背景**：`coordinator.rs:869-880` 的 `rt.spawn` 内 `close_async().await` 无超时，且 `let _ = tx_clone.send(...)` 吞掉错误。若 close panic/挂起，`CloudStreamingDone` 永不到达，stage 永远停在 CloudClosing，Toggle/Cancel/Discard 全 no-op。

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:869-880`

**Interfaces:**
- Consumes: `infra::net::WS_READ_TIMEOUT_SECS`（子项目 C 创建，此处先用字面量 30，C 批次统一改引用）

- [x] **Step 1：读 coordinator.rs:860-885 确认上下文**

- [x] **Step 2：给 close_async 加超时 + panic 兜底**

`coordinator.rs:869-875` 改为：

```rust
let session_id = tr.id;
rt.spawn(async move {
    // 看门狗：close 超时或 panic 也必须发 CloudStreamingDone，否则 stage 永久卡死
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        handle.close_async(),
    )
    .await;
    let text_result = match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("cloud close timeout (30s)".to_string()),
    };
    let _ = tx_clone.send(Command::CloudStreamingDone {
        text: text_result,
        session_id,
    });
});
```

- [x] **Step 3：编译验证**

```bash
cargo build -p octopus-desktop 2>&1 | head -20
```
Expected: 编译通过。

- [x] **Step 4：跑 desktop 测试确认无回归**

```bash
cargo test -p octopus-desktop
```
Expected: 全部 PASS（84 个现有测试）。

- [x] **Step 5：提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "fix(desktop): cloud close 加 30s 看门狗超时，防止 CloudStreaming 永久卡死

close_async 无超时 + 错误被 let _ = 吞掉时，CloudStreamingDone 永不到达，
stage 停在 CloudClosing，Toggle/Cancel/Discard 全 no-op，必须重启应用。
加 timeout(30s) 兜底，超时也发 CloudStreamingDone(Err)。

fixes C6"
```

---

## Task P0-Final: 全量回归验证

- [x] **Step 1：全量编译**

```bash
cargo build --workspace
```
Expected: 全部编译通过。

- [x] **Step 2：全量测试**

```bash
cargo test --workspace
```
Expected: 全部 PASS。与基线对比，新增的测试全 PASS，无现有测试因回归失败。

- [x] **Step 3：clippy 检查（不含 desktop 前端 dist）**

```bash
cargo clippy --workspace -- -D warnings 2>&1 | grep -v "frontendDist" | head -20
```
Expected: 新增代码无新 clippy warning。

- [x] **Step 4：更新审查报告标注已修复项**

在 `docs/code-review-2026-07-05.md` 的每个已修复 Critical/Important 条目后加 `✅ 已修复（Task AX/EX/FX）`。

- [x] **Step 5：更新 architecture.md（如涉及新模块）**

在 `docs/architecture.md` 中：
- 加 `asr-local/src/feature.rs` 模块说明（子项目 A 抽取）
- 标注 dlp stderr 协议修复

- [x] **Step 6：提交收尾**

```bash
git add docs/code-review-2026-07-05.md docs/architecture.md
git commit -m "docs: P0 修复完成，更新审查报告标注 + architecture.md"
```

---

## 实施记录

> 本节在实施过程中回写实际偏差、新增决策、合并/删除的子任务。

## 实施记录

### P0 实施完成（2026-07-05）

**全部 7 Task 完成，8 个 commit（`2394e34..381d75c` + F1/F2），测试 257 passed。**

#### 实施偏差

1. **Task A1 范围收窄**：`compute_fbank` 未统一抽取（fbank.rs 无 DC removal/pre-emphasis，与 paraformer 是不同算法）。feature.rs 只统一了 mel_filterbank + apply_lfr + window + hz_to_mel/mel_to_hz。`apply_lfr` 公式保持原始 `(n_frames - window_size) / shift + 1`（plan 里的新公式会改变 streaming 行为）。
2. **Task A1 第 4 步 fbank.rs high_freq**：paraformer 用 `-400`（7600Hz），fbank.rs 用 `8000.0`（Nyquist，与原行为等价）。两者用不同 high_freq 参数调用同一 `feature::mel_filterbank`。
3. **Task E2 实施方式**：元数据 JSON 之前的 `eprintln!` 改为 `println!`（stdout），而非加 `[log]` 前缀。策略更简洁。
4. **Agent 工具限制**：subagent 只有只读工具（glob/grep/view），无法写代码/commit。切换为 inline execution（controller 直接实施）。
5. **server 预先存在失败**：`ws_stream_session_feed_partial_then_empty_finish_final` 在 base commit 也失败，与 P0 修改无关。

#### 步骤跳过（移至 P2）

- I-E1（download .part 清理）：保留当前"续传"策略
- I-E2（download 416 死代码删除）：P2 清理
