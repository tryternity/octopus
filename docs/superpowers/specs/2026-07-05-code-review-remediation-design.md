# 代码审查修复设计规格

> 关联文档：
> - 审查报告：`docs/code-review-2026-07-05.md`
> - 实施计划：`docs/superpowers/plans/2026-07-05-code-review-fix-p0.md`（及 P1/P2）

## 1. 背景与目标

2026-07-05 对全 workspace（12 crate，约 36K 行）做了一次深度代码审查，发现 14 个 Critical、约 40 个 Important 及大量 Minor 问题。本规格定义这些问题的修复方案与测试方案。

**总目标**：消除所有 Critical 缺陷，解决跨模块共性根因（锁毒化、网络超时缺失、重复代码），建立回归测试防线，使代码达到「上线多用户可用」的质量基线。

**不在范围内**：
- 架构级重构（如 server 改为 per-request engine 隔离），仅做稳定性补丁
- 前端视觉重构
- 新功能开发

## 2. 子项目分解

按「根因主题 + 独立子系统」切分为 7 个子项目，按优先级分三批执行。

### 子项目 A：asr-local 正确性修复（P0）

**范围**：影响识别质量的核心 bug。

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| C1 | mel filterbank 权重在 Hz 空间计算（paraformer 已修，fbank/zipformer 未修） | `asr-local/src/fbank.rs:128-134`、`zipformer.rs:1283-1291` | 抽取 `asr-local/src/feature.rs` 公共模块，统一 mel 空间 filterbank 实现（参考 `paraformer.rs:599-614`），fbank/zipformer 复用 |
| C2 | whisper.rs 每次 `new` 都 `Box::leak` 24-48 个字符串 | `asr-local/src/whisper.rs:268-275` | 改为全局 `Lazy<Vec<...>>`（参考 `qwen3_asr.rs:67-78` 的 `CACHE_NAMES`） |
| I-1 | whisper 归一化用 `(v+1e-10).log10()` 而非 `v.max(1e-10).log10()` | `asr-local/src/whisper.rs:87-92` | 改为 `.max(1e-10).log10()` 对齐 sherpa-onnx `NormalizeWhisperFeatures` |
| I-2 | `audio.rs:19,44` hound sample `.unwrap()` 在 WAV 损坏时 panic | `asr-local/src/audio.rs` | 改 `collect::<Result<Vec<_>,_>>()?` |
| I-3 | `streaming_paraformer.rs` `raw_samples` 全会话累积无界增长 | `asr-local/src/streaming_paraformer.rs:169-172` | chunk 处理后 drain 已消费样本，仅保留必要 history（fbank 帧对齐的边界） |
| I-4 | `moonshine.rs:137` `uncached_out.len()-1` 空输出时下溢 panic | `asr-local/src/moonshine.rs:137` | 改 `.saturating_sub(1)` |
| I-5 | whisper.rs:268 注释（说 mean/std）与实现（max-based log）不符 | `asr-local/src/whisper.rs:678-679` | 修正注释 |

**抽取决策（C1 的修复手段）**：先修 bug 再抽取。创建 `asr-local/src/feature.rs`，包含：
- `pub fn compute_fbank(...)` — 统一 fbank 提取（参数化窗口类型：hamming / povey）
- `pub fn mel_filterbank(...)` — mel 空间 filterbank（参数化 high_freq）
- `pub fn apply_lfr(...)` — LFR 合并
- `pub fn hz_to_mel(hz) / mel_to_hz(mel)` — 统一公式

fbank.rs / paraformer.rs / zipformer.rs 改为引用公共模块，删除各自的私有副本。注意：whisper 特征路径（mel matrix）不共用 fbank，保持独立。

### 子项目 B：全局锁毒化整改（P1）

**范围**：消除 Mutex/RwLock 毒化级联崩溃。

**技术决策：引入 `parking_lot` 作为 workspace 依赖。**
- `parking_lot::Mutex/RwLock` 不携带毒化标志，持锁期间 panic 不会中毒锁。
- API 几乎兼容（`std::sync::Mutex` → `parking_lot::Mutex`），迁移成本主要是改 import + 去掉 `.unwrap()`。
- 轻量、纯 Rust、生态标准依赖。

**受影响位置**（按 crate）：

| crate | 位置 | 锁类型 | 改动 |
|-------|------|--------|------|
| infra | `db.rs:129` 全局 DB 连接 Mutex | `std::sync::Mutex` → `parking_lot::Mutex` | 去 `.unwrap()` |
| clipboard | `handle.rs:43-103`（9 处） | `std::sync::Mutex` → `parking_lot::Mutex` | 去 `.unwrap()` |
| ocr | `engine.rs:55,91` INIT_LOCK | `std::sync::Mutex` → `parking_lot::Mutex` | 去 `.unwrap()` |
| asr-local | `qwen3_asr.rs:156-158`、各引擎 session Mutex | `std::sync::Mutex` → `parking_lot::Mutex` | 去 `.unwrap()`；qwen3 三锁改分阶段加锁 |
| desktop | `runtime_config.rs`、`settings_commands.rs`、`coordinator.rs`、`model_commands.rs` 的 `RwLock<AppConfig>` | `std::sync::RwLock` → `parking_lot::RwLock` | 去 `.unwrap()` |
| desktop | `coordinator.rs:2084` shutdown_db `cell.lock().unwrap()` | → `parking_lot::Mutex` | 去 `.unwrap()` |
| desktop | `screenshot_commands.rs:24-29` 全局 `Mutex<Vec>` | `std::sync::Mutex` → `parking_lot::Mutex` | 去 `.unwrap()` |

**Workspace Cargo.toml**：在 `[workspace.dependencies]` 加 `parking_lot = "0.12"`，各 crate 按需引用。

### 子项目 C：全局网络超时（P0/P1）

**范围**：消除网络 I/O 永久阻塞。

**技术决策：建 `infra/src/net.rs` 统一超时常量 + helper。**

```rust
// infra/src/net.rs
pub const WS_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const WS_READ_TIMEOUT_SECS: u64 = 30;
pub const HTTP_TIMEOUT_SECS: u64 = 120;       // LLM 推理慢
pub const GRPC_CONNECT_TIMEOUT_SECS: u64 = 8;
pub const GRPC_REQUEST_TIMEOUT_SECS: u64 = 30;
pub const FILE_DOWNLOAD_TIMEOUT_SECS: u64 = 300;
```

各 crate 引用这些常量，避免超时值散落不一致。

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| C10 | asr-cloud 4 provider WS 全链路无超时 | `aliyun_stream.rs:99,155`、`bytedance_stream.rs:134,227`、`tencent_stream.rs:107,144`、`baidu_stream.rs:90,143` | `connect_async` 包 `tokio::time::timeout(WS_CONNECT_TIMEOUT)`；主循环 `ws.next()` 包 `tokio::time::timeout(WS_READ_TIMEOUT)`，超时发 `StreamEvent::Failed` |
| C11 | llm `chat_text` 无 HTTP 超时 | `llm/src/client.rs:102` | 改 `Client::builder().timeout(Duration::from_secs(HTTP_TIMEOUT)).build()?`；复用共享 Client（`once_cell::Lazy`）消除每次 new |
| C4 | desktop gRPC 首次 connect 在 timeout 包装外 | `desktop/src/engine_grpc.rs:31-36` | 把 `get_or_try_init` 移入 `fut`，或单独对 connect 加 `timeout(GRPC_CONNECT_TIMEOUT)` |
| C6 | desktop cloud close 无超时，失败静默吞掉 | `desktop/src/coordinator.rs:869-880` | spawn 内包 `tokio::time::timeout(WS_READ_TIMEOUT * 2, close_async())`；超时/panic 也发 `CloudStreamingDone(Err(...))` |
| C12 | dlp 下载无超时/无大小限制 | `dlp/src/main.rs:56,82` | `reqwest::Client::builder().timeout(FILE_DOWNLOAD_TIMEOUT).build()`；加 `MAX_DOWNLOAD_SIZE`（如 200MB）检查 |

### 子项目 D：server 稳定化（P1）

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| C7 | ASR 推理直接阻塞 tokio event loop | `server/src/main.rs:126-127,247` | 用 `tokio::task::spawn_blocking` 包裹 `transcribe_batch`；WS `stream.feed` 同理或独立线程 |
| C8 | 并发请求引擎切换竞态 | `server/src/main.rs:126` | 用 `engine_manager` 内部锁或请求级 `Mutex` 保护 switch+transcribe 原子性 |
| C9 | 默认 0.0.0.0 + 无认证 + permissive CORS | `server/src/main.rs:27,294` | 默认改 `127.0.0.1`；CORS 改为可配置（默认同源）；加可选 API token header 校验 |
| I-D1 | 手工 JSON 转义不完整 | `server/src/pipeline.rs:51-55` | 用 `serde_json::to_string` 替代手工拼接 |
| I-D2 | `/transcribe` 无 body limit | `server/src/main.rs:93` | 加 `DefaultBodyLimit::max(100 * 1024 * 1024)`（100MB 音频上限） |
| I-D3 | 无优雅关闭 | `server/src/main.rs:300` | 加 `axum::serve(...).with_graceful_shutdown(signal_handler)` |

### 子项目 E：download + dlp 数据完整性（P0）

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| C3 | download 多段遇 200 响应数据错位/静默损坏 | `download/src/core/downloader.rs:557,570-580` | 200 路径采用**截断写入**：仅写入 `[seg.begin, seg.end]` 区间（`written` 累计到 `seg.end - seg.begin + 1` 字节后丢弃多余），`new_downloaded = seg.end - seg.begin + 1`。这样多段文件大小不变、内容正确；已有 hash 校验不变 |
| C13 | dlp stderr 元数据 JSON 不是首行（协议违反） | `dlp/src/main.rs:226` | 把所有 `eprintln!` 改为 `eprintln!("[log] ...")` 前缀或移到 stdout；确保元数据 JSON 是 stderr 首行（移到 `Command::new(yt_dlp)` 之前不打印任何 stderr） |
| I-E1 | download 失败/取消时 .part 与 sidecar 未清理 | `download/src/core/downloader.rs:413` | 失败时清理 .part 文件（保留 sidecar 用于续传判断）；或文档化"保留用于续传"策略 |
| I-E2 | `download_segment_once` 416 分支不可达（死代码） | `download/src/core/downloader.rs:202-206` | 删除不可达分支 |
| I-E3 | dlp tempfile 死依赖 | `dlp/Cargo.toml:13` | 删除 `tempfile` 依赖 |

### 子项目 F：desktop 协调器健壮性（P0/P1）

**P0 部分**：

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| C5 | settings_window 非主线程调 AppKit（UB） | `desktop/src/settings_window.rs:79` | 改用 `MainThreadMarker::new()`（返回 Option）；通过 `app_handle.run_on_main_thread()` 调度 `set_dock_icon` |
| C6 | CloudStreaming 永久卡死（见子项目 C） | `desktop/src/coordinator.rs:869` | 见 C6 方案 |

**P1 部分**：

| ID | 问题 | 位置 | 修复方案 |
|----|------|------|---------|
| I-F1 | coordinator unreachable!() stage 重构后 panic | `desktop/src/coordinator.rs:814,1601,1642` | 改为 `log::error + return`（防御性降级） |
| I-F2 | 截图全局静态量无并发互斥 | `desktop/src/screenshot_commands.rs:24-29` | 用 `AtomicBool`（`compare_exchange`）门控，进行中则拒绝重复触发 |
| I-F3 | 启动期 expect 直接扼杀应用 | `desktop/src/main.rs:63,260`、`tray.rs`、`clipboard_commands.rs:243` | 配置加载失败 fallback default + warn；托盘失败进入"无托盘"模式；home_dir 失败返回错误而非 panic |

### 子项目 G：死代码 + clippy + 调试输出清理（P2）

| 类别 | 内容 |
|------|------|
| 死代码删除 | `infra/src/image_util.rs` 全文件（+ Cargo.toml 移除 image/webp 依赖，需确认无其他引用）；`desktop` 的 `unregister_shortcut`/`is_screenshot_active`/`send_scroll`/`close_all_pin_windows`/`Pipeline` trait；`download/verify.rs:48 if_range_value`；`capx/capture.rs:140 capture_display_excluding_window`；`asr-cloud` `ParsedServerFrame.serialization`/`COMP_NONE`/`byte0` |
| 死依赖 | `dlp/Cargo.toml` tempfile；`llm/Cargo.toml` serde_yaml(dev-dep) |
| C14 capx 跨平台编译 | `capx/src/capture.rs:340` 测试块加 `#[cfg(target_os = "macos")]` |
| 调试输出清理 | `asr-local/whisper.rs` 8 处 eprintln!→log::debug!；`paraformer.rs:319`；`desktop/screenshot_commands.rs` eprintln!；前端 `Result/index.tsx:155,162` console.log 删除；`asr-cloud/aliyun_stream.rs:198,442` info!→debug! |
| clippy | `cargo clippy --fix --workspace` 清理 118 个 lint；补 `#![warn(clippy::all)]` 到各 crate lib.rs |

## 3. 技术决策汇总

### 决策 1：锁毒化 → 引入 parking_lot
- **选择**：`parking_lot = "0.12"` 作为 workspace 依赖
- **理由**：不携带毒化标志，语义上消除整类问题；API 兼容；纯 Rust、生态标准
- **取舍**：新增轻量依赖

### 决策 2：网络超时 → 统一 infra/net.rs 常量
- **选择**：建 `infra/src/net.rs`，定义所有超时常量，各 crate 引用
- **理由**：避免超时值散落不一致（如 llm 的 chat_text 忘加但 test_connection 加了）
- **取舍**：多一层间接，但解决遗漏问题

### 决策 3：asr-local 重复代码 → 先修 bug 再抽取公共 feature.rs
- **选择**：创建 `asr-local/src/feature.rs`，统一 compute_fbank + mel_filterbank + apply_lfr
- **理由**：抽取本身就是 C1 的修复手段（让 fbank/zipformer 复用 paraformer 的正确实现）
- **约束**：whisper 特征路径不共用（使用预计算 mel matrix），保持独立

## 4. 测试方案

### 4.1 测试分层

| 层级 | 策略 | 适用子项目 |
|------|------|-----------|
| 单元测试 | 每个修复配回归测试，内联 `#[cfg(test)] mod tests` | A, B, C, D, E |
| 集成/协议测试 | mock server / 跨进程协议验证 | C, D, E |
| 编译验证 | 跨平台 check + clippy gate | G |
| 回归基线 | 实施前 `cargo test --workspace` 记录基线，每 Task 后比对 | 全部 |

### 4.2 各子项目关键测试用例

**子项目 A（asr-local）**：

| 测试 | 验证点 |
|------|--------|
| `test_mel_filterbank_matches_sherpa` | mel filterbank 输出对比 sherpa-onnx 参考值（固定 mel 点的权重），断言 Hz 空间 bug 已消除 |
| `test_whisper_cache_names_no_leak` | 连续创建 2 个 WhisperEngine，断言 `past_key_names` 引用相同全局 `Lazy`（地址相等） |
| `test_whisper_normalize_clamp` | 对接近 0 的 mel 值，断言用 `max(1e-10)` 而非 `+1e-10` |
| `test_read_wav_corrupted` | 喂入截断的 WAV bytes，断言返回 Err 而非 panic |
| `test_streaming_paraformer_drain` | 喂入大量 chunk 后，断言 `raw_samples.len()` 有上界（不超 N 秒数据） |
| `test_moonshine_empty_output_no_underflow` | mock ONNX 返回空输出，断言不 panic |

**子项目 B（锁毒化）**：

| 测试 | 验证点 |
|------|--------|
| `test_db_lock_no_poison` | 在 `with_db` 闭包内手动 panic，断言后续 `with_db` 调用仍正常 |
| `test_clipboard_lock_no_poison` | 在 `on_clipboard_change` 回调内 panic（mock），断言后续读写可用 |
| `test_config_rw_no_poison` | 在持写锁时 panic，断言后续读取可用 |

**子项目 C（网络超时）**：

| 测试 | 验证点 |
|------|--------|
| `test_ws_connect_timeout` | mock 慢速 WS server（不响应握手），断言 10s 超时返回 `Failed` |
| `test_ws_read_timeout` | mock WS 建连后不发数据，断言 30s 超时 |
| `test_llm_http_timeout` | mock 慢速 HTTP（延迟 > HTTP_TIMEOUT），断言超时返回 Err |
| `test_grpc_connect_timeout` | mock 不存在的 gRPC 端点，断言首次 connect 不超 timeout |
| `test_cloud_close_watchdog` | mock close_async 挂起，断言 watchdog 超时后发 `CloudStreamingDone(Err)` |

**子项目 D（server）**：

| 测试 | 验证点 |
|------|--------|
| `test_transcribe_spawn_blocking` | 并发 5 个 /transcribe，断言 /health 在推理期间仍响应 |
| `test_engine_switch_concurrent` | 并发请求不同 engine，断言无竞态（结果对应请求的 engine） |
| `test_default_bind_localhost` | 断言默认 host = 127.0.0.1 |
| `test_json_escape_control_chars` | ASR 输出含 \t\r，断言 JSON 合法可 parse |

**子项目 E（download/dlp）**：

| 测试 | 验证点 |
|------|--------|
| `test_download_200_truncated` | mock server 忽略 Range 返回 200 全文，断言写入仅 `[seg.begin, seg.end]` 区间，文件未损坏 |
| `test_download_200_hash_verify` | 同上，有 hash 时断言校验通过；无 hash 时断言文件大小 == total |
| `test_dlp_stderr_first_line_json` | mock yt-dlp 输出，断言 stderr 首行可 `serde_json::from_str` |

### 4.3 测试基础设施

- **mock server**：download 已用 `httpmock`（见 `downloader.rs:587`），server/llm 复用
- **mock WS**：asr-cloud 用 `tokio-tungstenite` 起 local WS server（accept 后不响应）
- **跨平台 CI**：C14 修复后加 GitHub Actions `cargo check --workspace` on linux/macos/windows

### 4.4 验证命令

```bash
# 基线（实施前）
cargo test --workspace 2>&1 | tee /tmp/test-baseline.txt

# 每 Task 后
cargo test -p <crate-name>
cargo clippy -p <crate-name> -- -D warnings

# 全量验证
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 5. 执行顺序

### 第一批 P0（先写 plan + 实施）
1. **子项目 A**（asr-local 正确性）— 最独立，影响识别质量
2. **子项目 E**（download + dlp）— 静默损坏 + 集成已坏
3. **子项目 F-P0**（C5 AppKit UB、C6 CloudStreaming 卡死）— 用户可感知崩溃

### 第二批 P1
4. **子项目 C**（全局网络超时）— 含 C4 gRPC、C10 asr-cloud、C11 llm
5. **子项目 D**（server 稳定化）
6. **子项目 B**（锁毒化）— 牵动全局，放 P1 后段以减少冲突
7. **子项目 F-P1**（unreachable! 降级、截图门控、启动 expect）

### 第三批 P2
8. **子项目 G**（死代码 + clippy + 调试输出）

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| parking_lot 迁移面广（全项目锁） | 分 crate 逐个迁移，每个独立编译验证；优先 infra/clipboard，desktop 最后 |
| asr-local feature.rs 抽取改变特征提取路径，可能引入回归 | 抽取前后用 sherpa-onnx 参考值做 A/B 对比测试；保留旧实现直到测试通过再删 |
| 网络超时值不当导致正常请求被截断 | LLM 超时设 120s（思考模型慢）；WS read 30s（语音流间隙）；提供配置覆盖 |
| server spawn_blocking 改造影响现有测试 | server 现有测试仅 3 个，覆盖低，风险可控 |

## 7. 文档同步

每个子项目实施后更新：
- `docs/architecture.md` — 对应章节（如 infra 新增 net.rs、parking_lot 迁移）
- `docs/superpowers/specs/` — 本 spec
- `docs/superpowers/plans/` — 对应 plan 回写实施记录
- `docs/code-review-2026-07-05.md` — 标注已修复项
