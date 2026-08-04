# 降级链 trait 抽象设计（阶段 3）

- 日期：2026-08-04
- 分支：`refactor/stitch-trait`
- Worktree：`.worktrees/refactor-stitch-trait`
- 类型：重构（核心控制流抽象，行为应保持等价）
- Baseline：`cargo test -p octopus-capx` → **49 passed**（main 880f620a）
- 前置：阶段 1（拆分）+ 阶段 2（bottom_strip helper / 常量分组）已合入 main

---

## 1. 背景与动机

### 1.1 现状

`stitch/fallback_chain.rs` 的 `try_fallback`（55-114）是一个 60 行的过程式 dispatcher，硬编码 5 步降级路径，每步有**异构的返回类型 + 异构的副作用**：

| # | 步骤 | 返回类型 | 成功时副作用（dispatcher 内手写） |
|---|---|---|---|
| 1 | `try_match_prev_frame` | `Option<f64>` (dy) | reset `best_guess_streak=0` + `ncc_stuck_count=0` → `apply_fallback_match(verify=false)` |
| 2 | `try_match_1d_projection` | `Option<(f64, f64, f64)>` (dy,conf,sad) | reset `best_guess_streak=0` → `apply_fallback_match(verify=true)` |
| 3 | `quick_stationary_check` | `f64` (sad) | 若 `< STATIONARY_SAD`：clear `dy_history` + reset `best_guess_streak=0` + `last_dy=None` → `Ok(false)` |
| 4 | `estimate_dy_hint` | `Option<f64>` (dy) | `best_guess_streak += 1`（门控 `< 3`）→ `apply_fallback_match(verify=true)` |
| 5 | 兜底 skip | — | `last_dy=None` → `Ok(false)` |

### 1.2 痛点

- **新增/调整降级步要改 dispatcher 多处**：加一步要改 if 链 + 对应副作用 + 测试
- **副作用散落**：每步的状态重置散在 dispatcher 各分支，读代码须跳 5 处才能理解"成功后做了什么"
- **测试只能端到端**：现有测试都通过 `process_frame` 触发整条链，无法单测某一步的"成功+副作用"组合
- **步骤间隐式耦合**：如 step 4 的 `best_guess_streak` 门控依赖 step 1/2 成功时 reset——这是隐式契约，新人改一步容易破坏另一步

### 1.3 目标

引入 `trait FallbackStep`，让每步：
1. 实现统一 `try_step() -> StepOutcome` 接口
2. **自带副作用**（封装在自己的 impl 里，dispatcher 不再手写）
3. 可独立单测（构造 Stitcher + 输入，直接调 `try_step` 验证 outcome + 状态）

### 1.4 非目标（本次不做）

- ❌ 不抽主匹配（`best_ncc_match` / `process_frame_inner` 的 NCC 路径）——主匹配有周期假匹配检测等复杂逻辑，与 fallback 形态不同
- ❌ 不拆子目录（`fallback/`）——5 个 impl 放在 `fallback_chain.rs` 内，避免过度工程
- ❌ 不改任何阈值 / 常量值
- ❌ 不改公开 API（`Stitcher::process_frame` / `Stitcher::finalize` 签名一字不改）
- ❌ 不动 `apply_fallback_match`（它是步骤 1/2/4 共享的"出口"，保留为 Stitcher 方法）
- ❌ 不动 `verify_alignment_2d` / `quick_stationary_check`（它们是步骤实现内部用的 helper，保留为 Stitcher 方法供 trait impl 调用）

---

## 2. trait 设计

### 2.1 核心 trait

```rust
/// 降级链单步的输出。步骤返回候选 dy + 自副作用已执行；dispatcher 据此决定链路。
pub(crate) enum StepOutcome {
    /// 本步成功求出 dy，apply_fallback_match 已在 step 内调用（或等效状态变更已完成）。
    /// dispatcher 应直接 return 该 step 的 Result<bool>。
    Applied(Result<bool>),
    /// 本步求出 dy 但未 apply，请求 dispatcher 走 apply_fallback_match(verify)。
    /// 保留这个变体以支持"延迟 apply"——目前所有步骤都直接 Applied，但保留扩展点。
    Candidate { dy: f64, confidence: f64, sad: f64, verify: bool },
    /// 本步判定画面静止，链路应短路返回 Ok(false)。
    Stationary,
    /// 本步未匹配，继续尝试下一步。
    Skip,
}

/// 降级链单步。每个实现封装一种 fallback 策略 + 其副作用。
pub(crate) trait FallbackStep {
    /// 步骤名（日志用）。
    fn name(&self) -> &'static str;

    /// 尝试本步降级。读取 stitcher 状态 + 输入帧几何，返回 StepOutcome。
    ///
    /// 实现约束：
    /// - 副作用（reset streak / clear history / 递增 streak）必须在本方法内完成
    /// - 返回 Applied 时，apply_fallback_match 也应在本方法内调用（封装完整）
    /// - 不应修改与本步无关的 Stitcher 字段
    fn try_step(&mut self, ctx: &FallbackCtx) -> StepOutcome;
}
```

### 2.2 步骤上下文（避免 &mut Stitcher 暴露太多）

```rust
/// 步骤执行上下文。聚合步骤所需的输入参数 + Stitcher 可变引用。
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
```

**为何不用 `&mut Stitcher` 直接传**：`FallbackCtx` 显式列出步骤所需输入，让 trait 实现无法触碰无关字段（如 `canvas_buf`、`sticky_top`）。步骤 1/2/4 调 `apply_fallback_match` 时通过 `ctx.stitcher.apply_fallback_match(...)` 访问。

### 2.3 5 个实现

| impl | 封装的现有方法 | 副作用（移入 impl） |
|---|---|---|
| `PrevFrameStep` | `try_match_prev_frame` | reset `best_guess_streak=0` + `ncc_stuck_count=0` |
| `Projection1DStep` | `try_match_1d_projection` | reset `best_guess_streak=0` |
| `StationaryStep` | `quick_stationary_check` + `< STATIONARY_SAD` 判定 | clear `dy_history` + reset `best_guess_streak=0` + `last_dy=None` |
| `BestGuessStep` | `estimate_dy_hint` + `< 3` 门控 | `best_guess_streak += 1` |
| `SkipStep`（终步） | 兜底 | `last_dy=None` |

每个 impl 是一个 zero-sized struct（`struct PrevFrameStep;`），持有 `Vec<Box<dyn FallbackStep>>` 在 Stitcher 构造时建立顺序。

### 2.4 重写后的 dispatcher

```rust
impl Stitcher {
    /// 降级链：迭代 steps，首个非 Skip 的 outcome 决定返回。
    pub(crate) fn try_fallback(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        canvas_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 抽样列 + max_scroll 算一次复用
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();

        // 步骤顺序：prev_frame → 1D → stationary → best_guess → skip
        let mut steps: [Box<dyn FallbackStep>; 5] = [
            Box::new(PrevFrameStep),
            Box::new(Projection1DStep),
            Box::new(StationaryStep),
            Box::new(BestGuessStep),
            Box::new(SkipStep),
        ];

        for mut step in steps {
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
            match step.try_step(&mut ctx) {
                StepOutcome::Applied(result) => return result,
                StepOutcome::Candidate { dy, confidence, sad, verify } => {
                    return ctx.stitcher.apply_fallback_match(
                        dy, confidence, sad, frame, curr_gray, canvas_gray, &sample_cols,
                        verify, w, eff_top, eff_bottom,
                    );
                }
                StepOutcome::Stationary => return Ok(false),
                StepOutcome::Skip => continue,
            }
        }
        unreachable!("SkipStep is terminal, always returns Applied(Ok(false))")
    }
}
```

**关键变化**：
- 60 行 if 链 → 25 行迭代
- 每步副作用封装在自己的 impl，dispatcher 不再手写 reset/clear
- 借用检查：`steps` 数组在 `for` 循环中 by-value 迭代（`for mut step in steps`），每个 step 独占 `ctx` 的 `&mut Stitcher`，避免同时多借

### 2.5 借用检查难点（已考虑）

`FallbackCtx` 持有 `&mut Stitcher`，而 `apply_fallback_match` 是 `&mut self` 方法。在 `StepOutcome::Candidate` 分支里 `ctx.stitcher.apply_fallback_match(...)` 需要 `ctx.stitcher` 可再借——因为 `step.try_step(&mut ctx)` 返回后 `ctx` 的借用释放，`ctx.stitcher` 可重新借用。**编译期可通过**。

`PrevFrameStep` 内部调 `ctx.stitcher.try_match_prev_frame(...)` 和 `ctx.stitcher.apply_fallback_match(...)`——都通过 `ctx.stitcher` 单一借用源，不冲突。

---

## 3. 不变量（必须保持）

拆分前后行为等价的硬约束：

1. **步骤顺序不变**：prev_frame → 1D → stationary → best_guess → skip
2. **每步的判定逻辑不变**：
   - prev_frame：prev_gray 存在 + dy>0 + validate_ncc_match 通过
   - 1D：`try_match_1d_projection` 返回 Some
   - stationary：`quick_stationary_check` 返回值 `< STATIONARY_SAD`
   - best_guess：`best_guess_streak < 3` 且 `estimate_dy_hint` 返回 Some
3. **每步的副作用不变**（见 2.3 表）
4. **apply_fallback_match 的 verify 参数不变**：prev_frame=false，1D/best_guess=true
5. **`cargo test -p octopus-capx` = 49 passed**（行为等价证明）
6. **公开 API 零变更**：`Stitcher::process_frame` / `finalize` / `new` 签名一字不改

---

## 4. TDD 实施策略

### 4.1 测试先行原则

每个 `FallbackStep` 实现先写测试再写代码。测试形态：

```rust
#[test]
fn prev_frame_step_applied_on_match() {
    // 构造 Stitcher，设置 prev_gray 与 curr_gray 已知重叠
    let (mut stitcher, frame, curr_gray, canvas_gray, ...) = setup_prev_frame_scenario();
    let mut step = PrevFrameStep;
    let ctx = FallbackCtx { stitcher: &mut stitcher, ... };
    let outcome = step.try_step(&ctx);
    assert!(matches!(outcome, StepOutcome::Applied(Ok(true))));
    assert_eq!(stitcher.best_guess_streak, 0, "prev_frame 成功应 reset streak");
    assert_eq!(stitcher.ncc_stuck_count, 0, "prev_frame 成功应 reset stuck");
}
```

### 4.2 测试覆盖矩阵

每个 step 测 3 种场景：
- **Applied/Stationary 场景**：构造该步能成功的输入，验证 outcome + 副作用
- **Skip 场景**：构造该步不匹配的输入，验证返回 `Skip` + 状态不变
- **边界场景**：步骤特定的边界（如 BestGuessStep 的 streak=3 门控）

### 4.3 dispatcher 回归测试

保留现有的端到端 fallback 测试（`test_fallback_expanded_search_range` 等），它们验证整条链路行为不变。

---

## 5. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| **借用检查失败** | 中 | 编译失败 | FallbackCtx 设计已考虑；如失败，调整 ctx 字段借用方式 |
| **副作用遗漏** | 中 | 行为变更（测试失败） | TDD：每步先写副作用断言测试 |
| **步骤顺序错误** | 低 | 行为变更 | steps 数组显式列出顺序；dispatcher 测试覆盖 |
| **性能回归**（Box<dyn>） | 极低 | 微秒级 | 5 个 Box<dyn> per frame，可忽略；如担心改 `static` 数组 |
| **apply_fallback_match 双调** | 低 | 逻辑错误 | Candidate 变体保留但本次不用——所有步骤直接 Applied |

---

## 6. 验证策略

| 层级 | 命令 | 通过标准 |
|---|---|---|
| 编译 | `cargo build -p octopus-capx` | 0 error 0 warning |
| 单元测试 | `cargo test -p octopus-capx` | **49 passed**（baseline）+ 新增 step 测试 |
| clippy | `cargo clippy -p octopus-capx --all-targets` | ≤ 9 baseline warning |
| 下游 | `cargo build --release -p octopus-desktop` | 0 error |
| e2e | 滚动截图 | 行为等价（用户手动验证） |

---

## 7. 文件改动范围

| 文件 | 改动 |
|---|---|
| `stitch/fallback_chain.rs` | 新增 trait + StepOutcome + FallbackCtx + 5 个 impl；重写 try_fallback dispatcher |
| `stitch/mod.rs` | 无改动（try_fallback 签名不变） |
| `stitch/graybuf.rs` | 无改动 |
| `stitch/ncc_match.rs` | 无改动 |
| `stitch/canvas_heal.rs` | 无改动 |

**改动局限在 fallback_chain.rs 单文件**——这是本次设计的关键优点。

---

## 8. 关联文档

- 阶段 1 spec：`docs/superpowers/specs/2026-08-04-stitch-refactor-design.md` §2.3（阶段 3 候选）
- 阶段 1 plan：`docs/superpowers/plans/2026-08-04-stitch-refactor.md`
- 现状代码：`crates/capx/src/stitch/fallback_chain.rs`（588 行）
- research：`docs/superpowers/specs/research/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md`（snow-shot 对照）
