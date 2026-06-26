# desktop 复用 cloud 协议层（消除协议层两份副本）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：✅ 已合并 main（`6a4593e`，ff-merge）。Task 1-6 编译/测试/云端流式 e2e 全通过（2026-06-25 本地云端 key 验证）。
> **动机**：cloud-asr-cli（`octopus-asr-cloud` crate）落地后，4 provider WSS 协议层临时存在两份副本——`octopus-asr-cloud`（cli/server 用，去 tauri）与 `octopus-desktop`（流式适配用，依赖 tauri runtime）。本 spec 收口这份技术债：删 desktop 协议副本，desktop 改指 cloud crate，协议层单源。
> **关联**：`2026-06-25-cloud-asr-cli-design.md` §8/§10（明确"第二步"范围）；ASR pipeline 总 spec `2026-06-23-asr-pipeline-design.md`。
> **范围**：desktop 删 5 个协议副本 + 改造 `cloud_pipeline.rs`/`coordinator.rs` 改指 cloud crate + cloud crate 加测试构造器 + 云端流式 e2e 回归。**不含**：`engine_aliyun.rs`、VadSegmented 归位（2c-3）、coordinator 清理（2d）。

---

## 1. 背景

cloud-asr-cli 第一步（已合并 main `bb967be`）为 cli/server 批处理新建了 `octopus-asr-cloud` crate：4 provider（Aliyun/ByteDance/Tencent/Baidu）WSS 协议层 + `CloudBatchEngine`，从 desktop `*_stream.rs` 1:1 复刻，**唯一改造是把 `open()` 内部从 `tauri::async_runtime::RuntimeHandle` + `rt.spawn` 改成 `tokio::spawn`**（去 tauri）。

结果：协议层字节级一致的两份副本同时存在——

| | desktop 副本（tauri 版） | cloud crate（tokio 版） |
|---|---|---|
| `*_stream.rs::open()` 第一参 | `&tauri::async_runtime::RuntimeHandle` | 无 |
| spawn 方式 | `rt.spawn(...)` | `tokio::spawn(...)` |
| 调用上下文要求 | 任意线程（tauri runtime 全局） | **须在 tokio runtime context** |
| 行数 | `*_stream.rs` 569/470/380/280 + `cloud_types.rs` 146 | 同名文件行数几乎一致（1:1 复刻） |

任何协议层改动（鉴权串、帧格式）此刻都要改两处，是必须尽快收口的技术债。cloud crate 已稳定（30 单测绿 + cli/server/desktop 批处理 e2e 通过），是收口时机。

## 2. 用户决策（brainstorming 2026-06-25）

1. **范围**：删 desktop 协议副本（`*_stream.rs` × 4 + `cloud_types.rs`），desktop `CloudPipelineEngine` 改指 cloud crate 协议层。`CloudPipelineEngine` 本身（流式适配）留 desktop。
2. **D1 runtime 兼容（核心）**：方案 **B**——cloud crate 零改动，desktop 用 `tauri::async_runtime::block_on` 进入 tokio context 后调 cloud crate 的 `open_cloud_session`。
3. **D2 类型归属**：desktop 全栈改用 `octopus_asr_cloud::{CloudStreamHandle, StreamEvent}`；cloud crate 加 `#[doc(hidden)] pub fn new_for_test()` 供 desktop 测试构造预载 handle。
4. **`engine_aliyun.rs` 不动**：它是 `AliyunEngine`（chunk 模式离线引擎，经 `engine_dispatch.rs` 用），与 `*_stream.rs` 长连接协议是两套（`aliyun_stream.rs:9` 文档自述）。

## 3. D1：runtime 兼容（方案 B）

### 3.1 问题

cloud crate 的 `open_cloud_session`（`config.rs:81`）同步返回 `CloudStreamHandle`，但各 provider `open()` 内部 `tokio::spawn`（如 `aliyun_stream.rs:53`）——**须在 tokio runtime 上下文调用**。

desktop `CloudPipelineEngine::tick` 在 coordinator 主线程（`std::thread`，**非 tokio context**）同步调 `open_cloud_session`（`cloud_pipeline.rs:307`）。desktop 现行副本靠 `tauri::async_runtime::handle()`（tauri runtime 全局、任意线程可 spawn）绕过；cloud crate 去 tauri 后该能力消失，直接调 `tokio::spawn` 会 panic（no reactor running）。

### 3.2 方案 B：cloud crate 零改动，desktop block_on 进 context

```rust
// desktop cloud_pipeline.rs：open_cloud_session 改为瘦 wrapper（保留，tick 调用点零改动）
pub(super) fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle, String> {
    // cloud crate 的 open_cloud_session 内部 tokio::spawn，须在 tokio context。
    // coordinator 主线程非 tokio，用 tauri runtime 的 block_on 进入（tauri runtime 即 tokio）。
    tauri::async_runtime::block_on(async {
        octopus_asr_cloud::open_cloud_session(asr_engine, language, pre_roll)
    })
    .map_err(|e| e.to_string())
}
```

### 3.3 为什么安全

- `tauri::async_runtime` 底层即 tokio multi_thread runtime；`block_on` 进入时设置 current context，使同步 `open_cloud_session` 内部的 `tokio::spawn` 可用。
- `open_cloud_session` 内部只 `spawn` 一条 reader task + 返回 mpsc channel 构造的 `CloudStreamHandle`（不 `await` 建连），future 立即 ready，`block_on` 立即返回、不阻塞 coordinator。
- tokio `Runtime::block_on` 在非 runtime 线程调用安全（coordinator 线程非 worker、未嵌套在 runtime 内，无 "can't call block_on from within async context" panic 风险）。

### 3.4 不选方案 A 的理由

方案 A（cloud crate `open()` 加 `tokio::runtime::Handle` 参数）虽然把 context 约束显式化，但：要改刚稳定的 cloud crate 4 个 stream + config + batch 签名（牵连 cli/server）；desktop 端仍要 `block_on` 拿 handle（tauri 不直接暴露底层 tokio Handle），没省掉 block_on。净亏。

## 4. D2：类型归属 + 测试构造器

### 4.1 全栈改用 cloud crate 类型

desktop 删 `cloud_types.rs` 后，所有 `CloudStreamHandle`/`StreamEvent` 引用改自 `octopus_asr_cloud`：
- `cloud_pipeline.rs`：`use crate::cloud_types::{CloudStreamHandle, StreamEvent};` → `use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};`
- `coordinator.rs`：close 路径用到的 `CloudStreamHandle`/`StreamEvent` 的 `use` 源改 `octopus_asr_cloud`（`take_close_handle` 返回类型、`Stage::CloudClosing` 的 `close_async` 调用随之）。

`PcmFrame` 是 `pub(crate)`，desktop 经 `push_pcm`/`finish` 间接用，不直接引用，无需改。

### 4.2 测试构造器（cloud crate 增量）

desktop `cloud_pipeline.rs` 有 5 个 drain 测试调 `CloudStreamHandle::new()`（其中 2 个经 `handle_with_events` helper、3 个直接调）构造预载 `StreamEvent` 的 handle 来测 `drain_cloud_session`（另有 3 个纯函数测试 `onset_confirmed`/`should_send_finish`/`take_preroll` 不用 handle）。drain 逻辑留 desktop，这些测试必须能在 desktop 构造预载 handle。

约束：cloud crate 的 `CloudStreamHandle::new()` 是 `pub(crate)`，且返回类型含 `mpsc::UnboundedReceiver<PcmFrame>`，而 `PcmFrame` 是 `pub(crate)`——直接把 `new()` 改 `pub` 会编译失败（pub fn 不能返回含私有类型的签名）。

**解决**：cloud crate 加一个只暴露 `pub` 类型的测试构造器（不泄露 `pub(crate) PcmFrame`）：

```rust
impl CloudStreamHandle {
    /// 仅供测试：构造 handle + result 发送端（预载事件用）。不暴露 pcm_rx / PcmFrame。
    #[doc(hidden)]
    pub fn new_for_test() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (handle, _pcm_rx, result_tx) = Self::new();
        (handle, result_tx)
    }
}
```

cloud crate 自身测试仍用 `pub(crate) new()`；desktop 测试改用 `new_for_test()`。`PcmFrame` 封装不动。这是 cloud crate 纯测试支持增量，不动协议逻辑。

## 5. D3：删除 / 改造清单

### 5.1 desktop 侧

| 动作 | 对象 | 备注 |
|---|---|---|
| 删文件 | `aliyun_stream.rs`、`bytedance_stream.rs`、`tencent_stream.rs`、`baidu_stream.rs`、`cloud_types.rs` | 5 个，cloud crate 1:1 |
| 删 `mod` | `main.rs` 的 `mod aliyun_stream;`/`mod bytedance_stream;`/`mod tencent_stream;`/`mod baidu_stream;`/`mod cloud_types;` | 5 行；`mod cloud_pipeline;`/`mod engine_aliyun;` 保留 |
| 改造 `cloud_pipeline.rs` | 见 5.2 | |
| `coordinator.rs` | **零改动**（靠类型推断：`take_close_handle`→`close_async`，`CloudStreamHandle` 类型由 cloud_pipeline 返回类型推断，无需 `use`） | close 路径 |
| 改造 `pipeline.rs` | `StreamingPipelineEngine::take_close_handle` trait 默认 + `StreamingPipeline` 包装方法签名 `crate::cloud_types::CloudStreamHandle` → `octopus_asr_cloud::CloudStreamHandle`（trait 与 impl 类型须一致，否则 E0053） | **实施盲点修正** |
| 改造 `engine_aliyun.rs` | `is_qwen_realtime_endpoint` + `samples_to_pcm_s16le` re-export 改指 `octopus_asr_cloud`（chunk 模式复用 cloud 协议层工具） | **实施盲点修正** |
| `Cargo.toml` | `cloud` feature 加 `octopus-asr-cloud`；可能瘦身 `tokio-tungstenite`/`uuid`/`base64`/`flate2`/`hmac`/`sha1` | plan 阶段 grep `use` 核实，仅当 desktop 删副本后不再直接用才删 |

### 5.2 `cloud_pipeline.rs` 改造明细

| 区块 | 动作 |
|---|---|
| `use` | `crate::cloud_types::{CloudStreamHandle, StreamEvent}` → `octopus_asr_cloud::{CloudStreamHandle, StreamEvent}`；删 `use tauri::async_runtime::RuntimeHandle;` |
| `resolve_cloud_entry` + `resolve_aliyun_config` + `resolve_bytedance_config` + `resolve_tencent_config` + `resolve_baidu_config`（5 个 fn，113-177） | **删**（cloud crate `config.rs` 有等价物） |
| `open_cloud_session`（181-213） | 改 3.2 的 block_on 瘦 wrapper（删 `RuntimeHandle` + `crate::*_stream::open` 分发，改为单行调 `octopus_asr_cloud::open_cloud_session`） |
| `CloudPipelineEngine`（216-399） | **逻辑零改动**（`session: Option<CloudStreamHandle>` 字段类型随 use 源变；`tick`/`finish_with_tail`/`reset`/`take_close_handle`/`current_partial`/`silence_duration`/`is_cloud` 不变） |
| `drain_cloud_session`/`onset_confirmed`/`should_send_finish`/`take_preroll`（38-108） | **逻辑零改动**（match `StreamEvent` 随 use 源变） |
| tests（401-569，共 8 个：5 drain + 3 纯函数） | 所有调 `CloudStreamHandle::new()` 处（`handle_with_events` helper + 3 个直接调）→ `CloudStreamHandle::new_for_test()`；3 个纯函数测试零改动；其余断言不变 |

### 5.3 cloud crate 侧

加 `CloudStreamHandle::new_for_test()`（4.2）+ 暴露两个 engine_aliyun 复用的 helper：`is_qwen_realtime_endpoint`（`aliyun_stream.rs` `pub(crate)`→`pub`）、`samples_to_pcm_s16le`（`cloud_types.rs` `pub(crate)`→`pub`）+ `lib.rs` 顶层 re-export `CloudStreamHandle`/`StreamEvent`/`samples_to_pcm_s16le`。协议逻辑（鉴权/帧/会话状态机）**零改动**。

### 5.4 依赖边界

```
octopus-desktop ──(cloud feature)──→ octopus-asr-cloud ──→ octopus-asr-local + octopus-infra
```

单向，无循环。`asr` 不依赖 `cloud`（cloud-asr-cli 第一步已确立）。desktop 仅在 `cloud` feature 开启时依赖 cloud crate。

## 6. 不在范围

- **`engine_aliyun.rs`**：`AliyunEngine`（chunk 模式离线引擎，`engine_dispatch.rs` 用）——实施时发现它**复用了** `aliyun_stream::is_qwen_realtime_endpoint` + `cloud_types::samples_to_pcm_s16le`（brainstorming 盲点，原以为零改动）。实际改指 `octopus_asr_cloud`（chunk 模式也复用 cloud 协议层工具，进一步消除重复）。
- **VadSegmented 归位（2c-3）**：独立设计。
- **coordinator 清理（2d）**：emit/DB/polish + transcript 全收敛进 pipeline。
- **cli/server 接入**：cloud-asr-cli 第一步已完成（批处理用 cloud crate），本次不改 cli/server。
- **cloud 协议层逻辑改动**：零行为差异搬迁，不动鉴权/帧格式/会话状态机。

## 7. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 方案 B 的 `block_on` 在 coordinator 主线程有"隐式须 context"约束 | 注释标明；`block_on` 包同步 fn 是进入 context 的标准手段，open 只 spawn+返回 channel handle 不阻塞；plan 含 e2e 回归验证 |
| cloud crate 加 `new_for_test()` 是封装小让步 | `#[doc(hidden)]` + 不暴露 `PcmFrame`（返回类型只含 `pub StreamEvent`）；仅测试用，非生产路径 |
| `Cargo.toml` cloud feature 瘦身可能误删仍被直接使用的 dep | plan 阶段 grep desktop src 确认每个 dep 的直接 `use`，仅删确认无直接引用的 |
| 删副本后 desktop cloud 编译/行为回归 | 双 feature 编译（cloud on/off）+ cloud_pipeline 8 测试 + cloud crate 31 测试 + 云端流式 e2e 回归（用户本地 key） |
| cloud crate 协议层与 desktop 副本字节级一致的前提 | cloud-asr-cli 第一步已验证 1:1 复刻（30 单测 + cli/server/desktop 批处理 e2e 通过）；改指后 e2e 回归再次确认 |

## 8. 验证清单

- [x] desktop `cargo build --features cloud` + `cargo build`（cloud off）双 feature 编译 0 error
- [x] desktop `cargo clippy --features cloud --all-targets` 本次新代码（cloud_pipeline/pipeline/engine_aliyun 改造 + cloud crate 可见性/re-export）0 新 warning。注：cloud crate 协议层（`*_stream.rs`）与 desktop（coordinator/transcript 等）有第一步遗留的预存 warning，非本次引入，不在范围
- [x] `cloud_pipeline.rs` 全部测试绿（8 个：5 个 drain 测试改用 `new_for_test` + 3 个纯函数测试零改动）
- [x] cloud crate 31 测试不变（加 `new_for_test` 不破坏）
- [x] `cargo check --workspace --all-targets` 0 error
- [x] **云端流式 e2e 回归**：desktop `--features cloud`，用户本地云端 key，本地流式 + 云端流式识别均正常（onset/partial/finish/close 全路径，2026-06-25 验证通过）
