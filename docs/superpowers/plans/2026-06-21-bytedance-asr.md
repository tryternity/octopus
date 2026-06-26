# 火山引擎豆包大模型流式 ASR 实施计划

> Spec：`docs/superpowers/specs/2026-06-21-bytedance-asr-design.md`
> 对标实现：DashScopeStreamSession + EngineCategory::Aliyun

## Task 1：infra 层 — AsrSection.bytedance 字段 + db.sql seed

### 1.1 crates/infra/src/db.rs

- `AsrSection` 新增字段（紧跟 `aliyun` 之后）：
  ```rust
  /// 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）。
  #[serde(default)]
  pub bytedance: Option<HashMap<String, ModelEntry>>,
  ```
- `load_asr_config` 新增 bytedance section 映射（对标 aliyun 的 match arm）
- struct initializer `asr = AsrSection { ... bytedance, }` 补字段

### 1.2 crates/infra/src/db.sql

在 moonshine seed 之后、aliyun cloud seed 之前，新增 bytedance seed：
```sql
-- 火山引擎豆包大模型 ASR（bigmodel_async 双向流式优化版）
-- endpoint 固定 wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async
-- source = X-Api-Resource-Id；secret_key = X-Api-Key（火山引擎控制台申请）
('asr','bytedance','Doubao-ASR','doubao-asr-1.0-streaming','volc.bigasr.sauc.duration','zh','火山引擎豆包大模型 ASR 1.0（bigmodel_async，时长计费，key 填 secret_key）',0,0,1),
('asr','bytedance','Doubao-ASR-2.0','doubao-asr-2.0-streaming','volc.seedasr.sauc.duration','zh','火山引擎豆包大模型 ASR 2.0（bigmodel_async，时长计费，key 填 secret_key）',0,0,0);
```

### 1.3 crates/infra/src/db.rs 测试

- `init_sql_is_idempotent` / `seed_then_load_round_trips` 的 seed 行数断言更新（当前 N → N+2）
- `load_asr_config` 测试新增 bytedance section 断言

### 验证
```bash
cargo test -p octopus-infra
```

---

## Task 2：asr config 层 — EngineCategory::ByteDance + 6 处映射

### 2.1 crates/asr/src/config.rs

1. `EngineCategory` 新增 `ByteDance` 变体
2. `engine_category_from_str` 无需改（bytedance 由 provider 路由，不通过 category str）
3. `resolve_category` 新增 `bytedance` provider 分支：
   ```rust
   if provider.eq_ignore_ascii_case("bytedance") {
       return Some(EngineCategory::ByteDance);
   }
   ```
4. `all_sections` 维度从 7→8，追加 `(cfg.asr.bytedance.as_ref(), EngineCategory::ByteDance)`
5. `provider_of` 新增 `ByteDance => "bytedance"`
6. `category_label` 新增 `ByteDance => "Doubao-ASR"`（与 DB category 列一致）
7. `pick_entry` 新增 bytedance match arm
8. `is_streaming_engine` 更新：ByteDance 也返回 false（云端引擎在 coordinator 由 `is_cloud_engine`
   路由，不进本地 StreamingSession——与 Aliyun 一致）

### 2.2 测试

config.rs 内联测试更新（struct literal 补 `bytedance: None`）

### 验证
```bash
cargo test -p octopus-asr-local --release
cargo run -p octopus-cli -- config   # 应列出 Doubao-ASR 引擎
```

---

## Task 3：desktop 层 — ByteDanceStreamSession 二进制协议实现

### 3.1 新文件 crates/desktop/src/bytedance_stream.rs

镜像 `dashscope_stream.rs` 的接口（`PcmFrame` / `StreamEvent` / session struct），
但内部实现火山的二进制帧协议。

**模块结构**：
```rust
// 常量（二进制协议）
const PROTOCOL_VERSION: u8 = 0x1;
const HEADER_SIZE: u8 = 0x1;
// Message types
const MSG_FULL_CLIENT_REQUEST: u8 = 0x1;
const MSG_AUDIO_ONLY_REQUEST: u8 = 0x2;
const MSG_FULL_SERVER_RESPONSE: u8 = 0x9;
const MSG_ERROR_RESPONSE: u8 = 0xF;
// Flags
const FLAG_NO_SEQUENCE: u8 = 0x0;
const FLAG_POS_SEQUENCE: u8 = 0x1;
const FLAG_NEG_SEQUENCE: u8 = 0x2;      // 末帧（负包）
const FLAG_NEG_WITH_SEQUENCE: u8 = 0x3; // 末帧 + seq
// Serialization
const SER_NONE: u8 = 0x0;
const SER_JSON: u8 = 0x1;
// Compression
const COMP_NONE: u8 = 0x0;
const COMP_GZIP: u8 = 0x1;

// 固定 endpoint
const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";

// 复用 dashscope_stream 的 PcmFrame + StreamEvent（pub(crate) re-export）
use crate::dashscope_stream::{PcmFrame, StreamEvent};
```

**二进制帧构造**：
- `build_full_client_request(api_key: &str, resource_id: &str, language: &str) -> (Vec<u8>, Vec<u8>)`
  返回 (WS 握手 headers 构造所需的 resource_id/request_id, 初始帧 bytes)
- `build_audio_frame(pcm: &[u8], is_last: bool) -> Vec<u8>` — 构造 AUDIO_ONLY_REQUEST 帧

**帧解析**：
- `parse_server_response(data: &[u8]) -> Result<(u8 /* msg_type */, u8 /* flags */, Vec<u8> /* payload */)>`
  从 4B header + 可选 seq + payload size 提取 payload
- 若 GZIP 压缩则 decompress

**Session struct**：
```rust
pub struct ByteDanceStreamSession {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpc::UnboundedReceiver<StreamEvent>,
}
```

**open()** 流程：
1. 构造 WS 握手 headers（`X-Api-Key` / `X-Api-Resource-Id` / `X-Api-Request-Id` / `X-Api-Sequence: -1`）
2. `connect_async` 建连
3. 发 FULL_CLIENT_REQUEST 帧（gzip JSON config）
4. 推 pre-roll PCM（AUDIO_ONLY_REQUEST）
5. spawn 后台 task 进入双向循环

**run_bytedance_session()** 后台 task：
- 双向 `tokio::select!`：
  - 收 `PcmFrame::Samples` → gzip → 发 AUDIO_ONLY_REQUEST 帧
  - 收 `PcmFrame::Finish` → 发末帧（flags=NEG_SEQUENCE）→ 等待最终响应
  - 收 WS message → `parse_server_response` → decompress → 解析 JSON →
    - `result.text` → `StreamEvent::Text`
    - flags=0x3（末帧）→ `StreamEvent::Finished`
    - MSG_ERROR_RESPONSE → `StreamEvent::Failed`

### 3.2 crates/desktop/src/main.rs

注册模块：`mod bytedance_stream;`（dashscope feature gated）

### 验证
```bash
cargo build -p octopus-desktop --features embedded,aliyun
```

---

## Task 4：coordinator 层 — 云端引擎分派

### 4.1 crates/desktop/src/coordinator.rs

1. `is_cloud_engine` 扩展：
   ```rust
   fn is_cloud_engine(config: &AppConfig) -> bool {
       let cat = octopus_asr_local::config::resolve_engine_category(&config.asr_engine);
       cat == Some(octopus_asr_local::config::EngineCategory::Aliyun)
           || cat == Some(octopus_asr_local::config::EngineCategory::ByteDance)
   }
   ```

2. `resolve_dashscope_config` 重构为 `resolve_cloud_config`（返回 enum）：
   ```rust
   enum CloudSession {
       DashScope(crate::dashscope_stream::DashScopeStreamSession),
       VolcEngine(crate::bytedance_stream::ByteDanceStreamSession),
   }
   ```
   根据 category 从对应 section（`asr.aliyun` / `asr.bytedance`）解析 endpoint + key + model

3. `Stage::CloudStreaming.session` 类型从 `Option<DashScopeStreamSession>` 改为 `Option<CloudSession>`

4. `handle_cloud_streaming_tick` 中 `session.push_pcm` / `session.try_recv_text` 通过 enum 分派：
   ```rust
   match session {
       CloudSession::DashScope(s) => s.push_pcm(...),
       CloudSession::VolcEngine(s) => s.push_pcm(...),
   }
   ```

5. `close_async` 路径同样通过 enum 分派

### 4.2 关键约束

- `StreamEvent` 复用 `dashscope_stream::StreamEvent`，不新建 enum
- `PcmFrame` 复用 `dashscope_stream::PcmFrame`（`pub(crate)` 可见性）
- coordinator 主体逻辑（VAD gating / onset 确认 / silence finalize）零改动——两个 provider
  的 session 接口完全一致

### 验证
```bash
cargo build -p octopus-desktop --features embedded,aliyun
cargo test -p octopus-desktop
```

---

## Task 5：CLI 接入（可选）

`crates/cli/src/main.rs` 的 `do_transcribe` 新增 `ByteDance` 分支：
```rust
Some(EngineCategory::ByteDance) => {
    anyhow::bail!("火山引擎 ASR 引擎仅支持流式模式（需 WS 连接），CLI transcribe 尚未接入")
}
```
（与 Aliyun 一致——云端流式引擎不在 CLI 单文件转录路径接入）

---

## Task 6：构建 + 测试 + 文档

### 6.1 构建
```bash
cargo build --release -p octopus-infra -p octopus-asr-local
cargo build --release -p octopus-desktop --features embedded,aliyun
cargo build --release -p octopus-cli
```

### 6.2 测试
```bash
cargo test -p octopus-infra
cargo test -p octopus-asr-local --release
cargo test -p octopus-desktop
```

### 6.3 文档
- `docs/architecture.md`：新增 bytedance provider 说明 + 端到端流程
- `docs/configuration.md`：新增 bytedance seed 表格 + Resource ID 说明

---

## 风险与验证策略

| 风险 | 缓解 |
|---|---|
| 无 API Key 无法实测 | 协议严格按文档实现；单元测试覆盖帧构造/解析；Key 到位后 e2e 验证 |
| 二进制帧字节序错误 | 用 `u32::to_be_bytes()` 确保大端 |
| gzip 压缩兼容性 | 用 `flate2::write::GzipEncoder`，与 Python `gzip` 兼容 |
| 末帧 EOF 信号错误 | flags=0x2（NEG_SEQUENCE）= 末帧；flags=0x3（NEG_WITH_SEQUENCE）= 末帧+seq（服务端响应用） |

## 用户验证步骤（Key 到位后）

1. 在火山引擎控制台开通豆包大模型 ASR，获取 API Key + Resource ID
2. `sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='<KEY>' WHERE model_name='doubao-asr-1.0-streaming';"`
3. 启动桌面应用，引擎选 `bytedance:Doubao-ASR:doubao-asr-1.0-streaming`
4. 录音测试流式识别

---

## 实施记录（2026-06-21 完成）

### 实际偏差与新增决策

1. **DashScope → Aliyun 重命名**（同期完成）：provider 名称从产品名 `DashScope` 改为厂商名 `Aliyun`，与新增的 `ByteDance`（厂商名）对齐。涉及：
   - cargo feature：`dashscope` → `aliyun`
   - 文件：`dashscope_stream.rs` → `aliyun_stream.rs`、`engine_dashscope.rs` → `engine_aliyun.rs`
   - 类型：`DashScopeStreamSession` → `AliyunStreamSession`、`DashscopeEngine` → `AliyunEngine`
   - 函数：`resolve_dashscope_config` → `resolve_aliyun_config`
   - `aliyun` feature 同时 gate 两个云端 provider（Aliyun + ByteDance），因为都依赖 WS 流式基础设施

2. **CloudSession enum 分派**（Task 4 实际实现）：新建 `cloud_session.rs` 模块，定义 `CloudSession` enum 包装 `Aliyun(AliyunStreamSession)` / `ByteDance(ByteDanceStreamSession)` 两个变体，提供统一的 `push_pcm` / `finish` / `try_recv_text` / `close_async` 方法。coordinator 的 `Stage::CloudStreaming.session` 字段类型从 `Option<AliyunStreamSession>` 改为 `Option<CloudSession>`。onset 开 WSS 时按 `EngineCategory` 分派构造对应变体。

3. **`PcmFrame` 可见性**：从 `enum PcmFrame` 改为 `pub(crate) enum PcmFrame`（定义在 `aliyun_stream.rs`），`StreamEvent` 已是 `pub`。两个 provider 共享这两个类型。

4. **`take_preroll` 辅助函数**：提取 pre-roll 缓冲区取样的逻辑为独立函数（避免两个 provider 分支重复）。

5. **whisper-tiny/base 从 seed 移除**（同期完成）：只保留 `whisper-small.en`（已验证可用），tiny/base 输出不稳定已从 db.sql 删除。

### 验证结果（2026-06-21）

| 验证项 | 结果 |
|---|---|
| `cargo build -p octopus-infra -p octopus-asr-local` | ✅ PASS |
| `cargo build -p octopus-cli` | ✅ PASS |
| `cargo build -p octopus-desktop --features embedded,aliyun` | ✅ PASS（0 warnings） |
| `cargo build -p octopus-server` | ✅ PASS |
| `cargo test -p octopus-infra` | ✅ 29 passed |
| `cargo test -p octopus-asr-local` | ✅ 54 passed (6 ignored) |
| `cargo test -p octopus-desktop` | ✅ 53 passed (1 ignored) |

### 未完成 / 待验证

- **e2e 实测**：无 API Key，协议严格按火山文档实现，5 个单元测试覆盖帧构造/解析/gzip roundtrip。Key 到位后需 e2e 验证。
- **`enable_ddc: false`**：config JSON 含此字段，语义待 Key 到位后验证（疑似 disable data compression）。

