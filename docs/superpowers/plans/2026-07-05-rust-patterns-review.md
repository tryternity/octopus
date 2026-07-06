# Rust-Patterns 专项审查实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 逐任务实施。

**Goal:** 消除 rust-patterns 审查发现的 3 个 P1 + 3 个 P2 问题。

**Architecture:** P1 替换生产路径 unwrap() 为错误传播/恢复；P2 收窄 pub API 面。

**Tech Stack:** Rust（anyhow + thiserror + ndarray）

**Spec:** `docs/superpowers/specs/2026-07-05-rust-patterns-review-design.md`

## Global Constraints

- 工作目录：`/Users/wudarui/workspace/agent/octopus/.worktrees/rust-review`
- 每步改完后 `cargo clippy` + `cargo test` 验证
- 测试中的 unwrap() 不动

---

### Task 1: P1-1 Mutex lock().unwrap() 替换

**Files:**
- Modify: `crates/cli/src/main.rs`（10 处）
- Modify: `crates/download/src/core/downloader.rs`（1 处）

- [x] **Step 1: 替换 cli/main.rs 全部 lock().unwrap()**

`sed -i '' 's/lock().unwrap()/lock().unwrap_or_else(|e| e.into_inner())/g'`

- [x] **Step 2: 替换 downloader.rs lock().unwrap()**

- [x] **Step 3: clippy + test 验证**

Run: `cargo clippy --workspace --exclude octopus-desktop --all-targets && cargo test -p octopus-cli -p octopus-download`
Expected: 零警告，全绿

- [x] **Step 4: Commit**

`fix: Mutex lock().unwrap() → unwrap_or_else(|e| e.into_inner())`

---

### Task 2: P1-2 HeaderValue parse unwrap 替换

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs:449`

- [x] **Step 1: 替换为 ? 传播**

```rust
.map_err(|e| format!("secret_key 含非法 HTTP header 字符: {}", e))?
```

- [x] **Step 2: Commit**

---

### Task 3: P1-3 ndarray as_slice().unwrap() 替换

**Files:**
- Modify: `crates/asr-local/src/streaming_paraformer.rs`（3 处：607, 663, 757）

- [x] **Step 1: encoder 输出 2 处 → ok_or_else 传播**

```rust
.enc_slice.as_slice().ok_or_else(|| anyhow::anyhow!("encoder output non-contiguous (shape={:?})", enc_tensor.shape()))?
```

- [x] **Step 2: decoder cache 1 处 → ok_or_else 传播**

```rust
.as_slice_mut().ok_or_else(|| anyhow::anyhow!("decoder cache non-contiguous (idx={})", i))?
```

- [x] **Step 3: clippy + test 验证**

Run: `cargo clippy -p octopus-asr-local --all-targets && cargo test -p octopus-asr-local`
Expected: 零警告，39 passed

- [x] **Step 4: Commit**

---

### Task 4: P2-1 pub(crate) 收窄

**Files:**
- Modify: `crates/clipboard/src/store.rs`（3 个函数）
- Modify: `crates/download/src/`（8 个函数，跨 5 文件）
- Modify: `crates/llm/src/prompt.rs`（3 个函数）
- Modify: `crates/llm/src/lib.rs`（移除 build_system_prompt re-export）

- [x] **Step 1: clipboard 3 个零调用函数 → pub(crate) + #[allow(dead_code)]**

- [x] **Step 2: download 8 个零调用函数 → pub(crate)**

- [x] **Step 3: llm 3 个零调用函数 → pub(crate)，移除 lib.rs re-export**

- [x] **Step 4: clippy + test 验证**

Run: `cargo clippy --workspace --exclude octopus-desktop --all-targets && cargo test -p octopus-clipboard -p octopus-download -p octopus-llm`
Expected: 零警告，全绿

- [x] **Step 5: Commit**

---

### Task 5: P2-2/P2-3 评估（无需改动）

- [x] **P2-3**：paste.rs thread::sleep 已在 `spawn_blocking` 中（coordinator.rs:1259-1260），不阻塞 async runtime。无需改动。
- [x] **P2-2**：cloud_pipeline block_on 需架构级重构，暂不动。

---

### Task 6: 同步 architecture.md

- [x] **Step 1: 更新 line 349**

`as_slice().unwrap()` → `as_slice().ok_or_else(|| anyhow!(...))?`（两处）

---

## Self-Review

### Spec coverage

| Spec section | Task |
|---|---|
| §2 P1-1 | Task 1 |
| §2 P1-2 | Task 2 |
| §2 P1-3 | Task 3 |
| §3 P2-1 | Task 4 |
| §3 P2-2 | Task 5 |
| §3 P2-3 | Task 5 |

### Placeholder scan

无 TBD/TODO。所有步骤含完整代码。

### Type consistency

`unwrap_or_else(|e| e.into_inner())` — PoisonError → MutexGuard，类型一致。
`ok_or_else(|| anyhow!(...))?` — Option<&[f32]> → Result<&[f32]>，类型一致。
