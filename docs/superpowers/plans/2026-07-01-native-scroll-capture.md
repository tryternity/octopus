# 原生 NSView 滚动截屏验证实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 独立 crate 原生 NSView 实现滚动截屏，验证滚轮穿透/截图排除/拼接可靠性

**Architecture:** NSWindow + NSView 原生覆盖窗口 + CGWindowList 截图 + 2D SAD 模板匹配拼接

**Tech Stack:** Rust + objc2/objc2-app-kit + core-graphics + capx (stitch)

**Spec:** `docs/superpowers/specs/2026-07-01-native-scroll-capture-design.md`

---

## Task 1-6: ✅ 全部完成

- [x] crate 骨架 + ScrollOverlay trait
- [x] macOS NSWindow + NSView 选区拉框（define_class! + mouseDown/Dragged/Up + drawRect）
- [x] CGWindowList 截图 + 排除自身
- [x] 焦点让出（get_window_pid_at_point + activateWithOptions）
- [x] 录制循环 + 拼接（33fps delta-time pacing）
- [x] desktop crate 集成（托盘菜单 + on_complete 入库）

## Task 7: ✅ 端到端验证通过

- [x] 托盘「滚动截屏」→ 覆盖窗口出现
- [x] 拖拽拉框选区
- [x] 选区确定 → 绿色边框 → 底层应用可滚动
- [x] 滚动 → 后台截图拼接
- [x] 托盘停止 → 长图入库
- [x] 拼接质量验证（无重叠/模糊/缺失）

---

## 实施偏差记录

### 偏差 1：drawRect 暗遮罩残留 → 截到白色 ✅

NSView 从"暗遮罩"切换到"录制"时，之前画的半透明像素残留在屏幕上 → CGWindowList 排除覆盖窗口后截到白色（底层应用被遮挡 → Occlusion Throttling 挂起）。

修复：`isOpaque = false` + 每帧 `drawRect` 开始时用 `CGContextClearRect`（C FFI）真正擦除。

### 偏差 2：窗口清理崩溃 ✅

后台线程操作 NSWindow（`orderOut`/`close`）→ Trace/BPT trap。

修复方案迭代：
1. `catch_unwind` → 不能捕获 foreign exception
2. `drop(windows)` → ARC 释放但窗口不关闭
3. **最终方案**：`Retained::into_raw` + `performSelectorOnMainThread:close`

### 偏差 3：托盘菜单重复触发 ✅

每次点「滚动截屏」都创建新覆盖窗口 → 屏幕层层变暗。

修复：`is_recording_active()` 切换逻辑——录制中点击→停止，空闲→启动。

### 偏差 4：FFT 相位相关拼接不稳定 ✅

1D 投影丢失 2D 空间信息 → 周期性列表行假匹配 → 重叠/缺失。

**最终方案**：2D SAD 空间模板匹配：
- 全量区间 [-220, 0] 2D 块匹配
- 静止锚点交叉验证（avg_sad_0 vs min_sad）
- 高分辨率模板（strip_h=80, step_x=2）
- 严格阈值（min_sad < 4.5, confidence > 0.20）

### 偏差 5：帧率优化 ✅

从固定 100ms sleep 改为 delta-time pacing（33fps/30ms），用 `Instant` 计算实际耗时 + 补偿 sleep。

---

## 后续工作

| 功能 | 优先级 | 说明 |
|---|---|---|
| 预览面板 | 中 | NSView 子类，底部固定，显示拼接进度 + 保存/复制/取消 |
| 坐标多屏修正 | 中 | 副屏 Y 翻转可能需要进一步验证 |
| 清理调试日志 | 低 | 去掉 frame center pixel + debug frame 保存 |
| 替换现有 WebView 滚动截图 | 中 | 确认原生方案稳定后替换 screenshot_commands.rs |
| Windows/Linux 实现 | 低 | 二期 |
| 标注工具原生重写 | 低 | 三期 |
