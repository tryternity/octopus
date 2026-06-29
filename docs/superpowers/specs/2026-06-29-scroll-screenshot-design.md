# 截图三期：滚动截屏设计

**日期**: 2026-06-29
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`

## 0. 概述

截图三期实现滚动截屏——用户框选区域后点击工具栏「滚动截图」按钮进入录制模式，手动滚动触控板，程序持续截取选区并通过 NCC 像素匹配拼接为长图，右侧/左侧实时预览。用户点击「停止滚动」结束，长图替换选区内容，回到正常截图流程（可标注/确认/保存）。

基于 `imageproc`（Sobel 梯度 + NCC 模板匹配）实现帧间拼接，参考 DigitShot 的 `stitch.rs`。新增 `crates/capx/src/stitch.rs` 拼接引擎。

## 1. 架构

```
选区确定 → 点击工具栏「滚动截图」按钮
         │
         ▼
┌─────────────────────────────────────┐
│  1. 进入滚动录制模式                  │
│     - 工具栏按钮变为「停止滚动」       │
│     - 旁边弹出实时预览窗口            │
│     - 选区边框变为绿色（录制中）       │
├─────────────────────────────────────┤
│  2. 用户手动滚动触控板               │
│     - 程序持续截取选区（~15fps）      │
│     - 每帧与上一帧做 NCC 匹配        │
│     - 检测重叠量 → 裁剪新内容        │
│     - 追加到长图 canvas              │
│     - 预览窗口实时更新               │
├─────────────────────────────────────┤
│  3. 用户点击「停止滚动」              │
│     - 生成最终长图 PNG               │
│     - 预览窗口关闭                   │
│     - 回到正常截图状态（可标注/确认）  │
└─────────────────────────────────────┘
```

### 新增模块

```
crates/capx/src/stitch.rs           # 拼接引擎：NCC 匹配 + 粘性检测 + 图像拼接
crates/desktop/src/screenshot_commands.rs  # start/stop_scroll_recording 命令
crates/desktop/frontend/src/pages/Screenshot/
  ├── ScrollPreview.tsx             # 实时预览组件（DOM 浮层）
  └── index.tsx                     # 滚动模式状态机扩展
```

### 依赖

- `imageproc = "0.25"`（Sobel 梯度 + NCC 模板匹配）
- 已有：`image`、`xcap`

## 2. 拼接引擎（stitch.rs）

### 2.1 数据结构

```rust
pub struct Stitcher {
    canvas: RgbaImage,          // 当前拼接结果（不断增长的长图）
    last_frame: GrayImage,      // 上一帧的边缘图（Sobel 梯度）
    sticky_top: u32,            // 粘性 header 高度（像素行数）
    sticky_bottom: u32,         // 粘性 footer 高度
    active_cols: Range<u32>,    // 活跃列范围（排除静态侧边栏）
    last_delta: i32,            // 上次重叠量（惯性预测）
    low_conf_streak: u32,       // 连续低置信帧数
}

pub struct StitchConfig {
    template_ratio: f32,    // 模板高度 = 有效高度 × 0.2
    min_confidence: f32,    // NCC 最低阈值 0.5
    inertia_px: i32,        // 惯性搜索窗口 ±100
    max_lowconf_streak: u32,// 连续低置信上限 8 帧
}
```

### 2.2 处理一帧的流程

```
新帧 RGBA
  │
  ├─ 1. 预处理：RGBA → 灰度 → Sobel 梯度
  ├─ 2. 首帧：初始化 sticky header/footer + active cols
  │     - sticky_top：比较首帧和第二帧，找顶部不变的行数
  │     - sticky_bottom：同理底部
  │     - active_cols：比较两帧差异，定位变化的列范围
  ├─ 3. 重复帧检测：稀疏采样比较（step=8），均值 < 2.0 → 跳过
  ├─ 4. NCC 模板匹配：
  │     - 从上一帧底部取模板（高度 = 有效高度 × 20%）
  │     - 在当前帧顶部 ±inertia_px 范围内搜索
  │     - 置信度 ≥ 0.5 → 命中；< 0.5 → 全范围重搜
  ├─ 5. 拼接：裁剪当前帧的非重叠行 → 追加到 canvas
  └─ 6. 更新状态：last_delta、缓存边缘图
```

### 2.3 粘性 header/footer 处理

- 初始化时检测（首对帧逐行比较，相同的顶部行 = sticky header）
- 每帧裁掉 sticky 区域后再做匹配
- 最终长图只在最顶部和最底部各保留一次

### 2.4 活跃列检测

- 比较两帧差异，哪些列在变化 = 滚动内容区域
- 只在活跃列范围内做 NCC（排除静态侧边栏干扰）

### 2.5 降级策略

- 连续低置信帧 < 8：保持上次 delta 硬拼接
- 连续低置信帧 ≥ 8：停止拼接，emit 警告，等待用户停止

## 3. 实时预览窗口（ScrollPreview）

### 3.1 位置

默认选区右侧，空间不足放左侧：
- `previewRight = sel.x + sel.w + 12 + 200 <= window.innerWidth`
- 右侧：`x = sel.x + sel.w + 12`
- 左侧：`x = sel.x - 12 - 200`
- `y = sel.y`

```
空间充足（默认右侧）：              空间不足（左侧）：
┌────────┬──────────┐            ┌──────┬────────┐
│        │ 预览     │            │ 预览 │        │
│ 选区   │ ┌──────┐ │            │┌────┐│ 选区   │
│        │ │ 长图  │ │            ││长图 ││        │
│        │ │      │ │            ││    ││        │
│        │ └──────┘ │            │└────┘│        │
│        │ 停止     │            │停止  │        │
└────────┴──────────┘            └──────┴────────┘
```

### 3.2 属性

- 宽度固定 200px（选区宽度的缩略图）
- 高度自适应内容（随拼接增长，最大不超屏幕高度的 80%）
- 顶部状态条：绿色圆点 + 「录制中」+ 已拼接高度（px）
- 追踪丢失：红色圆点 + 「追踪丢失」
- 底部「停止」按钮（等同于工具栏的停止按钮）
- 拼接图像通过 `canvas.toDataURL()` 缩放到预览宽度渲染

### 3.3 更新频率

- 拼接引擎每处理一帧 → emit `scroll://frame`（canvas base64）
- 前端监听事件 → 预览 `<img>` 替换 src（~15fps）

## 4. 数据流与状态机

### 4.1 前端状态机扩展

```
Mode 增加: "scrolling"

selected → 点击「滚动截图」按钮 → scrolling
    │                                    │
    │     ┌──────────────────────────────┤
    │     │ scrolling 模式：              │
    │     │ - 后端启动录制循环            │
    │     │ - 前端监听 scroll://frame     │
    │     │ - 预览窗口实时更新            │
    │     │ - 工具栏变为「停止滚动」      │
    │     │ - 选区边框变绿色              │
    │     └──────────────────────────────┤
    │                                    │
    │                              点击「停止滚动」
    │                                    │
    ├←───────────────────────────────────┘
    │
    ▼
selected（回到正常状态，长图作为选区内容）
```

### 4.2 后端录制循环

```
start_scroll_recording(label, x, y, w, h)
  │
  ├─ 1. 初始化 Stitcher（首帧 → 检测 sticky/active cols）
  ├─ 2. 循环（~15fps）：
  │     a. xcap capture_region(x, y, w, h) → RGBA
  │     b. stitcher.process_frame(rgba) → Option<new_rows>
  │     c. 有新行 → emit("scroll://frame", canvas_base64)
  │     d. 无新行（重复帧）→ 跳过
  │     e. 低置信连续 ≥ 8 → emit("scroll://warning", "追踪丢失")
  ├─ 3. stop_scroll_recording → 结束循环
  │     a. 最终 canvas → PNG bytes
  │     b. 入库（WebP BLOB + 剪贴板历史）
  │     c. emit("scroll://done", item_id)
  └─ 4. 前端收到 done → 回到 selected 模式
```

### 4.3 Tauri 命令

```rust
start_scroll_recording(label: String, x: f64, y: f64, w: f64, h: f64) → ()
stop_scroll_recording() → ()
```

### 4.4 事件

- `scroll://frame` — 每帧拼接后的 canvas base64（预览更新）
- `scroll://warning` — 追踪丢失警告
- `scroll://done` — 录制结束（含最终图片信息）

## 5. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 用户滚动太快 | 帧间重叠 < 20% → 置信度低于阈值 → 该帧丢弃不拼接 |
| 追踪连续丢失（≥8帧） | 预览窗口红色「追踪丢失」+ 选区边框变红 |
| 动态内容（广告/动画） | NCC 置信度低 → 帧被跳过，长图可能有缺口 |
| 水平偏移 | 检测到水平 delta > 5px → 该帧丢弃（仅支持垂直） |
| 选区含固定 header | 初始化时检测并裁剪，每帧跳过 sticky 区域 |
| 反向滚动（向上） | delta 为负 → 跳过该帧（不支持向上修正） |
| 长图过大（> 10000px） | 拼接正常但预览窗口限制显示高度，最终入库不受限 |
| 停止后选区内容 | 长图替换原选区底图，可继续标注/确认/保存 |
| 截图窗口关闭 | 录制循环自动停止（窗口不存在检测） |

**降级**：滚动截图失败不影响普通截图功能。start_scroll_recording 返回 Err → 回到 selected 模式 + toast 提示。

## 6. 实施分期

| 阶段 | 范围 |
|---|---|
| **Step 1** | capx/stitch.rs 拼接引擎（Stitcher + NCC + sticky/active cols + 单元测试） |
| **Step 2** | 后端录制循环 + start/stop_scroll_recording 命令 + 事件 |
| **Step 3** | 前端状态机 + 预览窗口 + 工具栏按钮 |
| **Step 4** | 端到端验证 |

## 7. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| NCC 匹配性能不足 | 中 | 帧率下降 | coarse-to-fine 搜索 + 活跃列裁剪 |
| 粘性元素检测不准 | 中 | 长图有重复 | 手动阈值调整 + 用户可接受小幅重复 |
| xcap capture_region 权限 | 低 | 无法截取选区 | 已有一期权限验证 |
| 预览窗口性能 | 低 | 卡顿 | 限制预览更新频率到 15fps |
