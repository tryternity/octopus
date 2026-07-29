# 推理 unwrap 优雅降级 + osascript Terminal spawn_blocking plan

> **Status: ✅ 已完成**（2026-07-29，分支 `daily_bugfix_0729`）
>
> **Spec**: [`2026-07-29-asr-unwrap-terminal-blocking.md`](../specs/2026-07-29-asr-unwrap-terminal-blocking.md)

## Phase A：推理路径 unwrap → 优雅降级（4 处）

### Task A.1-A.4: 4 处 unwrap

**Files:** `crates/asr-local/src/{whisper,paraformer,zipformer}.rs`

- [x] **Step 1: whisper.rs:345 `mel.as_slice().unwrap()` → `ok_or_else(anyhow!)?`**
- [x] **Step 2: paraformer.rs:202 `enc_slice.as_slice().unwrap()` → `ok_or_else(anyhow!)?`**
- [x] **Step 3: whisper.rs:467 `*tokens.last().unwrap()` → `last().copied().ok_or_else(anyhow!)?`**
- [x] **Step 4: zipformer.rs:1055 `*ans.last().unwrap()` → `if let Some(&last_byte) = ans.last()`**
- [x] **Step 5: `cargo test -p octopus-asr-local --lib` 全过（含 golden 测试）**

## Phase B：osascript Terminal spawn_blocking

### Task B.1: 调用点包 spawn_blocking

**Files:** `crates/desktop/src/action_bar_commands.rs:1896`

- [x] **Step 1: `launcher.spawn(...)` 改为 `spawn_blocking(move || launcher.spawn(...)).await??`**
- [x] **Step 2: `cargo build -p octopus-desktop --features embedded` + `cargo test -p octopus-desktop`**

## Phase C：全量验证 + 文档同步

- [x] **Step 1: `cargo test`（核心层 + asr-local + desktop）**
- [x] **Step 2: 更新 architecture.md ASR 引擎段（补 unwrap 优雅降级注记）**
- [x] **Step 3: review plan**
