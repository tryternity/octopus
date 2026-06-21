//! 把 HfRequest 解析为 DownloadTask 列表（调 api + glob + 构造 URL/hash）。

use std::path::PathBuf;
use crate::core::downloader::DownloadTask;
use crate::core::error::DownloadError;
use crate::core::verify::Hash;
use crate::hf::api::{fetch_siblings, HfSibling};
use crate::hf::glob::should_download;

const OFFICIAL_BASE: &str = "https://huggingface.co";

pub struct HfRequest {
    pub repo: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub source_url: Option<String>,   // 镜像，如 https://hf-mirror.com
    pub target_dir: PathBuf,
}

/// resolve 单文件的下载 URL（镜像在前）+ expected hash。
fn build_task(sib: &HfSibling, req: &HfRequest) -> Option<DownloadTask> {
    if !should_download(&sib.rfilename, &req.include, &req.exclude) { return None; }
    let mirror = req.source_url.as_deref().map(|s| s.trim_end_matches('/'));
    let mut urls: Vec<String> = Vec::new();
    if let Some(m) = mirror {
        urls.push(format!("{m}/{}/resolve/main/{}", req.repo, sib.rfilename));
    }
    urls.push(format!("{OFFICIAL_BASE}/{}/resolve/main/{}", req.repo, sib.rfilename));
    let url = urls.remove(0);
    let dest = req.target_dir.join(&req.repo).join(&sib.rfilename);
    let expected_hash = sib.lfs.as_ref().and_then(|l| l.oid.clone())
        .map(Hash::Sha256)
        .or_else(|| sib.etag.clone().map(Hash::Etag));
    Some(DownloadTask { url, mirrors: urls, dest, expected_hash })
}

pub async fn resolve_tasks(
    client: &reqwest::Client,
    req: HfRequest,
) -> Result<Vec<DownloadTask>, DownloadError> {
    let source = req.source_url.as_deref().map(|s| s.trim_end_matches('/')).unwrap_or(OFFICIAL_BASE).to_string();
    let siblings = fetch_siblings(client, &source, &req.repo).await?;
    let tasks: Vec<DownloadTask> = siblings.iter().filter_map(|s| build_task(s, &req)).collect();
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{MockServer, Method};

    fn req(repo: &str, inc: &[&str], exc: &[&str], mirror: Option<&str>, dir: &std::path::Path) -> HfRequest {
        HfRequest {
            repo: repo.into(),
            include: inc.iter().map(|s| s.to_string()).collect(),
            exclude: exc.iter().map(|s| s.to_string()).collect(),
            source_url: mirror.map(|s| s.to_string()),
            target_dir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn resolve_end_to_end_filters_and_builds_urls() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(Method::GET).path("/api/models/org/m");
            then.status(200).body(r#"{"siblings":[
                {"rfilename":"config.json","etag":"e1"},
                {"rfilename":"onnx/model_int8.onnx","etag":"e2","lfs":{"oid":"sha256hex"}},
                {"rfilename":"onnx/model_fp16.onnx","etag":"e3","lfs":{"oid":"other"}}
            ]}"#);
        });
        let client = reqwest::Client::new();
        let dir = tempfile::tempdir().unwrap();
        let r = req("org/m", &["onnx/*_int8.onnx"], &[], Some(&server.base_url()), dir.path());
        let tasks = resolve_tasks(&client, r).await.unwrap();
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert!(t.url.starts_with(&server.base_url()));
        assert!(t.url.ends_with("/org/m/resolve/main/onnx/model_int8.onnx"));
        // 官方源作 fallback mirror
        assert!(t.mirrors[0].starts_with("https://huggingface.co"));
        assert!(matches!(t.expected_hash, Some(Hash::Sha256(_))));
        assert_eq!(t.dest, dir.path().join("org/m").join("onnx/model_int8.onnx"));
    }
}
