# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ 手动滚动 → 33fps 截帧 + 1D FFT 相位相关亚像素拼接 → 停止后长图入库

**Architecture:** 透明覆盖层 + 焦点让出 + CGWindowList 排除 + 1D FFT 相位相关拼接

**Tech Stack:** Rust + rustfft + imageproc + objc2/objc2-app-kit + core-graphics + core-foundation + Tauri 2 + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## Task 1: FFT 相位相关拼接引擎 ✅

- [x] Cargo.toml 加 `rustfft = "6.2"`
- [x] Stitcher + StitchConfig（min_scroll_px=2.0, min_confidence=0.15）
- [x] project_vertical_range：Sobel 边缘 → 每行平均边缘强度 → 1D 信号
- [x] phase_correlation_dy：FFT → 归一化互功率谱 → IFFT → 峰值 + 抛物线亚像素
- [x] process_frame：detect_sticky → 裁 canvas → FFT 求位移 → 追加底部行 → 更新参考投影
- [x] finalize：补全最后一帧 sticky footer
- [x] detect_sticky：sticky header/footer 检测

## Task 2: capx/capture.rs 区域截图 ✅

- [x] capture_region_excluding_window（CGWindowList + BGRA→RGBA）
- [x] capture_window_region（指定窗口 ID 截图）

## Task 3: 后端录制循环 ✅

- [x] start_scroll_recording / stop_scroll_recording
- [x] save_frontmost_app / activate_prev_app
- [x] target_wid 改用选区中心检测（get_window_pid_at_point），非 PREV_ACTIVE_APP
- [x] 33fps 截图循环
- [x] 30ms 鼠标监视线程（预览区域切换 ignore + 自动激活选区下方应用）
- [x] 预览：底部裁剪 + 400px 宽 + CatmullRom
- [x] 停止：先恢复鼠标 → finalize → spawn_blocking 预览 → 入库
- [x] WebP 入库

## Task 4: 前端 scrolling 模式 ✅

- [x] clearRect 挖透明孔（Canvas 只画边框）
- [x] 选区外遮罩用 DOM div（不经过 Canvas）
- [x] scrolling 模式工具栏隐藏
- [x] startScroll / stopScroll
- [x] scroll://frame 事件监听
- [x] 预览面板 HUD 风格（毛玻璃 + 脉冲 REC + 2:1 按钮 + 底部对齐选区）

## Task 5: 托盘菜单 ✅

- [x] 「截图」→「开始截图」
- [x] 去掉「停止滚动截图」菜单
- [x] 截图进行中灰掉菜单（不改文字）
- [x] 分组分隔线 + 「剪  贴  板」双半角空格对齐
- [x] 引擎信息格式简化

## Task 6: 端到端验证 ✅

- [x] Cmd+Shift+D → 框选 → 滚动截图 → 停止 → 长图入库
- [x] 拼接结果无重叠、无缺失、无模糊
- [x] 预览清晰 + 底部最新内容可见
- [x] 停止时无鼠标假死
- [x] 选区下方应用自动激活
- [x] 预览面板按钮可点击

---

## 实施偏差记录

### 偏差 1-8：早期失败方案 ❌

NSPanel 崩溃、auto 模式体验差、简单 deactivate 不穿透、always_on_top 关闭被遮挡、Occlusion Throttling、Key Window 焦点锁定。

### 偏差 9：NCC 模板匹配（三种变体全部失败）❌

1. 双模板 PLL → delta 剧烈跳动，周期性假匹配
2. 底部 strip 固定 → 整数像素累积误差 → 模糊
3. 动态模板位置 → 帧间不一致 → delta 波动

### 偏差 10：FFT 相位相关（最终方案）✅

1D FFT 相位相关 + 抛物线亚像素拟合。

### 偏差 11：FFT 实现调试 ✅

- dy 方向反了 → 修正
- 投影长度不匹配 → detect_sticky 后重算
- 首帧 sticky 重复 → 初始化时裁掉 sticky
- 最后一帧缺失 → finalize 补全

### 偏差 12：预览体验优化 ✅

- 预览模糊 → 400px CatmullRom
- 看不到最后一行 → 底部裁剪 + finalize 后 emit
- 预览底部固定 → bottom 对齐选区 + justifyContent flex-end

### 偏差 13：停止时鼠标假死 ✅

先恢复鼠标事件 → 再 finalize → spawn_blocking 预览。

### 偏差 14：选区遮罩变暗 ✅

Canvas 像素遮罩 → DOM div 遮罩（选区内不经过 Canvas）。

### 偏差 15：target_wid 黑屏 ✅

PREV_ACTIVE_APP PID → 选区中心检测（get_window_pid_at_point）。

### 偏差 16：选区下方应用自动激活 ✅

CGWindowListCopyWindowInfo + bounds 命中 → activateWithOptions（run_on_main_thread）。
跳过 kCGWindowLayer != 0（桌面壁纸等）。

### 偏差 17：工具栏停止按钮不可点击 ✅

scrolling 模式下隐藏工具栏，停止/取消按钮移到预览面板中。
预览区域 interactiveRects 传给后端监视线程。

### 偏差 18：预览面板 HUD 重新设计 ✅

毛玻璃面板 + 琥珀色脉冲 REC + 等宽数字 + 2:1 按钮 + hover 过渡。

### 偏差 19：托盘菜单设计 ✅

- 分组分隔线
- 「剪  贴  板」「记  事  本」双半角空格对齐
- 截图灰掉菜单（不改文字）
- 引擎信息格式简化
- 菜单文案带快捷键（⌘⇧A 格式，从 config 动态读取）

### 偏差 20：UI 细节优化 ✅

- 滚动截屏按钮使用 `icons/scroll.svg` 图标
- 取消按钮图标 CSS filter 染红色
- tiptap 依赖拆分为独立 chunk（消除 bundle 过大警告）
- 清理 `[window-diag]` 诊断日志

---

## Spec Coverage

| spec 章节 | 实现 |
|---|---|
| §1.1 透明窗口 | Task 4 |
| §1.2 焦点让出 | Task 3 |
| §1.3 坐标映射 | Task 3 |
| §1.4 自动激活选区下方应用 | Task 3 |
| §2.1 FFT 核心算法 | Task 1 |
| §2.4 Sticky 处理 | Task 1 |
| §3 录制循环 | Task 3 |
| §3.1 停止流程 | 偏差 13 |
| §4.1 滚动模式 UI | Task 4 |
| §4.2 预览面板 | Task 4 |
| §5 托盘菜单 | Task 5 |
