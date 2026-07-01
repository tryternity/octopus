# 截图三期：滚动截屏设计

**日期**: 2026-06-29（2026-06-30 重写拼接引擎为 FFT 相位相关）
**状态**: manual 模式可用，FFT 拼接引擎稳定
**分支**: `feature/clipboard-research`

## 0. 概述

截图三期实现滚动截屏——用户框选区域后点击工具栏「滚动截图」按钮进入录制模式，在选区外手动滚动触控板/滚轮，后端 33fps 截帧 + **1D FFT 相位相关**亚像素位移估计 + 拼接成长图。

### 核心架构

**置顶透明覆盖层事件穿透 + 33fps 增量捕获 + 1D FFT 相位相关亚像素拼接**

## 1. 系统层

### 1.1 透明窗口 + Occlusion Throttling 破解

- `.transparent(true)` + 前端 `ctx.clearRect(x, y, w, h)` 挖 100% 透明孔
- macOS Window Server 识别底层窗口"可见"，保持底层应用持续 repainting

### 1.2 Key Window 焦点让出

- `save_frontmost_app()` 暂存前台 app PID
- `activate_prev_app()` 主线程 `NSRunningApplication.activateWithOptions` 激活前台 app
- 30ms 独立线程轮询鼠标位置 + 周期性检测选区下方应用并激活

### 1.3 坐标映射

- NSWindow Cocoa frame（原点左下）+ 主屏高度翻转 → Quartz 坐标
- `CGWindowListCreateImage` 只截选区 + 排除截图窗口
- `target_wid` 用选区中心点检测下方应用窗口（`get_window_pid_at_point`），而非 PREV_ACTIVE_APP

### 1.4 选区下方应用自动激活

- 30ms 监视线程每 ~500ms 用 `CGWindowListCopyWindowInfo` + bounds 命中检测鼠标下方应用
- 跳过 `kCGWindowLayer != 0`（桌面壁纸、Dock、菜单栏）
- 通过 PID `activateWithOptions` 激活（`run_on_main_thread` 主线程执行）
- 用户不需要先点击目标应用，直接在选区内滚动即可

## 2. 拼接引擎（stitch.rs）— 1D FFT 相位相关

### 2.1 核心算法

```
首帧 → 初始化 canvas
第二帧 → detect_sticky → 裁掉 canvas 的 sticky 区域 → 用第二帧有效区域初始化参考投影
后续帧 →
  ① Sobel 边缘 → 垂直投影（每行平均边缘强度）→ 1D 信号
  ② 1D FFT 相位相关（参考帧 vs 当前帧）：
     a. FFT(a), FFT(b)
     b. R = conj(Fa) * Fb / |conj(Fa) * Fb|  （归一化互功率谱）
     c. IFFT(R) → 峰值位置 = 位移 dy（亚像素）
     d. 抛物线拟合精化：0.1px 精度
  ③ dy < 0（内容上移 = 用户向下滚）→ 追加当前帧底部 |dy| 行到画布
  ④ 更新参考投影为当前帧
停止 → finalize：补全最后一帧的 sticky footer 区域 → emit 最终预览
```

### 2.2 相比 NCC 的优势

| 维度 | NCC 模板匹配 | FFT 相位相关 |
|---|---|---|
| 精度 | 整数像素 | 亚像素（抛物线拟合 ~0.1px） |
| 周期性内容 | 模板在 d、d±45 处得分接近→假匹配 | 频率域全局主峰→鲁棒 |
| 计算复杂度 | O(W×H×搜索范围) | O(N log N)，N=选区高度 |
| 模板选择 | 需找"纹理丰富"的 strip | 全图参与，无需选模板 |

### 2.3 数据结构

```rust
pub struct Stitcher {
    canvas: RgbaImage,
    reference_proj: Vec<f64>,   // 参考帧的 1D 垂直投影（有效区域）
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
}

pub struct StitchConfig {
    min_scroll_px: f64,    // 最小有效滚动（2.0px）
    min_confidence: f64,   // 相位相关峰值置信度（0.15）
}
```

### 2.4 Sticky 处理

- `detect_sticky`：首帧 vs 第二帧逐行比较，找顶部/底部不变的行
- canvas 初始化时**裁掉 sticky 区域**（只保留有效内容）
- `finalize`：停止时补全最后一帧的 sticky footer

### 2.5 位移方向

`R = conj(Fa) * Fb`：
- 用户向下滚动 → 当前帧内容上移 → 峰值在 `n-d` → `dy = d-n < 0`
- `dy < 0` → 向下滚动 → 追加当前帧底部 `|dy|` 行

### 2.6 降级策略

| 场景 | 处理 |
|---|---|
| 画面静止 | dy 接近 0 → 跳过 |
| 置信度低 | conf < 0.15 → 跳过 |
| 向上滚动 | dy > 0 → 跳过 |
| 滚动超过选区高度 | 异常 → 跳过 |

## 3. 录制循环

```
start_scroll_recording(x, y, w, h, win_label, interactive_rects)
  │
  ├─ 1. set_ignore_cursor_events(true) + activate_prev_app()
  ├─ 2. 30ms 鼠标监视线程（预览区域切换 ignore + 自动激活选区下方应用）
  ├─ 3. target_wid = 选区中心检测下方应用窗口
  ├─ 4. Stitcher::new(首帧)
  ├─ 5. 循环（30ms / 33fps）：
  │     a. capture_region_excluding_window(target_wid) → RGBA
  │     b. JPEG → emit("scroll://frame")
  │     c. stitcher.process_frame(rgba) → FFT 相位相关
  │     d. 预览（底部裁剪 400px CatmullRom）→ emit
  └─ 6. 先恢复鼠标事件 + activate(self)
       → stitcher.finalize() + spawn_blocking 最终预览 → emit
       → 长图入库
```

### 3.1 停止流程（防鼠标假死）

1. **先恢复鼠标**：`setIgnoresMouseEvents(false)` + `activate(self)`
2. **再 finalize + 预览**：`spawn_blocking` 中生成，不阻塞 tokio
3. **入库**：WebP 编码写 DB

## 4. 前端

### 4.1 滚动模式 UI

scrolling 模式下：
- **工具栏完全隐藏**（操作按钮在预览面板中）
- **选区遮罩用 DOM div**（不经过 Canvas，避免选区内像素残留变暗）
- **Canvas 只画绿色边框**
- **取消按钮图标红色**（CSS filter 染色）
- **滚动截屏按钮使用 `icons/scroll.svg`**

### 4.2 预览面板（HUD 风格）

```
┌──────────────────┐
│ ● REC    1234px  │  ← 琥珀色脉冲点 + 等宽数字高度
├──────────────────┤
│ 预览图            │  ← 底部裁剪，最新内容可见
│ ↑↑↑              │
├──────────────────┤
│ [停止录制] [取消]  │  ← 2:1 比例，等高 32px
└──────────────────┘
   ↑ 底部对齐选区底部
```

- 毛玻璃面板：`rgba(15,15,17,0.92)` + `backdrop-filter: blur(16px)`
- 按钮带 hover 过渡
- 停止录制 flex:2（红色实心），取消 flex:1（透明描边）

### 4.3 interactiveRects

scrolling 模式下只有预览面板区域（工具栏已隐藏），传给后端 30ms 监视线程用于鼠标穿透切换。

## 5. 托盘菜单

### 5.1 菜单结构

```
语音识别（⌘⇧A）
引擎  xxx · xxx
───────────────
开始截图（⌘⇧D）
剪  贴  板（⌘⇧F）
记  事  本
───────────────
系统管理
退出系统
```

- 分组用分隔线（PredefinedMenuItem::separator）
- 快捷键从 config 读取，Tauri Accelerator 格式转为符号（⌘⇧A 等）
- "剪  贴  板"和"记  事  本"每字间双半角空格对齐四字宽度
- 截图进行中菜单灰掉（`set_enabled(false)`），不改文字避免跳动
- 语音识别状态切换时保留快捷键（Idle 带快捷键，Recording/Processing 不带）

## 6. 依赖

- `rustfft = "6.2"`（FFT 相位相关）
- `imageproc = "0.25"`（Sobel 梯度）
- `objc2` + `objc2-app-kit`（焦点让出 + 应用激活）
- `core-graphics = "0.24"` + `core-foundation = "0.10"`（CGWindowList 截图 + 窗口信息查询）
