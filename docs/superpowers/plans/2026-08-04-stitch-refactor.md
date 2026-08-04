# stitch.rs 拆分实施计划（阶段 1：纯机械拆分）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 123KB 单文件 `crates/capx/src/stitch.rs` 拆成 `stitch/` 子目录 5 个职责聚焦的文件，零行为变更。

**Architecture:** 纯机械迁移——把现有代码按职责逐字剪贴到子模块，靠 `use super::*` + `pub(crate)` 可见性让符号无需改名。Rust split-impl 允许 `impl Stitcher` 块散布在多个文件，使 fallback/heal 方法能保留 inherent method 签名一字不动。

**Tech Stack:** Rust 2021 / image 0.25 / imageproc 0.25 / Tauri 2 workspace。`octopus-capx` crate。

## Global Constraints

- **硬约束：零行为变更**——`cargo test -p octopus-capx` 必须通过且测试数量 = **49**（baseline，2026-08-04 main 1fdbb6d5）
- **不删任何测试、不改任何测试数据/断言值**
- **不动常量值**——9 个模块常量原样搬
- **公开 API 签名零变更**——`pub struct Stitcher` / `pub struct StitchConfig` / `pub fn` 方法签名一字不改
- **`mod` 路径不变**——`crate::stitch::Stitcher` / `octopus_capx::stitch::Stitcher` 保持原样（lib.rs 不改）
- **Worktree**：所有改动在 `.worktrees/refactor-stitch-split` 分支 `refactor/stitch-split`，**未经用户明确指令不 push 到 main**
- **每步独立 commit**——每步 commit 后 `cargo test -p octopus-capx` 必须通过且 = 49 passed
- **0 warning**——`cargo build` / `cargo clippy --all-targets` 均 0 warning（仓库 `clippy::all` gate）

---

## File Structure

拆分前后对照（精确行号映射，源自 stitch.rs main 1fdbb6d5）：

| 目标文件 | 来源行号（stitch.rs） | 职责 | 预估行数 |
|---|---|---|---|
| `stitch/mod.rs` | 5–37, 315–377, 379–735, 1208–1322, 1420–1428 + 跨模块集成测试 | 模块常量 + `StitchConfig`/`Stitcher` struct + 编排（new/process_frame/process_frame_inner/primary_ncc/best_ncc_match/finalize）+ read API + rows_equal_buf + 集成测试 | ~600 |
| `stitch/graybuf.rs` | 39–174, 304–313 + 测试 1438–1547, 1797–1812, 1891–1912 | GrayBuf + to_feature_map + row_projection_means + feature map 对照测试 | ~280 |
| `stitch/ncc_match.rs` | 176–313 + 测试 2322–2431 | NccResult/PrimaryOutcome + ncc_match family + NCC 引擎测试 | ~300 |
| `stitch/canvas_heal.rs` | 848–964, 1324–1416 + 测试 1644–1675, 1865–1890, 1913–1972, 2029–2321 | 6 自愈机制 + detect_sticky + reseed + content_tail + heal/sticky 测试 + test-only impl | ~350 |
| `stitch/fallback_chain.rs` | 741–846, 993–1206 + 测试 1813–1827, 2029–2043, 2399–2556 | try_fallback + 5 try_*/verify_*/estimate + fallback 测试 | ~500 |

`crates/capx/src/lib.rs` 不改（`pub mod stitch; pub mod capture;`）。

---

## Task 0: 准备与 baseline 确认

**Files:**
- Verify: `crates/capx/src/stitch.rs` 存在、123KB
- Verify: `.worktrees/refactor-stitch-split` 在 `refactor/stitch-split` 分支

**Interfaces:** N/A（本任务只是确认安全网）

- [ ] **Step 1: 确认 worktree 状态**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
git status
git log --oneline -3
```

Expected: `On branch refactor/stitch-split` + 顶部 commit 是 spec commit（4206d409 或后续），工作区干净。

- [ ] **Step 2: 确认 baseline 测试**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected:
```
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <X>s
```

**记录这个数字 49——后续每个 Task 末尾都必须看到同样的数字。** 如果不是 49，停下，main 已经改了，先 rebase/同步再开工。

- [ ] **Step 3: 确认 baseline clippy**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
cargo clippy -p octopus-capx --all-targets 2>&1 | tail -5
```

Expected: 0 warning。如果有 warning，停下——baseline 不干净，先处理。

- [ ] **Step 4: 不 commit**（本任务只是验证，无文件改动）

---

## Task 1: 拆 graybuf.rs

**Files:**
- Create: `crates/capx/src/stitch/graybuf.rs`
- Rename: `crates/capx/src/stitch.rs` → `crates/capx/src/stitch/mod.rs`（git mv）
- Modify: `crates/capx/src/stitch/mod.rs`（删 39–174 + 304–313、顶部加 `mod graybuf;` + `use`）

**Interfaces:**
- Produces: `crate::stitch::graybuf::GrayBuf`（`pub(crate)`，通过 mod.rs `pub use` 重导出）、`to_feature_map`、`row_projection_means`
- Consumes: 仅 `image` / `imageproc` crate（无项目内依赖）

**关键不变量：** 拆完后 mod.rs 的 `use` 语句 + 顶部 `mod graybuf;` 必须让 mod.rs 内剩余代码编译通过、所有调用 `GrayBuf::from_rgba_roi` / `to_feature_map` / `row_projection_means` 的地方符号照常解析。

- [ ] **Step 1: 创建子目录并 git mv**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
mkdir -p crates/capx/src/stitch
git mv crates/capx/src/stitch.rs crates/capx/src/stitch/mod.rs
```

这把 `stitch.rs` 重命名为 `stitch/mod.rs`，git 会跟踪为 rename，blame 历史保留。此时 `crates/capx/src/stitch/` 是新目录、`mod.rs` 是原文件内容。

- [ ] **Step 2: 新建 graybuf.rs 文件骨架**

在 `crates/capx/src/stitch/graybuf.rs` 写入：

```rust
//! GrayBuf: 连续 row-major 灰度 buffer，替代 image::GrayImage。
//! 消除 get_pixel() 的坐标计算 + 边界检查开销，用整行切片直访。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。

use super::*;
```

文件先空骨架，下一步剪贴内容。

- [ ] **Step 3: 从 mod.rs 剪贴 graybuf 内容到 graybuf.rs**

**从 `stitch/mod.rs` 删除以下区段**（用 Edit 一段段做，每段确认 old_string 唯一）：

1. `struct GrayBuf { ... }` 及其上方的 `///` 文档注释（约 39–47）
2. `impl GrayBuf { ... }` 整块（49–105，含 `from_rgba_roi` / `row` / `to_gray_image`）
3. `fn to_feature_map(...)` 整块（107–174，含 Sobel + Welford 实现）
4. `fn row_projection_means(...)` 整块（304–313）

**粘到 `stitch/graybuf.rs`** 末尾（在 `use super::*;` 之后）。粘贴时保留原文逐字不变，包括所有注释。

**可见性调整**（graybuf.rs 内）：
- `struct GrayBuf` → `pub(crate) struct GrayBuf`
- `impl GrayBuf` 内的 `fn from_rgba_roi` / `fn row` / `fn to_gray_image` 全部加 `pub(crate)`
- 字段 `data` / `width` / `y_offset` 加 `pub(crate)`（fallback_chain.rs 的 `try_match_prev_frame` 直接读 `prev_gray.data.len()` / `prev_gray.width`）
- `fn to_feature_map` → `pub(crate) fn to_feature_map`
- `fn row_projection_means` → `pub(crate) fn row_projection_means`

**注意**：保留 `#[derive(Clone)]` 在 `GrayBuf` 上方。

- [ ] **Step 4: 在 mod.rs 顶部加 mod 声明 + use**

打开 `stitch/mod.rs`，在 `use anyhow::Result;` 等 use 语句之后、第一个常量 `STATIONARY_SAD` 之前，插入：

```rust
mod graybuf;
pub(crate) use graybuf::{to_feature_map, row_projection_means};
pub use graybuf::GrayBuf;
use graybuf::{to_feature_map as _, row_projection_means as _};
```

注意：
- `pub use graybuf::GrayBuf;` 保持 GrayBuf 对外路径不变（`octopus_capx::stitch::GrayBuf` 仍可用——查证 mod.rs 里 GrayBuf 是否本来就 `pub`，原本是 `struct GrayBuf`（私有），那就不需要 `pub use`，改为 `pub(crate) use graybuf::GrayBuf;`）。
- 实际上原 `struct GrayBuf` 是 `struct GrayBuf`（无私有也不 pub），所以是 crate 内默认私有；为了被兄弟模块用、加 `pub(crate) use graybuf::GrayBuf;`。
- 上一行 `pub(crate) use graybuf::{to_feature_map, row_projection_means};` 让 mod.rs 内的代码（如 `primary_ncc` 调 `to_feature_map`）继续工作。
- 如果 mod.rs 内有 `use graybuf::{... as _};` 不需要——只要 `pub(crate) use` 已经把名字引入 mod.rs 作用域就够了。**实际上最简写法**：

```rust
mod graybuf;
pub(crate) use graybuf::{GrayBuf, to_feature_map, row_projection_means};
```

这一行即可：声明 mod + 把 3 个名字引入 mod.rs 作用域（兄弟模块通过 `use super::*` 拿到）。

- [ ] **Step 5: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
cargo build -p octopus-capx 2>&1 | tail -20
```

Expected: `Finished` / 0 error 0 warning。

如果有 error，**先看完整 error 列表**再逐个修。典型 error：
- `cannot find type GrayBuf` → mod.rs `use` 没加全
- `to_feature_map is private` → graybuf.rs 内可见性没加 `pub(crate)`
- `field data of struct GrayBuf is private` → 字段没加 `pub(crate)`

修完再 build，直到 0 error 0 warning。

- [ ] **Step 6: 测试验证（关键——行为等价判据）**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected: `test result: ok. 49 passed; 0 failed; 0 ignored`。

**如果数字不是 49 = 行为变了 = 拆错了**。停下，回看哪段搬错或漏搬。

- [ ] **Step 7: 留测试原地（本 Task 不搬测试，下个 Task 再处理）**

测试仍在 `mod.rs` 的 `mod tests` 里，能跑就行。**搬测试独立做**——避免和"搬生产代码"混在一个 commit，git diff 更清晰。

- [ ] **Step 8: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
git add -A
git commit -m "refactor(capx): 拆 graybuf.rs——GrayBuf + to_feature_map + row_projection_means

- stitch.rs → stitch/mod.rs（git mv 保留 blame 历史）
- 新增 stitch/graybuf.rs：GrayBuf struct + from_rgba_roi/row/to_gray_image + to_feature_map（Sobel+Welford）+ row_projection_means
- 可见性：struct/字段/fn 全升 pub(crate)，对 crate 内兄弟模块可见
- mod.rs 顶部加 mod graybuf + pub(crate) use
- 测试暂留 mod.rs（下个 task 跟随各自模块搬出）

零行为变更：cargo test -p octopus-capx → 49 passed"
```

- [ ] **Step 9: 验证 commit 干净**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
git status
git show --stat HEAD
```

Expected: 工作区干净；commit 显示 `stitch.rs` rename 为 `stitch/mod.rs` + 新增 `stitch/graybuf.rs`。

---

## Task 2: 拆 ncc_match.rs

**Files:**
- Create: `crates/capx/src/stitch/ncc_match.rs`
- Modify: `crates/capx/src/stitch/mod.rs`（删 176–313、顶部加 `mod ncc_match;`）

**Interfaces:**
- Produces: `crate::stitch::ncc_match::NccResult`、`PrimaryOutcome`、`ncc_match`、`ncc_match_range`、`validate_ncc_match`、`parabolic_refine_from_response`、`downsample_grayimage`
- Consumes: `image::GrayImage` / `imageproc::template_matching` / `imageproc::definitions::Image`（无项目内依赖——NCC 引擎全是 free function）

**关键不变量：** 拆完后 mod.rs 内的 `primary_ncc` / `best_ncc_match` / `finalize` / `try_match_prev_frame`（在 fallback_chain，但本 Task 时仍在 mod.rs）调用 NCC 函数的地方照常解析。

- [ ] **Step 1: 新建 ncc_match.rs 骨架**

在 `crates/capx/src/stitch/ncc_match.rs` 写入：

```rust
//! NCC（归一化互相关）模板匹配引擎。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! 纯 free function，无 &self 依赖——主匹配/邻帧参考/finalize 都通过这一层。

use super::*;
```

- [ ] **Step 2: 从 mod.rs 剪贴 NCC 内容到 ncc_match.rs**

**从 `stitch/mod.rs` 删除**：

1. `use imageproc::definitions::Image;` 和 `use imageproc::template_matching::match_template;` 之类的 use（在 176–179 附近——需 Read 确认精确行）
2. `struct NccResult { ... }` 整块（约 182–190）
3. `enum PrimaryOutcome { ... }` 整块（约 192–200，含文档注释）
4. `fn ncc_match(...)` 整块（约 202–221）
5. `fn downsample_grayimage(...)` 整块（约 222–230）
6. `fn ncc_match_range(...)` 整块（约 231–254）
7. `fn validate_ncc_match(...)` 整块（约 255–283）
8. `fn parabolic_refine_from_response(...)` 整块（约 284–303）

**粘到 `stitch/ncc_match.rs`** 末尾（`use super::*;` 之后），逐字不变。

**可见性调整**（ncc_match.rs 内）：
- `struct NccResult` → `pub(crate) struct NccResult`，字段 `response` / `best_y` / `best_score` 加 `pub(crate)`
- `enum PrimaryOutcome` → `pub(crate) enum PrimaryOutcome`，变体 `Match` / `Mismatch` / `SizeError` 自动 pub(crate)（跟随 enum）
- 所有 `fn` 全部加 `pub(crate)`

- [ ] **Step 3: 在 mod.rs 顶部加 mod 声明 + use**

在 mod.rs 已有的 `mod graybuf;` 下方加：

```rust
mod ncc_match;
pub(crate) use ncc_match::{
    NccResult, PrimaryOutcome,
    ncc_match, ncc_match_range, validate_ncc_match,
    parabolic_refine_from_response, downsample_grayimage,
};
```

注意 `pub(crate) use` 同时完成两个作用：(1) 把名字引入 mod.rs 作用域供 mod.rs 内代码用；(2) 让兄弟模块通过 `use super::*` 拿到。

- [ ] **Step 4: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/refactor-stitch-split
cargo build -p octopus-capx 2>&1 | tail -20
```

Expected: 0 error 0 warning。典型 error：NCC 函数可见性漏加 → 编译器逐个指出。

- [ ] **Step 5: 测试验证**

Run:
```bash
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected: `test result: ok. 49 passed; 0 failed; 0 ignored`。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(capx): 拆 ncc_match.rs——NCC 引擎原语

- 新增 stitch/ncc_match.rs：NccResult/PrimaryOutcome + ncc_match + downsample_grayimage + ncc_match_range + validate_ncc_match + parabolic_refine_from_response
- 可见性：struct/enum/fn 全升 pub(crate)
- mod.rs 顶部加 mod ncc_match + pub(crate) use

零行为变更：cargo test -p octopus-capx → 49 passed"
```

---

## Task 3: 拆 canvas_heal.rs（首次 split-impl + 升 Stitcher 字段 pub(crate)）

**Files:**
- Create: `crates/capx/src/stitch/canvas_heal.rs`
- Modify: `crates/capx/src/stitch/mod.rs`（删 848–964, 1324–1416；升 Stitcher 字段 pub(crate)；加 `mod canvas_heal;`）

**Interfaces:**
- Produces: `impl Stitcher { extract_canvas_bottom_gray, canvas_bottom_constant, scan_canvas_constant_tail, reseed_canvas_from, detect_sticky, detect_content_tail, scan_content_tail_in, effective_strip_for, invalidate_cache }`（split-impl，所有方法保持 inherent method、签名不改）
- Consumes: `GrayBuf::from_rgba_roi`、`STICKY_DETECT_MAX` / `CONTENT_ROW_MAXMIN` / `CONTENT_TAIL_MAX_LUMA` / `MIN_STRIP` 常量（mod.rs）、`rows_equal_buf`（mod.rs）

**关键不变量：**
- `Stitcher` struct 字段必须先升 `pub(crate)`，否则 split-impl 文件里 `self.canvas_buf` 编不过
- 升字段可见性是本 Task 唯一对 struct 定义的改动，必须一字不改字段类型/默认值

**风险点：** split-impl 首次引入，最容易出错的 Task。一定要 `cargo build` 反复迭代到 0 error。

- [ ] **Step 1: 升 Stitcher 字段为 pub(crate)**

打开 `stitch/mod.rs`，找到 `pub struct Stitcher { ... }`（约 344–377），把所有字段前缀改为 `pub(crate)`：

```rust
pub struct Stitcher {
    pub(crate) canvas_w: u32,
    pub(crate) canvas_h: u32,
    pub(crate) canvas_buf: Vec<u8>,
    pub(crate) canvas_cache: Option<RgbaImage>,
    pub(crate) sticky_top: u32,
    pub(crate) sticky_bottom: u32,
    pub(crate) detected: bool,
    pub(crate) config: StitchConfig,
    pub(crate) last_dy: Option<f64>,
    pub(crate) dy_history: VecDeque<f64>,
    pub(crate) best_guess_streak: u32,
    pub(crate) ncc_stuck_count: u32,
    pub(crate) last_appended_dy: Option<f64>,
    pub(crate) same_dy_count: u32,
    pub(crate) prev_gray: Option<GrayBuf>,
    pub(crate) content_tail: u32,
    pub(crate) eff_strip_h: u32,
}
```

**不改类型、不改文档注释**。仅加 `pub(crate)` 前缀。

`StitchConfig` 字段原本就 `pub`（外部可设），保持不动。

- [ ] **Step 2: 新建 canvas_heal.rs 骨架**

在 `crates/capx/src/stitch/canvas_heal.rs` 写入：

```rust
//! Canvas 锚点自愈机制。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! 6 个自愈机制：seed-tail trim / content_tail 检测 / canvas-bottom-constant 修剪 /
//! destructive reseed / adaptive strip_h / sticky 顶底检测。
//! 所有方法为 inherent method（split-impl），签名一字不改。

use super::*;
```

- [ ] **Step 3: 从 mod.rs 剪贴 canvas heal 方法到 canvas_heal.rs**

**从 `stitch/mod.rs` 的 `impl Stitcher { ... }` 块中删除以下方法**（每个方法连同上方文档注释一起）：

1. `fn invalidate_cache(&mut self)` 整块（约 848–852）
2. `fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf` 整块（约 854–872）
3. `fn canvas_bottom_constant(&self) -> bool` 整块（约 874–898）
4. `fn scan_canvas_constant_tail(&self) -> u32` 整块（约 900–933）
5. `fn reseed_canvas_from(&mut self, frame: &RgbaImage, eff_top: u32, eff_bottom: u32)` 整块（约 935–964）
6. `fn detect_sticky(&mut self, frame: &RgbaImage)` 整块（约 1324–1342）
7. `fn detect_content_tail(...)` 整块（约 1359–1370，需 Read 确认精确范围）
8. `fn scan_content_tail_in(...)` 整块（约 1371–1410）
9. `fn effective_strip_for(...)` 整块（约 1411–1416）

**粘到 `stitch/canvas_heal.rs` 末尾**，逐字不变。

**包装成 split-impl**——在 canvas_heal.rs 末尾（所有方法外层）包一个 impl 块：

```rust
impl super::Stitcher {
    // 所有上面粘贴的方法
}
```

即把剪贴过来的 9 个方法（每个含上方文档注释）放进 `impl super::Stitcher { ... }` 内。

**不改任何方法签名**——保持 `fn invalidate_cache(&mut self)` / `fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf` 等原样。

- [ ] **Step 4: 在 mod.rs 顶部加 mod 声明**

在已有 `mod ncc_match;` 下方加：

```rust
mod canvas_heal;
```

无需 `use`——canvas_heal 是 `impl Stitcher` 方法，调用方（mod.rs 内的 `process_frame` 等）仍按 `self.detect_sticky(...)` / `self.extract_canvas_bottom_gray(...)` 调用，Rust 会自动找到 split-impl 块。

- [ ] **Step 5: 编译验证（关键——split-impl 首次）**

Run:
```bash
cargo build -p octopus-capx 2>&1 | tail -30
```

Expected: 0 error 0 warning。

**预期 error（如有）**：
- `field canvas_buf of struct Stitcher is private` → Step 1 字段升 pub(crate) 漏了某个
- `method detect_sticky not found` → impl 块没正确包 / 方法漏搬
- `cannot find type GrayBuf in canvas_heal.rs` → 顶部 `use super::*` 没加

修完再 build，直到 0 error 0 warning。

- [ ] **Step 6: 测试验证**

Run:
```bash
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected: `test result: ok. 49 passed; 0 failed; 0 ignored`。

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(capx): 拆 canvas_heal.rs——6 自愈机制 split-impl

- 新增 stitch/canvas_heal.rs：invalidate_cache / extract_canvas_bottom_gray / canvas_bottom_constant / scan_canvas_constant_tail / reseed_canvas_from / detect_sticky / detect_content_tail / scan_content_tail_in / effective_strip_for（split-impl inherent method，签名不改）
- Stitcher 字段全升 pub(crate)（不改类型/默认值）——split-impl 在兄弟文件能直接 self.<field>
- mod.rs 顶部加 mod canvas_heal
- 测试暂留 mod.rs（下个 task 跟随各自模块搬出）

零行为变更：cargo test -p octopus-capx → 49 passed"
```

---

## Task 4: 拆 fallback_chain.rs

**Files:**
- Create: `crates/capx/src/stitch/fallback_chain.rs`
- Modify: `crates/capx/src/stitch/mod.rs`（删 741–846, 993–1206；加 `mod fallback_chain;`）

**Interfaces:**
- Produces: `impl Stitcher { try_fallback, try_match_prev_frame, try_match_1d_projection, apply_fallback_match, verify_alignment_2d, estimate_dy_hint, quick_stationary_check }`（split-impl）
- Consumes: `GrayBuf` / `to_feature_map` / `ncc_match` / `validate_ncc_match` / `parabolic_refine_from_response` / `row_projection_means` / `extract_canvas_bottom_gray`（canvas_heal）/ 常量 `STATIONARY_SAD` / `FALLBACK_VERIFY_SAD` / `X_START_RATIO` / `X_END_RATIO` / `SAMPLE_STEP_X` / `DY_HISTORY_LEN`

**关键不变量：** fallback 方法间互相调用（如 `try_fallback` 调 `try_match_prev_frame` / `try_match_1d_projection` / `quick_stationary_check` / `estimate_dy_hint` / `apply_fallback_match`）必须在同一 impl 块内仍工作——split-impl 自然满足。

- [ ] **Step 1: 新建 fallback_chain.rs 骨架**

在 `crates/capx/src/stitch/fallback_chain.rs` 写入：

```rust
//! 五层降级链：NCC 失败时的兜底处理。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! dispatcher try_fallback 按序尝试：邻帧参考 NCC → 1D 投影 → 静止检测 → best-guess。
//! 所有方法为 inherent method（split-impl），签名一字不改。

use super::*;
```

- [ ] **Step 2: 从 mod.rs 剪贴 fallback 方法到 fallback_chain.rs**

**从 `stitch/mod.rs` 的 `impl Stitcher { ... }` 块中删除以下方法**（每个含上方文档注释）：

1. `fn try_match_prev_frame(&self, prev_gray: &GrayBuf, ...)` 整块（约 741–784）
2. `fn try_fallback(&mut self, ...)` 整块（约 787–846）
3. `fn quick_stationary_check(&self, ...)` 整块（约 968–982）
4. `fn verify_alignment_2d(&self, ...)` 整块（约 993–1036）
5. `fn estimate_dy_hint(&self)` 整块（约 1040–1053）
6. `fn try_match_1d_projection(&self, ...)` 整块（约 1059–1147）
7. `fn apply_fallback_match(&mut self, ...)` 整块（约 1148–1206）

**粘到 `stitch/fallback_chain.rs`** 末尾，逐字不变。

**包装成 split-impl**：

```rust
impl super::Stitcher {
    // 所有上面粘贴的方法
}
```

- [ ] **Step 3: 在 mod.rs 顶部加 mod 声明**

在已有 `mod canvas_heal;` 下方加：

```rust
mod fallback_chain;
```

- [ ] **Step 4: 编译验证**

Run:
```bash
cargo build -p octopus-capx 2>&1 | tail -30
```

Expected: 0 error 0 warning。

- [ ] **Step 5: 测试验证**

Run:
```bash
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected: `test result: ok. 49 passed; 0 failed; 0 ignored`。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(capx): 拆 fallback_chain.rs——五层降级链 split-impl

- 新增 stitch/fallback_chain.rs：try_match_prev_frame / try_fallback / quick_stationary_check / verify_alignment_2d / estimate_dy_hint / try_match_1d_projection / apply_fallback_match（split-impl inherent method，签名不改）
- mod.rs 顶部加 mod fallback_chain
- 测试暂留 mod.rs（下个 task 跟随各自模块搬出）

零行为变更：cargo test -p octopus-capx → 49 passed"
```

---

## Task 5: 搬测试到各自模块 + 收尾（文档/下游/clippy）

**Files:**
- Modify: `crates/capx/src/stitch/mod.rs`（删测试 mod 内已搬走的部分）
- Modify: `crates/capx/src/stitch/graybuf.rs`（加 `#[cfg(test)] mod tests`）
- Modify: `crates/capx/src/stitch/ncc_match.rs`（同）
- Modify: `crates/capx/src/stitch/canvas_heal.rs`（同）
- Modify: `crates/capx/src/stitch/fallback_chain.rs`（同）
- Modify: `docs/architecture.md`（capx 章节更新）
- Modify: `docs/features/screenshot.md`（如有 `stitch.rs:` 引用则改）

**Interfaces:** N/A（测试搬运，不改生产代码）

**关键不变量：**
- 每个测试不改断言、不改数据
- 跨模块测试 helper（`make_frame` 等）在 mod.rs 内 `pub(super)`
- 搬完后仍 `cargo test -p octopus-capx` = 49 passed

### 5A. 测试归类（精确清单）

| 测试 / helper | 现 mod.rs 行号 | 搬到 |
|---|---|---|
| `reference_feature_map` helper | 1438–1547 | graybuf.rs |
| `test_graybuf_color_pixel_luma` | 1691–1701 | graybuf.rs |
| `test_graybuf_matches_image_grayscale` | 1797–1812 | graybuf.rs |
| `test_sobel_pure_color_degrades` | 1891–1899 | graybuf.rs |
| `test_sobel_textured_has_features` | 1900–1912 | graybuf.rs |
| `test_ncc_matches_known_offset` | 2322–2334 | ncc_match.rs |
| `test_ncc_match_range_finds_known_offset` | 2335–2352 | ncc_match.rs |
| `test_ncc_match_range_rejects_out_of_range_offset` | 2353–2368 | ncc_match.rs |
| `test_two_stage_refine_preserves_subpixel` | 2369–2398 | ncc_match.rs |
| `test_sticky_detection` | 1676–1690 | canvas_heal.rs |
| `test_extract_canvas_bottom_gray` | 1865–1890 | canvas_heal.rs |
| `make_frame_dark_editor` helper | 1913–1932 | canvas_heal.rs（仅它用） |
| `test_dark_editor_moderate_density_ncc_works` | 1933–1951 | canvas_heal.rs |
| `test_dark_editor_bottom_strip_degrades_sobel` | 1952–1972 | canvas_heal.rs |
| `test_content_tail_black_bottom_still_stitches` | 2044–2091 | canvas_heal.rs |
| `test_detect_content_tail_frame_based` | 2092–2109 | canvas_heal.rs |
| `test_content_tail_updates_each_frame` | 2110–2134 | canvas_heal.rs |
| `test_short_selection_with_dark_tail_stitches` | 2135–2192 | canvas_heal.rs |
| `test_seed_dark_tail_trimmed_by_own_measurement` | 2193–2238 | canvas_heal.rs |
| `test_blank_seed_reseeded_from_content_frame` | 2239–2289 | canvas_heal.rs |
| `inject_constant_canvas_tail` test-only impl | 2270–2288 | canvas_heal.rs |
| `test_canvas_constant_tail_trimmed_mid_stream` | 2291–2321 | canvas_heal.rs |
| `test_fallback_expanded_search_range` | 1814–1826 | fallback_chain.rs |
| `test_fallback_1d_projection_low_texture` | 1827–1838 | fallback_chain.rs |
| `test_try_match_prev_frame_constant_strip_no_false_match` | 2029–2043 | fallback_chain.rs |
| `test_prev_frame_match_continuous_scroll` | 2399–2414 | fallback_chain.rs |
| `test_prev_frame_match_short_prev_returns_none` | 2415–2432 | fallback_chain.rs |
| `test_verify_alignment_2d_*`（6 个） | 2433–2520 | fallback_chain.rs |
| `test_fallback_1d_false_match_rejected_by_2d_verify` | 2521–2550 | fallback_chain.rs |
| `test_fallback_prev_frame_not_blocked_by_2d_verify` | 2551–2572 | fallback_chain.rs |
| `make_frame` helper | 1548–1574 | mod.rs（pub(super)） |
| `make_frame_textured` helper | 1575–1600 | mod.rs（pub(super)） |
| `make_frame_text_mixed` helper | 1601–1621 | mod.rs（pub(super)） |
| `make_frame_with_sticky` helper | 1644–1675 | mod.rs（pub(super)） |
| `canvas_bottom_strip` helper | 1622–1635 | mod.rs（pub(super)） |
| `verify_sample_cols` helper | 1636–1643 | mod.rs（pub(super)） |
| `test_stationary_frame_returns_false` | 1702–1712 | mod.rs |
| `test_known_scroll_appends_rows` | 1713–1735 | mod.rs |
| `test_scroll_direction_dy_negative` | 1736–1747 | mod.rs |
| `test_repeated_scroll_grows_canvas` | 1748–1766 | mod.rs |
| `test_canvas_returns_valid_rgba` | 1767–1780 | mod.rs |
| `test_finalize_appends_footer` | 1781–1796 | mod.rs |
| `test_canvas_anchored_recovers_after_failures` | 1839–1864 | mod.rs |
| `test_best_ncc_match_normal_frame_matched` | 1973–1989 | mod.rs |
| `test_best_ncc_match_solid_frame_no_panic` | 1990–2006 | mod.rs |
| `test_best_ncc_match_constant_canvas_strip_no_false_match` | 2007–2028 | mod.rs |

- [ ] **Step 1: 在 mod.rs 测试 mod 内把 helper 升 pub(super)**

打开 `stitch/mod.rs`，在 `#[cfg(test)] mod tests { ... }` 内，把跨模块 helper 前加 `pub(super)`：

```rust
pub(super) fn make_frame(width: u32, height: u32, scroll_offset: u32) -> RgbaImage { ... }
pub(super) fn make_frame_textured(...) -> RgbaImage { ... }
pub(super) fn make_frame_text_mixed(...) -> RgbaImage { ... }
pub(super) fn make_frame_with_sticky(...) -> RgbaImage { ... }
pub(super) fn canvas_bottom_strip(...) -> GrayBuf { ... }
pub(super) fn verify_sample_cols(...) -> Vec<usize> { ... }
```

`cfg(test)` 下 `pub(super)` 只在测试编译期可见、外部不可见。

- [ ] **Step 2: 搬 graybuf.rs 测试**

在 graybuf.rs 末尾加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{make_frame, /* 按需导 */};
```

从 mod.rs 的 `mod tests` 中**剪切**：`reference_feature_map` / `test_graybuf_color_pixel_luma` / `test_graybuf_matches_image_grayscale` / `test_sobel_pure_color_degrades` / `test_sobel_textured_has_features`（按 5A 表）。

粘到 graybuf.rs 的 `mod tests` 内。如果测试用 `make_frame`，在 use 语句里 import：`use super::super::make_frame;`（从 mod.rs 测试模块拿——前提是 Step 1 已 pub(super)）。

实际验证：Read graybuf.rs 的测试用到了哪些 helper，按需 import。

- [ ] **Step 3: 搬 ncc_match.rs 测试**

在 ncc_match.rs 末尾加 `#[cfg(test)] mod tests { use super::*; ... }`，从 mod.rs 剪切 4 个 NCC 测试（`test_ncc_matches_known_offset` 等）粘进去。

- [ ] **Step 4: 搬 canvas_heal.rs 测试**

在 canvas_heal.rs 末尾加 `#[cfg(test)] mod tests { use super::*; ... }`。从 mod.rs 剪切所有 canvas_heal 相关测试 + `make_frame_dark_editor` helper + `inject_constant_canvas_tail` test-only impl。

`inject_constant_canvas_tail` 是测试 only 的 inherent method：

```rust
#[cfg(test)]
impl super::super::Stitcher {
    pub(super) fn inject_constant_canvas_tail(&mut self, ...) { ... }
}
```

注意它必须留在 cfg(test) 下（不能进生产）。

- [ ] **Step 5: 搬 fallback_chain.rs 测试**

在 fallback_chain.rs 末尾加 `#[cfg(test)] mod tests { use super::*; ... }`。从 mod.rs 剪切所有 fallback 相关测试（按 5A 表）。

- [ ] **Step 6: 编译验证**

Run:
```bash
cargo build -p octopus-capx 2>&1 | tail -30
```

Expected: 0 error 0 warning。

典型 error：
- `make_frame not found` → helper 没 pub(super) / use 没引对
- `inject_constant_canvas_tail not found` → test-only impl 没正确放 cfg(test) 下

- [ ] **Step 7: 测试验证（关键）**

Run:
```bash
cargo test -p octopus-capx 2>&1 | tail -5
```

Expected: `test result: ok. 49 passed; 0 failed; 0 ignored`。

**如果数字 < 49**：测试漏搬了。回看 5A 表逐项核对。
**如果数字 > 49**：重复粘贴了。回看哪个测试在两个文件都有。
**如果有 fail**：测试搬错了模块（如 NCC 测试搬到 canvas_heal 但没引 ncc_match use）。

- [ ] **Step 8: 更新 docs/architecture.md**

打开 `docs/architecture.md`，找到 capx 章节（约 215 行附近）。把 stitch 表项的描述更新——从"单文件"改为"`stitch/` 目录 5 文件"，补一句话各述：

例：
```markdown
| `stitch` | 滚动截屏拼接引擎：**Canvas-Anchored NCC + Sobel 梯度匹配**。模块结构（2026-08-04 拆分）：
  - `stitch/mod.rs`：Stitcher struct + 编排（process_frame/finalize）+ read API
  - `stitch/graybuf.rs`：GrayBuf 灰度 buffer + to_feature_map（Sobel + Welford）
  - `stitch/ncc_match.rs`：NCC 引擎原语（ncc_match / validate / parabolic_refine）
  - `stitch/fallback_chain.rs`：五层降级链（NCC → 邻帧 → 1D → 静止 → best-guess）
  - `stitch/canvas_heal.rs`：六个自愈机制（content_tail / constant trim / reseed / sticky） |
```

保留原描述里的算法细节（Canvas-Anchored、Sobel 自写、两阶段 refine、六轮迭代等），仅更新文件结构部分。

- [ ] **Step 9: 查 docs/features/screenshot.md**

Run:
```bash
rg "stitch\.rs" docs/features/screenshot.md docs/ 2>&1
```

如有 `stitch.rs:line` 引用，更新为 `stitch/<file>.rs:line`。如无，跳过。

- [ ] **Step 10: Commit 测试搬运**

```bash
git add -A
git commit -m "refactor(capx): 测试跟随各自模块搬出

- graybuf.rs: graybuf/sobel 对照测试 + reference_feature_map helper
- ncc_match.rs: NCC 引擎原语测试（4 个）
- canvas_heal.rs: 6 自愈机制测试 + make_frame_dark_editor + test-only impl
- fallback_chain.rs: 5 层降级链测试 + verify_alignment_2d 测试
- mod.rs 保留：跨模块集成测试（process_frame 端到端/best_ncc_match）+ 跨模块 helper（make_frame 等）升 pub(super)

零行为变更：cargo test -p octopus-capx → 49 passed"
```

- [ ] **Step 11: 下游编译验证**

Run:
```bash
cargo build -p octopus-desktop 2>&1 | tail -10
```

Expected: 0 error 0 warning（确认公开 API 没破坏下游）。如有 error，公开 API 改动了——但本 plan 全程零公开 API 改动，不应该出现。如真有，是 dev feature 路径问题，单独看。

- [ ] **Step 12: clippy 全量验证**

Run:
```bash
cargo clippy -p octopus-capx --all-targets 2>&1 | tail -10
```

Expected: 0 warning。仓库有 `clippy::all` gate，任何 warning 都要处理。

- [ ] **Step 13: 最终 commit（文档同步）**

```bash
git add docs/architecture.md docs/features/screenshot.md
git commit -m "docs(capx): 同步 stitch 拆分后的模块结构

- architecture.md §capx：stitch 表项更新为 5 文件结构
- screenshot.md：如有 stitch.rs: 引用则更新路径（如无则不动）"
```

- [ ] **Step 14: 拆分最终验证（成功标准全部勾掉）**

按 spec §8 成功标准全跑一遍：

```bash
# 1. 测试数量
cargo test -p octopus-capx 2>&1 | tail -3
# Expected: 49 passed

# 2. 下游编译
cargo build -p octopus-desktop 2>&1 | tail -3
# Expected: Finished 0 error

# 3. clippy
cargo clippy -p octopus-capx --all-targets 2>&1 | tail -3
# Expected: 0 warning

# 4. 文件结构
ls -la crates/capx/src/stitch/
wc -l crates/capx/src/stitch/*.rs
# Expected: mod.rs graybuf.rs ncc_match.rs fallback_chain.rs canvas_heal.rs，每文件 ≤ 600 行

# 5. diff 只是 move
git diff main..HEAD --stat
# Expected: stitch.rs 删除、stitch/*.rs 新增，每文件纯 add（无大段生产代码 del——只有 mod.rs 有 delete 因为搬出去了）
```

如全部通过，**拆分完成**。报告用户。

---

## 完成后

向用户报告：
- 5 文件拆分完成
- `cargo test -p octopus-capx` = 49 passed（行为等价证明）
- 下游 + clippy 全过
- 文件行数对比表
- 阶段 2/3 候选项（留作独立 brainstorming）：finalize dedupe / extract_strip helper / 常量分组 / trait 抽象

**未经用户明确指令不 push 到 main**（worktree 纪律）。
