# stitch.rs 拆分设计（阶段 1：纯机械拆分）

- 日期：2026-08-04
- 分支：`refactor/stitch-split`
- Worktree：`.worktrees/refactor-stitch-split`
- 类型：重构（纯结构调整，零行为变更）
- Baseline：`cargo test -p octopus-capx` → **49 passed; 0 failed**（main 1fdbb6d5）

---

## 1. 背景与动机

`crates/capx/src/stitch.rs` 已膨胀至 **123KB / 2572 行**，承载滚动截屏拼接引擎的全部职责：

| 区段 | 行号 | 行数 | 占比 |
|---|---|---|---|
| 模块常量 | 5–37 | 33 | 1% |
| GrayBuf + to_feature_map | 39–174 | 136 | 5% |
| NCC 引擎（free fn） | 176–313 | 138 | 5% |
| StitchConfig + Stitcher struct | 315–377 | 63 | 2% |
| impl Stitcher | 379–1417 | **1039** | **40%** |
| rows_equal_buf | 1420–1428 | 9 | <1% |
| mod tests | 1430–2572 | **1143** | **44%** |

`impl Stitcher` 内聚集了 **5 层降级链 + 6 个自愈机制**，经 6 轮迭代修复（commit `a48aaeb5` 标题明示）已接近临界复杂度，新人接手门槛大，后续迭代成本攀升。

### 1.1 五层降级链（fallback chain）

主入口 `process_frame_inner`（513）在主 NCC 匹配失败时按序降级：

| # | 层 | 位置 | 机制 |
|---|---|---|---|
| 1 | 主 NCC 匹配 | `best_ncc_match`（692）→ `process_frame_inner`（523） | Sobel-then-gray NCC；stuck 计数（`ncc_stuck_count >= 5` → 视为静止） |
| 2 | 邻帧参考 NCC | `try_match_prev_frame`（741）← `try_fallback`（806） | 用前一帧底条带做模板的 NCC；verify=false |
| 3 | 1D 行投影 SAD | `try_match_1d_projection`（1059）← `try_fallback`（815） | 行均值 SAD 搜索；内部含静止检测 |
| 4 | 静止检测 | `quick_stationary_check`（968）← `try_fallback`（824） | 逐像素 SAD vs canvas strip，`< STATIONARY_SAD` → 跳过 |
| 5 | Best-guess | `estimate_dy_hint`（1040）← `try_fallback`（835） | dy_history 中位数；streak < 3 才用 |

dispatcher：`try_fallback`（787，`&mut self`）。共享出口：`apply_fallback_match`（1148）——dy 校验 + 可选 2D verify（`verify_alignment_2d` 993）+ canvas append + history 更新。

### 1.2 六个自愈机制（canvas heal）

| # | 机制 | 位置 | 作用 |
|---|---|---|---|
| 1 | seed-tail 修剪 | `process_frame`（427–435） | 首帧的暗尾不入 canvas seed |
| 2 | 每帧 content_tail 检测 | `detect_content_tail`（1359）/ `scan_content_tail_in`（1371） | 动态裁暗尾，eff_bottom 停在真实内容 |
| 3 | canvas 底部常数检测 + 非破坏性修剪 | `canvas_bottom_constant`（877）/ `scan_canvas_constant_tail`（909）/ trim（471–486） | 每帧若 canvas 底条常数则修剪（保留 ≥ MIN_STRIP） |
| 4 | 破坏性重播种 | `reseed_canvas_from`（939） | canvas 几乎全常数（无内容）时用当前帧重置 |
| 5 | 自适应 strip_h | `effective_strip_for`（1411） | 内容短时缩 strip_h，保 NCC 搜索范围为正 |
| 6 | sticky 区检测 | `detect_sticky`（1324）/ `rows_equal_buf`（1420） | 一次检测固定顶/底栏，从 eff_top/eff_bottom 排除 |

### 1.3 长期维护痛点

- 单文件 123KB，IDE 索引、git diff review、git blame 追溯都吃力
- 五层降级链 + 六自愈机制交织在同一 `impl` 块，读代码须反复跳行号
- 测试 1143 行与生产代码混在同一文件，文件结构不反映职责分层
- 新人接手须通读全文才能定位单一致命点（如"调静止阈值改哪里"）

---

## 2. 目标与非目标

### 2.1 目标（本次必达）

1. 把 `stitch.rs` 拆为 `stitch/` 子目录 5 个文件 + mod.rs，每文件 ≤ 600 行
2. **零行为变更**——`cargo test -p octopus-capx` 通过且测试数量与拆分前完全一致（49 passed）
3. 公开 API 签名（`pub fn`/`pub struct` 字段类型/方法签名）一字不改
4. `mod` 路径不变：`crate::stitch::Stitcher` / `octopus_capx::stitch::Stitcher` 保持原样
5. 下游 `cargo build -p octopus-desktop` 不破坏
6. `cargo clippy -p octopus-capx --all-targets` 0 warning（仓库 `clippy::all` gate）

### 2.2 非目标（本次显式不做）

- ❌ 不抽 `trait FallbackStep`（动核心控制流）
- ❌ 不动任何阈值/常量值（9 个常量原样搬）
- ❌ 不做 `finalize`:1277–1283 NCC dedupe
- ❌ 不抽 `fn extract_strip` helper（消除 `try_match_prev_frame` inline 切片）
- ❌ 不改 `process_frame` / `process_frame_inner` 控制流
- ❌ 不加队列解耦（research doc 借鉴 A，独立大需求）
- ❌ 不加双向滚动（research doc 借鉴 C，独立大需求）
- ❌ 不改 NCC/Sobel/特征提取算法
- ❌ 不改任何测试数据/断言值
- ❌ 不删任何测试

### 2.3 阶段化路线

| 阶段 | 内容 | 风险 | 状态 |
|---|---|---|---|
| **1** | 纯机械拆分 + 可见性调整 | 极低 | ✅ 完成（2026-08-04，merge `10f7d211`） |
| **2** | finalize dedupe / extract_strip helper / 常量分组 | 低 | 🟡 部分完成（2026-08-04，见下） |
| 3（远期） | 降级链 trait 抽象（`FallbackStep`）/ 队列解耦 / 双向滚动 | 中高 | 独立 brainstorming |

阶段 1 的 `pub(crate)` 可见性不阻碍阶段 2/3——dedupe、抽 helper、trait 化都在已可见范围内。

**阶段 2 实施记录（2026-08-04，分支 `refactor/stitch-cleanup`）**：
- ✅ 常量按功能注释分组（4 组：匹配阈值 / 采样几何 / 画布自愈 / 时序平滑），顺手修 `DY_HISTORY_LEN` doc 被前一行吞的小 bug
- ✅ 抽 `GrayBuf::bottom_strip(strip_h)` helper，消除 `try_match_prev_frame` 手工构造 GrayBuf 的重复切片；+3 单测覆盖 normal / exceeds_height / zero
- ❌ **finalize NCC dedupe 跳过**——读码后发现 finalize 与 `best_ncc_match` 控制流差异比本 spec 预期大：
  - `best_ncc_match`：双侧 Sobel 退化时**不兜底返 Mismatch**（避免常数模板 NCC 假匹配≈1.0）
  - `finalize`：双侧 Sobel 退化时**仍走灰度兜底**（历史遗留，早于 best_ncc_match 的退化规则建立）
  - 强行 dedupe 要么引入复杂参数化（`enum GrayFallbackStrategy { OnDegenerate, OnMismatch }`），要么变更行为。**保留现状**——这是设计差异，非简单重复

---

## 3. 模块布局

### 3.1 目标结构

```
crates/capx/src/
├── lib.rs                # 不动（pub mod stitch; pub mod capture;）
├── capture.rs            # 不动
└── stitch/
    ├── mod.rs            # ~600 行：struct + 编排 + 公开 API + 跨模块集成测试
    ├── graybuf.rs        # ~280 行：GrayBuf + to_feature_map + row_projection_means
    ├── ncc_match.rs      # ~300 行：NccResult/PrimaryOutcome + ncc_match family + NCC 测试
    ├── fallback_chain.rs # ~500 行：try_fallback + 5 try_*/verify_* + fallback 测试
    └── canvas_heal.rs    # ~350 行：6 自愈机制 + detect_sticky + reseed + heal 测试
```

### 3.2 各文件内容（按 stitch.rs 现有行号）

| 文件 | 现有行号 | 关键内容 |
|---|---|---|
| `mod.rs` | 5–37, 315–377, 379–735, 1208–1322, 1420–1428 | 模块常量；`StitchConfig`/`Stitcher` struct；`new`/`process_frame`/`process_frame_inner`/`primary_ncc`/`best_ncc_match`/`finalize`；公开 read API（`canvas`/`height`/`into_canvas`）；`rows_equal_buf`；跨模块集成测试 |
| `graybuf.rs` | 39–174, 304–313 | `struct GrayBuf` + `from_rgba_roi`/`row`/`to_gray_image`；`to_feature_map`（Sobel + Welford）；`row_projection_means`；feature map 对照测试（原 1463–1547） |
| `ncc_match.rs` | 176–313 | `NccResult`；`PrimaryOutcome`；`ncc_match`；`downsample_grayimage`；`ncc_match_range`；`validate_ncc_match`；`parabolic_refine_from_response`；NCC 引擎单元测试（原 2321–2516） |
| `fallback_chain.rs` | 741–846, 993–1206 | `try_fallback`（dispatcher）；`try_match_prev_frame`；`try_match_1d_projection`；`apply_fallback_match`；`verify_alignment_2d`；`estimate_dy_hint`；fallback 路径测试（原 1813–1890, 2432–2556） |
| `canvas_heal.rs` | 848–964, 1324–1416 | `extract_canvas_bottom_gray`；`canvas_bottom_constant`/`scan_canvas_constant_tail`；`reseed_canvas_from`；`detect_sticky`；`detect_content_tail`/`scan_content_tail_in`；`effective_strip_for`；`invalidate_cache`；canvas heal / content_tail / sticky 测试（原 1675–1812, 2043–2269, 2290–2320） |

### 3.3 关键决策点（为什么这样分）

1. **`primary_ncc`/`best_ncc_match` 留 mod.rs，不挪 ncc_match.rs**：它们是 `&self` 方法、读 `config.ncc_downsample_width`/`config.ncc_score_threshold`、被 `process_frame_inner` 和 `finalize` 调用——属于编排层。ncc_match.rs 只放**纯 free function** 的 NCC 引擎原语。
2. **`extract_canvas_bottom_gray` 归 canvas_heal.rs**：它读 `self.canvas_buf`、维护画布锚点，是 heal 职责的一部分。代码地图证实**没有独立的"strip 抽取层"存在**——`GrayBuf::from_rgba_roi` 在 util 层，`try_match_prev_frame` 内的切片是 inline 的，`extract_canvas_bottom_gray` 是单方法。
3. **`apply_fallback_match` 归 fallback_chain.rs**：它是 fallback 链"出口"（dy 校验 + 可选 2D verify + canvas append + history 更新），与 `try_fallback` dispatcher 紧耦合。
4. **`invalidate_cache` 归 canvas_heal.rs**：所有改 `canvas_buf` 的 heal 操作都要调它，跟随 heal。
5. **常量集中 mod.rs 顶部**：被 fallback/canvas/ncc 多模块引用，子模块通过 `use super::*` 拿到。常量是跨模块共享的、不该散落到子模块里。

---

## 4. 可见性与 split-impl 机制

### 4.1 可见性策略：宽 pub(crate)

| 子模块 | `pub(crate)` 导出 | 备注 |
|---|---|---|
| `graybuf.rs` | `GrayBuf`、`to_feature_map`、`row_projection_means` | 被所有 4 个兄弟模块 + mod.rs 用 |
| `ncc_match.rs` | `NccResult`、`PrimaryOutcome`、`ncc_match`、`ncc_match_range`、`validate_ncc_match`、`parabolic_refine_from_response`、`downsample_grayimage` | 被 mod.rs 的 `primary_ncc`/`best_ncc_match` + fallback_chain 的 `try_match_prev_frame` 用 |
| `fallback_chain.rs` | 无新增导出 | 所有 `try_*` 保持为 `impl Stitcher` 的 inherent method（split-impl） |
| `canvas_heal.rs` | 无新增导出 | 同上 |

**对 mod.rs 里的 `Stitcher` struct 字段**：加 `pub(crate)`（不改类型、不改默认值）。这是 split-impl 在兄弟文件中能 `self.canvas_buf` 的前提。

`pub(crate)` 宽暴露面理由：capx 是独立 crate，外部消费者只有 octopus-desktop；阶段 2/3 改动会频繁跨文件调用这些项，初期宽、后续按需收紧，避免反复改可见性。

### 4.2 Split-impl 写法

Rust 允许同一 type 的 `impl` 块散布在多个文件（同 crate 内）。这是本次拆分的核心技术手段：

```rust
// crates/capx/src/stitch/mod.rs
mod graybuf;
mod ncc_match;
mod fallback_chain;
mod canvas_heal;

pub use graybuf::{GrayBuf, to_feature_map, row_projection_means};   // 对外保持原 pub 路径
pub(crate) use ncc_match::{ncc_match_range, PrimaryOutcome, NccResult};

use graybuf::{GrayBuf, to_feature_map};
use ncc_match::{ncc_match_range, PrimaryOutcome};

pub struct Stitcher {
    pub(crate) canvas_buf: ...,
    pub(crate) canvas_h: u32,
    pub(crate) config: StitchConfig,
    // ... 其余字段全 pub(crate)
}

impl Stitcher {
    // new / process_frame / process_frame_inner / primary_ncc / best_ncc_match / finalize / read API
}

// crates/capx/src/stitch/fallback_chain.rs
use super::*;   // 拿到常量、Stitcher、GrayBuf、ncc_match family

impl super::Stitcher {
    pub(crate) fn try_fallback(&mut self, ...) -> ... { ... }
    pub(crate) fn try_match_prev_frame(&self, ...) -> ... { ... }
    // ... 5 个 try_*/verify_*/estimate
}
```

### 4.3 `use super::*` 的语义

子模块顶部 `use super::*;` 会带入：
- mod.rs 顶部的所有模块常量（`STATIONARY_SAD` 等 9 个）
- mod.rs 里 `use` 进来的 `GrayBuf`/`to_feature_map`/`ncc_match_range` 等

这让 fallback_chain.rs / canvas_heal.rs 的代码**逐字照搬**、无需任何符号改名。这是"零行为变更"能落地的关键技巧。

### 4.4 测试 helper 可见性

| helper | 去向 | 可见性 |
|---|---|---|
| `make_frame`/`make_frame_textured`/`make_frame_text_mixed` | mod.rs 测试模块 | `pub(super)`（cfg(test) 内） |
| `canvas_bottom_strip`/`verify_sample_cols` | canvas_heal.rs 测试模块 | 本地 |
| `make_frame_with_sticky`/`make_frame_dark_editor` | mod.rs 或 canvas_heal.rs（按主用方） | `pub(super)` |
| `reference_feature_map` | graybuf.rs 测试模块 | 本地 |
| 测试 only `impl Stitcher { inject_constant_canvas_tail }` | canvas_heal.rs 测试 | 本地 |

跨模块测试需要的 helper 走 `pub(super)`（cfg(test) 下，外部不可见）。

---

## 5. 迁移步骤

### 5.1 总原则

一次拆一个、每步可独立验证、可随时停。每个子模块作为一个独立 commit，每个 commit 后 `cargo test -p octopus-capx` 全过。

### 5.2 迁移顺序（从低风险到高风险）

| 步骤 | 内容 | 风险 | 为何此序 |
|---|---|---|---|
| **0. 准备** | baseline 测试已记录（49 passed）；worktree `refactor/stitch-split` 就绪 | — | 安全网 |
| **1. graybuf.rs** | 移 `GrayBuf` + `to_feature_map` + `row_projection_means`；mod.rs `mod graybuf; use graybuf::*;`；公开项 `pub use`；移对应测试 | 极低 | 全是 free function/struct，无 `&self`；被所有层用，先拆出来后续步骤都能引用 |
| **2. ncc_match.rs** | 移 NCC 引擎（`NccResult`/`PrimaryOutcome`/5 个 free fn）；`PrimaryOutcome` 升 `pub(crate)`；移 NCC 测试 | 低 | 全是 free function；依赖只到 `image`/`imageproc` |
| **3. canvas_heal.rs** | 移 6 自愈机制方法（split-impl 首次引入）；Stitcher 字段升 `pub(crate)`；移 heal/sticky/content_tail 测试 + test-only `impl Stitcher` 块 | 中 | split-impl 首次；要先升 `pub(crate)` 字段，编不过就立即发现 |
| **4. fallback_chain.rs** | 移 fallback 方法（split-impl）；移 fallback 测试 | 中 | 依赖 canvas_heal 的 `extract_canvas_bottom_gray` 等 |
| **5. 收尾** | mod.rs 此时剩 ~600 行：struct + 编排 + read API + 跨模块集成测试；更新 `docs/architecture.md` + `docs/features/screenshot.md`；下游 + clippy 验证 | 低 | 全部代码已到位，文档同步 |

**step 1→2 顺序说明**：graybuf 先于 ncc_match 是因为 graybuf 是基础层、拆出来后 mod.rs 里剩下的 free function 更少、更清晰。ncc_match 不直接依赖 graybuf（输入参数是 `GrayImage` 不是 `GrayBuf`），二者可互换。

### 5.3 每步标准操作流程（SOP）

```
1. 新建 crates/capx/src/stitch/<name>.rs
2. 从 stitch.rs（将变成 mod.rs）逐字剪贴对应行段到新文件
3. 新文件顶部加：
   use super::*;              // 拿常量、Stitcher、GrayBuf 等
4. mod.rs（原 stitch.rs）顶部加：
   mod <name>;                // 私有 mod
   pub(crate) use <name>::*;  // 对 crate 内重导出（按需）
5. cargo test -p octopus-capx  →  期望 49 passed
```

**每步不变量**：
- ✅ `cargo build -p octopus-capx` 0 error 0 warning
- ✅ `cargo test -p octopus-capx` 全过、数量不减（49）
- ✅ `git diff --stat` 应只显示 move（rename）、不应有大段 add/del
- ✅ 公开 API 签名（`pub fn`/`pub struct` 字段类型/方法签名）一字不改

### 5.4 搬法示例（以 step 2 ncc_match.rs 为例）

**前**（stitch.rs 176–313）：
```rust
struct NccResult { ... }   // 私有
enum PrimaryOutcome { ... } // 私有
fn ncc_match(...) -> NccResult { ... }
fn downsample_grayimage(...) -> image::GrayImage { ... }
fn ncc_match_range(...) -> NccResult { ... }
fn validate_ncc_match(...) -> Option<NccResult> { ... }
fn parabolic_refine_from_response(...) -> f64 { ... }
```

**后**（ncc_match.rs）：
```rust
use super::*;

pub(crate) struct NccResult { ... }
pub(crate) enum PrimaryOutcome { ... }
pub(crate) fn ncc_match(...) -> NccResult { ... }
pub(crate) fn downsample_grayimage(...) -> image::GrayImage { ... }
pub(crate) fn ncc_match_range(...) -> NccResult { ... }
pub(crate) fn validate_ncc_match(...) -> Option<NccResult> { ... }
pub(crate) fn parabolic_refine_from_response(...) -> f64 { ... }

#[cfg(test)]
mod tests {
    use super::*;
    // 原 stitch.rs 2321–2516 的 NCC 测试逐字搬来
}
```

**后**（stitch/mod.rs 顶部）：
```rust
mod graybuf;
mod ncc_match;
use graybuf::{GrayBuf, to_feature_map, row_projection_means};
use ncc_match::{ncc_match_range, PrimaryOutcome, NccResult};
```

**唯一改动**：6 处 `struct/enum/fn` 前加 `pub(crate)`。零行为变更。

---

## 6. 验证策略（evidence before assertions）

| 层级 | 命令 | 何时跑 | 通过标准 |
|---|---|---|---|
| **编译** | `cargo build -p octopus-capx` | 每步 | 0 error 0 warning |
| **单元测试** | `cargo test -p octopus-capx` | 每步 | 全过、**49 passed**（baseline） |
| **下游编译** | `cargo build -p octopus-desktop` | step 5 | 0 error（确认公开 API 没破坏） |
| **clippy** | `cargo clippy -p octopus-capx --all-targets` | step 5 | 0 warning（仓库 `clippy::all` gate） |
| **行为等价证明** | `git diff` review | 每步 | 只 move 不 change |

**行为等价的客观证据**：拆分前后 `cargo test -p octopus-capx 2>&1 | tail -3` 对比测试数量，都是 **49 passed** 即等价证明。任何测试失败 = 行为变了 = 拆错了。

---

## 7. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| **split-impl 字段可见性漏改** | 中 | 编译失败（易发现） | step 3 升 `pub(crate)` 时一次性改全；编译器会列出所有遗漏字段 |
| **`use super::*` 符号冲突** | 低 | 编译失败 | 本次纯 move、不新增同名 item，冲突面为零；如出现按编译器提示改名即可 |
| **测试 helper 跨模块引用断裂** | 中 | 测试编译失败 | helper 跟主用方走；跨模块的加 `pub(super)`（cfg(test) 下） |
| **git blame 历史断裂** | 确定 | 中（追溯变难） | 纯机械拆分固有代价；commit message 标注"from stitch.rs L<起>-L<止>"；`git log --follow` 对 mod.rs 仍可追溯主文件历史 |
| **文档路径引用失效** | 确定 | 低 | `architecture.md` / `screenshot.md` 里 `stitch.rs:line` 引用在 step 5 统一更新 |
| **下游编译破坏** | 低 | 高 | 公开 API 一字不改；step 5 跑 `cargo build -p octopus-desktop` 验证 |
| **6 轮自愈逻辑被误碰** | 低 | 极高 | 硬约束"零行为变更"；每步 `cargo test` 数量对比作行为等价证明 |

---

## 8. 成功标准（可客观验证）

拆分完成的判定——全部为机械性证据，无主观判断：

1. `cargo test -p octopus-capx`：通过、测试数量 **49**（与 baseline 一致）
2. `cargo build -p octopus-desktop`：0 error（下游不破坏）
3. `cargo clippy -p octopus-capx --all-targets`：0 warning（仓库 `clippy::all` gate）
4. `git diff main..HEAD --stat`：只显示 rename/move，`stitch.rs` 删除、`stitch/*.rs` 新增
5. `wc -l crates/capx/src/stitch/*.rs`：每个文件 ≤ 600 行，最大的 fallback_chain.rs ~500 行
6. 公开 API 审查：`pub fn`/`pub struct`/方法签名零变更

---

## 9. 文档同步（收尾必做）

| 文档 | 更新内容 |
|---|---|
| `docs/architecture.md` §capx | `stitch` 表项更新：从"单文件 123KB"改为"`stitch/` 目录 5 文件"，补模块职责一句话各述 |
| `docs/features/screenshot.md` | 如有 `stitch.rs:line` 引用则更新路径 |
| `docs/superpowers/specs/2026-08-04-stitch-refactor-design.md` | 本 spec 本身（含拆分前后结构、行号映射、验证证据） |

---

## 10. 关联文档

- [research/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md](research/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md) — snow-shot 对照，列出阶段 3 候选借鉴（队列解耦 / 双向滚动 / 主次比判据）
- `docs/architecture.md` §octopus-capx — 现状描述（待 step 5 更新）
- `docs/features/screenshot.md` — 截图功能用户视角文档（待 step 5 查路径引用）
- 近期 commit 链：`a48aaeb5`（六轮修复）/ `7cb9bb6c`（邻帧参考）/ `e6245003`（2D 验证）/ `a3d8cf3b`（Sobel+Welford）——本拆分不碰这些逻辑
