# 模型下载 crate 设计（octopus-download）

> **状态**：已批准（2026-06-21）。本 spec 是实现权威，后续 plan / 代码以此为准。
> **关联**：参考项目 `omniget`（`/Users/wudarui/workspace/agent/omniget`）、`mangofetch`（`/Users/wudarui/workspace/agent/mangofetch`），二者软链在主项目根，仅参考其 Rust 下载实现。

## Goal

新建一个**通用文件下载 crate**（`crates/download/`，包名 `octopus-download`），支持**分块并发 + 断点续传 + 完整性校验 + 镜像 fallback**。首要用途是替代 `huggingface-cli` 下载 ASR 大模型（解决终端用户门槛、国内镜像、按需选 `int8` 文件三个痛点），但 crate 本身不耦合 HuggingFace——HF 逻辑放在同 crate 的 `hf/` 模块，核心 `core/` 模块零 HF 知识。

## 背景与痛点

当前大模型（whisper / sensevoice / qwen3 / paraformer / zipformer-xlarge 等）下载方式（`docs/configuration.md:295-310`）：

```bash
pip install huggingface_hub
huggingface-cli download <repo>
```

三个痛点：
1. **终端用户门槛高**：要装 Python + pip + hf-cli 才能用大模型，对非专业用户不可接受。
2. **国内镜像**：`huggingface.co` 在国内访问墙/慢，hf-cli 需切 `HF_ENDPOINT=https://hf-mirror.com`，步骤易漏。
3. **整仓下载**：hf-cli 不加 `--include` 会拉整个 repo（含 `*_fp16.onnx`、`*_merged.onnx` 等不需要的文件），但实际只需 `int8` 量化文件。

本 crate 用 Rust 内置下载，参数化 `source-url`（镜像）+ `include/exclude` glob（选文件），断点续传 + 校验，无需 Python。

## 方案概述

**单 crate 两模块**：
- `src/core/`：通用下载器。输入 `(url, mirrors, dest, expected_hash)`，输出"文件下到 dest + 校验通过"。**不识 HF、不识 glob**。
- `src/hf/`：HF 适配层（依赖 core）。输入 `(repo, include, exclude, source_url, target_dir)`，调 HF API 列文件 → glob 过滤 → 构造 resolve URL + 提取 hash → 产出 `Vec<DownloadTask>` 交 core 下载。

**统一 segment 架构**（避免返工）：单文件 = 1 个 segment，是分块的退化。同一套代码路径（probe → 规划 segments → `set_len` 预分配 → 并发 Range+seek 写 → 进度汇总 → 校验 → rename）。单段时并发数=1，多段时并发加速。从单流"启用"分块只是改规划阈值。

**持久化两层分离**：
- 单文件断点续传：sidecar `<dest>.part.resume.json`（段级进度，和 `.part` 绑定，崩溃安全，下完即删）。**不进 sqlite**。
- 模型级管理（已下哪些、版本、校验状态）：属应用层集成（后续 task，扩 `models` 表），**不在本 MVP**。

## 数据流

以 `onnx-community/whisper-small.en` 为例：

```
输入: repo=onnx-community/whisper-small.en
      include=['*','onnx/*_int8.onnx']  exclude=['*/*','onnx/*_merged_int8.onnx']
      source_url=https://hf-mirror.com   target_dir=~/.octopus/models
   │
   ▼  [HF 适配层 src/hf/]
   1. GET {source_url}/api/models/{repo}
      → siblings[].rfilename + siblings[].etag + siblings[].lfs.oid(sha256, LFS 文件有)
   2. 对每个 rfilename 应用 include/exclude glob（对齐 hf-cli 语义）
      → 选中文件集
   3. 每个选中文件：
      - resolve URL = {source_url}/{repo}/resolve/main/{path}
      - mirrors     = [镜像URL, 官方URL(https://huggingface.co/...)]  # 镜像优先 fallback 官方
      - dest        = {target_dir}/{repo}/{path}
      - expected_hash = LFS 文件用 Sha256(lfs.oid)；非 LFS 小文件用 Etag
   │
   ▼  产出 Vec<DownloadTask>
   │
   ▼  [通用 core src/core/]
   逐文件 Downloader::download(task, progress_tx, cancel):
     1. probe: GET Range bytes=0-0（或 HEAD）→ total_size, accept_ranges, etag
        镜像 fallback：主源失败试 mirrors 下一个
     2. plan_segments(total, accept_ranges):
        - !accept_ranges || total 未知 || total < CHUNK_THRESHOLD → 1 段 [0, total)
        - else → N 段，每段 ~SEGMENT_SIZE，N = min(段数, MAX_CONCURRENT)
     3. load sidecar（若存在）: 三重校验 type/total_bytes/url_hash → 复用各段 downloaded；不符则丢弃重新规划
     4. ensure_part_file(dest.part): set_len(total) 预分配 sparse 文件
     5. 并发执行 segments（JoinSet + Semaphore，单段时并发=1）:
        each segment:
          - Range: bytes={begin+downloaded}-{end}
          - （不注入 If-Range：最终整文件 hash 校验兜底内容变更；注入反而让不支持它的镜像回退 200 全文重传）
          - seek(offset) + write，BufWriter 256KB
          - 段级重试（MAX_RETRIES_PER_SEGMENT，指数 backoff + jitter）
        progress pump: AtomicU64 fetch_add → 后台 task 250ms 推 mpsc::Sender<Progress>
        sidecar 回写: 段完成时（join_next）快照各段 downloaded，原子写（tmp+rename）
        cancel: CancellationToken 贯穿，取消时 abort 全部段
     6. 全部段 done → SHA256/etag 校验(expected_hash)
          - 失败：重试整文件下载 MAX_VERIFICATION_RETRIES 次，仍失败报 HashMismatch
     7. rename(dest.part → dest) + remove(sidecar)
```

## crate 结构

```
crates/download/
├── Cargo.toml
└── src/
    ├── lib.rs              # 导出 core + hf 公共 API
    ├── core/
    │   ├── mod.rs          # Downloader, DownloadTask, DownloadConfig
    │   ├── downloader.rs   # download() 主编排（probe→plan→并发→校验→rename）
    │   ├── segment.rs      # Segment 结构 + plan_segments
    │   ├── resume.rs       # sidecar 加载/保存/三重校验（原子写）
    │   ├── verify.rs       # If-Range 续传校验 + SHA256/etag 完整性校验
    │   ├── progress.rs     # Progress 结构 + mpsc + 节流
    │   └── error.rs        # DownloadError（thiserror）
    └── hf/
        ├── mod.rs          # HfRequest, resolve_tasks
        ├── api.rs          # GET /api/models/{repo} 解析 siblings
        ├── glob.rs         # include/exclude 过滤（对齐 hf-cli fnmatch）
        └── resolve.rs      # 构造 resolve URL + 镜像 + hash 提取
```

**核心模块（`core/`）不 import `hf/`**。`hf/` 依赖 `core/`。将来下别的源（非 HF）加新顶层模块即可。

## 核心 API

```rust
// ===== src/core/ =====

/// 期望校验值
#[derive(Debug, Clone)]
pub enum Hash {
    Sha256(String),   // hex
    Etag(String),
}

/// 单文件下载任务
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,              // 主源（通常是镜像）
    pub mirrors: Vec<String>,     // 备选源（含官方源），顺序 fallback
    pub dest: PathBuf,            // 最终落地路径
    pub expected_hash: Option<Hash>,
}

/// 下载器配置
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub connect_timeout: Duration,        // 默认 10s
    pub read_timeout: Duration,           // 默认 45s（单段无数据超时）
    pub segment_size: u64,                // 默认 4 MiB
    pub chunk_threshold: u64,             // 默认 16 MiB，小于此走单段
    pub max_concurrent: usize,            // 默认 8，clamp(1, 32)
    pub max_retries_per_segment: u32,     // 默认 3
    pub backoff_base: Duration,           // 默认 1s（指数：base * 2^attempt + jitter）
    pub max_verification_retries: u32,    // 默认 2（整文件校验失败重下次数）
    pub buf_kb: usize,                    // 默认 256
}
impl Default for DownloadConfig { /* 上述默认值 */ }

/// 进度上报（mpsc，不持久化）
#[derive(Debug, Clone)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,    // EMA 估算
}

/// 下载器
pub struct Downloader {
    client: reqwest::Client,       // rustls-tls, default-features=false
    config: DownloadConfig,
}
impl Downloader {
    pub fn new(config: DownloadConfig) -> Result<Self, DownloadError>;
    /// 下载单个 task。progress 实时推（250ms 节流）。cancel 可选。
    pub async fn download(
        &self,
        task: &DownloadTask,
        progress: tokio::sync::mpsc::Sender<Progress>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<(), DownloadError>;
}

// ===== src/hf/ =====

pub struct HfRequest {
    pub repo: String,                      // 如 "onnx-community/whisper-small.en"
    pub include: Vec<String>,              // glob 模式，多个 = OR
    pub exclude: Vec<String>,              // glob 模式，多个 = OR，优先于 include
    pub source_url: Option<String>,        // 如 "https://hf-mirror.com"；None=官方源
    pub target_dir: PathBuf,               // 默认 ~/.octopus/models
}
/// 解析 HF 请求为下载任务列表（调 API + glob + 构造 URL/hash）
pub async fn resolve_tasks(
    client: &reqwest::Client,
    req: HfRequest,
) -> Result<Vec<DownloadTask>, DownloadError>;
```

## 断点续传（sidecar）

文件 `<dest>.part.resume.json`，格式：

```json
{
  "type": "octopus-segmented",
  "url_hash": "<sha256(dest 路径) 前 16 hex，镜像无关>",
  "total_bytes": 12345678,
  "etag": "<probe etag，当前未注入 If-Range、靠 hash 兜底；保留字段供未来启用>",
  "segments": [
    {"begin": 0, "end": 4194303, "downloaded": 4194304},
    {"begin": 4194304, "end": 8388607, "downloaded": 1000000}
  ]
}
```

- **加载时三重校验**（任一不符即丢弃 sidecar、重新规划）：`type == "octopus-segmented"` && `total_bytes == probe 总长` && `url_hash == sha256(dest 路径)`。`url_hash` 基于 **dest 路径**而非 url——故换镜像（dest 不变）不触发重下，仅换目录/目标文件才失效（符合预期）。另：多段 sidecar 遇不支持 Range 的源（`accept_ranges=false`）会丢弃重规划为单段——否则会向不支持 Range 的服务器发分段请求、注定 200 全文错位。
- **原子写**：先写 `<dest>.part.resume.json.tmp` 再 `rename` 覆盖，崩溃时不留半截 JSON。
- **节奏**：段完成时（`download_chunked` 的 `join_next`）快照各段 `downloaded` 写一次（非独立定时 pump）。
- **清理**：下载成功 `rename(.part→dest)` 后 `remove_file(sidecar)`；致命错误（4xx）删 `.part` + sidecar；瞬时错误（5xx/超时）保留 `.part` + sidecar 待续传。
- **单段也记 sidecar**：保持架构统一（单段 = segments.len()==1 的特例），续传逻辑一套。

## 分块机制

- **规划**（`plan_segments`）：`accept_ranges && total >= chunk_threshold` → 多段；否则 1 段。段数 `N = min(total.div_ceil(segment_size), max_concurrent)`，余数逐段均摊。
- **预分配**：`ensure_part_file` 用 `File::set_len(total)` 打 sparse 洞，各段 `seek(begin + downloaded) + write` 直写最终位置——**无需下载完再合并**。
- **并发**：`tokio::task::JoinSet` + `Arc<Semaphore::new(max_concurrent)>`。每段任务 acquire 后才发请求。
- **进度汇总**：`Arc<AtomicU64>`，每段写一段 `fetch_add`，后台 pump 250ms 读总值推 mpsc + 算 EMA 速度。
- **段级重试**：每段独立重试 `max_retries_per_segment` 次，指数 backoff（`backoff_base * 2^attempt`）+ jitter。段失败回滚该段已计入的进度（减去 downloaded）。
- **响应判定**：`206` → 续写；`416` → 删该段进度从头；`200`（服务端忽略 Range）→ 当单流处理（truncate 重写该段）；其余按错误分类。
- **work-stealing 不做**（YAGNI）：模型源带宽稳定，静态分段 + 段级重试足够。

## 校验（补两参考项目的漏）

两参考项目都**没用 `If-Range`/`ETag`** 校验续传有效性，只靠 `content-length`/`url_hash`——同 URL 内容被替换会产出损坏文件。本设计补上：

1. **续传有效性**（`If-Range`）：Range 请求带 `If-Range: <etag>`。服务端返回 `206` = 内容未变、续传有效；返回 `200` = 内容已变、全量重下（truncate `.part`）。
2. **完整性**（SHA256/etag）：全段完成后，LFS 文件算 SHA256 比 `expected_hash=Sha256(lfs.oid)`；非 LFS 小文件比 `Etag`。`spawn_blocking` 流式 hash（8KB buffer，避免阻塞 runtime）。
3. **失败处理**：校验失败重下整文件 `max_verification_retries` 次，仍失败报 `HashMismatch`（删 `.part` + sidecar）。

## 镜像（source-url）

- HF 适配层接收 `source_url`（如 `https://hf-mirror.com`），用它替换官方域名生成镜像 URL。
- `task.mirrors = [镜像URL, 官方URL]`：镜像优先，失败 fallback 官方源。
- list API（`/api/models/{repo}`）也走 `source_url`（镜像需代理 `/api`，hf-mirror 支持）。
- core 层 `download()` 主源（`task.url`，通常是镜像）失败 → 依次试 `task.mirrors`。

## glob（对齐 hf-cli，关键风险点）

- hf-cli 的 `--include`/`--exclude` 用 Python `fnmatch`（Unix shell 风格：`*` `?` `[...]`）。
- 语义：多个 include = **任一匹配则包含（OR）**；多个 exclude = **任一匹配则排除（OR）**；**exclude 优先于 include**（先 include 选出，再 exclude 剔除）。
- path 相对 repo 根（如 `onnx/model_int8.onnx`）。
- **Rust 实现**：用 `glob` crate 或手写 fnmatch，但**必须与 hf-cli 实测对齐**（尤其 `*` 是否跨 `/`、`[...]` 字符类）。
- **测试**：用真实 `huggingface-cli download <repo> --include ... --exclude ... --dry-run`（hf-cli 支持 dry-run 列出将下载的文件）输出做 golden test，确保同样参数选出相同文件集。

## 目录布局

- `{target_dir}/{repo}/{path}`，`repo` 中的 `/` 作路径分隔。
- 默认 `~/.octopus/models/onnx-community/whisper-small.en/onnx/model_int8.onnx`。
- 复刻 repo 结构，多 repo 不冲突，路径有意义。
- MVP **不做 commit pinning**：直接 `resolve/main/{path}`（最新），校验靠 etag/sha256。版本管理（pin 特定 commit）后续。

## 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("fatal: HTTP {status} for {url}")]
    Fatal { status: u16, url: String },
    #[error("transient ({kind}): {message}")]
    Transient { kind: TransientKind, message: String },
    #[error("cancelled")]
    Cancelled,
    #[error("hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch { path: PathBuf, expected: String, actual: String },
    #[error("hf api error: HTTP {status} for {url}")]
    HfApi { status: u16, url: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
}

pub enum TransientKind {
    ServerError,   // 5xx
    RateLimited,   // 429
    Timeout,       // read/connect timeout
    Network,       // connection reset, dns, etc.
}
```

- **Fatal**（4xx 除 408/429）：不重试，删 `.part` + sidecar。
- **Transient**（5xx / 408 / 429 / 超时 / 网络）：重试 + 指数 backoff + jitter。
- **Cancelled**：CancellationToken 触发，停止。
- 不用参考项目的"字符串匹配 error message"分类法——按 `StatusCode` + `io::ErrorKind` 分类。

## 依赖（Cargo.toml）

```toml
[dependencies]
reqwest = { version = "0.12", default-features = false,
            features = ["stream", "http2", "rustls-tls"] }   # 对齐 workspace 现有版本
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }          # CancellationToken
futures = "0.3"
sha2 = "0.10"
thiserror = "<对齐 workspace>"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
glob = "0.3"                                                  # 或 fnmatch 等价实现
tracing = "<对齐 workspace>"

[dev-dependencies]
httpmock = "0.8"                                              # mock HTTP，对齐参考项目（mangofetch 0.8.3）
tokio = { version = "1", features = ["full", "test-util"] }
```

> 注：具体版本号在 plan 阶段对齐 workspace 现有（`cargo tree` / 各 crate Cargo.toml），避免引入重复版本。

## 测试策略

**core**（httpmock mock 服务器）：
- 单段（小文件）下载成功
- 多段（大文件）分块并发下载成功
- 断点续传：模拟中断（保留 `.part` + sidecar），重启后从段进度恢复
- `If-Range` 校验：mock 返回 206（续传）/ 200（内容变，重下）
- SHA256 校验：成功 / 失败（mock 错误内容）/ 失败重试
- 镜像 fallback：主源 500，镜像 200
- 取消：CancellationToken 触发后停止
- 错误分类：4xx→Fatal、5xx→Transient、超时→Transient
- sidecar 三重校验：total 不符 / url_hash 不符 → 丢弃重新规划

**hf**：
- glob 对齐 hf-cli：**golden test**，用真实 `huggingface-cli download <repo> --include ... --exclude ... --dry-run` 输出做期望（`include`/`exclude` 多组合）
- resolve URL 构造：镜像域名替换正确
- API 解析：mock `/api/models/{repo}` 返回 siblings，正确提取 rfilename / etag / lfs.oid

> glob golden test 的期望文件生成：`huggingface-cli download onnx-community/whisper-small.en --include '*' 'onnx/*_int8.onnx' --exclude '*/*' 'onnx/*_merged_int8.onnx' --dry-run`（需 Python 环境，仅生成期望时用，非 crate 运行时依赖）。

## MVP 边界（不含）

- **CLI**：先只做 lib。CLI 形态（独立 binary / `octopus-cli` 子命令）后续讨论。
- **sqlite 管理**：不建表。模型级管理（已下哪些、版本、校验状态）属应用层，后续与 `resolve_model_dir` 集成一起做。
- **`resolve_model_dir` 扩展**：现有只查 `~/.cache/huggingface/hub/`，需扩展支持 `~/.octopus/models/` + DB `models` 表 source 处理，才能让下载的模型被加载。**这是紧接的后续 task**（下载 lib 本身不依赖它）。
- **work-stealing**：YAGNI。
- **commit 版本 pinning**：直接 `resolve/main/`，后续。

## 后续工作（spec 外，记录待办）

1. CLI 形态设计（独立 binary vs `octopus-cli download model` 子命令）。
2. 应用层集成：`resolve_model_dir` 扩展支持 `~/.octopus/models/` + `models` 表（local_path / commit / verified / 文件清单）。
3. 下载任务队列 / 历史 / GUI 显示（如需要，sqlite 应用层）。
4. 可能的 work-stealing（仅当实测出现"某段卡死拖慢全局"才加）。
5. commit 版本 pinning（若需固定模型版本）。

## 关键设计决策记录（权衡动机）

- **统一 segment 架构 vs 先单流后分块**：选统一架构。单流 = 1 segment 退化，后续无返工。返工的唯一来源是"单流用 append、分块用 set_len+seek"写法不一——本设计单段也用 `set_len+seek` + sidecar，彻底消除。
- **sidecar vs sqlite 存进度**：选 sidecar。与 `.part` 强绑定、崩溃安全、下完即删、保持 crate 通用（无 sqlite 依赖）。sqlite 留给应用层模型管理。
- **`If-Range`/ETag 续传校验**：两参考项目都漏，本设计补上——HF 模型可能重传同名文件，必须防内容被换。
- **类型化错误 vs 字符串匹配**：两参考项目用 `anyhow.to_string().contains("HTTP 4xx")`（脆弱），本设计用 `thiserror` enum 按 `StatusCode`/`ErrorKind` 分类。
- **纯通用 core + HF 适配层分离**：呼应"不限于此"。core 零 HF 知识，将来下别的源加模块即可。
