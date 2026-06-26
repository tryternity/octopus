# ASR Pipeline 阶段3：server 迁移 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `octopus-server` 两条 ASR 路径（流式 `/ws/stream` + 批处理 `/transcribe`）从裸调/旧路径迁到 asr helper（`StreamingRunner` / `transcribe_batch`），消除手搓 VAD + 手拼 JSON，与 cli/desktop 三端统一。

**Architecture:** 新建 `server/src/pipeline.rs`——`WsStreamSession`（薄包 asr `StreamingRunner`：`new`/`feed`/`finish`/`reset`）+ `event_to_json`（`TranscriptEvent` 4 variant → `{type,text}` JSON）。`main.rs::handle_ws` 用 `WsStreamSession` 替裸 `StreamingSession` + 删 `detect_silence_gap_local`（手搓 VAD 已收编进 runner）+ 回推改 `event_to_json`；`main.rs::transcribe` 改走 `AsrEngineManager::transcribe_batch` + `PipelineConfig`。不接 cloud；polish/denoise 不进 server（spec §3.8/§3.6）。

**Tech Stack:** Rust / axum 0.8 ws / tokio / `octopus-asr`（`StreamingRunner`/`TranscriptEvent`/`StreamingEngine`/`StreamingSession`/`pipeline::transcribe_batch`/`PipelineConfig`）。

**Spec:** `docs/superpowers/specs/2026-06-25-asr-server-stage3-design.md`

> **对 spec §4.1 的微调（实施时确定，Task 4 回写 spec）：** `WsStreamSession::new` 签名由 spec 的 `new(engine: &str, correct)` 改为 `new(engine: Box<dyn StreamingEngine>, correct)`——解耦 `StreamingSession`（对齐 desktop `LocalPipelineEngine::from_session`「先构 session 再包」），且可注入 fake 单测。`handle_ws` 负责调 `StreamingSession::new(&engine)` 后装箱传入。

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/server/src/pipeline.rs`（**新建**） | WS↔`StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化（纯逻辑，可单测） | 新建：`WsStreamSession` + `event_to_json` + `#[cfg(test)] mod tests` |
| `crates/server/src/main.rs` | axum 路由 + WS/HTTP 胶水 | 加 `mod pipeline;`；`handle_ws` 迁移 + 删 `detect_silence_gap_local`；`transcribe` 改 `transcribe_batch` |

---

## Task 1: 新建 `server/src/pipeline.rs`（`WsStreamSession` + `event_to_json`，TDD）

**Files:**
- Create: `crates/server/src/pipeline.rs`
- Modify: `crates/server/src/main.rs`（加 `mod pipeline;`）
- Test: `crates/server/src/pipeline.rs`（`#[cfg(test)] mod tests`）

- [x] **Step 1: 在 `main.rs` 注册新模块**

在 `crates/server/src/main.rs` 顶部 `use` 区上方加一行模块声明：

```rust
mod pipeline;
```

放在文件第 1 行（`use axum::{...}` 之前）。

- [x] **Step 2: 写 `pipeline.rs` 骨架 + 失败测试（todo! 占位）**

创建 `crates/server/src/pipeline.rs`，内容如下（实现处用 `todo!()`，测试引用之 → 运行时 panic = RED）：

```rust
//! server 流式 pipeline：WS↔asr `StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化。
//!
//! 薄包 [`StreamingRunner`]（VAD 静音 + 标点 + accept/flush/finish + 纠错已收编）。
//! 不含 polish / denoise（总 spec §3.8/§3.6：留端，server 不依赖 llm/cpal）。

use anyhow::Result;
use octopus_asr::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

/// WS 流式会话：薄包 asr `StreamingRunner`。
pub struct WsStreamSession {
    runner: StreamingRunner,
}

impl WsStreamSession {
    /// 由已构造的流式引擎装箱传入（解耦 `StreamingSession`，便于测试注入 fake）。
    /// `correct` 来自 `app_config.asr_correct`（与批处理 `PipelineConfig.correct` 同源）。
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        todo!("Step 4 实现")
    }

    /// 喂一帧已降噪 16k 样本，返回本帧事件流（0..n 个 TranscriptEvent）。
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        todo!("Step 4 实现")
    }

    /// 收尾：runner.finish() → Final（追加句号 + 简繁归一）。
    pub fn finish(&mut self) -> TranscriptEvent {
        todo!("Step 4 实现")
    }

    /// 重置（会话间复用前调用）。
    pub fn reset(&mut self) {
        todo!("Step 4 实现")
    }
}

/// `TranscriptEvent` → server 私有 WS JSON（统一 `{type,text}`）。
///
/// `TranscriptEvent` 无 Serialize（仅 Debug/Clone），为不污染 asr crate
/// （总 spec §3.1：asr = 零件库 + 端做桥接），server 端 match 序列化。
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    todo!("Step 4 实现")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn event_to_json_all_variants() {
        assert_eq!(
            event_to_json(&TranscriptEvent::Partial("你好".into())),
            r#"{"type":"partial","text":"你好"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Committed("foo".into())),
            r#"{"type":"committed","text":"foo"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Final("end".into())),
            r#"{"type":"final","text":"end"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Error("boom".into())),
            r#"{"type":"error","text":"boom"}"#
        );
    }

    #[test]
    fn event_to_json_escapes_backslash_quote_newline() {
        // 输入：a"b\c（换行）d —— 先转 \ 再转 " 再转 \n，反斜杠成对。
        let ev = TranscriptEvent::Final("a\"b\\c\nd".into());
        assert_eq!(
            event_to_json(&ev),
            r#"{"type":"final","text":"a\"b\\c\nd"}"#
        );
    }

    /// 可编程 fake：第一次 accept 返 Some，之后 None；finish 返固定串。
    /// （server 私有测试设施；asr crate 内的 FakeStreamingEngine 非 pub，不能复用。）
    struct FakeEngine {
        next_accept: Mutex<Option<String>>,
        finish_text: String,
    }
    impl StreamingEngine for FakeEngine {
        fn accept_samples(&self, _samples: &[f32], _was_silent: bool) -> Result<Option<String>> {
            Ok(self.next_accept.lock().unwrap().take())
        }
        fn flush(&self, _insert_comma: bool) -> Result<Option<String>> {
            Ok(None)
        }
        fn finish(&self) -> Result<String> {
            Ok(self.finish_text.clone())
        }
        fn reset(&self) {}
    }

    #[test]
    fn ws_stream_session_feed_partial_then_empty_finish_final() {
        let engine = FakeEngine {
            next_accept: Mutex::new(Some("hi".into())),
            finish_text: "final".into(),
        };
        let mut s = WsStreamSession::new(Box::new(engine), false).unwrap();
        // 单帧 512 静音样本（32ms < 500ms 阈值），无论 VAD 是否存在都不触发 flush，
        // 只走 accept_samples → Partial（detect_silence_gap 在 vad=None 时返回 (false,false)）。
        assert_eq!(
            s.feed(&[0.0_f32; 512]),
            vec![TranscriptEvent::Partial("hi".into())]
        );
        // accept 已 take → 第二次 None → 空事件。
        assert!(s.feed(&[0.0_f32; 512]).is_empty());
        assert_eq!(s.finish(), TranscriptEvent::Final("final".into()));
    }
}
```

- [x] **Step 3: 跑测试验证 RED（todo! panic）**

Run: `cargo test -p octopus-server pipeline::tests 2>&1 | tail -30`
Expected: 编译通过，3 个测试中 `event_to_json_all_variants` / `event_to_json_escapes_backslash_quote_newline` / `ws_stream_session_feed_partial_then_empty_finish_final` 均 **FAIL**，报 `not yet implemented: Step 4 实现`（`todo!()` panic）。

- [x] **Step 4: 实现 `WsStreamSession` + `event_to_json`（替换 4 处 `todo!()`）**

用 Edit 把 `pipeline.rs` 中 4 处 `todo!("Step 4 实现")` 替换为真实实现。

`WsStreamSession::new` —— 替换为：
```rust
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new(engine, correct)?,
        })
    }
```

`WsStreamSession::feed` —— 替换为：
```rust
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        self.runner.push_samples(samples_16k)
    }
```

`WsStreamSession::finish` —— 替换为：
```rust
    pub fn finish(&mut self) -> TranscriptEvent {
        self.runner.finish()
    }
```

`WsStreamSession::reset` —— 替换为：
```rust
    pub fn reset(&mut self) {
        self.runner.reset()
    }
```

`event_to_json` —— 替换为：
```rust
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    let (ty, text) = match ev {
        TranscriptEvent::Partial(t) => ("partial", t),
        TranscriptEvent::Committed(t) => ("committed", t),
        TranscriptEvent::Final(t) => ("final", t),
        TranscriptEvent::Error(t) => ("error", t),
    };
    // 先转反斜杠，再转引号/换行，避免引号转义产生的反斜杠被二次转义。
    let escaped = text
        .replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n");
    format!(r#"{{"type":"{}","text":"{}"}}"#, ty, escaped)
}
```

- [x] **Step 5: 跑测试验证 GREEN**

Run: `cargo test -p octopus-server pipeline::tests 2>&1 | tail -15`
Expected: `3 passed; 0 failed`。

- [x] **Step 6: cargo check + clippy（零新 warning）**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，**无新 warning**（pipeline.rs 内 `WsStreamSession` 此时尚未被 main.rs 使用，可能报 `field runner never read` / dead_code——若出现，Task 2 接线后自动消失；此处记录 warning 数量，Task 2 后归零）。

- [x] **Step 7: Commit**

```bash
git add crates/server/src/pipeline.rs crates/server/src/main.rs
git commit -m "feat(server): WsStreamSession + event_to_json（阶段3 Task 1）

新建 server/src/pipeline.rs：WsStreamSession 薄包 asr StreamingRunner
（new(Box<dyn StreamingEngine>,correct)/feed(16k)/finish/reset）+
event_to_json（TranscriptEvent 4 variant → {type,text} JSON，含转义）。
3 单测绿。main.rs 注册 mod pipeline。"
```

---

## Task 2: `handle_ws` 迁移到 `WsStreamSession` + 删 `detect_silence_gap_local`

**Files:**
- Modify: `crates/server/src/main.rs`（`use` 区 + `handle_ws` L221-367 + 删 `detect_silence_gap_local` L175-219）

- [x] **Step 1: 加 `use` 导入**

在 `crates/server/src/main.rs` 的 `use` 区（L1-15 附近）加两行：

```rust
use octopus_asr::streaming_runner::TranscriptEvent;
use pipeline::{event_to_json, WsStreamSession};
```

（`use octopus_asr::engine::AsrEngineManager;` 之后即可。）

- [x] **Step 2: 删除 `detect_silence_gap_local` 整个函数**

删除 `crates/server/src/main.rs` 中 L175-219 的 `fn detect_silence_gap_local(...) -> bool { ... }` 整个函数（含前面的注释行 `// ── WebSocket ──` 保留，只删函数本身）。

- [x] **Step 3: 用新 `handle_ws` 替换旧实现**

把 `crates/server/src/main.rs` 中 `async fn handle_ws(...) { ... }`（L221-367）整个函数替换为：

```rust
async fn handle_ws(
    mut socket: axum::extract::ws::WebSocket,
    _engine_manager: Arc<AsrEngineManager>,
    engine: String,
    _language: String,
) {
    use futures_util::StreamExt;

    // Validate engine
    if octopus_asr::config::resolve_engine_category(&engine).is_none() {
        let _ = socket
            .send(Message::Text(
                event_to_json(&TranscriptEvent::Error(format!(
                    "Unknown engine '{}'",
                    engine
                )))
                .into(),
            ))
            .await;
        return;
    }

    let session = match octopus_asr::streaming_engine::StreamingSession::new(&engine) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "Failed to create streaming session: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    // correct 与批处理 PipelineConfig.correct 同源（app_config.asr_correct）。
    let correct = octopus_asr::config::load_app_config_cached().asr_correct;
    let mut stream = match WsStreamSession::new(Box::new(session), correct) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "VAD init: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // f32 PCM little-endian chunks
                let chunk: Vec<f32> = data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                if chunk.is_empty() {
                    continue;
                }
                for ev in stream.feed(&chunk) {
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                }
            }
            Ok(Message::Text(cmd)) => {
                if cmd == "flush" {
                    let ev = stream.finish();
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                    stream.reset();
                }
            }
            Ok(Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
}
```

行为对照（零差异）：输入协议不变（binary f32 PCM + `"flush"` text + Close）；静音 flush 由 `StreamingRunner` 内部处理（`PUNCTUATION_SILENCE_THRESHOLD = 0.5s`，与旧 `detect_silence_gap_local` 一致）；错误从手拼 `{error}` 改为 `{type:error}`。

- [x] **Step 4: cargo check + clippy（验证接线，Task 1 的 dead_code warning 应消失）**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，**零 warning**（`WsStreamSession`/`event_to_json` 已被 `handle_ws` 使用，dead_code 消除）。若 `Message`/`Query`/`State` 等已有 import 缺失，按编译器提示补齐。

- [x] **Step 5: Commit**

```bash
git add crates/server/src/main.rs
git commit -m "refactor(server): handle_ws 迁 WsStreamSession + 删 detect_silence_gap_local（阶段3 Task 2）

裸调 StreamingSession + 手搓 SileroVad/detect_silence_gap_local + 手拼
{text,final} → WsStreamSession（薄包 StreamingRunner，VAD 静音/标点内部
收编）。WS 回推改 event_to_json（{type:text}）。静音 flush 阈值 0.5s
一致，零行为差异。"
```

---

## Task 3: `transcribe` 改走 `transcribe_batch`

**Files:**
- Modify: `crates/server/src/main.rs:118-122`（`transcribe` 函数内的引擎调用）

- [x] **Step 1: 替换引擎调用**

在 `crates/server/src/main.rs` 的 `transcribe` 函数内，把这段（L121-122）：

```rust
    let text = state.engine_manager.switch_model(engine)
        .and_then(|_| state.engine_manager.transcribe(&samples, language));
```

替换为：

```rust
    let cfg = octopus_asr::pipeline::PipelineConfig::from_app_config(language);
    let text = state.engine_manager.switch_model(engine)
        .and_then(|_| state.engine_manager.transcribe_batch(&samples, &cfg));
```

说明：`switch_model(engine)` 保留（`transcribe_batch` 用 active engine，需先切）；`language: &str` 直接传 `PipelineConfig::from_app_config(language: &str)`；`transcribe_batch(&samples, &cfg) -> Result<String>` 与旧 `transcribe` 返回类型一致，`and_then` 链不变。`TranscribeResponse { text, duration_ms, rtf }` 格式不变。

- [x] **Step 2: cargo check + clippy**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，零 warning。若旧 `transcribe` 方法在 `AsrEngineManager` 上删除后无其他调用方，编译器会提示——本计划不删 `AsrEngineManager::transcribe`（可能仍有其他用途，留待后续清理）。

- [x] **Step 3: 跑 server 全部单测确认无回归**

Run: `cargo test -p octopus-server 2>&1 | tail -15`
Expected: Task 1 的 3 个测试仍 `3 passed; 0 failed`。

- [x] **Step 4: Commit**

```bash
git add crates/server/src/main.rs
git commit -m "refactor(server): transcribe 改走 transcribe_batch（阶段3 Task 3）

AsrEngineManager.transcribe → transcribe_batch + PipelineConfig::
from_app_config（对齐 cli：VAD 分段 + 纠错 + 简繁归一化）。
TranscribeResponse 格式不变。"
```

---

## Task 4: workspace 验证 + 文档同步 + e2e 交付

**Files:**
- Verify: 全 workspace
- Modify: `docs/superpowers/specs/2026-06-25-asr-server-stage3-design.md`（§4.1 签名回写）、`docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`（横幅阶段3）、`docs/architecture.md`（server crate 描述）

- [x] **Step 1: 全 workspace 编译 + 测试 + clippy**

Run:
```bash
cargo test --workspace --lib 2>&1 | tail -20
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" | head
```
Expected: workspace lib 测试全绿（含 Task 1 的 3 个 server 单测）；clippy 零新 warning（server crate 无 `unused`/`dead_code`）。

- [x] **Step 2: 回写 spec §4.1 签名微调**

在 `docs/superpowers/specs/2026-06-25-asr-server-stage3-design.md` §4.1，把 `WsStreamSession::new` 的签名说明由 `new(engine: &str, correct)` 改为 `new(engine: Box<dyn StreamingEngine>, correct)`，并在代码块与文字说明中体现「`handle_ws` 负责调 `StreamingSession::new(&engine)` 后装箱传入；解耦 + 可注入 fake 单测」。同步更新 §4.1 代码块与 §10 迁移映射表对应行。

- [x] **Step 3: 总 spec 横幅标注阶段3 已实施**

在 `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md` 横幅（L4-12 附近）追加一行：

```
> **阶段3 已实施（2026-06-25）**：server 两端点迁 asr helper——流式 `/ws/stream` 用 `WsStreamSession`（薄包 `StreamingRunner`），批处理 `/transcribe` 走 `transcribe_batch`。spec `2026-06-25-asr-server-stage3-design.md`，plan `2026-06-25-asr-server-stage3.md`。
```

并修订 §7「本次不迁 server」措辞：注明「本次」= 阶段1/2，阶段3 已补齐。

- [x] **Step 4: 同步 `architecture.md` server crate 描述**

在 `docs/architecture.md` 的 server crate 段落（或 crates 列表），把 server 描述从「单文件 main.rs，裸调 StreamingSession」更新为「`pipeline.rs`（WsStreamSession 薄包 StreamingRunner + event_to_json）+ `main.rs`（路由）；流式/批处理均走 asr helper」。

- [ ] **Step 5: e2e 手动回归清单（待用户本地，需 ASR 模型环境；代码/单测/文档已完成 2026-06-26）**

起服务并验证两条路径（需本地有 ASR 模型 + VAD 模型，参考 desktop e2e 环境）：

```bash
# 1. 起 server（终端 A）
cargo run -p octopus-server -- --port 3000

# 2. 流式 WS：发 16k f32 PCM，验回推 {type:text} 序列
#    （用项目既有 e2e 脚本或 wscat + 小 wav 转 raw f32；参考 desktop cloud e2e 的 WS 客户端写法）
#    预期：语音段 → {"type":"partial",...}；静音≥0.5s → {"type":"committed",...}；发 "flush" → {"type":"final",...}

# 3. 批处理 HTTP：POST /transcribe，验 transcribe_batch 结果
curl -s -X POST "http://localhost:3000/transcribe?engine=<engine>&language=zh" \
  --data-binary @<16k-wav-or-raw-pcm> | jq .
#    预期：{"text":"...","duration_ms":...,"rtf":...}，文本与旧 /transcribe 路径一致
```

若行为与旧路径有差异（尤其静音 flush 时机），记录并评估是否需调整（以 `StreamingRunner` 为准——desktop 已验证）。

- [x] **Step 6: Commit 文档 + 交付报告**

```bash
git add docs/superpowers/specs/2026-06-25-asr-server-stage3-design.md \
        docs/superpowers/specs/2026-06-23-asr-pipeline-design.md \
        docs/architecture.md
git commit -m "docs: 阶段3 server 迁移同步（spec §4.1 签名回写 + 总 spec 横幅 + architecture）"
```

交付报告确认：workspace 测试全绿、clippy 零 warning、e2e 通过（流式 `{type}` 序列 + 批处理 `transcribe_batch`）。

---

## Self-Review（plan 写完后自查）

**1. Spec coverage：**
- §2 范围（流式+批处理迁、不接 cloud、polish/denoise 不进）→ Task 1-3 全覆盖；边界由「不引入 cloud/polish/denoise 代码」保证 ✓
- §3 文件结构（pipeline.rs 新建 + main.rs 改）→ Task 1（pipeline.rs）+ Task 2/3（main.rs）✓
- §4 组件（WsStreamSession + event_to_json）→ Task 1 ✓
- §5 数据流（流式 + 批处理）→ Task 2（handle_ws）+ Task 3（transcribe）✓
- §6 接口契约（WS 输出 {type,text}、输入不变、批处理响应不变）→ Task 1 event_to_json + Task 2/3 保留格式 ✓
- §7 错误处理 → Task 2 handle_ws（建连失败回推 {type:error}、单帧 Error 继续）✓
- §8 删除项 → Task 2 Step 2（detect_silence_gap_local）✓
- §9 测试 → Task 1 单测（event_to_json + WsStreamSession）+ Task 4 e2e ✓
- §10 迁移映射 → Task 1-3 逐项 ✓
- §4.1 签名微调回写 → Task 4 Step 2 ✓

**2. Placeholder 扫描：** 无 TBD/TODO（`todo!()` 是 Task 1 TDD 的 RED 占位，Step 4 明确替换为实现，非遗留）。所有步骤含完整代码/命令 ✓

**3. Type consistency：** `WsStreamSession::new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self>` 在 Task 1（定义）与 Task 2（`WsStreamSession::new(Box::new(session), correct)`）一致；`feed(&[f32]) -> Vec<TranscriptEvent>` / `finish() -> TranscriptEvent` / `reset()` 一致；`event_to_json(&TranscriptEvent) -> String` 一致；`transcribe_batch(&samples, &cfg)` 与 asr `engine.rs:151` 签名一致 ✓
