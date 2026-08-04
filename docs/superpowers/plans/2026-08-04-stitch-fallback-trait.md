# 降级链 trait 抽象实施计划（阶段 3）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `try_fallback` 的 60 行 if 链重写为迭代 5 个 `FallbackStep` trait 实现的 dispatcher，零行为变更。

**Architecture:** trait + enum + ctx 模式。5 个 ZST impl 封装各步匹配逻辑 + 副作用。改动局限在 `fallback_chain.rs` 单文件。

**Tech Stack:** Rust 2021（无新依赖）。

## Global Constraints

- **硬约束：零行为变更**——`cargo test -p octopus-capx` = **49 passed**（baseline）
- **不改任何阈值/常量值**
- **公开 API 签名零变更**——`Stitcher::process_frame` / `finalize` / `try_fallback` 签名一字不改
- **不动 mod.rs / graybuf.rs / ncc_match.rs / canvas_heal.rs**（除可见性必要调整）
- **Worktree**：`.worktrees/refactor-stitch-trait` 分支 `refactor/stitch-trait`
- **TDD 优先**：每个 step 先写测试再写实现
- **每 Task 独立 commit** + `cargo test -p octopus-capx` 通过

---

## File Structure

只动一个文件：`crates/capx/src/stitch/fallback_chain.rs`（588 行 → 预估 700+ 行）。

新增内容：
- `enum StepOutcome`（4 变体）
- `struct FallbackCtx<'a>`（步骤上下文）
- `trait FallbackStep`（try_step + name）
- 5 个 ZST impl：`PrevFrameStep` / `Projection1DStep` / `StationaryStep` / `BestGuessStep` / `SkipStep`
- 重写 `try_fallback` dispatcher（迭代数组）
- 每个 step 的单元测试（3 场景：成功/Skip/边界）

保留不动：
- `try_match_prev_frame` / `try_match_1d_projection` / `quick_stationary_check` / `verify_alignment_2d` / `estimate_dy_hint` / `apply_fallback_match`（这些是 step impl 内部调用的 helper，保留为 Stitcher 方法）
- 现有所有测试（`test_fallback_*` / `test_verify_alignment_2d_*` 等回归测试不动）

---

## Task 1: trait 骨架 + StepOutcome + FallbackCtx（空 dispatcher）

**Files:**
- Modify: `crates/capx/src/stitch/fallback_chain.rs`

**Interfaces:**
- Produces: `StepOutcome` enum / `FallbackCtx` struct / `FallbackStep` trait（仅定义，尚无实现）
- 调整：现有 `try_fallback` 方法暂时保留不动（下个 task 替换）

- [x] **Step 1: 在 fallback_chain.rs 顶部加 trait 骨架**

在 `use super::*;` 之后、`impl super::Stitcher {` 之前插入：

```rust
// ===== 降级链 trait 抽象（2026-08-04 阶段 3 重构）=====

/// 降级链单步的输出。dispatcher 据此决定链路走向。
pub(crate) enum StepOutcome {
    /// 本步已应用（副作用 + apply_fallback_match 已在 step 内调用）。
    Applied(Result<bool>),
    /// 本步求出 dy 但未 apply，请求 dispatcher 走 apply_fallback_match(verify)。
    /// 保留扩展点——本次所有步骤都用 Applied。
    Candidate {
        dy: f64,
        confidence: f64,
        sad: f64,
        verify: bool,
    },
    /// 本步判定画面静止，链路应短路返回 Ok(false)。
    Stationary,
    /// 本步未匹配，继续下一步。
    Skip,
}

/// 步骤执行上下文。聚合步骤所需输入 + Stitcher 可变引用。
/// 显式列出字段，限制 step 只能触与本步相关的输入。
pub(crate) struct FallbackCtx<'a> {
    pub stitcher: &'a mut Stitcher,
    pub frame: &'a RgbaImage,
    pub curr_gray: &'a GrayBuf,
    pub canvas_gray: &'a GrayBuf,
    pub w: u32,
    pub eff_top: u32,
    pub eff_bottom: u32,
    pub sample_cols: &'a [usize],
}

/// 降级链单步。每个实现封装一种 fallback 策略 + 其副作用。
pub(crate) trait FallbackStep {
    /// 步骤名（日志用）。
    fn name(&self) -> &'static str;
    /// 尝试本步降级。
    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome;
}
```

- [x] **Step 2: cargo build 验证编译通过**

```bash
cargo build -p octopus-capx 2>&1 | tail -5
```

Expected: 0 error 0 warning（新类型未使用，但定义合法）。如出 "unused" warning 不用管——下个 task 会用。

- [x] **Step 3: cargo test 确认未破坏现有行为**

```bash
cargo test -p octopus-capx 2>&1 | grep "test result" | head -2
```

Expected: 49 passed。

- [x] **Step 4: Commit**

```bash
git add crates/capx/src/stitch/fallback_chain.rs
git commit -m "refactor(capx): 加 FallbackStep trait 骨架（阶段 3 task 1）

- enum StepOutcome（Applied/Candidate/Stationary/Skip）
- struct FallbackCtx（步骤上下文）
- trait FallbackStep（try_step + name）
- 仅定义，未接入 dispatcher

零行为变更：cargo test -p octopus-capx → 49 passed"
```

---

## Task 2: PrevFrameStep（TDD——首个 step，建立模式）

**Files:**
- Modify: `crates/capx/src/stitch/fallback_chain.rs`

**Interfaces:**
- Produces: `struct PrevFrameStep;` + `impl FallbackStep for PrevFrameStep`
- 调用：`ctx.stitcher.try_match_prev_frame(...)` + `ctx.stitcher.apply_fallback_match(verify=false)` + reset `best_guess_streak=0` + `ncc_stuck_count=0`

**封装现有 dispatcher 行为（55-114 行的 if 块）：**
```rust
if let Some(prev_gray) = &self.prev_gray {
    if let Some(dy) = self.try_match_prev_frame(prev_gray, curr_gray, eff_top, eff_bottom) {
        self.best_guess_streak = 0;
        self.ncc_stuck_count = 0;
        return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, canvas_gray, &sample_cols, false, w, eff_top, eff_bottom);
    }
}
```

- [x] **Step 1: 写测试（RED）——PrevFrameStep 成功场景**

在 fallback_chain.rs 的 `mod tests` 内加：

```rust
#[test]
fn prev_frame_step_applied_on_match() {
    // 构造：prev_gray 与 curr_gray 有重叠内容，try_match_prev_frame 能匹配
    // 用 make_frame_textured 构造连续滚动序列
    let f0 = make_frame_textured(TW, TH, 0, 5);
    let mut stitcher = Stitcher::new(f0.clone(), StitchConfig::default());
    let f1 = make_frame_textured(TW, TH, 0, 5);
    stitcher.process_frame(&f1).unwrap(); // init + 设置 prev_gray
    let f2 = make_frame_textured(TW, TH, 30, 5);
    // 先 process 一次让 prev_gray 被设置（init 时 prev_gray=None）
    // 实际需要 process_frame 一次让 prev_gray = f1 的灰度
    // 然后 step 应能用 prev_gray=f1 匹配 curr=f2

    // 构造 ctx（这里需要 curr_gray / canvas_gray，实际从 process_frame 内部状态取）
    // 测试技巧：直接调 process_frame 触发整条链，验证 canvas 增长
    let added = stitcher.process_frame(&f2).unwrap();
    assert!(added, "prev_frame step 应成功匹配");
    // 注意：这里实际测的是整条链；单 step 单测需要更精细的 setup
    // 下个 task 再加精细单测，本 task 先建立模式
}
```

**注**：精确的 step 单测需要从 Stitcher 内部提取 prev_gray/curr_gray/canvas_gray——这些是私有字段。两种选择：
- (A) 在 `#[cfg(test)]` 内加 helper fn 暴露这些字段
- (B) 用 `Stitcher` 的 test-only 方法（类似 canvas_heal 的 `inject_constant_canvas_tail`）

推荐 (A)：在 `mod tests` 内加 `fn stitcher_test_state(s: &Stitcher) -> (&GrayBuf, &GrayBuf)` 之类的 helper。如果借用难，直接用 process_frame 端到端验证（现有测试已覆盖）。

**实际策略调整**：由于精确单测 setup 复杂，且现有 fallback 端到端测试（`test_fallback_expanded_search_range` 等）已覆盖行为，本 task 先**实现 step + 用端到端测试验证**，下个 task 视需要补单测。

- [x] **Step 2: 实现 PrevFrameStep（GREEN）**

在 trait 定义之后加：

```rust
/// 步骤 1：相邻帧参考 NCC。
/// 用前一帧底部 strip 当模板，在当前帧做 NCC，求 dy。
pub(crate) struct PrevFrameStep;

impl FallbackStep for PrevFrameStep {
    fn name(&self) -> &'static str { "prev_frame" }

    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome {
        // 需要 prev_gray：从 stitcher 借用（注意借用冲突——先 clone 引用判断）
        let prev_gray_ref = match &ctx.stitcher.prev_gray {
            Some(g) => g,
            None => return StepOutcome::Skip,
        };
        let dy = match ctx.stitcher.try_match_prev_frame(
            prev_gray_ref, ctx.curr_gray, ctx.eff_top, ctx.eff_bottom,
        ) {
            Some(dy) => dy,
            None => return StepOutcome::Skip,
        };
        // 副作用：reset streak + stuck
        ctx.stitcher.best_guess_streak = 0;
        ctx.stitcher.ncc_stuck_count = 0;
        // apply（verify=false：dy 已过内部 validate_ncc_match）
        let result = ctx.stitcher.apply_fallback_match(
            dy, 0.0, 0.0, ctx.frame, ctx.curr_gray, ctx.canvas_gray,
            ctx.sample_cols, false, ctx.w, ctx.eff_top, ctx.eff_bottom,
        );
        StepOutcome::Applied(result)
    }
}
```

- [x] **Step 3: cargo build**

```bash
cargo build -p octopus-capx 2>&1 | tail -10
```

Expected: 0 error。注意借用：`prev_gray_ref` 借了 `ctx.stitcher`，调 `try_match_prev_frame` 是 `&self` 方法 OK；之后 `ctx.stitcher.best_guess_streak = 0` 需要 `&mut`——此时 `prev_gray_ref` 已超出作用域（在 match 里），借用释放。

如有借用错误，调整：把 prev_gray 判断和后续 mut 操作分两步，确保不可变借用先释放。

- [x] **Step 4: cargo test**

```bash
cargo test -p octopus-capx 2>&1 | grep "test result" | head -2
```

Expected: 49 passed（step 还未接入 dispatcher，行为未变）。

- [x] **Step 5: Commit**

```bash
git add crates/capx/src/stitch/fallback_chain.rs
git commit -m "refactor(capx): PrevFrameStep——相邻帧参考 NCC（阶段 3 task 2）

首个 FallbackStep 实现，建立模式：
- struct PrevFrameStep (ZST)
- try_step：prev_gray 存在 + try_match_prev_frame 返回 dy → reset streak/stuck + apply(verify=false)
- 未匹配 → Skip

尚未接入 dispatcher。零行为变更：49 passed"
```

---

## Task 3: Projection1DStep + StationaryStep + BestGuessStep + SkipStep（TDD）

**Files:**
- Modify: `crates/capx/src/stitch/fallback_chain.rs`

封装 dispatcher 余下 4 个分支。按 spec §2.3 表的副作用映射。

- [x] **Step 1: 实现 Projection1DStep**

```rust
/// 步骤 2：1D 行投影 SAD 匹配（低纹理场景）。
pub(crate) struct Projection1DStep;

impl FallbackStep for Projection1DStep {
    fn name(&self) -> &'static str { "1d_projection" }

    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome {
        let x_start = (ctx.w as f64 * X_START_RATIO) as u32;
        let x_end = (ctx.w as f64 * X_END_RATIO) as u32;
        let max_scroll = ctx.stitcher.config.max_scroll;
        match ctx.stitcher.try_match_1d_projection(
            ctx.canvas_gray, ctx.curr_gray, x_start, x_end,
            ctx.eff_top, ctx.eff_bottom, max_scroll, 10.0,
        ) {
            Some((dy, conf, sad)) => {
                log::info!("[stitch] fallback: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
                ctx.stitcher.best_guess_streak = 0;
                let result = ctx.stitcher.apply_fallback_match(
                    dy, conf, sad, ctx.frame, ctx.curr_gray, ctx.canvas_gray,
                    ctx.sample_cols, true, ctx.w, ctx.eff_top, ctx.eff_bottom,
                );
                StepOutcome::Applied(result)
            }
            None => StepOutcome::Skip,
        }
    }
}
```

- [x] **Step 2: 实现 StationaryStep**

```rust
/// 步骤 3：静止检测。画面没动时短路，不进 best_guess。
pub(crate) struct StationaryStep;

impl FallbackStep for StationaryStep {
    fn name(&self) -> &'static str { "stationary" }

    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome {
        let sad = ctx.stitcher.quick_stationary_check(
            ctx.curr_gray, ctx.canvas_gray, ctx.sample_cols,
        );
        if sad < STATIONARY_SAD {
            log::info!("[stitch] stationary detected (sad={:.2})", sad);
            ctx.stitcher.dy_history.clear();
            ctx.stitcher.best_guess_streak = 0;
            ctx.stitcher.last_dy = None;
            StepOutcome::Stationary
        } else {
            StepOutcome::Skip
        }
    }
}
```

- [x] **Step 3: 实现 BestGuessStep**

```rust
/// 步骤 4：历史 dy 中位数估算（best_guess）。streak < 3 才用。
pub(crate) struct BestGuessStep;

impl FallbackStep for BestGuessStep {
    fn name(&self) -> &'static str { "best_guess" }

    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome {
        if ctx.stitcher.best_guess_streak >= 3 {
            return StepOutcome::Skip;
        }
        match ctx.stitcher.estimate_dy_hint() {
            Some(dy) => {
                log::info!("[stitch] best-guess dy={:.1} (streak={})",
                    dy, ctx.stitcher.best_guess_streak + 1);
                ctx.stitcher.best_guess_streak += 1;
                let result = ctx.stitcher.apply_fallback_match(
                    dy, 0.0, 0.0, ctx.frame, ctx.curr_gray, ctx.canvas_gray,
                    ctx.sample_cols, true, ctx.w, ctx.eff_top, ctx.eff_bottom,
                );
                StepOutcome::Applied(result)
            }
            None => StepOutcome::Skip,
        }
    }
}
```

- [x] **Step 4: 实现 SkipStep（终步）**

```rust
/// 步骤 5（终步）：所有降级失败，skip 该帧。
pub(crate) struct SkipStep;

impl FallbackStep for SkipStep {
    fn name(&self) -> &'static str { "skip" }

    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome {
        log::info!("[stitch] all fallbacks exhausted, skipping frame");
        ctx.stitcher.last_dy = None;
        StepOutcome::Applied(Ok(false))
    }
}
```

- [x] **Step 5: cargo build + test**

```bash
cargo build -p octopus-capx 2>&1 | tail -10
cargo test -p octopus-capx 2>&1 | grep "test result" | head -2
```

Expected: 0 error / 49 passed（仍未接入 dispatcher）。

- [x] **Step 6: Commit**

```bash
git add crates/capx/src/stitch/fallback_chain.rs
git commit -m "refactor(capx): 4 个 FallbackStep 实现（阶段 3 task 3）

- Projection1DStep：1D 投影 SAD，reset streak + apply(verify=true)
- StationaryStep：静止检测，clear history + Stationary 短路
- BestGuessStep：dy 中位数，streak += 1 + apply(verify=true)，streak>=3 跳过
- SkipStep：终步，last_dy=None + Ok(false)

尚未接入 dispatcher。零行为变更：49 passed"
```

---

## Task 4: 重写 try_fallback dispatcher 接入 steps

**Files:**
- Modify: `crates/capx/src/stitch/fallback_chain.rs`

**关键风险点**：这是真正改变行为的 task（虽然目标零变更）。dispatcher 从 60 行 if 链换为迭代数组。

- [x] **Step 1: 重写 try_fallback（替换整个方法体）**

把现有 `try_fallback`（55-114 行）整个方法体替换为：

```rust
pub(crate) fn try_fallback(
    &mut self,
    frame: &RgbaImage,
    curr_gray: &GrayBuf,
    canvas_gray: &GrayBuf,
    w: u32,
    eff_top: u32,
    eff_bottom: u32,
) -> Result<bool> {
    let x_start = (w as f64 * X_START_RATIO) as u32;
    let x_end = (w as f64 * X_END_RATIO) as u32;
    let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
        .step_by(SAMPLE_STEP_X)
        .collect();

    let mut steps: [Box<dyn FallbackStep>; 5] = [
        Box::new(PrevFrameStep),
        Box::new(Projection1DStep),
        Box::new(StationaryStep),
        Box::new(BestGuessStep),
        Box::new(SkipStep),
    ];

    for step in &mut steps {
        let ctx = FallbackCtx {
            stitcher: self,
            frame,
            curr_gray,
            canvas_gray,
            w,
            eff_top,
            eff_bottom,
            sample_cols: &sample_cols,
        };
        match step.try_step(ctx) {
            StepOutcome::Applied(result) => return result,
            StepOutcome::Candidate { dy, confidence, sad, verify } => {
                return self.apply_fallback_match(
                    dy, confidence, sad, frame, curr_gray, canvas_gray,
                    &sample_cols, verify, w, eff_top, eff_bottom,
                );
            }
            StepOutcome::Stationary => return Ok(false),
            StepOutcome::Skip => continue,
        }
    }
    unreachable!("SkipStep is terminal")
}
```

**注意借用**：`let ctx = FallbackCtx { stitcher: self, ... }` 把 `&mut self` 借给 ctx，`step.try_step(ctx)` 消费 ctx（by value），借用在 match 后释放。下一次循环 iteration 重新借 self——Rust 允许（NLL）。

如借用报错，可能需要把 ctx 构造挪进 for 循环体（已经是了），或把 sample_cols 提前算（已经是了）。

- [x] **Step 2: cargo build**

```bash
cargo build -p octopus-capx 2>&1 | tail -20
```

Expected: 0 error。典型错误：
- `cannot borrow self as mut more than once` → 调整 ctx 生命周期
- `mismatched types` → StepOutcome 变体名拼错

- [x] **Step 3: cargo test（关键——行为等价判据）**

```bash
cargo test -p octopus-capx 2>&1 | tail -10
```

Expected: **49 passed**。如有 fail：
- 看是哪个测试 fail
- 对照 spec §3 不变量逐项核对（步骤顺序 / 判定逻辑 / 副作用 / verify 参数）
- 修对应 step 的实现

- [x] **Step 4: clippy**

```bash
cargo clippy -p octopus-capx --all-targets 2>&1 | grep -c "^warning:"
```

Expected: ≤ 9 baseline（无新增）。

- [x] **Step 5: Commit**

```bash
git add crates/capx/src/stitch/fallback_chain.rs
git commit -m "refactor(capx): try_fallback dispatcher 接入 FallbackStep 数组（阶段 3 task 4）

60 行 if 链 → 25 行迭代 5 个 Box<dyn FallbackStep>。
步骤顺序：prev_frame → 1d → stationary → best_guess → skip。

零行为变更：cargo test -p octopus-capx → 49 passed"
```

---

## Task 5: 补 step 单测 + 文档同步

**Files:**
- Modify: `crates/capx/src/stitch/fallback_chain.rs`（加 step 单测）
- Modify: `docs/architecture.md`（更新 stitch 描述）
- Modify: `docs/superpowers/specs/2026-08-04-stitch-fallback-trait-design.md`（实施记录）

- [x] **Step 1: 补 PrevFrameStep 单测（Skip 场景）**

测试 prev_gray=None 时返回 Skip：

```rust
#[test]
fn prev_frame_step_skip_when_no_prev_gray() {
    // 构造 Stitcher，prev_gray = None（init 后未 process_frame）
    let f0 = make_frame(TW, TH, 0);
    let mut stitcher = Stitcher::new(f0, StitchConfig::default());
    // new 后 prev_gray 应为 None
    assert!(stitcher.prev_gray.is_none());

    // 构造 ctx——但需要 curr_gray/canvas_gray/sample_cols
    // 用简单 GrayBuf 占位（本测试只验证 prev_gray=None 早退，不触达匹配）
    let curr_gray = GrayBuf { data: vec![0; TW as usize * TH as usize], width: TW as usize, y_offset: 0 };
    let canvas_gray = stitcher.extract_canvas_bottom_gray(stitcher.eff_strip_h);
    let frame = make_frame(TW, TH, 0);
    let sample_cols: Vec<usize> = (40..320).step_by(2).collect();

    let mut step = PrevFrameStep;
    let mut ctx = FallbackCtx {
        stitcher: &mut stitcher,
        frame: &frame,
        curr_gray: &curr_gray,
        canvas_gray: &canvas_gray,
        w: TW,
        eff_top: 0,
        eff_bottom: TH,
        sample_cols: &sample_cols,
    };
    let outcome = step.try_step(&mut ctx);
    assert!(matches!(outcome, StepOutcome::Skip), "prev_gray=None 应返回 Skip");
}
```

- [x] **Step 2: 补 BestGuessStep 边界单测（streak 门控）**

```rust
#[test]
fn best_guess_step_skip_when_streak_exhausted() {
    let f0 = make_frame(TW, TH, 0);
    let mut stitcher = Stitcher::new(f0, StitchConfig::default());
    // 手动设置 streak=3（熔断）+ dy_history 有内容（避免 estimate_dy_hint 返 None 干扰）
    stitcher.best_guess_streak = 3;
    // 即使有 dy_history，streak>=3 应直接 Skip
    stitcher.dy_history.push_back(-10.0);
    stitcher.dy_history.push_back(-10.0);

    let curr_gray = GrayBuf { data: vec![0; TW as usize * TH as usize], width: TW as usize, y_offset: 0 };
    let canvas_gray = stitcher.extract_canvas_bottom_gray(stitcher.eff_strip_h);
    let frame = make_frame(TW, TH, 0);
    let sample_cols: Vec<usize> = (40..320).step_by(2).collect();

    let mut step = BestGuessStep;
    let mut ctx = FallbackCtx {
        stitcher: &mut stitcher,
        frame: &frame,
        curr_gray: &curr_gray,
        canvas_gray: &canvas_gray,
        w: TW,
        eff_top: 0,
        eff_bottom: TH,
        sample_cols: &sample_cols,
    };
    let outcome = step.try_step(&mut ctx);
    assert!(matches!(outcome, StepOutcome::Skip), "streak>=3 应返回 Skip");
    // 验证 streak 没被递增
    assert_eq!(ctx.stitcher.best_guess_streak, 3);
}
```

- [x] **Step 3: 补 SkipStep 单测**

```rust
#[test]
fn skip_step_returns_applied_ok_false() {
    let f0 = make_frame(TW, TH, 0);
    let mut stitcher = Stitcher::new(f0, StitchConfig::default());
    stitcher.last_dy = Some(-10.0); // 设非 None，验证 step 会清

    let curr_gray = GrayBuf { data: vec![0; TW as usize * TH as usize], width: TW as usize, y_offset: 0 };
    let canvas_gray = stitcher.extract_canvas_bottom_gray(stitcher.eff_strip_h);
    let frame = make_frame(TW, TH, 0);
    let sample_cols: Vec<usize> = (40..320).step_by(2).collect();

    let mut step = SkipStep;
    let mut ctx = FallbackCtx {
        stitcher: &mut stitcher,
        frame: &frame,
        curr_gray: &curr_gray,
        canvas_gray: &canvas_gray,
        w: TW,
        eff_top: 0,
        eff_bottom: TH,
        sample_cols: &sample_cols,
    };
    let outcome = step.try_step(&mut ctx);
    match outcome {
        StepOutcome::Applied(Ok(false)) => (),
        other => panic!("期望 Applied(Ok(false))，实际 {:?}", other),
    }
    assert!(ctx.stitcher.last_dy.is_none(), "skip 应清 last_dy");
}
```

注意：`StepOutcome` 需 `derive(Debug)` 让 panic 信息可用。在 enum 定义加 `#[derive(Debug)]`。

- [x] **Step 4: cargo test（应 49 + 3 = 52）**

```bash
cargo test -p octopus-capx 2>&1 | grep "test result" | head -2
```

Expected: **52 passed**。

- [x] **Step 5: 更新 architecture.md**

找到 capx 章节的 stitch 描述（搜 "降级链" 或 "fallback"），把"过程式 5 步 if 链"改为"5 个 FallbackStep trait 实现的迭代 dispatcher"。

- [x] **Step 6: 更新 spec 加实施记录**

在 `docs/superpowers/specs/2026-08-04-stitch-fallback-trait-design.md` 末尾加 "## 9. 实施记录"，标注：
- 5 个 step 实现完成
- dispatcher 重写完成
- +3 单测
- 任何与 spec 偏差（如借用调整）

- [x] **Step 7: clippy + 下游 + Commit**

```bash
cargo clippy -p octopus-capx --all-targets 2>&1 | grep -c "^warning:"
cargo build -p octopus-desktop 2>&1 | tail -3  # 需先 build helper（scripts/build-macos-helper.sh）
```

Expected: clippy ≤ 9 / desktop 0 error。

```bash
git add -A
git commit -m "test(capx): +3 FallbackStep 单测 + 文档同步（阶段 3 task 5）

- PrevFrameStep: prev_gray=None 返 Skip
- BestGuessStep: streak>=3 返 Skip（门控验证）
- SkipStep: 终步返 Applied(Ok(false)) + 清 last_dy
- StepOutcome 加 derive(Debug)
- architecture.md / spec 实施记录同步

cargo test -p octopus-capx → 52 passed"
```

---

## Task 6: 最终验证 + review

- [x] **Step 1: 全量验证**

```bash
cargo test -p octopus-capx 2>&1 | grep "test result"
cargo clippy -p octopus-capx --all-targets 2>&1 | grep -c "^warning:"
wc -l crates/capx/src/stitch/fallback_chain.rs
```

Expected: 52 passed / ≤ 9 warning / fallback_chain.rs ~700 行。

- [x] **Step 2: commit 历史 + diff stat**

```bash
git log --oneline main..HEAD
git diff main..HEAD --stat
```

- [x] **Step 3: 报告用户 + 等 e2e 验证**

向用户报告：
- 5 个 FallbackStep 实现完成
- dispatcher 重写完成（60 行 → 25 行）
- 52 tests passed（49 原 + 3 新）
- 改动局限 fallback_chain.rs 单文件
- 等用户 e2e 滚动截图验证行为等价

**未经用户明确指令不 push 到 main。**
