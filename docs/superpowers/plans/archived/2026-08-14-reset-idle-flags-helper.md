# reset_idle_flags() helper 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 提取 `reset_idle_flags()` helper 收口 coordinator 所有 static flag 清理，根治同型遗漏。

**Architecture:** 在 mod.rs（3 个 static atomics 定义处）新增 `pub(crate) fn reset_idle_flags()`，清理 INSTANT_MODE + TRANSLATION_ACTIVE + recording_mode。替换 4 个文件 8 处手动清理序列。删除旧的 `reset_mode_flags_on_start_failure`（session.rs，漏了 TRANSLATION_ACTIVE）。

**Tech Stack:** Rust, static AtomicBool/AtomicU8, octopus-desktop crate

## Global Constraints

- 工作目录：`/Users/wudarui/workspace/agent/octopus/.worktrees/bugfix_pr_0801`
- 编译命令：`cargo build -p octopus-desktop --features "cloud,embedded,vault"`
- 测试命令：`cargo test -p octopus-desktop --features "cloud,embedded,vault"`
- 不替换条件 swap 语义（mod.rs PasteDone handler `INSTANT_MODE.swap` + paste.rs do_paste `TRANSLATION_ACTIVE.swap`）
- 不涉及 local 变量（editing/edit_buffer/pending_prepare/pending_flush）

**Spec:** `docs/superpowers/specs/2026-08-14-reset-idle-flags-helper-design.md`

---

### Task 1: 新增 `reset_idle_flags()` + 单测（TDD）

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/mod.rs`（:95 附近，`set_recording_mode` 之后）
- Test: `crates/desktop/src/engine/coordinator/mod.rs`（内联 `#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `pub(crate) fn reset_idle_flags()` —— 清 INSTANT_MODE + TRANSLATION_ACTIVE + recording_mode 全部归零

- [x] **Step 1: 写失败测试**

在 mod.rs 的 `#[cfg(test)] mod tests` 末尾加：

```rust
    #[test]
    fn reset_idle_flags_clears_all_three_statics() {
        // 设置全部非默认值
        super::INSTANT_MODE.store(true, Ordering::Relaxed);
        super::TRANSLATION_ACTIVE.store(true, Ordering::Relaxed);
        super::set_recording_mode(2);
        // 清零
        super::reset_idle_flags();
        // 验证三个 flag 全归零
        assert!(!super::INSTANT_MODE.load(Ordering::Relaxed));
        assert!(!super::TRANSLATION_ACTIVE.load(Ordering::Relaxed));
        assert_eq!(super::recording_mode(), 0);
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop --features "cloud,embedded,vault" reset_idle_flags_clears_all_three_statics 2>&1 | tail -5`
Expected: 编译失败（`reset_idle_flags` 未定义）

- [x] **Step 3: 实现 `reset_idle_flags()`**

在 mod.rs `set_recording_mode` 函数之后（约 :97）加：

```rust
/// 清所有 stage→Idle 出口应复位的 static flag。根治「同型遗漏」——
/// 新增 flag 只需改这一处，所有出口自动覆盖。
///
/// 不适用于条件 swap 语义的出口（PasteDone handler 读 INSTANT_MODE 旧值决定 UI、
/// do_paste 读 TRANSLATION_ACTIVE 旧值决定是否翻译）——那些出口用自己的 swap。
pub(crate) fn reset_idle_flags() {
    INSTANT_MODE.store(false, Ordering::Relaxed);
    TRANSLATION_ACTIVE.store(false, Ordering::Relaxed);
    set_recording_mode(0);
}
```

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --features "cloud,embedded,vault" reset_idle_flags_clears_all_three_statics 2>&1 | tail -5`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/engine/coordinator/mod.rs
git commit -m "refactor: 新增 reset_idle_flags() helper + 单测"
```

---

### Task 2: 替换所有手动 flag 清理序列

**Files:**
- Modify: `crates/desktop/src/engine/coordinator/session.rs`（删 `reset_mode_flags_on_start_failure` + 5 处调用改 `reset_idle_flags`）
- Modify: `crates/desktop/src/engine/coordinator/lifecycle.rs`（4 处替换）
- Modify: `crates/desktop/src/engine/coordinator/cancel_discard.rs`（2 处替换）
- Modify: `crates/desktop/src/engine/coordinator/polish.rs`（1 处替换）

**Interfaces:**
- Consumes: `reset_idle_flags()` from Task 1

- [x] **Step 1: session.rs —— 删旧 helper + 5 处调用改新**

删除 `reset_mode_flags_on_start_failure` 函数定义（约 :48-51），5 处 `reset_mode_flags_on_start_failure()` 调用改为 `super::reset_idle_flags()`。

用 grep 确认所有调用点：
```bash
grep -n "reset_mode_flags_on_start_failure" crates/desktop/src/engine/coordinator/session.rs
```

每处替换 `reset_mode_flags_on_start_failure()` → `super::reset_idle_flags()`。

- [x] **Step 2: lifecycle.rs —— 4 处替换**

四处手动 3-flag 序列（INSTANT_MODE.swap + TRANSLATION_ACTIVE.store + set_recording_mode(0)）替换为 `reset_idle_flags()`：

1. finalize_after_stop 空文本（约 :427-433）
2. finalize_after_stop AgentBridge（约 :446-449）
3. finalize_cloud 空文本（约 :547-552）
4. finalize_cloud AgentBridge（约 :572-575）

注意：lifecycle.rs 已 import `INSTANT_MODE, TRANSLATION_ACTIVE, set_recording_mode`——替换后这些 import 可能变为 unused，需清理。同时需确保 `reset_idle_flags` 可通过 `use super::{..., reset_idle_flags}` 或直接 `super::reset_idle_flags()` 调用。

- [x] **Step 3: cancel_discard.rs —— 2 处替换**

handle_cancel（约 :86-92）+ handle_discard（约 :253-259）的手动 3-flag 序列替换为 `reset_idle_flags()`。

cancel_discard.rs 已 import `INSTANT_MODE, TRANSLATION_ACTIVE, set_recording_mode`——替换后清理 unused import。

- [x] **Step 4: polish.rs —— 1 处替换**

start_final_polish_or_paste 空文本（约 :36-41）的 `set_recording_mode(0)` + `super::TRANSLATION_ACTIVE.store(false, ...)` 替换为 `super::reset_idle_flags()`。

- [x] **Step 5: 编译 + 看 warning**

Run: `cargo build -p octopus-desktop --features "cloud,embedded,vault" 2>&1 | tail -15`
Expected: 0 error。如有 unused import warning，清理对应 import。

- [x] **Step 6: 跑全量测试**

Run: `cargo test -p octopus-desktop --features "cloud,embedded,vault" 2>&1 | grep -E "test result|FAILED" | tail -3`
Expected: 547 过 0 失败（546 + 1 新增 Task 1 测试）

- [x] **Step 7: grep 确认无残留手动序列**

Run: `grep -rn "INSTANT_MODE.swap(false\|INSTANT_MODE.store(false" crates/desktop/src/engine/coordinator/ | grep -v "test\|mod.rs:73\|reset_idle_flags"`
Expected: 只剩 PasteDone handler（mod.rs 条件 swap）和 mod.rs 定义处，无手动清理残留。

Run: `grep -rn "reset_mode_flags_on_start_failure" crates/desktop/src/`
Expected: 零结果（旧 helper 已删）。

- [x] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: 替换 8 处手动 flag 清理为 reset_idle_flags()，删除旧 reset_mode_flags_on_start_failure"
```

---

### Task 3: 文档同步

**Files:**
- Modify: `docs/architecture.md`（coordinator 描述补 reset_idle_flags）
- Modify: `docs/superpowers/specs/archived/2026-08-03-full-audit-bugfix.md`（§53 记录）

- [x] **Step 1: architecture.md 补 helper 说明**

在 coordinator 状态机描述中补：`reset_idle_flags()` 统一收口 stage→Idle 出口的 INSTANT_MODE + TRANSLATION_ACTIVE + recording_mode 清理，根治同型遗漏。

- [x] **Step 2: 审查 spec §53 记录**

在 audit-bugfix spec 末尾补 §53 记录本次重构。

- [x] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: reset_idle_flags helper 架构同步"
```
