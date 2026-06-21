//! 端到端：HF resolve → download，全 httpmock。

use octopus_download::{Downloader, DownloadConfig, HfRequest, resolve_tasks};
use httpmock::{MockServer, Method};

#[tokio::test]
async fn hf_resolve_then_download_single_file() {
    let server = MockServer::start();
    // lfs.oid 必须是 body 的真实 sha256，否则 download 的 hash 校验会 mismatch。
    // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    let oid = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    // api
    server.mock(|when, then| {
        when.method(Method::GET).path("/api/models/org/m");
        then.status(200)
            .body(format!(r#"{{"siblings":[{{"rfilename":"model.onnx","etag":"e","lfs":{{"oid":"{oid}"}}}}]}}"#));
    });
    // probe
    server.mock(|when, then| {
        when.method(Method::GET).path("/org/m/resolve/main/model.onnx").header("Range", "bytes=0-0");
        then.status(206).header("Content-Range", "bytes 0-0/5").header("Accept-Ranges", "bytes");
    });
    // body
    server.mock(|when, then| {
        when.method(Method::GET).path("/org/m/resolve/main/model.onnx").header("Range", "bytes=0-4");
        then.status(206).body(b"hello");
    });
    let client = reqwest::Client::new();
    let dir = tempfile::tempdir().unwrap();
    let req = HfRequest {
        repo: "org/m".into(),
        include: vec!["model.onnx".into()],
        exclude: vec![],
        source_url: Some(server.base_url()),
        target_dir: dir.path().to_path_buf(),
    };
    let tasks = resolve_tasks(&client, req).await.unwrap();
    assert_eq!(tasks.len(), 1);
    let dl = Downloader::new(DownloadConfig::default()).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    dl.download(&tasks[0], tx, None).await.unwrap();
    assert_eq!(std::fs::read(dir.path().join("org/m/model.onnx")).unwrap(), b"hello");
}
