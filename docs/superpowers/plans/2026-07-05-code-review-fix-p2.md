# 代码审查修复 P2 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:executing-plans 或 superpowers:subagent-driven-development 逐任务实施。Steps 用 checkbox (`- [x]`) 跟踪。

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

- [x] **Step 1：验证 image_util 零调用**

```bash
rg "save_as_webp|save_as_png|save_as_jpeg|image_util" crates/ --glob '!crates/infra/src/image_util.rs'
```
Expected: 无命中（仅定义处）。

- [x] **Step 2：删除 image_util.rs + lib.rs 声明 + Cargo.toml 依赖**

```bash
rm crates/infra/src/image_util.rs
```
编辑 `lib.rs` 删 `pub mod image_util;`（或 `mod image_util;`）。
编辑 `Cargo.toml`：确认 `image` 和 `webp` 是否仅被 image_util 使用（`rg "use image\|use webp\|image::\|webp::" crates/infra/src/`），如否则删除。

- [x] **Step 3：验证并删除 desktop 死代码**

逐个验证零调用后删除（含 `#[allow(dead_code)]` 标注的）：
```bash
rg "unregister_shortcut" crates/desktop/src/ --glob '!*shortcut.rs'
rg "is_screenshot_active" crates/desktop/src/ --glob '!*screenshot_commands.rs'
rg "send_scroll" crates/desktop/src/ --glob '!*screenshot_commands.rs'
rg "close_all_pin_windows" crates/desktop/src/ --glob '!*pin_window.rs'
```

- [x] **Step 4：验证并删除 download/capx/asr-cloud 死代码**

```bash
rg "if_range_value" crates/download/src/ --glob '!*verify.rs'
rg "capture_display_excluding_window" crates/capx/src/ --glob '!*capture.rs'
rg "COMP_NONE|\.serialization" crates/asr-cloud/src/ --glob '!*bytedance_stream.rs'
```

删除 downlownloader.rs:202-206 不可达 416 else 分支。

- [x] **Step 5：删除死依赖**

`dlp/Cargo.toml` 删 `tempfile`（如 P0 未删）。`llm/Cargo.toml` 删 `serde_yaml`（dev-dep）。

- [x] **Step 6：编译验证**

```bash
cargo build --workspace
```
Expected: 编译通过。如有遗漏的引用，编译器会报错，逐个修复。

- [x] **Step 7：提交**

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

- [x] **Step 1：测试块加 cfg 门控**

`capture.rs:340` 的 `#[cfg(test)] mod tests` 改为 `#[cfg(all(test, target_os = "macos"))] mod tests`，或给每个调用 `bgra_to_rgba` 的测试加 `#[cfg(target_os = "macos")]`。

- [x] **Step 2：验证非 macOS 编译（如有 Linux 环境）**

```bash
# macOS 上无法直接验证 Linux 编译，但可以检查 cfg 一致性
cargo test -p octopus-capx  # macOS 上应 PASS
```

- [x] **Step 3：提交**

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

- [x] **Step 1：asr-local eprintln! → log::debug!**

```bash
rg "eprintln!" crates/asr-local/src/ -n
```
逐个改为 `log::debug!`（保留信息内容）。确认 asr-local 已有 `log` 依赖（`rg "log" crates/asr-local/Cargo.toml`）。

- [x] **Step 2：desktop screenshot_commands eprintln! → log::debug!**

```bash
rg "eprintln!" crates/desktop/src/screenshot_commands.rs -n
```

- [x] **Step 3：asr-cloud aliyun info! → debug!（热路径日志洪水）**

`aliyun_stream.rs:198-201` 和 `:442-445` 的 `log::info!` 改为 `log::debug!`。

- [x] **Step 4：前端删 console.log**

`Result/index.tsx:155,162` 删除两行 `console.log`。
```bash
rg "console.log" crates/desktop/frontend/src/ -n
```
删除所有进生产的 `console.log`（保留 dev-only 的如有标注）。

- [x] **Step 5：编译 + 测试**

```bash
cargo build -p octopus-asr-local -p octopus-desktop -p octopus-asr-cloud
cargo test -p octopus-asr-local -p octopus-asr-cloud
```

- [x] **Step 6：提交**

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

- [x] **Step 1：自动修复可修的 lint**

```bash
cargo clippy --fix --workspace --allow-dirty --allow-staged 2>&1 | tail -20
```

- [x] **Step 2：检查剩余 lint**

```bash
cargo clippy --workspace --all-targets 2>&1 | grep "^warning:" | head -30
```

- [x] **Step 3：手动修复剩余 lint**

常见手动修复：
- `needless_range_loop`(19) → `enumerate()` 或直接索引
- `manual_is_multiple_of`(12) → `.is_multiple_of()`
- `too_many_arguments`(3) → 参数封装为 struct

- [x] **Step 4：加 clippy gate 到各 lib.rs**

在各 crate 的 `lib.rs` 顶部加（如未有）：
```rust
#![warn(clippy::all)]
```

- [x] **Step 5：编译 + 测试 + clippy 零警告验证**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | grep -c "^warning" 
```
Expected: clippy 0 warning（desktop 前端 dist 除外）。

- [x] **Step 6：提交**

```bash
git add -A
git commit -m "cleanup: cargo clippy --fix 修复 118 个 lint + 加 clippy::all gate

needless_range_loop/manual_is_multiple_of/redundant_closure/useless_conversion
等全量自动+手动修复。

fixes 共性4(clippy)"
```

---

## Task P2-Final: 全量回归验证

- [x] **Step 1：全量编译 + 测试**

```bash
cargo build --workspace
cargo test --workspace
```

- [x] **Step 2：clippy 零警告**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [x] **Step 3：前端构建验证**

```bash
cd crates/desktop/frontend && npm run build
```
Expected: 前端构建通过，无 console.log 残留导致的 lint 错误。

- [x] **Step 4：更新审查报告 — 标注全部已修复**

在 `docs/code-review-2026-07-05.md` 检查所有条目是否标注 `✅ 已修复`。

- [x] **Step 5：更新 architecture.md**

- [x] **Step 6：提交收尾**

```bash
git add docs/
git commit -m "docs: P2 清理完成，全量审查修复收官"
```

---

## 实施记录

> 本节在实施过程中回写实际偏差。

### G1 实施偏差

- **Pipeline trait 删除额外影响**：trait impl 块（`impl Pipeline for StreamingPipeline` 和 `impl Pipeline for VadSegmentedPipeline`）也需删除。`VadSegmentedPipeline` 的 `finish`/`reset` 原仅在 trait impl 中定义，删除后需补为 inherent 方法。
- **feature-gated 死代码**：`StreamingPipeline::current_partial`/`is_cloud` 仅 cloud feature 下调用，`VadSegmentedPipeline::reset` 当前未被 coordinator 调用（stage 切换直接 drop）。三者保留并加 `#[allow(dead_code)]`（功能完整、未来可能启用）。
- **dlp/llm 死依赖**：P0/P1 已删除 `tempfile`（dlp）和 `serde_yaml`（llm dev-dep），G1 确认无残留。
- **downloader.rs 416 分支**：原 else 分支注释 "416 等" 有误（416 被 classify_status 归为 Fatal 早已 return）。重构为 `if 200 → seg.begin else → start`，注释更正为 "仅 200/206 可达"。

### G4 实施偏差

- **clippy lint 总数**：原计划 "118 个"，实际 auto-fix 后剩余 70 个手动修，最终全部清零。
- **needless_range_loop 在 CIF 热路径**：`streaming_paraformer.rs` 的 `run_cif`/`run_cif_final` 各 3 处 `for j in 0..feat` 索引 enc_row + cache，改 iterator 后可读性下降，加 `#[allow(clippy::needless_range_loop)]` 函数级标注。
- **too_many_arguments (4处)**：`run_decoder_step`/`try_match_1d_projection`/`apply_fallback_match`/`do_paste` 均为内部函数、参数语义独立无法封装为 struct，加 `#[allow]`。
- **type_complexity (2处)**：`extract_kv` 返回 4-tuple（ONNX KV cache 语义）、DB 行映射 10-tuple，加 `#[allow]`（type alias 增加间接层无收益）。
- **vec_init_then_push**：`config.rs` 的 cfg-gated EP push 改用 `vec![#[cfg(...)] ...]` 宏内 cfg（比 `#[allow]` 更干净）。
- **测试同步**：`desktop/pipeline.rs` 3 个测试预期未含 `Speaking(true)` 首帧事件（pre-existing 不同步），补齐。

### P2-Final 偏差

- **server/pipeline.rs pre-existing test failure**：`ws_stream_session_feed_partial_then_empty_finish_final` — 512 静音样本在有 VAD 模型环境下被门控（`seen_speech` 不置 true）→ accept_samples 不调用 → 返回空。**已在 follow-up 修复**：`StreamingRunner` 新增 `new_no_vad` 构造（vad=None 跳过门控），server 测试用此验证纯 relay 管线。

### Follow-up（2026-07-05，审查报告 Important + Minor + FTS5）

> P2 完成后的 follow-up 轮次，处理审查报告遗留项。

- **Important 清单（I-H 系列）**：
  - I-H1 `save_app_config_at` 30 条写入包 `unchecked_transaction` → `commit`（原子，中途崩溃全回滚）
  - I-H2 `ensure_db` 打开后设 `PRAGMA journal_mode=WAL; busy_timeout=5000`（多任务并发友好）
  - I-H3 voice 历史搜索切 FTS5 MATCH（详见独立 spec `2026-07-05-fts5-search-design.md`）
  - I-D1 JSON 转义已确认用 serde_json（P2 期间完成，spec 补标 ✅）
  - I-F1 `main.rs:102` `unreachable!()` → 穷举 match（coordinator.rs 原三处已在 stage 重构中消失）
- **Minor 精选**：
  - `llm/client.rs` max_tokens 系数 `×1.2` → `×2.0`（中文 1-2 token/char，×1.2 致润色截断；3 处）
  - `asr-cloud/aliyun_stream.rs` `bearer` → `Bearer` 统一（RFC 7235 case-insensitive，同文件 Qwen-ASR 路径已用大写）
  - moonshine saturating_sub 已在 P0 Task A3 完成
- **FTS5 搜索切换**（I-H3，独立 spec）：
  - v18 迁移：backfill 历史 voice 行进 `clipboard_history_fts` 索引（幂等 `NOT IN`）
  - `list_transcriptions_search_at`：>=3 字符走 MATCH（trigram 倒排索引），<3 字符回退 LIKE
  - `escape_fts5_match`：双引号包裹 phrase，内部双引号双写转义
  - 6 个新测试全过
- **选中替换诊断日志**（为偶发失败定位做证据采集）：
  - `transcript.rs` 8 处 `log::debug!("[select] ...")` 覆盖 pending_delete 全生命周期
  - `coordinator.rs` 2 处跨会话播种 correlation log
- **Important 项收尾**（I-F2 / I-F3 / M-4）：
  - I-F2 `screenshot_commands.rs` `AtomicBool` CAS 门控 + `BusyGuard` RAII（Drop 释放），`start_screenshot` 入口门控，覆盖快捷键 + 托盘两个调用路径
  - I-F3 `create_tray` 返回 `Result`（11 处 `expect`→`map_err?`），调用方 log 降级（无托盘菜单仍可用快捷键）；clipboard handle `expect`→`?`；`home_dir().expect`→`or_else(dirs::home_dir).ok_or?`。`main.rs:470` tauri build `expect` 保留（真正 fatal 无降级路径）
  - M-4 `infra/db.rs` 3 处生产代码 `filter_map(|r| r.ok())` → `collect_rows(rows, context)` helper（失败行 `log::warn` 跳过而非静默丢弃）。测试代码保留 filter_map
- **Minor 收尾**（M-5/M-6/M-7）：
  - M-5 `baidu_stream.rs` `Message::Close` 不再无条件发 Finished——空结果时发 Failed 暴露异常关闭
  - M-6 `capx/stitch.rs` 2 处 `from_raw().expect()` → match `Some/None` 降级（log + 1×1 空图，不 panic）
  - M-7 asr-cloud 4 provider JoinHandle 丢弃——评估后**决定不修**（close_async 30s 超时兜底 + panic task 自动回收 + 已有 error log）
- **Bug B 修复**（跨会话选中替换 idle_selection 残留/stale，2026-07-05 方案 C）：
  - `coordinator.rs` 移除 `idle_selection` 后端缓存（11 处引用删除），改两阶段 Toggle：`emit("prepare-record")` → 前端 `invoke("start_recording", {prepareId, selection})` → `begin_recording(selection)` 种子
  - 前端 `currentSelectionRef`（mouseup 缓存 `{start,end,text}`，blur/selectionchange 清空）
  - `start_recording` Tauri command 参数名 `prepare_id` → 前端 `prepareId`（camelCase）
  - 看门狗 200ms `FallbackStart` 超时兜底（冷启动前端未 mount）
- **从右往左选到开头失效修复**（前端拖选三重陷阱，详见 spec `2026-07-03-asr-cursor-insert-design.md` §15）：
  - 陷阱 1：`Range.startContainer` 飘移到父容器 → `clampRangeToContainer` 用 `compareBoundaryPoints` 裁剪
  - 陷阱 2：React `onMouseUp` 不在 textRef 外触发 → `onMouseDown` 时在 `document` 上注册一次性 mouseup listener
  - 陷阱 3：mouseup 时鼠标在容器外 → `getBoundingClientRect()` 判断 X 坐标方向（左边界外→offset=0，右边界外→末尾）
  - 兜底：`mouseDownOffsetRef` 缓存起点 offset，mouseup 时 min/max 重建选区（不依赖 mouseup 瞬间的 DOM Selection 状态）
  - 中间废弃方案：`isSelectingRef` 阻止 `renderResultNow`（导致卡死/state 不同步）、`selectionchange` 高频缓存（性能问题），均已移除

