use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Deserialize, Debug)]
struct YtdlpMetadata {
    title: Option<String>,
    duration: Option<f64>,
    uploader: Option<String>,
    _filename: Option<String>,
}

#[derive(Serialize, Debug)]
struct VideoMetadataOutput {
    title: String,
    duration: f64,
    author: String,
}

/// 下载文件清理守卫（RAII）：确保 ffmpeg spawn/wait 失败（`?` 提前返回）也清理临时下载文件，
/// 避免转码失败时磁盘泄漏（2026-07-09 审查 6e73257 修复）。
/// Drop 用同步 `std::fs::remove_file`——单文件 unlink 亚毫秒，可接受；
/// async Drop 不可行，故不沿用 `tokio::fs`。`keep=true`（`--unclear`）时 drop 不删。
struct DownloadedFileGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for DownloadedFileGuard {
    fn drop(&mut self) {
        if self.keep {
            eprintln!("Keeping downloaded video file: {:?}", self.path);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn has_binary_on_path(name: &str) -> bool {
    let cmd = if cfg!(target_os = "windows") { "where" } else { "which" };
    Command::new(cmd)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn get_binary_path(name: &str) -> Result<PathBuf> {
    // 1. 检查 ~/.octopus/bin/
    let home_bin = octopus_infra::octopus_config_home().join("bin").join(name);
    #[cfg(target_os = "windows")]
    let home_bin = home_bin.with_extension("exe");

    if home_bin.exists() {
        return Ok(home_bin);
    }

    // 2. 检查系统 PATH
    if has_binary_on_path(name).await {
        return Ok(PathBuf::from(name));
    }

    anyhow::bail!(
        "Binary '{}' not found. Please add it to PATH or place it in ~/.octopus/bin/.",
        name
    )
}

async fn download_file(url: &str, dest: &Path) -> Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    const MAX_DOWNLOAD_SIZE: u64 = 200 * 1024 * 1024; // 200MB
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP download failed with status: {}", response.status());
    }
    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_SIZE {
            anyhow::bail!("download too large ({}MB > 200MB limit)", len / 1024 / 1024);
        }
    }

    // 写 .part 临时文件，下载完整后原子 rename 到 dest。避免下载中断（断网/超时/中止）
    // 残留半成品：若直接写 dest，残留文件会被 get_binary_path 的 exists() 误判为已就绪、
    // 跳过重新下载，导致后续永远执行损坏 binary。.part 残留无害——dest 仍不存在，下次
    // get_binary_path 仍判定缺失并重新下载，create(.part) 会自动 truncate 覆盖残留。
    let part: PathBuf = format!("{}.part", dest.to_string_lossy()).into();
    let mut file = fs::File::create(&part).await?;
    let mut stream = response.bytes_stream();
    let mut total: u64 = 0;
    while let Some(item) = stream.next().await {
        let chunk = item.context("Error while downloading chunk")?;
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_SIZE {
            anyhow::bail!("download exceeded size limit");
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file); // 关闭句柄再 rename（Windows 要求文件未被占用）
    fs::rename(&part, dest).await?;
    Ok(())
}

async fn prepare_dependencies() -> Result<()> {
    let bin_dir = octopus_infra::octopus_config_home().join("bin");
    fs::create_dir_all(&bin_dir).await?;

    // 1. 检查并自动下载 yt-dlp
    if get_binary_path("yt-dlp").await.is_err() {
        println!("yt-dlp not found. Downloading latest yt-dlp binary...");
        let url = if cfg!(target_os = "windows") {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        } else {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"
        };

        let dest_name = if cfg!(target_os = "windows") { "yt-dlp.exe" } else { "yt-dlp" };
        let dest_path = bin_dir.join(dest_name);

        download_file(url, &dest_path).await.context("Failed to download yt-dlp")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dest_path).await?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest_path, perms).await?;
        }
        println!("yt-dlp downloaded successfully and made executable.");
    }

    // 2. 检查 ffmpeg
    if get_binary_path("ffmpeg").await.is_err() {
        let install_msg = if cfg!(target_os = "macos") {
            "ffmpeg not found! Please install ffmpeg using Homebrew: `brew install ffmpeg`"
        } else if cfg!(target_os = "windows") {
            "ffmpeg not found! Please download ffmpeg from https://ffmpeg.org/download.html and add it to your system PATH."
        } else {
            "ffmpeg not found! Please install ffmpeg via your system package manager (e.g. `sudo apt install ffmpeg`)."
        };
        anyhow::bail!("{}", install_msg);
    }

    Ok(())
}

struct Args {
    url: String,
    output: Option<PathBuf>,
    unclear: bool,
}

fn parse_args() -> Result<Args> {
    let args: Vec<String> = std::env::args().collect();
    let mut url = None;
    let mut output = None;
    let mut unclear = false;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-o" || arg == "--output" {
            if i + 1 < args.len() && !args[i + 1].starts_with("-") {
                output = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else {
                output = Some(PathBuf::new());
                i += 1;
            }
        } else if arg == "--unclear" {
            unclear = true;
            i += 1;
        } else if arg.starts_with("-") {
            anyhow::bail!("Error: unknown option {}", arg);
        } else {
            if url.is_some() {
                anyhow::bail!("Error: multiple URLs specified");
            }
            url = Some(arg.clone());
            i += 1;
        }
    }

    let url = url.ok_or_else(|| anyhow!("Usage: octopus-dlp <URL> [-o/--output <FILE>] [--unclear]"))?;
    Ok(Args { url, output, unclear })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let url = &args.url;

    // 准备依赖项
    if let Err(e) = prepare_dependencies().await {
        eprintln!("Dependency error: {}", e);
        std::process::exit(1);
    }

    let yt_dlp = get_binary_path("yt-dlp").await?;
    let ffmpeg = get_binary_path("ffmpeg").await?;

    let work_dir = octopus_infra::octopus_config_home().join("tmp");
    fs::create_dir_all(&work_dir).await?;

    // 计算 URL 的 MD5 以获得唯一的缓存文件名
    let url_md5 = format!("{:x}", md5::compute(url));
    let output_template = work_dir.join(format!("{}.%(ext)s", url_md5));
    let output_template_str = output_template.to_string_lossy().to_string();

    let mut final_output = args.output;
    if let Some(ref path) = final_output {
        if path.as_os_str().is_empty() {
            final_output = Some(work_dir.join(format!("{}.wav", url_md5)));
        }
    }

    // 1. 获取元数据 JSON
    println!("Retrieving video metadata...");
    let info_output = Command::new(&yt_dlp)
        .arg("--dump-json")
        .arg("-f")
        .arg("ba/b")
        .arg("-o")
        .arg(&output_template_str)
        .arg(url)
        .output()
        .await?;

    if !info_output.status.success() {
        let err = String::from_utf8_lossy(&info_output.stderr);
        println!("Failed to get video info from yt-dlp: {}", err);
        std::process::exit(1);
    }

    let metadata: YtdlpMetadata = serde_json::from_slice(&info_output.stdout)
        .context("Failed to parse yt-dlp metadata JSON")?;

    let downloaded_file = metadata._filename
        .ok_or_else(|| anyhow!("yt-dlp metadata does not contain output filename"))?;
    let downloaded_filepath = PathBuf::from(downloaded_file);

    // 输出包含标题、时长等元数据的 JSON 作为 stderr 的首行，以便主进程读取
    let output_meta = VideoMetadataOutput {
        title: metadata.title.unwrap_or_else(|| "Unknown Title".to_string()),
        duration: metadata.duration.unwrap_or(0.0),
        author: metadata.uploader.unwrap_or_else(|| "Unknown Author".to_string()),
    };
    let meta_json = serde_json::to_string(&output_meta)?;
    eprintln!("{}", meta_json); // 输出到 stderr，防止干扰 stdout 中的 pcm 采样数据

    // 2. 执行音频下载（如果开启了 --unclear 且文件已存在，则跳过下载）
    let cache_exists = downloaded_filepath.exists();
    if cache_exists && args.unclear {
        println!("Cached video file found at: {:?}", downloaded_filepath);
        println!("Skipping download (--unclear is enabled).");
    } else {
        println!("Downloading audio track...");
        let download_status = Command::new(&yt_dlp)
            .arg("-f")
            .arg("ba/b")
            .arg("-o")
            .arg(&output_template_str)
            .arg(url)
            .status()
            .await?;

        if !download_status.success() {
            eprintln!("Failed to download media using yt-dlp.");
            std::process::exit(1);
        }
    }

    if !downloaded_filepath.exists() {
        eprintln!("Downloaded file does not exist at: {:?}", downloaded_filepath);
        std::process::exit(1);
    }

    // 清理守卫：覆盖其后所有退出路径（ffmpeg spawn/wait `?` 提前返回 + 正常完成 + exit(1)），
    // drop 时按 --unclear 决定删除或保留。替代原手动清理块，消除早返回泄漏。
    let _cleanup_guard = DownloadedFileGuard {
        path: downloaded_filepath.clone(),
        keep: args.unclear,
    };

    // 3. 转码分离音频并输出
    let mut ffmpeg_cmd = Command::new(&ffmpeg);
    ffmpeg_cmd.arg("-y").arg("-i").arg(&downloaded_filepath);

    if let Some(ref path) = final_output {
        eprintln!("Separating audio and saving WAV to file: {:?}", path);
        ffmpeg_cmd
            .arg("-f").arg("wav") // 强制输出标准 WAV 格式（包含 44 字节文件头，可直接播放）
            .arg("-ar").arg("16000")
            .arg("-ac").arg("1")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    } else {
        eprintln!("Separating audio and streaming raw f32le PCM to stdout...");
        ffmpeg_cmd
            .arg("-f").arg("f32le")
            .arg("-ar").arg("16000")
            .arg("-ac").arg("1")
            .arg("-c:a").arg("pcm_f32le")
            .arg("-")
            .stdout(Stdio::inherit())
            .stderr(Stdio::null());
    }

    let mut ffmpeg_child = ffmpeg_cmd.spawn()?;
    let ffmpeg_status = ffmpeg_child.wait().await?;
    // 临时文件清理由 _cleanup_guard 在函数退出（含上方 ? 提前返回）时统一处理

    if !ffmpeg_status.success() {
        eprintln!("ffmpeg execution failed during transcoding.");
        std::process::exit(1);
    }

    eprintln!("Audio extraction completed successfully.");
    Ok(())
}
