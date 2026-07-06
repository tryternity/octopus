# 滚动拼接借鉴改造（第一阶段 A+B）— 设计

- 日期：2026-07-06
- 分支：`borrow/scroll-stitch-A-B`（从 `feature-0706` 分叉，带对比 spec）
- 范围：capx 滚动截图第一阶段借鉴改造 —— **A 捕获/拼接队列解耦** + **B NCC 主次比判据**
- 关联文档：
  - 借鉴依据 `docs/superpowers/specs/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md`
  - 后续阶段 F（配置外置）、D（缩放匹配）另起 spec，不在本阶段
- 改动文件：`crates/desktop/src/screenshot_commands.rs`（A）、`crates/capx/src/stitch.rs`（B）

## 1. 背景与目标

对比 spec §3 指出：octopus 滚动截图主循环（`screenshot_commands.rs:1089`）把 `capture` / `process_frame` / `preview 编码` 串在一个 30ms tick 里——preview 编码每帧 `await`、`process_frame` 未 `spawn_blocking`，两者任一偏慢就拉长 tick，下一帧截晚。对比 spec §4-B 指出：NCC 验证缺主次比判据，周期假匹配靠跨帧 `same_dy_count` 状态机兜，可加单帧入口硬过滤简化。

**目标**
- **A**：拆生产/消费两 task，用 `tokio::sync::watch` 通道丢旧保新，capture 不再被 CPU 拖漂节拍。
- **B**：`validate_ncc_match` 加单帧主次比硬过滤（次峰 ≥ 主峰一半 → 拒绝），单帧入口拒绝周期/重复纹理歧义。

**非目标**
- 不动 `Stitcher` 公共接口（`process_frame`/`finalize`/`canvas`/`canvas_buf_slice`/`height`/`canvas_w` 签名不变）。
- 不动 `same_dy_count`（B-min：它仍管「连续相同 dy + 均匀滚动合法放行」；主次比只抓单帧歧义，职责不同）。
- 不做 F（配置外置）、D（缩放匹配）——后续阶段。
- 不照搬 snow-shot 三段式（CaptureService/Queue/Stitch + tauri::State）——octopus 是进程内单次录制、后端自驱，YAGNI。
- 不为消费 task panic 加守护（现状主 task panic 同样卡死，非本次引入）。

## 2. 架构（A 方案 1：两 task + watch 通道）

```
生产 task (新 spawn)                       消费 task (原主循环改造)
  while RECORDING:                           while let Ok(()) = rx.changed().await {
    tick(30ms)                                  let frame = rx.borrow().clone();   // 最新,中间帧已被覆盖
    match spawn_blocking(capture) {             stitcher.process_frame(&frame);   // &mut 在此侧
      Ok(frame) => { let _ = tx.send(frame); }     // 覆盖 = 丢旧保新        last_frame = Some(frame);
      Err(_) => continue,                         // 截屏失败跳过           spawn_blocking(preview 编码).await; emit("scroll://frame")
    }                                         }
  drop(tx)   // RECORDING false → 退出          // tx dropped → changed() Err → 退出
                                            finalize(last_frame); 入库/剪贴板(同现状)
```

**为什么 watch 而非 mpsc**：背压策略选了「有界丢旧保新」。mpsc 生产侧只能 send、无法 pop 旧帧，做不到「丢旧保新」。`tokio::sync::watch` 天然**只保最新值**（生产 `send` 覆盖前值），且 `drop(sender)` 让消费侧 `changed()` 返回 `Err`——停止信号免费拿到，语义完全匹配。

**stitcher `&mut` 归属**：全程在消费 task，无锁、无共享。preview 编码依赖 `canvas_buf_slice`+`height`，故 stitcher 必须留消费侧。

**生命周期**：两个 task 都看 `SCROLL_RECORDING` atomic（与现有鼠标监听 task `992` 一致）。生产 task 退出时 `drop(tx)` → 消费 `changed()` 得 `Err` → drain（watch 无积压）→ `finalize(last_frame)`。

## 3. 改动面

| 文件 | 改动 | 接口影响 |
|---|---|---|
| `crates/desktop/src/screenshot_commands.rs` | `start_scroll_recording`：首帧截屏 + `Stitcher::new` 后，把现有 while 循环拆为生产 task（capture→`tx.send`）与消费 task（`rx.changed`→process→preview→emit）；停止/finalize/入库/鼠标监听/窗口管理全部不动 | 无 |
| `crates/capx/src/stitch.rs` | `validate_ncc_match` 加 top-2 主次比判据；新增 `NCC_PEAK_GAP` 常量；启用原被忽略的 `best_y` 参数 | `Stitcher` 公共接口零变更 |

A、B 互不依赖：A 纯编排层，B 纯算法验证函数。各自独立 commit、独立回归。

## 4. 数据流与错误处理（A）

- **正常**：capture → `tx.send`（覆盖=丢旧保新）→ `rx.changed` → `process_frame` → preview 编码 → emit。
- **capture 失败**（spawn_blocking 返回 Err）：生产侧 `continue`，不入队，消费无感（丢这一帧，正确）。
- **消费慢、生产快**：watch 自动覆盖中间帧，消费每次拿最新——丢旧保新，内存恒定。拼接正确性由 canvas-anchored 保证（`test_canvas_anchored_recovers_after_failures` 守护）。
- **停止时序**：`RECORDING=false` → 生产下次 tick 退出并 `drop(tx)` → 消费 `changed()` 得 `Err` → watch 无积压 → `finalize(last_frame)`。`last_frame` = 消费侧最后处理帧（已对齐画布）。
- **Cancel 模式**：保持现状——drain + finalize 后判 `stop_mode==Cancel` 跳过入库。代价仅多等最后一两帧；不为 Cancel 单独加速（YAGNI）。
- **首帧**：循环外单独截给 `Stitcher::new`（现状 1049-1077 不变）；生产 task 从第二帧起 send。watch 初始值用 sentinel（消费侧跳过）。
- **消费 task panic**：现状主 task panic 也卡死（RECORDING 仍 true），非本次引入，不在此加守护。

## 5. B 主次比判据

`validate_ncc_match`（stitch.rs:160）当前只扫一遍找 min/max（区分度 `max-min≥0.1`）。改造后：

```rust
const NCC_PEAK_GAP: usize = 8;   // 主峰邻域半宽，排除"同峰肩部"被误当次峰

fn validate_ncc_match(response: &Image<Luma<f32>>, best_y: usize, best_score: f32) -> bool {
    if best_score < NCC_SCORE_THRESHOLD { return false; }          // 0.65，原有

    let h = response.height() as usize;
    let mut min_score = f32::MAX;
    let mut max2 = 0.0f32;                                          // 次峰：排除主峰邻域
    for y in 0..h {
        let v = response.get_pixel(0, y as u32)[0];
        if v < min_score { min_score = v; }
        if (y as isize - best_y as isize).abs() > NCC_PEAK_GAP as isize && v > max2 {
            max2 = v;
        }
    }
    if best_score - min_score < 0.1 { return false; }              // 区分度，原有（max=best_score）
    if max2 > best_score * 0.5 { return false; }                   // 新增：次峰≥主峰一半 → 歧义拒绝
    true
}
```

要点：
- **`best_y` 原被 `_` 忽略，现启用**——定位主峰，排除 `±NCC_PEAK_GAP` 邻域。NCC response 相邻 y 高度相关（同峰肩部），不隔开会把肩部误当次峰。
- response 是 1 列 × N 行（template/search 同宽），只扫 `x=0`。
- **次峰判据** `max2 > best_score × 0.5`（主峰不足次峰 2 倍）→ 多峰歧义，拒绝走 fallback。对齐 snow-shot `max_count < second_max_count * 2` 语义。
- 边界：response 过短（高 ≤ 2×GAP）→ 无邻域外点 → `max2=0` → 不拒绝（区分度检查兜底）。
- **不动 `same_dy_count`**（B-min）。

## 6. 测试

| 项 | 做法 |
|---|---|
| **B 单测**（stitch.rs `#[cfg(test)]`） | ① `test_validate_rejects_ambiguous_response`：双等高峰(间隔>GAP) → `false`；② `test_validate_accepts_dominant_peak`：单峰主导 → `true`；③ `test_validate_peak_gap_excludes_neighbors`：次峰在 GAP 内(肩部) → 不当次峰 → `true`；④ `test_validate_short_response_passes`：高≤2×GAP → 不拒绝 |
| **A 测试** | `start_scroll_recording` 是 tauri command + spawn task + 平台截屏 API，难单测。靠①手动 e2e（快速滚动观察不丢内容、预览流畅、停止 finalize 正确）+ ②stitch 公共接口零变更 → 现有 stitch.rs 全部测试守护算法不回归。不为 A 抽「通用生产-消费骨架」单测（YAGNI） |
| **回归** | A、B 各自独立 commit；每个 commit 后 `cargo test -p octopus-capx` 全绿；最后手动滚动截图一轮 |

## 7. 风险

- **watch clone 开销**：每帧消费侧 `borrow().clone()` RgbaImage（数 MB），与现状 preview clone 同级，可接受。
- **`NCC_PEAK_GAP` 取值**：8px 经验值，配 4 个单测锁定行为；若 e2e 发现误拒均匀滚动，调大并补用例。
- **A 编排改动较大**：`start_scroll_recording` 是长函数，拆 task 时务必保持停止/finalize/入库/鼠标监听/窗口管理原语义不变，靠手动 e2e 验证。
