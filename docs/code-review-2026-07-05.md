# Octopus 全量代码审查报告

> **修复状态**：P0 + P1 + P2 全部完成（14 Critical 全修，共性 1/2/3/4/5 已解决）。详见 [修复设计规格](superpowers/specs/2026-07-05-code-review-remediation-design.md)。

- **审查日期**：2026-07-05
- **审查范围**：全 workspace（12 个 crate，约 36,000 行 Rust + 前端 React/TS）
- **审查方法**：并行分派 5 个审查 agent 按 crate 分组深度只读审查 + `cargo clippy` 静态分析
- **工作区**：`.worktrees/code-review`（基于 main @ 0db5932）

## TL;DR

代码整体工程质量较高，注释异常详尽（每个非显然选择都标注了原因），核心算法（CIF/LFR/状态机/KV cache）实现到位，测试覆盖了关键回归点。但存在 **4 类跨模块的共性问题**，以及 **14 个 Critical 级缺陷**。建议按「优先修复」清单的顺序处理。

---

## 一、跨模块共性问题

这些问题在多个 crate 中以相同模式出现，建议作为专项整改而非逐点修补。

### 共性 1：Mutex/RwLock 锁毒化级联崩溃（全局性）

项目大量使用 `.lock().unwrap()` / `.read().unwrap()`。一旦任一持锁路径 panic，锁中毒，**之后所有访问同一锁的调用全部 panic**，且无法自愈（需重启）。受影响的关键路径：

| 位置 | 锁 | 影响 |
|------|----|------|
| `infra/src/db.rs:129` | 全局 DB 连接 `Mutex` | 整个应用所有 DB 操作瘫痪 |
| `clipboard/src/handle.rs:43-103` | `ClipboardContext`（9 处 unwrap） | 剪贴板永久停摆 |
| `desktop/src/...` 多处（runtime_config/coordinator/settings） | `RwLock<AppConfig>` | 设置/工具栏系统瘫痪 |
| `ocr/src/engine.rs:55,91` | `INIT_LOCK` 单例 | OCR 全不可用 |
| `asr-local/...qwen3_asr.rs:156` | 三 session 同时上锁 | 推理线程崩溃 |
| `desktop/src/coordinator.rs:2084` | `shutdown_db` 的 `cell.lock().unwrap()` | 退出流程 panic |

**建议**：统一改用 `parking_lot::Mutex/RwLock`（不携带毒化标志），或封装 `.unwrap_or_else(\|e\| e.into_inner())` 恢复模式。

### 共性 2：网络 I/O 全链路缺超时（全局性）

几乎所有网络操作都缺少超时保护，远端静默丢包时会**永久阻塞**：

| 位置 | 操作 | 后果 |
|------|------|------|
| `asr-cloud` 4 provider 的 `connect_async()` + `ws.next()` | WS 建立/读取 | 批引擎 `block_on` 永久卡死，UI 僵死 |
| `llm/src/client.rs:102` | `reqwest::blocking`（`test_connection` 反而正确设了 10s） | LLM 润色永久阻塞 |
| `dlp/src/main.rs:56,82` | `reqwest::get`（含下载可执行文件） | 进程卡死 + 供应链风险 |
| `desktop/src/engine_grpc.rs:31` | gRPC 首次 `connect()` 在 timeout 包装外 | coordinator 卡死 |
| `desktop/src/coordinator.rs:869` | `close_async()` 无超时 | CloudStreaming 永久卡死，Toggle/Cancel 全废 |

**建议**：用 `tokio::time::timeout` / reqwest `.timeout()` 包裹所有跨网络调用，建立项目级超时规范。

### 共性 3：大量重复代码（约 30% 可消除）

| 重复内容 | 位置 | 规模 |
|----------|------|------|
| `compute_fbank` 三份 | `paraformer.rs` / `fbank.rs` / `zipformer.rs` | ~80 行×3 |
| `apply_lfr` 两份（完全相同） | `paraformer.rs` / `fbank.rs` | ~22 行×2 |
| mel filterbank（一份正确、两份有 bug，见 Critical #1） | 同上 | — |
| 4 家云端 provider session 主循环 | `asr-cloud/*_stream.rs` | ~800 行同构 |
| 单段/多段下载逻辑双份 | `download/downloader.rs:141-222` vs `494-582` | ~120 行×2 |
| CLI 两个 streaming e2e 函数 | `cli/src/main.rs:627-947` | ~170 行×2 |
| desktop tick 线程三胞胎 | `coordinator.rs` | 结构完全相同 |
| desktop polish 双分支 / 截图入库三处 | `coordinator.rs` / `screenshot_commands.rs` | — |

**建议**：优先抽取 `compute_fbank` + mel filterbank 到 `asr-local` 公共模块（同时修掉 Critical #1 的 bug）。

### 共性 4：生产路径残留调试输出

- `asr-local/whisper.rs`（8 处 `eprintln!`，含 mel stats/prompt tokens）
- `asr-local/paraformer.rs:319`（CMVN debug）
- `desktop/screenshot_commands.rs`（多处 `eprintln!`）
- 前端 `Result/index.tsx:155,162`（高频 `console.log`）
- `asr-cloud/aliyun_stream.rs:198,442`（每条 partial 都 `info!`，日志洪水）

**建议**：统一改 `log::debug!`/`log::trace!` 并清理。

### 共性 5：死代码 / 无用依赖

- `infra/src/image_util.rs` 三函数全项目零调用（连带 `image`/`webp` crate 依赖可疑）
- `asr-cloud` `ParsedServerFrame.serialization` 字段解析后从不读取；`COMP_NONE`/`byte0` 等死字段
- `dlp/Cargo.toml` 的 `tempfile`、`llm/Cargo.toml` 的 `serde_yaml`（dev-dep）无任何使用
- `desktop` 多处 `#[allow(dead_code)]`：`unregister_shortcut`/`is_screenshot_active`/`send_scroll`/`close_all_pin_windows`/`Pipeline` trait
- `download/src/core/verify.rs:48` `if_range_value` 无生产调用；`downloader.rs:202` 416 分支不可达
- `capx/src/capture.rs:140` `capture_display_excluding_window`

---

## 二、严重问题（Critical，必须修复）

### C1. mel 滤波器权重斜率在 Hz 空间计算——已知 bug 未跨引擎传播
- **位置**：`asr-local/src/.../fbank.rs:128-134`、`zipformer.rs:1283-1291`
- **对比**：`paraformer.rs:593-614` 注释明确记载"此前在 Hz 空间计算权重导致 fbank 输出完全不同"，已修复为 mel 空间均匀分布。但 `fbank.rs`（SenseVoice-orig/FireRed 使用）和 `zipformer.rs`（Zipformer CTC/Transducer 非 whisper 路径）**仍是旧 bug**。
- **后果**：显著影响相关引擎的特征正确性 → 识别质量下降。
- **修复**：抽取一份正确的 mel filterbank（mel 空间计算权重），三处共用。

### C2. whisper.rs 每次 `new` 都 `Box::leak` 24-48 个字符串——内存泄漏
- **位置**：`asr-local/src/.../whisper.rs:268-275`
- **对比**：`qwen3_asr.rs:67-78` 已修为全局 `Lazy<CACHE_NAMES>`（注释"审查 #5：原实现每次实例化都 leak"），whisper 同类问题未修。
- **后果**：`AsrEngineManager` LRU 淘汰反复创建/丢弃 WhisperEngine，泄漏持续累积。

### C3. download 多段下载遇 200 响应数据错位/静默损坏
- **位置**：`download/src/core/downloader.rs:557,570-580`
- 服务端声称支持 Range 但实际返回 200 全文时，代码把全文从段 offset 写入，无 hash 时**用户拿到损坏模型文件**。
- **修复**：200 路径下仅写 `[seg.begin, seg.end]` 区间或报错重投。

### C4. desktop gRPC 首次连接不受超时保护
- **位置**：`desktop/src/engine_grpc.rs:31-36`
- `get_or_try_init`（含真实 `connect()`）在 `tokio::time::timeout` 包装**外**。远端不响应时首次 transcribe 无限阻塞 → coordinator 卡死。对比同文件 `health_check:67` 正确。

### C5. desktop macOS 非主线程调 AppKit——UB 风险
- **位置**：`desktop/src/settings_window.rs:79`（`MainThreadMarker::new_unchecked()`）
- `open_settings`（Tauri worker 线程）直接调，AppKit 非主线程调用是**未定义行为**。对比 `compact_editor_window.rs:81` 通过 `run_on_main_thread` 投递。

### C6. desktop cloud close 失败 → CloudStreaming 永久卡死
- **位置**：`desktop/src/coordinator.rs:869-880`
- `let _ = tx_clone.send(...)` 吞掉错误，`CloudStreamingDone` 永不到达。stage 永远停在 CloudClosing，Toggle/Cancel/Discard 全 no-op，必须重启应用。`close_async` 也无超时。

### C7. server ASR 推理直接阻塞 tokio 事件循环
- **位置**：`server/src/main.rs:126-127,247`
- `transcribe_batch`（CPU 密集）在 async handler 直接调用。对比 `desktop` 正确用 `spawn_blocking`。并发请求耗尽 worker 线程，`/health` 都无响应。

### C8. server 并发请求引擎切换竞态
- **位置**：`server/src/main.rs:126-127`
- `switch_model` 与 `transcribe_batch` 间无锁，并发请求不同 engine 会互相切换，**静默错误结果**。

### C9. server 默认 0.0.0.0 + 无认证 + permissive CORS
- **位置**：`server/src/main.rs:27,294`
- 局域网任意设备/网页可调用 `/transcribe`、`/ws/stream`。本地工具应默认 `127.0.0.1`。

### C10. asr-cloud 四 provider WebSocket 全链路无超时
- **位置**：`aliyun_stream.rs:99,155,370,426`、`bytedance_stream.rs:134,227`、`tencent_stream.rs:107,144`、`baidu_stream.rs:90,143`
- `CLOUD_CLOSE_TIMEOUT_SECS=8s` 仅覆盖 close，不覆盖连接建立和流式读取。静默丢包 → 永久卡死。

### C11. llm `chat_text` 无 HTTP 超时
- **位置**：`llm/src/client.rs:102`（`reqwest::blocking::Client::new()` 无超时）
- 同文件 `test_connection:202` 反而正确设了 10s。网络异常时永久阻塞。

### C12. dlp 无超时/无大小限制/无校验下载可执行文件
- **位置**：`dlp/src/main.rs:56-73,82-91,97`
- yt-dlp 二进制裸 `reqwest::get` 下载后直接 `set_mode(0o755)` 执行，MITM 可注入恶意可执行文件（供应链风险）。

### C13. dlp stderr 元数据 JSON 协议违反
- **位置**：`dlp/src/main.rs:226`（及 195/208/234/251 前的 eprintln）
- `docs/architecture.md:157` 约定"stderr 首行 = 元数据 JSON"，但代码先打印了多行日志，消费方 `from_str` 失败 → 元数据静默丢弃。

### C14. capx 单元测试非 macOS 平台编译失败
- **位置**：`capx/src/capture.rs:340-362`
- `bgra_to_rgba` 标了 `#[cfg(target_os="macos")]` 但 `#[cfg(test)] mod tests` 无门控。Linux/Windows CI 编译失败。

---

## 三、重要问题（Important，应修复）

### asr-local
- **whisper 归一化用 `(v+1e-10).log10()` 而非 `v.max(1e-10).log10()`**（`whisper.rs:87-92`）——与 sherpa-onnx 不符，小能量帧系统性偏高。
- **`audio.rs:19,44` hound sample `.unwrap()` 在 WAV 损坏时 panic**——应 `collect::<Result>()?`。
- **`streaming_paraformer.rs` `raw_samples` 全会话累积无界增长**——长会话可达数百 MB，应 drain 已消费样本。
- **`moonshine.rs:137` `uncached_out.len()-1` 空输出时下溢 panic**——改 `saturating_sub`。
- **`whisper.rs:268` 注释与实现不符**（说 subtract mean/divide std，实为 max-based log）。

### desktop
- **`coordinator.rs:814,1601,1642` `unreachable!()` 在 stage 重构后会 panic**——coordinator 线程死则录音/编辑/润色全死。建议改 `log::error + return`。
- **`screenshot_commands.rs:24-29` 截图全局静态量无并发互斥**——狂按快捷键会清空 PENDING_IMAGES 导致错误。对比滚动截图用了 `swap` 门控。
- **前端 `Result/index.tsx:447` `handleTextMouseUp` 用 state 而非 ref，高频流式下滞后**——同文件其它处统一用 `displayedRef.current`。
- **前端 `useClipboardHistory.ts:28` useEffect 缺 `fetchItems` 依赖**——隐患。
- **`main.rs:63,260` + `tray.rs` + `clipboard_commands.rs:243` 启动期 `.expect()` 直接扼杀应用**——配置/托盘失败应降级而非退出。

### infra / cli / server
- **`db.rs:332-378` `save_app_config_at` 30 条写入无事务**——中途崩溃配置半更新。应包 `transaction`。
- **DB 无 WAL 模式、无 busy_timeout**（`db.rs:113-116`）——server 多任务访问易 `SQLITE_BUSY`。
- **`pipeline.rs:51-55` 手工 JSON 转义不完整**——未转义 `\t`/`\r`/控制字符，ASR 输出含制表符产生非法 JSON。应用 `serde_json`。
- **`db.rs:436` `filter_map(|r| r.ok())` 静默丢弃失败行**——模型加载/历史搜索损坏不报错。
- **CLI `main.rs` 音频回调 `lock().unwrap()` 在实时线程 panic**（591/605/680/697/849/867）。
- **server `/transcribe` 无请求体大小限制**（`main.rs:93`）——超大 body 耗尽内存。
- **`db.rs:983` 识别历史搜索用 LIKE 而非已有的 FTS5**——已建索引却白维护。

### asr-cloud / llm / ocr
- **`baidu_stream.rs:195` 把 `Message::Close` 当 Finished**（其他三家当 Failed）——掩盖服务端错误关闭。
- **4 provider `tokio::spawn` 后台 task panic 被静默吞掉**（JoinHandle 丢弃）——`result_rx` 永不收事件。
- **`llm/client.rs:151` `max_tokens = chars × 1.2` 对中文偏低**——润色结果被截断。建议中文 `× 2`。
- **`ocr/engine.rs:55` `INIT_LOCK.lock().unwrap()` 中毒级联**。

### capx / clipboard
- **`capx/stitch.rs:76,681` `from_raw(...).expect()` 数据不一致时 panic**——不变量分散在 8 处手工维护。
- **`capx/stitch.rs:693` `canvas_buf_slice` 无边界检查越界 panic**。
- **`capx/capture.rs:235-299` unsafe 块内大量 `.unwrap()`**——CF 类型不符时 UB。
- **`clipboard/handle.rs:42` 写失败后 suppress_flag 未回滚**——误抑制下一次事件。
- **`clipboard/store.rs:112` offset u32 乘法溢出**（debug panic / release 回绕）。
- **`clipboard/cleanup.rs:41` `max_age_days=0` 删除所有非收藏项**——无下限保护。

---

## 四、次要问题（Minor，精选）

- **死代码**：见共性 5。`whisper_mel_matrix.rs` 16000 行静态表（80% 为 0）可改运行时 Sliley 公式生成。
- **`zipformer.rs:276` BBPE byte 32 重复映射**（` ` 和 `⁇` 都映射 32），意图不明需注释。
- **Aliyun Authorization 大小写不一致**：`aliyun_stream.rs:95` 小写 `bearer` vs `:366` 大写 `Bearer`（desktop `engine_aliyun.rs:54/350` 同样）。建议统一 `Bearer`。
- **`transcript.rs:84-89` 三个方法互为别名**（display_text/full/db_text 都转发 finish_text）——稳定后应统一。
- **前端 `ModelsPanel.tsx:44` 用 `any`**，`Settings/index.tsx` 已有完整 `ConfigResponse` 类型。
- **前端 `lib/tauri.ts` 薄包装无价值**，各页面混用，未统一。
- **`screenshot_commands.rs:599` 冗余 `as f64`**。
- **Tencent `tencent_stream.rs:291` 自实现 `percent_encode` 重复造轮子**。
- **Baidu `cuid` 复用 session UUID**——UV 统计失真可能触发限流。
- **download backoff 无真正 jitter**（注释自承）。
- **`epoch_to_ymd_hms` 重复造轮子**，项目已用 chrono。

---

## 五、clippy 静态分析

`cargo clippy --workspace --all-targets`（在 worktree，desktop 因 `frontendDist` 不存在编译失败——需先 build 前端，非代码 bug）。

- **共 118 个警告，全部为次要 lint**，无严重项。
- 主要分布：`needless_range_loop`(19)、`manual_is_multiple_of`(12)、`redundant_closure`(8)、`useless_conversion`(6)、`collapsible_if`(5)、`borrow_deref_ref`(4) 等。
- 可用 `cargo clippy --fix` 自动修复大部分。

---

## 六、整体评价

| 模块 | 评价 |
|------|------|
| **asr-local** | 核心算法实现到位、注释详尽。最大风险是「知识未跨引擎同步」：paraformer 修过的 mel filterbank / Box::leak 没传播到 fbank/zipformer/whisper，同类 bug 仍在生产路径。 |
| **desktop** | 逻辑密度高、设计深思（单线程 mpsc + stage 状态机、DB actor、LCP 纠正）。风险集中在「少量超时/panic 路径会让 coordinator 卡死且无自愈」+「RwLock 毒化级联」。前端质量好于后端。 |
| **infra** | 三模块中质量最高：SQL 全参数化、schema 设计合理、测试充分。主要不足是单连接+Mutex 的可扩展性 + 缺事务。`image_util` 完全死代码。 |
| **cli** | 功能完整但重复严重（两个 streaming 函数、show_config 多分支）。音频回调 lock().unwrap() 是脆弱点。 |
| **server** | 问题最集中：CPU 密集推理跑在 async runtime、并发引擎切换竞态、手工 JSON 转义、无认证/CORS 全开。上线多用户前必须修。 |
| **asr-cloud** | 协议层专业、错误传播清晰。全链路缺超时是上线必补项。四 provider 高度同构值得抽 trait。 |
| **llm** | 简洁聚焦。`chat_text` 无超时 + `max_tokens` 中文偏低是真问题。增量润色无 post-validation 有静默污染风险。 |
| **ocr** | DCL 单例、切分、busy lock 设计合理。主要风险是 INIT_LOCK 中毒。 |
| **dlp** | 偏「能跑」非「稳健」：stderr 协议违反、供应链下载风险、死依赖。 |
| **capx** | stitch 算法（NCC+亚像素+假匹配检测）有清晰文档化。panic 风险在 expect/索引边界。 |
| **clipboard** | cleanup/store 抽象干净。主要风险是 Mutex 中毒放大 + suppress_flag 误抑制。 |
| **download** | probe/分段/sidecar/校验链路完整、有多源 fallback 测试。多段 200 响应处理不安全是静默损坏风险。 |

---

## 七、优先修复建议

### P0（立即，影响正确性/数据完整性/可用性）
1. **C1** mel filterbank bug 跨引擎修复 + 抽公共实现
2. **C2** whisper Box::leak 泄漏
3. **C3** download 多段 200 静默损坏
4. **C6** desktop CloudStreaming 卡死（coordinator 无自愈）
5. **C13** dlp stderr 协议违反（集成已坏）

### P1（上线前，安全/稳定性）
6. **C7/C8/C9** server 三件套（spawn_blocking / 引擎锁 / 127.0.0.1 + 认证）
7. **C10/C11/C4** 全链路网络超时（asr-cloud / llm / gRPC）
8. **C12** dlp 供应链下载安全
9. **C5** desktop AppKit 主线程 UB
10. **共性 1** 锁毒化级联（统一 parking_lot）

### P2（质量改善）— ✅ 全部完成
11. ✅ **C14** capx 测试跨平台编译（加 `cfg(all(test, target_os = "macos"))` 门控）
12. Important 清单中的错误处理 / 事务 / WAL（后续迭代）
13. **共性 3** 重复代码抽取（compute_fbank 优先，同时消除 bug）（后续迭代）
14. ✅ **共性 4** 调试输出清理（whisper/paraformer/desktop screenshot/aliyun eprintln→log::debug!, 前端 console.log 删除）
15. ✅ `cargo clippy --fix` 清理 lint（70+ 自动+手动修复，零警告 + `#![warn(clippy::all)]` gate）
16. ✅ **共性 5** 死代码删除（image_util 全文件、desktop 5处、download/capx/asr-cloud 死代码、Pipeline trait）
