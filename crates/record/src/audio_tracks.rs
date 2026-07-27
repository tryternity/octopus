//! 录屏音轨元数据 + 配置推断。

/// ffprobe 给的原始音轨信息（无 source 判断）。
#[derive(Debug, Clone, PartialEq)]
pub struct RawAudioTrack {
    pub index: u32,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
}

/// 写入 DB / mp4 metadata / 发给前端的音轨描述。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: u32,
    pub source: AudioTrackSource,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioTrackSource {
    Microphone,
    System,
    /// merge_audio_tracks 命令产出的合并单轨
    Merged,
    Unknown,
}

/// 按 helper add 顺序（mic 先 track 0，system 后 track 1）+ 配置推断每轨 source。
///
/// spike 验证 helper 双轨 add 顺序（commit f8bbe8ed）：都开时 microphoneAudioInput
/// 先 add（track 1），systemAudioInput 后 add（track 2）。所以 track index 0/1 →
/// mic/system。只开一边时该轨是 track 0。
pub fn infer_audio_tracks(
    raw: Vec<RawAudioTrack>,
    system_enabled: bool,
    mic_enabled: bool,
    mic_device_name: Option<&str>,
) -> Vec<AudioTrack> {
    raw.into_iter().enumerate().map(|(i, r)| {
        let source = match (i, mic_enabled, system_enabled) {
            (0, true, _) => AudioTrackSource::Microphone,
            (0, false, true) => AudioTrackSource::System,
            (1, true, true) => AudioTrackSource::System,
            _ => AudioTrackSource::Unknown,
        };
        AudioTrack {
            index: r.index,
            source,
            codec: r.codec,
            sample_rate: r.sample_rate,
            channels: r.channels,
            device_name: match source {
                AudioTrackSource::Microphone => mic_device_name.map(String::from),
                _ => None,
            },
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(idx: u32, codec: &str, sr: u32, ch: u32) -> RawAudioTrack {
        RawAudioTrack { index: idx, codec: codec.into(), sample_rate: sr, channels: ch }
    }

    #[test]
    fn infer_only_mic_track0_is_microphone() {
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 1)],
            false,  // system_enabled
            true,   // mic_enabled
            Some("UGREEN MIC"),
        );
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].source, AudioTrackSource::Microphone);
        assert_eq!(tracks[0].device_name.as_deref(), Some("UGREEN MIC"));
    }

    #[test]
    fn infer_only_system_track0_is_system() {
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 2)],
            true, false, None,
        );
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].source, AudioTrackSource::System);
        assert_eq!(tracks[0].device_name, None);
    }

    #[test]
    fn infer_both_mic_first_system_second() {
        // helper add 顺序：mic 先（track 0），system 后（track 1）
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 1), raw(1, "aac", 48000, 2)],
            true, true, Some("MacBook Mic"),
        );
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source, AudioTrackSource::Microphone);
        assert_eq!(tracks[0].device_name.as_deref(), Some("MacBook Mic"));
        assert_eq!(tracks[1].source, AudioTrackSource::System);
        assert_eq!(tracks[1].device_name, None);
    }

    #[test]
    fn infer_empty_when_no_raw() {
        let tracks = infer_audio_tracks(vec![], true, true, None);
        assert!(tracks.is_empty());
    }

    #[test]
    fn infer_unknown_when_config_disagrees_with_actual() {
        // 配置都开但实际只 1 轨（如 mic 权限拒绝降级）→ 超出配置预期的标 unknown
        let tracks = infer_audio_tracks(
            vec![raw(0, "aac", 48000, 2), raw(1, "aac", 48000, 2), raw(2, "aac", 48000, 2)],
            true, true, None,
        );
        assert_eq!(tracks[2].source, AudioTrackSource::Unknown);
    }

    #[test]
    fn audio_track_serde_roundtrip_camel_case() {
        let t = AudioTrack {
            index: 1,
            source: AudioTrackSource::Microphone,
            codec: "aac".into(),
            sample_rate: 48000,
            channels: 2,
            device_name: Some("UGREEN".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        // camelCase + lowercase enum
        assert!(json.contains("\"sampleRate\""));
        assert!(json.contains("\"deviceName\""));
        assert!(json.contains("\"microphone\""));
        let back: AudioTrack = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn audio_track_source_merged_serializes() {
        let s = serde_json::to_string(&AudioTrackSource::Merged).unwrap();
        assert_eq!(s, "\"merged\"");
    }
}
