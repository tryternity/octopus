# macOS ConcealedType 跳过记录 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在剪贴板 watcher 检测到 `org.nspasteboard.ConcealedType` 时静默跳过，防止密码管理器复制的敏感数据被入库 + FTS 索引 + 跨设备同步。

**Architecture:** 在 `crates/clipboard/src/watcher.rs::handle_clipboard_change` 函数开头（files/image/text 三分支之前）加 macOS 平台条件块，用 `handle.has(ContentFormat::Other(...))` 检测 ConcealedType，命中则 `return`。复用现有 clipboard-rs 0.3.5 的 `ContentFormat::Other` 任意类型检测能力（底层 `availableTypeFromArray`），零新依赖。

**Tech Stack:** Rust + clipboard-rs 0.3.5 + `#[cfg(target_os = "macos")]` 平台隔离

**Spec:** `docs/superpowers/specs/2026-08-05-macos-concealed-type-skip.md`

## Global Constraints

- 仅 macOS 生效（`#[cfg(target_os = "macos")]`），Windows/Linux 无 ConcealedType 概念
- ConcealedType 常量值：`"org.nspasteboard.ConcealedType"`（nspasteboard.org 社区约定）
- 检测点必须在 files/image/text 分支**之前**（避免误入 text 分支入库）
- 不动 octopus autotype 的 `suppress_next` 机制（跨平台保底 + macOS 双重保险）
- `handle_clipboard_change` 签名是 `pub fn handle_clipboard_change(handle: &crate::ClipboardHandle)`（**无返回值**，用 `return;` 而非 `return Ok(())`）

---

## File Structure

| 文件 | 责任 | 改动 |
|---|---|---|
| `crates/clipboard/src/watcher.rs` | ConcealedType 检测（5 行新增） | Modify: 在 `handle_clipboard_change` 函数开头加 `#[cfg(target_os = "macos")]` 块 |

无新文件。无 Cargo.toml 改动（clipboard-rs 0.3.5 已含 `ContentFormat::Other`）。

---

### Task 1: ConcealedType 检测实现

**Files:**
- Modify: `crates/clipboard/src/watcher.rs:80-87`（`handle_clipboard_change` 函数开头）

**Interfaces:**
- Consumes: `crate::ClipboardHandle::has(ContentFormat) -> bool`（handle.rs:131，已有）；`clipboard_rs::common::ContentFormat::Other(String)`（watcher.rs:81 已 `use`）
- Produces: ConcealedType 命中时早返回，下游 files/image/text 分支不执行

- [ ] **Step 1: 在 `handle_clipboard_change` 开头加 ConcealedType 检测**

打开 `crates/clipboard/src/watcher.rs`，定位第 80 行 `pub fn handle_clipboard_change(handle: &crate::ClipboardHandle) {`。在函数体的最开头（第 81 行 `use clipboard_rs::common::{ContentFormat, RustImage};` **之后**、第 86 行 `// 按优先级判断类型` 注释**之前**）插入：

```rust
    // ── ConcealedType 检测（macOS 密码管理器保护）──
    // 1Password / Bitwarden / iCloud Keychain 等复制密码时标记
    // org.nspasteboard.ConcealedType，明确告知消费方不要记录。
    // 静默跳过避免密码明文入库 + FTS5 索引 + 跨设备 sync 传播。
    // 仅 macOS——Windows/Linux 无此约定；octopus autotype 走 suppress_next
    // 双重保险（跨平台保底 + macOS ConcealedType 兜底）。
    #[cfg(target_os = "macos")]
    {
        const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
        if handle.has(ContentFormat::Other(CONCEALED_TYPE.to_string())) {
            return;
        }
    }
```

注意：`ContentFormat` 已在第 81 行 `use clipboard_rs::common::{ContentFormat, RustImage};` 导入，无需额外 import。

- [ ] **Step 2: 编译验证（含 macOS 平台 + 跨平台编译检查）**

Run:
```bash
cargo build --release -p octopus-clipboard
```
Expected: 编译成功，0 error 0 warning。

`#[cfg(target_os = "macos")]` 块在 macOS 上编译；如需验证 Windows/Linux 不受影响（CI 场景），可跑：
```bash
cargo check --target x86_64-pc-windows-gnu -p octopus-clipboard 2>&1 | head
```
Expected: ConcealedType 块被跳过（不编译），无 error。

- [ ] **Step 3: 跑 clipboard crate 现有测试（回归）**

Run:
```bash
cargo test -p octopus-clipboard --lib
```
Expected: 所有现有测试通过（无 ConcealedType 标记时行为不变）。24 个测试全过。

- [ ] **Step 4: Commit**

```bash
git add crates/clipboard/src/watcher.rs
git commit -m "feat(clipboard): macOS ConcealedType 跳过记录——防密码管理器敏感数据泄露

handle_clipboard_change 开头加 #[cfg(target_os=\"macos\")] 块检测
org.nspasteboard.ConcealedType（1Password/Bitwarden/iCloud Keychain 等
复制密码时标记），命中静默 return。clipboard-rs ContentFormat::Other
底层 availableTypeFromArray 支持任意 pasteboard 类型，零新依赖。

不动 autotype 的 suppress_next（跨平台保底 + macOS 双重保险）。"
```

---

### Task 2: 手动 e2e 验证（无自动化测试，spec §5 说明单测局限）

**Files:**
- 无文件改动（验证任务）

**Interfaces:**
- 无

**说明：** spec §5 已分析单测局限——clipboard-rs 的 `has()` 走真 NSPasteboard，clipboard crate 无 objc2 依赖无法在测试中设 ConcealedType 标记。集成测试需放 desktop crate（有 objc2），但跨 crate 测试 clipboard 的内部函数 `handle_clipboard_change` 不直接（需要 desktop 测试调 clipboard crate 的 pub fn，且 desktop 的 `copy_concealed` 走 suppress_next 会干扰）。权衡后采用手动 e2e 验证——改动仅 5 行安全检测，e2e 比 mock 测试更直接可靠。

- [ ] **Step 1: 启动 desktop 应用**

```bash
cargo run --release -p octopus-desktop --features embedded,cloud,vault,custom-protocol
```

- [ ] **Step 2: 用 1Password / Bitwarden / iCloud Keychain 复制一个密码**

操作：打开密码管理器，复制任意一条密码到剪贴板。

- [ ] **Step 3: 打开剪贴板浮窗（Cmd+Shift+D），确认密码**不**在列表中**

预期：剪贴板历史列表**没有**刚复制的密码条目。
对比测试：复制一段普通文本，确认**正常入库**（说明 ConcealedType 检测没有误伤正常复制）。

- [ ] **Step 4: 验证 octopus 自己的 autotype 仍正常**

操作：打开 vault，autotype 一条密码到某个输入框。
预期：密码正常粘贴到目标应用，且**不**进入剪贴板历史（suppress_next 双重保险生效）。

- [ ] **Step 5: 无需 commit（纯验证任务）**

如果验证失败（密码仍入库 / 正常文本被误跳过），回到 Task 1 检查实现。验证通过则进入 Task 3。

---

### Task 3: 文档同步

**Files:**
- Modify: `docs/architecture.md`（clipboard watcher 章节）

**Interfaces:**
- 无

- [ ] **Step 1: 在 architecture.md 的 clipboard watcher 章节补 ConcealedType 检测说明**

找到 architecture.md 描述 `watcher` / `on_clipboard_change` / `handle_clipboard_change` 的段落（约 line 178 附近，`| watcher | ClipboardWatcher：后台线程跑...` 行）。在该段的类型判断描述后补一句：

> **ConcealedType 检测（2026-08-05，macOS only）**：`handle_clipboard_change` 开头（files/image/text 分支前）用 `handle.has(ContentFormat::Other("org.nspasteboard.ConcealedType"))` 检测密码管理器标记，命中静默 return——防 1Password/Bitwarden/iCloud Keychain 复制的密码明文入库 + FTS 索引 + 跨设备 sync。仅 macOS（`#[cfg(target_os = "macos")]`）；octopus autotype 的 `suppress_next` 不变（跨平台保底 + macOS 双重保险）。详见 [spec](superpowers/specs/2026-08-05-macos-concealed-type-skip.md)。

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(architecture): 同步 ConcealedType 检测说明"
```

---

## Self-Review

**1. Spec coverage:**

| Spec 章节 | 覆盖 Task |
|---|---|
| §2.1 核心改动（handle_clipboard_change 开头检测） | Task 1 Step 1 |
| §2.2 为什么用 ContentFormat::Other | Task 1 代码注释 + Global Constraints |
| §2.3 常量位置（本地定义） | Task 1 Step 1 代码 |
| §2.4 autotype 兼容（不动 suppress_next） | Global Constraints + Task 2 Step 4 验证 |
| §3 影响面 | Task 2 e2e 验证 |
| §4 不变量（静默跳过 / 平台隔离 / 检测点位置） | Global Constraints + Task 1 代码 |
| §5 测试（单测局限 + e2e） | Task 2（手动 e2e，spec §5 已说明为何不写自动化测试） |
| §6 YAGNI 边界 | Global Constraints（明确不做占位条目/配置项/跨平台映射） |

无遗漏。

**2. Placeholder scan:** 无 TBD/TODO/「适当处理」等占位。Task 1 代码完整。Task 2 是验证任务无代码。

**3. Type consistency:** `ContentFormat::Other(String)` 签名与 clipboard-rs 0.3.5 一致（common.rs:91）；`handle.has(ContentFormat) -> bool` 与 handle.rs:131 一致；`handle_clipboard_change` 无返回值（用 `return;` 非 `return Ok(())`）已修正。

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-05-macos-concealed-type-skip.md`. 改动极小（watcher.rs 5 行 + architecture.md 1 段），建议 **Inline Execution**（本 session 直接执行，无需 subagent 调度）。
