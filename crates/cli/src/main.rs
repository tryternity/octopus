use anyhow::Result;
use clap::{Parser, Subcommand};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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
    }
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
    let samples = octopus_asr::audio::read_wav_16k(wav_path)?;
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
            octopus_asr::audio::read_wav_16k(&out_path_owned)
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
    let engines = octopus_asr::config::list_engines()?;
    if engines.is_empty() {
        anyhow::bail!("No ASR engines configured in DB");
    }

    println!("可用模型：");
    for (i, e) in engines.iter().enumerate() {
        let cat_name = match e.category {
            octopus_asr::config::EngineCategory::Whisper => "Whisper",
            octopus_asr::config::EngineCategory::SenseVoice => "SenseVoice",
            octopus_asr::config::EngineCategory::Paraformer => "Paraformer",
            octopus_asr::config::EngineCategory::Qwen3Asr => "Qwen3-ASR",
            octopus_asr::config::EngineCategory::Zipformer => "Zipformer",
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

    Ok(engines[choice - 1].name.clone())
}

fn run_e2e(language: &str) -> Result<()> {
    let model = select_model()?;
    let bare = octopus_asr::config::parse_model_spec(&model).name().to_string();
    // Use streaming mode for Paraformer and Zipformer models
    let category = octopus_asr::config::resolve_engine_category(&model);
    if category == Some(octopus_asr::config::EngineCategory::Paraformer) {
        return run_e2e_streaming_paraformer(&bare);
    }
    if category == Some(octopus_asr::config::EngineCategory::Zipformer) {
        return run_e2e_streaming_zipformer(&bare);
    }

    println!("Recording from config... Press Enter to stop.\n");

    let all_samples = record_from_config()?;
    let duration = all_samples.len() as f64 / 16000.0;
    println!("Recorded {} samples ({:.2}s)", all_samples.len(), duration);

    // VAD filter
    let vad_path = octopus_asr::config::find_silero_vad()?;
    let mut vad = octopus_asr::vad::SileroVad::new(&vad_path)?;
    let speech = octopus_asr::audio::filter_speech(&all_samples, &mut vad, 480, 0.5);

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

fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let bare = octopus_asr::config::parse_model_spec(model).name();
    let category = octopus_asr::config::resolve_engine_category(model);
    match category {
        Some(octopus_asr::config::EngineCategory::Whisper) => {
            octopus_asr::whisper::transcribe(bare, samples, language)
        }
        Some(octopus_asr::config::EngineCategory::Paraformer) => {
            octopus_asr::paraformer::transcribe(bare, samples, language)
        }
        Some(octopus_asr::config::EngineCategory::Qwen3Asr) => {
            octopus_asr::qwen3_asr::transcribe(bare, samples, language)
        }
        Some(octopus_asr::config::EngineCategory::Zipformer) => {
            octopus_asr::zipformer::transcribe(bare, samples, language)
        }
        Some(octopus_asr::config::EngineCategory::SenseVoice) | None => {
            octopus_asr::sensevoice::transcribe(bare, samples, language)
        }
    }
}

fn show_config() -> Result<()> {
    println!("Config & Model Discovery");
    println!("{}", "=".repeat(70));
    println!(
        "OCTOPUS_HOME: {}",
        octopus_infra::octopus_config_home().display()
    );

    let config = octopus_asr::config::load_config()?;
    let app_cfg = octopus_infra::config::load_config()?;
    match octopus_asr::config::resolve_active_engine(&app_cfg.asr_engine) {
        Ok(r) => println!(
            "ASR active: {} (category: {:?}, from config.yaml asr_engine='{}')",
            r.name, r.category, app_cfg.asr_engine
        ),
        Err(e) => println!(
            "ASR active: <resolve error: {}> (asr_engine='{}')",
            e, app_cfg.asr_engine
        ),
    }

    let vad_path = octopus_asr::config::find_silero_vad()?;
    let vad_size = std::fs::metadata(&vad_path)?.len() as f64 / 1_048_576.0;
    println!("  VAD model (固定路径): {} ({:.1} MB)", vad_path.display(), vad_size);

    if let Some(whisper) = &config.asr.whisper {
        for (id, entry) in whisper {
            let hf = octopus_asr::config::resolve_model_dir(&entry.source)?;
            let onnx = octopus_asr::config::find_onnx_dir(&hf);
            println!("  Whisper [{}]: {}", id, entry.source);
            println!("    ONNX dir: {}", onnx.display());
        }
    }

    if let Some(sensevoice) = &config.asr.sensevoice {
        for (id, entry) in sensevoice {
            let hf = octopus_asr::config::resolve_model_dir(&entry.source)?;
            println!("  SenseVoice [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf.display());
        }
    }

    if let Some(paraformer) = &config.asr.paraformer {
        for (id, entry) in paraformer {
            let hf = octopus_asr::config::resolve_model_dir(&entry.source)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("error: {}", e));
            println!("  Paraformer [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf);
        }
    }

    if let Some(qwen3_asr) = &config.asr.qwen3_asr {
        for (id, entry) in qwen3_asr {
            let hf = octopus_asr::config::resolve_model_dir(&entry.source)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("error: {}", e));
            println!("  Qwen3-ASR [{}]: {}", id, entry.source);
            println!("    HF cache: {}", hf);
        }
    }

    if let Some(zipformer) = &config.asr.zipformer {
        for (id, entry) in zipformer {
            let hf = octopus_asr::config::resolve_model_dir(&entry.source)
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
                samples_clone.lock().unwrap().extend_from_slice(&mono);
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
                samples_clone.lock().unwrap().extend_from_slice(&mono);
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
    octopus_asr::audio::resample_to_16k(&recorded, sample_rate)
}

/// Streaming e2e for Paraformer: Mic → chunk → StreamingParaformer → partial text
/// Prints [partial] results every ~625ms and [final] on Enter.
fn run_e2e_streaming_paraformer(model: &str) -> Result<()> {
    println!("Streaming Paraformer e2e — model: {}", model);
    println!("Speak into the microphone. Press Enter to stop.\n");

    let mut engine = octopus_asr::streaming_paraformer::StreamingParaformer::new(model)?;

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
        Some(octopus_asr::audio::AudioResampler::new(native_rate)?)
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
                buffer_clone.lock().unwrap().extend_from_slice(&mono);
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
                    buffer_clone.lock().unwrap().extend_from_slice(&mono);
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
            let mut buf = buffer.lock().unwrap();
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
        let mut buf = buffer.lock().unwrap();
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

    let mut engine = octopus_asr::streaming_zipformer::StreamingZipformer::new(model)?;

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
        Some(octopus_asr::audio::AudioResampler::new(native_rate)?)
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
                    bc.lock().unwrap().extend_from_slice(&mono);
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
                    bc.lock().unwrap().extend_from_slice(&mono);
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
            let mut buf = buffer.lock().unwrap();
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
        let mut buf = buffer.lock().unwrap();
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

    let samples = octopus_asr::audio::read_wav_16k(wav_path)?;
    let duration = samples.len() as f64 / 16000.0;
    println!(
        "Stream-test: {} samples ({:.2}s), model: {}",
        samples.len(),
        duration,
        model
    );

    let bare = octopus_asr::config::parse_model_spec(model).name();
    let category = octopus_asr::config::resolve_engine_category(model);
    match category {
        Some(octopus_asr::config::EngineCategory::Zipformer) => {
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
    let mut engine = octopus_asr::streaming_paraformer::StreamingParaformer::new(model)?;

    let chunk_size = 10_000;
    let mut chunk_idx = 0;
    let mut accumulated = String::new();
    let start = std::time::Instant::now();

    for chunk in samples.chunks(chunk_size) {
        match engine.accept_samples(chunk)? {
            Some(text) => {
                accumulated.push_str(&text);
                let t = chunk_idx as f64 * 0.625;
                println!("[chunk {} @{:.1}s] {}", chunk_idx, t, accumulated);
            }
            None => {}
        }
        chunk_idx += 1;
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
    let mut engine = octopus_asr::streaming_zipformer::StreamingZipformer::new(model)?;

    // Chunk size: enough samples for one chunk of fbank frames.
    // Approx: chunk_shift * Z_FRAME_SHIFT = chunk_shift * 160 samples.
    // For shift=64: ~10240 samples; for shift=32: ~5120 samples.
    // Use 625ms chunks (~10000 samples) as a reasonable default.
    let chunk_size = 10_000;
    let mut chunk_idx = 0;
    let start = std::time::Instant::now();

    for chunk in samples.chunks(chunk_size) {
        match engine.accept_samples(chunk)? {
            Some(text) => {
                let t = chunk_idx as f64 * 0.625;
                println!("[chunk {} @{:.1}s] {}", chunk_idx, t, text);
            }
            None => {}
        }
        chunk_idx += 1;
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
