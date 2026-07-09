# octopus-dlp 下载转码 sidecar — 设计

- 日期：2026-07-09（回顾性补 spec，crate 已实现稳定）
- 分支：worktree-system-status-page
- 状态：已实现（回顾性文档化）

## 背景

`octopus-dlp` 是独立子进程二进制，把在线音视频 URL 转成 ASR 可消费的 16kHz mono PCM。cli `transcribe-url` 子命令（URL → 文本转写）spawn 它取音频流。封装 `yt-dlp`（下载）+ `ffmpeg`（转码）两个外部 binary，统一处理依赖管理、缓存复用、临时文件清理。

## 职责

- 在线音视频 → 16kHz mono f32le PCM 流（stdout，默认）或 WAV 文件（`-o`）
- 自动管理 yt-dlp binary（缺失则下载到 `~/.octopus/bin`）
- 检查 ffmpeg（缺失提示用户装，不自动下载）
- 下载文件 MD5 缓存复用（`--unclear` 命中缓存跳过下载 + 转码后保留）
- 转码后清理临时下载文件（RAII，转码失败也清）

## 非目标

- 视频画面下载（仅音频流 `-f ba/b`）
- 非 ASR 格式（仅 16kHz mono，为 ASR 定制）
- 并发多 URL
- ffmpeg 自动下载（体积大、平台包管理差异大，让用户装）
- 库化（sidecar 子进程，非 crate 内嵌）—— 见「关键决策」1

## 调用场景

cli `octopus transcribe-url <URL>` → spawn `octopus-dlp <URL>` → stdout PCM 流回 cli → cli 送 ASR engine → 输出文本。或 `-o file.wav` 落 WAV 文件供离线复用。

## 架构

```
cli transcribe-url
  ├ 找 octopus-dlp binary（exe 同目录 → ~/.octopus/bin → cargo run 兜底）
  ├ spawn: octopus-dlp <URL> [--unclear] [-o <FILE>]
  │    stdout=piped（流模式）/ null（文件模式），stderr=piped
  ├ stderr_task: 逐行读
  │    首行 VideoMeta JSON → 打印视频信息 + 存 duration（进度估算）
  │    其余 → [dlp] 前缀转发
  ├ stdout: f32le PCM 流 → leftover 跨 read 拼接对齐 → collect samples
  └ wait + status 检查

octopus-dlp（子进程）
  ├ prepare_dependencies（yt-dlp 自动下载 / ffmpeg 检查）
  ├ yt-dlp --dump-json → 元数据（title/duration/uploader/_filename）
  ├ stderr 首行 eprintln 元数据 JSON（VideoMetadataOutput）
  ├ yt-dlp -f ba/b 下载音频（--unclear + 文件存在 → 跳过）
  ├ ffmpeg -i 下载文件 → f32le PCM stdout / WAV 文件
  └ RAII DownloadedFileGuard drop 清理临时下载文件
```

## 后端设计

### 依赖管理（`prepare_dependencies`）

- `get_binary_path(name)`：先 `~/.octopus/bin/<name>`（Windows 加 `.exe`），再系统 PATH（`which`/`where`）
- **yt-dlp**：缺失则自动下载 GitHub releases latest（Unix `yt-dlp` / Windows `yt-dlp.exe`）到 `~/.octopus/bin/`，Unix 设 `0o755`
- **ffmpeg**：仅检查存在，缺失按平台提示（macOS `brew install` / Windows 官网 / Linux `apt install`），**不自动下载**（体积大 + 平台包管理差异）
- **`download_file`（binary 下载）**：reqwest GET，超时 300s，`MAX_DOWNLOAD_SIZE=200MB`，流式累计 size 检查；**`.part` 临时文件 + 原子 rename**——防下载中断（断网/超时/中止）残留半成品 binary 被 `get_binary_path` 的 `exists()` 误判就绪、跳过重下导致永远执行损坏 binary。`.part` 残留无害（dest 仍不存在，下次重下 `create(.part)` 自动 truncate 覆盖）

### 主流程

1. `work_dir = ~/.octopus/tmp`，`url_md5 = md5(url)` → 缓存模板 `~/.octopus/tmp/{md5}.%(ext)s`
2. `yt-dlp --dump-json -f ba/b -o {template}` 取元数据 JSON（含 `_filename` 实际下载文件名）
3. stderr **首行** `eprintln` 元数据 JSON（`VideoMetadataOutput { title, duration, author }`）——分离 stdout/stderr 避免元数据混入 PCM 字节流
4. `yt-dlp -f ba/b -o {template}` 下载音频流；`--unclear` 且文件已存在则跳过（缓存复用）
5. `ffmpeg -y -i {下载文件}` 转码：
   - 流模式（默认）：`-f f32le -ar 16000 -ac 1 -c:a pcm_f32le -`，stdout = PCM 流，stderr null
   - 文件模式（`-o`）：`-f wav -ar 16000 -ac 1 {path}`，stdout/stderr null
6. RAII 清理（见下）

### 缓存复用

下载文件名 = URL 的 MD5，跨次调用同一 URL 命中 `~/.octopus/tmp/{md5}.{ext}`。`--unclear` 标志双重作用：① 命中缓存则跳过下载；② 转码后保留文件（不删）。无 `--unclear` 则转码后删除。

### 临时文件清理（RAII）

`DownloadedFileGuard { path, keep }`：在下载文件确认存在后、ffmpeg spawn 前创建，drop 时按 `keep`（=`--unclear`）决定删除或保留。覆盖**所有退出路径**：

- ffmpeg `spawn()?` / `wait().await?` 的 `?` 提前返回（IO 错误）
- 正常完成
- `exit(1)`（转码失败 / 文件缺失）

Drop 用同步 `std::fs::remove_file`（async Drop 不可行；单文件 unlink 亚毫秒，可接受）。

> **2026-07-09 审查修复（commit dbf0d15）**：原手动清理块在 `spawn()?` / `wait().await?` 的 `?` 提前返回时被跳过，致下载文件泄漏磁盘。改 RAII 守卫消除早返回泄漏。

## 子进程协议契约（dlp ↔ cli）

| 通道 | 内容 |
|---|---|
| **stdout** | f32le PCM 采样流（16kHz mono，流模式）；文件模式 null |
| **stderr** | **首行**：元数据 JSON `{title, duration, author}`；其余：进度/日志 |
| **exit code** | 0 = 成功；非 0 = 失败（依赖缺失/下载失败/转码失败） |

**cli 侧消费**：

- stderr 逐行解析，首行匹配 `VideoMeta` 结构 → 打印视频信息（Title/Author/Duration）+ 存 `duration`（进度估算用）；非 JSON 行 `[dlp]` 前缀转发
- stdout PCM 流：**leftover 跨 read 拼接对齐**——管道是字节流，`read` 返回 n 不保证 4 字节对齐，直接 `chunks_exact(4)` 丢尾部 1-3 字节 → 永久字节错位 → 杂音、识别全毁。用 `leftover` 累积，每次只处理到 4 字节边界，余数留下次拼接；进程结束时尾部仍不对齐则警告丢弃

## 关键决策

1. **sidecar 子进程而非 crate 内嵌**：yt-dlp/ffmpeg 是外部 binary，子进程隔离崩溃（yt-dlp panic 不拖垮 cli）+ 进程间管道天然背压（cli 消费慢则 dlp 写阻塞，不爆内存）。代价：二进制查找逻辑 + cargo run 兜底。
2. **stdout/stderr 分离元数据**：元数据走 stderr 首行，PCM 走 stdout，避免 JSON 字节混入 PCM 流破坏采样对齐（与 cli 侧 leftover 对齐互为前提）。
3. **MD5 URL 缓存 + `--unclear`**：同 URL 跨次复用下载文件省重复下载；`--unclear` 让用户显式保留（调试/离线复用）。
4. **`.part` 原子下载 binary**：防半成品残留被 `exists()` 误判就绪。
5. **ffmpeg 不自动下载、yt-dlp 自动**：ffmpeg 体积大（~80MB）+ 平台包管理差异（brew/apt/官网），让用户装更稳；yt-dlp 小（单一 binary）+ GitHub releases 统一，自动下载体验好。
6. **RAII 清理**：转码失败也清临时文件，防磁盘泄漏（覆盖 `?` 早返回）。
7. **cargo run 兜底**：开发期无预编译 dlp binary 时仍可跑（`cargo run -p octopus-dlp --quiet -- <url>`）。

## 边界与错误处理

- yt-dlp 元数据获取失败（非 0 status）→ `exit(1)`
- 下载失败（非 0 status）→ `exit(1)`
- 下载成功但文件缺失 → `exit(1)`
- ffmpeg 转码失败（非 0 status）→ RAII 先清临时文件，再 `exit(1)`
- binary 下载超 200MB → `bail`
- ffmpeg 缺失 → 按平台提示安装后 `bail`
- cli 侧：dlp 非 0 exit → `bail!("octopus-dlp process exited with error status")`

## 测试

dlp crate 无单测（薄子进程包装，依赖外部 yt-dlp/ffmpeg + 网络，难单测）。验证靠：

- **编译**：`cargo check -p octopus-dlp`
- **e2e**：cli `transcribe-url <真实 URL>` 端到端（需 yt-dlp + ffmpeg 就绪 + 网络）
- **RAII 清理**：手动 e2e（模拟 ffmpeg 缺失致 spawn 失败，确认 `~/.octopus/tmp/{md5}.*` 被清）

## 涉及文件

- `crates/dlp/src/main.rs`（sidecar 全部逻辑：依赖管理 + 主流程 + RAII 清理）
- `crates/cli/src/main.rs`（`transcribe-url`：spawn dlp + 协议消费 + PCM leftover 对齐）
