# 云端 ASR 6 接口审查修复 + 去重重构

> 基于 2026-06-20 代码审查报告。4 个云端 provider（Aliyun/ByteDance/Tencent/Baidu，共 5 个协议变体）
> 存在 **2 个 Bug + ~250 行结构性重复 + 3 处架构异味**。

## 范围

| 优先级 | 项 | 说明 |
|---|---|---|
| P0 | Bug1 + Bug2 | 影响正确性，必须先修 |
| P1 | 类型归属 + CloudStreamHandle | 消除 ~170 行重复 |
| P2 | coordinator dispatch 统一 | 消除 ~200 行重复 |
| P3 | 小整洁 | accumulate_display 推广 + 常量 |

**不做**：cargo feature 改名（`aliyun` → `cloud`），影响面大（Cargo.toml + 所有 `#[cfg]` + 脚本 + 文档），留作后续独立任务。

## P0 Bug 修复

### Bug1：3 provider 缺 WS 断连 Failed 上报

`aliyun_stream.rs`（含 Qwen 变体）在 `ws.next() = None`（服务端意外断开）时上报 `StreamEvent::Failed("WS 连接意外关闭")`，
但 ByteDance/Tencent/Baidu 仅静默 `break`，循环退出后 `if !finished` 误发 `Finished`。
→ coordinator 把残缺 partial 当最终结果 paste，用户看不到错误。

**修复**：3 处 `None => break` 改为先发 `Failed` 再 break；循环后 `if !finished` 块的 `Finished` 改 `Failed`（或直接删除冗余分支）。

### Bug2：baidu `if !finished { } else { }` 两分支完全相同

`baidu_stream.rs:262-276`。`finished=true` 路径循环内已 `break`，不会走到 else。
→ 删除 else，`if !finished` 改发 `Failed`（配合 Bug1）。

## P1 类型归属 + CloudStreamHandle

### 新建 `cloud_types.rs`（`#[cfg(feature = "aliyun")]`）

从 `aliyun_stream.rs` / `engine_aliyun.rs` 提取共用类型到独立模块：

```rust
pub(crate) enum PcmFrame { Samples(Vec<u8>), Finish }
pub enum StreamEvent { Text(String), Finished, Failed(String) }
pub(crate) fn samples_to_pcm_s16le(samples: &[f32]) -> Vec<u8> { ... }
const CLOUD_CLOSE_TIMEOUT_SECS: u64 = 8;

/// 4 provider 共用的 session 句柄（消除 4×4=16 个方法实现 → 4 个共用方法）
pub struct CloudStreamHandle {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpsc::UnboundedReceiver<StreamEvent>,
}
impl CloudStreamHandle {
    pub fn new() -> (Self, Receiver<PcmFrame>, Sender<StreamEvent>);
    pub fn push_pcm(&self, samples: &[f32]) -> Result<()>;      // 共用
    pub fn finish(&self) -> Result<()>;                          // 共用
    pub fn try_recv_text(&mut self) -> Option<StreamEvent>;      // 共用
    pub async fn close_async(self) -> Result<String>;            // 共用（含 8s 超时）
}
```

### 改造 4 provider

- 删除各自 `XxxStreamSession` struct + impl 块（4×~60 行）
- `open()` 返回 `CloudStreamHandle`，内部 `CloudStreamHandle::new()` + `rt.spawn(run_xxx_session(...))`
- 保留各自 `run_xxx_session` 协议函数 + 协议特定 helper（build_signed_url / build_client_frame 等）

### 简化 cloud_session.rs + coordinator

- 删除 `CloudSession` enum（4 变体 + 4 方法 dispatch，共 62 行）→ 统一用 `CloudStreamHandle`
- `coordinator.rs`：`session: Option<CloudSession>` → `Option<CloudStreamHandle>`
- onset dispatch 不再包 enum 变体

## P2 coordinator dispatch 统一

### 提取 `resolve_cloud_entry`

4 个 `resolve_xxx_config` 结构一致，只差 section 名 + 校验。提取：
```rust
fn resolve_cloud_entry(section: Option<&HashMap<...>>, provider: &str, model: &str) -> Result<&ModelEntry, String>
```
4 个 provider 的 resolve 变成 ~5 行薄封装。

### 提取 `open_cloud_session`

onset dispatch 的 4 个 ~30 行分支提取为 1 个函数：
```rust
fn open_cloud_session(cat: EngineCategory, config: &AppConfig, pre_roll: Vec<f32>) -> Result<CloudStreamHandle, String>
```
调用方简化为 ~5 行。

## P3 小整洁

- `accumulate_display`（baidu 已有）推广到 tencent（line 240-245 手写同样逻辑）
- `8s` 超时魔数 → `CLOUD_CLOSE_TIMEOUT_SECS` 常量（P1 已在 cloud_types 提取）

## 任务分解

| # | 任务 | 文件 | 验证 |
|---|---|---|---|
| 1 | 写 plan | 本文件 | — |
| 2 | P0-Bug1 | bytedance/tencent/baidu `_stream.rs` | cargo test |
| 3 | P0-Bug2 | baidu_stream.rs | cargo test |
| 4 | 新建 cloud_types.rs | cloud_types.rs, main.rs | cargo build |
| 5 | 改造 4 provider | 4×_stream.rs + engine_aliyun.rs | cargo build |
| 6 | 简化 cloud_session + coordinator | cloud_session.rs, coordinator.rs | cargo test |
| 7 | resolve + dispatch 统一 | coordinator.rs | cargo test |
| 8 | accumulate_display + 常量 | tencent/baidu/cloud_types | cargo test |
| 9 | 全套验证 | — | cargo build + test 全部 crate |
| 10 | 文档 + 提交 | architecture.md 等 | git commit |

## 预期收益

- 消除 ~250 行重复代码
- 修复 2 个正确性 Bug
- provider 寄生依赖消除（PcmFrame/StreamEvent/samples_to_pcm_s16le 不再寄居 aliyun 模块）
- 新增 provider 成本：从 ~30 分钟/7 步降至 ~15 分钟/3 步（只需写 run_xxx_session + resolve 薄封装）

## 实施记录（2026-06-20）

实际实现与计划基本一致，偏差记录：

- **P0-Bug1 修复范围扩大**：不仅修了 `ws.next()=None`，还发现 `ws.next()=Some(Err(e))` 和协议错误码（tencent code≠0）路径同样误发 `Finished`。统一改为发 `Failed` + `return Ok(())`，删除循环后冗余的 `if !finished` 块。
- **P1 CloudStreamHandle 成功消除 ~440 行**：8 files changed, 247 insertions(+), 687 deletions(-)。`cloud_session.rs` 完全删除，4 个 provider struct 删除。
- **P2 resolve_cloud_entry 需生命周期标注**：函数返回 `&ModelEntry` 引用，编译器要求显式 `'a`。
- **P3 跳过**：8s 常量已在 P1 的 `cloud_types.rs` 提取（`CLOUD_CLOSE_TIMEOUT_SECS`）；`accumulate_display` 推广到 tencent 收益极小（数据结构不同：BTreeMap vs Vec），不值得强行抽象。
- **cargo feature 改名跳过**：`aliyun` → `cloud` 影响面大（Cargo.toml + 所有 `#[cfg]` + 脚本 + 文档），留作后续独立任务。
