# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ 手动滚动触控板 → 后端增量截帧 + PLL 拼接 → 停止后长图入库

**Architecture:** 透明覆盖层 + clearRect 挖孔 + 协同焦点让出 + CGWindowList 排除截图窗口 + 双模板 PLL 拼接引擎

**Tech Stack:** Rust + imageproc 0.25 + image 0.25 + xcap + objc2/objc2-app-kit + core-graphics + Tauri 2 + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/Cargo.toml` | Modify | 加 imageproc 依赖 |
| `crates/capx/src/stitch.rs` | Create | 拼接引擎：NCC + PLL + 双模板 + 黄金列 |
| `crates/capx/src/lib.rs` | Modify | pub mod stitch |
| `crates/capx/src/capture.rs` | Modify | capture_region_excluding_window（区域截图排除 overlay） |
| `crates/desktop/Cargo.toml` | Modify | 加 objc2-app-kit、core-graphics |
| `crates/desktop/src/screenshot_commands.rs` | Modify | start/stop_scroll_recording + 焦点让出 + 坐标映射 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | scrolling 模式 + clearRect 挖孔 + 预览 + 工具栏 |

---

## Task 1: capx/stitch.rs 拼接引擎 ✅

**Files:** `crates/capx/src/stitch.rs`, `lib.rs`, `Cargo.toml`

- [x] Cargo.toml 加 imageproc
- [x] lib.rs 加 pub mod stitch
- [x] Stitcher + StitchConfig
- [x] process_frame：重复检测 + sticky/active 检测 + NCC 匹配 + 裁剪追加
- [x] detect_sticky_and_active：sticky header/footer + active_cols + match_cols（黄金列）
- [x] match_template：PLL 局部跟踪 + 失锁全局重捕
- [x] find_best_template_y：边缘投影最大 Y 坐标
- [x] ncc_score：限制 200px 匹配宽度
- [x] is_duplicate：稀疏采样均值 < 2.0

## Task 2: capx/capture.rs 区域截图 ✅

**Files:** `crates/capx/src/capture.rs`, `Cargo.toml`

- [x] capture_region_excluding_window(exclude_window_id, rect_x, rect_y, rect_w, rect_h)
  - CGWindowListCreateImage 只截选区区域 + 排除 overlay
  - BGRA → RGBA 转换
- [x] Cargo.toml 加 core-graphics + core-foundation（macOS）

## Task 3: 后端录制循环 ✅

**Files:** `crates/desktop/src/screenshot_commands.rs`

- [x] start_scroll_recording(x, y, w, h, win_label, interactive_rects)
- [x] stop_scroll_recording()
- [x] main.rs 注册命令

### Task 3a: macOS 焦点让出 ✅

- [x] save_frontmost_app()：NSWorkspace.frontmostApplication() 暂存 PID
- [x] activate_prev_app()：主线程 activateWithOptions 激活前台应用
- [x] set_window_ignores_mouse_events()：主线程 setIgnoresMouseEvents

### Task 3b: 坐标映射 ✅

- [x] get_window_cocoa_frame()：NSWindow frame（Cocoa 坐标，原点左下）
- [x] get_primary_screen_height()：主屏高度
- [x] win_origin_y = primary_h - (cy + ch)：翻转为 Quartz 坐标
- [x] 物理像素 crop 坐标 = (全局逻辑 - 显示器逻辑偏移) × scale

### Task 3c: 截图循环 ✅

- [x] 120ms 间隔 spawn_blocking 截图
- [x] capture_region_excluding_window 只截选区区域
- [x] JPEG 编码选区画面 → emit("scroll://frame")
- [x] stitcher.process_frame
- [x] 预览缩略图 → emit("scroll://frame")
- [x] 停止后 WebP 入库

### Task 3d: 30ms 鼠标监视线程 ✅

- [x] 独立 tokio task，30ms 轮询 CGEvent 鼠标位置
- [x] interactive_rects 判定 → set_ignore_cursor_events 切换
- [x] 不在轮询中调 activate/deactivate

## Task 4: 前端 scrolling 模式 ✅

**Files:** `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

- [x] scrolling 模式 draw()：clearRect 挖透明孔 + 绿色边框
- [x] startScroll()：计算 interactiveRects 传后端
- [x] stopScroll()
- [x] scroll://frame 事件监听 → Canvas 重绘 + 预览更新
- [x] scroll://done 事件监听 → 回到 selected 模式
- [x] 滚动截图按钮（工具栏）
- [x] 预览浮层（右侧优先）

## Task 5: 端到端验证 ✅

- [x] Cmd+Shift+D 触发截图 → 双屏窗口出现
- [x] 框选区域 → 点滚动截图按钮
- [x] 在选区外滚动 → 底层应用跟随滚动
- [x] 截帧拼接 → 预览实时更新
- [x] 点「停止」→ 长图入库 → 剪贴板浮窗可见

---

## 实施偏差与重构记录

### 偏差 1：窗口隐藏方案（失败）❌

录制时隐藏截图窗口。用户无法看到选区和工具栏。

### 偏差 2：NSPanel 方案（失败）❌

`to_panel()` 的 `object_setClass` swizzling 在 WKWebView 创建后执行 → Trace/BPT trap 崩溃（exit 133）。NonactivatingPanel 阻断 IME 输入。

### 偏差 3：auto 模式 CGEvent 模拟滚轮（放弃）❌

截到截图窗口自身 + 副屏坐标错 + 不可控速度。用户明确需要手动滚动。

### 偏差 4：简单 deactivate（失败）❌

`NSApp.deactivate()` 无法可靠让 trackpad scrollWheel 路由到底层应用。需要精确激活前台应用。

### 偏差 5：always_on_top(false)（失败）❌

关掉置顶后截图窗口被底层应用遮挡，截到错误内容（如 Finder）。

### 偏差 6：硬编码工具栏坐标（失败）❌

后端 `y + h + 8.0` 与前端动态居中位置不一致 → 工具栏点击失效。改为前端传递 `interactiveRects`。

### 偏差 7：120ms 轮询鼠标穿透切换（失败）❌

穿透切换延迟太高（120ms），点击穿透到底层应用。改为 30ms 独立线程。

### 偏差 8：Occlusion Throttling（最终突破）✅

**根因发现**：不透明置顶窗口遮挡底层应用 → macOS 挂起底层 GPU 渲染 → Chrome 停止 repainting → 滚轮无响应。

**解决**：`.transparent(true)` + `ctx.clearRect` 挖透明孔 → Window Server 保持底层应用高频渲染。

### 偏差 9：Key Window 焦点锁定（最终突破）✅

**根因发现**：即使 `setIgnoresMouseEvents(true)`，截图窗口仍是 Key Window → scrollWheel 路由到截图窗口而非底层应用。

**解决**：`save_frontmost_app()` + `activate_prev_app()` 协同焦点让出。

### 偏差 10：拼接算法改进 ✅

- **严格置信度过滤**：低置信帧丢弃，不猜测位移
- **裁剪修正**：`new_start + tpl_h` 消除模板重叠
- **黄金列锁定**：边缘投影最密集的 200px 列
- **双模板动态 Y**：底部 1/3 + 中上部 2/3 一致性校验
- **PLL 位移跟踪**：局部窗口 `[last_dy - 20, last_dy + 80]` 防周期混淆
- **NCC 宽度限制**：200px 上限避免 Retina CPU 饱和

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 三层架构 | Task 1 + 2 + 3 + 4 |
| §2.1 Occlusion Throttling | Task 4（transparent + clearRect）|
| §2.2 Key Window 焦点让出 | Task 3a（save/activate_prev_app）|
| §2.3 坐标映射 | Task 3b（Cocoa frame + Quartz 翻转）|
| §2.4 CGWindowList 排除 | Task 2（capture_region_excluding_window）|
| §3 拼接引擎 | Task 1 |
| §3.2 黄金列 | Task 1（detect_sticky_and_active match_cols）|
| §3.3 双模板 | Task 1（find_best_template_y × 2）|
| §3.4 PLL 跟踪 | Task 1（match_template 局部+全局）|
| §3.6 降级策略 | Task 1（low_conf_streak + is_duplicate）|
| §4 状态机 | Task 4（scrolling 模式）|
| §4.2 录制循环 | Task 3c |
| §4.3 区域化 cursor | Task 3d（30ms 独立线程）|

---

## 后续优化方向

1. 模板匹配并行化（rayon / Metal Compute Shader）
2. 卡尔曼滤波位移预测
3. 动态捕获间隔（滚动事件驱动）
4. 多窗口架构（消除 30ms 竞态）
