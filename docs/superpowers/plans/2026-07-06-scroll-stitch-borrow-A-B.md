# 滚动拼接借鉴改造（第一阶段 A+B）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans（inline 执行，批量 + checkpoint）。A+B 聚焦、需反复编译测试，inline 比 subagent 往返更快。

**Goal:** A 把滚动截图 capture/process 拆成生产-消费两 task（watch 通道丢旧保新）；B 给 `validate_ncc_match` 加主次比硬过滤。

**Architecture:** 见 `docs/superpowers/specs/2026-07-06-scroll-stitch-borrow-A-B-design.md`。A 改 `screenshot_commands.rs::start_scroll_recording`（编排层，接口零变更）；B 改 `stitch.rs::validate_ncc_match`（算法验证，`Stitcher` 公共接口零变更）。两者独立 commit。

**Tech Stack:** Rust + Tauri 2 + tokio + imageproc。测试 `cargo test -p octopus-capx`。

**约定：** 所有 cargo 命令带 `--manifest-path` 指向 worktree（worktree-cwd-trap）；git 用 `git -C <worktree>` 绝对路径。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/capx/src/stitch.rs` | `validate_ncc_match` 加主次比判据（B）；新增 `NCC_PEAK_GAP` 常量；启用 `best_y`。加 4 单测 | 修改 |
| `crates/desktop/src/screenshot_commands.rs` | `start_scroll_recording` 拆生产/消费两 task + `tokio::sync::watch`（A） | 修改 |

---

## Task 1: B 主次比判据（TDD，先做）

**Files:**
- Modify: `crates/capx/src/stitch.rs`（`NCC_PEAK_GAP` 常量 + `validate_ncc_match` + 新增 4 测试）

- [ ] **Step 1: 写 4 个失败测试**（追加到 `#[cfg(test)] mod tests`）

```rust
use imageproc::definitions::Image;

/// 构造 1 列 × rows 行的 response，指定位置的峰值为给值，其余为 base。
fn make_response(rows: u32, peaks: &[(u32, f32)], base: f32) -> Image<image::Luma<f32>> {
    let mut r = Image::new(1, rows);
    for y in 0..rows { r.put_pixel(0, y, image::Luma([base])); }
    for &(y, v) in peaks { r.put_pixel(0, y, image::Luma([v])); }
    r
}

#[test]
fn test_validate_rejects_ambiguous_response() {
    // 双等高峰（间隔 15 > GAP=8）：主峰 y=5=0.9，次峰 y=20=0.9 → max2=0.9 > 0.9*0.5 → 拒绝
    let r = make_response(30, &[(5, 0.9), (20, 0.9)], 0.1);
    assert!(!validate_ncc_match(&r, 5, 0.9));
}

#[test]
fn test_validate_accepts_dominant_peak() {
    // 单峰主导：主峰 0.9，远处次峰 0.3（< 0.45）→ 接受
    let r = make_response(30, &[(5, 0.9), (20, 0.3)], 0.1);
    assert!(validate_ncc_match(&r, 5, 0.9));
}

#[test]
fn test_validate_peak_gap_excludes_neighbors() {
    // 次峰在 GAP 内（肩部，y=5+3=8 ≤ 5+8）：不当次峰，max2 仍是远处的 0.2 → 接受
    let r = make_response(30, &[(5, 0.9), (8, 0.85), (20, 0.2)], 0.1);
    assert!(validate_ncc_match(&r, 5, 0.9));
}

#[test]
fn test_validate_short_response_passes() {
    // 高 ≤ 2*GAP=16：无邻域外次峰 → max2=0 → 不拒绝（区分度兜底）
    let r = make_response(12, &[(5, 0.9)], 0.1);
    assert!(validate_ncc_match(&r, 5, 0.9));
}
```

- [ ] **Step 2: 跑确认失败**（`validate_ncc_match` 还没主次比）

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml test_validate_rejects_ambiguous_response`
Expected: FAIL（当前 validate 接受双峰）。

- [ ] **Step 3: 实现** —— 加常量 + 改 `validate_ncc_match`

在 `// ===== NCC 匹配参数 =====` 段加：
```rust
/// 主峰邻域半宽（像素）。主次比检测时排除 [best_y-GAP, best_y+GAP]，
/// 避免同一峰的肩部被误当独立次峰。NCC response 相邻 y 高度相关。
const NCC_PEAK_GAP: usize = 8;
```

`validate_ncc_match` 改为（签名 `best_y` 去掉下划线）：
```rust
fn validate_ncc_match(response: &Image<image::Luma<f32>>, best_y: usize, best_score: f32) -> bool {
    if best_score < NCC_SCORE_THRESHOLD {
        return false;
    }
    let h = response.height() as usize;
    let mut min_score = f32::MAX;
    let mut max2 = 0.0f32;
    for y in 0..h {
        let v = response.get_pixel(0, y as u32)[0];
        if v < min_score { min_score = v; }
        if (y as isize - best_y as isize).abs() > NCC_PEAK_GAP as isize && v > max2 {
            max2 = v;
        }
    }
    if best_score - min_score < 0.1 {
        return false;
    }
    if max2 > best_score * 0.5 {
        return false;
    }
    true
}
```

- [ ] **Step 4: 跑确认通过**

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: PASS（全部，含新 4 测 + 既有）。

- [ ] **Step 5: Commit**

```bash
git -C <WT> add crates/capx/src/stitch.rs
git -C <WT> commit -m "feat(capx): NCC validate 加主次比判据，拒绝周期/重复纹理歧义匹配"
```

---

## Task 2: A 队列解耦

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs::start_scroll_recording`（1089-1166 主循环段）

- [ ] **Step 1: 改造主循环为生产/消费两 task + watch 通道**

要点：
1. 首帧截屏 + `Stitcher::new`（现状 1049-1077）**不变**。
2. 建 `let (tx, mut rx) = tokio::sync::watch::channel(None::<Option<image::RgbaImage>>);` —— 或更简：用 sentinel。实际用 `watch::channel(()初始标记)`，生产 send 帧前先发 sentinel 让消费等待。简化：watch 初值放一个 `Option<RgbaImage>` = None，消费 `changed` 后 `borrow` 若 None 则跳过（首帧前的占位）。但首帧已在 Stitcher::new，生产从第二帧 send Some(frame)。
   - **推荐实现**：`watch::channel(None::<image::RgbaImage>)`；生产每帧 `tx.send(Some(frame))`；消费 `changed().await` 后 `let frame = rx.borrow().as_ref()?.clone()`（None 跳过）。停止时生产 `drop(tx)` → 消费 `changed()` 得 `Err` → 退出。
3. 生产 task：`tauri::async_runtime::spawn` 内 `while RECORDING { tick(30ms); spawn_blocking(capture) → tx.send(Some(frame)); } ` 然后 `drop(tx)`。capture 逻辑（macOS CGWindowList / 非 macOS crop_region_rgba_direct）原样搬入。
4. 消费 task：原 `while RECORDING { tick; capture; process; preview; emit }` 改为 `while let Ok(()) = rx.changed().await { let frame = match rx.borrow().clone() { Some(f)=>f, None=>continue }; process_frame; last_frame=Some(frame); spawn_blocking(preview).await; emit }`。
5. 两个 task 都 `let ah = ah.clone()` 等捕获所需变量；消费 task 持有 `stitcher`（&mut）。**生产 task 不能碰 stitcher**。
6. 生产/消费 task 句柄 `.await`（或 tokio::join）等两者都结束，再进停止流程（finalize/入库/窗口/剪贴板，现状 1168-1306 原样保留）。
7. 鼠标监听 task（992）不动。

- [ ] **Step 2: 编译 + capx 全量测试**

Run:
```
cargo build --manifest-path <WT>/crates/desktop/Cargo.toml
cargo test --manifest-path <WT>/crates/capx/Cargo.toml
```
Expected: build 通过；capx 全绿（A 不动 stitch.rs，但确认无回归）。

- [ ] **Step 3: Commit**

```bash
git -C <WT> add crates/desktop/src/screenshot_commands.rs
git -C <WT> commit -m "feat(desktop): 滚动截图 capture/process 拆生产-消费两 task,watch 通道丢旧保新"
```

---

## Task 3: 手动 e2e（用户）

无 e2e 基建，A 靠手动验证。交付用户后：
- [ ] 启动桌面端，区域截图 → 滚动截图模式
- [ ] 平稳慢速滚动一页 → 长图拼接完整、无断带、无重复段
- [ ] 快速连续滚动 → 预览不卡顿、不丢大段内容（丢旧保新预期：可能跳预览帧，但拼接结果连续）
- [ ] 停止（复制/保存/取消三模式）→ finalize 正确补全底部、入库/剪贴板/对话框正常
- [ ] 回滚验证：若主次比误拒均匀滚动（拼接质量下降），调大 `NCC_PEAK_GAP` 并补用例
