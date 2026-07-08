# 流式 ASR 引擎复用（StreamingSessionManager）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给流式 ASR 引擎补一个对齐离线 `AsrEngineManager` 的 `StreamingSessionManager`，desktop 录音复用常驻引擎（reset 而非 new），消除每次录音秒级重载。

**Architecture:** 新增 `StreamingSessionManager`（`crates/asr-local/src/streaming_engine.rs`），按模型缓存 `Arc<dyn StreamingEngine>`，`active_session(spec, lang)` 懒加载取用 + `reset()` 复用。配套把 `StreamingRunner.engine` 从 `Box` 改 `Arc`（让 pipeline drop 时不销毁引擎）。仅 desktop 接入，server/cloud 不动。详见 spec `docs/superpowers/specs/2026-07-06-streaming-session-manager-design.md`。

**Tech Stack:** Rust，`parking_lot::Mutex`/`std::sync::{Arc,RwLock}`，ort 2.0.0-rc.12（`Session::run` 是 `&mut`，决定不能并发共享 Session），Tauri State 注入。

**Worktree:** `worktree-arch-fixes`（已含 ④①③ 架构修复，base `6ce7b36`）。

---

## File Structure

| 文件 | 责任 | 改动 |
|---|---|---|
| `crates/asr-local/src/streaming_engine.rs` | `StreamingSession` 定义 + 新增 `StreamingSessionManager` | 新增 manager（switch_model/set_active/active_session/active_name）+ tests |
| `crates/asr-local/src/streaming_runner.rs` | `StreamingEngine` trait + `StreamingRunner` | `engine` 字段 `Box→Arc`，`new`/`new_no_vad` 接 `Arc`，测试 helper 同步 |
| `crates/desktop/src/pipeline.rs` | `LocalPipelineEngine` 壳 | `from_session` 接 `Arc<dyn StreamingEngine>` |
| `crates/desktop/src/main.rs` | Tauri setup/State 注入 | 新增 `StreamingSessionManager` Arc + `app.manage` |
| `crates/desktop/src/coordinator.rs` | 录音流程 | 录音命令加 `streaming_manager` State；:811 改 `active_session + reset` |

**不改**：`crates/server`（per-connection new 不变）、`cloud_pipeline.rs`（独立路径）、`switch_asr_engine`（懒加载自动覆盖模型变更）、离线 `AsrEngineManager`。

---

## Task 1: StreamingSessionManager 核心（asr-local，TDD）

**Files:**
- Modify: `crates/asr-local/src/streaming_engine.rs`（顶部 import + 文件末尾追加 manager + tests）

- [x] **Step 1: 加 import**

`streaming_engine.rs` 顶部当前：
```rust
use anyhow::{Context, Result};
use parking_lot::Mutex;

use crate::sentence_separator;
```
改为：
```rust
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config;
use crate::sentence_separator;
use crate::streaming_runner::StreamingEngine;
```

- [x]**Step 2: 写失败测试（文件末尾追加 tests mod）**

```rust
#[cfg(test)]
mod manager_tests {
    use super::*;
    use parking_lot::Mutex;

    /// 计 reset 次数的 fake（impl StreamingEngine），绕过真实模型加载。
    struct FakeEngine {
        resets: Mutex<usize>,
    }
    impl FakeEngine {
        fn new() -> Self {
            Self { resets: Mutex::new(0) }
        }
        fn reset_count(&self) -> usize {
            *self.resets.lock()
        }
    }
    impl StreamingEngine for FakeEngine {
        fn accept_samples(&self, _: &[f32], _: bool, _: bool) -> Result<Option<String>> {
            Ok(None)
        }
        fn flush(&self, _: bool) -> Result<Option<String>> {
            Ok(None)
        }
        fn finish(&self) -> Result<String> {
            Ok(String::new())
        }
        fn reset(&self) {
            *self.resets.lock() += 1;
        }
    }

    #[test]
    fn active_session_reuses_cached_same_arc() {
        // set_active 注入 fake → 多次 active_session 同 spec 应返回同一 Arc（不重载）
        let mgr = StreamingSessionManager::new();
        let fake = Arc::new(FakeEngine::new()) as Arc<dyn StreamingEngine>;
        mgr.set_active_for_test("modelA", fake);
        let s1 = mgr.active_session("modelA", "zh").unwrap();
        let s2 = mgr.active_session("modelA", "zh").unwrap();
        assert!(Arc::ptr_eq(&s1, &s2), "复用应返回同一 Arc（不重载）");
    }

    #[test]
    fn active_session_empty_cache_unknown_model_errors() {
        // 空 manager + 不存在的模型 → switch_model 内部 StreamingSession::new 失败 → Err
        let mgr = StreamingSessionManager::new();
        let r = mgr.active_session("definitely-nonexistent-model-xyz", "zh");
        assert!(r.is_err(), "空缓存且模型不存在应返回 Err");
    }

    #[test]
    fn active_name_tracks_set_active() {
        let mgr = StreamingSessionManager::new();
        assert_eq!(mgr.active_name(), "");
        mgr.set_active_for_test(
            "modelB",
            Arc::new(FakeEngine::new()) as Arc<dyn StreamingEngine>,
        );
        assert_eq!(mgr.active_name(), "modelB");
    }

    #[test]
    fn reset_propagates_to_cached_engine() {
        // 复用前调 reset → 转发到缓存的引擎（验证 Arc<dyn StreamingEngine>::reset 可达）
        let mgr = StreamingSessionManager::new();
        let fake = Arc::new(FakeEngine::new());
        let fake_for_count = fake.clone(); // 共享计数
        mgr.set_active_for_test("modelC", fake as Arc<dyn StreamingEngine>);
        let s = mgr.active_session("modelC", "zh").unwrap();
        s.reset();
        s.reset();
        assert_eq!(fake_for_count.reset_count(), 2, "reset 应转发到底层引擎");
    }
}
```

- [x]**Step 3: 运行测试确认失败**

Run: `cargo test --manifest-path crates/asr-local/Cargo.toml StreamingSessionManager 2>&1 | tail -20`（或 `manager_tests`）
Expected: 编译失败 `cannot find type StreamingSessionManager` / `method set_active_for_test`。

- [x]**Step 4: 实现 StreamingSessionManager（文件末尾、tests mod 之前追加）**

```rust
// ── StreamingSessionManager：流式引擎复用（对齐离线 AsrEngineManager）──

/// 流式引擎复用管理器：按模型缓存 `Arc<dyn StreamingEngine>`，desktop 录音时
/// `active_session()` 取 Arc clone + `reset()` 复用，避免每次录音重载 ONNX Session。
///
/// 与离线 `AsrEngineManager` 的差异：流式 `StreamingSession` 有连接级状态
/// （punct_prefix/decoder_caches…），靠 **reset 复用** 而非并发共享（ort
/// `Session::run` 是 `&mut`，本就不能跨连接并发）。持 `Arc<dyn StreamingEngine>`
/// 与 `StreamingRunner` 一致，便于测试注入 fake。
pub struct StreamingSessionManager {
    cached: RwLock<HashMap<String, Arc<dyn StreamingEngine>>>,
    active_name: RwLock<String>,
}

impl Default for StreamingSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingSessionManager {
    pub fn new() -> Self {
        Self {
            cached: RwLock::new(HashMap::new()),
            active_name: RwLock::new(String::new()),
        }
    }

    /// 加载并切换 active 流式模型。spec 经 `parse_model_spec` 取裸名作缓存键；
    /// `language` 决定段间分隔符（英文空格 / 其他中文逗号）。
    /// 重复切同模型（active_name 已 == bare）短路返回 Ok。
    pub fn switch_model(&self, spec: &str, language: &str) -> Result<()> {
        let bare = config::parse_model_spec(spec).model_name().to_string();
        {
            let active = self.active_name.read().unwrap();
            if *active == bare {
                return Ok(());
            }
        }
        let session = StreamingSession::new(spec, language)?;
        self.set_active(&bare, Arc::new(session));
        Ok(())
    }

    /// 缓存入值 + 设 active_name（内部 helper）。
    fn set_active(&self, bare: &str, engine: Arc<dyn StreamingEngine>) {
        self.cached.write().unwrap().insert(bare.to_string(), engine);
        *self.active_name.write().unwrap() = bare.to_string();
    }

    /// 测试注入点：绕过真实模型加载，直接塞 fake + 设 active。
    #[cfg(test)]
    fn set_active_for_test(&self, bare: &str, engine: Arc<dyn StreamingEngine>) {
        self.set_active(bare, engine);
    }

    /// 取 active session 的 Arc clone。`active_name == spec` 且缓存命中 → 直接返回（复用，不重载）；
    /// 否则 `switch_model(spec, lang)` 懒加载后返回。模型变更（spec≠active）自动 switch 覆盖，
    /// 故 `switch_asr_engine` 命令无需主动联动本 manager。
    pub fn active_session(
        &self,
        spec: &str,
        language: &str,
    ) -> Result<Arc<dyn StreamingEngine>> {
        let bare = config::parse_model_spec(spec).model_name().to_string();
        {
            let active = self.active_name.read().unwrap();
            if *active == bare {
                if let Some(e) = self.cached.read().unwrap().get(&bare) {
                    return Ok(e.clone());
                }
            }
        }
        self.switch_model(spec, language)?;
        self.cached
            .read()
            .unwrap()
            .get(&bare)
            .cloned()
            .with_context(|| format!("active_session: just switched '{}' but cache miss", bare))
    }

    pub fn active_name(&self) -> String {
        self.active_name.read().unwrap().clone()
    }
}
```

- [x]**Step 5: 运行测试确认通过**

Run: `cargo test --manifest-path crates/asr-local/Cargo.toml manager_tests 2>&1 | tail -20`
Expected: 4 tests PASS。

- [x]**Step 6: clippy**

Run: `cargo clippy --manifest-path crates/asr-local/Cargo.toml --lib 2>&1 | tail -10`
Expected: 无 warning。

- [x]**Step 7: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes add crates/asr-local/src/streaming_engine.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes commit -m "feat(asr): 新增 StreamingSessionManager 流式引擎复用

对齐离线 AsrEngineManager：按模型缓存 Arc<dyn StreamingEngine>，
active_session(spec,lang) 懒加载 + reset 复用，避免每次录音重载 Session。
持 trait object 便于测试注入 fake；4 单测覆盖复用/切换/未命中/reset 转发。"
```

---

## Task 2: StreamingRunner Box→Arc + LocalPipelineEngine::from_session 接 Arc

> 跨 asr-local + desktop 的原子改动（保持 workspace 可编译）。`Box→Arc` 让 pipeline drop 时仅释放 Arc clone、manager 原 Arc 仍持有 → 引擎不销毁。

**Files:**
- Modify: `crates/asr-local/src/streaming_runner.rs:174,191,209,379-385,437`
- Modify: `crates/desktop/src/pipeline.rs:141-148`

- [x]**Step 1: 改 StreamingRunner.engine 字段 + 构造签名**

`streaming_runner.rs:174` 字段：
```rust
    engine: Box<dyn StreamingEngine>,
```
→
```rust
    engine: Arc<dyn StreamingEngine>,
```

`streaming_runner.rs:191` `new`：
```rust
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
```
→
```rust
    pub fn new(engine: Arc<dyn StreamingEngine>, correct: bool) -> Result<Self> {
```

`streaming_runner.rs:209` `new_no_vad`：
```rust
    pub fn new_no_vad(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
```
→
```rust
    pub fn new_no_vad(engine: Arc<dyn StreamingEngine>, correct: bool) -> Result<Self> {
```

文件顶部 import 已有 `use anyhow::Result;`，加 `use std::sync::Arc;`（若未有）。

- [x]**Step 2: 改 streaming_runner.rs 测试 helper 的构造**

`streaming_runner.rs:379-385` `runner()`：
```rust
    fn runner(fake: FakeStreamingEngine) -> StreamingRunner {
        let mut r = StreamingRunner::new(Box::new(fake), false).unwrap();
```
→
```rust
    fn runner(fake: FakeStreamingEngine) -> StreamingRunner {
        let mut r = StreamingRunner::new(Arc::new(fake), false).unwrap();
```

`streaming_runner.rs:437`（`push_samples_gates_silence_until_speech_when_vad_present` 内）：
```rust
        let mut r = StreamingRunner::new(
            Box::new(FakeStreamingEngine::new(
```
→
```rust
        let mut r = StreamingRunner::new(
            Arc::new(FakeStreamingEngine::new(
```

- [x]**Step 3: 改 LocalPipelineEngine::from_session 接 Arc**

`crates/desktop/src/pipeline.rs:141-148`：
```rust
pub struct LocalPipelineEngine(StreamingRunner);

impl LocalPipelineEngine {
    /// 构造 local 引擎，包已创建的 `StreamingSession`（保留 coordinator 的引擎降级逻辑，见 Step 1.4 ④）。
    /// 内部构造 `StreamingRunner`（含 VAD 预热，2a/2b）。
    pub fn from_session(session: StreamingSession, correct: bool) -> anyhow::Result<Self> {
        Ok(Self(StreamingRunner::new(Box::new(session), correct)?))
    }
}
```
→
```rust
pub struct LocalPipelineEngine(StreamingRunner);

impl LocalPipelineEngine {
    /// 构造 local 引擎，包已取用的流式引擎 Arc（来自 StreamingSessionManager，
    /// 录音结束 pipeline drop 仅释放此 Arc clone，manager 原 Arc 仍持有 → 引擎不销毁、下次复用）。
    /// 内部构造 `StreamingRunner`（含 VAD 预热，2a/2b）。
    pub fn from_session(engine: Arc<dyn StreamingEngine>, correct: bool) -> anyhow::Result<Self> {
        Ok(Self(StreamingRunner::new(engine, correct)?))
    }
}
```

`pipeline.rs` 顶部 import 补（若未有）：
```rust
use std::sync::Arc;
use octopus_asr_local::streaming_runner::StreamingEngine;
```
（确认 `StreamingRunner` 现有 import 路径，沿用。）

- [x]**Step 4: 编译 + 测试**

Run: `cargo test --manifest-path crates/asr-local/Cargo.toml 2>&1 | tail -15`
Expected: 全绿（含 streaming_runner 既有 11 测试 + manager_tests 4 测试）。

Run: `cargo build --manifest-path crates/desktop/Cargo.toml 2>&1 | tail -15`
Expected: **编译失败在 coordinator.rs:854**（`from_session(streaming_engine, false)` 类型不匹配——`streaming_engine` 现是 owned `StreamingSession`，而 `from_session` 改接 `Arc<dyn StreamingEngine>`）。这是预期的，Task 3 修复。

- [x]**Step 5: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes add crates/asr-local/src/streaming_runner.rs crates/desktop/src/pipeline.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes commit -m "refactor(asr,desktop): StreamingRunner.engine Box→Arc，from_session 接 Arc

让 StreamingSessionManager 与 StreamingPipeline 可同时持有同一引擎 Arc：
录音结束 pipeline drop 仅释放 Arc clone，manager 原 Arc 仍持有 → 引擎不销毁。
asr-local 测试 helper 同步 Box→Arc。desktop coordinator 接入留 Task 3。"
```

---

## Task 3: desktop main 注入 StreamingSessionManager + coordinator 改用 active_session

**Files:**
- Modify: `crates/desktop/src/main.rs`（`engine_manager` 注入处 ~355/415 旁）
- Modify: `crates/desktop/src/coordinator.rs`（录音命令签名 + :811 创建块）

- [x]**Step 1: main.rs 注入 StreamingSessionManager**

定位 `main.rs:415` 附近 `app.manage(engine_manager.clone());`，在其**下方**追加流式 manager 注入：

```rust
            // 暴露 engine_manager 为 State（审查 三2）：switch_asr_engine / set_config 切引擎时
            // 后台 switch_model 预热需要它。DispatchEngine 持有的是 clone，此处再 clone 托管。
            app.manage(engine_manager.clone());

            // 流式引擎复用 manager（②）：desktop 录音 reset() 复用常驻 StreamingSession，
            // 避免每次录音重载 ONNX Session。对齐离线 engine_manager 的注入方式。
            let streaming_manager = Arc::new(
                octopus_asr_local::streaming_engine::StreamingSessionManager::new(),
            );
            app.manage(streaming_manager);
```

（确认 `use std::sync::Arc;` 已在 main.rs 顶部——`engine_manager` 用了 `Arc::new`，已有。）

- [x]**Step 2: coordinator 录音命令加 streaming_manager State 参数**

定位 `coordinator.rs:811` 所在的录音命令函数（`start_recording` 或同名 tauri command）。在其参数列表加（对齐 `switch_asr_engine` 取 `engine_manager: State<'_, Arc<AsrEngineManager>>` 的写法，见 `runtime_config.rs:300-304`）：

```rust
    streaming_manager: State<'_, std::sync::Arc<octopus_asr_local::streaming_engine::StreamingSessionManager>>,
```

并在 `crates/desktop/src/main.rs` 的 invoke_handler 注册处（~188 `runtime_config::switch_asr_engine` 附近）确认该录音命令已注册（它本就注册，参数由 Tauri 自动从 State 注入，无需改注册）。

- [x]**Step 3: 改 coordinator:811 创建块为 active_session + reset**

当前（`coordinator.rs:807-829`）：
```rust
    if use_streaming {
        const FALLBACK_STREAMING_SPEC: &str = "local:zipformer:zipformer-small-ctc";
        let streaming_engine = match StreamingSession::new(&config.asr_engine, &config.language) {
            Ok(session) => session,
            Err(e) => {
                warn!(
                    "StreamingSession '{}' 创建失败 ({}), 降级到默认引擎 '{}'",
                    config.asr_engine, e, FALLBACK_STREAMING_SPEC
                );
                match StreamingSession::new(FALLBACK_STREAMING_SPEC, &config.language) {
                    Ok(session) => session,
                    Err(e2) => {
                        error!("默认引擎 StreamingSession 也失败: {}", e2);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                        return;
                    }
                }
            }
        };
```

改为（`active_session` 懒加载 + `reset()` 复用；保留 fallback 链）：
```rust
    if use_streaming {
        // 流式引擎复用（②）：从 StreamingSessionManager 取常驻引擎 Arc + reset 清状态，
        // 不再每次录音 StreamingSession::new 重载 Session。模型变更由 active_session 懒加载覆盖。
        const FALLBACK_STREAMING_SPEC: &str = "local:zipformer:zipformer-small-ctc";
        let streaming_engine = match streaming_manager
            .active_session(&config.asr_engine, &config.language)
        {
            Ok(arc) => {
                arc.reset();
                arc
            }
            Err(e) => {
                warn!(
                    "流式引擎 '{}' 取用失败 ({}), 降级到默认引擎 '{}'",
                    config.asr_engine, e, FALLBACK_STREAMING_SPEC
                );
                match streaming_manager
                    .active_session(FALLBACK_STREAMING_SPEC, &config.language)
                {
                    Ok(arc) => {
                        arc.reset();
                        arc
                    }
                    Err(e2) => {
                        error!("默认流式引擎也失败: {}", e2);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                        return;
                    }
                }
            }
        };
```

`streaming_engine` 类型现在是 `Arc<dyn StreamingEngine>`。下游 :854 `LocalPipelineEngine::from_session(streaming_engine, false)` 已接 `Arc<dyn StreamingEngine>`（Task 2），无需再改。

- [x]**Step 4: 清理 coordinator 未用 import**

`coordinator.rs:10` `use octopus_asr_local::streaming_engine::StreamingSession;`——若 Task 3 后 `StreamingSession` 不再被 coordinator 直接引用（改走 manager），改为：
```rust
use octopus_asr_local::streaming_engine::StreamingSessionManager;
```
（若 coordinator 他处仍用 `StreamingSession`，保留并追加 `StreamingSessionManager`。编译器会提示。）

- [x]**Step 5: 编译**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml 2>&1 | tail -20`
Expected: 编译通过至 `tauri::generate_context!()`（`main.rs` 末尾，缺 `dist/` 是预存环境问题，非本次引入——与 ④①③ 验证时一致）。

- [x]**Step 6: clippy（desktop lib 部分）**

Run: `cargo clippy --manifest-path crates/desktop/Cargo.toml 2>&1 | grep -E "warning|error" | head -20`
Expected: 无本次引入的 warning（忽略 dist 相关）。

- [x]**Step 7: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes add crates/desktop/src/main.rs crates/desktop/src/coordinator.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes commit -m "feat(desktop): 录音接入 StreamingSessionManager 复用引擎

main 注入 StreamingSessionManager State；coordinator 录音改走
manager.active_session() + reset()，不再每次 StreamingSession::new 重载。
保留 FALLBACK_STREAMING_SPEC 降级链。模型变更由 active_session 懒加载覆盖，
switch_asr_engine 无需主动联动。"
```

---

## Task 4: 验证 + 文档同步

**Files:**
- Verify: 全量测试 + reset 完整性核对
- Modify: `docs/asr_archiveture_opt.md`（§4 加 StreamingSessionManager）

- [x]**Step 1: asr-local 全量测试**

Run: `cargo test --manifest-path crates/asr-local/Cargo.toml 2>&1 | tail -15`
Expected: 全绿（manager_tests 4 + streaming_runner 11 + 既有约 95 测试）。

- [x]**Step 2: 核对 Zipformer reset 完整性（spec §10 待确认项）**

已 read-only 核实（无需改代码）：
- `StreamingZipformer::reset`（`streaming_zipformer.rs:252-264`）：清 `sample_buffer`/`history_samples`/`token_ids`/`prev_id` + 所有 `states` 归零。✓
- `StreamingZipformerTransducer::reset`（`:712-727`）：清 `sample_buffer`/`history_samples`/`emitted_ids` + `token_buf` 重置为 `[-1,…,-1,0]` + `states` 归零。✓

结论：三种流式引擎 reset 均干净，复用无状态泄漏。无需补全。

- [x]**Step 3: server 回归（确认未受影响）**

Run: `cargo test --manifest-path crates/server/Cargo.toml 2>&1 | tail -10`
Expected: 4 测试绿（注意 `ws_stream_session` 预存债 `pipeline-ws-stream-preexisting-fail`，若它挂起是 main 041e678 预存，非本次引入——勿动）。

- [x]**Step 4: workspace 编译**

Run: `cargo build --workspace --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes/Cargo.toml 2>&1 | tail -10`
Expected: 编译通过至 desktop `generate_context!()`（dist 缺失，预存）。

- [x]**Step 5: 文档同步**

在 `docs/asr_archiveture_opt.md` §4.1（`AsrEngineManager` 小节）后追加 §4.1b 或并入 §4.3 客户端宿主段落：

```markdown
### 4.1b 流式引擎复用（StreamingSessionManager）

流式 `StreamingSession` 每次录音曾 `new` 重载 encoder+decoder 两个 ONNX Session（秒级）。
现引入 `StreamingSessionManager`（对齐离线 `AsrEngineManager`），按模型缓存
`Arc<dyn StreamingEngine>`，desktop 录音时 `active_session(spec, lang)` 懒加载取用 +
`reset()` 复用，录音结束 pipeline drop 仅释放 Arc clone、manager 保留引擎 → 不再重载。

关键约束：ort `Session::run` 是 `&mut`，Session 不能跨连接并发共享；流式
`StreamingSession` 又有连接级状态，故靠 **reset 复用**（非并发共享）。配套把
`StreamingRunner.engine` 由 `Box` 改 `Arc` 让「drop 不销毁」成立。仅 desktop 接入；
server（每连接独立状态）与 cloud（独立路径）不受影响。
```

- [x]**Step 6: 提交文档**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes add docs/asr_archiveture_opt.md
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/arch-fixes commit -m "docs(asr): §4.1b 记录 StreamingSessionManager 流式引擎复用"
```

- [x]**Step 7: desktop e2e 手测（用户执行）**

构建 desktop（需先 `npm run build` 生成 dist，在 `crates/desktop/` 目录），录音两次：
- 首次：manager 未命中 → `switch_model` 加载（秒级，一次性）。
- 第二次：`active_session` 命中缓存 + `reset()`（毫秒级）→ **启动延迟应明显下降**。
- 切换 ASR 模型后再录音：`active_session` 自动 switch 新模型，旧模型留缓存。

确认无状态泄漏（第二次录音不含第一次的文本残留）。

---

## 验收清单

- [x]`cargo test -p octopus-asr-local` 全绿（+4 manager 测试）
- [x]`cargo test -p octopus-server` 绿（预存 ws_stream 债除外）
- [x]`cargo build --workspace` 过（desktop dist 预存缺失除外）
- [x]clippy 无本次引入 warning
- [x]desktop e2e：连录两次，第二次启动延迟大降，无状态泄漏
- [x]文档 `asr_archiveture_opt.md` §4.1b 已同步

## 不在本次（YAGNI）

| 项 | 原因 |
|---|---|
| 动静字段拆 Static/Dynamic struct | ort `&mut` 下无并发收益，纯整洁度 |
| server 多实例池化 | server 是桌面端辅助，非大并发 |
| `switch_asr_engine` 主动联动 manager | `active_session` 懒加载已覆盖模型变更 |
| ~~manager 缓存上限（max_cache）~~ | **2026-07-09 审查 Q3 已加 max_cache=2**（见「实施记录 → 后续审查修复」），原「不需要」假设被多引擎切换场景推翻 |

---

## 实施记录

### 后续审查修复（2026-07-09，commit d6c2d71）

后端审查对 `StreamingSessionManager` 提 2 真 bug，在原 4 Task 之上增量修复（spec §10 / YAGNI 表已同步修订）：

- **max_cache=2 驱逐**（审查 Q3）：原「不设上限」假设未覆盖用户配置多流式引擎反复切换场景（每个 Session 数百 MB，无上限致 OOM）。`set_active` 入缓存前淘汰非 active + `probe(Unload)`，对齐离线 `AsrEngineManager`。+2 单测：`set_active_evicts_when_over_capacity_keeps_active` / `set_active_no_evict_when_reinserting_existing_key`。
- **model_probe 接入**（审查 Q4）：`switch_model` 加 `probe(Before/After)`（id=`asr:<bare>`），状态页统计流式引擎内存（与离线 `load_engine_into_cache` 对称）。

> 实施偏差 3「未设 max_cache」据此修订：当时（2026-07-06）确实未设，2026-07-09 审查后补。

### 已完成并合 main（2026-07-06）

**4 Task 全绿，commit `d2964f0..237df45`（arch-fixes worktree，已 ff-merge main + push origin）。** e2e 通过（连录两次第二次启动延迟大降、无状态泄漏）。asr_archiveture_opt.md §4.1b 已同步（commit dd0c60d）。

#### 实施偏差

1. **Zipformer reset 完整性（spec §10 待确认项）**：已 read-only 核实三种流式引擎 reset 均干净——`StreamingZipformer::reset`（streaming_zipformer.rs:252-264）清 `sample_buffer`/`history_samples`/`token_ids`/`prev_id` + `states` 归零；`StreamingZipformerTransducer::reset`（:712-727）清 `sample_buffer`/`history_samples`/`emitted_ids` + `token_buf` 重置 + `states` 归零。无需补全。
2. **`switch_asr_engine` 联动**：未主动联动——`active_session(spec, lang)` 懒加载（spec≠active 自动 switch 覆盖）已处理模型变更，确认 spec §10 该项的结论为「不需要」。
3. **manager 缓存上限**：未设 max_cache（spec §10 该项结论为「不需要」），流式模型种类少。
4. **server WsStreamSession**：虽 server 不接入 manager，但 `StreamingRunner` Box→Arc（Task 2）连带要求 `WsStreamSession` 同步改 Arc（commit 2f59ffd），否则编译断。
5. **后续 drain 修复**：e2e 暴露 paraformer `raw_samples` drain 停滞 bug（与绝对帧索引 `fi*SHIFT` 不兼容），commit 237df45 移除 drain 已修，新增 fbank 持续增长回归测试。

#### arch-fixes worktree 已清

原 worktree `worktree-arch-fixes` 完成后已 `git worktree remove` + 删分支；本计划内 `git -C .../arch-fixes` 命令为历史记录（实际执行路径），不再有效。
