# 云端 ASR 下沉：cli 批处理接入（`octopus-asr-cloud` crate）

> 2026-06-25 初版（brainstorming 产出）。
> **状态**：已实现（plan `docs/superpowers/plans/2026-06-25-cloud-asr-cli.md`，8 task 全完成；asr-cloud 30 单测绿、workspace check 0 error、新代码 clippy 0 warning；e2e 待用户本地云端 key 手动验）。
> **动机**：cli/server 转译音频文件应能选云端 ASR 引擎（DashScope/ByteDance/Tencent/Baidu），不必只靠本地 onnx。当前云端 ASR 全锁在 desktop crate（依赖 `tauri::async_runtime`），cli 够不到。
> **关联**：ASR pipeline 总 spec `2026-06-23-asr-pipeline-design.md`；2c-2 cloud 流式已合并 main（`fa2becc`）。
> **范围（本次）**：建 `octopus-asr-cloud` crate（WSS 协议层 + 批引擎）+ cli 接入。**不含**：desktop 复用（第二步，后续）、流式适配（留 desktop）、VadSegmented（2c-3）。

---

## 1. 背景与问题

ASR pipeline 重构的大愿景：asr 模块含一切 ASR 能力（含云端），desktop 只是壳。当前现实：

- **本地 ASR**（onnx：zipformer/whisper/qwen3 等）在 `octopus-asr`，cli/server/desktop 共用，走同步 `OfflineAsrEngine` trait。
- **云端 ASR**（4 provider WSS 流式）全在 `octopus-desktop`：`baidu_stream.rs`/`bytedance_stream.rs`/`aliyun_stream.rs`/`tencent_stream.rs` + `cloud_types.rs` + `cloud_pipeline.rs`。签名 `open(rt: &tauri::async_runtime::RuntimeHandle, ...)`——依赖 tauri runtime，cli/server 够不到。
- `asr` crate 是**纯同步**（无 tokio），被 cli/server/desktop 共用；`desktop` 才有 `tokio` + `tokio-tungstenite`（`cloud` feature）。

结果：cli 转译音频文件**只能本地 ASR**，无法选云端 API。这违背「asr 含一切 ASR」的愿景，也限制了 cli/server 的实用性。

## 2. 用户决策（brainstorming 2026-06-25）

1. **范围**：协议层 + 批处理下沉 asr 层；desktop 流式适配（`CloudPipelineEngine`）留 desktop。
2. **crate 结构**：新建 `octopus-asr-cloud`（依赖 asr，`asr` 保持纯同步零污染）。
3. **cli 配置**：复用 `AppConfig.asr.{provider}`（与 desktop 同源，不另建配置）。
4. **时机**：分两步——本次只 cli（cloud crate + 批引擎 + cli 接入，desktop 零改动、`*_stream.rs` 副本暂留）；后续第二步再让 desktop 删副本、改指 cloud 协议层。

## 3. 架构

### 3.1 crate 依赖图

```
octopus-asr-cloud ──→ octopus-asr        (impl OfflineAsrEngine trait)
                 ──→ octopus-infra       (ModelEntry, parse_model_spec, config 类型)
                 ──→ tokio, tokio-tungstenite(native-tls), uuid, base64, flate2, hmac, sha1

octopus-cli ──→ octopus-asr              (AsrEngineManager, pipeline, config)
            ──→ octopus-asr-cloud        (CloudBatchEngine, 云端分流)
            ──→ octopus-infra

octopus-desktop（本次不动）──→ 仍用自己的 *_stream.rs 副本
```

**依赖单向**：`asr ← cloud`，`asr` 不依赖 `cloud`（避免循环）。cli 同时依赖两者，在 cli 层做本地/云端分流。`asr` 保持纯同步、零 tokio。

### 3.2 三层分工

| 层 | crate | 形态 | 本次 |
|---|---|---|---|
| **协议层**（4 provider WSS） | `octopus-asr-cloud` | 纯 **async fn**（建连/鉴权/帧编解码/消息循环），**不自己 spawn** | ✅ 新建（从 desktop 复刻） |
| **批引擎** | `octopus-asr-cloud` | `CloudBatchEngine impl asr::OfflineAsrEngine`：整段音频→VAD 分段→每段推 WSS→拼接。同步，内部 `Runtime::new().block_on` | ✅ 新建 |
| **流式适配** | desktop | `CloudPipelineEngine`+`CloudStreamHandle`+coordinator 桥接 | ⏸ 不动（第二步复用协议层） |

### 3.3 runtime 方案（关键简化）

cloud 协议层是**纯 async fn 不 spawn** → 不依赖具体 runtime、不造 trait、不依赖 tauri：

- **批引擎**（cli/server）：内部 `tokio::runtime::Runtime::new().block_on(async { ... })`。cli 主线程非 tokio context，无嵌套 runtime 风险。
- **desktop**（第二步）：`tauri::async_runtime::spawn` 驱动 cloud 协议层 async fn，沿用现有同步/异步桥接。

cloud crate 只暴露 async 协议 fn + 同步批引擎，spawn 上下文由调用方定。无需 `AsyncRuntime` trait 或 Handle 注入。

## 4. `octopus-asr-cloud` crate

### 4.1 协议层（4 provider WSS，纯 async fn）

> **实施修正**（核对 desktop 源码后，详见 plan 顶部「据实修正」）：`open()` 保持**同步**签名（仅 `CloudStreamHandle::new()` + `tokio::spawn` + 返回 handle，不 await），唯一 async 收尾在 `CloudStreamHandle::close_async`；`CloudBatchEngine` 不自己 VAD 分段（`asr::pipeline::transcribe_segments` 自动分段 + CJK 连接）；`is_cloud_spec`/`from_spec` 用 `parse_model_spec` 的 **3-part provider 前缀**判云端（不查 DB），须 `provider:category:model_name` 三段 spec。

从 desktop `baidu_stream.rs`/`bytedance_stream.rs`/`aliyun_stream.rs`/`tencent_stream.rs` 复刻协议逻辑（建连、鉴权、二进制/JSON 帧编解码、WS 收发循环），改造为 **async fn**（去掉 `open()` 内部的 `tauri::async_runtime::spawn`，改为调用方驱动的 async fn）：

- `async fn open_<provider>(handle: tokio::runtime::Handle, config: ProviderConfig, pre_roll: &[f32]) -> Result<CloudStream>`
- `CloudStream`：暴露 `push_pcm(&self, samples)` / `finish(&self)` / `try_recv_event() -> Option<StreamEvent>` / `close_async()`（类型沿用 desktop `cloud_types.rs` 的 `PcmFrame`/`StreamEvent`/`CloudStreamHandle` 语义，迁入 cloud crate）。

**复刻原则**：协议字节级、鉴权算法、帧格式 1:1 照搬 desktop（零行为差异），仅把「同步 open + 内部 tauri spawn」重构为「async fn + 调用方 spawn」。

### 4.2 批引擎 `CloudBatchEngine`

```rust
pub struct CloudBatchEngine { /* provider config + tokio Handle */ }

impl OfflineAsrEngine for CloudBatchEngine {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        // 内部 Runtime::new().block_on：VAD 分段 → 每段一个 WSS session → 收 utterance → CJK 规则拼接
    }
    fn skip_corrector(&self) -> bool { /* 与 desktop 云端一致：云端已纠错，跳过本地 corrector？plan 确认 */ }
}
```

**音频策略**（plan 阶段最终确认）：复用 `asr::audio::segment_audio_vad` + `asr::vad::SileroVad` 把长音频分段，每段开一个云端 WSS session（短音频直连一个 session），收每段 `StreamEvent::Text`/`Finished`，按 CJK/非 CJK 规则拼接（复用 `asr::pipeline::transcribe_segments` 的连接逻辑或抽出共享）。这是「每段一个短 session」的 chunk 模式，适合批处理（无需维持长连接 onset/close 状态）。

### 4.3 provider 分发

复刻 desktop `cloud_pipeline.rs` 的分发：`EngineCategory`（Aliyun/Bytedance/Tencent/Baidu）+ `resolve_cloud_entry`/`resolve_<provider>_config`（从 `AppConfig.asr.<provider>` 查 `ModelEntry`，校验 `secret_key` 非空，返回 `(source, secret_key, model_name)`）。

cloud crate 暴露统一入口：
```rust
impl CloudBatchEngine {
    pub fn from_spec(spec: &str) -> Result<Self>;  // "aliyun:qwen-asr" → 解析 category + model_name → resolve config
}
```
`EngineCategory` + spec 解析逻辑在 cloud crate 定义（复用 infra `parse_model_spec` 拿 model_name；category 前缀解析新增）。desktop 第二步改用 cloud crate 的这套，消除重复。

## 5. cli 接入

`crates/cli/src/pipeline.rs::run` 改造为本地/云端分流：

```rust
pub fn run(model_spec: &str, language: &str, samples: &[f32]) -> Result<String> {
    let engine: Box<dyn OfflineAsrEngine> = if is_cloud_spec(model_spec) {
        Box::new(CloudBatchEngine::from_spec(model_spec)?)  // cli 直接构造云端引擎
    } else {
        let mgr = AsrEngineManager::new();
        mgr.switch_model(model_spec)?;                       // 本地 onnx
        mgr.into_active_engine()?                            // 取 Arc<dyn OfflineAsrEngine>（需加 getter）
    };
    let cfg = PipelineConfig::from_app_config(language);
    transcribe_batch(&*engine, samples, &cfg)                // 现有编排零改动
}
```

**依赖边界**：`AsrEngineManager`（asr crate）不支持云端（asr 不依赖 cloud）。分流在 cli 层完成，两端都产出 `dyn OfflineAsrEngine`，`transcribe_batch` 无感。

`AsrEngineManager` 需补一个公开 getter（取 `active_engine: Arc<dyn OfflineAsrEngine>`），供 cli 本地分支使用。

## 6. config 复用

云端 ASR 配置走 `octopus_asr::config::load_config().asr.{aliyun,bytedance,tencent,baidu}`（`Option<HashMap<String, ModelEntry>>`），与 desktop 完全同源。`ModelEntry`（`crates/infra/src/db.rs:13`）：`source`/`language`/`secret_key`/`is_local`/`is_enabled`/`is_streaming`/`description`。

各 provider 字段语义复刻 desktop 约定（`source`/`secret_key`/`model_name` 在不同 provider 复用为 endpoint/api_key/dev_pid 等，见 desktop `resolve_<provider>_config`）。cloud crate 依赖 infra 拿 `ModelEntry` 类型 + `parse_model_spec`。

## 7. 测试策略

- **协议层帧编解码**：纯函数（字节级编解码、鉴权串构造、Tencent HMAC-SHA1 签名、ByteDance gzip 帧）→ 单元测试，不连真实 WSS。
- **批引擎**：WSS 难单测，用 `#[ignore]` 真实 key 集成测试（同 desktop DashScope 模式），需用户本地 key 跑。
- **cli 分流**：单测 `is_cloud_spec` / spec 解析；端到端转译用真实 key 手动验（plan 列 e2e 清单）。

## 8. 不在范围

- **desktop 复用**（第二步）：删 desktop `*_stream.rs`/`cloud_types.rs` 协议副本，`CloudPipelineEngine` 改调 cloud crate 协议层。需云端流式 e2e 回归。
- **流式适配**（`CloudPipelineEngine`）：留 desktop，本次不动。
- **VadSegmented 归位**：2c-3，独立设计。
- **server 接入**：同 crate 自动可用（gRPC server 可构造 `CloudBatchEngine`），但本次不验证、不接入。

## 9. 风险与权衡

| 风险 | 缓解 |
|---|---|
| 协议层临时两份（desktop 副本 + cloud crate），第二步前重复维护 | 接受临时重复；协议字节级稳定，改动概率低；第二步尽快合并 |
| 批引擎音频策略（chunk 模式 vs 长连接）未定 | plan 阶段读 desktop `engine_aliyun.rs`（chunk 模式）确认，复用最贴近批处理的形态 |
| 长音频云端超时/限长 | VAD 分段控制单 session 长度；超时分多 session |
| `asr` 不能依赖 `cloud` 的循环约束 | 分流放 cli 层，`asr` 只出 trait；依赖单向 |
| cli 二进制增大（拉 tokio+tungstenite） | cli 本就需 ASR 能力，云端是可选价值；cloud crate 仅 cli/server/desktop 按需依赖，不影响纯本地构建路径 |

## 10. 后续（非本次）

- **第二步**：desktop 复用 cloud 协议层（删副本、`CloudPipelineEngine` 改指 cloud crate），云端流式 e2e 回归。
- **2c-3**：VadSegmented 归位。
- **2d**：coordinator 清理。
