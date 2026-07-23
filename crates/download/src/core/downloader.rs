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
use crate::core::segment::{Segment, plan_segments};
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
    /// 预期文件大小（来自 manifest），作为 probe 拿不到 content-length 时的 fallback。
    /// 某些 CDN（如 CloudFront）对非 LFS 小文件返回 200 无 content-length，
    /// 此时用 manifest 的 size 预分配文件 + 校验。
    pub expected_size: Option<u64>,
}

impl Default for DownloadTask {
    fn default() -> Self {
        Self {
            url: String::new(),
            mirrors: Vec::new(),
            dest: PathBuf::new(),
            expected_hash: None,
            expected_size: None,
        }
    }
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
        // 206 Partial Content → content-range 头取 total（标准 Range 响应）
        // 200 OK（服务端忽略 Range 返回全文，或 307 重定向后 CDN 返回 200）→
        //   无 content-range，fallback 用 content-length 作为 total（= 全文件大小）。
        //   此时 accept_ranges=false（plan_segments 会生成单段整文件下载）。
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|cr| cr.split('/').nth(1))
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| {
                // 200 响应：content-length = 全文件大小
                if status == 200 {
                    resp.headers()
                        .get("content-length")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                }
            });
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
    /// 会先 create_dir_all 父目录（dest 常为 {target}/{repo}/{path}，深层路径父目录可能不存在）。
    pub fn ensure_part_file(dest: &Path, total: u64) -> std::io::Result<std::fs::File> {
        let part = part_path(dest);
        if let Some(parent) = part.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
    /// 委托自由函数 download_segment_with_client——消除重复实现，确保方法版与
    /// 生产路径（download_chunked）走同一份代码（200 跳过 begin 字节 + stream timeout）。
    pub async fn download_segment(
        &self,
        url: &str,
        part_path: &Path,
        seg: Segment,
        counter: &AtomicU64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Segment, DownloadError> {
        download_segment_with_client(&self.client, &self.config, url, part_path, seg, counter, cancel).await
    }

    /// 并发下载多段。每段独立 task，Semaphore 限并发，进度累计到 counter。
    /// 段完成时回写 state（若提供）并落盘 sidecar（dest 提供），支持崩溃续传。
    #[allow(clippy::too_many_arguments)] // 内部编排方法，参数随下载语义自然增长
    pub async fn download_chunked(
        &self,
        url: &str,
        part_path: &Path,
        segments: Vec<Segment>,
        counter: Arc<AtomicU64>,
        cancel: Option<CancellationToken>,
        state: Option<Arc<std::sync::Mutex<crate::core::resume::ResumeState>>>,
        dest: Option<&Path>,
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
            // 段完成：回写共享 state 并落盘 sidecar（崩溃续传用）
            if let (Some(st), Some(d)) = (&state, dest) {
                let snapshot = {
                    let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
                    if i < g.segments.len() {
                        g.segments[i].downloaded = seg.downloaded;
                    }
                    g.clone()
                };
                let _ = crate::core::resume::save(d, &snapshot);
            }
            results[i] = Some(seg);
        }
        Ok(results.into_iter().map(|x| x.expect("every idx filled")).collect())
    }

    /// 下载单个 task：probe → 规划 → 并发 → 进度 pump → 校验 → rename。
    /// 镜像 fallback：主 url 失败依次试 mirrors（含 Fatal——镜像可能缺文件）。
    pub async fn download(
        &self,
        task: &DownloadTask,
        progress: mpsc::Sender<Progress>,
        cancel: Option<CancellationToken>,
    ) -> Result<(), DownloadError> {
        // 镜像候选：主 url 在前，mirrors 随后
        let mut sources: Vec<String> = vec![task.url.clone()];
        sources.extend(task.mirrors.iter().cloned());

        let mut last_err: Option<DownloadError> = None;
        for src in &sources {
            if let Some(c) = &cancel {
                if c.is_cancelled() {
                    return Err(DownloadError::Cancelled);
                }
            }
            match self.download_from_source(src, task, progress.clone(), cancel.as_ref()).await {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or(DownloadError::Fatal { status: 0, url: task.url.clone() }))
    }

    /// 单源下载：probe → 加载/规划分段 → 预分配 .part → 并发 → 校验 → 原子转正。
    async fn download_from_source(
        &self,
        url: &str,
        task: &DownloadTask,
        progress: mpsc::Sender<Progress>,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), DownloadError> {
        // probe 可能因网络慢/超时失败（307 重定向需两次 TLS 握手）。
        // 若有 manifest 的 expected_size，probe 失败不中断——用它作为 total，
        // accept_ranges=false（单段下载），让 download_segment 直接 GET 全文。
        let probe = match self.probe(url).await {
            Ok(p) => p,
            Err(e) if task.expected_size.is_some() => {
                log::warn!("[download] probe 失败（{}），用 manifest expected_size 继续: {}", task.dest.display(), e);
                ProbeResult { total: None, accept_ranges: false, etag: None }
            }
            Err(e) => return Err(e),
        };
        // total 优先用 probe 结果（content-range/content-length）；拿不到时 fallback
        // 到 manifest 的 expected_size（某些 CDN 200 响应无 content-length 头，
        // 如 CloudFront 对非 LFS 小文件的 200 chunked 响应）。
        let total = probe.total.or(task.expected_size).ok_or_else(|| {
            transient(TransientKind::Network, "no content-length and no expected_size")
        })?;
        // probe 失败 fallback 时 accept_ranges=false，确保单段下载
        let accept_ranges = probe.accept_ranges && probe.total.is_some();

        // 规划：加载 sidecar 复用进度，否则重新规划。
        // sidecar 的 url_hash 基于 dest（镜像无关），故镜像源也可复用——这是设计意图：
        // 镜像即"同文件不同 URL"，内容一致时复用进度省带宽，内容不一致由最终 hash 校验兜底。
        // 唯一需丢弃 sidecar 的情况：多段 sidecar 遇到不支持 Range 的源（否则会向不支持
        // Range 的服务器发分段 Range 请求，注定得到 200 全文且 offset 错位）。单段 sidecar
        // 无此问题——即便服务端忽略 Range 返回全文，200 重写路径会从头覆盖整个单段=整文件。
        let segs = match crate::core::resume::load(&task.dest, total) {
            Some(state)
                if !state.segments.is_empty()
                    && (accept_ranges || state.segments.len() == 1) =>
            {
                log::info!("resume: 侧载 sidecar，{} 段", state.segments.len());
                state.segments
            }
            _ => plan_segments(
                total,
                accept_ranges,
                self.config.segment_size,
                self.config.chunk_threshold,
                self.config.max_concurrent,
            ),
        };

        // 预分配 .part
        let _ = Downloader::ensure_part_file(&task.dest, total)?;
        let part = part_path(&task.dest);

        // 进度计数：累加 sidecar 恢复的已下字节
        let downloaded_start: u64 = segs.iter().map(|s| s.downloaded).sum();
        let counter = Arc::new(AtomicU64::new(downloaded_start));

        // sidecar 状态：段完成时由 download_chunked 回写
        let state = Arc::new(std::sync::Mutex::new(crate::core::resume::new_state(
            &task.dest,
            total,
            probe.etag.clone(),
            segs.clone(),
        )));

        // 进度 pump：250ms 推 mpsc（独立 task，自带 sender clone，避免 move 主 progress）
        let pump_tx = progress.clone();
        let pump_counter = Arc::clone(&counter);
        let pump_cancel = cancel.cloned();
        let progress_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            let mut est = SpeedEstimator::new();
            let mut last_inst = tokio::time::Instant::now();
            loop {
                interval.tick().await;
                if let Some(c) = &pump_cancel {
                    if c.is_cancelled() {
                        break;
                    }
                }
                let bytes = pump_counter.load(Ordering::Relaxed);
                let now = tokio::time::Instant::now();
                let spd = est.update(bytes, now - last_inst, 0.4, Duration::from_millis(300));
                last_inst = now;
                let _ = pump_tx
                    .send(Progress {
                        downloaded_bytes: bytes,
                        total_bytes: Some(total),
                        speed_bps: Some(spd),
                    })
                    .await;
                if bytes >= total {
                    break;
                }
            }
        });

        // 执行下载（段完成回写 state + 落盘 sidecar）
        let done = self
            .download_chunked(
                url,
                &part,
                segs,
                Arc::clone(&counter),
                cancel.cloned(),
                Some(Arc::clone(&state)),
                Some(task.dest.as_path()),
            )
            .await;

        // 停 pump
        progress_handle.abort();

        done?;

        // 校验 + 失败重下：hash 不匹配时删 .part + sidecar 重下整个文件，而非只重算 hash。
        // （hash 确定性：同一文件重算必然再失败；必须重新下载才有意义。）
        if let Some(expected) = &task.expected_hash {
            let mut verify_ok = false;
            for attempt in 0..=self.config.max_verification_retries {
                if crate::core::verify::verify(&part, expected).await? {
                    verify_ok = true;
                    break;
                }
                if attempt < self.config.max_verification_retries {
                    log::warn!("hash mismatch (attempt {}), 删除 .part 重新下载", attempt + 1);
                    let _ = std::fs::remove_file(&part);
                    crate::core::resume::remove(&task.dest);
                    // 重新规划段 + 重下——复用主 counter + 重启 pump 让前端看到重下进度
                    let new_segs = plan_segments(
                        total,
                        accept_ranges,
                        self.config.segment_size,
                        self.config.chunk_threshold,
                        self.config.max_concurrent,
                    );
                    let _ = Downloader::ensure_part_file(&task.dest, total)?;
                    // 重置主 counter 为 0（重下从头开始），重启进度泵
                    counter.store(0, Ordering::Relaxed);
                    let retry_pump_tx = progress.clone();
                    let retry_pump_counter = Arc::clone(&counter);
                    let retry_pump_cancel = cancel.cloned();
                    let retry_pump = tokio::spawn(async move {
                        let mut interval = tokio::time::interval(Duration::from_millis(250));
                        let mut est = SpeedEstimator::new();
                        let mut last_inst = tokio::time::Instant::now();
                        loop {
                            interval.tick().await;
                            if let Some(c) = &retry_pump_cancel { if c.is_cancelled() { break; } }
                            let bytes = retry_pump_counter.load(Ordering::Relaxed);
                            let now = tokio::time::Instant::now();
                            let spd = est.update(bytes, now - last_inst, 0.4, Duration::from_millis(300));
                            last_inst = now;
                            let _ = retry_pump_tx.send(Progress {
                                downloaded_bytes: bytes,
                                total_bytes: None, // 重下期间不设 total（避免与首次 total 冲突）
                                speed_bps: Some(spd),
                            }).await;
                            if bytes >= total { break; }
                        }
                    });
                    let retry_result = self.download_chunked(
                        url,
                        &part,
                        new_segs,
                        Arc::clone(&counter),
                        cancel.cloned(),
                        None,
                        None,
                    ).await;
                    retry_pump.abort();
                    // 重下失败：清理 sidecar 后返回错误（不跳过清理）
                    if let Err(e) = retry_result {
                        let _ = std::fs::remove_file(&part);
                        crate::core::resume::remove(&task.dest);
                        return Err(e);
                    }
                }
            }
            if !verify_ok {
                let actual = match expected {
                    Hash::Sha256(_) => crate::core::verify::compute_sha256(&part).await.unwrap_or_default(),
                    Hash::Etag(_) => String::new(),
                };
                let _ = std::fs::remove_file(&part);
                crate::core::resume::remove(&task.dest);
                return Err(DownloadError::HashMismatch {
                    path: task.dest.clone(),
                    expected: format!("{expected:?}"),
                    actual,
                });
            }
        }

        // 原子转正
        std::fs::rename(&part, &task.dest)?;
        crate::core::resume::remove(&task.dest);
        let _ = progress
            .send(Progress {
                downloaded_bytes: total,
                total_bytes: Some(total),
                speed_bps: None,
            })
            .await;
        Ok(())
    }
}

/// .part 路径：dest + ".part"
pub(crate) fn part_path(dest: &Path) -> PathBuf {
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

    use std::io::{Seek, SeekFrom, Write, BufWriter};
    let file = std::fs::OpenOptions::new().write(true).open(part_path)?;

    let mut writer = BufWriter::with_capacity(cfg.buf_kb * 1024, file);
    let mut stream = resp.bytes_stream();
    let mut written_this_call: u64 = 0;

    // RAII counter guard：drop 时若未 commit，自动 fetch_sub 回滚本次累加的字节。
    // 统一覆盖所有中途失败路径（reqwest 错误 / Io 错误 / timeout / 流提前结束），
    // 避免 counter 虚高 >100%。
    struct CounterGuard<'a> {
        counter: &'a AtomicU64,
        amount: u64,
        committed: bool,
    }
    impl Drop for CounterGuard<'_> {
        fn drop(&mut self) {
            if !self.committed && self.amount > 0 {
                self.counter.fetch_sub(self.amount, Ordering::Relaxed);
            }
        }
    }
    let mut counter_guard = CounterGuard { counter, amount: 0, committed: false };

    // 宏：累加 written + counter + guard，避免每处手写 3 行
    macro_rules! add_written {
        ($n:expr) => {{
            let n = $n as u64;
            written_this_call += n;
            counter.fetch_add(n, Ordering::Relaxed);
            counter_guard.amount += n;
        }};
    }

    if status == 200 {
        // 200 全文：服务端忽略 Range，返回整个文件。
        // 本段只需 [seg.begin, seg.end] 区间的字节——
        // 先 seek 到段的起始位置，然后跳过流中 seg.begin 个字节。
        writer.seek(SeekFrom::Start(seg.begin))?;
        let mut skipped: u64 = 0;
        while skipped < seg.begin {
            if let Some(c) = cancel { if c.is_cancelled() { return Err(DownloadError::Cancelled); } }
            match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
                Ok(Some(chunk)) => {
                    let chunk = chunk.map_err(map_reqwest_transient)?;
                    let remain = (seg.begin - skipped) as usize;
                    if chunk.len() > remain {
                        // chunk 跨越 skip 边界——保留 remain 之后的部分作为段数据
                        let data = &chunk[remain..];
                        let seg_remain = ((seg.end - seg.begin + 1) - written_this_call) as usize;
                        let write_len = data.len().min(seg_remain);
                        if write_len > 0 {
                            writer.write_all(&data[..write_len])?;
                            add_written!(write_len);
                        }
                        break;
                    } else {
                        skipped += chunk.len() as u64;
                    }
                }
                Ok(None) => break, // 流提前结束
                Err(_) => return Err(transient(TransientKind::Timeout, "stream skip timeout")),
            }
        }
        // 继续读取段数据
        let seg_capacity = seg.end - seg.begin + 1;
        while written_this_call < seg_capacity {
            match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
                Ok(Some(chunk)) => {
                    if let Some(c) = cancel { if c.is_cancelled() { return Err(DownloadError::Cancelled); } }
                    let chunk = chunk.map_err(map_reqwest_transient)?;
                    let remain = (seg_capacity - written_this_call) as usize;
                    let write_len = chunk.len().min(remain);
                    if write_len == 0 { continue; } // 空 chunk 不代表流结束
                    writer.write_all(&chunk[..write_len])?;
                    add_written!(write_len);
                }
                Ok(None) => break,
                Err(_) => return Err(transient(TransientKind::Timeout, "stream read timeout")),
            }
        }
    } else {
        // 206 续传：从 start 位置开始写入
        writer.seek(SeekFrom::Start(start))?;
        loop {
            match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
                Ok(Some(chunk)) => {
                    if let Some(c) = cancel { if c.is_cancelled() { return Err(DownloadError::Cancelled); } }
                    let bytes = chunk.map_err(map_reqwest_transient)?;
                    writer.write_all(&bytes)?;
                    add_written!(bytes.len());
                }
                Ok(None) => break,
                Err(_) => return Err(transient(TransientKind::Timeout, "stream read timeout")),
            }
        }
    }
    writer.flush()?;
    // 流提前结束校验：200 路径需写满 seg_capacity；206 路径需写满段剩余（end - start + 1）。
    // 不足说明服务端提前断流（网络中断/CDN 错误），返回 transient 触发段级重试。
    let expected_written = if status == 200 {
        seg.end - seg.begin + 1
    } else {
        end - start + 1
    };
    if written_this_call < expected_written {
        // guard drop 自动回滚 counter（未 commit）
        return Err(transient(TransientKind::Network, format!(
            "stream ended early: wrote {} of {} bytes for segment [{},{}]",
            written_this_call, expected_written, seg.begin, seg.end
        )));
    }
    // 200 截断：downloaded = 段大小；206 续传则累加
    counter_guard.committed = true; // 成功——guard 不回滚
    let new_downloaded = if status == 200 { written_this_call } else { seg.downloaded + written_this_call };
    Ok(Segment { begin: seg.begin, end: seg.end, downloaded: new_downloaded })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{MockServer, Method};
    use tempfile::tempdir;
    use tokio::sync::mpsc;

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

    #[tokio::test]
    async fn download_segment_200_truncates_to_segment_range() {
        // 服务端忽略 Range，返回 200 全文（30 字节），段 [10,19] 仅应写入 10 字节
        let server = MockServer::start();
        let full_body: Vec<u8> = (0..30u8).collect();
        let total_len = full_body.len() as u64;
        server.mock(|when, then| {
            when.method(Method::GET).path("/f");
            then.status(200).body(full_body.clone());
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _file = Downloader::ensure_part_file(&dest, total_len).unwrap();
        let part = part_path(&dest);
        let seg = Segment { begin: 10, end: 19, downloaded: 0 };
        let counter = AtomicU64::new(0);
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let out = dl
            .download_segment(&server.url("/f"), &part, seg, &counter, None)
            .await
            .unwrap();
        // downloaded 应 = 段大小 10，不是全文 30
        assert_eq!(out.downloaded, 10, "200 路径应截断为段大小");
        // .part 大小应 = total（预分配不变）
        let written = std::fs::read(&part).unwrap();
        assert_eq!(written.len(), total_len as usize, ".part 大小应 = total");
        // seg.begin=10 处应写入全文 [10,19] 字节（非 [0,9]——200 全文需跳过前 10 字节）
        assert_eq!(&written[10..20], &full_body[10..20], "段区间内容正确（200 跳过 offset 前字节）");
        // 成功路径：counter 应 = 段大小 10（guard committed 不回滚）
        assert_eq!(counter.load(Ordering::Relaxed), 10, "成功路径 counter = 段大小");
    }

    #[tokio::test]
    async fn download_segment_short_stream_returns_transient() {
        // 服务端返回 206 但 body 比声明的段短（只发 80 字节，段需 100）→ 应返回 Transient 而非 Ok
        let server = MockServer::start();
        let short_body = vec![0xABu8; 80];
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-99");
            then.status(206)
                .header("Content-Range", "bytes 0-99/100")
                .body(short_body.clone());
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _ = Downloader::ensure_part_file(&dest, 100).unwrap();
        let part = part_path(&dest);
        let seg = Segment { begin: 0, end: 99, downloaded: 0 };
        let counter = AtomicU64::new(0);
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let result = dl.download_segment(&server.url("/f"), &part, seg, &counter, None).await;
        // download_segment 有段级重试（max_retries_per_segment=3），每次都短 body，
        // 最终重试耗尽返回 Transient。不是 Ok（静默成功）也不是 HashMismatch。
        assert!(result.is_err(), "短流应返回错误而非静默成功");
        let err = result.unwrap_err();
        match err {
            DownloadError::Transient { .. } => {} // 正确：transient 触发段级重试
            other => panic!("期望 Transient，得到 {:?}", other),
        }
        // 守护 counter 回滚：每次重试收到 80B 后 Transient 回滚，重试耗尽后 counter 应为 0。
        // 若 RAII guard 被改坏（回滚失效），counter 会虚高（80*4=320）。
        assert_eq!(counter.load(Ordering::Relaxed), 0, "中途失败后 counter 应回滚到 0");
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
        let server = MockServer::start();
        // 100 字节，分 2 段（每段 50）
        let total: u64 = 100;
        let body: Vec<u8> = (0..total as u8).collect();
        let half = 50u64;
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes=0-{}", half - 1));
            then.status(206).body(&body[0..half as usize]);
        });
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes={half}-{}", total - 1));
            then.status(206).body(&body[half as usize..total as usize]);
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let _ = Downloader::ensure_part_file(&dest, total).unwrap();
        let part = part_path(&dest);
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let segs = plan_segments(total, true, half, 0, 2); // threshold=0 强制多段
        assert_eq!(segs.len(), 2);
        let counter = Arc::new(AtomicU64::new(0));
        let done = dl
            .download_chunked(&server.url("/f"), &part, segs, counter, None, None, None)
            .await
            .unwrap();
        assert!(done.iter().all(|s| s.is_done()));
        let written = std::fs::read(&part).unwrap();
        assert_eq!(written, body);
    }

    #[tokio::test]
    async fn download_end_to_end_single_segment_verify_rename() {
        let server = MockServer::start();
        let body = b"hello-download-crate"; // 20 bytes
        let body_len = body.len() as u64;
        // SHA256 of body
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(body);
        let hex: String = h.finalize().iter().map(|b| format!("{:02x}", b)).collect();

        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-0");
            then.status(206)
                .header("Content-Range", format!("bytes 0-0/{body_len}"))
                .header("Accept-Ranges", "bytes");
        });
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", format!("bytes=0-{}", body_len - 1));
            then.status(206).body(body);
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let task = DownloadTask {
            url: server.url("/f"),
            mirrors: vec![],
            dest: dest.clone(),
            expected_hash: Some(Hash::Sha256(hex)),
            ..Default::default()
        };
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        dl.download(&task, tx, None).await.unwrap();
        // dest 已 rename 落地
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 进度收到 total
        let last = rx.recv().await.unwrap();
        assert_eq!(last.total_bytes, Some(body_len));
    }

    #[tokio::test]
    async fn download_mirror_fallback_on_500() {
        let bad = MockServer::start();
        let good = MockServer::start();
        bad.mock(|when, then| {
            when.method(Method::GET).path("/f");
            then.status(500);
        });
        good.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-0");
            then.status(206).header("Content-Range", "bytes 0-0/5").header("Accept-Ranges", "bytes");
        });
        good.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-4");
            then.status(206).body(b"hello");
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let task = DownloadTask {
            url: bad.url("/f"),
            mirrors: vec![good.url("/f")],
            dest,
            expected_hash: None, ..Default::default()
        };
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        dl.download(&task, tx, None).await.unwrap();
    }

    #[tokio::test]
    async fn download_cancelled_returns_cancelled() {
        let server = MockServer::start();
        // probe 延迟 300ms；取消在 100ms 发生 → download_chunked 段任务首检即 Cancelled
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-0");
            then.status(206)
                .header("Content-Range", "bytes 0-0/1000000")
                .header("Accept-Ranges", "bytes")
                .delay(Duration::from_millis(300));
        });
        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        let task = DownloadTask {
            url: server.url("/f"),
            mirrors: vec![],
            dest,
            expected_hash: None, ..Default::default()
        };
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            t2.cancel();
        });
        let (tx, _rx) = mpsc::channel(16);
        let err = dl.download(&task, tx, Some(token)).await.unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
    }

    #[tokio::test]
    async fn download_drops_multi_segment_sidecar_when_source_no_range() {
        // 多段 sidecar（前次支持 Range 的源崩溃遗留）遇到不支持 Range 的源：
        // 必须丢弃 sidecar 改单段，否则会向不支持 Range 的服务器发分段 Range 请求。
        let server = MockServer::start();
        let total: u64 = 100;
        let body: Vec<u8> = (0..total as u8).collect();

        // probe：返回 total 但不带 Accept-Ranges → accept_ranges=false
        server.mock(|when, then| {
            when.method(Method::GET).path("/f").header("Range", "bytes=0-0");
            then.status(206).header("Content-Range", format!("bytes 0-0/{total}"));
        });
        // 仅 mock 单段全文件请求。若 guard 未生效（沿用多段 sidecar），
        // 会发 bytes=0-49 / bytes=50-99 两段，二者未 mock → download 失败。
        server.mock(|when, then| {
            when.method(Method::GET)
                .path("/f")
                .header("Range", format!("bytes=0-{}", total - 1));
            then.status(206).body(body.clone());
        });

        let dir = tempdir().unwrap();
        let dest = dir.path().join("f");
        // 预置多段 sidecar（模拟前次崩溃遗留）
        let multi = crate::core::resume::new_state(
            &dest,
            total,
            None,
            vec![
                Segment { begin: 0, end: 49, downloaded: 0 },
                Segment { begin: 50, end: 99, downloaded: 0 },
            ],
        );
        crate::core::resume::save(&dest, &multi).unwrap();

        let task = DownloadTask {
            url: server.url("/f"),
            mirrors: vec![],
            dest: dest.clone(),
            expected_hash: None, ..Default::default()
        };
        let dl = Downloader::new(DownloadConfig::default()).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        dl.download(&task, tx, None).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }
}
