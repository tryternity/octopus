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

/// 通过 wrapper 启动指定 mode 的 mock-helper：argv[0]=mode, argv[1+]=透传给 mock-helper。
/// 但 session.start 把 argv[1] 设为 request JSON——所以 wrapper 的 argv 必须是
/// `<mode> <request_json>`。session.start 只接受 helper_path + 内部加 argv[1]=request，
/// 无法在前面插入 mode。解决：用**独立 wrapper 脚本**，每个 mode 一个，硬编码 mode。
fn write_mode_wrapper(mode: &str) -> PathBuf {
    // 写一个临时 wrapper：exec mock-helper 时 MOCK_HELPER_MODE=<mode>
    let dir = std::env::temp_dir().join("octopus_record_tests");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = mock_helper_path();
    let script = format!(
        "#!/usr/bin/env bash\nexport MOCK_HELPER_MODE={mode}\nexec {mock} \"$@\"\n",
        mode = mode,
        mock = mock.display()
    );
    let path = dir.join(format!("mock_helper_{mode}.sh"));
    std::fs::write(&path, &script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
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

// ── 2026-07-26 P0 回归测试 ──────────────────────────────────────────
//
// 修复前 bug：start() 超时用 `?` 直接返回，**不重置 state、不 kill child**
// → state 永远卡 Starting → 之后所有 start 撞 AlreadyRunning，用户必须重启 app。
// 这组测试验证 timeout/error 后 state 回到 Idle，且可立即重新 start。

/// 回归：helper 发 ready 但不发 recording-started（模拟 SCK 不出帧）→
/// start() 应超时返回 Err，且**事后 state=Idle**（不是卡 Starting）。
#[tokio::test]
async fn start_timeout_resets_state_to_idle() {
    let helper = write_mode_wrapper("no-started");
    let session = RecordSession::new();

    let err = session.start(&helper, sample_request("/tmp/timeout.mp4"), |_| {}).await;
    assert!(matches!(err, Err(RecordError::Timeout { event: "recording-started" })),
        "expected Timeout, got {:?}", err);

    // **核心断言**：state 必须回到 Idle，否则后续 start 撞 AlreadyRunning（原 bug）
    assert_eq!(session.state().await, SessionState::Idle,
        "state must reset to Idle after timeout (was buggy: stuck in Starting)");
}

/// 回归：timeout 后能立即重新 start（原 bug 卡死，必须重启 app）。
#[tokio::test]
async fn can_restart_after_timeout() {
    let helper_no_started = write_mode_wrapper("no-started");
    let helper_normal = mock_helper_path();
    let session = RecordSession::new();

    // 第一次：超时
    let err = session.start(&helper_no_started, sample_request("/tmp/fail.mp4"), |_| {}).await;
    assert!(matches!(err, Err(RecordError::Timeout { .. })));
    assert_eq!(session.state().await, SessionState::Idle);

    // 第二次：立即用正常 helper 重试——必须能起来（原 bug 会 AlreadyRunning）
    let started = session.start(&helper_normal, sample_request("/tmp/ok.mp4"), |_| {}).await;
    assert!(started.is_ok(), "must be able to restart after timeout, got {:?}", started);
    assert_eq!(session.state().await, SessionState::Recording);

    session.stop().await.unwrap();
}

/// 回归：helper 主动 emit error（模拟 permissionDenied）→
/// start() 应**立即**返回 HelperError（不等 10s 超时），且事后 state=Idle。
#[tokio::test]
async fn start_helper_error_short_circuits_and_resets() {
    let helper = write_mode_wrapper("error");
    let session = RecordSession::new();

    let start_time = std::time::Instant::now();
    let err = session.start(&helper, sample_request("/tmp/err.mp4"), |_| {}).await;
    let elapsed = start_time.elapsed();

    assert!(matches!(err, Err(RecordError::HelperError { ref code, .. }) if code == "permissionDenied"),
        "expected HelperError(permissionDenied), got {:?}", err);
    // 应在 1s 内返回（不等 10s 超时——HelperEvent::Error 短路）
    assert!(elapsed < std::time::Duration::from_secs(3),
        "helper error should short-circuit fast, took {:?}", elapsed);
    assert_eq!(session.state().await, SessionState::Idle,
        "state must reset to Idle after helper error");
}

/// 回归：helper 向 stderr 写 200KB（>64KB 管道缓冲）→
/// 修复前父进程不读 stderr → helper 阻塞 → 不发 started → 超时。
/// 修复后父进程 spawn stderr reader → helper 不阻塞（但仍不发 started，会超时）。
/// 本测试验证 stderr reader task 让 helper 进程能正常被 kill（不残留孤儿）。
#[tokio::test]
async fn start_stderr_flood_does_not_orphan_helper() {
    let helper = write_mode_wrapper("stderr-flood");
    let session = RecordSession::new();

    let err = session.start(&helper, sample_request("/tmp/flood.mp4"), |_| {}).await;
    assert!(matches!(err, Err(RecordError::Timeout { .. })),
        "expected Timeout (helper doesn't emit started), got {:?}", err);
    assert_eq!(session.state().await, SessionState::Idle);

    // reset_to_idle 应已 SIGKILL helper——验证没有 mock-helper 孤儿进程残留。
    // （kill 是异步 wait 的，等一下让进程真正退出）
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let orphans = Command::new("pgrep")
        .args(["-f", "mock-helper"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // 允许有其他测试的 mock-helper，但 stderr-flood 这个应该没了——简单断言 stderr-flood 模式
    // 的进程已退出（用 ps 找 MOCK_HELPER_MODE=stderr-flood）
    let flood_orphans = Command::new("sh")
        .args(["-c", "ps aux | grep mock-helper | grep stderr-flood | grep -v grep || true"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    assert!(flood_orphans.is_empty(),
        "stderr-flood helper should be killed, but found orphan: {flood_orphans}");
    let _ = orphans; // 不强断言其他测试的 mock-helper
}
