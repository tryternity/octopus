# 剪贴板恢复竞态修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 desktop 审查一3——paste 经剪贴板粘贴后恢复原剪贴板的竞态（Cmd+V 后 sleep 50ms 不足，慢系统粘贴未落地就恢复→旧内容被粘进目标应用）。

**Architecture:** 纯延迟：把 `paste_via_clipboard` 中 Cmd+V 后、恢复剪贴板前的固定 sleep 50ms 提为命名常量 `PASTE_RESTORE_DELAY = 200ms`。无 probe、不可配（spec 判 YAGNI）。

**Tech Stack:** Rust + enigo（键盘模拟）+ tauri-plugin-clipboard-manager。

**Spec:** `docs/superpowers/specs/2026-06-21-clipboard-restore-race-design.md`

> **状态：✅ 已实现**（commit `e0f1420`：`PASTE_RESTORE_DELAY = 200ms`；GUI e2e 通过 2026-06-20；worktree 已清理合并 main）。下方 step 勾选标记实际完成进度。

> **测试策略说明（偏离 TDD 的理由）**：本改动无单元测试——`paste_via_clipboard` 依赖系统剪贴板 + enigo GUI 键盘交互 + 目标应用粘贴行为，无法离线隔离测试；为单次时序修复引入 mock 框架属 YAGNI。验证靠 `cargo check`（编译）+ 逻辑审查（确认仅 L119 改动、L89 不动）+ 手动 GUI e2e（Task 3 / followups plan 记录）。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/src/paste.rs` | `paste_via_clipboard` 粘贴+恢复时序 | **改**：加常量 + L119 sleep 改用常量 |
| `docs/superpowers/plans/2026-06-20-desktop-audit-followups.md` | desktop 审查后续待办 | **改**：§1 标题/状态→已实现、§2 补 e2e 项 |

---

## Task 1: paste.rs — PASTE_RESTORE_DELAY 常量 + L119 改用常量

**Files:**
- Modify: `crates/desktop/src/paste.rs`

> **关键陷阱**：L89（写剪贴板后）与 L119（Cmd+V 后）是两处**完全相同**的 `std::thread::sleep(Duration::from_millis(50));`。只改 L119，L89 不动。Step 1 的常量插入与 Step 2 的 Edit 都给出了精确锚点，务必按上下文定位。

- [x] **Step 1: 顶部新增常量（`use` 块之后、`PasteMethod` 之前）**

old：
```rust
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Paste method configuration
```

new：
```rust
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Cmd+V 后等待系统粘贴落地、再恢复原剪贴板的延迟。
/// 审查一3 竞态修复：原 50ms 在慢系统/高负载下不足——粘贴未落地就恢复，
/// 旧内容被粘进目标应用。200ms 为保守估值；跨平台无可靠「已落地」信号，
/// 故纯延迟、固定值（probe / 可配置均判 YAGNI）。
const PASTE_RESTORE_DELAY: Duration = Duration::from_millis(200);

/// Paste method configuration
```

- [x] **Step 2: L119 改用常量（带上下文区分 L89）**

old（含「Mod release」+「仅在不保留识别结果时恢复」上下文，唯一定位 L119）：
```rust
    enigo
        .key(mod_key, Direction::Release)
        .map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;

    std::thread::sleep(Duration::from_millis(50));

    // 仅在不保留识别结果时恢复原剪贴板。
```

new：
```rust
    enigo
        .key(mod_key, Direction::Release)
        .map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;

    std::thread::sleep(PASTE_RESTORE_DELAY);

    // 仅在不保留识别结果时恢复原剪贴板。
```

- [x] **Step 3: 编译验证**

Run: `cargo check -p octopus-desktop`
Expected: PASS，零 error 零新 warning（`Duration` 已 `use` 于 L7，常量直接可用）。

- [x] **Step 4: 逻辑审查（确认仅改 L119）**

Run: `git diff crates/desktop/src/paste.rs`
Expected: 仅两处变化——① 顶部新增 `PASTE_RESTORE_DELAY` 常量 + 注释；② Cmd+V 后那处 `sleep(Duration::from_millis(50))` → `sleep(PASTE_RESTORE_DELAY)`。**L89（写剪贴板后的 sleep）必须仍是 `Duration::from_millis(50)` 未变。**

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/paste.rs
git commit -m "fix(desktop): 剪贴板恢复竞态——Cmd+V 后 sleep 50ms→200ms（审查一3）"
```

---

## Task 2: followups plan 文档同步

**Files:**
- Modify: `docs/superpowers/plans/2026-06-20-desktop-audit-followups.md`

- [x] **Step 1: §1 标题 + 状态行改「已实现」**

标题 old：
```markdown
## 1. P2 — 一3 剪贴板恢复竞态（延后，低优先级）
```
标题 new：
```markdown
## 1. 一3 剪贴板恢复竞态（✅ 已实现）
```

状态行 old：
```markdown
**状态**：明确延后，未排期。当前不阻塞任何路径。
```
状态行 new：
```markdown
**状态**：✅ 已实现（worktree `clipboard-restore-race`，`PASTE_RESTORE_DELAY = 200ms`；spec `2026-06-21-clipboard-restore-race-design.md`）。行为正确性待 GUI e2e（见 §2）。
```

- [x] **Step 2: §2 GUI e2e 清单补「剪贴板恢复竞态」项**

表格末行 old：
```markdown
| 三1 | 云端 CloudClosing 期间点 Cancel / Discard | Cancel：不粘贴不写库；Discard：写库保历史不粘贴 |
```

表格末行 new（追加一3 行）：
```markdown
| 三1 | 云端 CloudClosing 期间点 Cancel / Discard | Cancel：不粘贴不写库；Discard：写库保历史不粘贴 |
| 一3 | `write_to_clipboard=false` + 慢系统/高负载 → 识别粘贴 | 目标应用粘进识别文本（非之前剪贴板内容） |
```

- [x] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-06-20-desktop-audit-followups.md
git commit -m "docs(plan): desktop-audit followups 同步一3 已实现 + e2e 项"
```

---

## Task 3: 整体验证 + 收尾

- [x] **Step 1: workspace 编译**

Run: `cargo check --workspace --all-targets`
Expected: PASS，零 warning 回归。

- [x] **Step 2: 确认 e2e 待本地（环境无 GUI）**

本 worktree / CI 无 GUI、无真实音频、无真实目标应用粘贴场景——剪贴板恢复竞态的行为正确性留 GUI e2e（已记入 followups §2）。**不在本 task 跑 e2e。**

- [x] **Step 3: 收尾（留 worktree，不合并）**

按 `superpowers:finishing-a-development-branch`：main 正用于 e2e 测试，本修复**留在 worktree 分支 `worktree-clipboard-restore-race`，暂不合并**。待 GUI e2e 通过后再 merge 回 main。

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §3.1 常量 + L119 改动 | Task 1 Step 1-2 |
| §3.2 不改（L89 / 守卫 / 其他路径） | Task 1 Step 4（审查确认 L89 未变） |
| §4 测试（无单测 + e2e 项补 followups） | Task 2 Step 2 + Task 3 Step 2 |
| §6 涉及文件（paste.rs + followups） | Task 1 + Task 2 |
