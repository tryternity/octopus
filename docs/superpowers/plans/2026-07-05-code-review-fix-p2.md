# 代码审查修复 P2 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:executing-plans 或 superpowers:subagent-driven-development 逐任务实施。Steps 用 checkbox (`- [ ]`) 跟踪。

**Goal:** 清理死代码、死依赖、生产路径调试输出，修跨平台编译，清理 118 个 clippy lint。机械化操作为主，风险低。

**Architecture:** 删除确认无调用的代码，`eprintln!`/`console.log` 改 `log::debug!` 或删除，`cargo clippy --fix` 自动修复。

**Tech Stack:** Rust + Rust clippy + TypeScript。

## Global Constraints

- 删除死代码前必须确认全项目零调用（grep 验证）
- clippy fix 后必须编译 + 测试通过
- 前置依赖：P0 + P1 已完成

---

## Task G1: 删除死代码 + 死依赖

**Files:**
- Delete: `crates/infra/src/image_util.rs`（全文件零调用）
- Modify: `crates/infra/src/lib.rs`（删 `mod image_util;`）
- Modify: `crates/infra/Cargo.toml`（删 `image`/`webp` 依赖，如仅 image_util 用）
- Delete code: `crates/desktop/src/shortcut.rs:47-61` unregister_shortcut
- Delete code: `crates/desktop/src/screenshot_commands.rs:57-61` is_screenshot_active
- Delete code: `crates/desktop/src/screenshot_commands.rs:866-882` send_scroll
- Delete code: `crates/desktop/src/pin_window.rs:142-154` close_all_pin_windows
- Delete code: `crates/desktop/src/pipeline.rs:120-135` Pipeline trait（如 coordinator 未用）
- Delete code: `crates/download/src/core/verify.rs:48-51` if_range_value
- Delete code: `crates/download/src/core/downloader.rs:202-206` 不可达 416 分支
- Delete code: `crates/capx/src/capture.rs:140-170` capture_display_excluding_window
- Delete code: `crates/asr-cloud/src/bytedance_stream.rs:71` COMP_NONE + serialization 字段 + byte0
- Modify: `crates/dlp/Cargo.toml`（删 tempfile，如 P0 未删）
- Modify: `crates/llm/Cargo.toml`（删 serde_yaml dev-dep）

- [ ] **Step 1：验证 image_util 零调用**

```bash
rg "save_as_webp|save_as_png|save_as_jpeg|image_util" crates/ --glob '!crates/infra/src/image_util.rs'
```
Expected: 无命中（仅定义处）。

- [ ] **Step 2：删除 image_util.rs + lib.rs 声明 + Cargo.toml 依赖**

```bash
rm crates/infra/src/image_util.rs
```
编辑 `lib.rs` 删 `pub mod image_util;`（或 `mod image_util;`）。
编辑 `Cargo.toml`：确认 `image` 和 `webp` 是否仅被 image_util 使用（`rg "use image\|use webp\|image::\|webp::" crates/infra/src/`），如否则删除。

- [ ] **Step 3：验证并删除 desktop 死代码**

逐个验证零调用后删除（含 `#[allow(dead_code)]` 标注的）：
```bash
rg "unregister_shortcut" crates/desktop/src/ --glob '!*shortcut.rs'
rg "is_screenshot_active" crates/desktop/src/ --glob '!*screenshot_commands.rs'
rg "send_scroll" crates/desktop/src/ --glob '!*screenshot_commands.rs'
rg "close_all_pin_windows" crates/desktop/src/ --glob '!*pin_window.rs'
```

- [ ] **Step 4：验证并删除 download/capx/asr-cloud 死代码**

```bash
rg "if_range_value" crates/download/src/ --glob '!*verify.rs'
rg "capture_display_excluding_window" crates/capx/src/ --glob '!*capture.rs'
rg "COMP_NONE|\.serialization" crates/asr-cloud/src/ --glob '!*bytedance_stream.rs'
```

删除 downlownloader.rs:202-206 不可达 416 else 分支。

- [ ] **Step 5：删除死依赖**

`dlp/Cargo.toml` 删 `tempfile`（如 P0 未删）。`llm/Cargo.toml` 删 `serde_yaml`（dev-dep）。

- [ ] **Step 6：编译验证**

```bash
cargo build --workspace
```
Expected: 编译通过。如有遗漏的引用，编译器会报错，逐个修复。

- [ ] **Step 7：提交**

```bash
git add -A
git commit -m "refactor: 删除死代码与死依赖

- infra/image_util.rs 全文件（零调用）+ image/webp 依赖
- desktop: unregister_shortcut/is_screenshot_active/send_scroll/close_all_pin_windows/Pipeline trait
- download: if_range_value + 不可达 416 分支
- capx: capture_display_excluding_window
- asr-cloud: COMP_NONE/serialization/byte0
- dlp: tempfile 依赖；llm: serde_yaml dev-dep

fixes 共性5"
```

---

## Task G2: capx 跨平台编译修复（修 C14）

**Files:**
- Modify: `crates/capx/src/capture.rs:340-362`

- [ ] **Step 1：测试块加 cfg 门控**

`capture.rs:340` 的 `#[cfg(test)] mod tests` 改为 `#[cfg(all(test, target_os = "macos"))] mod tests`，或给每个调用 `bgra_to_rgba` 的测试加 `#[cfg(target_os = "macos")]`。

- [ ] **Step 2：验证非 macOS 编译（如有 Linux 环境）**

```bash
# macOS 上无法直接验证 Linux 编译，但可以检查 cfg 一致性
cargo test -p octopus-capx  # macOS 上应 PASS
```

- [ ] **Step 3：提交**

```bash
git add crates/capx/src/capture.rs
git commit -m "fix(capx): 测试块加 target_os=macos 门控，修非 macOS 编译失败

bgra_to_rgba 标了 cfg(macos) 但 tests 未门控，Linux/Windows CI 编译失败。

fixes C14"
```

---

## Task G3: 清理生产路径调试输出

**Files:**
- Modify: `crates/asr-local/src/whisper.rs`（8 处 eprintln! → log::debug!）
- Modify: `crates/asr-local/src/paraformer.rs:319-324`（eprintln! → log::debug!）
- Modify: `crates/desktop/src/screenshot_commands.rs`（eprintln! → log::debug!）
- Modify: `crates/asr-cloud/src/aliyun_stream.rs:198-201,442-445`（info! → debug!）
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx:155,162`（删 console.log）

- [ ] **Step 1：asr-local eprintln! → log::debug!**

```bash
rg "eprintln!" crates/asr-local/src/ -n
```
逐个改为 `log::debug!`（保留信息内容）。确认 asr-local 已有 `log` 依赖（`rg "log" crates/asr-local/Cargo.toml`）。

- [ ] **Step 2：desktop screenshot_commands eprintln! → log::debug!**

```bash
rg "eprintln!" crates/desktop/src/screenshot_commands.rs -n
```

- [ ] **Step 3：asr-cloud aliyun info! → debug!（热路径日志洪水）**

`aliyun_stream.rs:198-201` 和 `:442-445` 的 `log::info!` 改为 `log::debug!`。

- [ ] **Step 4：前端删 console.log**

`Result/index.tsx:155,162` 删除两行 `console.log`。
```bash
rg "console.log" crates/desktop/frontend/src/ -n
```
删除所有进生产的 `console.log`（保留 dev-only 的如有标注）。

- [ ] **Step 5：编译 + 测试**

```bash
cargo build -p octopus-asr-local -p octopus-desktop -p octopus-asr-cloud
cargo test -p octopus-asr-local -p octopus-asr-cloud
```

- [ ] **Step 6：提交**

```bash
git add -A
git commit -m "cleanup: 生产路径调试输出改 log::debug! / 删除

whisper 8处 eprintln、paraformer CMVN、desktop screenshot、aliyun 热路径 info!
前端 Result console.log。

fixes 共性4"
```

---

## Task G4: clippy 全量修复

**Files:**
- 各 crate（自动修复）

- [ ] **Step 1：自动修复可修的 lint**

```bash
cargo clippy --fix --workspace --allow-dirty --allow-staged 2>&1 | tail -20
```

- [ ] **Step 2：检查剩余 lint**

```bash
cargo clippy --workspace --all-targets 2>&1 | grep "^warning:" | head -30
```

- [ ] **Step 3：手动修复剩余 lint**

常见手动修复：
- `needless_range_loop`(19) → `enumerate()` 或直接索引
- `manual_is_multiple_of`(12) → `.is_multiple_of()`
- `too_many_arguments`(3) → 参数封装为 struct

- [ ] **Step 4：加 clippy gate 到各 lib.rs**

在各 crate 的 `lib.rs` 顶部加（如未有）：
```rust
#![warn(clippy::all)]
```

- [ ] **Step 5：编译 + 测试 + clippy 零警告验证**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | grep -c "^warning" 
```
Expected: clippy 0 warning（desktop 前端 dist 除外）。

- [ ] **Step 6：提交**

```bash
git add -A
git commit -m "cleanup: cargo clippy --fix 修复 118 个 lint + 加 clippy::all gate

needless_range_loop/manual_is_multiple_of/redundant_closure/useless_conversion
等全量自动+手动修复。

fixes 共性4(clippy)"
```

---

## Task P2-Final: 全量回归验证

- [ ] **Step 1：全量编译 + 测试**

```bash
cargo build --workspace
cargo test --workspace
```

- [ ] **Step 2：clippy 零警告**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3：前端构建验证**

```bash
cd crates/desktop/frontend && npm run build
```
Expected: 前端构建通过，无 console.log 残留导致的 lint 错误。

- [ ] **Step 4：更新审查报告 — 标注全部已修复**

在 `docs/code-review-2026-07-05.md` 检查所有条目是否标注 `✅ 已修复`。

- [ ] **Step 5：更新 architecture.md**

- [ ] **Step 6：提交收尾**

```bash
git add docs/
git commit -m "docs: P2 清理完成，全量审查修复收官"
```

---

## 实施记录

> 本节在实施过程中回写实际偏差。

（待实施时填写）
