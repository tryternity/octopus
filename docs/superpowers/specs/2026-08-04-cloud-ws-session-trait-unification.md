# 云端 ASR WS Session 循环 trait 统一设计

- 日期：2026-08-04
- 分支：`refactor/too-many-arguments`
- Worktree：`.worktrees/refactor/too-many-arguments`
- 类型：重构（架构改进，零行为变更）
- 触发：代码审查问题 B（4 provider WS session 循环骨架重复）

---

## 1. 背景与动机

`crates/asr-cloud` 有 **5 个 session 函数**（aliyun 含 2 个）实现同一职责——"建连 → 初始帧 → pre-roll → 双向 select! 循环 → 终态"：

| 函数 | 文件:行 | 行数 |
|---|---|---|
| `run_baidu_session` | `baidu_stream.rs:87-267` | 180 |
| `run_tencent_session` | `tencent_stream.rs:99-255` | 156 |
| `run_bytedance_session` | `bytedance_stream.rs:104-327` | 223 |
| `run_ws_session`（aliyun FunASR） | `aliyun_stream.rs:79-292` | 213 |
| `run_qwen_realtime_session`（aliyun Qwen） | `aliyun_stream.rs:383-589` | 206 |

### 1.1 精确重复度量

审查报告说"~600 行重复"**夸大了**。实际：

| 重复类型 | 范围 | 行数/份 | × 5 份 |
|---|---|---:|---:|
| **逐字重复**（select! 双向循环 + 4 路错误 match + PCM dispatch 头） | 每份 session 中段 | ~30 | ~150 |
| **结构相似**（建连 timeout + pre-roll + Message::Close 终态判定） | 每份头尾 | ~40 | ~200 |
| **provider 特定**（鉴权 headers、协议帧、结果解析、状态结构） | 各不相同 | ~80-150 | — |

**结论**：逐字重复约 150 行，结构相似约 200 行，合计 ~350 行可收敛；provider 特定部分（结果解析、状态管理、协议编码）无法合并。

### 1.2 痛点

- **错误处理路径漂移**：5 份 WS 错误 match（Err timeout / Ok(None) / Ok(Some(Err))）逐字复制，但文本前缀（"baidu WS read timeout" / "tencent WS read timeout" / ...）和个别失败语义略有差异。历史上 #3、#4、H1、G1、G2、R1 等 bug 都源于"改了 A 没同步改 B"。
- **新增 provider 成本高**：当前 5 份 × ~200 行，新增一家要复制 ~200 行骨架再填协议。
- **测试覆盖割裂**：每份 session 各自有 `close_frame_emits_*` 测试，靠 `spawn_xxx_and_collect` helper 复制 4 份。

### 1.3 目标

抽出 `WsSessionLoop` 骨架（双向 select! 循环 + 错误处理 + pre-roll + Close 终态判定），各 provider 用 trait 填协议特定 hook。**零行为变更**——所有现有测试（含 `#[ignore]` real-model）保持通过，不改对外 API（`provider::open` 签名不变）。

### 1.4 非目标

- ❌ 不改对外 API：`provider::open(...)` 签名、`CloudStreamHandle`、`open_cloud_session`、`PcmFrame`/`StreamEvent` 全不动
- ❌ 不动协议逻辑：SQL/帧格式/JSON 解析/状态机一字不改
- ❌ 不合并 aliyun 两套协议（FunASR vs Qwen）——它们协议差异大，保持两份 trait 实现
- ❌ 不抽 `accumulate_display`（句间分隔符逻辑，已在 baidu 抽出，tencent/aliyun 内联——另议，见 §6）

---

## 2. trait 设计

### 2.1 核心 trait：`WsSessionHandler`

```rust
/// WS session 协议 hook——各 provider 实现，由 [`run_ws_session_loop`] 驱动。
///
/// 生命周期：build_connect_request → build_init_message → build_pcm_message（× N）→
///           build_finish_message → handle_message（× N，直到终态）。
///
/// handler 持有 provider 特定状态（如 baidu 的 fin_texts、aliyun-FunASR 的 committed）。
pub(crate) trait WsSessionHandler {
    /// provider 标签（用于错误消息前缀，如 "baidu"/"tencent"）。
    const LABEL: &'static str;

    // ── 建连阶段 ──

    /// 构造 connect_async 的请求（含 provider 特定鉴权 headers）。
    ///
    /// 调用方传入 endpoint URL，handler 返回带 headers 的 request。
    fn build_connect_request(&self, endpoint: &str)
        -> anyhow::Result<tokio_tungstenite::tungstenite::handshake::client::Request>;

    // ── 发送阶段（返回 Message 由 loop 统一 ws.send）──

    /// 初始帧（建连后立即发）：START / run-task / session.update / FULL_CLIENT_REQUEST。
    fn build_init_message(&self) -> anyhow::Result<Message>;

    /// pre-roll / 实时 PCM 帧：返回要 send 的 Message。
    /// - baidu/tencent/aliyun-FunASR：`Message::Binary(samples_to_pcm_s16le(...))`
    /// - bytedance：`Message::Binary(build_client_frame(AUDIO, ...))`
    /// - aliyun-Qwen：`Message::Text(json!{input_audio_buffer.append, audio: base64})`
    fn build_pcm_message(&self, pcm_s16le: &[u8]) -> anyhow::Result<Message>;

    /// finish 帧（收到 PcmFrame::Finish 时发）：FINISH / end / finish-task / 末帧 / session.finish。
    fn build_finish_message(&self) -> anyhow::Result<Message>;

    // ── 接收阶段 ──

    /// 处理一条 WS 消息（Text/Binary/Close 均会传入，handler 内部 match）。
    ///
    /// 返回值指示 loop 后续动作：
    /// - `Continue`：继续循环（handler 已自行 send StreamEvent::Text，或忽略）
    /// - `TerminalFinished`：识别完成，loop 应发 Finished 后 return（handler 已发最终 Text）
    /// - `TerminalFailed(String)`：失败，loop 应发 Failed 后 return
    ///
    /// **Close 帧终态判定由 handler 负责**（各 provider 稳态判据不同：baidu 用 fin_texts 非空、
    /// bytedance 用 last_text.is_some、aliyun-FunASR 用 committed 非空、aliyun-Qwen 用
    /// accumulated_text 非空）。handler 收到 `Message::Close` 时自行判断并返回 Terminal*。
    fn handle_message(&mut self, msg: Message, result_tx: &mpsc::UnboundedSender<StreamEvent>)
        -> HandleOutcome;
}

pub(crate) enum HandleOutcome {
    /// 继续循环
    Continue,
    /// 终态：识别完成（handler 已发最终 Text，loop 只补发 Finished）
    TerminalFinished,
    /// 终态：失败（loop 发 Failed(msg)）
    TerminalFailed(String),
}
```

### 2.2 骨架函数：`run_ws_session_loop`

```rust
/// WS session 主循环骨架——驱动建连 → init → pre-roll → 双向循环 → 终态。
///
/// 收敛 5 份 session 的共同逻辑：
/// - connect timeout（WS_CONNECT_TIMEOUT_SECS）
/// - 发 init + pre-roll（非空才发）
/// - 双向 select!{ pcm_rx.recv() => dispatch, ws.next() with timeout => 4 路 match }
/// - 错误处理（timeout/None/Some(Err)）统一发 Failed 后 return
/// - handle_message 返回 Terminal* 时发终态事件后 return
///
/// handler 负责：建连 request、协议帧构造、消息解析、Close 终态判定。
async fn run_ws_session_loop<H: WsSessionHandler>(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: &str,
    pre_roll_samples: &[f32],
    mut handler: H,
) -> anyhow::Result<()> {
    let label = H::LABEL;
    // 1. 建连（handler 构造 request，loop 负责 timeout + connect_async）
    let request = handler.build_connect_request(endpoint)
        .with_context(|| format!("{label} WS 请求构造失败"))?;
    let (mut ws, _resp) = tokio::time::timeout(
        std::time::Duration::from_secs(octopus_infra::net::WS_CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{label} WS connect timeout"))?
    .with_context(|| format!("{label} WS 连接失败: {endpoint}"))?;

    // 2. 发 init 帧
    let init_msg = handler.build_init_message()?;
    ws.send(init_msg).await
        .with_context(|| format!("{label} WS 发送初始帧失败"))?;

    // 3. 推 pre-roll（非空才发）
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(pre_roll_samples);
        let pre_roll_msg = handler.build_pcm_message(&pcm)?;
        ws.send(pre_roll_msg).await
            .with_context(|| format!("{label} WS 发送 pre-roll PCM 失败"))?;
    }

    // 4. 双向循环
    loop {
        tokio::select! {
            frame = pcm_rx.recv() => match frame {
                Some(PcmFrame::Samples(pcm)) => {
                    let msg = handler.build_pcm_message(&pcm)?;
                    ws.send(msg).await
                        .with_context(|| format!("{label} WS 发送音频帧失败"))?;
                }
                Some(PcmFrame::Finish) => {
                    let msg = handler.build_finish_message()?;
                    ws.send(msg).await
                        .with_context(|| format!("{label} WS 发送结束帧失败"))?;
                }
                None => break,
            }
            msg = tokio::time::timeout(
                std::time::Duration::from_secs(octopus_infra::net::WS_READ_TIMEOUT_SECS),
                ws.next(),
            ) => {
                let msg = match msg {
                    Err(_) => {
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("{label} WS read timeout")
                        ));
                        return Ok(());
                    }
                    Ok(None) => break,
                    Ok(Some(Err(e))) => {
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("{label} WS 读错误: {e}")
                        ));
                        return Ok(());
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match handler.handle_message(msg, &result_tx) {
                    HandleOutcome::Continue => {}
                    HandleOutcome::TerminalFinished => {
                        let _ = result_tx.send(StreamEvent::Finished);
                        return Ok(());
                    }
                    HandleOutcome::TerminalFailed(reason) => {
                        let _ = result_tx.send(StreamEvent::Failed(reason));
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}
```

### 2.3 provider 实现示例（baidu）

```rust
// baidu_stream.rs
pub(crate) struct BaiduHandler {
    appid_int: i64,
    appkey: String,
    dev_pid_int: i64,
    cuid: String,
    language: String,
    // 结果累积状态
    fin_texts: Vec<String>,
    current_partial: String,
}

impl WsSessionHandler for BaiduHandler {
    const LABEL: &'static str = "baidu";

    fn build_connect_request(&self, endpoint: &str) -> Result<Request> {
        // baidu 无鉴权 headers，直接 endpoint.into_client_request()
        // （实际 baidu 拼 sn query，在 handler 内完成）
        let sn = uuid::Uuid::new_v4().to_string();
        let url = format!("{endpoint}?sn={sn}");
        url.as_str().into_client_request()
    }

    fn build_init_message(&self) -> Result<Message> {
        let start = json!({"type":"START","data":{
            "appid": self.appid_int, "appkey": self.appkey,
            "dev_pid": self.dev_pid_int, "cuid": self.cuid,
            "format":"pcm", "sample":16000
        }});
        Ok(Message::Text(start.to_string()))
    }

    fn build_pcm_message(&self, pcm: &[u8]) -> Result<Message> {
        Ok(Message::Binary(pcm.to_vec()))
    }

    fn build_finish_message(&self) -> Result<Message> {
        Ok(Message::Text(r#"{"type":"FINISH"}"#.into()))
    }

    fn handle_message(&mut self, msg: Message, tx: &Sender<StreamEvent>) -> HandleOutcome {
        match msg {
            Message::Text(text) => {
                // 原 baidu JSON 解析逻辑搬进来（err_no 判定 / MID_TEXT / FIN_TEXT / HEARTBEAT）
                // 累积 fin_texts / current_partial，发 StreamEvent::Text(display)
                // ...
                HandleOutcome::Continue
            }
            Message::Close(_) => {
                // 原 baidu Close 终态判定（fin_texts 非空 → Finished，否则 Failed）
                if !self.fin_texts.is_empty() {
                    let display = accumulate_display(&self.fin_texts, &self.current_partial, &self.language);
                    let _ = tx.send(StreamEvent::Text(display));
                    HandleOutcome::TerminalFinished
                } else if !self.current_partial.is_empty() {
                    HandleOutcome::TerminalFailed("baidu WS 连接关闭但仅收到非稳态 partial".into())
                } else {
                    HandleOutcome::TerminalFailed("baidu WS 连接关闭但未收到识别结果".into())
                }
            }
            _ => HandleOutcome::Continue,
        }
    }
}

// run_baidu_session 改为薄包装
async fn run_baidu_session(
    pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    config: BaiduSessionConfig,
) -> Result<()> {
    let BaiduSessionConfig { endpoint, appid, appkey, dev_pid, language, pre_roll_samples } = config;
    let appid_int = appid.parse().with_context(|| format!("baidu appid '{appid}' 不是有效整数"))?;
    let dev_pid_int = dev_pid.parse().with_context(|| format!("baidu dev_pid '{dev_pid}' 不是有效整数"))?;
    let handler = BaiduHandler {
        appid_int, appkey, dev_pid_int, cuid: uuid::Uuid::new_v4().to_string(),
        language, fin_texts: Vec::new(), current_partial: String::new(),
    };
    run_ws_session_loop(pcm_rx, result_tx, &endpoint, &pre_roll_samples, handler).await
}
```

其余 4 个 provider（tencent/bytedance/aliyun-FunASR/aliyun-Qwen）同模式。

---

## 3. 不变量（必须保持）

1. **零行为变更**——5 份 session 的协议帧、JSON 解析、状态机、终态判定逻辑一字不改地搬进 trait 实现
2. **对外 API 不变**：
   - `provider::open(...)` 签名、返回 `Result<CloudStreamHandle>` 不动
   - `CloudStreamHandle` / `PcmFrame` / `StreamEvent` 不动
   - `open_cloud_session` / batch 引用不动
3. **测试全过**：
   - `cargo test -p octopus-asr-cloud` 全过（含 5 个 `close_frame_emits_*` WS mock 测试）
   - 错误消息文本保持兼容（测试断言 `"应发 Failed"` 等不依赖具体前缀，但保守起见保持原文本）
4. **错误消息语义不变**：5 份的 `"xxx WS read timeout"` / `"xxx WS 读错误"` / `"xxx WS connect timeout"` 文本保持——用 `H::LABEL` 插值生成，与原文本逐字一致
5. **`#[ignore]` real-model 测试不动**（需真实 provider 凭据，不在 CI 跑）

---

## 4. 关键设计决策

### 4.1 为什么用 trait 而非闭包

报告建议 `run_ws_session_loop(ws, pcm_rx, result_tx, build_start_frame, handle_text_msg, finish_frame)`——**纯闭包不够**：
- handler 需持有可变状态（fin_texts / committed / last_text / accumulated_text），闭包捕获 `&mut` 在 select! 循环里跨 await 会有 borrow 问题
- 6 个 hook（build_connect_request/build_init/build_pcm/build_finish/handle_message + 状态）如果全做成闭包参数，签名比 trait 更乱
- trait 的 `handle_message(&mut self, ...)` 天然支持状态变更

### 4.2 为什么 handle_message 接 `Message` 而非 `Text`/`Binary`

bytedance 收 `Message::Binary`，其余收 `Message::Text`——让 handler 内部 match，trait 不预分流。这样 trait 对所有 provider 统一，且未来若有 provider 收 Ping 也能扩展。

### 4.3 为什么 Close 终态判定放 handler 而非 loop

5 个 provider 的稳态判据完全不同：
- baidu：`!fin_texts.is_empty()`
- tencent：`!stable_segments.is_empty()`
- bytedance：`last_text.is_some()`
- aliyun-FunASR：`!committed.is_empty()`
- aliyun-Qwen：`!accumulated_text.is_empty()`

判据依赖 handler 内部状态，loop 无法统一。handler 收 Close 时自行判断，返回 `TerminalFinished` / `TerminalFailed`，loop 只负责发终态事件。

### 4.4 为什么不合并 aliyun 两套协议

aliyun `open()` 根据 endpoint 分发到 FunASR 或 Qwen——两套协议差异大（binary vs base64-JSON、run-task vs session.update、result-generated vs input_audio_transcription）。它们各自独立实现 trait，保持清晰。`open()` 仍是薄分发层。

### 4.5 `accumulate_display` 暂不统一

baidu 已抽出 `accumulate_display`，tencent/aliyun-FunASR/aliyun-Qwen 内联了等价逻辑（句间 sep 守卫）。**本次不合并**——它们的状态结构不同（Vec<String> / BTreeMap<i64,String> / String::push_str），统一需要先统一状态结构，超出本次范围。留作后续改进。

---

## 5. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 搬迁引入笔误（帧格式/JSON 字段） | 中 | 协议错乱 | 每搬一个 provider 立即跑该 provider 的 `close_frame_emits_*` WS mock 测试 |
| 错误消息文本漂移 | 低 | 测试断言失败 | 用 `H::LABEL` 插值，逐字对照原文本；5 个 LABEL 与原前缀一致 |
| Close 终态判定遗漏 | 中 | 吞错/误报成功 | handler 的 Close 分支逐字搬迁，5 个 provider 各自的 `close_frame_emits_*` 测试覆盖 |
| borrow 检查失败（handler &mut in select） | 低 | 编译失败 | trait 的 `handle_message(&mut self)` 在 select! 的 `msg => { ... }` 块内调用，msg 已从 ws 解绑，无 borrow 冲突 |
| bytedance request headers 构造搬迁 | 低 | 建连失败 | build_connect_request 内逐字搬 headers 构造逻辑，测试覆盖 |

---

## 6. 关联

- 代码审查报告问题 B（本轮）
- spec `2026-06-21-tencent-asr-design.md`（腾讯 ASR 原始设计，含 Close 终态判定语义）
- spec P2-1 WS mock（`test_ws_server.rs`，5 份 session 各有 `close_frame_emits_*` 测试）

---

## 7. 成功标准

1. `cargo build -p octopus-asr-cloud`：0 error 0 warning
2. `cargo test -p octopus-asr-cloud`：全过（含 5 个 `close_frame_emits_*` + 句间 sep 测试）
3. `cargo clippy -p octopus-asr-cloud --all-targets`：0 warning
4. 新增 `WsSessionHandler` trait + `HandleOutcome` enum + `run_ws_session_loop` 骨架函数
5. 5 个 provider 各实现 `WsSessionHandler`，`run_xxx_session` 改为薄包装（≤15 行）
6. 5 份 session 总行数从 ~980 行降到 ~550 行（骨架 ~80 + 5 handler ~80 × 5 = 480，加包装 50）
7. 对外 API 不变（`provider::open` 签名、`CloudStreamHandle`、`open_cloud_session` 全不动）

---

## 8. 实施记录（review plan 回写，2026-08-04/05）

### 8.1 实际偏差

| 偏差点 | spec 原设计 | 实际实现 | 原因 |
|---|---|---|---|
| `build_init_message` 返回类型 | `Result<Message>` | `Result<Option<Message>>` | tencent 无独立 init 帧（鉴权在 URL），None 时 loop 跳过此步。这是实施时发现的 trait 设计缺陷——并非所有 provider 都有 init 帧 |
| baidu `sn`/`cuid` 关系 | 各自独立 UUID | 共用 handler 的 `self.cuid`（sn=cuid） | 原实现 `let cuid = sn.clone()`，两者必须相同。handler 构造时生成 cuid，build_connect_request 用 `self.cuid` 作 sn query 保持不变量 |
| bytedance 错误传播 | handle_message 返回 TerminalFailed | 同 spec，但 parse_server_frame/decompress/JSON parse 三处错误从原 `.context()?` 改为 `match → TerminalFailed(format!)` | 原 `?` 向上传播成 session 函数 Err，由 open() 的 `tx_for_err.send(Failed)` 补发；新设计 handle_message 不能 `?`（返回 HandleOutcome），改为显式 TerminalFailed。错误消息文本保持与原 `.context("xxx")` 一致 |
| tencent JSON parse 错误 | 原 `.context("tencent 响应 JSON 解析失败")?` | TerminalFailed("tencent 响应 JSON 解析失败: {e}") | 同上，错误消息对齐原 context 文本 |
| handler 不持有 endpoint | spec 示例 handler 持 endpoint 字段 | handler 不持 endpoint，endpoint 作为 `run_ws_session_loop` 第 3 参数传入 | 避免 `&handler.endpoint` + move handler 借用冲突。endpoint 只是 build_connect_request 的入参，无需持久化 |
| aliyun FunASR/Qwen 共享 LABEL | spec 未明说 | FunASR 用 `LABEL = "aliyun"`，Qwen 用 `LABEL = "qwen-asr"` | 原代码两份 session 的错误前缀就是这两个不同的字符串（"aliyun WS..." vs "qwen-asr WS..."），各自独立 LABEL 保持不变量 |

### 8.2 未合并项（留作后续）

- **`accumulate_display` 仍未跨 provider 统一**：baidu 已抽出，tencent/aliyun-FunASR/aliyun-Qwen 仍各自内联等价的"句间 sep 守卫"逻辑。原因：4 个 provider 的状态结构不同（Vec<String> / BTreeMap<i64,String> / String 累积），统一需要先统一状态结构，超出本次范围。各 provider 内部的 sep 守卫逻辑经测试覆盖（`funasr_partial_inserts_sep_between_sentences` 等），行为正确。

### 8.3 实际行数

| 文件 | 改前 | 改后 | 变化 |
|---|---:|---:|---:|
| `session_loop.rs`（新增） | 0 | 175 | +175 |
| `baidu_stream.rs` | 473 | 456 | -17（run 函数 -140，handler +123）|
| `tencent_stream.rs` | 508 | 464 | -44 |
| `bytedance_stream.rs` | 689 | 668 | -21 |
| `aliyun_stream.rs` | 751 | 772 | +21（两份 handler 共 +200，两份 run 函数 -180）|
| **合计** | **2421** | **2535** | **+114** |

**注**：总行数略增（+114），但**逐字重复代码消除**（5 份 select! 双向循环 + 4 路错误 match 共 ~150 行 → 0，收敛到 session_loop.rs 的 80 行）；新增的 handler 代码是各 provider 协议特定逻辑（非重复），可读性提升。

### 8.4 验证结果（2026-08-05）

- `cargo build -p octopus-asr-cloud`：✅ 0 error 0 warning
- `cargo test -p octopus-asr-cloud`：✅ 58 passed, 0 failed, 1 ignored（real-model）
- `cargo clippy -p octopus-asr-cloud --all-targets`：✅ 0 warning
- `cargo check -p octopus-desktop`：✅ 调用方零影响
- 5 个 provider 的 `close_frame_emits_*` WS mock 测试全过（行为不变的关键证据）
- bytedance G1/H1 空 text 回归 + aliyun FunASR partial sep 回归全过
