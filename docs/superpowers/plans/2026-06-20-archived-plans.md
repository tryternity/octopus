# 归档实施计划（2026-06-20）

> **归档说明**（2026-06-21）：以下 3 个 plan 对应功能均已实现并合并 main，各自文档原样合并归档于此，原独立文件已删除。每个章节以 `📄 <原文件名>` 标注来源。
> **交叉引用**：正文内 `[xxx.md](./xxx.md)` 链接为合并前原文件名，现指向本归档文件内同名章节；对应 specs 见 `docs/superpowers/specs/2026-06-20-archived-design.md`。

---

## 📄 `2026-06-20-desktop-audit-followups.md`

# desktop 实现审查 · 后续待办

> 来源：`docs/superpowers/specs/2026-06-20-desktop-implementation-audit.md`（7 条审查的复核 + P0/P1 实施）。
> 状态（2026-06-20）：P0（一1/一2/二1）+ P1（二2/三2/三1）**均已合并 main**（P0 `44b8ab8`、P1 `9a19b6b`）。本文档后续事项**均已处理**：§1 一3 已实现（`e0f1420`）、§4 跨会话护栏已实现（`cfe78f4`）、§2 GUI e2e 已通过（2026-06-20，用户确认）。

---

## 1. 一3 剪贴板恢复竞态（✅ 已实现）

**背景**：paste 流程会先备份用户当前剪贴板 → 写入识别文本 → 触发系统粘贴（Cmd+V）→ 恢复原剪贴板。若「恢复」发生在系统粘贴动作完成之前，恢复的旧内容会被粘进目标应用（用户看到的是自己之前的剪贴板，而非识别文本）。

**审查结论**：真实但低危——仅在慢速系统 / 高延迟粘贴路径上偶发，绝大多数场景粘贴是同步完成。详见 audit spec §3.3。

**修法草图**（实施时再细化）：
- `paste::paste` 成功返回后，延迟一个保守时长（~150–300ms，需按平台实测）再 restore 剪贴板；
- 或改为「restore 前 probe 粘贴是否落地」的信号（更复杂，YAGNI，优先纯延迟）。
- 注意 macOS / Windows / Linux 粘贴异步性不同，延迟可能需按平台分档。

**状态**：✅ 已实现（`PASTE_RESTORE_DELAY = 200ms`，`e0f1420`；spec `2026-06-21-clipboard-restore-race-design.md`）。GUI e2e 已通过（2026-06-20，见 §2）。

---

## 2. GUI e2e 验证（✅ 已通过 2026-06-20）

P0/P1 的修复逻辑均由 `cargo check` + 逻辑审查 + 既有单测保证，但**行为正确性留 GUI e2e**——CI 环境无 GUI / 无真实音频设备 / 无真实 DashScope key，以下项需在本地桌面环境手动验证：

| 来源 | 验证项 | 预期 |
|---|---|---|
| 一1 | 录音中 Esc（Cancel）→ 立即重开新录音 → 旧中间润色结果 | 不污染新会话 transcript / 不写错 DB 行 |
| 一2 | 云端引擎 + 无效 API Key → 触发语音 onset | 结果窗报「⚠️ 云端识别失败」，状态复位，下次 onset 重试 |
| 二1 | 设置窗改 denoise_mode / 硬件加速 → 保存 | **本次生效**（无需重启），asr 缓存已 reload |
| 二2 | 设置窗改麦克风设备 → 保存 → Toggle 新录音 | 用新设备采集（非旧设备） |
| 三2 | 设置窗切 ASR 引擎 → 立即 Toggle 录音 | 首次识别无明显懒加载卡顿（已后台预热） |
| 三1 | 云端录音中连按 Toggle 停止 → 网络模拟慢 close | 主线程不卡（快捷键不堆积），close 完成后自动粘贴 |
| 三1 | 云端 CloudClosing 期间点 Cancel / Discard | Cancel：不粘贴不写库；Discard：写库保历史不粘贴 |
| 一3 | `write_to_clipboard=false` + 慢系统/高负载 → 识别粘贴 | 目标应用粘进识别文本（非之前剪贴板内容） |
| 一1+ | 启用最终润色 → Esc Cancel → 立刻重开+停止触发润色 → 等旧润色返回 | 新会话粘进**新**润色文本（非旧会话）；日志见 `FinalPolishDone session_id mismatch ... 丢弃` |
| 三1+ | 云端停止(CloudClosing) → Esc/Discard → 立刻重开云端+停止 → 等旧 close 返回 | 新会话粘进**新**云端文本（非旧会话）；日志见 `CloudStreamingDone session_id mismatch ... 丢弃` |

**状态**：✅ 已通过（2026-06-20 本地 GUI 验证，用户确认）。

> 一1+ / 三1+ 两条触发苛刻（需卡在润色/close 窗口内 Cancel+重开+再停），难稳定手动复现；主要靠护栏逻辑正确性 + mismatch 日志验证。

---

## 4. FinalPolishDone / CloudStreamingDone 跨会话护栏（✅ 已实现）

**背景**：审查一1 当时仅给中间润色 `PolishDone` 加了 `session_id` 护栏，认为最终润色 `FinalPolishDone` 已被 stage guard 保护（`handle_toggle` 对 `Stage::Polishing` 忽略 Toggle）。**复核发现该推理有漏洞**——它只覆盖「Cancel 后保持 Idle」，漏了「Cancel（→Idle）+ 立刻重开新录音 + 再次停止触发润色 → 新 `Stage::Polishing`」：旧会话迟到的 `FinalPolishDone` 会匹配新 Polishing，用新 id + 旧润色文本 `do_paste` → 跨会话文本污染。`CloudStreamingDone`（审查三1 引入）同理：CloudClosing 期间 Cancel/Discard 清回 Idle（绕过 Toggle 忙保护），重开云端会话 → 新 CloudClosing，旧 close 结果 `set_full` 覆盖新 transcript。

**触发条件**（窄但真实）：润色 1~3s / close 在飞窗口内 Cancel + 重开 + 再次停止，且旧结果恰好落在新会话的同名 stage 窗口内。命中即静默跨会话污染（粘进/落库错会话文本）。

**修复**（对称于既有 `PolishDone` 护栏，`coordinator.rs` 单文件，机械低风险）：
- `Command::FinalPolishDone` / `CloudStreamingDone` 各加 `session_id: i64`（= 发起时的 transcript.id）。
- spawn 处带 id：最终润色 spawn（`start_final_polish_or_paste`，`id` 已在 L1035 取出）、云端 close spawn（`handle_toggle` CloudStreaming arm，`tr.id`）。
- handler 入口校验当前 stage id == session_id，mismatch 则 warn/debug + return（不动当前 stage）：`handle_final_polish_done`（`Polishing.id`）、`handle_cloud_streaming_done`（`CloudClosing.transcript.id`）。

**验证**：`cargo check --workspace --all-targets` 零 warning；`cargo test -p octopus-desktop` 36 passed / 0 failed。无单测（coordinator 全 Tauri 耦合，与一1 session_id 护栏同理 YAGNI）；行为正确性留 GUI e2e（见 §2 一1+/三1+）。

**状态**：✅ 已实现（本 worktree `clipboard-restore-race`）。audit spec §3.1/§4 已同步修正原「FinalPolishDone 已被保护」结论。

---

## 3. 关联（非本文档范围，仅交叉引用）

- **dashscope ASR 真实 key e2e**：云端 WS 引擎是 `#[ignore]` 测试，从未用真实 DashScope key 跑过端到端。属独立 workstream，见 memory `parallel-workstreams`，不在本审查范围。

## 📄 `2026-06-20-ort-cross-platform.md`

# ort 跨平台 EP feature 条件化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 ort 的硬件加速 EP feature 按平台条件化（mac=coreml、linux=cuda、win=directml），与代码层 `#[cfg]` 1:1 对齐，为 macOS 上曾发生的 cuda/directml EP segfault 提供feature-level 二道防线（首道是 config.rs 的 cfg gate）。

**Architecture:** Cargo target-specific dependency——base `[dependencies]` 放 `download-binaries`，三个 `[target.'cfg(target_os=...)']` 各放对应 EP feature；代码层 `config.rs` win 块删 CUDA 注册。

**Tech Stack:** Rust + Cargo target-specific dependencies + ort 2.0.0-rc.12 + `#[cfg(target_os)]` 条件编译

**设计 spec:** `docs/superpowers/specs/2026-06-20-ort-cross-platform-feature-design.md`

**Worktree:** `feature/ort-cross-platform`（`.claude/worktrees/ort-cross-platform`）。EnterWorktree 工具切入失败，session CWD 已手动落在 worktree 内；git 操作直接在本目录跑。

---

## 执行结果（2026-06-20，已落地）

commits：`66a8a73`（Task 1 Cargo.toml）、`21fb2fb`（Task 2 config.rs）。

**三项实测结论（部分修正了 spec/plan 的初始前提，详见 spec §7）：**

1. **mac 体积收益 = 0**：删 cuda/directml feature 后 mac 二进制仍 54M（56,778,304 字节），零下降。这两个 feature 在 mac 本就是 size no-op（Rust 引用早被 `#[cfg]` 排除；GPU 预编译库无 mac 版从未下载）。本改动价值不在体积。
2. **「feature↔#[cfg] 不一致则编译失败」不成立**：ort EP 类型无条件编译，仅 `register()` 内 FFI 块按 feature gate（feature off 时返回 `MissingFeature`、不碰 FFI）。故编译器**不**抓不一致。但 feature 关闭仍提供**真·defense-in-depth**——cuda/directml feature off 时，即便有人退化 config.rs 的 cfg gate，`register()` 也不走 FFI dlopen-libcuda（segfault 路径），从而不崩。
3. **`default-features=false` 实测去掉**（保留默认集开启）：关掉会缺 `tls-native`，`download-binaries` 编译即报缺 TLS feature；而默认集本不含 cuda/directml/coreml，保留不影响目标。

**验证状态：** mac 单测 45 passed/0 failed、release build 通过、**GUI e2e 已通过（2026-06-20，CoreML 加速正常、无 segfault 回归）**；linux/win 交叉 check 受阻于目标工具链（openssl-sys / esaxx-rs），非 ort，留用户在对应平台本地 `cargo check` 兜底。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/asr/Cargo.toml` | ort 依赖声明 | 改 target-specific（base + 3 平台块） |
| `crates/asr/src/config.rs` | `apply_session_acceleration` 的 EP 注册 | win 块删 CUDA + 更新注释 |

无新建文件。

## 测试策略

配置 + 条件编译改动，无传统单测新增。验证三层：

1. **现有测试不回归**：`cargo test -p octopus-asr` 全过。
2. **mac 编译+release**：`cargo check/build --release` 通过。
3. **mac e2e**：desktop 录音 + CoreML 加速正常、无 segfault 回归（用户本地）。

> feature 矩阵的 linux/win 正确性：交叉 `cargo check` 受阻于目标工具链（非 ort），改由源码 gate 结构核验（spec §7.3）+ 用户在对应平台本地自测。

---

## Task 1: Cargo.toml 改 target-specific ort ✅

**Files:**
- Modify: `crates/asr/Cargo.toml`（ort 声明 + 文件末尾追加 3 个 target 块）

- [x] **Step 1: 替换 ort 声明为 target-specific 形态**

base 行留在 `[dependencies]` 顶部，三个 `[target.'cfg']` 块追加到 `[dependencies]` 表**末尾**（flate2 之后）——TOML 表头切换活跃表，放 ort 行正下方会让 ndarray/anyhow 等后续依赖泄漏进 windows target。最终形态（实测去掉 `default-features=false`，见 Step 3）：

```toml
# ort EP feature 按平台条件化（spec 2026-06-20）：与 config.rs apply_session_acceleration
# 的 #[cfg(target_os)] 1:1 对齐（设计意图，非编译器硬约束——ort EP 类型无条件编译、仅 register()
# FFI 块按 feature gate，故不一致不会编译失败；真正的 feature-level 保护见 spec §7.3）。
# base 放跨平台必需的 download-binaries，各平台只启用对应 EP。
# 不用 default-features=false：ort 默认集 = std/ndarray/tracing/download-binaries/tls-native/
# copy-dylibs/api-24，关掉会缺 TLS（download-binaries 编译即报缺 TLS feature）。关键在于
# 默认集本就不含 cuda/directml/coreml——保留默认集不影响「各平台只编入本平台 EP」的目标。
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }
```

文件末尾（flate2 之后）追加：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

- [x] **Step 2: mac 本地验证编译**

Run: `cargo check -p octopus-asr`
结果：初版带 `default-features=false` 时**报错**——`download-binaries` 要求 TLS feature（关掉默认集丢了 `tls-native`）。去掉 false 后通过（16.3s）。

- [x] **Step 3: 实测 `default-features = false` 结论**

**结论：去掉 `default-features = false`，保留默认集开启。** ort 默认集 = `["std","ndarray","tracing","download-binaries","tls-native","copy-dylibs","api-24"]`，关掉缺 `tls-native` 致 download-binaries 编译失败。关键：默认集**本不含** cuda/directml/coreml，故保留不影响「各平台只编入本平台 EP」目标。

- [x] **Step 4: commit**（`66a8a73`）

```
build(asr): ort EP feature 按平台条件化（mac=coreml/linux=cuda/win=directml）
```
（含 default-features 实测结论 + target 块放末尾的 TOML 正确性说明；Cargo.lock 顺带 webpki-root-certs 1.0.7→1.0.8 patch bump）

---

## Task 2: config.rs win 块删 CUDA 注册 ✅

**Files:**
- Modify: `crates/asr/src/config.rs`（`apply_session_acceleration` 的 win cfg 块 + 上方注释）

- [x] **Step 1: 改 win cfg 块（删 CUDA）+ 更新注释**

```rust
    // macOS=CoreML、Linux=CUDA、Windows=DirectML（Cargo feature 同步按平台条件化，见 Cargo.toml）。
    let mut providers = Vec::new();
    #[cfg(target_os = "macos")]
    providers.push(ort::ep::CoreMLExecutionProvider::default().build());
    #[cfg(target_os = "linux")]
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
    #[cfg(target_os = "windows")]
    providers.push(ort::ep::DirectMLExecutionProvider::default().build());
```

> **实测更正原前提**：原 plan/spec 称「win 无 cuda feature → `CUDAExecutionProvider` 类型不存在会编不过」。实测 ort EP 类型**无条件编译**（仅 `register()` 内 FFI 块按 feature gate），故留着也能编译（运行时 register 返回 `MissingFeature`→CPU fallback）。删除**非编译必需**，但仍是正确改动——避免注册一个注定失败的死 EP。详见 spec §5.3/§7.5。

- [x] **Step 2: mac 本地验证（win 块不参与 mac 编译，应仍过）**

Run: `cargo check -p octopus-asr`
结果：通过（1.37s）。

- [x] **Step 3: commit**（`21fb2fb`）

```
refactor(asr): win EP 收敛为仅 DirectML（删 CUDA 注册）
```

---

## Task 3: mac 本地完整验证 ✅

**Files:** 无改动，仅验证。

- [x] **Step 1: asr 单测不回归**

Run: `cargo test -p octopus-asr`
结果：**45 passed / 0 failed / 6 ignored**（无回归）。

- [x] **Step 2: desktop release 编译**

Run: `cargo build --release -p octopus-desktop`
结果：通过（5m07s，strip+lto+codegen-units 预期慢）。

- [x] **Step 3: desktop e2e（CoreML 加速正常、无 segfault 回归）—— 已通过（2026-06-20 用户本地）**

启动 desktop app → 触发录音快捷键 → 确认：转写出文字、日志含 `Successfully registered EPs!`、无 SIGSEGV。**结果：通过**——CoreML 加速正常工作、无 segfault 回归（本改动根治的点）。

- [x] **Step 4: 无代码改动不 commit；Step 1-3 全过**

---

## Task 4: linux/windows 交叉编译验证 ⚠️（受目标工具链阻塞，非 ort）

**Files:** 无改动，仅验证。

- [x] **Step 1: 安装交叉编译 target**

Run: `rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc`
结果：两个 target 安装成功。

- [x] **Step 2: linux 交叉 check —— 受阻**

Run: `cargo check -p octopus-asr --target x86_64-unknown-linux-gnu`
结果：**卡在 `openssl-sys` build script**——mac→linux 缺 openssl dev sysroot / pkg-config 交叉配置。非 ort、非本改动（openssl-sys 来自 ort 默认集 `tls-native`）。

- [x] **Step 3: windows 交叉 check —— 受阻**

Run: `cargo check -p octopus-asr --target x86_64-pc-windows-msvc`
结果：**卡在 `esaxx-rs` C++ build script**（jieba-rs 的 C++ 依赖）——mac 无 MSVC C++ 工具链，`clang++` 找不到 `<cstdint>`。非 ort、非本改动。

- [x] **Step 4: 降级处理（按 plan 原预案）**

两端均为目标平台 C/C++ 工具链缺失（非 ort-sys 下载问题）。feature 矩阵正确性改由：
- **源码 gate 结构核验**（spec §7.3）：ort EP 类型无条件编译、仅 `register()` FFI 块按 feature gate；
- **mac coreml 实证代理**：mac（有 coreml feature）编译通过，证明「feature present + 类型引用 → 编通过」模式成立，directml/cuda 同构。
- linux/win 运行正确性留用户在对应平台本地 `cargo check` + 自测（spec §7.2 已声明环境限制）。

**未**为绕过而改回全开 feature——那会破坏本改动目的。

- [x] **Step 5: 验证 task，无 commit**

---

## Task 5: mac 体积对比 ✅（结果：0 下降）

**Files:** 无改动，仅度量。

- [x] **Step 1: rebuild release 并对比**

Run: `cargo build --release -p octopus-desktop`（已在 Task 3 Step 2 产出）→ `stat` 二进制。
结果：**54M（56,778,304 字节），与基线（profile 优化后的 54M）相同，零下降**。

原因（spec §7.1）：cuda/directml feature 在 mac 是 size no-op——① 其 Rust 类型早被 config.rs `#[cfg]` 排除、从未编进 mac 二进制；② 其 GPU 预编译库（libcuda/cudnn/DirectML.dll）无 mac 版、从未下载/链接。本改动价值在 segfault defense-in-depth（§7.3），非体积。

- [x] **Step 2: 度量 task，无 commit；数据已记录**

---

## 已知限制（来自 spec §7）

- linux/win **运行未实测** + 交叉 check 受目标工具链阻塞（openssl-sys / esaxx-rs），运行正确性靠用户在对应平台自测。
- mac **体积收益 = 0**（cuda/directml 在 mac 是 size no-op）。
- feature↔#[cfg] **非编译器硬约束**；真正的保护是 `register()` 的 feature gate（defense-in-depth）。
- `default-features` 保留开启（实测后定）。
- win CUDA 删除**非编译必需**（类型存在），但运行时清洁，仍正确。
- win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。

## 📄 `2026-06-20-zipformer-transducer.md`

# Zipformer Transducer (RNN-T) 引擎 Implementation Plan

> 状态：**已实现**（离线 commit `465e901` merge `f238c47`；流式 commit `415e89c` merge `109cccb`；归一化 fix commit `0d7ef5c`）
> Spec：`docs/superpowers/specs/2026-06-20-zipformer-transducer-design.md`

**Goal:** 将原 `ZipformerEngine`（仅 CTC）重命名为 `ZipformerCtcEngine`，新增 `ZipformerTransducerEngine`（RNN-T 三 session 架构，离线 + 流式），支持两个新中文模型。

---

## File Structure（实际）

- `crates/asr/src/zipformer.rs` — 重命名 + 新增 Transducer struct + 共享函数提取
- `crates/asr/src/streaming_zipformer.rs` — 新增 `StreamingZipformerTransducer`（流式 RNN-T）
- `crates/asr/src/streaming_engine.rs` — `StreamingSession` 枚举新增 `ZipformerTransducer` 变体 + `ZipformerStreamOps` trait
- `crates/asr/src/engine.rs` — 路由更新（`decoder.onnx` 检测分流）
- `crates/infra/src/db.sql` — seed 新增两个 Transducer 模型
- `docs/architecture.md` / `docs/configuration.md` — 文档同步

---

## Tasks（已完成）

### Task 1: 重命名 ZipformerEngine → ZipformerCtcEngine ✅

- [x] `pub struct ZipformerEngine` → `pub struct ZipformerCtcEngine`
- [x] `impl ZipformerEngine` → `impl ZipformerCtcEngine`
- [x] `impl OfflineAsrEngine for ZipformerEngine` → `for ZipformerCtcEngine`
- [x] `transcribe()` 公开函数 + 测试中的引用更新
- [x] 验证：7 处引用全部改名，grep 无 `ZipformerEngine` 残留

### Task 2: 提取共享函数 ✅

- [x] `load_vocab(hf_path) -> Result<Vec<String>>` — tokens.txt 解析（从 CTC `new()` 提取）
- [x] `initial_encoder_states(session) -> Vec<(String, StateValue)>` — encoder 缓存初始化（从 CTC `new()` 提取）
- [x] `decode_token_ids(vocab, is_bbpe, ids) -> String` — token ID → 文本（BBPE + SentencePiece byte-fallback，从 CTC `transcribe()` 提取）
- [x] CTC `new()` / `transcribe()` 重构使用共享函数

### Task 3: 实现 ZipformerTransducerEngine ✅

- [x] struct 定义（三 `Mutex<Session>` + chunk_len/shift/context_size/vocab/is_bbpe/initial_states/is_whisper）
- [x] `new()`：发现 encoder/decoder/joiner 三文件，加载三 session，读 metadata（T/decode_chunk_len/feature/context_size），encoder_dim 从输出 shape 动态读
- [x] `run_decoder()` / `run_joiner()` 辅助方法
- [x] `impl OfflineAsrEngine::transcribe()`：
  - [x] 特征提取（compute_whisper_features_linear + normalize）
  - [x] Chunked encoder 推理（同 CTC：chunk 循环 + state 管理）
  - [x] RNN-T greedy decoding（token_buf 初始化 `[-1,...,-1,0]`，每 frame joiner→argmax，非 blank 发射 + 重跑 decoder）
  - [x] 内循环安全上限 20 次/frame
  - [x] token 解码（decode_token_ids 共享函数）

### Task 4: 路由更新 ✅

- [x] `engine.rs` import 改为 `{ZipformerCtcEngine, ZipformerTransducerEngine}`
- [x] `EngineCategory::Zipformer` 分支：检测 `decoder.onnx` 存在性分流

### Task 5: DB seed + 文档同步 ✅

- [x] `db.sql` seed 新增 `zipformer-zh-transducer`（154M）和 `zipformer-xlarge-transducer`（726M）
- [x] `architecture.md`：Zipformer 引擎族表格（CTC vs Transducer 对比）+ 流式判定描述更新
- [x] `configuration.md`：seed 表 + is_streaming 说明 + 模型下载命令
- [x] spec 文档：`docs/superpowers/specs/2026-06-20-zipformer-transducer-design.md`

### Task 6: 验证 ✅

- [x] `cargo build -p octopus-asr`：clean（0 warning）
- [x] `cargo build --release -p octopus-desktop --features "embedded dashscope"`：clean
- [x] `cargo build --release -p octopus-server -p octopus-cli`：clean
- [x] zh-int8 测试：`"对我做了介绍哈那么我想说的是大家如果对我的研究感兴趣呢"` ✓
- [x] xlarge 测试：`"给我做了介绍我想说的是大家如果对我的研究感兴趣"` ✓
- [x] `cargo test -p octopus-desktop --features "embedded dashscope"`：48 passed
- [x] ASR 测试：淗41 passed（3 pre-existing failures 因 HF cache 缺文件，非本次改动）

### Task 7: 流式 Transducer 引擎 ✅

- [x] `StreamingZipformerTransducer` struct（三 `Mutex<Session>` + 跨 chunk 状态 token_buf / emitted_ids / states）
- [x] `new_from_entry(entry)` — 避免 `StreamingSession::new` 双重 DB 查找
- [x] `process_chunks()` / `flush()` / `finish()` / `reset()` 生命周期方法
- [x] `run_chunk()` 两阶段借用（encoder output 提取为 owned Vec<f32> 后再调 decoder/joiner）
- [x] 流式 RNN-T greedy decoding（per frame joiner→argmax，非 blank 发射 + 重跑 decoder，内循环上限 20）
- [x] `StreamingSession` 枚举新增 `ZipformerTransducer` 变体
- [x] `ZipformerStreamOps` trait 抽象 CTC/Transducer 统一接口（accept/flush/finish/reset）
- [x] `streaming_engine.rs::new()` 检测 `decoder.onnx` 分流
- [x] 测试：`test_streaming_zipformer_transducer` 流式 partial 逐 chunk 增量输出 ✓

### Task 8: Whisper 特征归一化 3 根因修复 ✅

对比 sherpa-onnx 官方 C++ 实现（`math.cc` + `online-recognizer-transducer-impl.h`），定位并修复 3 个根因：

- [x] **根因 1：归一化公式错误** — `normalize_whisper_features` 最后一步从 `clamped - clamp_min`（范围 0-8）修正为 `(clamped + 4.0) / 4.0`（范围~0-2，与 sherpa-onnx 一致）
- [x] **根因 2：Transducer history 泄漏** — `process_chunks` 保留全部未消费样本改为仅保留最后 1 帧（与 CTC 引擎一致）
- [x] **根因 3：归一化 scope** — 从 pseudo-global（每次重算 history+buffer 全局归一化）回退为 per-chunk（与 sherpa-onnx 一致）
- [x] 覆盖 CTC + Transducer 两套流式引擎的 `process_chunks` 和 `finish` 共四处
- [x] `cargo build -p octopus-asr`：clean（0 warning）
- [x] 流式 Transducer 测试：输出从乱码变为与离线完全一致的可识别中文 ✓

### Task 9: 代码审核修复 ✅

经外部审核 + sherpa-onnx 源码对照，处理 3 项：

- [x] **测试路径硬编码 snapshot hash → 动态查找**：HF cache 用 commit hash 命名 snapshot 目录，硬编码特定 hash 在 hash 变化或部分拉取时 panic。改为 `hf_snapshot()` 辅助函数 `read_dir` 动态查找，找不到则 graceful skip。此前 3 个 CTC 测试因此失败，现在 45 passed 0 failed。
- [x] **重叠帧冗余解码——经查无需修改**：审核建议裁剪重叠帧输出，但 sherpa-onnx `online-ctc-greedy-search-decoder.cc` 同样解码全部 `num_out_frames`，依靠 CTC `y != prev_id` 去重。我们的实现一致，属于标准行为。
- [x] **离线全局归一化注释**：审核建议离线也改 per-chunk，但 sherpa-onnx 离线 Transducer 对 whisper 特征完全不做归一化，我们的 chunk 循环模拟需要归一化才能工作。加注释说明差异，避免误改。

---

## 实际实现偏离原 plan

1. **`encoder_dim` 字段移除**：原 plan 设计 struct 含 `encoder_dim` 字段，但 `transcribe()` 实际从 encoder 输出 shape 动态读 `enc_dim`（每 chunk 都读），struct 字段闲置触发 dead_code warning。删除 struct 字段，保留 `new()` 中的 shape 读取（仅用于初始化日志）。

2. **`ort::inputs!` 宏返回 Vec 非 Result**：原闭包写法 `ort::inputs!{...}?` 编译失败（ort 2.0.0-rc.12 的 `inputs!` 宏直接返回 `Vec`，不是 `Result`）。去掉 `?`。

3. **Session::run 需 &mut self**：原设计 decoder/joiner 用裸 `Session`，但 `run(&mut self)` 要求可变借用。改 `Mutex<Session>` 包裹（encoder 已是 `Mutex`），辅助方法内 `lock().unwrap()` 拿 `&mut Session`。

4. **闭包改方法**：原设计在 `transcribe()` 内用闭包 `run_decoder` / `run_joiner`，但 `&self` 借用 + `Mutex::lock` 生命周期复杂。改为 `impl` 方法 `fn run_decoder(&self, ...)` / `fn run_joiner(&self, ...)`，更清晰。

5. **`new_from_entry` 避免双重 DB 查找**：流式引擎原设计接收 bare_name 内部查 DB，但 `StreamingSession::new` 已通过 `resolve_active_engine` 解析出 entry。改为 `new_from_entry(entry)` 直接接收已解析 entry——避免双重查找 + 可能选错 entry（同名跨 provider 场景）。

6. **两阶段借用（run_chunk）**：ort 2.0.0-rc.12 的 `SessionOutputs` 持有 session 借用，调 decoder/joiner 前必须结束该借用。`run_chunk` 先从 encoder `SessionOutputs` 提取 encoder_out 到 owned `Vec<f32>`（借用结束），再用 owned 数据调 decoder/joiner session。

7. **`ZipformerStreamOps` trait**：原设计在 `StreamingSession` 的 accept/flush/finish/reset 中为 CTC 和 Transducer 各写一套分支，重复严重。提取 trait 统一分发，`StreamingSession` 仅持 `Box<dyn ZipformerStreamOps>`。

8. **Whisper 特征归一化 3 根因修复（P0 bug fix）**：对比 sherpa-onnx C++ 源码发现 3 个根因——①归一化公式错误（`clamped - clamp_min` → `(clamped + 4) / 4`，尺度差 4 倍）；②Transducer history 泄漏（保留全部未消费样本而非 1 帧，导致 max_v 跨 tick 跳变）；③归一化 scope（pseudo-global 回退为 per-chunk，与 sherpa-onnx 一致）。首次尝试用 pseudo-global 修复方向错误（以为 per-chunk 是 bug，实则 sherpa-onnx 恰恰用 per-chunk），系统性对比参考实现后定位真正根因。


