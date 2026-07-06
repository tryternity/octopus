use anyhow::Result;
use clap::{Parser, Subcommand};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

mod pipeline;

#[derive(Parser)]
#[command(name = "octopus-cli", about = "ASR inference toolkit", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available input devices
    Devices,
    /// Transcribe WAV file
    Transcribe {
        /// Path to WAV file
        wav_path: String,
        /// ASR engine name（DB models 表中的 name，用 `octopus-cli config` 查看可选值）
        #[arg(long, default_value = "sherpa-onnx-sense-voice-funasr-nano-int8")]
        model: String,
        /// Language: auto, zh, en, ja, ...
        #[arg(long, default_value = "auto")]
        language: String,
    },
    /// Mic → VAD → ASR → text（交互式选择模型）
    E2e {
        /// Language: auto, zh, en, ja, ...
        #[arg(long, default_value = "auto")]
        language: String,
    },
    /// Show model discovery info
    Config,
    /// Stream-test: feed WAV file chunk-by-chunk to streaming ASR engine
    StreamTest {
        /// Path to WAV file
        wav_path: String,
        /// ASR engine name (from DB models table; paraformer/zipformer section)
        #[arg(long, default_value = "paraformer-streaming")]
        model: String,
    },
    /// Transcribe URL (Bilibili, YouTube, etc.) by extracting audio and speech recognition
    TranscribeUrl {
        /// URL of the video/audio
        url: String,
        /// ASR engine name（DB models 表中的 name，用 `octopus-cli config` 查看可选值）
        #[arg(long, default_value = "sherpa-onnx-sense-voice-funasr-nano-int8")]
        model: String,
        /// Language: auto, zh, en, ja, ...
        #[arg(long, default_value = "auto")]
        language: String,
        /// Output path to save the separated WAV file
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        output: Option<String>,
        /// Do not delete downloaded video file, skip download if cached file exists
        #[arg(long)]
        unclear: bool,
    },
    /// 下载 HuggingFace 模型到 ~/.octopus/models/<repo>
    Download {
        /// HF repo，如 onnx-community/whisper-small（与 DB models 的 entry.source 一致）
        repo: String,
        /// 只下匹配的文件（glob，对齐 hf-cli，`*` 跨 `/`）。空 = 下整库
        #[arg(long)]
        include: Vec<String>,
        /// 排除匹配的文件
        #[arg(long)]
        exclude: Vec<String>,
        /// HF 镜像 host（如 https://hf-mirror.com），覆盖 config 的 download_mirror
        #[arg(long)]
        mirror: Option<String>,
    },
    /// 同步本地模型状态：扫描所有本地 ASR 模型，就绪的算 sha256 清单写入 secret_key + 置 is_enabled=true；未就绪置 false
    SyncModels,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Devices => list_devices(),
        Commands::Transcribe {
            wav_path,
            model,
            language,
        } => transcribe_file(&wav_path, &model, &language),
        Commands::E2e { language } => run_e2e(&language),
        Commands::Config => show_config(),
        Commands::StreamTest { wav_path, model } => stream_test(&wav_path, &model),
        Commands::TranscribeUrl {
            url,
            model,
            language,
            output,
            unclear,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(transcribe_url(&url, &model, &language, output.as_deref(), unclear))
        }
        Commands::Download {
            repo,
            include,
            exclude,
            mirror,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_download(&repo, &include, &exclude, mirror.as_deref()))
        }
        Commands::SyncModels => run_sync_models(),
    }
}

/// 同步本地模型状态：遍历所有本地 ASR 模型，就绪的算 sha256 清单写入 secret_key + 置
/// is_enabled=true；未就绪置 false。末尾 reload 运行时缓存。供首次填充 secret_key 或批量复核用。
fn run_sync_models() -> Result<()> {
    let rows = octopus_infra::db::list_all_local_asr_models()?;
    let mut ready = 0usize;
    let mut missing = 0usize;
    for r in &rows {
        match octopus_asr_local::config::resolve_model_dir(&r.source) {
            Ok(dir) => {
                let manifest = octopus_asr_local::manifest::bootstrap_manifest(&dir)?;
                let count =
                    serde_json::from_str::<octopus_asr_local::manifest::Manifest>(&manifest)?.len();
                octopus_infra::db::set_model_secret_key(&r.model_name, &manifest)?;
                octopus_infra::db::set_model_enabled(&r.model_name, true)?;
                ready += 1;
                println!(
                    "✓ {} [{}]: 就绪，{} 个文件已记录清单到 secret_key",
                    r.model_name, r.source, count
                );
            }
            Err(_) => {
                octopus_infra::db::set_model_enabled(&r.model_name, false)?;
                missing += 1;
                println!(
                    "✗ {} [{}]: 文件未就绪，置 is_enabled=false",
                    r.model_name, r.source
                );
            }
        }
    }
    octopus_asr_local::config::reload_models_config();
    println!(
        "\n完成：{} 个就绪（已写 sha256 清单），{} 个未就绪",
        ready, missing
    );
    Ok(())
}

fn list_devices() -> Result<()> {
    println!("Available input devices:");
    let host = cpal::default_host();
    let devices: Vec<_> = host
        .input_devices()?
        .filter_map(|d| {
            let _name = d.name().ok()?;
            Some(d)
        })
        .collect();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    for (i, device) in devices.iter().enumerate() {
        let name = device.name().unwrap_or_default();
        let is_default = default.as_ref() == Some(&name);
        let marker = if is_default { " (default)" } else { "" };
        println!("  [{}] {}{}", i, name, marker);
    }
    Ok(())
}

fn transcribe_file(wav_path: &str, model: &str, language: &str) -> Result<()> {
    if !std::path::Path::new(wav_path).exists() {
        anyhow::bail!("File not found: {}", wav_path);
    }
    let samples = octopus_asr_local::audio::read_wav_16k(wav_path)?;
    let duration = samples.len() as f64 / 16000.0;
    println!(
        "Audio: {} samples ({:.2}s), model: {}, language: {}",
        samples.len(),
        duration,
        model,
        language
    );

    let start = std::time::Instant::now();
    let text = do_transcribe(model, language, &samples)?;
    let elapsed = start.elapsed();

    println!("{}", text);
    eprintln!(
        "{:.2}s (RTF: {:.2}x)",
        elapsed.as_secs_f64(),
        duration / elapsed.as_secs_f64()
    );
    Ok(())
}

async fn transcribe_url(url: &str, model: &str, language: &str, output: Option<&str>, unclear: bool) -> Result<()> {
    use tokio::process::Command;
    use std::process::Stdio;
    use tokio::io::{BufReader, AsyncBufReadExt, AsyncReadExt};

    #[derive(serde::Deserialize, Debug)]
    struct VideoMeta {
        title: String,
        duration: f64,
        author: String,
    }

    let resolved_output = if let Some(out_path) = output {
        if out_path.is_empty() {
            let url_md5 = format!("{:x}", md5::compute(url));
            Some(octopus_infra::octopus_config_home().join("tmp").join(format!("{}.wav", url_md5)))
        } else {
            Some(std::path::PathBuf::from(out_path))
        }
    } else {
        None
    };

    // 尝试寻找 octopus-dlp 二进制文件，如果存在则直接运行，消除 cargo run 开销：
    // 1. 查找当前运行的 CLI 相同目录下的二进制
    // 2. 查找 octopus_config_home()/bin/ 目录下的二进制
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    let mut octopus_dlp_bin = None;
    if let Some(mut dir) = exe_dir {
        #[cfg(target_os = "windows")]
        dir.push("octopus-dlp.exe");
        #[cfg(not(target_os = "windows"))]
        dir.push("octopus-dlp");

        if dir.exists() {
            octopus_dlp_bin = Some(dir);
        }
    }

    if octopus_dlp_bin.is_none() {
        let mut home_bin = octopus_infra::octopus_config_home().to_path_buf();
        home_bin.push("bin");
        #[cfg(target_os = "windows")]
        home_bin.push("octopus-dlp.exe");
        #[cfg(not(target_os = "windows"))]
        home_bin.push("octopus-dlp");

        if home_bin.exists() {
            octopus_dlp_bin = Some(home_bin);
        }
    }

    if let Some(ref bin_path) = octopus_dlp_bin {
        println!("Spawning octopus-dlp process ({}) directly to extract audio from: {} ...", bin_path.display(), url);
    } else {
        println!("Spawning octopus-dlp process via cargo run to extract audio from: {} ...", url);
    }

    let mut cmd = if let Some(ref bin_path) = octopus_dlp_bin {
        let mut c = Command::new(bin_path);
        c.arg(url);
        c
    } else {
        let mut c = Command::new("cargo");
        c.arg("run")
            .arg("--quiet")
            .arg("--package")
            .arg("octopus-dlp")
            .arg("--")
            .arg(url);
        c
    };

    if unclear {
        cmd.arg("--unclear");
    }
    if let Some(ref out_path) = resolved_output {
        cmd.arg("-o").arg(out_path);
    }

    let stdout_cfg = if resolved_output.is_none() {
        Stdio::piped()
    } else {
        Stdio::null()
    };

    let mut child = cmd
        .stdout(stdout_cfg)
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take().unwrap();

    let duration_sec = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let duration_sec_clone = duration_sec.clone();

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Ok(meta) = serde_json::from_str::<VideoMeta>(&line) {
                println!("--- Video Info ---");
                println!("Title: {}", meta.title);
                println!("Author: {}", meta.author);
                println!("Duration: {:.2}s", meta.duration);
                println!("------------------");
                duration_sec_clone.store(meta.duration.round() as u32, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            eprintln!("[dlp] {}", line);
        }
    });

    let mut samples = Vec::new();
    let start_extract = std::time::Instant::now();

    if let Some(mut stdout) = stdout {
        println!("Streaming audio data from pipeline and decoding raw float PCM...");
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            for raw_sample in chunk[..n].chunks_exact(4) {
                let sample = f32::from_le_bytes(raw_sample.try_into().unwrap());
                samples.push(sample);
            }
        }
        let extract_elapsed = start_extract.elapsed();
        println!(
            "Audio extraction finished. Read {} samples in {:.2}s.",
            samples.len(),
            extract_elapsed.as_secs_f64()
        );
    }

    let status = child.wait().await?;
    let _ = stderr_task.await;

    if !status.success() {
        anyhow::bail!("octopus-dlp process exited with error status");
    }

    if let Some(ref out_path) = resolved_output {
        let out_path_str = out_path.to_string_lossy().to_string();
        let out_path_owned = out_path_str.clone();
        samples = tokio::task::spawn_blocking(move || {
            octopus_asr_local::audio::read_wav_16k(&out_path_owned)
        }).await??;
        let extract_elapsed = start_extract.elapsed();
        println!(
            "Audio extraction finished (saved to {}). Read {} samples in {:.2}s.",
            out_path_str,
            samples.len(),
            extract_elapsed.as_secs_f64()
        );
    }

    if samples.is_empty() {
        anyhow::bail!("No audio samples extracted from URL");
    }

    println!("Initializing ASR engine (model: {})...", model);
    let start_transcribe = std::time::Instant::now();
    let text = do_transcribe(model, language, &samples)?;
    let transcribe_elapsed = start_transcribe.elapsed();

    println!("\n--- Transcription Result ---");
    println!("{}", text);
    println!("----------------------------");
    let duration_val = duration_sec.load(std::sync::atomic::Ordering::Relaxed) as f64;
    println!(
        "Transcribe time: {:.2}s (RTF: {:.2}x)",
        transcribe_elapsed.as_secs_f64(),
        duration_val / transcribe_elapsed.as_secs_f64()
    );

    Ok(())
}

/// 列出所有可用模型，让用户输入数字选择
fn select_model() -> Result<String> {
    let engines = octopus_asr_local::config::list_engines()?;
    if engines.is_empty() {
        anyhow::bail!("No ASR engines configured in DB");
    }

    println!("可用模型：");
    for (i, e) in engines.iter().enumerate() {
        let cat_name = match e.category {
            octopus_asr_local::config::EngineCategory::Whisper => "Whisper",
            octopus_asr_local::config::EngineCategory::Paraformer => "Paraformer",
            octopus_asr_local::config::EngineCategory::Qwen3Asr => "Qwen3-ASR",
            octopus_asr_local::config::EngineCategory::Zipformer => "Zipformer",
            octopus_asr_local::config::EngineCategory::Moonshine => "Moonshine",
            octopus_asr_local::config::EngineCategory::Aliyun => "Aliyun(云)",
            octopus_asr_local::config::EngineCategory::ByteDance => "ByteDance(云)",
            octopus_asr_local::config::EngineCategory::Tencent => "Tencent(云)",
            octopus_asr_local::config::EngineCategory::Baidu => "Baidu(云)",
            octopus_asr_local::config::EngineCategory::SenseVoiceOrig => "SenseVoice(原版)",
            octopus_asr_local::config::EngineCategory::FireRed => "FireRed",
        };
        let desc = if e.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", e.description)
        };
        println!("  {}. {} [{}]{}", i + 1, e.name, cat_name, desc);
    }

    print!("\n请选择模型 (1-{}): ", engines.len());
    std::io::Write::flush(&mut std::io::stdout())?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let choice: usize = input
        .trim()
        .parse()
        .ok()
        .and_then(|n: usize| if n >= 1 && n <= engines.len() { Some(n) } else { None })
        .ok_or_else(|| anyhow::anyhow!("无效选择，请输入 1-{} 之间的数字", engines.len()))?;

    // 构造 3-part spec "{provider}:{category_str}:{model_name}" 返回给下游。
    // category_str 复用 octopus_asr_local::config::category_label（统一映射，Aliyun → "aliyun"）。
    let picked = &engines[choice - 1];
    let cat_str = octopus_asr_local::config::category_label(picked.category);
    Ok(format!("{}:{}:{}", picked.provider, cat_str, picked.name))
}

fn run_e2e(language: &str) -> Result<()> {
    let model = select_model()?;
    let bare = octopus_asr_local::config::parse_model_spec(&model).model_name().to_string();
    // Use streaming mode for Paraformer and Zipformer models
    let category = octopus_asr_local::config::resolve_engine_category(&model);
    if category == Some(octopus_asr_local::config::EngineCategory::Paraformer) {
        return run_e2e_streaming_paraformer(&bare);
    }
    if category == Some(octopus_asr_local::config::EngineCategory::Zipformer) {
        return run_e2e_streaming_zipformer(&bare);
    }

    println!("Recording from config... Press Enter to stop.\n");

    let all_samples = record_from_config()?;
    let duration = all_samples.len() as f64 / 16000.0;
    println!("Recorded {} samples ({:.2}s)", all_samples.len(), duration);

    // VAD filter
    let vad_path = octopus_asr_local::config::find_silero_vad()?;
    let mut vad = octopus_asr_local::vad::SileroVad::new(&vad_path)?;
    let speech = octopus_asr_local::audio::filter_speech(&all_samples, &mut vad, 480, 0.5);

    if speech.is_empty() {
        println!("No speech detected!");
        return Ok(());
    }
    let speech_dur = speech.len() as f64 / 16000.0;
    println!(
        "Speech: {} samples ({:.2}s), model: {}, language: {}",
        speech.len(),
        speech_dur,
        model,
        language
    );

    let start = std::time::Instant::now();
    let text = do_transcribe(&model, language, &speech)?;
    let elapsed = start.elapsed();

    println!("{}", text);
    eprintln!(
        "{:.2}s (RTF: {:.2}x)",
        elapsed.as_secs_f64(),
        duration / elapsed.as_secs_f64()
    );
    Ok(())
}

/// 批处理转写入口（transcribe / transcribe-url 共用）：委托 `pipeline::run`，
/// 走 AsrEngineManager + transcribe_batch（VAD 分段 + 纠错 + 简繁归一化）。
fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    pipeline::run(model, language, samples)
}

fn show_config() -> Result<()> {
    println!("Config & Model Discovery");
    println!("{}", "=".repeat(70));
    println!(
        "OCTOPUS_HOME: {}",
        octopus_infra::octopus_config_home().display()
    );

    let config = octopus_asr_local::config::load_config()?;
    let app_cfg = octopus_infra::config::load_config()?;
    match octopus_asr_local::config::resolve_active_engine(&app_cfg.asr_engine) {
        Ok(r) => println!(
            "ASR active: {} (category: {:?}, from config.yaml asr_engine='{}')",
            r.name, r.category, app_cfg.asr_engine
        ),
        Err(e) => println!(
            "ASR active: <resolve error: {}> (asr_engine='{}')",
            e, app_cfg.asr_engine
        ),
    }

    let vad_path = octopus_asr_local::config::find_silero_vad()?;
    let vad_size = std::fs::metadata(&vad_path)?.len() as f64 / 1_048_576.0;
    println!("  VAD model (固定路径): {} ({:.1} MB)", vad_path.display(), vad_size);

    if let Some(whisper) = &config.asr.whisper {
        for (id, entry) in whisper {
            let hf = octopus_asr_local::config::resolve_model_dir(&entry.source)?;
            let onnx = octopus_asr_local::config::find_onnx_dir(&hf);
            println!("  Whisper [{}]: {}", id, entry.source);
            println!("    ONNX dir: {}", onnx.display());
        }
    }

    if let Some(paraformer) = &config.asr.paraformer {
        for (id, entry) in paraformer {
            let hf = octopus_asr_local::config::resolve_model_dir(&entry.source)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("error: {}", e));
            println!("  Paraformer [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf);
        }
    }

    if let Some(qwen3_asr) = &config.asr.qwen3_asr {
        for (id, entry) in qwen3_asr {
            let hf = octopus_asr_local::config::resolve_model_dir(&entry.source)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("error: {}", e));
            println!("  Qwen3-ASR [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf);
        }
    }

    if let Some(zipformer) = &config.asr.zipformer {
        for (id, entry) in zipformer {
            let hf = octopus_asr_local::config::resolve_model_dir(&entry.source)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("error: {}", e));
            println!("  Zipformer [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf);
        }
    }

    println!("\n✓ OK");
    Ok(())
}

fn record_from_config() -> Result<Vec<f32>> {
    let app_cfg = octopus_infra::config::load_config()?;
    let device_name = if app_cfg.microphone.is_empty() {
        ""
    } else {
        &app_cfg.microphone
    };

    let host = cpal::default_host();
    let device = if device_name.is_empty() {
        host.default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device"))?
    } else {
        host.input_devices()?
            .find(|d| d.name().map(|n| n.contains(device_name)).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device_name))?
    };

    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    println!("Recording from: {}", device.name().unwrap_or_default());
    println!("Sample rate: {}, Channels: {}", sample_rate, channels);

    let samples = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let samples_clone = samples.clone();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|c| c.iter().sum::<f32>() / channels as f32)
                    .collect();
                samples_clone.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
            },
            |err| eprintln!("Audio error: {}", err),
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|c| {
                        c.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>() / channels as f32
                    })
                    .collect();
                samples_clone.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
            },
            |err| eprintln!("Audio error: {}", err),
            None,
        )?,
        fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
    };

    stream.play()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    drop(stream);

    let recorded = std::sync::Arc::try_unwrap(samples)
        .unwrap()
        .into_inner()
        .unwrap();
    octopus_asr_local::audio::resample_to_16k(&recorded, sample_rate)
}

/// Streaming e2e for Paraformer: Mic → chunk → StreamingParaformer → partial text
/// Prints [partial] results every ~625ms and [final] on Enter.
fn run_e2e_streaming_paraformer(model: &str) -> Result<()> {
    println!("Streaming Paraformer e2e — model: {}", model);
    println!("Speak into the microphone. Press Enter to stop.\n");

    let mut engine = octopus_asr_local::streaming_paraformer::StreamingParaformer::new(model)?;

    // Set up microphone
    let app_cfg = octopus_infra::config::load_config()?;
    let device_name = if app_cfg.microphone.is_empty() {
        ""
    } else {
        &app_cfg.microphone
    };

    let host = cpal::default_host();
    let device = if device_name.is_empty() {
        host.default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device"))?
    } else {
        host.input_devices()?
            .find(|d| d.name().map(|n| n.contains(device_name)).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device_name))?
    };

    let config = device.default_input_config()?;
    let native_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    println!(
        "Recording from: {} ({}Hz, {}ch)",
        device.name().unwrap_or_default(),
        native_rate,
        channels
    );

    // Shared sample buffer
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let buffer_clone = buffer.clone();
    // Resample state for non-16kHz devices
    let mut resampler = if native_rate != 16000 {
        Some(octopus_asr_local::audio::AudioResampler::new(native_rate)?)
    } else {
        None
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|c| c.iter().sum::<f32>() / channels as f32)
                    .collect();
                buffer_clone.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
            },
            |err| eprintln!("Audio error: {}", err),
            None,
        )?,
        cpal::SampleFormat::I16 => {
            let buffer_clone = buffer.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| {
                            c.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    buffer_clone.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
                },
                |err| eprintln!("Audio error: {}", err),
                None,
            )?
        }
        fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
    };

    stream.play()?;

    // Polling interval is always ~625ms real-time regardless of sample rate.
    // The resample step converts native-rate samples to 16kHz before feeding the engine.
    let chunk_duration_ms: u64 = 625;

    // Wait for Enter on a separate thread
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_clone = done.clone();
    let enter_thread = std::thread::spawn(move || {
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        done_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let mut accumulated = String::new();

    while !done.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(chunk_duration_ms));

        // Drain the buffer
        let raw_samples: Vec<f32> = {
            let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buf)
        };

        if raw_samples.is_empty() {
            continue;
        }

        // Resample if needed (48kHz → 16kHz etc.)
        let samples_16k = if let Some(ref mut r) = resampler {
            match r.resample(&raw_samples) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("\n[resample error] {}", e);
                    continue;
                }
            }
        } else {
            raw_samples
        };

        // Feed to streaming engine
        match engine.accept_samples(&samples_16k) {
            Ok(Some(text)) => {
                accumulated.push_str(&text);
                println!("[partial] {}", accumulated);
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            Ok(None) => {}
            Err(e) => eprintln!("\n[error] {}", e),
        }
    }

    // Wait for Enter thread
    let _ = enter_thread.join();

    // Flush remaining
    // Drain one last time
    let raw_samples: Vec<f32> = {
        let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *buf)
    };
    let samples_16k = if let Some(ref mut r) = resampler {
        let mut s = r.resample(&raw_samples)?;
        s.extend(r.flush()?);
        s
    } else {
        raw_samples
    };
    if !samples_16k.is_empty() {
        engine.accept_samples(&samples_16k).ok();
    }

    match engine.finish() {
        Ok(text) if !text.is_empty() => {
            accumulated = text;
        }
        Ok(_) => {}
        Err(e) => eprintln!("\n[finish error] {}", e),
    }

    // Clean up: drop stream to stop recording
    drop(stream);

    println!("\n[final]   {}", accumulated);
    Ok(())
}

/// Streaming e2e for Zipformer: Mic → chunk → StreamingZipformer → partial text
fn run_e2e_streaming_zipformer(model: &str) -> Result<()> {
    println!("Streaming Zipformer e2e — model: {}", model);
    println!("Speak into the microphone. Press Enter to stop.\n");

    let mut engine = octopus_asr_local::streaming_zipformer::StreamingZipformer::new(model)?;

    // Set up microphone (same pattern as paraformer e2e)
    let app_cfg = octopus_infra::config::load_config()?;
    let device_name = if app_cfg.microphone.is_empty() {
        ""
    } else {
        &app_cfg.microphone
    };

    let host = cpal::default_host();
    let device = if device_name.is_empty() {
        host.default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device"))?
    } else {
        host.input_devices()?
            .find(|d| d.name().map(|n| n.contains(device_name)).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device_name))?
    };

    let config = device.default_input_config()?;
    let native_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let mut resampler = if native_rate != 16000 {
        Some(octopus_asr_local::audio::AudioResampler::new(native_rate)?)
    } else {
        None
    };

    println!(
        "Recording from: {} ({}Hz, {}ch)",
        device.name().unwrap_or_default(),
        native_rate,
        channels
    );

    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let bc = buffer.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| c.iter().sum::<f32>() / channels as f32)
                        .collect();
                    bc.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
                },
                |err| eprintln!("Audio error: {}", err),
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let bc = buffer.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| {
                            c.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    bc.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&mono);
                },
                |err| eprintln!("Audio error: {}", err),
                None,
            )?
        }
        fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
    };

    stream.play()?;

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_clone = done.clone();
    let enter_thread = std::thread::spawn(move || {
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        done_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    while !done.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(625));

        let raw_samples: Vec<f32> = {
            let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buf)
        };

        if raw_samples.is_empty() {
            continue;
        }

        let samples_16k = if let Some(ref mut r) = resampler {
            match r.resample(&raw_samples) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("\n[resample error] {}", e);
                    continue;
                }
            }
        } else {
            raw_samples
        };

        match engine.accept_samples(&samples_16k) {
            Ok(Some(text)) => {
                println!("[partial] {}", text);
            }
            Ok(None) => {}
            Err(e) => eprintln!("\n[error] {}", e),
        }
    }

    let _ = enter_thread.join();

    // Flush remaining
    let raw_samples: Vec<f32> = {
        let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *buf)
    };
    let samples_16k = if let Some(ref mut r) = resampler {
        let mut s = r.resample(&raw_samples)?;
        s.extend(r.flush()?);
        s
    } else {
        raw_samples
    };
    if !samples_16k.is_empty() {
        engine.accept_samples(&samples_16k).ok();
    }

    match engine.finish() {
        Ok(text) if !text.is_empty() => {
            println!("[final]   {}", text);
        }
        Ok(_) => {}
        Err(e) => eprintln!("\n[finish error] {}", e),
    }

    drop(stream);
    Ok(())
}

/// Stream-test: feed a WAV file chunk-by-chunk through streaming ASR engine.
/// Prints [partial] after each chunk and [final] at the end.
fn stream_test(wav_path: &str, model: &str) -> Result<()> {
    if !std::path::Path::new(wav_path).exists() {
        anyhow::bail!("File not found: {}", wav_path);
    }

    let samples = octopus_asr_local::audio::read_wav_16k(wav_path)?;
    let duration = samples.len() as f64 / 16000.0;
    println!(
        "Stream-test: {} samples ({:.2}s), model: {}",
        samples.len(),
        duration,
        model
    );

    let bare = octopus_asr_local::config::parse_model_spec(model).model_name();
    let category = octopus_asr_local::config::resolve_engine_category(model);
    match category {
        Some(octopus_asr_local::config::EngineCategory::Zipformer) => {
            stream_test_zipformer(samples, duration, bare)
        }
        _ => stream_test_paraformer(samples, duration, bare),
    }
}

fn stream_test_paraformer(
    samples: Vec<f32>,
    duration: f64,
    model: &str,
) -> Result<()> {
    let mut engine = octopus_asr_local::streaming_paraformer::StreamingParaformer::new(model)?;

    let chunk_size = 10_000;
    let mut accumulated = String::new();
    let start = std::time::Instant::now();

    for (chunk_idx, chunk) in samples.chunks(chunk_size).enumerate() {
        if let Some(text) = engine.accept_samples(chunk)? {
            accumulated.push_str(&text);
            let t = chunk_idx as f64 * 0.625;
            println!("[chunk {} @{:.1}s] {}", chunk_idx, t, accumulated);
        }
    }

    let final_text = engine.finish()?;
    let elapsed = start.elapsed();

    println!("[final]   {}", final_text);
    eprintln!(
        "{:.2}s (RTF: {:.2}x)",
        elapsed.as_secs_f64(),
        duration / elapsed.as_secs_f64()
    );
    Ok(())
}

fn stream_test_zipformer(
    samples: Vec<f32>,
    duration: f64,
    model: &str,
) -> Result<()> {
    let mut engine = octopus_asr_local::streaming_zipformer::StreamingZipformer::new(model)?;

    // Chunk size: enough samples for one chunk of fbank frames.
    // Approx: chunk_shift * Z_FRAME_SHIFT = chunk_shift * 160 samples.
    // For shift=64: ~10240 samples; for shift=32: ~5120 samples.
    // Use 625ms chunks (~10000 samples) as a reasonable default.
    let chunk_size = 10_000;
    let start = std::time::Instant::now();

    for (chunk_idx, chunk) in samples.chunks(chunk_size).enumerate() {
        if let Some(text) = engine.accept_samples(chunk)? {
            let t = chunk_idx as f64 * 0.625;
            println!("[chunk {} @{:.1}s] {}", chunk_idx, t, text);
        }
    }

    let final_text = engine.finish()?;
    let elapsed = start.elapsed();

    println!("[final]   {}", final_text);
    eprintln!(
        "{:.2}s (RTF: {:.2}x)",
        elapsed.as_secs_f64(),
        duration / elapsed.as_secs_f64()
    );
    Ok(())
}

// ── download 子命令 ──

/// 构造 HF 下载请求。mirror 优先级：cli `--mirror` > config `download_mirror` > 空（官方源）。
/// target_dir 固定 `~/.octopus/models`，与 `resolve_model_dir` 第 3 级（`~/.octopus/models/<repo>`）一致。
fn build_hf_request(
    repo: String,
    include: Vec<String>,
    exclude: Vec<String>,
    cli_mirror: Option<String>,
    config_mirror: &str,
) -> octopus_download::HfRequest {
    let mirror = cli_mirror
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let c = config_mirror.trim();
            if c.is_empty() {
                None
            } else {
                Some(c.to_string())
            }
        });
    octopus_download::HfRequest {
        repo,
        include,
        exclude,
        source_url: mirror,
        target_dir: octopus_infra::octopus_config_home().join("models").to_path_buf(),
    }
}

/// 执行下载：resolve 文件列表 → 逐文件 Downloader::download + 进度打印。
/// 失败透传 anyhow（resolve 网络 / hash 校验 / 镜像 fallback 均由 download crate 处理）。
async fn run_download(
    repo: &str,
    include: &[String],
    exclude: &[String],
    cli_mirror: Option<&str>,
) -> Result<()> {
    let app_cfg = octopus_infra::config::load_config()?;
    let req = build_hf_request(
        repo.to_string(),
        include.to_vec(),
        exclude.to_vec(),
        cli_mirror.map(|s| s.to_string()),
        &app_cfg.download_mirror,
    );

    println!("解析 {} 的文件列表...", repo);
    let dl = octopus_download::Downloader::new(octopus_download::DownloadConfig::default())
        .map_err(|e| anyhow::anyhow!("初始化下载器失败: {e:?}"))?;
    let tasks = octopus_download::resolve_tasks(dl.client(), req)
        .await
        .map_err(|e| anyhow::anyhow!("resolve 失败: {e:?}"))?;
    if tasks.is_empty() {
        anyhow::bail!("没有匹配的文件——检查 --include/--exclude glob");
    }
    println!(
        "共 {} 个文件 → {}",
        tasks.len(),
        octopus_infra::octopus_config_home().join("models").display()
    );

    for (i, task) in tasks.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, tasks.len(), task.dest.display());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<octopus_download::Progress>(64);
        // rx move 进 printer：download 返回后 tx drop → channel 关闭 → rx.recv() 返回 None → printer 自然退出。
        // 勿在主作用域再 rx.close()——rx 已 move 进闭包，访问即 use-of-moved-value 编译错。
        let printer = tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                if let Some(total) = p.total_bytes {
                    let pct = p.downloaded_bytes as f64 / total as f64 * 100.0;
                    // 速度：download crate 250ms 推送 EMA 估算；下大模型时是关键 UX。
                    let spd = p
                        .speed_bps
                        .map(|s| format!(" {:.2} MB/s", s / 1_048_576.0))
                        .unwrap_or_default();
                    eprint!(
                        "\r  {}/{} bytes ({:.1}%){}   ",
                        p.downloaded_bytes, total, pct, spd
                    );
                }
            }
        });
        dl.download(task, tx, None)
            .await
            .map_err(|e| anyhow::anyhow!("下载 {} 失败: {e:?}", task.dest.display()))?;
        let _ = printer.await;
        // \x1b[2K 清当前行——进度行可能比 "✓ done" 长（大文件字节数多），不清会残留尾巴。
        eprintln!("\r\x1b[2K  ✓ done");
    }

    println!("\n完成。模型位于 ~/.octopus/models/{}/", repo);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_hf_request;

    #[test]
    fn build_request_cli_mirror_overrides_config() {
        // --mirror 优先于 config download_mirror
        let req = build_hf_request(
            "onnx-community/whisper-small".into(),
            vec!["onnx/*_int8.onnx".into()],
            vec![],
            Some("https://hf-mirror.com".into()),
            "https://ignored.example.com",
        );
        assert_eq!(req.repo, "onnx-community/whisper-small");
        assert_eq!(req.source_url.as_deref(), Some("https://hf-mirror.com"));
        assert_eq!(req.include, vec!["onnx/*_int8.onnx"]);
        assert!(req.target_dir.ends_with("models"));
    }

    #[test]
    fn build_request_config_mirror_when_no_cli() {
        // 无 --mirror → 用 config
        let req = build_hf_request(
            "org/m".into(),
            vec![],
            vec![],
            None,
            "https://hf-mirror.com",
        );
        assert_eq!(req.source_url.as_deref(), Some("https://hf-mirror.com"));
    }

    #[test]
    fn build_request_none_when_both_empty() {
        // cli 空 + config 空 → None（官方源，由 download crate 默认）
        let req = build_hf_request("org/m".into(), vec![], vec![], Some(String::new()), "");
        assert!(req.source_url.is_none());
        assert!(req.target_dir.ends_with("models"));
    }

    #[test]
    fn build_request_target_dir_under_octopus_models() {
        // target_dir 必须是 octopus_config_home/models（与 resolve_model_dir 第 3 级一致）
        let req = build_hf_request("org/m".into(), vec![], vec![], None, "");
        let expected = octopus_infra::octopus_config_home().join("models");
        assert_eq!(req.target_dir, expected);
    }
}
