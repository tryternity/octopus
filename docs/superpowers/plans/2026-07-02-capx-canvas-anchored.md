# Canvas-Anchored 匹配 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 匹配输入源从上一帧改为画布底部 strip，根治累积漂移导致的丢内容。

**Architecture:** 移除 `self.reference` 字段，每帧从 `canvas_buf` 底部提取 STRIP_H 行 RGBA 转灰度作为匹配模板。`find_overlap_spatial_ext` 的 ref_buf 从完整帧变为 strip_h 高度的短灰度图，简化模板提取（ref_buf 本身即模板）。三级降级链同步改造。

**关联文档:** [spec](../specs/2026-07-02-capx-canvas-anchored-design.md)

---

## 关键约束

1. **API 零改动**：`new/process_frame/finalize/canvas/height` 签名不变
2. **灰度公式不变**：`(2126*R + 7152*G + 722*B) / 10000`
3. **现有 16 测试必须保持全绿**
4. **worktree**: `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx`

---

## Task 1: 新增 `extract_canvas_bottom_gray` + 移除 `self.reference` 字段

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 在 `impl Stitcher` 中（`is_stationary` 之前）新增 `extract_canvas_bottom_gray` 方法**

```rust
    /// 从画布底部提取 strip_h 行 RGBA 转灰度，作为 Canvas-Anchored 匹配模板。
    /// 无论多少帧匹配失败，画布底部始终是最新已确认内容 → 消除累积漂移。
    fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
        let row_bytes = self.canvas_w as usize * 4;
        let start_row = self.canvas_h.saturating_sub(strip_h);
        let mut data = Vec::with_capacity(strip_h as usize * self.canvas_w as usize);
        for y in start_row..self.canvas_h {
            let row_start = y as usize * row_bytes;
            for x in 0..self.canvas_w as usize {
                let off = row_start + x * 4;
                let r = self.canvas_buf[off] as u32;
                let g = self.canvas_buf[off + 1] as u32;
                let b = self.canvas_buf[off + 2] as u32;
                let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
                data.push(luma as u8);
            }
        }
        GrayBuf { data, width: self.canvas_w as usize }
    }
```

- [ ] **Step 2: 移除 `self.reference` 字段**

从 `Stitcher` struct 中删除 `reference: GrayBuf` 字段及其文档注释。从 `new()` 中删除 `reference: GrayBuf { data: Vec::new(), width: 0 }` 初始化。

- [ ] **Step 3: 暂时注释掉所有 `self.reference` 引用（编译会报错，逐一处理）**

此时编译会有多处 `self.reference` 报错。**暂时不加 `#[allow(dead_code)]`**——Task 2-4 会逐一替换为 `self.extract_canvas_bottom_gray(STRIP_H)`。

先用 `grep -n "self.reference" crates/capx/src/stitch.rs` 列出所有引用点，了解范围。

- [ ] **Step 4: Commit（WIP，允许编译不过——但实际我们会在 Task 2 立即修复）**

> 实际上不要提交编译不过的代码。改为：Task 1 和 Task 2 一起完成后再提交。Task 1 只做 Step 1（加方法）+ Step 2（移除字段），然后立即进 Task 2 替换引用。

---

## Task 2: 改造 `process_frame` 主匹配为 Canvas-Anchored

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `process_frame` 初始化分支——移除 `self.reference = GrayBuf::from_rgba(frame)`**

初始化分支中删除 `self.reference = GrayBuf::from_rgba(frame);` 这一行。Canvas-Anchored 不需要在初始化时存 reference——下一帧直接从 canvas 底部提取。

- [ ] **Step 2: 修改 `process_frame` 主匹配分支——用画布底部替代 reference**

旧：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);
        // ...
        let texture = estimate_texture_density(&curr_buf, &sample_cols, template_y);
        let sad_accept = self.dynamic_sad_accept(texture);
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            ...
```

新：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        // ...
        let texture = estimate_texture_density(&canvas_ref, &sample_cols, 0);
        let sad_accept = self.dynamic_sad_accept(texture);
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(
            &canvas_ref,
            &curr_buf,
            ...
```

> **注意 `estimate_texture_density` 的 `template_y` 参数**：canvas_ref 只有 strip_h 行，template_y 应为 0（整个 canvas_ref 就是模板条）。

- [ ] **Step 3: 修改主匹配成功后的状态更新——移除 `self.reference = curr_buf`**

删除 `self.reference = curr_buf;` 这一行。Canvas-Anchored 不需要存 curr_buf 作为 reference。

但注意 `curr_buf` 在降级链中仍需使用（借用），且 `apply_fallback_match` 中也不再需要 `self.reference = curr_buf.clone()`。检查 `curr_buf` 的所有权——主匹配成功后 `curr_buf` 不再被 move，可以继续借用给后续代码（但主匹配成功直接 return，不会执行降级链）。

- [ ] **Step 4: 修改降级链——传入 `&canvas_ref` 替代 `self.reference`**

降级链中 `try_match` 和 `try_match_1d_projection` 内部引用 `&self.reference`。改为在 `process_frame` 中把 `canvas_ref` 传给降级链。

由于 `try_match` / `try_match_1d_projection` 是 `&self` 方法，无法接收外部参数。两个选择：
- **A**（推荐）：改为接收 `ref_buf: &GrayBuf` 参数
- **B**：改为 `&mut self` 并存 `canvas_ref` 到临时字段

选 A。修改 `try_match` 和 `try_match_1d_projection` 签名，新增 `ref_buf: &GrayBuf` 参数：

```rust
    fn try_match(
        &self,
        ref_buf: &GrayBuf,  // 新增
        curr: &GrayBuf,
        ...
    ) -> Option<(f64, f64, f64)> {
        find_overlap_spatial_ext(ref_buf, curr, ...)
    }
```

`try_match_1d_projection` 同理，把内部 `&self.reference` 替换为 `ref_buf`。

降级链调用处传入 `&canvas_ref`。

- [ ] **Step 5: 修改 `apply_fallback_match`——移除 `self.reference = curr_buf.clone()`**

删除 `self.reference = curr_buf.clone();` 这一行。

- [ ] **Step 6: 编译验证**

Run: `cargo check -p octopus-capx 2>&1 | tail -5`
Expected: `Finished`（无 `self.reference` 引用残留）

若有 `self.reference` 残留报错，用 `grep -n "self.reference" crates/capx/src/stitch.rs` 定位并修复。

- [ ] **Step 7: 运行测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

- [ ] **Step 8: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 改为 Canvas-Anchored 匹配（画布底部 strip 替代 reference 帧）"
```

---

## Task 3: 改造 `find_overlap_spatial_ext` 适配短 ref_buf + 简化模板提取

**Files:** `crates/capx/src/stitch.rs`

> Canvas-Anchored 后，ref_buf 只有 strip_h 行（画布底部），不再需要 `extract_template` 单独提取模板——ref_buf 本身就是模板。

- [ ] **Step 1: 修改 `find_overlap_spatial_ext` 内部——ref_buf 即模板，简化 extract_template 调用**

当前 `extract_template` 从 ref_buf 的 `[template_y, template_y+strip_h)` 行提取模板。Canvas-Anchored 后 ref_buf 本身就只有 strip_h 行，template_y 恒为 0。

把 `extract_template(ref_buf, template_y, &sample_cols, strip_h)` 改为直接从 ref_buf 第 0 行开始提取（template_y = 0）。

或者更简洁：直接传 `template_y = 0` 给 `extract_template`。但 `template_y` 在 `find_overlap_spatial_ext` 中还用于计算 `min_y_offset`/`max_y_offset` 和最终 dy——这些值是 curr_buf 坐标系下的，与 ref_buf 的内部行号无关。

**关键理解**：`template_y` 是 curr_buf 坐标系下的"模板底部位置"（`eff_bottom - strip_h`）。ref_buf 的行号 0..strip_h 对应 curr_buf 中 `template_y..template_y+strip_h` 的期望对齐位置。所以 `extract_template(ref_buf, 0, &sample_cols, strip_h)` 是正确的——从 ref_buf 第 0 行提取。

修改 `find_overlap_spatial_ext` 中：
```rust
    // 旧
    let tpl = extract_template(ref_buf, template_y, &sample_cols, strip_h);
    // 新（ref_buf 行号从 0 开始）
    let tpl = extract_template(ref_buf, 0, &sample_cols, strip_h);
```

- [ ] **Step 2: 修改 `estimate_confidence` 和 `sparse_sad_at_offset` 中的 ref_buf 行号引用**

这些函数内部用 `ref_buf.row((template_y as usize) + dy)` 访问 ref_buf。Canvas-Anchored 后 ref_buf 只有 strip_h 行，行号应从 0 开始。

修改 `estimate_confidence` 和 `sparse_sad_at_offset`：把 `ref_buf.row((template_y as usize) + dy)` 改为 `ref_buf.row(dy)`。

但这两个函数需要知道 ref_buf 的行号映射。**最简洁方案**：给这两个函数也传 `template_y_for_ref = 0`，或者直接在调用时把 ref_buf 视为从第 0 行开始。

实际上 `sparse_sad_at_offset` 和 `estimate_confidence` 中 `template_y` 用于定位 ref_buf 的行。改为：
- `sparse_sad_at_offset(ref_buf, curr_buf, sparse_cols, ref_offset, y_offset, strip_h)`，其中 `ref_offset` 是 ref_buf 内部的行号偏移（Canvas-Anchored 时为 0 + dy）

> **简化决策**：由于 ref_buf 的行号 0..strip_h 恰好对应原来的 `template_y..template_y+strip_h`，只需把所有 `ref_buf.row(template_y + dy)` 改为 `ref_buf.row(dy)`。`search_best_offset` 不访问 ref_buf（它用预提取的 tpl），所以不受影响。`estimate_confidence` 和 `sparse_sad_at_offset` 需要改。

具体修改 `sparse_sad_at_offset`：
```rust
fn sparse_sad_at_offset(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    sparse_cols: &[usize],
    strip_h: u32,
    y_offset: u32,  // curr_buf 中的 y_offset
) -> f64 {
    let strip_h = strip_h as usize;
    let mut sad: u64 = 0;
    let mut count = 0u64;
    for dy in (0..strip_h).step_by(2) {
        let ref_row = ref_buf.row(dy);  // 旧：ref_buf.row(template_y + dy)；新：ref_buf.row(dy)
        let curr_row = curr_buf.row(y_offset as usize + dy);
        ...
    }
}
```

移除 `template_y` 参数（ref_buf 行号从 0 开始，不需要偏移）。

同步修改 `estimate_confidence` 的调用和内部逻辑。

- [ ] **Step 3: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "refactor(capx): find_overlap_spatial_ext 适配短 ref_buf（画布底部 strip）"
```

---

## Task 4: 改造 `finalize` + `try_match_1d_projection` 为 Canvas-Anchored

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `finalize`——用画布底部替代 `self.reference`**

旧：
```rust
        let last_buf = GrayBuf::from_rgba(last_frame);
        ...
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            ...
```

新：
```rust
        let last_buf = GrayBuf::from_rgba(last_frame);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        ...
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &canvas_ref,
            &last_buf,
            ...
```

- [ ] **Step 2: 修改 `try_match_1d_projection`——接收 `ref_buf: &GrayBuf` 参数替代 `&self.reference`**

把内部所有 `&self.reference` 替换为 `ref_buf`。签名新增 `ref_buf: &GrayBuf`。

- [ ] **Step 3: 修改降级链中 `try_match_1d_projection` 的调用——传入 `&canvas_ref`**

`process_frame` 降级链中：
```rust
                if let Some((dy, conf, sad)) = self.try_match_1d_projection(
                    &canvas_ref,  // 新增
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept,
                ) {
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

`cargo check -p octopus-desktop` 确认 API 兼容。

- [ ] **Step 5: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): finalize + try_match_1d_projection 改为 Canvas-Anchored"
```

---

## Task 5: 新增 Canvas-Anchored 测试

**Files:** `crates/capx/src/stitch.rs`（测试模块）

- [ ] **Step 1: 新增"连续失败后恢复"测试（核心验证）**

```rust
    #[test]
    fn test_canvas_anchored_recovers_after_failures() {
        // 构造 5 帧序列，中间帧匹配失败（相同帧模拟静止→无追加）
        // 验证后续帧能与画布底部正确对齐，不位移突变
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        // 帧 2: 滚动 30px，成功追加
        let f2 = make_frame(TW, TH, 30);
        let added2 = s.process_frame(&f2).unwrap();
        assert!(added2);
        let h_after_2 = s.height();

        // 帧 3: 相同帧（静止），不追加
        let f3 = make_frame(TW, TH, 30);
        s.process_frame(&f3).unwrap();

        // 帧 4: 滚动到 60px，应能与画布底部（~30px 位置）正确对齐
        let f4 = make_frame(TW, TH, 60);
        let added4 = s.process_frame(&f4).unwrap();
        assert!(added4, "Canvas-Anchored 应在中间静止帧后恢复匹配");
        let h_after_4 = s.height();
        assert!(h_after_4 > h_after_2, "恢复后画布应继续增长");
    }
```

- [ ] **Step 2: 新增"画布底部提取正确性"测试**

```rust
    #[test]
    fn test_extract_canvas_bottom_gray() {
        // 构造已知画布内容，验证提取的底部 strip 灰度正确
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        // 初始化（裁掉 sticky 后画布 = 首帧有效区域）
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap();

        // 提取底部 strip
        let bottom_gray = s.extract_canvas_bottom_gray(STRIP_H);
        assert_eq!(bottom_gray.width, TW as usize);
        // data 长度 = strip_h × width
        // 底部 strip 对应画布最后 STRIP_H 行
        // 验证：手动从 canvas 计算底部 strip 灰度，与 extract 结果比对
        let canvas = s.canvas();
        let canvas_h = canvas.height();
        let mut expected = Vec::new();
        for y in (canvas_h - STRIP_H)..canvas_h {
            for x in 0..TW {
                let px = canvas.get_pixel(x, y);
                let luma = (2126 * px[0] as u32 + 7152 * px[1] as u32 + 722 * px[2] as u32) / 10000;
                expected.push(luma as u8);
            }
        }
        // 只比对抽样列（estimate_texture_density 用 sample_cols）
        assert_eq!(bottom_gray.data.len(), STRIP_H as usize * TW as usize);
        // 比对前几行确认一致
        for i in 0..TW as usize {
            assert_eq!(bottom_gray.row(0)[i], expected[i], "底部 strip 首行不一致 @ x={}", i);
        }
    }
```

- [ ] **Step 3: 编译 + 运行全部测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 16 + 2 = 18 passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增 Canvas-Anchored 恢复 + 画布底部提取正确性测试"
```

---

## Task 6: 文档同步

**Files:** spec + architecture.md

- [ ] **Step 1: 更新 spec 状态为实施完成**

- [ ] **Step 2: 更新 architecture.md stitch 描述——标注 Canvas-Anchored**

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(capx): 同步 Canvas-Anchored 匹配实施记录"
```

---

## 验收清单

- [ ] `cargo test -p octopus-capx` 全绿（≥18 个测试）
- [ ] `cargo check -p octopus-capx -p octopus-desktop` 无错误
- [ ] API 零改动
- [ ] `self.reference` 字段完全移除，无残留引用
- [ ] 文档同步
