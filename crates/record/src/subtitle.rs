//! 录屏自动字幕数据模型 + SRT 生成 + 选轨逻辑（纯逻辑，无 ASR 依赖）。
//!
//! 设计详见 `docs/superpowers/specs/2026-07-28-record-auto-subtitle-design.md`。
//! ASR 调用由 desktop 编排层桥接（方案 B）——本模块只负责 mp4→PCM、cue 模型、SRT 格式化、选轨。

use crate::audio_tracks::AudioTrackSource;
use std::path::Path;
use thiserror::Error;

/// 一条字幕 cue（跨 Tauri 边界的 DTO，camelCase 序列化）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// 字幕生成结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleResult {
    pub cues: Vec<SubtitleCue>,
    pub srt_text: String,
    pub model: String,
    pub track_used: AudioTrackSource,
    /// 润色结果（None=未尝试润色）。前端据此显示提示。
    ///
    /// 用 `Option<String>` 而非强类型 `PolishOutcome`——后者定义在 desktop crate，
    /// record crate 不能依赖 desktop。desktop 序列化时把 `PolishOutcome` 转成字符串
    /// （如 `"polished"` / `"fallbackRatio"` / `"noLlmConfig"` / `"failed:msg"`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish_outcome: Option<String>,
}

/// 进度阶段（emit 给前端，外层 kebab + 变体 camelCase）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },    // 0~30%
    Recognizing { percent: u32 },        // 30~40%
    Polishing { percent: u32 },          // 40~90%（LLM 润色，可选）
    Finalizing { percent: u32 },         // 90~100%
    Done { cue_count: usize },
    Error { message: String },
}

/// 选轨偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPreference {
    Auto,
    Microphone,
    System,
}

#[derive(Debug, Error)]
pub enum SubtitleError {
    #[error("录制无任何音轨")]
    NoAudioTrack,
    #[error("无 system 音轨")]
    NoSystemTrack,
    #[error("ffmpeg 调用失败: {0}")]
    Ffmpeg(String),
    #[error("ffmpeg 输出解码失败: {0}")]
    Decode(String),
}

/// 把 SubtitleCue 列表格式化为标准 SRT 文本。
///
/// 格式：序号从 1 开始；时间 `HH:MM:SS,mmm`（毫秒用逗号）；cue 间空行分隔；末尾保留换行。
/// 空 cues 返回空字符串。
pub fn generate_srt(cues: &[SubtitleCue]) -> String {
    if cues.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (idx, c) in cues.iter().enumerate() {
        out.push_str(&format!("{}\n", idx + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            format_srt_timestamp(c.start_ms),
            format_srt_timestamp(c.end_ms)
        ));
        out.push_str(&c.text);
        out.push('\n');
        if idx < cues.len() - 1 {
            out.push('\n');
        }
    }
    out
}

/// ms → "HH:MM:SS,mmm"（SRT 标准用逗号分隔毫秒）。
fn format_srt_timestamp(ms: u64) -> String {
    let total_sec = ms / 1000;
    let millis = ms % 1000;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, millis)
}

/// 按 preference 选音轨，返回 (track_index, track_source)。
///
/// - Auto / Microphone：优先 mic；mic 不存在则 fallback system，再 fallback 第一条（Merged/Unknown）。
/// - System：强制 system，不存在则 `NoSystemTrack`。
/// - 空 audio_tracks：`NoAudioTrack`。
pub fn select_track(
    meta: &crate::store::RecordingMeta,
    pref: TrackPreference,
) -> Result<(usize, AudioTrackSource), SubtitleError> {
    if meta.audio_tracks.is_empty() {
        return Err(SubtitleError::NoAudioTrack);
    }
    match pref {
        TrackPreference::Auto | TrackPreference::Microphone => {
            // 优先 mic
            if let Some(t) = meta
                .audio_tracks
                .iter()
                .find(|t| t.source == AudioTrackSource::Microphone)
            {
                return Ok((t.index as usize, AudioTrackSource::Microphone));
            }
            // fallback system
            if let Some(t) = meta
                .audio_tracks
                .iter()
                .find(|t| t.source == AudioTrackSource::System)
            {
                return Ok((t.index as usize, AudioTrackSource::System));
            }
            // 再 fallback 第一条（Merged/Unknown）
            let t = &meta.audio_tracks[0];
            Ok((t.index as usize, t.source))
        }
        TrackPreference::System => meta
            .audio_tracks
            .iter()
            .find(|t| t.source == AudioTrackSource::System)
            .map(|t| (t.index as usize, AudioTrackSource::System))
            .ok_or(SubtitleError::NoSystemTrack),
    }
}

/// 用 ffmpeg 从 mp4 抽取指定音轨为 16k mono f32le PCM。
///
/// ffmpeg 调用形态：`ffmpeg -y -i <mp4> -map 0:a:<idx> -ar 16000 -ac 1 -f f32le pipe:1`
/// 读 stdout → 每 4 字节一个 f32（little-endian）。
///
/// **同步 `std::process::Command`**：ffmpeg 跑几秒，会阻塞调用线程。调用方负责用
/// `tokio::task::spawn_blocking` 包一层避免阻塞 tokio runtime（参考 desktop
/// `generate_subtitle` 命令的实现）。
///
/// 不写单测（依赖外部 ffmpeg + 真实 mp4，归 e2e）。
pub fn extract_audio_track_to_pcm(
    mp4_path: &Path,
    track_index: usize,
    ffmpeg_path: &Path,
) -> Result<Vec<f32>, SubtitleError> {
    let output = std::process::Command::new(ffmpeg_path)
        .arg("-y")
        .arg("-i").arg(mp4_path)
        .arg("-map").arg(format!("0:a:{}", track_index))
        .arg("-ar").arg("16000")
        .arg("-ac").arg("1")
        .arg("-f").arg("f32le")
        .arg("pipe:1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| SubtitleError::Ffmpeg(format!("spawn 失败: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SubtitleError::Ffmpeg(format!(
            "退出码非 0: {}",
            stderr.chars().take(500).collect::<String>()
        )));
    }

    // f32le → Vec<f32>
    let bytes = &output.stdout;
    if bytes.len() % 4 != 0 {
        return Err(SubtitleError::Decode(format!(
            "PCM 字节数 {} 不是 4 的倍数",
            bytes.len()
        )));
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(samples)
}

// ─────────────────────────────────────────────────────────────────────────────
// v2：SRT 文件读写（不存 DB，直接与 mp4 同目录同名）
// ─────────────────────────────────────────────────────────────────────────────

/// 计算下一个 SRT 文件路径（不覆盖已有，递增序号）。
///
/// 命名规则：`<mp4_stem>.<N>.srt`，N 从 1 递增。
/// 扫描 mp4 同目录下已存在的 `<stem>.*.srt`，取最大 N + 1。
/// 不存在任何 .srt 时返回 `<stem>.1.srt`。
///
/// **纯路径计算，不创建文件**——调用方拿到路径后自己写。
pub fn next_srt_path(mp4_path: &Path) -> std::path::PathBuf {
    let dir = mp4_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = mp4_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    // 扫已存在的 <stem>.N.srt，找最大 N
    let mut max_n: u32 = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 匹配 <stem>.<N>.srt
            let prefix = format!("{}.", stem);
            if name.starts_with(&prefix) && name.ends_with(".srt") {
                let mid = &name[prefix.len()..name.len() - 4];
                if let Ok(n) = mid.parse::<u32>() {
                    if n > max_n {
                        max_n = n;
                    }
                }
            }
        }
    }
    dir.join(format!("{}.{}.srt", stem, max_n + 1))
}

/// 找最新的 SRT 文件路径（N 最大的那个）。不存在返回 None。
pub fn latest_srt_path(mp4_path: &Path) -> Option<std::path::PathBuf> {
    let dir = mp4_path.parent()?;
    let stem = mp4_path.file_stem()?.to_str()?;
    let prefix = format!("{}.", stem);
    let mut best: Option<(u32, std::path::PathBuf)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".srt") {
            let mid = &name[prefix.len()..name.len() - 4];
            if let Ok(n) = mid.parse::<u32>() {
                if best.as_ref().map_or(true, |(bn, _)| n > *bn) {
                    best = Some((n, entry.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 解析 SRT 文本为 cue 列表。
///
/// 标准 SRT 格式：
/// ```text
/// 1
/// 00:00:01,234 --> 00:00:03,567
/// 字幕文本（可多行）
///
/// 2
/// ...
/// ```
///
/// 容错：跳过无法解析的 cue（序号/时间戳/文本任一缺失）；空行 tolerated；BOM tolerated。
/// 时间戳同时接受 SRT 标准 `,` 毫秒分隔和 VTT 的 `.`（兼容性）。
pub fn parse_srt(text: &str) -> Vec<SubtitleCue> {
    parse_srt_impl(text)
}

fn parse_srt_impl(text: &str) -> Vec<SubtitleCue> {
    let text = text.trim_start_matches('\u{feff}');
    let mut cues = Vec::new();
    let mut lines_iter = text.lines().peekable();
    while let Some(line) = lines_iter.next() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 跳过序号行（纯数字）
        if line.chars().all(|c| c.is_ascii_digit()) && !line.is_empty() {
            // 下一行应该是时间戳行
            match lines_iter.next() {
                Some(ts_line) => {
                    let ts_line = ts_line.trim();
                    if let Some((start_ms, end_ms)) = parse_srt_timestamp_line(ts_line) {
                        // 收集后续文本行直到空行
                        let mut text_lines = Vec::new();
                        while let Some(&peek) = lines_iter.peek() {
                            if peek.trim().is_empty() {
                                lines_iter.next();
                                break;
                            }
                            text_lines.push(lines_iter.next().unwrap().trim());
                        }
                        if !text_lines.is_empty() {
                            cues.push(SubtitleCue {
                                start_ms,
                                end_ms,
                                text: text_lines.join("\n"),
                            });
                        }
                    }
                    // 时间戳行解析失败 → 跳过这个块（容错）
                }
                None => break,
            }
        }
    }
    cues
}

/// 解析时间戳行 `00:00:01,234 --> 00:00:03,567`（逗号或点分隔毫秒）。
fn parse_srt_timestamp_line(line: &str) -> Option<(u64, u64)> {
    let line = line.trim();
    let parts: Vec<&str> = line.split("-->").collect();
    if parts.len() != 2 {
        return None;
    }
    let start = parse_srt_time(parts[0].trim())?;
    let end = parse_srt_time(parts[1].trim())?;
    Some((start, end))
}

/// 解析单个时间戳 `00:00:01,234` → 毫秒。
fn parse_srt_time(s: &str) -> Option<u64> {
    // HH:MM:SS,mmm 或 HH:MM:SS.mmm
    let s = s.trim();
    let (hms, ms_str) = if let Some(idx) = s.find(|c: char| c == ',' || c == '.') {
        (&s[..idx], &s[idx + 1..])
    } else {
        // 无毫秒部分
        (s, "0")
    };
    let hms_parts: Vec<&str> = hms.split(':').collect();
    if hms_parts.len() != 3 {
        return None;
    }
    let h: u64 = hms_parts[0].parse().ok()?;
    let m: u64 = hms_parts[1].parse().ok()?;
    let s: u64 = hms_parts[2].parse().ok()?;
    let ms: u64 = ms_str.parse().ok()?;
    Some(h * 3600_000 + m * 60_000 + s * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: u64, end: u64, text: &str) -> SubtitleCue {
        SubtitleCue { start_ms: start, end_ms: end, text: text.into() }
    }

    #[test]
    fn generate_srt_basic_3_cues() {
        let cues = vec![
            cue(1234, 3567, "第一句"),
            cue(4000, 6500, "第二句"),
            cue(7000, 9500, "第三句"),
        ];
        let srt = generate_srt(&cues);
        assert_eq!(srt,
            "1\n00:00:01,234 --> 00:00:03,567\n第一句\n\
             \n2\n00:00:04,000 --> 00:00:06,500\n第二句\n\
             \n3\n00:00:07,000 --> 00:00:09,500\n第三句\n");
    }

    #[test]
    fn generate_srt_empty_returns_empty_string() {
        assert_eq!(generate_srt(&[]), "");
    }

    #[test]
    fn generate_srt_single_cue() {
        let srt = generate_srt(&[cue(0, 1500, "单句")]);
        assert_eq!(srt, "1\n00:00:00,000 --> 00:00:01,500\n单句\n");
    }

    #[test]
    fn generate_srt_hour_boundary() {
        // 1 小时 + 234ms = 3601234ms
        let srt = generate_srt(&[cue(3_601_234, 3_602_500, "跨小时")]);
        assert!(srt.contains("01:00:01,234 --> 01:00:02,500"));
    }

    #[test]
    fn subtitle_progress_done_serializes_camel_case() {
        let p = SubtitleProgress::Done { cue_count: 5 };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"cueCount\":5"), "应输出 camelCase cueCount，实际: {}", json);
        assert!(!json.contains("cue_count"), "不应有 snake_case cue_count，实际: {}", json);
    }

    #[test]
    fn subtitle_progress_polishing_serializes_kebab_tag() {
        // 外层 tag="stage" + kebab-case → "polishing"；字段 percent camelCase 无影响。
        let p = SubtitleProgress::Polishing { percent: 50 };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"stage\":\"polishing\""), "应输出 kebab-case polishing，实际: {}", json);
        assert!(json.contains("\"percent\":50"), "应输出 percent，实际: {}", json);
    }

    #[test]
    fn subtitle_result_polish_outcome_none_skipped_in_json() {
        // polish_outcome=None 时序列化应省略字段（skip_serializing_if）。
        let r = SubtitleResult {
            cues: vec![],
            srt_text: String::new(),
            model: "test".into(),
            track_used: AudioTrackSource::Microphone,
            polish_outcome: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("polish_outcome"), "None 应被省略，实际: {}", json);
        assert!(!json.contains("polishOutcome"), "None 应被省略，实际: {}", json);
    }

    #[test]
    fn subtitle_result_polish_outcome_some_serialized() {
        let r = SubtitleResult {
            cues: vec![],
            srt_text: String::new(),
            model: "test".into(),
            track_used: AudioTrackSource::Microphone,
            polish_outcome: Some("polished".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"polishOutcome\":\"polished\""), "应输出 camelCase polishOutcome，实际: {}", json);
    }

    #[test]
    fn subtitle_result_polish_outcome_default_none_on_deserialize() {
        // 旧 JSON（无 polishOutcome 字段）反序列化应得到 None（#[serde(default)]）。
        let old_json = r#"{"cues":[],"srtText":"","model":"x","trackUsed":"microphone"}"#;
        let r: SubtitleResult = serde_json::from_str(old_json).unwrap();
        assert_eq!(r.polish_outcome, None);
    }

    // ── select_track 测试（Task 1.3）──────────────────────────────────────────
    use crate::audio_tracks::AudioTrack;
    use crate::store::RecordingMeta;

    fn track(idx: u32, src: AudioTrackSource) -> AudioTrack {
        AudioTrack { index: idx, source: src, codec: "aac".into(), sample_rate: 48000, channels: 2, device_name: None }
    }

    fn meta_with_tracks(tracks: &[AudioTrack]) -> RecordingMeta {
        RecordingMeta {
            id: 1, file_path: "/tmp/x.mp4".into(), title: "".into(), duration_ms: 1000,
            width: 1920, height: 1080, fps: 30, codec: "h264".into(),
            has_system_audio: !tracks.is_empty(), has_microphone: tracks.iter().any(|t| t.source == AudioTrackSource::Microphone),
            audio_tracks: tracks.to_vec(), source_type: "display".into(), file_size: 100,
            has_thumbnail: false, is_favorite: false, created_at: "2026-07-28T00:00:00Z".into(),
        }
    }

    #[test]
    fn select_track_auto_prefers_microphone() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone), track(1, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::Auto).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::Microphone));
    }

    #[test]
    fn select_track_microphone_explicit() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone)]);
        let (idx, used) = select_track(&m, TrackPreference::Microphone).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::Microphone));
    }

    #[test]
    fn select_track_system_explicit() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone), track(1, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::System).unwrap();
        assert_eq!((idx, used), (1, AudioTrackSource::System));
    }

    #[test]
    fn select_track_auto_fallback_to_system_when_no_mic() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::System)]);
        let (idx, used) = select_track(&m, TrackPreference::Auto).unwrap();
        assert_eq!((idx, used), (0, AudioTrackSource::System));
    }

    #[test]
    fn select_track_empty_returns_no_audio_track_error() {
        let m = meta_with_tracks(&[]);
        assert!(matches!(select_track(&m, TrackPreference::Auto), Err(SubtitleError::NoAudioTrack)));
    }

    #[test]
    fn select_track_system_but_none_returns_no_system_error() {
        let m = meta_with_tracks(&[track(0, AudioTrackSource::Microphone)]);
        assert!(matches!(select_track(&m, TrackPreference::System), Err(SubtitleError::NoSystemTrack)));
    }

    // ── v2: parse_srt / generate_srt roundtrip ──

    #[test]
    fn parse_srt_basic_3_cues() {
        let srt = "1\n00:00:01,234 --> 00:00:03,567\n第一句\n\
                   \n2\n00:00:04,000 --> 00:00:06,500\n第二句\n\
                   \n3\n00:00:07,000 --> 00:00:09,500\n第三句\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].start_ms, 1234);
        assert_eq!(cues[0].end_ms, 3567);
        assert_eq!(cues[0].text, "第一句");
        assert_eq!(cues[2].start_ms, 7000);
    }

    #[test]
    fn parse_srt_empty_returns_empty() {
        assert!(parse_srt("").is_empty());
        assert!(parse_srt("   \n\n  ").is_empty());
    }

    #[test]
    fn parse_srt_accepts_dot_milliseparator() {
        // VTT 风格 `.` 毫秒分隔也应兼容
        let srt = "1\n00:00:01.500 --> 00:00:02.000\n测试\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start_ms, 1500);
        assert_eq!(cues[0].end_ms, 2000);
    }

    #[test]
    fn parse_srt_skips_malformed_block() {
        // 序号行后跟非时间戳行 → 该块跳过，继续解析后续块
        let srt = "1\nnot a timestamp\n\
                   \n2\n00:00:01,000 --> 00:00:02,000\n有效\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "有效");
    }

    #[test]
    fn parse_srt_handles_bom_and_crlf() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\nCRLF\r\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "CRLF");
    }

    #[test]
    fn generate_then_parse_roundtrip() {
        let original = vec![
            cue(0, 1500, "开始"),
            cue(2000, 3500, "中间一句"),
            cue(4000, 6500, "多词"),
            cue(3_601_234, 3_602_500, "跨小时"),
        ];
        let srt = generate_srt(&original);
        let parsed = parse_srt(&srt);
        assert_eq!(parsed, original);
    }
}
