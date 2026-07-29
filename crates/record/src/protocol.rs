//! Helper 协议的 JSON schema：主进程与 helper 子进程之间的所有数据结构。
//!
//! 协议传输层见 spec §2.1：argv[1]=RecordingRequest，stdout=HelperEvent 流，stdin=命令。

use serde::{Deserialize, Serialize};

// ── 主进程 → helper（argv[1]）──────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RecordingRequest {
    pub schema_version: u32,
    pub recording_id: i64,
    pub source: Source,
    pub video: VideoConfig,
    pub audio: AudioConfig,
    pub outputs: Outputs,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Display { display_id: u32 },
    Window { window_id: u32 },
    Area { display_id: u32, x: i32, y: i32, width: u32, height: u32 },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VideoConfig {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub codec: VideoCodec,
    pub bitrate: Option<u32>,
    pub hide_system_cursor: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec { H264, Hevc }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AudioConfig {
    pub system: SystemAudioConfig,
    pub microphone: MicrophoneConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SystemAudioConfig {
    pub enabled: bool,
    pub excludes_current_process: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MicrophoneConfig {
    pub enabled: bool,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Outputs {
    pub screen_path: String,
}

// ── helper → 主进程（stdout 事件流）──────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum HelperEvent {
    Ready { schema_version: u32 },
    RecordingStarted { timestamp_ms: i64, width: u32, height: u32 },
    RecordingPaused { timestamp_ms: i64 },
    RecordingResumed { timestamp_ms: i64 },
    RecordingStopped { screen_path: String, duration_ms: i64, file_size: u64 },
    Warning { code: String, message: String },
    Error { code: String, message: String },
}

// ── 枚举辅助（源选择 / 权限）─────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionStatus { Granted, Denied, NotDetermined }

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrivacySection { ScreenCapture, Microphone, Accessibility }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: u32,
    pub title: Option<String>,
    pub app_name: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MicrophoneInfo {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_request_roundtrip_display() {
        let req = RecordingRequest {
            schema_version: 1,
            recording_id: 1773,
            source: Source::Display { display_id: 1 },
            video: VideoConfig {
                fps: 30, width: 2560, height: 1600,
                codec: VideoCodec::H264, bitrate: None, hide_system_cursor: false,
            },
            audio: AudioConfig {
                system: SystemAudioConfig { enabled: true, excludes_current_process: true },
                microphone: MicrophoneConfig {
                    enabled: false, device_id: None, device_name: None,
                },
            },
            outputs: Outputs { screen_path: "/tmp/test.mp4".into() },
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RecordingRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn source_tagged_enum_serializes_with_type_field() {
        let s = Source::Window { window_id: 42 };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""type":"window""#));
        assert!(json.contains(r#""window_id":42"#));
    }

    #[test]
    fn source_area_roundtrip() {
        let s = Source::Area { display_id: 1, x: 100, y: 200, width: 800, height: 600 };
        let json = serde_json::to_string(&s).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn helper_event_started_parses_kebab_case_tag() {
        let line = r#"{"event":"recording-started","timestamp_ms":1000,"width":1920,"height":1080}"#;
        let e: HelperEvent = serde_json::from_str(line).unwrap();
        match e {
            HelperEvent::RecordingStarted { width, height, .. } => {
                assert_eq!(width, 1920);
                assert_eq!(height, 1080);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn helper_event_stopped_parses_all_fields() {
        let line = r#"{"event":"recording-stopped","screen_path":"/tmp/x.mp4","duration_ms":30000,"file_size":1048576}"#;
        let e: HelperEvent = serde_json::from_str(line).unwrap();
        match e {
            HelperEvent::RecordingStopped { screen_path, duration_ms, file_size } => {
                assert_eq!(screen_path, "/tmp/x.mp4");
                assert_eq!(duration_ms, 30000);
                assert_eq!(file_size, 1048576);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn helper_event_error_parses() {
        let line = r#"{"event":"error","code":"permission-denied","message":"Screen recording permission required"}"#;
        let e: HelperEvent = serde_json::from_str(line).unwrap();
        match e {
            HelperEvent::Error { code, message } => {
                assert_eq!(code, "permission-denied");
                assert!(message.contains("Screen recording"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn video_codec_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&VideoCodec::H264).unwrap(), r#""h264""#);
        assert_eq!(serde_json::to_string(&VideoCodec::Hevc).unwrap(), r#""hevc""#);
    }

    #[test]
    fn permission_status_serializes_camel_case() {
        // AGENTS.md 序列化规范：Tauri 边界 enum 变体统一 camelCase。
        // 多词变体 NotDetermined → "notDetermined"（非 lowercase 的 "notdetermined"）。
        assert_eq!(serde_json::to_string(&PermissionStatus::Granted).unwrap(), r#""granted""#);
        assert_eq!(serde_json::to_string(&PermissionStatus::Denied).unwrap(), r#""denied""#);
        assert_eq!(serde_json::to_string(&PermissionStatus::NotDetermined).unwrap(), r#""notDetermined""#);
    }

    #[test]
    fn privacy_section_serializes_camel_case() {
        assert_eq!(serde_json::to_string(&PrivacySection::ScreenCapture).unwrap(), r#""screenCapture""#);
        assert_eq!(serde_json::to_string(&PrivacySection::Microphone).unwrap(), r#""microphone""#);
        assert_eq!(serde_json::to_string(&PrivacySection::Accessibility).unwrap(), r#""accessibility""#);
        // 反序列化（前端 invoke 传参）也必须 camelCase
        assert_eq!(
            serde_json::from_str::<PrivacySection>(r#""screenCapture""#).unwrap(),
            PrivacySection::ScreenCapture,
        );
    }
}
