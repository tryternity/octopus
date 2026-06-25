# 云端 ASR 下沉 cli 实施计划（`octopus-asr-cloud` crate）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `octopus-asr-cloud` crate（4 provider WSS 协议层 + 批引擎），让 cli 转译音频文件可选云端 ASR（DashScope/ByteDance/Tencent/Baidu），desktop 本次零改动。

**Architecture:** 协议层从 desktop `*_stream.rs` 1:1 复刻（仅改 spawn 方式），`CloudBatchEngine impl asr::OfflineAsrEngine`（单段→单 WSS session→完整文本，分段由 `asr::pipeline::transcribe_segments` 自动完成）；cli 层做本地/云端分流，两端都产出 `dyn OfflineAsrEngine` 喂 `transcribe_batch`。依赖单向 `asr ← cloud`。

**Tech Stack:** Rust workspace；tokio + tokio-tungstenite(native-tls)；复用 `octopus-asr`（trait/config）+ `octopus-infra`（ModelEntry/parse_model_spec）。

**关联 spec：** `docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md`。

---

## 实施时对 spec 措辞的两点据实修正（核对 desktop 源码后）

1. **`open()` 保持同步、非 async**（spec §4.1 写的是 `async fn open`）。核对 `crates/desktop/src/{aliyun,bytedance,tencent,baidu}_stream.rs`：各 `open()` 仅做 `CloudStreamHandle::new()` + `spawn(session task)` + 立即返回 handle，**不 await 任何 future**。故 cloud crate 的 `open()` 也保持同步签名，唯一改造是：去掉 `rt: &tauri::async_runtime::RuntimeHandle` 参数，`rt.spawn(...)` → `tokio::spawn(...)`。真正的 async 收尾在 `CloudStreamHandle::close_async`。语义与 spec 一致，措辞更省事。

2. **CloudBatchEngine 不自己 VAD 分段**（spec §4.2 倾向"复用 segment_audio_vad"）。核对 `crates/asr/src/pipeline.rs:73` `transcribe_segments`：它已实现 VAD 分段 + CJK/非 CJK 连接，并对**每段**调 `engine.transcribe(seg)`。cli 调用链 `transcribe_batch → transcribe_segments → cloud_engine.transcribe(seg)` 会自动分段。故 `CloudBatchEngine::transcribe` 的语义是「**单段**音频（≤30s，由上层保证）→ 单个 WSS session → 完整文本」，无需自己分段、无需自己拼接。大幅简化批引擎。

3. **`is_cloud_spec` / `from_spec` 用 `parse_model_spec` 的 3-part provider 前缀判断，不查 DB**（spec §4.3 倾向"复用 resolve_engine_category"）。核对 `crates/infra/src/db.rs:239-252` `parse_model_spec`：**2-part**（1 个冒号，如 `"aliyun:qwen-asr"`）按 `NameOnly` 兜底——provider 字段丢失。故云端分流必须用 **3-part spec**（`provider:category:model_name`，如 `aliyun:Fun-ASR:fun-asr-realtime`；category 见 `asr/config.rs:299 category_label`：Aliyun=Fun-ASR / ByteDance=Doubao-ASR / Tencent=Tencent-ASR / Baidu=Baidu-ASR）。用 `parse_model_spec` 取 3-part 的 `provider` 字段判云端，**不调 `resolve_engine_category`**——后者内部 `load_config()` 查 DB，会让分流与单测依赖 DB 命中状态。2-part/裸名 → `NameOnly` → 非云端（走本地分支，本地 `switch_model` 对云端裸名 bail，与现状一致）。`from_spec` 同此判定、不查 DB；DB 查找推迟到 `transcribe` 内 `open_cloud_session`（resolve_*_config）。

---

## 两个开放项的最终结论（已核对源码）

| 开放项 | 结论 | 依据 |
|---|---|---|
| 批引擎音频策略 | 单段单 session，分段交给 `transcribe_segments` | `asr/pipeline.rs:73-143` 已做 VAD 分段+CJK 连接，每段调 `engine.transcribe(seg)` |
| `skip_corrector` | `CloudBatchEngine::skip_corrector() -> true` | 桌面端云端流式不走 `transcribe_batch`、从不用 corrector；云端结果质量高，本地拼音纠错对齐「跳过」 |

---

## File Structure（本次 crate）

```
crates/asr-cloud/                 # 新建 crate
├── Cargo.toml                    # Task 1
└── src/
    ├── lib.rs                    # Task 1（mod + re-export，逐 task 补）
    ├── cloud_types.rs            # Task 1（迁自 desktop/cloud_types.rs）
    ├── aliyun_stream.rs          # Task 2（复刻 desktop/aliyun_stream.rs）
    ├── bytedance_stream.rs       # Task 3（复刻 desktop/bytedance_stream.rs）
    ├── tencent_stream.rs         # Task 4（复刻 desktop/tencent_stream.rs）
    ├── baidu_stream.rs           # Task 4（复刻 desktop/baidu_stream.rs）
    ├── config.rs                 # Task 5（resolve_*_config + open_cloud_session）
    └── batch.rs                  # Task 6（CloudBatchEngine impl OfflineAsrEngine）
```

修改的既有文件：
- `Cargo.toml`（workspace members，Task 1）
- `crates/asr/src/engine.rs`（加 `active_engine` getter，Task 7）
- `crates/cli/Cargo.toml`（加 octopus-asr-cloud 依赖，Task 7）
- `crates/cli/src/pipeline.rs`（本地/云端分流，Task 7）
- 文档（Task 8）：spec 横幅、`docs/architecture.md`、记忆。

**desktop 本次零改动**：`crates/desktop/src/{aliyun,bytedance,tencent,baidu}_stream.rs`/`cloud_types.rs`/`cloud_pipeline.rs` 副本暂留（第二步再合并）。

---

## 复刻通用规则（Task 2/3/4 共用，避免每个 task 重复）

每个 `*_stream.rs` 从 desktop 复制到 asr-cloud 时，做且仅做以下改造：

1. **`use` 路径**：`use crate::cloud_types::{...}` 不变（cloud crate 内 `cloud_types` 同名模块）。
2. **`open()` 签名**：去掉首参 `rt: &tauri::async_runtime::RuntimeHandle`；函数体 `rt.spawn(async move {...})` → `tokio::spawn(async move {...})`；其余（参数、返回 `Result<CloudStreamHandle>`、内部 `CloudStreamHandle::new()` + 分发 + 错误 `tx_for_err.send(StreamEvent::Failed(...))`）**逐字照搬**。
3. **`run_xxx_session` 及全部 helper**：**逐字照搬**（协议字节级、鉴权算法、帧格式、WS 收发循环 1:1，零行为差异）。
4. 模块文档头注释里"tauri::async_runtime"措辞改为"tokio runtime（调用方 block_on 驱动）"。

> 复制源用 Read 读 desktop 对应文件全文，整体粘贴后做上述 4 点改造。不要手抄协议常量/帧格式——必须从源文件复制，保证字节级一致。

---

## Task 1: 建 `octopus-asr-cloud` crate 骨架 + cloud_types 迁移

**Files:**
- Create: `crates/asr-cloud/Cargo.toml`
- Create: `crates/asr-cloud/src/lib.rs`
- Create: `crates/asr-cloud/src/cloud_types.rs`
- Modify: `Cargo.toml`（workspace members）

- [ ] **Step 1: 注册 workspace member**

编辑 `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/model-mgmt-ui/Cargo.toml`，把 `members` 行改为：

```toml
members = ["crates/infra", "crates/asr", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download"]
```

- [ ] **Step 2: 写 crate Cargo.toml**

Create `crates/asr-cloud/Cargo.toml`（依赖版本对齐 `crates/desktop/Cargo.toml`）：

```toml
[package]
name = "octopus-asr-cloud"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-asr = { path = "../asr" }
octopus-infra = { path = "../infra" }

# Async + WSS（wss:// 需 native-tls）
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"

# 协议层依赖（与 desktop cloud feature 一致）
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
base64 = "0.22"
flate2 = "1"
hmac = "0.12"
sha1 = "0.10"

# 通用
anyhow = "1"
log = "0.4"
```

- [ ] **Step 3: 写 lib.rs 骨架**

Create `crates/asr-cloud/src/lib.rs`：

```rust
//! 云端 ASR（cli/server 批处理用）。
//!
//! 4 provider（Aliyun/ByteDance/Tencent/Baidu）WSS 协议层 + 批引擎（impl
//! `octopus_asr::engine::OfflineAsrEngine`）。协议层从 `octopus-desktop` 复刻
//!（见各 `*_stream.rs`），改造为不依赖 tauri runtime：`open()` 内部用 `tokio::spawn`，
//! 调用方（`CloudBatchEngine`）在自有 tokio runtime 上 `block_on` 驱动。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md`。

pub mod cloud_types;
```

- [ ] **Step 4: 写 cloud_types 测试（先写测试，TDD）**

先 Read `crates/desktop/src/cloud_types.rs` 全文确认内容（本 task 迁移它）。然后 Create `crates/asr-cloud/src/cloud_types.rs`，**整体复制 desktop 版本**，做以下改造：
- `pub(crate) enum PcmFrame` → `pub enum PcmFrame`（cloud crate 内部跨模块用，但保持 pub(crate) 亦可；本 task 保持 `pub(crate)`，与 desktop 一致）。
- `pub(crate) fn samples_to_pcm_s16le` → 保持 `pub(crate)`。
- 顶部模块文档注释把"coordinator → 后台 WS task"措辞保留（语义仍成立：CloudBatchEngine 扮演 coordinator 角色推音频）。
- `use anyhow::{anyhow, bail, Result};` / `use tokio::sync::mpsc;` 不变。
- 3 个单测（`test_samples_to_pcm_s16le_empty/basic/clamp`）**逐字复制**。

最终 `cloud_types.rs` 内容 = desktop `cloud_types.rs` 全文（含 `PcmFrame`/`StreamEvent`/`CloudStreamHandle`/`CLOUD_CLOSE_TIMEOUT_SECS`/`samples_to_pcm_s16le` + tests），无需任何逻辑改动（该文件不依赖 tauri）。

- [ ] **Step 5: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib`
Expected: 3 passed（samples_to_pcm_s16le 三个单测），0 failed。

- [ ] **Step 6: workspace check 确认注册无误**

Run: `cargo check -p octopus-asr-cloud`
Expected: 编译通过（cloud_types 无 tauri 依赖，应干净通过）。

- [ ] **Step 7: Commit**

```bash
git add crates/asr-cloud Cargo.toml
git commit -m "feat(asr-cloud): 新建 crate 骨架 + 迁移 cloud_types（PcmFrame/StreamEvent/CloudStreamHandle）"
```

---

## Task 2: 协议层 aliyun（DashScope）

**Files:**
- Create: `crates/asr-cloud/src/aliyun_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`（加 `pub mod aliyun_stream;`）

aliyun 协议最复杂（Fun-ASR/Paraformer 任务型 + Qwen-ASR Realtime 两套），但纯函数可单测面有限（主要是 `is_qwen_realtime_endpoint`）。WSS 主体靠 desktop 已验证逻辑 + `#[ignore]` 真实 key 集成测试。

- [ ] **Step 1: 复制 + 改造 aliyun_stream.rs**

Read `crates/desktop/src/aliyun_stream.rs` 全文。Create `crates/asr-cloud/src/aliyun_stream.rs`，整体粘贴，按「复刻通用规则」改造：
- `open()` 签名改为（去掉 `rt`，`rt.spawn` → `tokio::spawn`）：

```rust
/// 建连 + 初始化 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 根据 `endpoint` 路径自动选择协议：
/// - 含 `/v1/realtime` → Qwen-ASR Realtime 会话协议（OpenAI Realtime 风格）
/// - 否则 → Fun-ASR/Paraformer 任务型协议（run-task/finish-task）
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。批引擎 `CloudBatchEngine`
/// 在自有 runtime 的 `block_on` 内调用。
/// `pre_roll_samples` 是 f32[-1,1] 样本（批处理传空 Vec：整段一次推，无需前导）。
pub fn open(
    endpoint: String,
    key: String,
    model: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    let is_qwen = is_qwen_realtime_endpoint(&endpoint);
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = if is_qwen {
            run_qwen_realtime_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        } else {
            run_ws_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        };
        if let Err(e) = result {
            log::error!("aliyun stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- `run_ws_session` / `run_qwen_realtime_session` / `is_qwen_realtime_endpoint` 及所有 helper：**逐字照搬** desktop 版本。
- 模块文档头：把"tauri::async_runtime（tokio handle）"改为"tokio runtime（CloudBatchEngine 的 block_on 驱动）"。

- [ ] **Step 2: 注册模块**

`crates/asr-cloud/src/lib.rs` 末尾加：

```rust
pub mod aliyun_stream;
```

- [ ] **Step 3: 复制 desktop 已有单测（含 is_qwen_realtime_endpoint）**

desktop `aliyun_stream.rs` 已带：`is_qwen_realtime_endpoint`（L282，pub(crate)）+ L508 起的 `mod tests`（5 个测试）。复刻时把 `is_qwen_realtime_endpoint` 函数 + 整个 `#[cfg(test)] mod tests {...}` **逐字复制**到 cloud crate 版本（字节级验证已存在，无需新编）。确认 `is_qwen_realtime_endpoint` 判定逻辑含 `/v1/realtime` 子串。

- [ ] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib aliyun`
Expected: `is_qwen_realtime_endpoint_detects_realtime` PASS。

- [ ] **Step 5: 编译验证（含 native-tls / serde_json 依赖生效）**

Run: `cargo check -p octopus-asr-cloud`
Expected: 编译通过，0 error。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/aliyun_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 aliyun(DashScope) WSS 协议层（open 去 tauri + tokio::spawn）"
```

---

## Task 3: 协议层 bytedance（豆包二进制帧）

**Files:**
- Create: `crates/asr-cloud/src/bytedance_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

bytedance 是二进制帧协议（4B header + payload + gzip），帧编解码纯函数可单测，价值最高。

- [ ] **Step 1: 复制 + 改造 bytedance_stream.rs**

Read `crates/desktop/src/bytedance_stream.rs` 全文。Create `crates/asr-cloud/src/bytedance_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连 + 发初始 config + 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。
pub fn open(
    api_key: String,
    resource_id: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_session(pcm_rx, result_tx, api_key, resource_id, language, pre_roll_samples)
                .await;
        if let Err(e) = result {
            log::error!("bytedance stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

> 注意：desktop 版 `run_session` 的确切名以源文件为准（可能是 `run_bytedance_session`）；照搬源文件的函数名，`open` 内调用名与之对齐。

- 帧编解码常量（`PROTOCOL_VERSION`/`MSG_*`/`FLAG_*`/`SER_*`）、`build_*`/`parse_*` helper、`run_session`：**逐字照搬**。

- [ ] **Step 2: 注册模块**

`lib.rs` 加 `pub mod bytedance_stream;`

- [ ] **Step 3: 复制 desktop 已有帧编解码单测**

desktop `bytedance_stream.rs` L385 起的 `mod tests` 已带 5 个测试：`test_build_client_frame_audio` / `test_build_client_frame_last`（帧构造，校验 4B header：byte0=0x11、msg_type、flags、ser、comp）+ `test_gzip_roundtrip` + `test_parse_server_frame_response` / `test_parse_server_frame_error`。帧构造函数实际名 `build_client_frame(msg_type, flags, serialization, compression, payload_raw)`（5 参数）。复刻时**逐字复制**该 `mod tests`——它依赖的 `build_client_frame`/`parse_server_frame`/`gzip_compress`/`decompress_or_raw`/协议常量本就在 `run_bytedance_session` 同文件，Step 1 整体复制已含。字节级验证已存在，无需新编。

- [ ] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib bytedance`
Expected: 帧编解码单测 PASS。

- [ ] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error（flate2 Gzip 依赖生效）。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/bytedance_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 bytedance(豆包) 二进制帧 WSS 协议层 + 帧编解码单测"
```

---

## Task 4: 协议层 tencent（HMAC-SHA1 签名）+ baidu（START 帧鉴权）

**Files:**
- Create: `crates/asr-cloud/src/tencent_stream.rs`
- Create: `crates/asr-cloud/src/baidu_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

两个相对简单（tencent 签名构造 + baidu START 帧），合并到一个 task。

- [ ] **Step 1: 复刻 tencent_stream.rs**

Read `crates/desktop/src/tencent_stream.rs` 全文。Create `crates/asr-cloud/src/tencent_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连（含签名 URL）+ 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid_secretid: String,
    secret_key: String,
    engine_model_type: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = run_tencent_session(
            pcm_rx, result_tx, appid_secretid, secret_key, engine_model_type, pre_roll_samples,
        )
        .await;
        if let Err(e) = result {
            log::error!("tencent stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- 签名构造 helper（拼 `sign_str` → HMAC-SHA1 → base64 → URL-encode）+ `run_tencent_session`：**逐字照搬**。

- [ ] **Step 2: 复刻 baidu_stream.rs**

Read `crates/desktop/src/baidu_stream.rs` 全文。Create `crates/asr-cloud/src/baidu_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连 + 发 START 帧 + 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid: String,
    appkey: String,
    dev_pid: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_baidu_session(pcm_rx, result_tx, appid, appkey, dev_pid, pre_roll_samples).await;
        if let Err(e) = result {
            log::error!("baidu stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- `run_baidu_session`（含 START 帧 JSON 构造、UUID `sn`、双向循环、FINISH）：**逐字照搬**。

- [ ] **Step 3: 注册模块**

`lib.rs` 加：

```rust
pub mod tencent_stream;
pub mod baidu_stream;
```

- [ ] **Step 4: 复制 desktop 已有签名单测（tencent）**

desktop `tencent_stream.rs` L298 起的 `mod tests` 已带 7 个测试：`test_percent_encode_special_chars` / `test_percent_encode_alphanumeric`（URL 编码）+ `test_build_signed_url_structure` / `_deterministic` / `_different_keys`（签名 URL 结构/确定性/密钥敏感性）。签名函数实际名 `build_signed_url(appid, secretid, secret_key, engine_model_type, voice_id)`（5 参数，含 voice_id）。复刻时**逐字复制**该 `mod tests`——依赖的 `build_signed_url`/`percent_encode` Step 1 整体复制已含。

- [ ] **Step 5: 复制 desktop 已有单测（baidu）**

desktop `baidu_stream.rs` L230 起的 `mod tests` 已带 6 个测试。复刻时**逐字复制**该 `mod tests`。

- [ ] **Step 6: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib`
Expected: tencent 签名 + baidu endpoint 单测 PASS（连同前序 task 的测试全绿）。

- [ ] **Step 7: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error（hmac/sha1/base64 依赖生效）。

- [ ] **Step 8: Commit**

```bash
git add crates/asr-cloud/src/tencent_stream.rs crates/asr-cloud/src/baidu_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 tencent(HMAC-SHA1) + baidu(START 帧) WSS 协议层 + 签名单测"
```

---

## Task 5: config 分发（resolve_*_config + open_cloud_session）

**Files:**
- Create: `crates/asr-cloud/src/config.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

复刻 `crates/desktop/src/cloud_pipeline.rs:110-213` 的 resolve_* + open_cloud_session，去 tauri、改同步（open 同步）。

- [ ] **Step 1: 写 config.rs（迁移 resolve_* + open_cloud_session）**

Create `crates/asr-cloud/src/config.rs`：

```rust
//! 云端 ASR 配置解析 + provider 分发（复刻 desktop cloud_pipeline.rs 的 open 部分）。
//!
//! 与 desktop 差异：无 tauri runtime 依赖；`open_cloud_session` 同步返回 `CloudStreamHandle`
//!（各 provider `open()` 内部 `tokio::spawn`，须在 tokio 上下文调用）。

use crate::cloud_types::CloudStreamHandle;
use anyhow::{bail, Result};
use octopus_asr::config::{self, EngineCategory};

/// 通用云端配置解析：从 DB section 取 ModelEntry + 校验 secret_key 非空。
fn resolve_cloud_entry<'a>(
    section: Option<&'a std::collections::HashMap<String, octopus_infra::db::ModelEntry>>,
    provider: &'a str,
    model_name: &'a str,
) -> std::result::Result<&'a octopus_infra::db::ModelEntry, String> {
    let entry = section
        .and_then(|m| m.get(model_name))
        .ok_or_else(|| format!("{} ASR 模型 '{}' 未在 DB 配置", provider, model_name))?;
    if entry.secret_key.is_empty() {
        return Err(format!("{} ASR 模型 '{}' 的 secret_key 为空", provider, model_name));
    }
    Ok(entry)
}

/// 解析 Aliyun（DashScope）配置（endpoint + key + model_name）。
fn resolve_aliyun_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.aliyun.as_ref(), "aliyun", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 ByteDance（豆包）配置（resource_id + api_key + model_name）。
fn resolve_bytedance_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.bytedance.as_ref(), "bytedance", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Tencent（腾讯云）配置（appid:secretid + secret_key + engine_model_type）。
fn resolve_tencent_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.tencent.as_ref(), "tencent", &model_name)?;
    if !entry.source.contains(':') {
        return Err(format!(
            "tencent ASR 模型 '{}' 的 source 字段格式应为 appid:secretid（当前='{}'）",
            model_name, entry.source
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Baidu（百度云）配置（appid + api_key + dev_pid）。
fn resolve_baidu_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.baidu.as_ref(), "baidu", &model_name)?;
    if entry.source.is_empty() {
        return Err(format!(
            "baidu ASR 模型 '{}' 的 source 字段（AppID）为空",
            model_name
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 根据 spec 解析配置 + 打开对应云端 WS session（同步返回句柄）。
///
/// `asr_engine` 是完整 spec（如 `aliyun:qwen-asr`）。**须在 tokio runtime 上下文调用**
///（各 provider `open` 内部 `tokio::spawn`）。
pub fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle> {
    match config::resolve_engine_category(asr_engine) {
        Some(EngineCategory::Aliyun) => {
            let (endpoint, key, model) = resolve_aliyun_config(asr_engine)?;
            crate::aliyun_stream::open(endpoint, key, model, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::ByteDance) => {
            let (resource_id, api_key, _) = resolve_bytedance_config(asr_engine)?;
            crate::bytedance_stream::open(api_key, resource_id, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::Tencent) => {
            let (appid_secretid, secret_key, engine_model_type) =
                resolve_tencent_config(asr_engine)?;
            crate::tencent_stream::open(
                appid_secretid,
                secret_key,
                engine_model_type,
                language.to_string(),
                pre_roll,
            )
            .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::Baidu) => {
            let (appid, appkey, dev_pid) = resolve_baidu_config(asr_engine)?;
            crate::baidu_stream::open(appid, appkey, dev_pid, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        _ => bail!("当前引擎非云端（spec='{}'），无法开启 WSS", asr_engine),
    }
}
```

> **核对**：`octopus_asr::config::load_config()` / `resolve_engine_category()` / `EngineCategory` 均 pub（desktop `cloud_pipeline.rs:129/186/188` 跨 crate 已用）；`octopus_infra::db::{ModelEntry, parse_model_spec}` pub（desktop `cloud_pipeline.rs:114/131` 已用）。`AppConfig.asr.{aliyun,bytedance,tencent,baidu}` 字段类型 = `Option<HashMap<String, ModelEntry>>`（见 desktop resolve_* 用法）。若字段名/类型有出入，以 desktop `cloud_pipeline.rs:127-177` 为准对齐。

- [ ] **Step 2: 注册模块 + re-export**

`lib.rs` 加：

```rust
pub mod config;
pub use config::open_cloud_session;
```

- [ ] **Step 3: 写 open_cloud_session 错误路径单测（先写测试）**

在 `config.rs` 末尾加（非法 spec 在 resolve 前就 bail，不需真实 key）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_cloud_session_rejects_local_spec() {
        // 本地引擎 spec（如 whisper）→ resolve_engine_category 返回非云端 → bail。
        // 无需 tokio runtime（在 spawn 前就返回 Err）。
        let res = open_cloud_session("whisper", "zh", Vec::new());
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("非云端") || msg.contains("无法开启 WSS"));
    }

    #[test]
    fn open_cloud_session_rejects_unresolvable_spec() {
        // 不存在的 spec → resolve_engine_category 返回 None → bail。
        let res = open_cloud_session("nonexistent:foo:bar", "zh", Vec::new());
        assert!(res.is_err());
    }
}
```

- [ ] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib config`
Expected: 2 passed。

- [ ] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/config.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): config 分发（resolve_*_config + open_cloud_session，去 tauri）"
```

---

## Task 6: CloudBatchEngine impl OfflineAsrEngine

**Files:**
- Create: `crates/asr-cloud/src/batch.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

批引擎核心：`from_spec` 解析 + 建 runtime；`transcribe` 单段单 session（block_on open + 分块 push + close_async）；`skip_corrector=true`。

- [ ] **Step 1: 写 from_spec 错误路径测试（先写测试）**

Create `crates/asr-cloud/src/batch.rs`，先写测试模块：

```rust
//! 云端 ASR 批引擎（impl `octopus_asr::engine::OfflineAsrEngine`）。
//!
//! 语义：`transcribe(samples, language)` = 单段音频（≤30s，由上层 `transcribe_segments`
//! 保证）→ 单个 WSS session → 完整文本。VAD 分段 + CJK 连接由
//! `asr::pipeline::transcribe_segments` 自动完成，本引擎不分段、不拼接。
//!
//! `skip_corrector() = true`：云端结果质量高，跳过本地拼音纠错（对齐桌面端云端行为）；
//! 简繁转换仍由 `transcribe_batch` 处理。

use crate::open_cloud_session;
use anyhow::{bail, Result};
use octopus_asr::engine::OfflineAsrEngine;
use octopus_infra::db::{parse_model_spec, ModelSpec};

/// 分块推送粒度（采样点）：200ms @ 16kHz = 3200。平滑灌入避免单帧过大。
const CLOUD_PUSH_CHUNK_SAMPLES: usize = 3200;

/// 判断 spec 是否云端 ASR（3-part provider 前缀为 aliyun/bytedance/tencent/baidu）。
///
/// 用 `parse_model_spec` 取 provider 字段，**不查 DB**（纯字符串解析，可单测）。
/// 2-part/裸名 → `NameOnly` → false（走本地分支）。3-part 是标准 spec 格式
///（如 `aliyun:Fun-ASR:fun-asr-realtime`）。cli 分流与本 crate 的 `from_spec` 共用此判定。
pub fn is_cloud_spec(spec: &str) -> bool {
    matches!(
        parse_model_spec(spec),
        ModelSpec::Full { provider, .. } if is_cloud_provider(provider)
    )
}

/// provider 字符串是否云端（大小写不敏感）。
fn is_cloud_provider(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("aliyun")
        || provider.eq_ignore_ascii_case("bytedance")
        || provider.eq_ignore_ascii_case("tencent")
        || provider.eq_ignore_ascii_case("baidu")
}

/// 云端 ASR 批引擎。
pub struct CloudBatchEngine {
    /// 完整 3-part spec（如 `aliyun:Fun-ASR:fun-asr-realtime`），`open_cloud_session` 据此解析配置。
    spec: String,
    /// 自有 tokio runtime（驱动各 provider `open` 的 `tokio::spawn` + `close_async`）。
    rt: tokio::runtime::Runtime,
}

impl CloudBatchEngine {
    /// 从 spec 构造。校验 provider 前缀为云端（不查 DB）+ 建 runtime。
    /// DB 查找（resolve_*_config）推迟到 `transcribe` 内的 `open_cloud_session`。
    pub fn from_spec(spec: &str) -> Result<Self> {
        if !is_cloud_spec(spec) {
            bail!(
                "非云端 ASR spec（'{}'）；CloudBatchEngine 仅支持 3-part 云端 spec \
                 （aliyun/bytedance/tencent/baidu:category:model_name）",
                spec
            );
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { spec: spec.to_string(), rt })
    }
}

impl OfflineAsrEngine for CloudBatchEngine {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        let spec = self.spec.clone();
        let lang = language.to_string();
        self.rt.block_on(async move {
            let mut handle = open_cloud_session(&spec, &lang, Vec::new())?;
            // 分块推 PCM（批处理一次推完；空 samples 也安全：不进循环，直接 finish）。
            for chunk in samples.chunks(CLOUD_PUSH_CHUNK_SAMPLES) {
                handle.push_pcm(chunk)?;
            }
            // close_async：发 Finish + 收最终结果（超时上限 CLOUD_CLOSE_TIMEOUT_SECS=8s）。
            handle.close_async().await
        })
    }

    fn skip_corrector(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cloud_spec_recognizes_3part_cloud() {
        // 3-part 云端 spec（provider 前缀为云端）→ true（不查 DB）。
        // category 段取 asr/config.rs category_label 的实际值。
        assert!(is_cloud_spec("aliyun:Fun-ASR:fun-asr-realtime"));
        assert!(is_cloud_spec("bytedance:Doubao-ASR:doubao-asr-1.0-streaming"));
        assert!(is_cloud_spec("tencent:Tencent-ASR:16k_zh"));
        assert!(is_cloud_spec("baidu:Baidu-ASR:15372"));
    }

    #[test]
    fn is_cloud_spec_rejects_local_3part_bare_and_2part() {
        // 本地 3-part（provider=local）→ false。
        assert!(!is_cloud_spec("local:zipformer:zipformer-small-ctc"));
        // 裸名 → NameOnly → false。
        assert!(!is_cloud_spec("zipformer-small-ctc"));
        // 2-part → NameOnly 兜底 → false（须 3-part 才判云端）。
        assert!(!is_cloud_spec("aliyun:fun-asr-realtime"));
    }

    #[test]
    fn from_spec_rejects_non_cloud() {
        assert!(CloudBatchEngine::from_spec("local:zipformer:zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("aliyun:fun-asr-realtime").is_err()); // 2-part
    }

    #[test]
    fn from_spec_accepts_cloud_3part() {
        // 云端 3-part → 构造成功（不查 DB、不连网；仅建 runtime）。
        assert!(CloudBatchEngine::from_spec("aliyun:Fun-ASR:fun-asr-realtime").is_ok());
    }
}
```

- [ ] **Step 2: 注册模块 + re-export**

`lib.rs` 加：

```rust
pub mod batch;
pub use batch::{CloudBatchEngine, is_cloud_spec};
```

- [ ] **Step 3: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib batch`
Expected: `from_spec_rejects_local_engine` + `from_spec_rejects_garbage` PASS。

- [ ] **Step 4: 加真实 key 集成测试（#[ignore]）**

在 `batch.rs` 测试模块追加（用户提供本地 DashScope key 时手动跑）：

```rust
    /// 真实 DashScope 集成测试：`cargo test -p octopus-asr-cloud --lib -- --ignored batch::real_aliyun`。
    /// 需 ~/.octopus/config.yaml 的 asr.aliyun.<model> 配好 secret_key。
    /// 用 `cargo run` 录一段样本或用现成 wav → f32 样本后断言非空文本。
    #[ignore]
    #[test]
    fn real_aliyun_transcribe_nonempty() {
        // 占位：实际验证靠 cli 端到端（Task 8 e2e 清单）。
        // 此测试保留为「有本地 key 时的最小集成入口」，样本来源由用户准备。
        // 无样本时直接返回，避免误失败。
        eprintln!("[ignore] 跳过：需本地 DashScope key + 音频样本，见 Task 8 e2e 清单");
    }
```

- [ ] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud --all-targets`
Expected: 0 error（含 test target）。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/batch.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): CloudBatchEngine impl OfflineAsrEngine（单段单 session，skip_corrector=true）"
```

---

## Task 7: AsrEngineManager getter + cli 本地/云端分流

**Files:**
- Modify: `crates/asr/src/engine.rs`（加 `active_engine` getter）
- Modify: `crates/cli/Cargo.toml`（加 octopus-asr-cloud 依赖）
- Modify: `crates/cli/src/pipeline.rs`（分流）

- [ ] **Step 1: 给 AsrEngineManager 加 active_engine getter**

编辑 `crates/asr/src/engine.rs`，在 `transcribe_batch` 方法后（`impl AsrEngineManager` 块内，约 L163 后）加：

```rust
    /// 取出当前 active engine（供 cli 分流后统一调 `pipeline::transcribe_batch`）。
    ///
    /// 与本地/云端分流配合：cli 本地分支构造 `AsrEngineManager` + `switch_model` 后取
    /// `Arc<dyn OfflineAsrEngine>`，与云端分支的 `CloudBatchEngine` 同为 `dyn OfflineAsrEngine`，
    /// 喂同一 `transcribe_batch`。
    pub fn active_engine(&self) -> Result<Arc<dyn OfflineAsrEngine>> {
        self.active_engine
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No active ASR engine loaded in AsrEngineManager"))
    }
```

- [ ] **Step 2: cli 加 octopus-asr-cloud 依赖**

编辑 `crates/cli/Cargo.toml`，在 `[dependencies]` 加（位置参考既有 octopus-asr 行）：

```toml
octopus-asr-cloud = { path = "../asr-cloud" }
```

> 先 Read `crates/cli/Cargo.toml` 确认既有 `octopus-asr` 行的写法，紧随其后加。

- [ ] **Step 3: 写 is_cloud_spec 测试（先写测试）**

编辑 `crates/cli/src/pipeline.rs`，整体替换为（先写测试 + is_cloud_spec）：

```rust
//! CLI 批处理转写 pipeline：本地 / 云端分流 → `transcribe_batch`（VAD + 纠错 + 简繁）。
//!
//! 分流在 cli 层（`asr` crate 不依赖 `asr-cloud`，避免循环）：
//! - 云端 spec（aliyun/bytedance/tencent/baidu）→ `CloudBatchEngine::from_spec`。
//! - 本地 onnx → `AsrEngineManager` + `active_engine`。
//! 两端都经 `asr::pipeline::transcribe_batch` 编排（VAD 分段 + 纠错 + 简繁）。

use anyhow::Result;
use octopus_asr::engine::{AsrEngineManager, OfflineAsrEngine};
use octopus_asr::pipeline::{transcribe_batch, PipelineConfig};
use octopus_asr_cloud::{is_cloud_spec, CloudBatchEngine};

/// 批处理转写：分流 → transcribe_batch（VAD 分段 + 纠错 + 简繁）。
///
/// `model` 为 DB models 表的 model_name（支持 `provider:category:model` spec）。
/// 云端 spec → `CloudBatchEngine`（内部 WSS，`skip_corrector=true`）；本地 → onnx 引擎。
pub fn run(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let cfg = PipelineConfig::from_app_config(language);
    if is_cloud_spec(model) {
        let engine = CloudBatchEngine::from_spec(model)?;
        transcribe_batch(&engine, samples, &cfg)
    } else {
        let mgr = AsrEngineManager::new();
        mgr.switch_model(model)?;
        let engine = mgr.active_engine()?;
        transcribe_batch(&engine, samples, &cfg)
    }
}

// cli 层无可单测的纯函数：is_cloud_spec 在 octopus-asr-cloud crate（Task 6）已测；
// run 需真实引擎 / WSS，验证靠 cargo check + clippy + Task 8 e2e 清单。
```

> **核对**：`octopus_asr::pipeline::transcribe_batch` 是 pub（`asr/pipeline.rs:46`）。`resolve_engine_category` / `EngineCategory` pub（见 Task 5 核对）。若 `is_cloud_spec_recognizes_cloud_prefixes` 中某前缀解析不出云端（取决于 `resolve_engine_category` 实现），Read `crates/asr/src/config.rs` 的 `resolve_engine_category` + `EngineCategory` 前缀表，用**实际能解析为云端**的 spec 形态替换测试用例。

- [ ] **Step 4: 确认 is_cloud_spec 测试在 cloud crate 通过**

`is_cloud_spec` 单测在 `octopus-asr-cloud`（Task 6 Step 1），cli 层无单测（run 需真实引擎/WSS）。

Run: `cargo test -p octopus-asr-cloud --lib batch`
Expected: `is_cloud_spec_*` + `from_spec_*` 全 PASS（Task 6 已验证）。

- [ ] **Step 5: workspace 编译验证（关键里程碑：cli 拉通 cloud crate）**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。asr-cloud 全链路（cli → asr-cloud → asr/infra）编译通过。

- [ ] **Step 6: clippy 零新 warning**

Run: `cargo clippy -p octopus-asr-cloud -p octopus-cli --all-targets -- -D warnings`
Expected: 0 warning（新代码）。若 asr-cloud/cli 既有 warning 与本次无关，用 `-W` 而非 `-D` 区分；目标只看新代码无 warning。

- [ ] **Step 7: Commit**

```bash
git add crates/asr/src/engine.rs crates/cli/Cargo.toml crates/cli/src/pipeline.rs
git commit -m "feat(cli): 本地/云端 ASR 分流（AsrEngineManager::active_engine + CloudBatchEngine）"
```

---

## Task 8: workspace 测试 + 文档同步 + e2e 清单

**Files:**
- Modify: `docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md`（横幅状态）
- Modify: `docs/architecture.md`（加 octopus-asr-cloud）
- Modify: 记忆 `parallel-workstreams.md` + `MEMORY.md`
- Create: 本 plan 同目录无需新建（e2e 清单写在本 task）

- [ ] **Step 1: workspace 全量测试**

Run: `cargo test --workspace`
Expected: 全绿（含 asr-cloud 全部单测；`#[ignore]` 的真实 key 测试跳过）。

- [ ] **Step 2: workspace check + clippy 兜底**

Run: `cargo check --workspace --all-targets`
Run: `cargo clippy --workspace --all-targets`
Expected: check 0 error；clippy 无本次引入的新 warning。

- [ ] **Step 3: 更新 spec 横幅状态**

编辑 `docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md`，把顶部状态行：

```
> **状态**：设计中（待用户审 → writing-plans）。
```
改为：
```
> **状态**：已实现且 e2e 通过（plan `docs/superpowers/plans/2026-06-25-cloud-asr-cli.md`，8 task 全完成；workspace 测试绿；e2e 用户本地云端 key 验通过 2026-06-25）。
```

并在 §4.1「协议层」开头加一句实施修正注记：

```
> **实施修正**：`open()` 保持同步签名（仅 `tokio::spawn`），`close_async` 才是 async 收尾；
> CloudBatchEngine 不自己分段（`transcribe_segments` 自动分段）。详见 plan 顶部「两点据实修正」。
```

- [ ] **Step 4: 更新 architecture.md**

Read `docs/architecture.md`，在 crate 列表/workspace 结构处加 `octopus-asr-cloud`（云端 ASR WSS 协议层 + 批引擎，cli 批处理用；desktop 第二步复用）。若无明确 crate 清单段，在最接近的「模块/crate 说明」处补一段：

```markdown
- `crates/asr-cloud`（`octopus-asr-cloud`）：云端 ASR（Aliyun/ByteDance/Tencent/Baidu）WSS
  协议层 + 批引擎 `CloudBatchEngine`（impl `OfflineAsrEngine`）。cli 批处理转译音频文件可选云端
  API；desktop 流式适配暂留 desktop（第二步合并）。依赖 `octopus-asr`（单向）。
```

- [ ] **Step 5: 更新记忆 parallel-workstreams.md**

在 `parallel-workstreams.md` 的 ASR pipeline 阶段2 条目（item 7）末尾，或作为新进展，补一行：

```
**云端 ASR 下沉 cli（已实施，worktree-model-mgmt-ui）**：新建 octopus-asr-cloud crate
（4 provider WSS 协议层 1:1 复刻 desktop + CloudBatchEngine impl OfflineAsrEngine，
skip_corrector=true）+ cli 本地/云端分流（is_cloud_spec + AsrEngineManager::active_engine）。
desktop 本次零改动（*_stream.rs 副本暂留，第二步合并）。e2e 通过（用户本地云端 key 验，2026-06-25）。
```

同步更新 `MEMORY.md` 索引行的尾部「2c-3/2d 待」前后，提及 cloud-asr-cli 已实施。

- [ ] **Step 6: 文档提交**

```bash
git add docs/superpowers/specs/2026-06-25-cloud-asr-cli-design.md docs/architecture.md
git commit -m "docs: 云端 ASR 下沉 cli 实施完成同步（spec 横幅 + architecture）"
```

（记忆文件在仓库外，不进 git，Step 5 用 Write 工具直接写。）

- [x] **Step 7: e2e 手动验证清单（用户本地云端 key 验通过，2026-06-25）**

实现完成后，向用户给出以下 e2e 清单（用户本地有云端 key 时执行）：

```
# 前置：~/.octopus/config.yaml 的 asr.<provider>.<model> 配好 secret_key（与 desktop 同源）。

# 1. 云端转译（aliyun 示例，替换为实际配置的 model spec）
octopus-cli transcribe --model "aliyun:qwen-asr" --language zh path/to/test.wav
# 预期：输出识别文本（非空、内容正确）；云端结果不再走本地拼音纠错。

# 2. 云端转译长音频（>30s，触发 VAD 分段 → 多 session）
octopus-cli transcribe --model "aliyun:qwen-asr" --language zh path/to/long.wav
# 预期：分段识别 + CJK 连接，输出连贯文本。

# 3. 其他 provider（按已配置的 key 轮测）
octopus-cli transcribe --model "bytedance:<model>" --language zh test.wav
octopus-cli transcribe --model "tencent:<model>" --language zh test.wav
octopus-cli transcribe --model "baidu:<model>" --language zh test.wav

# 4. 回归：本地 onnx 仍正常（分流本地分支未受影响）
octopus-cli transcribe --model "zipformer-small-ctc" --language zh test.wav

# 5. 错误路径：未配置 key 的云端 spec → 友好报错（非 panic）
octopus-cli transcribe --model "aliyun:not-configured" --language zh test.wav
# 预期：报 "aliyun ASR 模型 'not-configured' 未在 DB 配置" 或 secret_key 为空。
```

- [ ] **Step 8: 标记 plan 全部完成**

本 plan 所有 task checkbox 勾选；向用户报告实施完成 + e2e 清单，进入 `finishing-a-development-branch`（保留 worktree / ff-merge 由用户定）。

---

## Spec Coverage 自检

| spec 章节 | 覆盖 task |
|---|---|
| §3.1 crate 依赖图（asr←cloud，cli 依赖两者） | Task 1（Cargo.toml）+ Task 7（cli 依赖） |
| §3.2 三层分工（协议层/批引擎/流式留 desktop） | Task 2-4（协议层）+ Task 6（批引擎）+ desktop 不动 |
| §3.3 runtime（block_on + tokio::spawn） | Task 6（CloudBatchEngine.rt）+ Task 2-4（open tokio::spawn） |
| §4.1 协议层（4 provider WSS，去 tauri） | Task 2/3/4 |
| §4.2 CloudBatchEngine（transcribe + skip_corrector） | Task 6 |
| §4.3 provider 分发（EngineCategory + resolve_*） | Task 5 |
| §5 cli 接入（is_cloud_spec + active_engine getter） | Task 7 |
| §6 config 复用（AppConfig.asr.{provider}） | Task 5（resolve_*） |
| §7 测试策略（协议纯函数单测 + #[ignore] 真实 key） | Task 1-6 各单测 + Task 6 #[ignore] + Task 8 e2e |
| §9 风险（临时两份/超时/循环约束） | 两份=desktop 不动（Task 范围）；超时=close_async 8s（Task 1 迁移）；循环=cli 分流（Task 7） |

---

## 备注：desktop 第二步（非本次，spec §8/§10）

本次完成后，desktop 仍用自己的 `*_stream.rs` 副本。第二步（独立后续）：
- 删 desktop `{aliyun,bytedance,tencent,baidu}_stream.rs` + `cloud_types.rs` 协议副本；
- `cloud_pipeline.rs` 的 `open_cloud_session` / `resolve_*` 改调 `octopus_asr_cloud`；
- `CloudPipelineEngine` 持 `CloudStreamHandle` 改用 cloud crate 类型；
- 云端流式 e2e 回归（本地 + 云端）。
本次不触碰，留 spec §8 记录。
