//! 录屏后处理命令子模块（macOS 独占）。
//!
//! 从 record_commands/mod.rs 拆出（Task 1.3）。包含：
//! - ffmpeg helpers（probe/find/hint/check）
//! - export_gif（MP4 → GIF）
//! - MergeResult + RecordTaskEvent + merge_audio_tracks（双轨合并单轨）
//! - generate_subtitle / generate_subtitle_inner（ASR 字幕生成）
//! - read_subtitle / reveal_subtitle（字幕文件读写）
//! - LlmOption + list_subtitle_llms + capitalize（LLM 下拉）

#![cfg(target_os = "macos")]

use octopus_record::{RecordError, RecordStore, RecordingMeta};
use tauri::{command, AppHandle, Emitter, State};

use crate::core::error_util::e2s;
use super::{with_db_blocking, now_iso};

/// 同步探测 ffmpeg 是否存在（不报错，返回 Option）。
///
/// 查找顺序：`~/.octopus/bin/ffmpeg`（用户手动放/dlp 下载缓存）→ 系统 PATH（which）。
/// 复制自 `crates/dlp/src/main.rs:42-73` 的 `get_binary_path`（dlp 是 binary crate 无 lib，
/// 无法 use 导入；desktop crate 已有 4 处 which 内联副本，容忍此模式）。
///
/// `pub(crate)`：被同 crate 的 `record_audio_probe::write_audio_tracks_metadata` 调用
/// （Task 2.3 在 stop_and_store_inner 入库成功后写 mp4 udta metadata）。
pub(crate) fn probe_ffmpeg() -> Option<std::path::PathBuf> {
    // 1. ~/.octopus/bin/ffmpeg
    let home_bin = octopus_infra::octopus_config_home().join("bin").join("ffmpeg");
    if home_bin.exists() {
        return Some(home_bin);
    }
    // 2. 系统 PATH（which，与 agent_adapter.rs / paste.rs 同模式）
    let on_path = std::process::Command::new("which")
        .arg("ffmpeg")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if on_path {
        return Some(std::path::PathBuf::from("ffmpeg"));
    }
    None
}

/// 查找 ffmpeg 二进制路径（报错版本，给 export_gif 用）。
///
/// 未找到时返回错误，文案引导多种安装方式（brew + bash 下载 + 手动）。
/// 前端 GIF 按钮 disabled 时 tooltip 也用同一文案（通过 check_ffmpeg 命令拿）。
async fn find_ffmpeg() -> Result<std::path::PathBuf, String> {
    if let Some(p) = probe_ffmpeg() {
        return Ok(p);
    }
    Err(ffmpeg_missing_hint())
}

/// ffmpeg 缺失时的安装引导文案（多种方式）。
///
/// 不只 brew——用户可能没装 brew，提供 bash 直接下载静态二进制 + 手动放置两种。
/// 文案是多语言 key 的 fallback（i18n 加载前/英文化场景），前端通过 check_ffmpeg
/// 命令拿 bool 后用自己的 i18n key 渲染 tooltip。
fn ffmpeg_missing_hint() -> String {
    "ffmpeg 未找到。安装方式：\n\
     1. brew install ffmpeg\n\
     2. curl -L https://evermeet.cx/ffmpeg/getrelease/zip -o /tmp/ffmpeg.zip && unzip /tmp/ffmpeg.zip -d ~/.octopus/bin/\n\
     3. 手动下载放到 ~/.octopus/bin/ffmpeg"
        .into()
}

/// 探测 ffmpeg 是否可用（前端 GIF 按钮据此决定是否灰禁）。
///
/// 返回 bool：true=可用，false=未找到（前端显示 tooltip 引导安装）。
#[command]
pub async fn check_ffmpeg() -> bool {
    probe_ffmpeg().is_some()
}
/// 把已录制的 MP4 转成 GIF。
///
/// 输出位置：源 MP4 同目录、同名换 `.gif`（`-y` 覆盖）。
/// ffmpeg 参数（spec §2.20）：`fps=15,scale=800:-1:flags=lanczos -loop 0`。
///
/// 进度反馈：emit `record://task` + `RecordTaskEvent::GifStarted { id }` → ffmpeg 跑完 →
/// 成功 emit `RecordTaskEvent::GifDone { id, path }` / 失败 emit `RecordTaskEvent::GifFailed { id, error }`。
/// 前端 invoke 返回值也能拿到路径/错误，事件作为多窗口同步备用。
#[command]
pub async fn export_gif(app: AppHandle, id: i64) -> Result<String, String> {
    use octopus_infra::paths::resolve_recording_path;

    // 1. 查 DB 拿 file_path
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let input = resolve_recording_path(&file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }

    // 2. 解析 ffmpeg
    let ffmpeg = find_ffmpeg().await?;

    // 3. 输出路径：源文件同目录、同名换 .gif
    let output = input.with_extension("gif");

    // 4. emit 起点
    let _ = app.emit("record://task", RecordTaskEvent::GifStarted { id });

    // 5. spawn ffmpeg
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(&input)
        .arg("-vf").arg("fps=15,scale=800:-1:flags=lanczos")
        .arg("-loop").arg("0")
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        let _ = app.emit(
            "record://task",
            RecordTaskEvent::GifFailed { id, error: "ffmpeg 转 GIF 失败".into() },
        );
        return Err("ffmpeg 转 GIF 失败（退出码非 0）。可能原因：源文件损坏 / ffmpeg 版本过旧".into());
    }

    let path_str = output.to_string_lossy().to_string();
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::GifDone { id, path: path_str.clone() },
    );
    Ok(path_str)
}

/// `merge_audio_tracks` 的返回值——新 recording 的 id 与文件绝对路径。
///
/// 前端拿到后跳详情 / reveal in Finder；事件 `record://task` +
/// `RecordTaskEvent::MergeDone` 作为多窗口同步备用。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub new_id: i64,
    pub file_path: String,
}

/// 录屏异步任务（GIF 导出 / 音轨合并）的进度事件。
///
/// 统一替代原 `record://gif-{started,done,failed}` + `record://merge-{started,done,failed}`
/// 6 个事件名。前端未来如需监听，单个 `listen("record://task", ...)` + `switch(payload.event)` 即可。
///
/// 与 `HelperEvent`（`record://event`）同模式：内部 tagged enum。
/// 变体名 kebab-case（外层 `rename_all`）+ 字段 camelCase（变体级 `rename_all`）——
/// 遵循 `AGENTS.md`「序列化 casing 规范」。
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RecordTaskEvent {
    #[serde(rename_all = "camelCase")]
    GifStarted {
        id: i64,
    },
    #[serde(rename_all = "camelCase")]
    GifDone {
        id: i64,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    GifFailed {
        id: i64,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    MergeStarted {
        id: i64,
    },
    /// `new_id` 指向合并后新生成的 recording 记录 id（序列化为 `newId`）。
    #[serde(rename_all = "camelCase")]
    MergeDone {
        id: i64,
        new_id: i64,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    MergeFailed {
        id: i64,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleStarted {
        id: i64,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleProgress {
        id: i64,
        stage: octopus_record::SubtitleProgress,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleDone {
        id: i64,
        cue_count: usize,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleFailed {
        id: i64,
        error: String,
    },
}

/// 把双轨录屏 mp4（mic + system）用 ffmpeg `amix` 合并成单轨，另存为新文件 + INSERT 新 DB 记录。
///
/// **amix（非 amerge）**：spike 发现 mic 常为 mono、system 为 stereo，`amerge` 要求两输入
/// 声道数相同会失败；`amix` 自动处理声道差异（mono 自动 upmix 到 stereo 混音），更稳健。
///
/// 流程（仿 `export_gif:863` 模式）：
/// 1. 查 DB 拿原 recording meta（`with_db_blocking` + `RecordStore::get`）
/// 2. 校验 `audio_tracks.len() >= 2`（非多音轨直接报错，不浪费 ffmpeg 调用）
/// 3. `find_ffmpeg` + `merged_output_path` 算输出路径
/// 4. emit `record://task` + `RecordTaskEvent::MergeStarted { id }` → spawn ffmpeg → 成功 emit done / 失败 emit failed
/// 5. ffmpeg 参数：`-filter_complex [0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]`
///    + `-map 0:v -map [a] -c:v copy -c:a aac -b:a 192k`（视频流拷贝不重编码，音频重编码 AAC）
/// 6. 失败删 merged.mp4（避免半残文件占空间 + DB 不入库）
/// 7. ffprobe 探测 merged 文件音轨（应单轨，source=Merged）；ffprobe 不可用兜底构造一个 Merged track
/// 8. 写 mp4 metadata（失败仅 warn，不阻断——DB 已有 audio_tracks 兜底）
/// 9. INSERT 新 recording 记录（file_path = merged.mp4 绝对路径，title 加 `(merged)` 后缀，新 id）
///
/// **失败删 merged.mp4**：与 export_gif 不同（gif 失败也删 gif 但 export_gif 没写——本命令显式删，
/// 因 merged 文件体积大且与源同目录，半残文件会混淆用户）。
///
/// **stderr piped 但暂不用**：与 export_gif 一致（export_gif 用 `Stdio::null()`）。Phase 5 e2e
/// 如需诊断 ffmpeg 错误细节，再改成 piped + 读 stderr 进 error 文案。当前先 null 保持简洁。
///
/// **new_id**：`chrono::Utc::now().timestamp_millis()`——与 `start_with_config:305` 的
/// `recording_id` 同体系（chrono 毫秒戳），desktop crate 已有 chrono 依赖（Cargo.toml）。
#[command]
pub async fn merge_audio_tracks(app: AppHandle, id: i64) -> Result<MergeResult, String> {
    use octopus_infra::paths::resolve_recording_path;
    use octopus_record::audio_tracks::{AudioTrack, AudioTrackSource};

    // 1. 查 DB 拿原 recording meta（连同 file_path 一起，省去第二次查 DB）
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)?.ok_or(RecordError::NotFound(id))
    })
    .await?;

    // 2. 校验：非多音轨直接报错（不浪费 ffmpeg 调用）
    if meta.audio_tracks.len() < 2 {
        return Err("不是多音轨录屏，无需合并".into());
    }

    let input = resolve_recording_path(&meta.file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }

    // 3. ffmpeg + 输出路径
    let ffmpeg = find_ffmpeg().await?;
    let output = crate::record::record_audio_probe::merged_output_path(&input);

    // 4. emit 起点（前端切 loading 态）
    let _ = app.emit("record://task", RecordTaskEvent::MergeStarted { id });

    // 5. spawn ffmpeg amix
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(&input)
        .arg("-filter_complex")
        .arg("[0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]")
        .arg("-map")
        .arg("0:v")
        .arg("-map")
        .arg("[a]")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        // 6. 失败删 merged.mp4（避免半残文件占空间）
        let _ = std::fs::remove_file(&output);
        let _ = app.emit(
            "record://task",
            RecordTaskEvent::MergeFailed { id, error: "ffmpeg amix 失败".into() },
        );
        return Err("ffmpeg amix 失败（退出码非 0）".into());
    }

    // 7. 探测 merged 文件音轨（应单轨）。ffprobe 不可用 / 解析失败 → 兜底构造一个 Merged track。
    let merged_tracks: Vec<AudioTrack> =
        match crate::record::record_audio_probe::probe_ffprobe() {
            Some(ffprobe) => {
                match crate::record::record_audio_probe::probe_audio_tracks(&ffprobe, &output).await {
                    Ok(raw) if !raw.is_empty() => vec![AudioTrack {
                        index: 0,
                        source: AudioTrackSource::Merged,
                        codec: raw[0].codec.clone(),
                        sample_rate: raw[0].sample_rate,
                        channels: raw[0].channels,
                        device_name: None,
                    }],
                    _ => vec![AudioTrack {
                        index: 0,
                        source: AudioTrackSource::Merged,
                        codec: "aac".into(),
                        sample_rate: 48000,
                        channels: 2,
                        device_name: None,
                    }],
                }
            }
            None => vec![AudioTrack {
                index: 0,
                source: AudioTrackSource::Merged,
                codec: "aac".into(),
                sample_rate: 48000,
                channels: 2,
                device_name: None,
            }],
        };

    // 8. 写 mp4 metadata（失败不阻断——DB 已有 audio_tracks 兜底）
    if let Err(e) = crate::record::record_audio_probe::write_audio_tracks_metadata(
        &ffmpeg,
        &output,
        &merged_tracks,
    )
    .await
    {
        log::warn!("[record] merged mp4 metadata 写入失败: {e}");
    }

    // 9. INSERT 新 recording 记录
    let file_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let new_id = chrono::Utc::now().timestamp_millis();
    let file_path_str = output.to_string_lossy().to_string();
    let title = if meta.title.is_empty() {
        "merged".to_string()
    } else {
        format!("{} (merged)", meta.title)
    };
    let new_meta = RecordingMeta {
        id: new_id,
        file_path: file_path_str.clone(),
        title,
        duration_ms: meta.duration_ms,
        width: meta.width,
        height: meta.height,
        fps: meta.fps,
        codec: meta.codec.clone(),
        has_system_audio: true,
        has_microphone: true,
        audio_tracks: merged_tracks,
        source_type: meta.source_type.clone(),
        file_size,
        has_thumbnail: false,
        is_favorite: false,
        created_at: now_iso(),
    };

    with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.insert(&new_meta, None)
    })
    .await?;

    let _ = app.emit(
        "record://task",
        RecordTaskEvent::MergeDone { id, new_id, path: file_path_str.clone() },
    );

    Ok(MergeResult {
        new_id,
        file_path: file_path_str,
    })
}

// ── 字幕生成（P3）──────────────────────────────────────────────

/// 生成字幕：选轨 → ffmpeg 抽 16k mono PCM → ASR 带时间戳转写 → cue + SRT → UPDATE DB。
///
/// **编排**（与 `merge_audio_tracks` 同模式，但 ASR 段为同步阻塞）：
/// 1. emit `SubtitleStarted { id }`
/// 2. 查 DB 拿 `RecordingMeta`
/// 3. `select_track` 按 preference 选轨（Auto/Microphone/System）
/// 4. emit `SubtitleProgress::ExtractingAudio { 10 }`
/// 5. **`spawn_blocking`** 包 `extract_audio_track_to_pcm`（ffmpeg 跑数秒，避免阻塞 tokio runtime）
/// 6. emit `SubtitleProgress::Recognizing { 40 }`
/// 7. `engine_manager.active_engine()` 拿 `Arc<dyn OfflineAsrEngine>`
/// 8. `transcribe_segments_with_timestamps` → `Vec<TimestampedSegment>`
/// 8.5. （可选）`polish=Some` 时 emit `Polishing { 50 }` + LLM 整段润色 + 文本回填
/// 9. emit `SubtitleProgress::Finalizing { 90 }`
/// 10. 转 `SubtitleCue` + `generate_srt` + `SubtitleResult`
/// 11. UPDATE DB（cues 序列化为 JSON 字符串）
/// 12. emit `SubtitleDone { id, cue_count }`
///
/// **错误降级**：任一步失败 → emit `SubtitleFailed { id, error }` + 返回 Err。
/// VAD 分段完全为空（无声）→ cues 为空 + srt_text=""（不报错，前端显示「无字幕」）。
///
/// **engine_manager 参数**：Tauri 2 要求 `State` 在前。`Arc<AsrEngineManager>` 在
/// `main.rs:1051` 通过 `app.manage` 注入（与 `runtime_config.rs:348` 同模式）。
///
/// **model 名**：用 `octopus_asr_local::config::resolve_active_engine("asr")`（与
/// `main.rs:980` 启动预热同模式），返回 `Result<ResolvedEngine>`，取 `.name`。
#[command]
pub async fn generate_subtitle(
    app: AppHandle,
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::record::subtitle_polish::PolishOption>,
) -> Result<octopus_record::SubtitleResult, String> {
    // 内层 runner：返回 Result，外层 catch 后 emit SubtitleFailed。
    // 这样所有失败路径统一走一处 emit，避免漏发。
    match generate_subtitle_inner(&app, &engine_manager, id, track, polish).await {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = app.emit(
                "record://task",
                RecordTaskEvent::SubtitleFailed { id, error: e.clone() },
            );
            Err(e)
        }
    }
}

/// `generate_subtitle` 的实际编排（不带错误 emit，由外层统一处理）。
async fn generate_subtitle_inner(
    app: &AppHandle,
    engine_manager: &std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::record::subtitle_polish::PolishOption>,
) -> Result<octopus_record::SubtitleResult, String> {
    use octopus_infra::paths::resolve_recording_path;

    log::info!("[subtitle] generate_subtitle_inner 开始 id={}", id);
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleStarted { id });

    // 1. 查 DB 拿 RecordingMeta（连同 file_path 一起，省去第二次查 DB）
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)?.ok_or(RecordError::NotFound(id))
    })
    .await?;
    log::info!("[subtitle] step1 查 DB 完成 file_path={} audio_tracks={}",
        meta.file_path, meta.audio_tracks.len());

    // 2. 解析 TrackPreference
    let pref = match track.as_deref() {
        Some("system") => octopus_record::TrackPreference::System,
        Some("microphone") => octopus_record::TrackPreference::Microphone,
        _ => octopus_record::TrackPreference::Auto,
    };

    // 3. 选轨
    let (track_idx, track_used) =
        octopus_record::select_track(&meta, pref).map_err(e2s)?;
    log::info!("[subtitle] step3 选轨完成 track_idx={} track_used={:?}", track_idx, track_used);

    // 4. emit 进度：抽音轨
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::ExtractingAudio { percent: 10 },
        },
    );

    // 5. ffmpeg 抽 PCM（spawn_blocking 包裹——ffmpeg 跑数秒，避免阻塞 tokio runtime）
    let ffmpeg = find_ffmpeg().await?;
    let input = resolve_recording_path(&meta.file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }
    log::info!("[subtitle] step5 开始抽 PCM mp4={} ffmpeg={}", input.display(), ffmpeg.display());
    let mp4_path_clone = input.clone();
    let pcm = tokio::task::spawn_blocking(move || {
        octopus_record::extract_audio_track_to_pcm(&mp4_path_clone, track_idx, &ffmpeg)
    })
    .await
    .map_err(|e| format!("extract join error: {e}"))?
    .map_err(e2s)?;
    log::info!("[subtitle] step5 抽 PCM 完成 samples={} ({:.1}s)", pcm.len(), pcm.len() as f64 / 16000.0);

    // 6. emit 进度：识别中
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::Recognizing { percent: 40 },
        },
    );

    // 7. 拿 active engine（State 注入的 AsrEngineManager）+ PipelineConfig
    let engine = engine_manager
        .active_engine()
        .map_err(|e| format!("获取 ASR 引擎失败: {e}"))?;
    log::info!("[subtitle] step7 拿到 active engine");
    let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config("zh");

    // 8. ASR 带时间戳转写（同步阻塞 CPU 密集——但 engine.transcribe 内部已并发，
    //    且通常总耗时 < ffmpeg；为简化暂不再 spawn_blocking，如发现卡 UI 再包）。
    log::info!("[subtitle] step8 开始 ASR transcribe（这一步可能耗时几十秒）...");
    let mut timestamped =
        octopus_asr_local::pipeline::transcribe_segments_with_timestamps(engine.as_ref(), &pcm, &cfg)
            .map_err(|e| format!("ASR 失败: {e}"))?;
    log::info!("[subtitle] step8 ASR 完成 segments={}", timestamped.len());

    // 8.5. LLM 润色（可选，polish=Some 时触发）
    //     整段润色（保留 [[N]] 标记边界）→ 拆回 cue → 文本回填 timestamped（时间戳不变）。
    //     失败降级（NoLlmConfig/Failed/FallbackRatio）走 polish_subtitle_cues 内部，
    //     不会让整条字幕流程报错——润色失败 = 用 ASR 原文本 + 提示用户。
    let polish_outcome_str: Option<String> = if let Some(polish_opt) = polish.as_ref() {
        let _ = app.emit(
            "record://task",
            RecordTaskEvent::SubtitleProgress {
                id,
                stage: octopus_record::SubtitleProgress::Polishing { percent: 50 },
            },
        );
        let texts: Vec<String> = timestamped.iter().map(|t| t.text.clone()).collect();
        log::info!("[subtitle] step8.5 开始 LLM 润色 cues={}", texts.len());
        let (polished_texts, outcome) =
            crate::record::subtitle_polish::polish_subtitle_cues(texts, polish_opt, app).await;
        // 文本回填（时间戳不变）；长度一定一致（polish_subtitle_cues 保证）
        for (seg, new_text) in timestamped.iter_mut().zip(polished_texts) {
            seg.text = new_text;
        }
        log::info!("[subtitle] step8.5 润色完成 outcome={:?}", outcome);
        let outcome_str = match outcome {
            crate::record::subtitle_polish::PolishOutcome::Skipped => None,
            crate::record::subtitle_polish::PolishOutcome::Polished => Some("polished".into()),
            crate::record::subtitle_polish::PolishOutcome::FallbackRatio => Some("fallbackRatio".into()),
            crate::record::subtitle_polish::PolishOutcome::NoLlmConfig => Some("noLlmConfig".into()),
            crate::record::subtitle_polish::PolishOutcome::Failed(msg) => Some(format!("failed:{msg}")),
        };
        outcome_str
    } else {
        None
    };

    // 9. emit 进度：组装
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::Finalizing { percent: 90 },
        },
    );

    // 10. 转 SubtitleCue + 生成 SRT（文本取自 step8.5 润色后的 timestamped）
    let cues: Vec<octopus_record::SubtitleCue> = timestamped
        .into_iter()
        .map(|t| octopus_record::SubtitleCue {
            start_ms: t.start_ms,
            end_ms: t.end_ms,
            text: t.text,
        })
        .collect();
    let model = octopus_asr_local::config::resolve_active_engine("asr")
        .map(|r| r.name)
        .unwrap_or_else(|_| "unknown".to_string());
    let srt_text = octopus_record::generate_srt(&cues);
    let result = octopus_record::SubtitleResult {
        cues: cues.clone(),
        srt_text: srt_text.clone(),
        model: model.clone(),
        track_used,
        polish_outcome: polish_outcome_str,
    };

    // 11. 写 SRT 文件到磁盘（v2：不存 DB，与 mp4 同目录同名 xxx.N.srt）
    let srt_path = octopus_record::next_srt_path(&input);
    log::info!("[subtitle] step11 写 SRT 文件 {} cues={}", srt_path.display(), cues.len());
    std::fs::write(&srt_path, &srt_text)
        .map_err(|e| format!("写 SRT 文件失败 {}: {e}", srt_path.display()))?;
    log::info!("[subtitle] step11 SRT 文件写入完成");

    // 12. emit done（cues 为空也走 Done——VAD 无声属于「正常无字幕」，不是错误）
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleDone { id, cue_count: cues.len() },
    );
    log::info!("[subtitle] step12 emit Done 完成，函数即将返回");

    Ok(result)
}

/// 导出 SRT 文件到指定路径。
///
/// 读取最新字幕（v2：从磁盘 .srt 文件解析，不查 DB）。
///
/// 扫描 mp4 同目录的 `<stem>.N.srt`，取 N 最大的（最新版本）→ 解析为 SubtitleResult。
/// 不存在 → None（前端显示「生成字幕」按钮）。
/// `track_used` 从 audio_tracks 第一条 source 推（仅前端 fallback 提示用）。
#[command]
pub async fn read_subtitle(
    id: i64,
) -> Result<Option<octopus_record::SubtitleResult>, String> {
    // 查 DB 拿 file_path + audio_tracks（用于解析 mp4 路径 + track_used 推断）
    let meta_opt: Option<octopus_record::RecordingMeta> = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)
    })
    .await?;
    let meta = match meta_opt {
        Some(m) => m,
        None => return Ok(None),
    };
    let mp4 = octopus_infra::paths::resolve_recording_path(&meta.file_path);
    let srt_path = match octopus_record::latest_srt_path(&mp4) {
        Some(p) => p,
        None => return Ok(None),
    };
    let srt_text = std::fs::read_to_string(&srt_path)
        .map_err(|e| format!("读 SRT 文件失败 {}: {e}", srt_path.display()))?;
    let cues = octopus_record::parse_srt(&srt_text);
    let track_used = meta
        .audio_tracks
        .first()
        .map(|t| t.source)
        .unwrap_or(octopus_record::AudioTrackSource::Unknown);
    // model 字段用 srt 文件名占位（v2 不再持久化 model 名——序号即版本）
    let model = srt_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Some(octopus_record::SubtitleResult {
        cues,
        srt_text,
        model,
        track_used,
        polish_outcome: None,
    }))
}

/// 在 Finder 显示录屏对应的最新 SRT 文件（v2：替代 export_subtitle）。
///
/// 找最新 .srt 文件，在文件管理器定位。不存在 → Err。
#[command]
pub async fn reveal_subtitle(id: i64) -> Result<String, String> {
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let mp4 = octopus_infra::paths::resolve_recording_path(&meta);
    let srt_path = octopus_record::latest_srt_path(&mp4)
        .ok_or_else(|| "字幕未生成".to_string())?;
    crate::platform::sys_open::reveal_path(&srt_path)?;
    Ok(srt_path.to_string_lossy().to_string())
}

// ── 字幕 LLM 润色：列出可用 LLM（弹框下拉填充）──────────────────

/// LLM 下拉选项（前端弹「润色」对话框时填 select）。
///
/// - `key`：`{provider}:{model_name}`（如 `openai:gpt-4o`），传回 `generate_subtitle` 的
///   `polish.llmKey` 字段（subtitle_polish 暂按默认 LLM 走，key 仅记录用户选择）。
/// - `label`：`{model_name} ({Provider})`（如 `GPT-4o (Openai)`），provider 首字母大写。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmOption {
    pub key: String,
    pub label: String,
}

/// 列出可用 LLM（弹框下拉填充）。
///
/// 查 `models` 表 `domain='llm' AND is_available=1`（文件就绪/配置完整即列出，
/// 不要求 is_enabled——用户可选用任一已配置的 LLM 润色）。
/// 按 `model_name` 字母序排序，方便前端展示。
#[command]
pub async fn list_subtitle_llms() -> Result<Vec<LlmOption>, String> {
    with_db_blocking(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT provider, model_name FROM models WHERE domain='llm' AND is_available=1 ORDER BY model_name",
        )?;
        let rows = stmt.query_map([], |r| {
            let provider: String = r.get(0)?;
            let model_name: String = r.get(1)?;
            let key = format!("{}:{}", provider, model_name);
            let label = format!("{} ({})", model_name, capitalize(&provider));
            Ok(LlmOption { key, label })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    })
    .await
}

/// 首字母大写（用于 LLM label 的 provider 显示：`openai` → `Openai`）。
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

