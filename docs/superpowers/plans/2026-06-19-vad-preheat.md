# 启动/录音性能优化 Implementation Plan：VAD session 缓存（①），lock-free 音频（③可选）

> 状态：**已实现**（commits `c15c159` + `569f94b` + `07a1503`，merge main `07a1503`）
> Spec：`docs/superpowers/specs/2026-06-19-vad-preheat-design.md`（v3）

**Goal:** VAD 的 ONNX Session 全局缓存，`SileroVad::new()` 廉价化，消除首次按快捷键的录音启动延迟；preheat 预加载 VAD。

**实际实现偏离原 plan 两点**（subagent-driven 实施中发现，已修正）：

1. **struct 字段 `Arc<Session>` → `Arc<Mutex<Session>>`**：原 plan 假定 `Session::run(&self)`，实施时验证 ort 源码（`session/mod.rs:212`）`run` 是 `&mut self`——`Arc<Session>` 编译失败（deref 只给 `&Session`）。改 `Arc<Mutex<Session>>`，`compute()` 里 `self.session.lock().unwrap()` 拿 `&mut Session`。`Session: Send + Sync` 断言通过（回退非因 Send/Sync）。
2. **新增持锁 get-or-insert 修复 TOCTOU**（commit `07a1503`）：原 plan 的 `get→drop lock→load→re-lock insert` 有 TOCTOU 窗口（并发 miss 重复加载 + 互相覆盖致 `Arc::ptr_eq` 失败）。code review 后改为整个 get-or-insert 在持 cache lock 期间完成，消除 race + 删掉为掩盖它而加的 TEST_GATE 测试 hack。

---

## File Structure（实际）

- `crates/asr/src/vad.rs` — `SileroVad.session: Arc<Mutex<Session>>`；`VAD_SESSIONS` 缓存（持锁 get-or-insert）；`compute` lock；2 单测；Send+Sync 断言。
- `crates/desktop/src/main.rs` — preheat 后台线程追加 VAD 预加载。
- `crates/desktop/src/coordinator.rs` — **不改**（零改动已验证）。

---

## Tasks（已完成）

### Task 1: vad.rs session 缓存 + Arc<Mutex<Session)>  ✅ commit c15c159

- [x] Step 1: `Session: Send + Sync` 静态断言（通过）
- [x] Step 2-3: import + `VAD_SESSIONS` static + `vad_sessions()` helper；struct `session: Arc<Mutex<Session>>`
- [x] Step 4: `new()` 缓存（命中 clone Arc + zeros；miss 加载 + insert）
- [x] Step 5: 单测 `same_path_shares_session`（`Arc::ptr_eq`）+ `compute_returns_probability_in_range`
- [x] Step 6: commit `c15c159`

### Task 2: main.rs preheat 预加载 VAD session  ✅ commit 569f94b

- [x] Step 1: preheat 后台线程闭包内（ASR switch_model 之后）追加 VAD `SileroVad::new` 预加载，失败降级 warn
- [x] Step 2: commit `569f94b`

### Task 2.5: 持锁 get-or-insert 修复 TOCTOU  ✅ commit 07a1503

（code review 后追加，非原 plan）
- [x] `new()` 改为持 cache lock 完成 get-or-insert（消除 TOCTOU + 重复加载）
- [x] 删除 TEST_GATE 测试 hack（持锁后并发 miss 也命中同一 Arc，`ptr_eq` 恒成立）
- [x] 3 次多线程 `cargo test` 无 flake
- [x] commit `07a1503`

### Task 3（可选，未实现）: ③ lock-free 音频 ring buffer

> 不做。无证据锁是瓶颈。①完成后若 profiling 显示热路径延迟再启动。

- [ ] 评估而非实现（条件未满足）

### Task 4: 验证  ✅

- [x] `cargo test -p octopus-asr`：42 passed, 6 ignored
- [x] `cargo check --workspace --all-targets`：clean
- [x] coordinator 零改动（`git diff af809c8..07a1503 -- coordinator.rs` 空）
- [ ] 手动：首次按快捷键录音启动延迟显著降低（待用户本地确认）

---

## 实施记录（subagent-driven）

- implementer：task1+2 DONE_WITH_CONCERNS（发现 `run &mut self`，走 Mutex 回退）
- spec review：✅ 回退方案逐项正确（lock 作用域、reset 不锁、无死锁、coordinator 零改动）
- code review：Approved with recommendations（TEST_GATE + TOCTOU 指向同根因）
- implementer 修：持锁 get-or-insert + 删 TEST_GATE → DONE，3 次测试无 flake
- 未做（非阻塞）：`.lock().unwrap()` poison 容错（`into_inner()`）、compute lock 作用域收窄、commit `c15c159` message 准确性（rebase 限制未 amend）
