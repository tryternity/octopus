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
}

/// 进度阶段（emit 给前端，外层 kebab + 变体 camelCase）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },
    Recognizing { percent: u32 },
    Finalizing { percent: u32 },
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

pub type SubtitleResult2<T> = std::result::Result<T, SubtitleError>;

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
            deleted_at: None,
            subtitle_cues: None, subtitle_srt: None, subtitle_model: None,
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
}
