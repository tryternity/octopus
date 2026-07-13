# 视频音频下载（DLP Sidecar）

> `octopus-dlp`——独立 sidecar 进程，从在线 URL 下载音频/视频并转码为 16kHz mono PCM 供 ASR 使用。封装 yt-dlp + ffmpeg。

源文件：`crates/dlp/src/`。

## 1. 架构：独立进程而非 crate

sidecar 作为独立子进程运行（非 workspace crate 内联），核心收益：
- **崩溃隔离**：yt-dlp panic 不会杀死调用方进程。
- **管道背压**：进程间 pipe 天然提供流控。
- **stdout/stderr 分离**：metadata JSON 发到 stderr（首行），PCM 音频发到 stdout——避免音频流字节对齐问题。

## 2. 关键设计

### 2.1 stdout/stderr 分离 + 4 字节对齐

pipe 是字节流，`read(n)` 返回值不保证 4 字节对齐。PCM 是 `f32le`（4 字节/采样），错位会导致永久性音频损坏。

- cli 端用 `leftover` 缓冲区对齐：读取后检查尾部是否 4 字节整数倍，余数留到下一轮。
- metadata（时长/格式/标题）走 stderr 首行 JSON，与音频流完全隔离。

### 2.2 URL 缓存 + RAII 清理

- **MD5 URL 缓存**：命中跳过下载，直接转码。
- `--unclear`（yt-dlp `--no-clean-info` 反向）：下载后保留中间文件，供转码复用。
- **`DownloadedFileGuard`（RAII）**：覆盖所有退出路径（含 `?` early-return），确保临时文件删除。审查修复 commit `dbf0d15` 补了磁盘泄漏。

### 2.3 yt-dlp 自动下载

- yt-dlp 体积小，启动时自动从 GitHub releases 下载。
- ffmpeg **不**自动下载（~80MB，各平台包管理器不同），需用户自行安装。

## 3. CLI 使用

```bash
# 转录在线 URL
cargo run -p octopus-cli -- transcribe-url "https://..." --model sensevoice
```
