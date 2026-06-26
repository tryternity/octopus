# desktop 复用 cloud 协议层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 desktop 的 4 个 `*_stream.rs` + `cloud_types.rs` 协议层副本（共 5 文件），`CloudPipelineEngine` 改指 `octopus-asr-cloud` crate，消除协议层两份重复维护的技术债，零行为差异。

**Architecture:** cloud crate（第一步已合并 main）协议层零改动；desktop 仅在 `cloud_pipeline.rs` 用 `tauri::async_runtime::block_on` 包一层进入 tokio context 后调 cloud crate 的 `open_cloud_session`（方案 B）；`CloudStreamHandle`/`StreamEvent` 类型源从 `crate::cloud_types` 切到 `octopus_asr_cloud`；cloud crate 加一个 `#[doc(hidden)] pub fn new_for_test()` 供 desktop 的 5 个 drain 测试构造预载事件的 handle。

**Tech Stack:** Rust workspace；`octopus-asr-cloud`（tokio + tokio-tungstenite WSS）、`octopus-desktop`（tauri 2，`cloud` feature gate）；tauri async_runtime 即 tokio。

**Spec:** `docs/superpowers/specs/2026-06-25-desktop-cloud-dedupe-design.md`
**Worktree:** `worktree-model-mgmt-ui`（已就位，叠加分支）

> **实施状态**：✅ 已合并 main（`6a4593e`，ff-merge）。Task 1-6 全完成，云端流式 e2e 2026-06-25 本地云端 key 验证通过。
>
> **实施修正**（vs 原 plan，3 处盲点 + 1 时序）：
> - **Task 2 时序**：原"接入+瘦身"合一，删 flate2/hmac/sha1 deps 时 `bytedance_stream`/`tencent_stream` 副本仍 `use` 它们→编译断。拆为 Task 2 仅接入 octopus-asr-cloud，瘦身随 Task 4 删副本（`57685df`）。
> - **Task 3 盲点**：`pipeline.rs` 的 `StreamingPipelineEngine::take_close_handle` trait 签名（+ 包装方法）也写死 `crate::cloud_types::CloudStreamHandle`，须同步切 `octopus_asr_cloud`（否则 E0053 trait 类型不匹配）。cloud crate `lib.rs` 补 `CloudStreamHandle`/`StreamEvent` 顶层 re-export（`2e15bfd`）。
> - **Task 4 盲点**：`engine_aliyun.rs`（chunk 模式）复用 `aliyun_stream::is_qwen_realtime_endpoint` + `cloud_types::samples_to_pcm_s16le`（原以为零改动）。改指 cloud crate；cloud crate 顺势把这两个 helper `pub(crate)`→`pub` + re-export `samples_to_pcm_s16le`（`c5b73cf`）。

---

## 文件结构

| 文件 | 动作 | 责任 |
|---|---|---|
| `crates/asr-cloud/src/cloud_types.rs` | 改（加 1 fn） | 加 `new_for_test` 测试构造器（D2） |
| `crates/desktop/Cargo.toml` | 改 | cloud feature 接入 `octopus-asr-cloud` + 瘦身 flate2/hmac/sha1 |
| `crates/desktop/src/cloud_pipeline.rs` | 改 | use 源切 cloud crate + 删 5 resolve fn + open_cloud_session 改 block_on wrapper + tests 改 new_for_test |
| `crates/desktop/src/coordinator.rs` | **不改** | 靠类型推断，编译验证零改动 |
| `crates/desktop/src/main.rs` | 改 | 删 5 个 `#[cfg(feature="cloud")] mod *_stream/cloud_types` |
| `crates/desktop/src/{aliyun_stream,bytedance_stream,tencent_stream,baidu_stream,cloud_types}.rs` | **删** | 协议层副本（cloud crate 1:1） |

每个 Task 末尾编译通过 + commit（搬迁为主，frequent commits）。

---

## Task 1: cloud crate 加 `new_for_test` 测试构造器

**Files:**
- Modify: `crates/asr-cloud/src/cloud_types.rs`（`impl CloudStreamHandle` 块内 `new()` 旁加 `new_for_test`；tests mod 加 1 测试）

**Why:** desktop 删 `cloud_types.rs` 后，其 `cloud_pipeline.rs` 5 个 drain 测试需跨 crate 构造预载 `StreamEvent` 的 `CloudStreamHandle`。cloud crate 的 `new()` 是 `pub(crate)` 且返回类型含 `pub(crate) PcmFrame`，无法直接暴露；新增只返回 `(Self, UnboundedSender<StreamEvent>)` 的 `new_for_test` 绕过此约束。

- [x] **Step 1: 写失败测试（验证 new_for_test 返回的 sender 能投递到 handle）**

在 `crates/asr-cloud/src/cloud_types.rs` 的 `#[cfg(test)] mod tests`（文件末尾 `}` 前）追加：

```rust
    #[test]
    fn new_for_test_returns_handle_and_event_sender() {
        // new_for_test 构造的 (handle, sender)：sender 预载事件后 handle.try_recv_text 能取到。
        // 供跨 crate（desktop cloud_pipeline 测试）构造预载事件的 handle。
        let (mut handle, tx) = CloudStreamHandle::new_for_test();
        let _ = tx.send(StreamEvent::Text("hello".to_string()));
        assert!(
            matches!(handle.try_recv_text(), Some(StreamEvent::Text(t)) if t == "hello"),
            "new_for_test 预载的事件应能被 try_recv_text 取到"
        );
    }
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr-cloud new_for_test`
Expected: 编译失败 `no function named new_for_test`（方法尚未定义）。

- [x] **Step 3: 实现 new_for_test**

在 `crates/asr-cloud/src/cloud_types.rs` 的 `impl CloudStreamHandle {` 块内、`pub(crate) fn new(...)` 之后插入：

```rust
    /// 仅供测试：构造 handle + result 发送端（预载事件用）。不暴露 pcm_rx / `pub(crate) PcmFrame`。
    ///
    /// 返回 `(handle, result_tx)`：测试向 `result_tx` 投递 `StreamEvent` 后，`handle.try_recv_text`
    /// 可取到。供 desktop `cloud_pipeline::handle_with_events` 等 drain 测试跨 crate 构造预载 handle。
    #[doc(hidden)]
    pub fn new_for_test() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (handle, _pcm_rx, result_tx) = Self::new();
        (handle, result_tx)
    }
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr-cloud`
Expected: PASS（含新增 `new_for_test_returns_handle_and_event_sender` + 原 3 个 cloud_types 测试，共 ≥4）。

- [x] **Step 5: Commit**

```bash
git add crates/asr-cloud/src/cloud_types.rs
git commit -m "feat(asr-cloud): 加 CloudStreamHandle::new_for_test 测试构造器

供 desktop cloud_pipeline drain 测试跨 crate 构造预载事件的 handle。
#[doc(hidden)] pub，返回 (Self, UnboundedSender<StreamEvent>)，
不暴露 pub(crate) PcmFrame。desktop-cloud-dedupe 第二步 D2。"
```

---

## Task 2: desktop Cargo.toml 接入 cloud crate + 瘦身

**Files:**
- Modify: `crates/desktop/Cargo.toml`（`[dependencies]` 删 flate2/hmac/sha1 + 加 octopus-asr-cloud；`[features]` cloud 改写；注释同步）

**Why:** desktop 改指 cloud crate 后需声明依赖；`flate2`/`hmac`/`sha1` 仅被待删的 `bytedance_stream.rs`/`tencent_stream.rs` 直接 use（已 grep 确认），删副本后 desktop 不再直接用，从 cloud feature 与 `[dependencies]` 移除（cloud crate 自身依赖它们，作为 transitive dep 仍编译可用）。`tokio-tungstenite`/`uuid`/`base64`/`futures-util` 仍被 `engine_aliyun.rs`（cloud，不删）/`engine_ws.rs`（remote-ws）/`settings_commands.rs` 用，**保留**。

- [x] **Step 1: 改 [dependencies]——删 flate2/hmac/sha1，加 octopus-asr-cloud**

把 `crates/desktop/Cargo.toml` 的云端 WS 依赖段（当前 L50-59）：

```toml
# 云端 ASR WS engine（cloud feature 用）
# uuid 用于生成 task_id / event_id / request_id / voice_id；走 wss:// 必须 TLS。
# base64 用于 Qwen-ASR Realtime 协议（audio 字段为 base64 PCM）+ Tencent 签名 Base64。
# flate2 用于 ByteDance ASR 二进制协议（gzip 压缩 payload）。
# hmac + sha1 用于 Tencent ASR 签名鉴权（HMAC-SHA1）。
uuid = { version = "1", features = ["v4"], optional = true }
base64 = { version = "0.22", optional = true }
flate2 = { version = "1", optional = true }
hmac = { version = "0.12", optional = true }
sha1 = { version = "0.10", optional = true }
```

替换为：

```toml
# 云端 ASR WS engine（cloud feature 用）
# uuid 用于生成 task_id / event_id / request_id / voice_id；走 wss:// 必须 TLS。
# base64 用于 Qwen-ASR Realtime 协议（audio 字段为 base64 PCM）+ Tencent 签名 Base64。
#（flate2/hmac/sha1 已随 *_stream.rs 副本删去，下沉 octopus-asr-cloud；engine_aliyun.rs chunk 模式仅需 uuid/base64。）
uuid = { version = "1", features = ["v4"], optional = true }
base64 = { version = "0.22", optional = true }

# 云端 ASR 协议层（4 provider WSS + 批引擎，下沉 crate；desktop cloud feature 复用）
octopus-asr-cloud = { path = "../asr-cloud", optional = true }
```

- [x] **Step 2: 改 [features].cloud——删 flate2/hmac/sha1，加 octopus-asr-cloud**

把当前 cloud feature 行（L81）：

```toml
cloud = ["tokio-tungstenite", "tokio-tungstenite?/native-tls", "uuid", "futures-util", "base64", "flate2", "hmac", "sha1"]
```

替换为：

```toml
# 云端 ASR WS 流式识别（Aliyun / ByteDance / Tencent / Baidu）：
# 协议层下沉 octopus-asr-cloud（4 provider WSS 1:1 复刻自原 desktop *_stream.rs）。
# tokio-tungstenite 启用 native-tls 以支持 wss://（engine_aliyun chunk 模式 + settings 连接测试亦用）。
cloud = ["tokio-tungstenite", "tokio-tungstenite?/native-tls", "uuid", "futures-util", "base64", "octopus-asr-cloud"]
```

- [x] **Step 3: 验证依赖解析 + cloud feature 编译（desktop 代码尚未改，应仍通过）**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功（此时 desktop 仍用 `crate::cloud_types` 副本，新依赖 `octopus-asr-cloud` 仅被引入未使用，不影响编译；可能有无害的 unused warning，下一步代码改完即消）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/Cargo.toml
git commit -m "build(desktop): cloud feature 接入 octopus-asr-cloud + 瘦身 flate2/hmac/sha1

协议层下沉 cloud crate 后，flate2/hmac/sha1 仅 cloud crate 自身依赖（transitive），
desktop 不再直接 use（原仅 bytedance_stream/tencent_stream 副本用）。
tokio-tungstenite/uuid/base64/futures-util 保留（engine_aliyun/engine_ws/settings_commands 用）。
desktop-cloud-dedupe 第二步 D3。"
```

---

## Task 3: cloud_pipeline.rs 改造（use 源 + open wrapper + 删 resolve + tests）

**Files:**
- Modify: `crates/desktop/src/cloud_pipeline.rs`（use 区 L8-13；删 L113-177 共 5 个 resolve fn；open_cloud_session L181-213 改写；tests 4 处 `new()` 调用改 `new_for_test()`）

**Why:** 这是搬迁核心：类型源切到 cloud crate，配置解析/open 分发改调 cloud crate，`CloudPipelineEngine`/drain 逻辑零改动。改后 `crate::cloud_types` 不再被 `cloud_pipeline.rs` 引用（`cloud_types.rs` 文件本身暂留，下一 Task 删，期间为 dead code 但编译通过）。

- [x] **Step 1: 改 use 区——切 cloud crate，删 RuntimeHandle**

把 `crates/desktop/src/cloud_pipeline.rs` 顶部 use 区（L8-13）：

```rust
use crate::cloud_types::{CloudStreamHandle, StreamEvent};
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use tauri::async_runtime::RuntimeHandle;
```

替换为：

```rust
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};
```

- [x] **Step 2: 删 5 个 resolve fn（L110-177 整段）**

删除从注释 `// ── open/resolve helpers（迁自 coordinator.rs:1504-1617...`（约 L110）到 `resolve_baidu_config` 结束 `}`（L177）的整段——含：`resolve_cloud_entry`、`resolve_aliyun_config`、`resolve_bytedance_config`、`resolve_tencent_config`、`resolve_baidu_config` 共 5 个 fn（这些 cloud crate `config.rs` 已有等价物）。

删除后，该区域紧接着 `take_preroll` fn（L108 结束）之后直接是改造后的 `open_cloud_session`（见 Step 3）。

- [x] **Step 3: open_cloud_session 改 block_on 瘦 wrapper（方案 B）**

把原 `open_cloud_session`（删完 resolve 后，原 L181-213 的整段）替换为：

```rust
/// onset dispatch：根据引擎 spec 打开对应云端 WSS session（返回句柄）。
///
/// cloud crate 的 `open_cloud_session` 内部 `tokio::spawn`，**须在 tokio context**；
/// coordinator 主线程非 tokio，用 `tauri::async_runtime::block_on` 进入（tauri runtime 即 tokio）。
/// `block_on` 内同步 `open` 只 spawn reader task + 返回 channel handle（不 await 建连），立即返回，
/// 不阻塞 coordinator 主线程。
pub(super) fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle, String> {
    tauri::async_runtime::block_on(async {
        octopus_asr_cloud::open_cloud_session(asr_engine, language, pre_roll)
    })
    .map_err(|e| e.to_string())
}
```

- [x] **Step 4: tests 改 new_for_test（4 处）**

`crates/desktop/src/cloud_pipeline.rs` tests mod 里，所有 `CloudStreamHandle::new()` 调用（返回三元组 `(handle, _pcm_rx, result_tx)`）改为 `CloudStreamHandle::new_for_test()`（返回二元组 `(handle, result_tx)`）。共 4 处：

(a) `handle_with_events` helper（约 L407-413）：

```rust
    fn handle_with_events(events: Vec<StreamEvent>) -> CloudStreamHandle {
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
        for ev in events {
            let _ = result_tx.send(ev);
        }
        handle
    }
```

(b)(c)(d) `drain_finished_emits_committed_with_comma`、`drain_finished_no_double_comma_when_committed_ends_with_comma`、`drain_failed_emits_error_clears_partial` 三个测试里各自的：

```rust
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
```

替换为：

```rust
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
```

（共 3 处，逐个替换；`_pcm_rx` 不再存在因 `new_for_test` 不返回它。）

- [x] **Step 5: cloud feature 编译验证（coordinator 应零改动）**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功。`coordinator.rs` 靠类型推断（L845 `if let Some(handle) = pipeline.take_close_handle()` + L858 `handle.close_async().await`），`CloudStreamHandle` 类型由 `cloud_pipeline::CloudPipelineEngine::take_close_handle` 返回类型推断而来，无需 `use`，**预期零改动**。

若编译报 `coordinator.rs` 缺 `CloudStreamHandle` 类型：在 `coordinator.rs` 顶部 use 区加 `#[cfg(feature = "cloud")] use octopus_asr_cloud::CloudStreamHandle;`（但据 grep 确认 coordinator 无显式类型标注，不应需要）。

- [x] **Step 6: cloud_pipeline 8 测试验证**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline`
Expected: PASS（8 个：`drain_text_updates_partial_no_event` / `drain_finished_emits_committed_with_comma` / `drain_finished_no_double_comma_when_committed_ends_with_comma` / `drain_finished_no_partial_no_event_no_comma` / `drain_failed_emits_error_clears_partial` / `onset_confirmed_requires_two_consecutive` / `should_send_finish_only_when_speaking_not_closing_silence_enough` / `take_preroll_last_n_samples`）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/cloud_pipeline.rs
git commit -m "refactor(desktop): cloud_pipeline 改指 octopus-asr-cloud 协议层

use 源 crate::cloud_types → octopus_asr_cloud；删 5 个 resolve_*_config（cloud crate 有）；
open_cloud_session 改 block_on 瘦 wrapper（方案 B：tauri runtime 进 tokio context）；
8 测试 new() → new_for_test()。CloudPipelineEngine/drain 逻辑零改动。
coordinator 靠类型推断零改动。desktop-cloud-dedupe 第二步 D2+D3。"
```

---

## Task 4: 删 main.rs 5 mod + 删 5 个协议副本文件

**Files:**
- Modify: `crates/desktop/src/main.rs`（删 5 个 `#[cfg(feature = "cloud")] mod *_stream / cloud_types`）
- Delete: `crates/desktop/src/aliyun_stream.rs`、`bytedance_stream.rs`、`tencent_stream.rs`、`baidu_stream.rs`、`cloud_types.rs`

**Why:** Task 3 后 `cloud_pipeline.rs` + `coordinator.rs` 已不引用 `crate::cloud_types` / `crate::*_stream`，可安全删 mod 与文件。保留 `mod engine_aliyun`（chunk 模式离线引擎，另一套）+ `mod cloud_pipeline`（改造后留）。

- [x] **Step 1: 删 main.rs 的 5 个 cloud mod**

`crates/desktop/src/main.rs` 的 mod 区当前（L3-20 节选）：

```rust
mod audio;
mod config;
mod coordinator;
#[cfg(feature = "cloud")]
mod aliyun_stream;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod bytedance_stream;
#[cfg(feature = "cloud")]
mod tencent_stream;
#[cfg(feature = "cloud")]
mod baidu_stream;
#[cfg(feature = "cloud")]
mod cloud_types;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

删除 `aliyun_stream`、`bytedance_stream`、`tencent_stream`、`baidu_stream`、`cloud_types` 共 5 个 `#[cfg(feature = "cloud")] mod xxx;`（含各自上方 cfg 行）。改后 mod 区：

```rust
mod audio;
mod config;
mod coordinator;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

- [x] **Step 2: 删 5 个协议副本文件**

```bash
git rm crates/desktop/src/aliyun_stream.rs crates/desktop/src/bytedance_stream.rs crates/desktop/src/tencent_stream.rs crates/desktop/src/baidu_stream.rs crates/desktop/src/cloud_types.rs
```

Expected: 5 个文件删除，staged。

- [x] **Step 3: 双 feature 编译验证**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功（cloud on：协议层走 octopus-asr-cloud，`engine_aliyun`/`cloud_pipeline` 仍在）。

Run: `cargo build -p octopus-desktop`
Expected: 编译成功（cloud off：5 个 mod 与 cloud_pipeline/engine_aliyun 都不编译，default=embedded）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "refactor(desktop): 删 4 个 *_stream.rs + cloud_types.rs 协议层副本

协议层单源下沉 octopus-asr-cloud，消除两份字节级副本的技术债。
main.rs 删 5 个 #[cfg(cloud)] mod；保留 engine_aliyun（chunk 模式离线引擎，另一套）+
cloud_pipeline（流式适配，Task 3 改造）。desktop-cloud-dedupe 第二步 D3。"
```

---

## Task 5: 全量验证（双 feature build/clippy/test + workspace check）

**Files:** 无代码改动（仅验证；若有 clippy/编译修复则改对应文件）

- [x] **Step 1: cloud on 全 target 构建 + clippy**

Run: `cargo build -p octopus-desktop --features cloud --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --features cloud --all-targets`
Expected: desktop 新代码（cloud_pipeline.rs）0 warning。预存的 infra/llm/asr warning 与本次无关（cloud 协议层本就零 warning）。

- [x] **Step 2: cloud off 构建（default embedded）**

Run: `cargo build -p octopus-desktop --all-targets`
Expected: 0 error（cloud 副本已删，cloud off 不受影响）。

- [x] **Step 3: asr-cloud 30 测试不变**

Run: `cargo test -p octopus-asr-cloud`
Expected: PASS（Task 1 加的 new_for_test 测试 + 原 30 个，无回归）。

- [x] **Step 4: desktop cloud_pipeline 8 测试**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline`
Expected: PASS（8 个）。

- [x] **Step 5: workspace check**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

- [x] **Step 6: 若 Step 1-5 发现问题则修复并 commit；全绿则进 Task 6**

若 clippy 报新 warning 或编译错误，修复后：

```bash
git add <修复的文件>
git commit -m "fix(desktop): desktop-cloud-dedupe 全量验证修复"
```

若全绿，无 commit，直接进 Task 6。

---

## Task 6: e2e 回归清单（交付用户）+ 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-06-25-desktop-cloud-dedupe-design.md`（横幅状态）
- Modify: `docs/superpowers/plans/2026-06-25-desktop-cloud-dedupe.md`（横幅 + Task checkbox）
- 不改代码

**Why:** GUI/网络集成无自动化 e2e；云端流式需用户本地云端 key 验证。交付手动清单 + 同步文档状态（z_sync_superpowers 精神）。

- [x] **Step 1: 编译 desktop release（cloud）交付用户跑 e2e**

Run: `cargo build -p octopus-desktop --features cloud --release`
Expected: release 二进制生成（交付用户本地云端 key 验证）。

- [x] **Step 2: 交付 e2e 手动验证清单（不自动化）**

向用户提供以下清单（用户本地云端 key 跑），覆盖云端流式全路径 + 本地流式回归：

```
desktop cloud e2e 回归（--features cloud release 二进制）：
1. 云端流式 onset：选云端引擎（aliyun/bytedance/tencent/baidu 之一），说话 → 结果窗口"正在聆听…"→ partial 实时更新（验证 open_cloud_session block_on 路径 + push_pcm）
2. 云端流式 finish：停说 ≥ pause_polish_threshold → 句末提交、逗号拼接、进 DB（验证 drain Finished → Committed）
3. 云端 close：Toggle 停止 → spawn close_async → CloudStreamingDone finalize + 粘贴（验证 take_close_handle + close_async + Stage::CloudClosing 护栏）
4. 云端识别失败恢复：模拟断网/错误 key → "⚠️ 云端识别失败" 提示，下次 onset 重试（验证 drain Failed）
5. 本地流式回归：切本地引擎（embedded）→ 流式识别正常（验证 cloud 改造不影响 local 路径）
6. cloud off 回归：cargo build（无 cloud）→ embedded 本地识别正常（验证 default 路径不受影响）
```

> ✅ **2026-06-25 e2e 验证通过**：用户本地云端 key 跑全 6 项，云端流式 + 本地流式 + cloud off 全路径正常。

- [x] **Step 3: 同步 spec 横幅状态**

`docs/superpowers/specs/2026-06-25-desktop-cloud-dedupe-design.md` 顶部横幅：

```
> **状态**：设计待实施。
```

改为：

```
> **状态**：已实现（8 task 全完成，Task 1-5 编译/测试通过；e2e 待用户本地云端 key 验证）。
```

- [x] **Step 4: 同步 plan 横幅 + Task checkbox**

`docs/superpowers/plans/2026-06-25-desktop-cloud-dedupe.md` 顶部加横幅（Goal 上方）：

```
> **状态**：已实现（Task 1-5 编译/测试通过；Task 6 e2e 待用户本地云端 key 验证）。
```

并把 Task 1-5 所有 `[ ]` 改 `[x]`（Task 6 的 Step 1-3 改 `[x]`，Step 2「交付 e2e 清单」保持 `[ ]` 标注待用户验证）。

- [x] **Step 5: Commit 文档同步**

```bash
git add docs/superpowers/specs/2026-06-25-desktop-cloud-dedupe-design.md docs/superpowers/plans/2026-06-25-desktop-cloud-dedupe.md
git commit -m "docs(desktop-cloud-dedupe): 同步实施状态（Task 1-5 通过，e2e 待本地云端 key）"
```

---

## Self-Review

**1. Spec coverage:**
- §3 D1 方案 B（block_on）→ Task 3 Step 3 ✅
- §4 D2 类型归属（use 源切换）→ Task 3 Step 1 ✅；new_for_test → Task 1 ✅
- §5.1 删 5 文件 → Task 4 Step 2 ✅；删 5 mod → Task 4 Step 1 ✅；Cargo.toml 瘦身 → Task 2 ✅
- §5.2 cloud_pipeline use/resolve/open/tests → Task 3 ✅
- §5.3 cloud crate 加 new_for_test → Task 1 ✅（协议层零改动，仅 1 fn）
- §5.4 依赖边界（单向）→ Task 2 Cargo.toml ✅
- §8 验证清单（双 feature build/clippy + 8 测试 + 30 测试 + workspace check + e2e）→ Task 5 + Task 6 ✅
- coordinator 零改动（spec §4.1 隐含）→ Task 3 Step 5 验证 ✅

**2. Placeholder scan:** 无 TBD/TODO；每步含完整代码或精确命令 + 预期输出。✅

**3. Type consistency:**
- `new_for_test` 签名 `(Self, UnboundedSender<StreamEvent>)` —— Task 1 定义、Task 3 Step 4 使用（二元组 destructuring），一致 ✅
- `open_cloud_session` 返回 `Result<CloudStreamHandle, String>` —— Task 3 定义，`CloudPipelineEngine::tick` 调用点（L307，未改）期望该签名，一致 ✅
- `CloudStreamHandle`/`StreamEvent` 全部源自 `octopus_asr_cloud` —— Task 3 Step 1 use 后，drain/tests/take_close_handle 统一，一致 ✅
