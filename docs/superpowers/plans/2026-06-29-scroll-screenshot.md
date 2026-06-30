# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ 手动滚动 → 33fps 截帧 + FFT 相位相关亚像素拼接 → 停止后长图入库

**Architecture:** 透明覆盖层 + 焦点让出 + CGWindowList 排除 + FFT 相位相关拼接

**Tech Stack:** Rust + rustfft + imageproc + objc2/objc2-app-kit + core-graphics + Tauri 2 + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## Task 1: FFT 相位相关拼接引擎 ✅

**Files:** `crates/capx/src/stitch.rs`, `Cargo.toml`

- [x] Cargo.toml 加 `rustfft = "6.2"`
- [x] Stitcher + StitchConfig（min_scroll_px=2.0, min_confidence=0.15）
- [x] project_vertical：Sobel 边缘 → 每行平均边缘强度 → 1D 信号
- [x] phase_correlation_dy：FFT → 归一化互功率谱 → IFFT → 峰值 + 抛物线亚像素
- [x] process_frame：FFT 求位移 → 向下滚动追加底部行 → 更新参考投影
- [x] detect_sticky：sticky header/footer 检测

## Task 2: capx/capture.rs 区域截图 ✅

- [x] capture_region_excluding_window（CGWindowList + BGRA→RGBA）

## Task 3: 后端录制循环 ✅

- [x] start_scroll_recording / stop_scroll_recording
- [x] save_frontmost_app / activate_prev_app
- [x] 33fps 截图循环
- [x] 30ms 鼠标监视线程
- [x] WebP 入库

## Task 4: 前端 scrolling 模式 ✅

- [x] clearRect 挖透明孔
- [x] startScroll / stopScroll
- [x] scroll://frame 事件监听
- [x] 预览浮层

## Task 5: 端到端验证 ✅

- [x] Cmd+Shift+D → 框选 → 滚动截图 → 停止 → 长图入库

---

## 实施偏差记录

### 偏差 1-8：早期失败方案 ❌

NSPanel 崩溃、auto 模式体验差、简单 deactivate 不穿透、always_on_top 关闭被遮挡、Occlusion Throttling、Key Window 焦点锁定。

### 偏差 9：NCC 模板匹配（三种变体全部失败）❌

1. **双模板 PLL** → delta 值 265-455 剧烈跳动，周期性假匹配
2. **底部 strip 固定** → 整数像素累积误差 → 模糊
3. **动态模板位置** → 模板位置帧间不一致 → delta 波动

**根因**：
- NCC 整数像素滑窗无法处理 12.7px 等非整数位移 → 每帧 0.3-0.7px 累积 → 模糊
- 100px 模板在 45px 行高列表中，d 和 d±45 得分接近 → 假匹配 → 行重复

### 偏差 10：FFT 相位相关（最终方案）✅

**方案**：1D FFT 相位相关 + 抛物线亚像素拟合。

**优势**：
- 亚像素精度（0.1px）→ 消除累积模糊
- 频率域全局主峰 → 对周期性内容鲁棒
- O(N log N) → 比 NCC 更快

**实现**：
- Sobel 边缘 → 垂直投影（每行平均边缘强度）→ 1D 信号
- FFT(a), FFT(b) → 归一化互功率谱 → IFFT → 峰值 = 位移
- 抛物线拟合峰值附近三点 → 亚像素精化

---

## Spec Coverage

| spec 章节 | 实现 |
|---|---|
| §1.1 透明窗口 | Task 4 |
| §1.2 焦点让出 | Task 3 |
| §1.3 坐标映射 | Task 3 |
| §2.1 FFT 核心算法 | Task 1 |
| §2.2 NCC→FFT 优势 | 偏差 9-10 |
| §2.4 位移方向 | Task 1 |
| §3 录制循环 | Task 3 |
