//! 下载器：probe → 规划 → 并发段 → 进度/sidecar pump → 校验 → rename。
//! 本文件含：类型、config、probe、ensure_part_file、download_single_segment。
//! 并发分块（download 多段）在 Task 8/9 补全。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::{DownloadError, TransientKind, classify_status, ErrorClass};
use crate::core::progress::{Progress, SpeedEstimator};
// 注：plan_segments 暂未使用（task 9 并发编排时导入并使用）。此处仅导入 Segment。
use crate::core::segment::Segment;
use crate::core::verify::Hash;

/// 下载器配置。
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub segment_size: u64,
    pub chunk_threshold: u64,
    pub max_concurrent: usize,
    pub max_retries_per_segment: u32,
    pub backoff_base: Duration,
    pub max_verification_retries: u32,
    pub buf_kb: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(45),
            segment_size: 4 * 1024 * 1024,
            chunk_threshold: 16 * 1024 * 1024,
            max_concurrent: 8,
            max_retries_per_segment: 3,
            backoff_base: Duration::from_secs(1),
            max_verification_retries: 2,
            buf_kb: 256,
        }
    }
}

/// 单文件下载任务。
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub mirrors: Vec<String>,
    pub dest: PathBuf,
    pub expected_hash: Option<Hash>,
}

/// probe 结果。
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub total: Option<u64>,
    pub accept_ranges: bool,
    pub etag: Option<String>,
}

pub struct Downloader {
    client: reqwest::Client,
    config: DownloadConfig,
}

impl Downloader {
    pub fn new(config: DownloadConfig) -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            // 不设全局 timeout：单段读超时由 download_segment_once 的 tokio::time::timeout 控制。
            // reqwest 0.12 的 .timeout() 仅接 Duration（无 Option 重载），故省略该调用即可。
            .user_agent("octopus-download/0.1")
            .build()?;
        Ok(Self { client, config })
    }

    pub fn client(&self) -> &reqwest::Client { &self.client }
    pub fn config(&self) -> &DownloadConfig { &self.config }

    /// 探测：GET Range bytes=0-0 拿 total / accept-ranges / etag。
    pub async fn probe(&self, url: &str) -> Result<ProbeResult, DownloadError> {
        let resp = tokio::time::timeout(
            self.config.connect_timeout * 2,
            self.client.get(url).header("Range", "bytes=0-0").send(),
        )
        .await
        .map_err(|_| transient(TransientKind::Timeout, format!("probe timeout: {url}")))?
        .map_err(map_reqwest_transient)?;

        let status = resp.status().as_u16();
        if let Some(class) = classify_status(status) {
            return Err(class_to_error(class, status, url));
        }
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|cr| cr.split('/').nth(1))
            .and_then(|s| s.parse::<u64>().ok());
        let accept_ranges = resp
            .headers()
            .get("accept-ranges")
            .map(|v| v.to_str().map(|s| s.eq_ignore_ascii_case("bytes")).unwrap_or(false))
            .unwrap_or(false);
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Ok(ProbeResult { total, accept_ranges, etag })
    }

    /// 预分配 .part 文件到 total（sparse）。若已存在且 size!=total 则重新分配。
    pub fn ensure_part_file(dest: &Path, total: u64) -> std::io::Result<std::fs::File> {
        let part = part_path(dest);
        if let Ok(meta) = std::fs::metadata(&part) {
            if meta.len() != total {
                let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&part)?;
                f.set_len(total)?;
                return Ok(f);
            }
            return std::fs::OpenOptions::new().write(true).open(&part);
        }
        let f = std::fs::File::create(&part)?;
        f.set_len(total)?;
        Ok(f)
    }

    /// 单段下载（也是多段每一段的内核）。
    /// 写入 part_path 的 [begin, end]，从 begin+downloaded 续。
    /// progress 计入 counter。返回更新后的 Segment（downloaded 可能增加）。
    pub async fn download_segment(
        &self,
        url: &str,
        part_path: &Path,
        seg: Segment,
        counter: &AtomicU64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Segment, DownloadError> {
        let mut attempt = 0u32;
        loop {
            if let Some(c) = cancel { if c.is_cancelled() { return Err(DownloadError::Cancelled); } }
            let result = self.download_segment_once(url, part_path, &seg, counter, cancel).await;
            match result {
                Ok(new_seg) => return Ok(new_seg),
                Err(DownloadError::Transient { .. }) | Err(DownloadError::Http(_)) | Err(DownloadError::Io(_)) => {
                    attempt += 1;
                    if attempt > self.config.max_retries_per_segment {
                        return Err(result.unwrap_err());
                    }
                    let backoff = backoff(self.config.backoff_base, attempt);
                    log::warn!("segment [{},{}] attempt {attempt} failed, retry in {backoff:?}", seg.begin, seg.end);
                    tokio::time::sleep(backoff).await;
                }
                Err(other) => return Err(other), // Fatal/Cancelled/HashMismatch 直接上抛
            }
        }
    }

    /// 单次段下载尝试。206→续写；200→truncate 重写该段；416→该段重头。
    async fn download_segment_once(
        &self,
        url: &str,
        part_path: &Path,
        seg: &Segment,
        counter: &AtomicU64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Segment, DownloadError> {
        let start = seg.next_offset();
        let end = seg.end;
        if start > end { return Ok(*seg); } // 已完成
        // task 9 编排时会在此插入 If-Range header（req = req.header(...)），届时移除 allow。
        #[allow(unused_mut)]
        let mut req = self.client.get(url).header("Range", format!("bytes={start}-{end}"));
        if let Some(ir) = crate::core::verify::if_range_value(None) {
            // etag 由调用方在 multi-segment 编排时注入；单段 probe 的 etag 经参数透传见 Task 9
            let _ = ir;
        }
        let resp = tokio::time::timeout(self.config.read_timeout, req.send())
            .await
            .map_err(|_| transient(TransientKind::Timeout, "segment read timeout"))?
            .map_err(map_reqwest_transient)?;

        let status = resp.status().as_u16();
        if let Some(class) = classify_status(status) {
            return Err(class_to_error(class, status, url));
        }

        use std::io::{SeekFrom, Write, Seek};
        let mut file = std::fs::OpenOptions::new().write(true).open(part_path)?;
        let write_offset = if status == 206 || status == 200 {
            // 206=续传从 start；200=服务端忽略 Range，从头覆盖该段
            let off = if status == 200 { seg.begin } else { start };
            file.seek(SeekFrom::Start(off))?;
            off
        } else {
            // 416 等：该段重头
            file.seek(SeekFrom::Start(seg.begin))?;
            seg.begin
        };

        let mut writer = std::io::BufWriter::with_capacity(self.config.buf_kb * 1024, file);
        let mut stream = resp.bytes_stream();
        let mut written_this_call: u64 = 0;
        while let Some(chunk) = stream.next().await {
            if let Some(c) = cancel { if c.is_cancelled() { return Err(DownloadError::Cancelled); } }
            let bytes = chunk.map_err(map_reqwest_transient)?;
            writer.write_all(&bytes)?;
            written_this_call += bytes.len() as u64;
            counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        writer.flush()?;
        // 200 重写时，该段 downloaded 应等于整段长；206 续传则累加
        let new_downloaded = if status == 200 { (write_offset - seg.begin) + written_this_call } else { seg.downloaded + written_this_call };
        Ok(Segment { begin: seg.begin, end: seg.end, downloaded: new_downloaded })
    }

    /// 并发下载多段。每段独立 task，Semaphore 限并发，进度累计到 counter。
    pub async fn download_chunked(
        &self,
        url: &str,
        part_path: &Path,
        segments: Vec<Segment>,
        counter: Arc<AtomicU64>,
        cancel: Option<CancellationToken>,
    ) -> Result<Vec<Segment>, DownloadError> {
        use tokio::task::JoinSet;
        let sem = Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrent));
        let url = Arc::new(url.to_string());
        let part = Arc::new(part_path.to_path_buf());
        let total = segments.len();
        let mut join: JoinSet<Result<(usize, Segment), DownloadError>> = JoinSet::new();

        for (i, seg) in segments.into_iter().enumerate() {
            let url = Arc::clone(&url);
            let part = Arc::clone(&part);
            let counter = Arc::clone(&counter);
            let sem = Arc::clone(&sem);
            let cancel = cancel.clone();
            // &self 不能 move 进 spawn：拷出 client clone（reqwest::Client 内部 Arc，廉价）+ cfg clone。
            let client = self.client.clone();
            let cfg = self.config.clone();
            join.spawn(async move {
                let _permit = sem.acquire().await.map_err(|_| {
                    DownloadError::Transient { kind: TransientKind::Network, message: "semaphore closed".into() }
                })?;
                download_segment_with_client(&client, &cfg, &url, &part, seg, &counter, cancel.as_ref())
                    .await
                    .map(|s| (i, s))
            });
        }

        let mut results = vec![None; total];
        while let Some(res) = join.join_next().await {
            let (i, seg) = res
                .map_err(|e| DownloadError::Transient {
                    kind: TransientKind::Network,
                    message: format!("join: {e}"),
                })??;
            results[i] = Some(seg);
        }
        Ok(results.into_iter().map(|x| x.expect("every idx filled")).collect())
    }
}

/// .part 路径：dest + ".part"
pub fn part_path(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_os_string();
    p.push(".part");
    PathBuf::from(p)
}

fn transient(kind: TransientKind, msg: impl Into<String>) -> DownloadError {
    DownloadError::Transient { kind, message: msg.into() }
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    // 指数：base * 2^(attempt-1)，封顶 60s。jitter 用 attempt 派生（脚本环境无 rand）。
    let mul = 2u64.saturating_pow(attempt.saturating_sub(1));
    let dur = base.as_millis() as u64 * mul;
    Duration::from_millis(dur.min(60_000))
}

fn map_reqwest_transient(e: reqwest::Error) -> DownloadError {
    if e.is_timeout() {
        transient(TransientKind::Timeout, e.to_string())
    } else if e.is_connect() || e.is_request() {
        transient(TransientKind::Network, e.to_string())
    } else {
        DownloadError::Http(e)
    }
}

fn class_to_error(class: ErrorClass, status: u16, url: &str) -> DownloadError {
    match class {
        ErrorClass::Fatal => DownloadError::Fatal { status, url: url.to_string() },
        ErrorClass::Transient(kind) => DownloadError::Transient { kind, message: format!("HTTP {status}") },
    }
}

/// 段下载自由函数（spawned task 友好：不持 &Downloader）。
/// 与 Downloader::download_segment 行为一致（带计数器重试 + 指数 backoff）。
async fn download_segment_with_client(
    client: &reqwest::Client,
    cfg: &DownloadConfig,
    url: &str,
    part_path: &Path,
    seg: Segment,
    counter: &AtomicU64,
    cancel: Option<&CancellationToken>,
) -> Result<Segment, DownloadError> {
    let mut attempt = 0u32;
    loop {
        if let Some(c) = cancel {
            if c.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
        }
        match download_segment_once_with_client(client, cfg, url, part_path, seg, counter, cancel).await {
            Ok(s) => return Ok(s),
            Err(DownloadError::Transient { .. }) | Err(DownloadError::Http(_)) | Err(DownloadError::Io(_)) => {
                attempt += 1;
                if attempt > cfg.max_retries_per_segment {
                    return Err(DownloadError::Transient {
                        kind: TransientKind::Network,
                        message: format!("segment exhausted after {attempt} attempts"),
                    });
                }
                tokio::time::sleep(backoff(cfg.backoff_base, attempt)).await;
            }
            Err(other) => return Err(other),
        }
    }
}

/// 单次段下载尝试（自由函数版）。206→从 next_offset 续写；200→从头覆盖该段。
async fn download_segment_once_with_client(
    client: &reqwest::Client,
    cfg: &DownloadConfig,
    url: &str,
    part_path: &Path,
    seg: Segment,
    counter: &AtomicU64,
    cancel: Option<&CancellationToken>,
) -> Result<Segment, DownloadError> {
    let start = seg.next_offset();
    let end = seg.end;
    if start > end {
        return Ok(seg); // 已完成
    }
    let req = client.get(url).header("Range", format!("bytes={start}-{end}"));
    // 直接传 &str（不写 .into()）：transient 接 impl Into<String>，
    // .into() 会因 &str: Into<&str>（自反）与 Into<String> 双解触发 E0283 歧义。
    let resp = tokio::time::timeout(cfg.read_timeout, req.send())
        .await
        .map_err(|_| transient(TransientKind::Timeout, "segment read timeout"))?
        .map_err(map_reqwest_transient)?;

    let status = resp.status().as_u16();
    if let Some(class) = classify_status(status) {
        return Err(class_to_error(class, status, url));
    }

    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(part_path)?;
    let write_offset = if status == 200 { seg.begin } else { start };
    file.seek(SeekFrom::Start(write_offset))?;

    let mut writer = std::io::BufWriter::with_capacity(cfg.buf_kb * 1024, file);
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        if let Some(c) = cancel {
            if c.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
        }
        let bytes = chunk.map_err(map_reqwest_transient)?;
        writer.write_all(&bytes)?;
        written += bytes.len() as u64;
        counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    }
    writer.flush()?;
    // 200 重写：该段 downloaded 应等于整段已写字节；206 续传则累加
    let new_downloaded = if status == 200 {
        (write_offset - seg.begin) + written
    } else {
        seg.downloaded + written
    };
    Ok(Segment { begin: seg.begin, end: seg.end, downloaded: new_downloaded })
}

// 注：SpeedEstimator/plan_segments/concurrency/progress pump/sidecar pump 在 Task 8/9 编排时接线。
// 此处保留占位引用以避免未使用告警（实际接线后移除）。
#[allow(dead_code)]
fn _unused_keep_types(_s: SpeedEstimator, _p: Progress, _segs: Vec<Segment>, _tx: mpsc::Sender<Progress>, _a: Arc<u64>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{MockServer, Method};
    use tempfile::tempdir;

    #[tokio::test]
    async fn probe_returns_total_and_accept_ranges() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(Method::GET).path("/m.onnx").header("Range", "bytes=0-0");
            then.status(206)
                .header("Content-Range", "bytes 0-0/12345")
                .header("Accept-Ranges", "bytes")
                .header("ETag", "\"abc\"")
                .body("x");
        });
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let p = dl.probe(&server.url("/m.onnx")).await.unwrap();
        assert_eq!(p.total, Some(12345));
        assert!(p.accept_ranges);
        assert_eq!(p.etag.as_deref(), Some("\"abc\""));
    }

    #[tokio::test]
    async fn probe_404_is_fatal() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(Method::GET).path("/missing");
            then.status(404);
        });
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let err = dl.probe(&server.url("/missing")).await.unwrap_err();
        assert!(matches!(err, DownloadError::Fatal { status: 404, .. }));
    }

    #[tokio::test]
    async fn download_single_segment_writes_part() {
        let server = MockServer::start();
        let body = b"hello world payload data!!"; // 26 bytes
        let body_len = body.len() as u64;
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes=0-{}", body_len - 1));
            then.status(206).body(*body);
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _file = Downloader::ensure_part_file(&dest, body_len).unwrap();
        let part = part_path(&dest);
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let seg = Segment { begin: 0, end: body_len - 1, downloaded: 0 };
        let counter = AtomicU64::new(0);
        let out = dl.download_segment(&server.url("/f"), &part, seg, &counter, None).await.unwrap();
        assert_eq!(out.downloaded, body_len);
        assert_eq!(counter.load(Ordering::Relaxed), body_len);
        let written = std::fs::read(&part).unwrap();
        assert_eq!(written, body);
    }

    #[test]
    fn ensure_part_file_creates_sized() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _ = Downloader::ensure_part_file(&dest, 9999).unwrap();
        let part = part_path(&dest);
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 9999);
    }

    #[test]
    fn backoff_grows_exponentially() {
        let b1 = backoff(Duration::from_secs(1), 1);
        let b2 = backoff(Duration::from_secs(1), 2);
        let b3 = backoff(Duration::from_secs(1), 3);
        assert!(b2 > b1);
        assert!(b3 > b2);
    }

    #[tokio::test]
    async fn download_chunked_writes_full_file_in_order() {
        use crate::core::segment::plan_segments;
        let server = MockServer::start();
        // 100 字节，分 2 段（每段 50）
        let total: u64 = 100;
        let body: Vec<u8> = (0..total as u8).collect();
        let half = 50u64;
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes=0-{}", half - 1));
            then.status(206).body(body[0..half as usize].to_vec());
        });
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes={half}-{}", total - 1));
            then.status(206).body(body[half as usize..total as usize].to_vec());
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _ = Downloader::ensure_part_file(&dest, total).unwrap();
        let part = part_path(&dest);
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let segs = plan_segments(total, true, half, 0, 2); // threshold=0 强制多段
        assert_eq!(segs.len(), 2);
        let counter = Arc::new(AtomicU64::new(0));
        let done = dl.download_chunked(&server.url("/f"), &part, segs, counter, None).await.unwrap();
        assert!(done.iter().all(|s| s.is_done()));
        let written = std::fs::read(&part).unwrap();
        assert_eq!(written, body);
    }
}
