//! Streaming Paraformer 推理性能基准（z_perf Step 1，2026-07-17）。
//!
//! 目标：量化 streaming ASR 的 ort 推理热路径（accept_samples per-chunk）。
//! 这是 streaming 真正的大头——fbank 已测（~1.87ms/16k samples），ort Session::run 才是
//! 每chunk 的主要开销（encoder + decoder 各一次 FFI 调用）。
//!
//! Setup 复用 test_streaming_paraformer_real_model 的模型加载逻辑：
//!   - 模型：csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en（HF cache）
//!   - 测试 wav：test_wavs/0.wav（10s 中英混合）
//!   - chunk_size：9600 samples = 600ms（与生产 streaming 配置一致）
//!
//! 跑法：
//!   cargo bench -p octopus-asr-local --bench streaming_paraformer -- --save-baseline <name>
//!   cargo bench -p octopus-asr-local --bench streaming_paraformer -- --baseline <name>
//!
//! 前置：HF cache 须有该模型 + test_wavs。缺失则 bench 自动 skip（返回空 group）。

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use octopus_asr_local::audio::read_wav_16k;
use octopus_asr_local::streaming_paraformer::StreamingParaformer;

/// HF cache snapshot 路径解析（复用 streaming_paraformer.rs:848 的 test helper 逻辑）。
/// 返回 test_wavs 目录（含 0.wav）。
fn hf_test_wavs(repo: &str) -> Option<std::path::PathBuf> {
    let base = format!(
        "{}/.cache/huggingface/hub/models--{}",
        std::env::var("HOME").unwrap_or_default(),
        repo.replace('/', "--")
    );
    let snapshots = std::path::Path::new(&base).join("snapshots");
    if !snapshots.exists() {
        return None;
    }
    let mut entries = std::fs::read_dir(&snapshots).ok()?;
    entries
        .next()?
        .ok()
        .map(|e| e.path().join("test_wavs"))
        .filter(|p| p.exists())
}

const REPO: &str = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
const ENGINE_NAME: &str = "paraformer-bilingual";
const CHUNK_MS: usize = 600;
const SAMPLE_RATE: usize = 16000;

/// 基准 1：StreamingParaformer::new（ort Session 构造 + 模型加载）。
/// 分离「加载」与「推理」——加载是 one-shot，推理才是 per-chunk 热路径。
fn bench_engine_new(c: &mut Criterion) {
    // 加载是重活（几百 ms），减少采样数避免 bench 跑太久
    let mut group = c.benchmark_group("streaming_paraformer_new");
    group.sample_size(10); // 默认 100，加载模型 10 次够了
    group.measurement_time(std::time::Duration::from_secs(20));

    group.bench_function("paraformer-bilingual", |b| {
        b.iter(|| {
            let engine = StreamingParaformer::new(black_box(ENGINE_NAME)).expect("new");
            black_box(engine);
        });
    });
    group.finish();
}

/// 基准 2：accept_samples per-chunk 推理（streaming 真正的热路径）。
/// engine 在 iter 外加载一次，只测推理循环。
fn bench_accept_samples(c: &mut Criterion) {
    let test_wavs = match hf_test_wavs(REPO) {
        Some(p) => p,
        None => {
            eprintln!(
                "[skip] HF cache 未找到 {} test_wavs，streaming_paraformer bench 跳过",
                REPO
            );
            return;
        }
    };
    let wav_path = test_wavs.join("0.wav");
    if !wav_path.exists() {
        eprintln!("[skip] 测试 wav 不存在: {}", wav_path.display());
        return;
    }
    let samples = read_wav_16k(wav_path.to_str().unwrap()).expect("read_wav_16k");
    eprintln!(
        "[bench] 样本数: {} ({:.2}s)",
        samples.len(),
        samples.len() as f32 / SAMPLE_RATE as f32
    );

    let chunk_size = SAMPLE_RATE * CHUNK_MS / 1000; // 9600
    let chunks: Vec<&[f32]> = samples.chunks(chunk_size).collect();
    eprintln!("[bench] chunk 数: {} ({}ms/chunk)", chunks.len(), CHUNK_MS);

    let mut group = c.benchmark_group("streaming_paraformer_accept_samples");
    group.throughput(Throughput::Elements(chunks.len() as u64));

    // 不同 chunk 数测整体推理（1 / 5 / all chunks），看 per-chunk 是否稳定
    for &n_chunks in &[1usize, 5, chunks.len()] {
        let n_chunks = n_chunks.min(chunks.len());
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}chunks", n_chunks)),
            &n_chunks,
            |b, &n| {
                b.iter_batched(
                    // 每次 iter 重建 engine（推理有状态：累积 token/caches），保证可比
                    || StreamingParaformer::new(ENGINE_NAME).expect("new"),
                    |mut engine| {
                        for chunk in chunks.iter().take(n) {
                            let _ = engine
                                .accept_samples(black_box(chunk))
                                .expect("accept_samples");
                        }
                        black_box(engine);
                    },
                    criterion::BatchSize::PerIteration,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_engine_new, bench_accept_samples);
criterion_main!(benches);
