//! fbank 性能基准（z_perf Step 0a，2026-07-17）。
//!
//! 目标：为 LTO/codegen-units 等 release profile 优化提供 before/after 量化数据。
//! 热函数：compute_fbank（80-bin log-fbank FFT + DC removal + pre-emphasis + mel filterbank）
//! 这是 streaming ASR 每 chunk 都跑的高频 CPU 热点（crates/asr-local/src/fbank.rs）。
//!
//! 跑法：
//!   cargo criterion -p octopus-asr-local --bench fbank --message-format json > /tmp/z-perf/bench.json
//! 对照（启用 LTO 前后）：
//!   cargo criterion -p octopus-asr-local --bench fbank -- --save-baseline nolto
//!   # 改 Cargo.toml 启用 lto=fat 后
//!   cargo criterion -p octopus-asr-local --bench fbench -- --baseline nolto

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use octopus_asr_local::fbank; // 内部 re-export 了 feature::{apply_lfr, hamming_window, povey_window}

/// 生成 16kHz 正弦波单声道样本（模拟语音采样，不含噪声——测纯计算开销）。
fn synth_samples(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / 16000.0;
            // 多频混合，模拟语音频谱分布
            (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.3
                + (2.0 * std::f32::consts::PI * 880.0 * t).sin() * 0.2
                + (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5
        })
        .collect()
}

/// 基准 1：纯 compute_fbank（FFT 热循环），不同输入长度。
/// 1600 / 16000 / 48000 samples ≈ 100ms / 1s / 3s 音频 @ 16kHz。
fn bench_compute_fbank(c: &mut Criterion) {
    let window = fbank::hamming_window(400);
    let preemph = 0.97;

    let mut group = c.benchmark_group("compute_fbank");
    for &n_samples in &[1600usize, 16000, 48000] {
        let samples = synth_samples(n_samples);
        // frame_shift=160，帧数 ≈ n_samples/160，标 throughput 为帧数
        let n_frames = (n_samples.saturating_sub(400)) / 160 + 1;
        group.throughput(Throughput::Elements(n_frames as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(n_samples),
            &samples,
            |b, samples| {
                b.iter(|| {
                    let out = fbank::compute_fbank(
                        black_box(samples),
                        black_box(&window),
                        black_box(preemph),
                    )
                    .expect("fbank");
                    black_box(out);
                });
            },
        );
    }
    group.finish();
}

/// 基准 2：compute_fbank_features（含 ×32768 缩放 + LFR 堆叠），完整 SenseVoice 路径。
fn bench_compute_fbank_features(c: &mut Criterion) {
    let mut group = c.benchmark_group("compute_fbank_features");
    for &n_samples in &[1600usize, 16000] {
        let samples = synth_samples(n_samples);
        group.bench_with_input(
            BenchmarkId::from_parameter(n_samples),
            &samples,
            |b, samples| {
                b.iter(|| {
                    let out = fbank::compute_fbank_features(black_box(samples)).expect("fbank");
                    black_box(out);
                });
            },
        );
    }
    group.finish();
}

/// 基准 3：apply_lfr（纯堆叠，无 FFT），隔离 LFR 开销。
fn bench_apply_lfr(c: &mut Criterion) {
    // 先算一份 fbank 输出作为 LFR 输入
    let samples = synth_samples(16000);
    let window = fbank::hamming_window(400);
    let fbank_out = fbank::compute_fbank(&samples, &window, 0.97).expect("fbank");

    c.bench_function("apply_lfr_1s", |b| {
        b.iter(|| {
            let out = fbank::apply_lfr(black_box(&fbank_out), 7, 6);
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_compute_fbank, bench_compute_fbank_features, bench_apply_lfr);
criterion_main!(benches);
