//! HuggingFace API：GET /api/models/{repo} 解析文件 siblings。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct HfSibling {
    pub rfilename: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub lfs: Option<LfsInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LfsInfo {
    pub oid: Option<String>, // sha256
}

#[derive(Debug, Clone, Deserialize)]
struct ModelInfo {
    siblings: Vec<HfSibling>,
}

/// 拉取 repo 的文件列表。source_url 如 "https://hf-mirror.com"（无尾斜杠）或官方源。
pub async fn fetch_siblings(
    client: &reqwest::Client,
    source_url: &str,
    repo: &str,
) -> Result<Vec<HfSibling>, crate::core::error::DownloadError> {
    let base = source_url.trim_end_matches('/');
    let url = format!("{base}/api/models/{repo}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(crate::core::error::DownloadError::Http)?;
    let status = resp.status().as_u16();
    if crate::core::error::classify_status(status).is_some() {
        return Err(crate::core::error::DownloadError::HfApi { status, url });
    }
    let info: ModelInfo = resp
        .json()
        .await
        .map_err(crate::core::error::DownloadError::Http)?;
    Ok(info.siblings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method, MockServer};

    #[tokio::test]
    async fn fetch_parses_siblings_and_lfs() {
        let server = MockServer::start();
        let body = r#"{
            "siblings": [
                {"rfilename": "config.json", "etag": "small-etag"},
                {"rfilename": "onnx/model_int8.onnx", "etag": "lfs-etag", "lfs": {"oid": "abcdef0123456789"}}
            ]
        }"#;
        server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/models/onnx-community/whisper-small.en");
            then.status(200).body(body);
        });
        let client = reqwest::Client::new();
        let sibs = fetch_siblings(
            &client,
            &server.base_url(),
            "onnx-community/whisper-small.en",
        )
        .await
        .unwrap();
        assert_eq!(sibs.len(), 2);
        assert_eq!(sibs[0].rfilename, "config.json");
        assert_eq!(
            sibs[1].lfs.as_ref().and_then(|l| l.oid.as_deref()),
            Some("abcdef0123456789")
        );
    }
}
