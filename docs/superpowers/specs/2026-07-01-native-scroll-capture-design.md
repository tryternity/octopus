# 原生 NSView 滚动截屏设计

**日期**: 2026-07-01（2026-07-01 更新拼接引擎为 2D SAD）
**状态**: macOS 验证通过，滚动截屏功能基本可用
**分支**: `feature/clipboard-research`（验证期间禁止同步到 main）
**目标**: 用原生 NSView/NSWindow 替换 WebView 实现滚动截屏

## 0. 概述

独立 crate `crates/scroll-capture/` 实现原生 NSView 滚动截屏。不碰现有 `screenshot_commands.rs` 代码。托盘菜单「滚动截屏」直接触发。

## 1. 文件结构

```
crates/scroll-capture/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 公共接口（start/stop/is_recording_active）
│   ├── stitch.rs           # 2D SAD 模板匹配拼接引擎
│   ├── macos/
│   │   ├── mod.rs           # macOS 模块入口
│   │   ├── overlay_impl.rs  # NSScrollOverlayWindow + NSScrollOverlayView + 录制循环
│   │   ├── capture.rs       # CGWindowList 截图 + 排除自身
│   │   └── helpers.rs       # 焦点让出 + 坐标转换
├── docs/superpowers/specs/  # 设计文档
└── docs/superpowers/plans/  # 实施计划
```

## 2. 核心架构

### 2.1 窗口管理（原生 NSWindow + NSView）

**NSScrollOverlayWindow**（继承 NSWindow）：
- borderless + transparent + floating (level=3)
- `canBecomeKey = true`（接收键盘事件）
- `canBecomeMainWindow = false`

**NSScrollOverlayView**（继承 NSView）：
- `isOpaque = false`（关键：让系统知道窗口不透明区域需要重绘）
- `drawRect`：每次绘制先 `CGContextClearRect` 真正擦除之前内容
- 状态机：idle → selecting → recording
- mouseDown/mouseDragged/mouseUp：拖拽拉框选区
- keyDown：ESC（keyCode 53）停止录制

### 2.2 选区拉框

```
mouseDown → 记录起点，state = selecting
mouseDragged → 更新选区矩形 + setNeedsDisplay（暗遮罩 + 绿色边框）
mouseUp → 选区确定（> 10px）
  → state = recording
  → 全屏 CGContextClearRect（擦除暗遮罩，让底层应用可见）
  → 只画绿色边框
  → setIgnoresMouseEvents(true)（滚轮穿透）
  → 激活选区下方应用（get_window_pid_at_point + activateWithOptions）
  → 启动录制线程
```

### 2.3 截图排除

`CGWindowListCreateImage(rect, kCGWindowListOptionOnScreenBelowWindow, windowNumber, ...)`

用覆盖窗口的 `windowNumber` 排除自身。只截选区矩形范围内的内容。

### 2.4 录制循环

```
33fps（30ms interval）+ delta-time 精确帧率控制：
  start_time = Instant::now()
  ...
  elapsed = start_time.elapsed()
  if elapsed < 30ms:
    sleep(30ms - elapsed)
```

## 3. 拼接引擎（2D SAD 空间模板匹配）

### 3.1 从 FFT 相位相关迁移到 2D SAD

**FFT 相位相关的问题**：
- 1D 投影丢失了 2D 空间信息（文字字符的排布），周期性列表行产生多个高分峰值
- 置信度阈值难以平衡：太低→误匹配，太高→跳帧

**2D SAD 的优势**：
- 直接在全量区间 [-220, 0] 做 2D 像素级 SAD 块匹配
- 保留 2D 文字排布特征，真实对齐点 SAD 绝对最小
- 无状态：不需要速度预测/惯性跟踪，每帧独立全局搜索

### 3.2 算法流程

```
1. 灰度转换：reference_gray + curr_gray
2. 区域裁剪：排除左 10%（图标/树状图）+ 右 20%（滚动条/时间戳）
3. 模板：reference 底部 80px strip
4. 静止锚点：计算 dy=0 处 avg_sad_0
   → 如果 avg_sad_0 < 2.0 → 判定为静止，跳过
5. 全量搜索 [min_y_offset, max_y_offset]：
   step_x = 2（每 2 列采样，双倍空间解析度）
   找到 SAD 最小的 best_y_offset
6. 静止锚点交叉验证：
   如果 avg_sad_0 < min_sad + 1.0 → 仍是静止，跳过
   （防止减速/弹跳时的周期性假匹配）
7. 置信度估计：
   最佳 SAD vs 其他偏移的均值比例
8. 阈值：min_sad < 4.5 且 confidence > 0.20
```

### 3.3 配置

```rust
StitchConfig {
    min_scroll_px: 2.0,    // 最小有效位移
    min_confidence: 0.25,  // 置信度阈值（实际验证调高到 0.25）
}
```

### 3.4 Sticky 处理

- 首帧 vs 第二帧逐行比较，检测 sticky header/footer
- canvas 初始化时裁掉底部 sticky_bottom（保留顶部 sticky_top 作为上下文）
- finalize 时补全最后一帧的 sticky footer

## 4. 跨平台 trait（预留）

```rust
pub trait ScrollOverlay: Send {
    fn create(monitors: &[MonitorInfo]) -> Self;
    fn set_scroll_through(&self, enabled: bool);
    fn window_ids(&self) -> Vec<u64>;
    fn destroy(self);
}
```

| 平台 | 窗口 | 滚轮穿透 | 截图排除 |
|---|---|---|---|
| macOS | NSWindow + NSView | setIgnoresMouseEvents | CGWindowList + windowNumber |
| Windows | HWND + WS_EX_LAYERED | WS_EX_TRANSPARENT | SetWindowDisplayAffinity |
| Linux/X11 | override-redirect | 不设输入掩码 | 截图时短暂隐藏 |

## 5. 窗口清理

录制结束（stop/ESC）后：
- `performSelectorOnMainThread:close`（主线程关闭，避免后台线程操作 NSWindow）
- `Retained::into_raw` 放弃 Rust 所有权，close 后 ARC 自动释放

## 6. 托盘入口

托盘菜单「滚动截屏」切换：
- 空闲 → `scroll_capture::start(on_complete)`
- 录制中 → `scroll_capture::stop()`

on_complete 回调在 desktop crate 中处理入库（insert_image_data + insert_clipboard_item + emit clipboard://changed + 写系统剪贴板）。

## 7. 验证结果

- ✅ 原生 NSView 选区拉框
- ✅ 滚轮穿透（setIgnoresMouseEvents 原生可靠）
- ✅ CGWindowList 排除覆盖窗口
- ✅ 底层应用不被 Occlusion Throttling 挂起（CGContextClearRect 真正擦除）
- ✅ 2D SAD 拼接引擎稳定（无重叠/缺失/模糊）
- ✅ 快速滚动、暂停、弹跳都正确处理
- ✅ 窗口清理不崩溃
