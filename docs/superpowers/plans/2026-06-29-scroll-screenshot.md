# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ 手动滚动触控板 → 后端 33fps 增量截帧 + 底部 strip NCC 拼接 → 停止后长图入库

**Architecture:** 透明覆盖层 + clearRect 挖孔 + 协同焦点让出 + CGWindowList 排除截图窗口 + 底部 strip NCC 匹配拼接

**Tech Stack:** Rust + imageproc 0.25 + image 0.25 + xcap + objc2/objc2-app-kit + core-graphics + Tauri 2 + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/Cargo.toml` | Modify | 加 imageproc 依赖 |
| `crates/capx/src/stitch.rs` | Create | 拼接引擎：底部 strip NCC + 距离惩罚 + 重复帧检测 |
| `crates/capx/src/lib.rs` | Modify | pub mod stitch |
| `crates/capx/src/capture.rs` | Modify | capture_region_excluding_window（区域截图排除 overlay） |
| `crates/desktop/Cargo.toml` | Modify | 加 objc2-app-kit、core-graphics |
| `crates/desktop/src/screenshot_commands.rs` | Modify | start/stop_scroll_recording + 焦点让出 + 坐标映射 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | scrolling 模式 + clearRect 挖孔 + 预览 + 工具栏 |

---

## Task 1: capx/stitch.rs 拼接引擎 ✅

- [x] Stitcher + StitchConfig（template_ratio=0.20, min_confidence=0.65）
- [x] process_frame：重复检测 → Sobel edges → 底部 strip 模板 → NCC 搜索 → 裁剪追加
- [x] detect_sticky_and_match_cols：sticky header/footer + match_cols
- [x] is_duplicate_fast：稀疏采样均值差 < 3.0
- [x] ncc_score：全宽度匹配
- [x] finalize：补全最后一帧 sticky_bottom 区域

## Task 2: capx/capture.rs 区域截图 ✅

- [x] capture_region_excluding_window(exclude_window_id, rect_x, rect_y, rect_w, rect_h)
- [x] BGRA → RGBA 转换

## Task 3: 后端录制循环 ✅

- [x] start_scroll_recording(x, y, w, h, win_label, interactive_rects)
- [x] stop_scroll_recording()
- [x] save_frontmost_app() / activate_prev_app()
- [x] set_window_ignores_mouse_events()（主线程）
- [x] get_window_cocoa_frame() + get_primary_screen_height()（坐标转换）
- [x] 33fps 截图循环（30ms interval）
- [x] stitcher.finalize()
- [x] 30ms 鼠标监视线程（interactive_rects 区域切换）
- [x] WebP 入库

## Task 4: 前端 scrolling 模式 ✅

- [x] scrolling 模式 draw()：clearRect 挖透明孔 + 绿色边框
- [x] startScroll()：计算 interactiveRects 传后端
- [x] stopScroll()
- [x] scroll://frame 事件监听
- [x] 滚动截图按钮 + 预览浮层

## Task 5: 端到端验证 ✅

- [x] Cmd+Shift+D 触发截图 → 双屏窗口出现
- [x] 框选区域 → 点滚动截图按钮
- [x] 在选区外滚动 → 底层应用跟随滚动
- [x] 截帧拼接 → 预览实时更新
- [x] 点「停止」→ 长图入库

---

## 实施偏差与重构记录

### 偏差 1-6：早期失败方案 ❌

1. 窗口隐藏 → 用户看不到选区
2. NSPanel → Trace/BPT trap 崩溃
3. auto 模式 → 体验差
4. 简单 deactivate → scrollWheel 不穿透
5. always_on_top(false) → 窗口被遮挡
6. 硬编码工具栏坐标 → 点击失效

### 偏差 7：Occlusion Throttling（突破）✅

不透明置顶窗口 → macOS 挂起底层 GPU 渲染。`.transparent(true)` + `clearRect` 解决。

### 偏差 8：Key Window 焦点锁定（突破）✅

`save_frontmost_app()` + `activate_prev_app()` 协同焦点让出。

### 偏差 9：双模板 + PLL 拼接（重写）✅

旧版：固定模板位置 + 双模板一致性 + PLL 跟踪。delta 值剧烈波动（265→455），周期性假匹配。

新版：底部 strip NCC + 距离惩罚 + 全局 fallback + 加速度检查。更简单更稳定。

### 偏差 10：拼接引擎重写中的 bug（已修复）✅

- **canvas vs frame 高度不匹配** → panic（越界）。修复：用 `cmp_h = canvas_h.min(frame_h)`
- **模板取自 canvas 高度而非 last_edges** → 匹配全失败。修复：`tpl_y_start = last_edges.height() - tpl_h`
- **缺重复帧检测** → 画面没动也无限追加。修复：`is_duplicate_fast`（均值差 < 3.0）

### 偏差 11：帧率提升 120ms → 30ms ✅

33fps 高频捕获减少帧间位移，降低拼接断裂概率。

### 偏差 12：搜索窗口 + 距离惩罚 ✅

- 首帧搜索窗口 `[tpl_y - 20, tpl_y + 5]`
- 后续帧期望位置 ±窗口 + 距离惩罚 `adjusted_score = score - distance × 0.004`
- 加速度 > 30px 拒绝
- 全局搜索阈值提高到 0.85

### 偏差 13：失锁恢复 ✅

连续 3 帧匹配失败 → 重锁模板 + 重置位移。

### 偏差 14：finalize 补全 ✅

录制结束时补全最后一帧的 sticky_bottom 区域（eff_bottom 到 h）。

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 三层架构 | Task 1 + 2 + 3 + 4 |
| §2.1 Occlusion Throttling | Task 4（transparent + clearRect）|
| §2.2 Key Window 焦点让出 | Task 3（save/activate_prev_app）|
| §2.3 坐标映射 | Task 3（Cocoa frame + Quartz 翻转）|
| §2.4 CGWindowList 排除 | Task 2（capture_region_excluding_window）|
| §3.1 拼接核心思路 | Task 1（底部 strip NCC）|
| §3.3 搜索窗口策略 | Task 1（距离惩罚 + 加速度检查）|
| §3.4 失锁恢复 | Task 1（3 帧重锁）|
| §3.5 finalize | Task 1 |
| §3.6 降级策略 | Task 1（重复检测 + 全局 fallback）|
| §4 状态机 | Task 4（scrolling 模式）|
| §4.2 录制循环 | Task 3（33fps）|
| §4.3 区域化 cursor | Task 3（30ms 独立线程）|

---

## 后续优化方向

1. 模板匹配并行化（rayon / Metal Compute Shader）
2. 卡尔曼滤波位移预测
3. 动态捕获间隔（滚动事件驱动）
4. 多窗口架构（消除 30ms 竞态）
