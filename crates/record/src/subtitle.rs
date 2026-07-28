//! 录屏自动字幕数据模型 + SRT 生成 + 选轨逻辑（纯逻辑，无 ASR 依赖）。
//!
//! 设计详见 `docs/superpowers/specs/2026-07-28-record-auto-subtitle-design.md`。
//! ASR 调用由 desktop 编排层桥接（方案 B）——本模块只负责 mp4→PCM、cue 模型、SRT 格式化、选轨。

use crate::audio_tracks::AudioTrackSource;
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
}
