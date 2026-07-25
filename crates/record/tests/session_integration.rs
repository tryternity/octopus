//! RecordSession 集成测试：用 mock-helper 二进制验证状态机。

use octopus_record::*;
use std::path::PathBuf;
use std::process::Command;

fn mock_helper_path() -> PathBuf {
    // mock-helper 编译产物在 workspace target/debug/（cargo workspace 共享 target）
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let candidates = [
        // workspace_root/target/debug/mock-helper（crate 在 crates/<name>/ 下，往上两级）
        PathBuf::from(&manifest_dir).join("../../target/debug/mock-helper"),
        PathBuf::from(&manifest_dir).join("../target/debug/mock-helper"),
        PathBuf::from("target/debug/mock-helper"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    panic!("mock-helper not found, run: cargo build -p octopus-record --bin mock-helper");
}

fn sample_request(screen_path: &str) -> RecordingRequest {
    RecordingRequest {
        schema_version: 1,
        recording_id: 1001,
        source: Source::Display { display_id: 1 },
        video: VideoConfig {
            fps: 30, width: 1920, height: 1080,
            codec: VideoCodec::H264, bitrate: None, hide_system_cursor: false,
        },
        audio: AudioConfig {
            system: SystemAudioConfig { enabled: true, excludes_current_process: true },
            microphone: MicrophoneConfig { enabled: false, device_id: None, device_name: None },
        },
        outputs: Outputs { screen_path: screen_path.into() },
    }
}

#[tokio::test]
async fn start_pause_resume_stop_state_transitions() {
    // 先确保 mock-helper 编译
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let _ = Command::new("cargo")
        .args(["build", "-p", "octopus-record", "--bin", "mock-helper"])
        .current_dir(&manifest_dir)
        .status()
        .expect("failed to build mock-helper");

    let helper = mock_helper_path();
    let session = RecordSession::new();
    let events = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let started = session.start(&helper, sample_request("/tmp/test.mp4"), move |e| {
        let events_clone = events_clone.clone();
        tokio::spawn(async move {
            events_clone.lock().await.push(e);
        });
    }).await.unwrap();
    assert_eq!(started.width, 1920);
    assert_eq!(started.height, 1080);
    assert_eq!(session.state().await, SessionState::Recording);

    session.pause().await.unwrap();
    assert_eq!(session.state().await, SessionState::Paused);

    session.resume().await.unwrap();
    assert_eq!(session.state().await, SessionState::Recording);

    session.stop().await.unwrap();
    assert_eq!(session.state().await, SessionState::Idle);
}

#[tokio::test]
async fn start_twice_returns_already_running() {
    let helper = mock_helper_path();
    let session = RecordSession::new();
    let _ = session.start(&helper, sample_request("/tmp/test1.mp4"), |_| {}).await;

    let err = session.start(&helper, sample_request("/tmp/test2.mp4"), |_| {}).await;
    assert!(matches!(err, Err(RecordError::AlreadyRunning)));

    session.kill().await.unwrap();
}
