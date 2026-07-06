# 已归档设计规格（2026-06-29 ~ 2026-07-02）

> 以下功能均已实现并合并 main（或已下线归档）。本文件由 11 份原独立 spec 文件合并归档，原文件已删除。
> spec↔plan 旧路径交叉引用随归档失效，按主题在 plans/2026-07-02-archived-plans.md 内查同名章节。

## 目录

- 2026-06-29-scroll-screenshot-design.md
- 2026-06-30-compact-editor-design.md
- 2026-06-30-notepad-design.md
- 2026-06-30-scroll-stitch-research.md
- 2026-07-01-image-preview-design.md
- 2026-07-01-pin-screenshot-design.md
- 2026-07-02-capx-canvas-anchored-design.md
- 2026-07-02-capx-ncc-sobel-design.md
- 2026-07-02-capx-stitch-robustness-design.md
- 2026-07-02-ipc-binary-design.md
- 2026-07-02-notepad-type-migration-design.md


---

## 来自原文件 `2026-06-29-scroll-screenshot-design.md`

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
│ [保存] [复制] [取消]│  ← 等宽三按钮，等高 32px
└──────────────────┘
   ↑ 底部对齐选区底部
```

- 毛玻璃面板：`rgba(15,15,17,0.92)` + `backdrop-filter: blur(16px)`
- 按钮带 hover 过渡
- 三按钮等宽：保存(蓝 #3b82f6) / 复制(绿 #22c55e) / 取消(透明描边)
- 停止模式通过 `stop_scroll_recording_with_mode(mode)` 传后端：
  - **保存(save)**：入库 + 写系统剪贴板 + 弹 `blocking_save_file` 对话框
  - **复制(copy)**：入库 + 写系统剪贴板（`handle.write_image`）
  - **取消(cancel)**：不入库，直接关窗口

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


---

## 来自原文件 `2026-06-30-compact-editor-design.md`

# 精简编辑器（Compact Editor）设计

> 日期：2026-06-30
> 状态：**已实现**（OCR/剪贴板走独立精简编辑器窗；语音 Result 改为原地编辑框尺寸双模式）。后端 `cargo test` 全绿；e2e 待用户手动验证。详见 plan 顶部「实现状态」。
> 关联：`docs/superpowers/plans/2026-06-30-compact-editor.md`
> 关联：`docs/superpowers/specs/2026-06-30-notepad-design.md`（完整版记事本，本设计与之并列、不替代）

> **2026-06-30 设计修订（语音 Result 改为原地双模式）：** 原 §3.5① 让语音 Result「展开编辑」弹独立精简编辑器窗——**取消**。
> 语音 Result 改为**编辑框尺寸双模式**（精简 520×116 小条 / 长篇 720×480 撑满）+ 工具栏「放大/缩小」开关切换。窗口物理固定 720×480（setSize 在透明无边框悬浮窗被 NSWindow 拒绝，改 CSS 伪装切容器尺寸 + 透明区点击穿透）。Result **不再调用** `openCompactEditor`。
> 精简编辑器（独立窗）**仅保留给 OCR 与剪贴板文本**。详见 §3.5① 重写。

## 1. 背景与目标

完整版记事本（notepad_window）已实现，但本期更需要一个**精简编辑器**：纯编辑工具，只有「轻工具栏 + 文本正文区」，没有标题输入框、没有左侧分类侧栏、**不持久化为笔记**——编辑结果**还给调用方**。

核心诉求：
1. **OCR 识别**后，用户目前只能在剪贴板浮窗的小框里改文本，不舒服。希望「点编辑 → 迅速展开成舒适的大编辑器」。
2. **剪贴板文本条目**（含 ASR 语音文本）希望「快速打开编辑」。

本设计提供一个可被三处复用的纯文本编辑器窗口，编辑后的文本通过事件返回给发起调用的窗口。

## 2. 范围

**做：**
- 新建独立窗口 `compact_editor_window`（纯文本编辑器）。
- 顶部轻工具栏：撤销/重做、字号 −/+、字符计数、查找/替换、清空、保存/取消。
- 跨窗口「文本返回」事件契约（request_id 区分多调用方）。
- 三处集成：① 语音 Result（编辑框**原地双模式** + 放大/缩小开关，**不用独立窗**）；② OCR（替换原系统 TextEdit，用独立窗）；③ 剪贴板文本条目（新增「编辑」按钮，用独立窗）。独立精简编辑器窗仅 ②③ 复用。

**不做（YAGNI）：**
- 不做富文本（TipTap/ProseMirror）——三个调用方都是纯文本，textarea 对中文输入法（IME）友好且所见即所得，足够。
- 不做标题、分类、收藏、搜索——这些属于完整版记事本。
- 不持久化为笔记——编辑器是无状态工具，结果只还调用方。
- 不做窗口 keep-alive——关窗即销毁、再开重建（页面轻，重建快；如日后觉得卡再改 show/hide）。

## 3. 架构

### 3.1 窗口与生命周期

新建窗口管理 `crates/desktop/src/compact_editor_window.rs`，镜像 `notepad_window.rs`：

- `WINDOW_LABEL = "compact_editor_window"`。
- 单例：`get_webview_window` 命中已存在则 `show + set_focus` 并通过事件推送新文本（见 §3.3 并发开窗）；否则创建。
- 创建参数：`.title("编辑")`、`.inner_size(720, 560)`、`.min_inner_size(480, 360)`、`.decorations(true)`（原生标题栏）、`.visible(true)`、`.resizable(true)`、居中。
- macOS 激活策略：开窗切 `Regular`（Dock 显图标），关窗切回 `Accessory`，与 notepad/settings 对称。新增 `on_compact_editor_closed(app)`，并在 `main.rs` 的 `RunEvent::WindowEvent { Destroyed }` 分支按 label 挂载。
- 生命周期：**关窗即销毁**（destroy-on-close，`close()` 触发 Destroyed 走清理）。再次打开重建窗口。

### 3.2 后端命令（`crates/desktop/src/compact_editor_commands.rs`，薄层）

复用 `result_window.rs` 的「写 PENDING → 建窗/聚焦 → 前端 mount 拉取」模式，但 PENDING 携带结构体：

```rust
// 静态 PENDING：open 时写入，前端 mount 时 take。
// 用 Mutex<Option<PendingEdit>>，无 ready 握手——编辑器窗口是按需创建（非预建隐藏窗），
// open 必然「先写 PENDING 再建窗」，React mount 时 get 一定能读到，无 TOCTOU。
static PENDING: Mutex<Option<PendingEdit>> = Mutex::new(None);

struct PendingEdit { text: String, request_id: String }

#[tauri::command]
pub fn open_compact_editor(initial_text: String, request_id: String, app_handle: AppHandle);

#[tauri::command]
pub fn get_pending_compact_edit() -> Option<PendingEdit>;  // 前端 mount 时 take

#[tauri::command]
pub fn close_compact_editor(app_handle: AppHandle);
```

`open_compact_editor` 内部：
1. `*PENDING.lock() = Some(PendingEdit { text, request_id })`。
2. 若窗口已存在：`emit("compact-editor://load", { text, request_id })` + show + focus（前端 mount 时已 take 过首次 PENDING，并发再开走事件推送）。
3. 否则：建窗（React mount → 调 `get_pending_compact_edit` → take → 载入）。

`close_compact_editor`：`get_webview_window(...)?.close()`（触发 Destroyed → macOS 切 Accessory）。

三个命令在 `main.rs` 的 `generate_handler!` 注册（紧邻 `notepad_window::open_notepad`）。

> `PendingEdit` 经 Tauri IPC 序列化为 camelCase：`{ text, requestId }`。前端取 `requestId`。

### 3.3 事件契约（核心：文本如何还回去）

> 仅 OCR（②）与剪贴板文本（③）走此跨窗口契约。语音 Result（①）改为原地双模式编辑，不跨窗口、不参与此契约。

**request_id 由调用方前端生成**（`crypto.randomUUID()`，跨窗口无碰撞）。一次完整握手：

1. **调用方**：生成 `requestId` → `invoke("open_compact_editor", { initialText, requestId })` → 记住 `requestId` → `listen("compact-editor://result", handler)`，handler 内按 `requestId` 过滤命中才应用。
2. **保存**：编辑器 `emit("compact-editor://result", { requestId, text })` → `invoke("close_compact_editor")`。
3. **取消 / X 关窗**：编辑器 `emit("compact-editor://cancel", { requestId })` → 关窗（unmount 时也兜底发一次 cancel，防悬空监听）。
4. **load（并发再开）**：后端向已存在窗口 `emit("compact-editor://load", { text, requestId })`，前端监听载入新文本。

`emit` 广播到所有窗口；各调用方按 `requestId` 过滤，互不串扰。

### 3.4 编辑器组件（`crates/desktop/frontend/src/pages/CompactEditor/index.tsx`）

- 主体：全高 `<textarea>`（IME 友好），填充窗口剩余空间。
- 顶部工具栏（lucide-react 图标，风格对齐剪贴板浮窗）：

  | 工具 | 实现 |
  |---|---|
  | ↶ / ↷ 撤销·重做 | `document.execCommand('undo'/'redo')` 触发 textarea 原生栈（实用；Cmd+Z/Y 原生也生效） |
  | A− / A+ 字号 | 调 textarea `style.fontSize`（可读性），记忆到 localStorage |
  | 123 字符计数 | `[...text].length`（按码点，中文 1 字） |
  | 🔍 查找/替换 | 顶部展开查找条：输入框 + 命中数 + 上一个/下一个 + 替换框 + 替换/全部替换；用 `setSelectionRange` 高亮+滚动 |
  | ⌫ 清空 | 清空 textarea（二次确认） |
  | 保存 / 取消 | 见 §3.3 事件 |

- 快捷键：`Cmd/Ctrl+Enter` 保存、`Esc` 取消、`Cmd/Ctrl+F` 唤出查找。
- mount：`invoke("get_pending_compact_edit")` → 有则载入 `{text, requestId}` 并 focus textarea；同时 `listen("compact-editor://load")` 处理并发再开。
- unmount：兜底 `emit("compact-editor://cancel", { requestId })`（已保存则不发；用 ref 标记 saved 状态区分）。
- 关闭按钮走「取消」语义（X 关 = 不保存）。

### 3.5 三处集成

**① 语音 Result（`pages/Result/index.tsx`）—— 编辑框尺寸双模式（CSS 伪装，不弹独立窗）**

物理窗口固定 **720×480**（`result_window`，`resizable(true)` / `decorations(false)` / `transparent` / `always_on_top`，创建即定死尺寸），前端按模式用 CSS 切「可见容器」尺寸，**全程不调 setSize**——透明无边框悬浮窗上 `setSize`/`setFrame` 被 NSWindow 拒绝（min/max 放宽到 [100,4000]、720×480 在区间内仍读回旧值，实锤），故改 CSS 伪装 + 透明区点击穿透：

| 维度 | 精简态（默认） | 长篇态 |
|---|---|---|
| 物理窗口 | 720×480（固定） | 720×480（同左） |
| 可见容器（CSS） | 顶部居中 520×116 小条 | 撑满 720×480 |
| 文本区 | `max-h-[63px]` | `h-full` 撑满 |
| 透明区 | 小条下方透明（`body{background:transparent}`） | 无 |
| 点击穿透 | 透明区穿透到后方应用 | 整窗可交互 |

- 外层透明包裹 `relative w-full h-full`，内层 `#result-container` 绝对定位 `top-0 left-1/2 -translate-x-1/2`，className 按 `expanded` 切 `w-[720px] h-[480px]`（长篇）或 `w-[520px] h-[116px]`（精简），加 `transition-all duration-200`；`visible` 控 `opacity-0/100` 显隐。
- 工具栏「放大/缩小」开关按钮：精简态「放大」（`expand-edit` 四角向外）→ 长篇；长篇态「缩小」（`minimize` 四角向内）→ 精简。
- `toggleExpand`（纯 CSS，无 setSize）：`setExpanded(next)` + `invoke("set_result_click_through", { expanded: next })`。
- **点击穿透（必须 Rust 轮询）**：精简态小条下方透明区要穿透到后方应用。前端 `setIgnoreCursorEvents(true)` 不可行——一旦 ignore，NSWindow 零鼠标事件、连 tracking area 都禁，前端检测不到光标重新进入小条 → 重入失效。故 `result_window.rs::start_click_through_poller` 后台线程（~33ms）读全局鼠标 `CGEvent.location()`，按窗口 `outer_position()`/`scale_factor` 算小条屏幕矩形，光标在矩形外 → `setIgnoresMouseEvents(true)` 穿透、在内 → `false` 可交互；直调 NSWindow `setIgnoresMouseEvents`（比 Tauri `set_ignore_cursor_events` 封装可靠，复用 `screenshot_commands` 同款）。切长篇时前端调 `set_result_click_through(expanded=true)` 立即关穿透。
- 编辑**完全不变**：仍走现有 `toggleEdit`（`contentEditable` + `enter_edit_mode`/`commit_edit`/`cancel_edit`），两种模式下均可编辑，零新编辑逻辑。
- **移除**原 `openExpandEdit` / `applyResultText` 与 `openCompactEditor` import——Result 不再依赖精简编辑器窗口。
- **移除**「存入记事本」工具按钮 + `saveToNote` 回调——长篇模式已可直接原地大窗口编辑，无需导入记事本（后端 `save_transcription_to_note`/`current_transcription_id` 命令保留作基础设施）。
- Rust 侧 `result_window.rs` 创建 `.resizable(true)` + 固定 `.inner_size(720,480)`；移除 `set_result_window_mode` 命令，新增 `set_result_click_through`。窗口位置仍由 `window_position` 机制保存恢复。
- 边界：长篇态向下展开占满 720×480，若原位置近屏幕底部可能部分超出——MVP 不重算位置（用户可拖动窗口），e2e 观察。

**② OCR（`clipboard_commands.rs::ocr_image` + `ClipboardItem.tsx::handleOcr`）**
- 后端：`ocr_image` **删除** `open_text_editor_with_content(&text)` 调用（不再打开系统 TextEdit）；`open_text_editor_with_content` 函数本身若仅此处引用则一并删除。`ocr_image` 仍返回 `text`（前端拿到）。
- 前端 `handleOcr`：OCR 成功后生成 `requestId` → `invoke("open_compact_editor", { initialText: text(需 ocr_image 返回值), requestId })` → `listen` 按 rid 过滤 → 命中 `invoke("set_clipboard_item_text", { itemId: item.id, text })` + `onChanged()` 刷新列表。
  - 注意：当前 `handleOcr` 调 `ocr_image` 未取返回值，需改为 `const text = await invoke<string>("ocr_image", { id })`。
- `update_search_text`（识别后落 search_text）+ `handle.write_text`（写系统剪贴板）保留——这些是「识别结果」的落库与剪贴板同步，编辑器只负责让用户随后修改文本。

**③ 剪贴板文本条目（`ClipboardItem.tsx`）**
- hover 操作区新增「编辑」按钮（lucide `SquarePen`/`Pencil`，挨着「存入记事本」`NotebookPen`）。
- 显示条件：`item.item_type !== "image" && item.item_type !== "file"`（即文本/语音文本可编辑；图片走 OCR、文件不可编辑）。
- 点击 → 生成 `requestId` → `invoke("open_compact_editor", { initialText: item.content, requestId })` → `listen` 按 rid 过滤 → 命中 `invoke("set_clipboard_item_text", { itemId: item.id, text })` + `onChanged()`。

**共享：`set_clipboard_item_text` 命令（②③共用）**
- 新增 `#[tauri::command] set_clipboard_item_text(item_id: i64, text: String, handle: State<ClipboardHandle>)` 于 `clipboard_commands.rs`：
  1. `octopus_clipboard::store::update_content(conn, item_id, &text)`（新增 store 函数，镜像现有 `update_search_text`，同时写 `clipboard_history.content` 与 `search_text`）。
  2. `handle.write_text(&text)` 同步系统剪贴板。
  3. 注册到 `generate_handler!`。

## 4. 数据流图

```
调用方(OCR/Clipboard)
  │ requestId = uuid()
  │ invoke open_compact_editor(initialText, requestId)
  ▼
compact_editor_commands::open_compact_editor
  │ PENDING = {text, requestId}
  │ 建窗(首次) 或 emit load + focus(已存在)
  ▼
CompactEditor mount  ──get_pending_compact_edit──► PENDING.take()
  │ 用户编辑 textarea（撤销/重做/字号/查找替换/清空）
  │ 保存: emit("compact-editor://result", {requestId, text}) + close
  ▼
调用方 listen(result) ──rid 命中──► 应用文本
  • OCR/Clipboard → set_clipboard_item_text(itemId, text)

（语音 Result 不走此流——原地双模式编辑，见 §3.5①）
```

## 5. 错误处理与边界

| 场景 | 处理 |
|---|---|
| X 关窗 / unmount | emit cancel（rid 兜底），调用方清 pending 监听，不应用 |
| 并发再开（A 开着，B 再开） | 后端 emit load 推 B 的 {text,rid}；A 的 rid ≠ B 的 rid，A 的 listener 不命中 → A 不应用（无害） |
| 空文本 | textarea 正常，保存返回空字符串，调用方按需处理（Result/Clipboard 接受空） |
| 超长 OCR 文本 | textarea 原生滚动 + 字数提示，无上限 |
| 中文输入法 | textarea 原生 IME 安全（优于 contentEditable/TipTap） |
| 编辑器窗口被系统关闭（非取消按钮） | unmount 兜底 emit cancel |
| OCR 未识别到文本 | `ocr_image` 现已 `Err`，前端 catch 走原 `ocrDone` 提示，不开编辑器 |

## 6. 测试

**后端单测（`compact_editor_commands.rs`）：**
- `open_compact_editor` 写 PENDING → `get_pending_compact_edit` 读回正确 `{text, requestId}` 并 take 清空。窗口创建本身是 Tauri 集成层，不单测。
- `set_clipboard_item_text`：调用后 `clipboard_history.content` 与 `search_text` 均更新（用 `_at` 内存 DB 变体，镜像现有剪贴板 store 测试）。
- `update_content` store 函数单测（`crates/clipboard`）。

**前端单测（CompactEditor）：**
- 字符计数、字号增减、查找/替换匹配与高亮逻辑（纯函数抽离可单测）。
- 保存/取消 emit 事件（mock `invoke`/`emit`，断言 payload `{requestId, text}`）。

**e2e（手动，跨窗口+IME，单测覆盖不到）：**
1. Result（双模式）：识别中文 → 点「放大」切长篇 → 可见容器撑满 720×480、编辑区撑满 → 编辑 → 保存 → 文本落库 → 点「缩小」切回 520×116 小条；精简态小条下方透明区可点击穿透到后方应用。
2. OCR：图片识别 → 自动开编辑器 → 改文本 → 保存 → 确认剪贴板条目内容 + 系统剪贴板均更新。
3. 剪贴板文本条目：点「编辑」→ 改 → 保存 → 确认列表与系统剪贴板更新。
4. 边界：取消 / X 关窗不应用文本；并发开窗不串扰。

## 7. 文档同步

- `docs/architecture.md`：crate 树与窗口列表新增 `compact_editor_window`；命令清单加 `open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor` / `set_clipboard_item_text`。
- 本 spec → `docs/superpowers/plans/2026-06-30-compact-editor.md`（writing-plans 产出）。

## 8. 与完整版记事本的关系

- 完整版 notepad_window 保留不动（未来需要标题/分类/富文本/持久化时用它）。
- 精简编辑器是「纯编辑工具」，与 notepad 不共享状态、不共享窗口、不互调。
- 唯一共享资源是 `image_data`（已由 C1 修复保证 note-img 引用不被剪贴板清理误删）；精简编辑器只处理纯文本，不涉及图片。


---

## 来自原文件 `2026-06-30-notepad-design.md`

# 记事本（内容收集箱）功能设计

**日期**: 2026-06-30
**状态**: 设计稿，待评审
**分支**: `worktree-feature-notepad`（worktree: `.claude/worktrees/feature-notepad`）

## 0. 概述

为 octopus 新增一个「内容收集箱」式的记事本：ASR / OCR / 转译记录的识别结果可一键存入记事本做整理，并在记事本内继续编辑。形态为独立窗口（左侧笔记列表 + 右侧富文本编辑器）。

- **编辑格式**：富文本为内部模型（所见即所得），Markdown / 纯文本作为序列化与导入导出格式——一个引擎三种格式互通。
- **存入语义**：每次「存入记事本」= 新建一条笔记，自动记录来源（语音 / OCR / 剪贴板）与时间戳，关联原记录 id，来源徽标可点击回溯。
- **技术选型**：前端富文本引擎用 TipTap（基于 ProseMirror，React 生态最成熟，有 markdown 序列化扩展）。

后端为新建独立 crate `octopus-notepad`（仅依赖 `octopus-infra`），承载笔记全部业务逻辑；`infra/db` 加表与迁移；`desktop` 加薄 Tauri command 层 + 窗口 + 前端页面。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── notepad/            # octopus-notepad — 新增，仅依赖 infra
│   ├── Cargo.toml      # infra, scraper（HTML→text）, anyhow
│   └── src/
│       ├── lib.rs      # pub use model / store / serialize / export
│       ├── model.rs    # Note / NoteSource / NoteFilter / NoteSort
│       ├── store.rs    # CRUD + FTS 搜索（infra::with_db）
│       ├── serialize.rs# content_html → content_text 抽取（scraper）
│       └── export.rs   # 导入/导出文件 I/O（~/Documents/octopus/notes/）
├── infra/              # db：加 notes / notes_fts 表 + v9 迁移 + 触发器
└── desktop/            # Tauri 命令 + 窗口 + 前端
    ├── src/
    │   ├── note_commands.rs   # 薄 command 层，转调 octopus-notepad
    │   └── notepad_window.rs  # notepad_window 窗口管理
    └── frontend/src/
        ├── pages/Notepad/
        │   ├── index.tsx          # 三栏布局
        │   ├── NoteList.tsx       # 列表 + 搜索 + 来源筛选 + 分页
        │   ├── NoteEditor.tsx     # TipTap 编辑器 + 工具栏 + 自动保存
        │   └── editor/extensions.ts # TipTap 扩展 + Image NodeView
        ├── lib/notepad.ts         # invoke 封装
        └── hooks/useNotes.ts      # 列表状态 + notepad://changed 监听
```

**依赖关系**：`infra ← notepad ← desktop`

### 1.2 为什么独立 crate

与 `octopus-clipboard` / `octopus-ocr` / `octopus-capx` 一致：核心能力做成仅依赖 `infra` 的独立 crate，业务逻辑（CRUD、序列化、文件 I/O）下沉到 crate，`desktop` 只留薄 command 层与 UI。好处：逻辑可单测、可被未来 cli/server 复用、与 desktop 的 Tauri/前端耦合解耦。

`infra` 只承担最底层——表 schema、迁移、`with_db` 访问入口（与 `clipboard_history` 表同级）。所有笔记业务逻辑在 `octopus-notepad`。

### 1.3 模块边界

| 单元 | 职责 | 依赖 |
|---|---|---|
| `infra::db` | notes/notes_fts 表 schema + v9 迁移 + 触发器 + `with_db` | 无 |
| `notepad::model` | 数据结构定义 | infra |
| `notepad::store` | CRUD + FTS 搜索 + 排序分页 | infra |
| `notepad::serialize` | HTML→纯文本抽取（生成 content_text，后端为 source of truth） | scraper |
| `notepad::export` | 导入读文件 / 导出写文件 | std |
| `desktop::note_commands` | Tauri command 转调 notepad crate | notepad |
| 前端 `Notepad/*` | UI + TipTap 编辑器 | react, @tiptap/* |

## 2. 数据模型

### 2.1 `notes` 表（infra/db，v9 迁移新增）

```sql
CREATE TABLE IF NOT EXISTS notes (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  title         TEXT,                         -- 可空，空则列表显示正文截取
  content_html  TEXT    NOT NULL DEFAULT '',   -- 富文本内部格式（TipTap getHTML）
  content_text  TEXT    NOT NULL DEFAULT '',   -- 纯文本抽取，FTS 索引 + 列表预览
  source        TEXT    NOT NULL DEFAULT 'manual',  -- asr/ocr/clipboard/manual
  source_ref_id INTEGER,                       -- 关联 transcription_id 或 clipboard_history.id
  is_pinned     INTEGER NOT NULL DEFAULT 0,
  is_favorite   INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT    NOT NULL,
  updated_at    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_notes_source  ON notes(source);
```

**字段说明**：
- `content_html`：TipTap `getHTML()` 产物。图片节点为 `<img src="note-img:<hash>" alt="...">`（见 §6.2），引用 `image_data.hash`，不存临时 blob URL。
- `content_text`：由后端 `serialize::extract_text(html)` 抽取（scraper 去 tag），**前端 `update_note` 只传 `content_html`**，后端生成 text——后端为 source of truth，避免前端漏传/篡改导致 FTS 失真。
- `source_ref_id`：溯源外键。`PRAGMA foreign_keys` 关闭下不做 DB 级约束（与 clipboard 一致），引用有效性由应用层查询判断（原记录删除则徽标灰显）。

### 2.2 `notes_fts`（FTS5，trigram，仿 `clipboard_history_fts`）

```sql
CREATE VIRTUAL TABLE notes_fts USING fts5(
  title, content_text,
  content='notes', content_rowid='id', tokenize='trigram'
);
-- 3 触发器（AFTER INSERT / DELETE / UPDATE OF title,content_text）自动同步
```

迁移幂等策略：v8→v9 时 `notes_fts` drop+create 使旧库生效（仿 v7→v8 的 fts 重建手法）。

### 2.3 搜索规则（store.rs）

- 搜索词 ≥3 字符：包成 phrase 走 trigram MATCH（`notes_fts MATCH '"query"'`）。
- <3 字符：`content_text`/`title` LIKE 子串 fallback。
- 排序：`is_pinned DESC, updated_at DESC, id DESC`（置顶优先 + 二级排序消除同秒抖动，仿 clipboard）。
- 分页：`limit/offset`，前端手动「加载更多」（仿 Settings 管理页）。

## 3. `octopus-notepad` crate 接口

### 3.1 model.rs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NoteSource { Asr, Ocr, Clipboard, Manual }

impl NoteSource {
    pub fn as_str(&self) -> &'static str { /* "asr"/"ocr"/"clipboard"/"manual" */ }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NoteFilter {
    pub source: Option<NoteSource>,
    pub favorite: bool,
    pub pinned: bool,
    pub search: Option<String>,  // None 或 <3 字符 → LIKE；≥3 → FTS MATCH
    pub limit: i64,
    pub offset: i64,
}
```

### 3.2 store.rs

```rust
pub fn list_notes(filter: &NoteFilter) -> Result<Vec<Note>>;
pub fn count_notes(filter: &NoteFilter) -> Result<i64>;
pub fn get_note(id: i64) -> Result<Option<Note>>;

/// 新建。initial_html 由调用方提供（识别结果转 <p>.../；手建为空）。
/// content_text 由内部 serialize 抽取。created_at/updated_at = now。
pub fn create_note(source: NoteSource, source_ref_id: Option<i64>, initial_html: &str) -> Result<i64>;

/// 更新正文/标题。content_text 由 content_html 重新抽取；updated_at = now。
/// title 为空串则存 NULL（列表显示用 content_text 截取）。
pub fn update_note(id: i64, title: &str, content_html: &str) -> Result<()>;

pub fn delete_notes(ids: &[i64]) -> Result<usize>;
pub fn toggle_pinned(id: i64) -> Result<()>;
pub fn toggle_favorite(id: i64) -> Result<()>;
```

全部经 `infra::with_db(|conn| ...)`，错误用 `anyhow::Result`。`update_note`/`toggle_*`/`delete_notes` 在 desktop command 层成功后 `emit("notepad://changed")`（store 层不 emit，保持纯逻辑可单测）。

### 3.3 serialize.rs

```rust
/// content_html → 纯文本：scraper 解析，按块拼接（<p>/<h*>/<li> 间加换行），<img> 转 "[图片]"。
pub fn extract_text(html: &str) -> String;
```

### 3.4 export.rs

```rust
pub const NOTES_DIR: &str = "octopus/notes";  // 相对 Documents，跨平台用 dirs::document_dir()

/// 导出：把前端序列化好的字符串写到 ~/Documents/octopus/notes/<safe_title>.<ext>。返回绝对路径。
pub fn write_export(filename_stem: &str, ext: &str, content: &str) -> Result<PathBuf>;

/// 导入：读 .md 文件原文返回（md→HTML 的解析在前端 TipTap，后端只做 I/O）。
pub fn read_import(path: &Path) -> Result<String>;
```

格式转换（HTML↔md↔txt）放前端 TipTap（最准），后端只落盘/读文件。

## 4. desktop Tauri commands（`note_commands.rs`）

薄封装，转调 `octopus-notepad`，成功写操作 `emit("notepad://changed")`：

```rust
#[tauri::command] fn list_notes(filter: NoteFilter) -> Result<Vec<Note>, String>;
#[tauri::command] fn count_notes(filter: NoteFilter) -> Result<i64, String>;
#[tauri::command] fn get_note(id: i64) -> Result<Option<Note>, String>;
#[tauri::command] fn create_note(source, source_ref_id, initial_html) -> Result<i64, String>;
#[tauri::command] fn update_note(id: i64, title: String, content_html: String) -> Result<(), String>;  // 自动保存；title 空串=无标题
#[tauri::command] fn delete_notes(ids: Vec<i64>) -> Result<usize, String>;
#[tauri::command] fn toggle_pinned(id) -> Result<(), String>;
#[tauri::command] fn toggle_favorite(id) -> Result<(), String>;
#[tauri::command] fn export_note(stem, ext, content) -> Result<String, String>;   // 返回路径
#[tauri::command] fn import_note_from_file(path) -> Result<String, String>;        // 返回 md 原文
```

### 4.1 集成入口 command（识别结果 → 笔记）

```rust
/// 语音结果 → 新建笔记：取转写文本 → <p> 包裹 → create_note(Asr, Some(transcription_id))
#[tauri::command] fn save_transcription_to_note(transcription_id: i64) -> Result<i64, String>;

/// OCR 结果 → 新建笔记：text → <p> 包裹 → create_note(Ocr, None)
#[tauri::command] fn save_ocr_to_note(text: String) -> Result<i64, String>;
```

`save_transcription_to_note` 内部查 `transcriptions` 拿内容；查不到对应记录时返回错误（不静默建空笔记）。成功后 `emit("notepad://changed")`。（`save_clipboard_to_note` 已于 2026-07-01 移除——剪贴板条目不再存入记事本。）

### 4.2 溯源回溯

`get_note` 已返回 `source` + `source_ref_id`。前端「查看来源」按钮：
- `asr` → 复用 `open_settings(initial_page="history")` + 定位到 `transcription_id`（HistoryPanel 已有按 id 定位能力则复用，否则滚动高亮）
- `clipboard` → `open_settings(initial_page="clipboard")` + 定位 `clipboard_history.id`
- `ocr` / `manual` / `source_ref_id` 已失效 → 徽标灰显 + tooltip「原记录已删除」，不提供跳转

## 5. 窗口与入口

### 5.1 `notepad_window`

- Rust 动态创建（`WebviewWindowBuilder`，label=`notepad_window`），独立窗口、原生标题栏、可调大小、位置记忆（复用 settings_window 的窗口位置记忆机制）。
- `App.tsx` 加 `case "notepad_window": return <Notepad />`。
- 托盘菜单加「记事本」项 → `open_notepad()`（show + set_focus；已开则聚焦）。

### 5.2 全局快捷键

**默认不绑**（octopus 快捷键已拥挤，避免冲突）。设置页留一个可配置项（后续接入现有 shortcut 配置体系），MVP 可不做。

### 5.3 各识别结果「存入记事本」入口

lucide `NotebookPen` 图标按钮，点击调对应 §4.1 command：

| 位置 | 调用 |
|---|---|
| `Settings/HistoryPanel.tsx` 识别记录行操作 | `save_transcription_to_note(...)` |
| OCR 流程（OCR 后文本） | `save_ocr_to_note(text)` |

> **已移除入口**（2026-07-01）：`Result/index.tsx` 结果窗工具栏（长篇模式原地编辑替代）、`Clipboard/ClipboardItem.tsx` 剪贴板浮窗条目、`Settings/ClipboardPanel.tsx` 剪贴板管理页行操作的「存入记事本」按钮均已移除——后端 `save_clipboard_to_note` 命令 + `saveClipboardToNote` helper 一并删除。

存入成功 toast 提示「已存入记事本」（不强制弹出窗口，避免打断当前流程）。

## 6. 前端

### 6.1 TipTap 编辑器配置（`editor/extensions.ts`）

- `StarterKit`（段落 / H1-3 / 列表 / 引用 / 代码块 / 粗斜体 / 历史）
- `Link`
- 自定义 `Image` NodeView（见 §6.2）
- `tiptap-markdown`（md 序列化；实施时锁定兼容 React 19 的 TipTap v3 版本）

工具栏：粗 / 斜 / H1-3 / 无序列表 / 有序列表 / 引用 / 代码 / 分割线 / 图片 / 链接 / 撤销 / 重做 + 导入 / 导出按钮。

### 6.2 Image NodeView（关键）

内部 src 用稳定协议 `note-img:<hash>`，引用 `image_data.hash`：

- **插入图片**：调用 `insert_image(hash, alt)` → 编辑器 `image` 节点 `attrs = { src: "note-img:" + hash, alt }`。
- **渲染（NodeView 组件）**：解析 `src` 的 `note-img:` 前缀取 hash → `invoke('get_image_blob', hash)` 拿 WebP bytes → `URL.createObjectURL` → 渲染 `<img src={blobUrl}>`。blob URL 在组件卸载时 `revokeObjectURL`。
- **序列化（getHTML）**：TipTap 输出 `<img src="note-img:<hash>" alt="...">`，src 始终是稳定协议，**不存临时 blob URL**——笔记内容可持久化、跨会话还原。
- **取图（`get_note_image(hash)` command）**：新增于 `note_commands.rs`，调 `octopus_clipboard::store::get_image_blob(conn, hash)` 取原图 WebP → 编码为 `data:image/webp;base64,...` 返回（仿现有 `get_image_thumb` 的 data URL 手法，避免 IPC 字节数组 4-5x 膨胀）。**notepad crate 不依赖 clipboard**——图片 BLOB 获取由 desktop command 层桥接。
- **插入图片**：文件选择 → 读图 → `clipboard::store::insert_image_data` 入库（复用 `image_data` + SHA-256 去重）得 hash → 编辑器插入 `note-img:<hash>`。

### 6.3 NoteEditor.tsx

- `useEditor`（TipTap），`content` = 当前 note 的 `content_html`。
- `onUpdate` debounce 800ms → `update_note(id, title, getHTML())`（自动保存，防丢失）。
- 标题输入框；空标题传空串 `update_note(id, "", ...)`，列表显示用正文截取。
- 导出：编辑器序列化（md 用 `editor.storage.markdown.getMarkdown()` / txt 用 `getText()` / html 用 `getHTML()`）→ `export_note(stem, ext, content)` → toast 路径。**HTML 导出**前遍历 `<img src="note-img:...">` 调 `get_note_image(hash)` 替换为 data URL，使导出 HTML 自包含可在外部打开；md/txt 导出图片以 "[图片]" 占位。
- 导入：文件选择 → `import_note_from_file(path)` 取 md 原文 → 解析为 TipTap JSON（tiptap-markdown）→ `setContent` + `update_note`。

### 6.4 NoteList.tsx

- 搜索框 + 来源筛选 tab（全部 / 语音 / OCR / 剪贴板 / 收藏）→ 改 `NoteFilter` → `list_notes`。
- 笔记项：标题（或正文截取）/ 预览（content_text 前 N 字）/ 相对时间 / 来源徽标（可点回溯）/ 置顶图钉 / 收藏星。
- 手动「加载更多」分页（`offset += limit`）。
- 监听 `notepad://changed` 事件 → 自动 `list_notes` 刷新（仿 `useClipboardHistory`）。

### 6.5 lib / hooks

- `lib/notepad.ts`：所有 invoke 封装（仿 `lib/tauri.ts`）。
- `hooks/useNotes.ts`：列表 + filter + 分页状态 + `notepad://changed` 监听。

## 7. 数据流

```
存入：
  识别结果[存入按钮] → save_*_to_note(ref_id)
    → 查 transcriptions/clipboard_history 取内容 → 转 HTML
    → notepad::create_note（写 notes + content_text 抽取 + fts 触发器同步）
    → emit("notepad://changed") → 记事本窗口(若开) 列表刷新
    → toast「已存入记事本」

编辑：
  TipTap onUpdate(debounced 800ms) → update_note(id, title, getHTML())
    → notepad::update_note（重抽 content_text + 更新 fts + updated_at）
    → emit("notepad://changed")（列表预览/时间刷新；当前编辑项不重渲染避免光标跳动）

导入/导出：
  导出：TipTap 序列化(md/txt/html) → export_note 写 ~/Documents/octopus/notes/ → toast 路径
  导入：选 .md → import_note_from_file 读原文 → tiptap-markdown 解析 → setContent → update_note

溯源：
  徽标点击 → open_settings(history|clipboard) + 定位 ref_id；ref 失效 → 灰显
```

## 8. 错误处理

| 场景 | 处理 |
|---|---|
| DB CRUD 失败 | command 返回 `Err(String)`，前端 toast |
| `save_*_to_note` 查不到原记录 | 返回错误（不静默建空笔记），toast「原记录不存在」 |
| 溯源 `source_ref_id` 对应记录已删 | 徽标灰显 + tooltip「原记录已删除」，不跳转 |
| 图片 `get_image_blob` 失败（hash 失效） | NodeView 显示占位图 + alt |
| 自动保存冲突 | MVP 单窗口编辑同一 note 无并发；多窗口同时开同一 note 不在 MVP 范围 |
| 导出目录不可写 | 返回错误，toast 提示路径 |
| TipTap 内容过大 | SQLite text 无硬限；前端编辑器可加软限提示（可选） |

## 9. 测试

**后端 infra/db**（单测）：
- notes CRUD 正确性
- `content_text` 由 `content_html` 抽取正确（含 `<img>` → "[图片]"）
- FTS：≥3 字符 MATCH / <3 字符 LIKE fallback
- 触发器同步（insert/update/delete 后 fts 一致）
- v9 迁移幂等（drop+create fts 后旧库可搜）
- 排序：置顶优先 + updated_at + id 二级

**后端 notepad crate**（单测）：
- store：list/count filter 各分支、toggle、delete 批量
- serialize：各种 HTML 结构抽取
- export：文件读写 + 路径安全（stem 含特殊字符转义）

**后端 desktop note_commands**（集成）：
- `save_transcription_to_note` / `save_ocr_to_note` → 笔记存在 + `source`/`source_ref_id`/`content_html` 正确；原记录不存在时报错

**前端**（组件 + 序列化）：
- TipTap 渲染 content_html + 反序列化一致
- HTML ↔ md ↔ txt 三向序列化（导出文本正确）
- Image NodeView：`note-img:<hash>` → blob 渲染；getHTML 仍为稳定协议
- 自动保存 debounce 触发 update_note

**e2e**：
- 语音识别 → 结果窗「存入」→ 记事本列表可见 + 来源徽标=语音 + 点击溯源跳到识别记录
- 剪贴板图片条目「存入」→ 笔记含 `<img src="note-img:...">` → 编辑器渲染图片
- 编辑笔记 → 搜索命中 → 导出 `.md` 文本正确

## 10. 范围与非目标（YAGNI）

- ❌ 标签 / 文件夹 / 多级目录（MVP 扁平 + 来源筛选 + 收藏/置顶 + 搜索）
- ❌ 多端同步 / cli / server 接入（crate 已留口，MVP 仅 desktop）
- ❌ 全局快捷键默认绑定（设置项留位，MVP 可不做）
- ❌ 笔记内嵌非图片附件（仅图片，复用 image_data）
- ❌ md/txt 导出保留图片（MVP 以 "[图片]" 占位；仅 HTML 导出把图片 inline 成 data URL 自包含）
- ❌ 协同编辑 / 多窗口同 note 并发（单窗口编辑）


---

## 来自原文件 `2026-06-30-scroll-stitch-research.md`

# 滚动截屏拼接技术调研方案

**日期**: 2026-06-30
**状态**: ✅ 调研完成，实施已完成（最终采用 Canvas-Anchored NCC + Sobel，非原推荐的 FFT）
**分支**: `feature/clipboard-research`

---

## 一、问题诊断：当前实现为何重叠与丢帧

### 1.1 当前算法回顾

当前 `stitch.rs` 采用**底部 strip NCC（归一化互相关）模板匹配**：

1. 从上一帧 edges 底部取一个 strip（≈20% 选区高度）作为模板
2. 在当前帧中滑动搜索最佳 NCC 匹配位置
3. 从匹配位置之后裁剪新内容追加到画布

### 1.2 三个结构性缺陷

| 缺陷 | 根因 | 表现 |
|---|---|---|
| **亚像素精度缺失** | NCC 按整数像素滑窗，真实滚动位移往往是 12.7px、38.3px 等非整数 | 每帧 0.3-0.7px 累积误差 → 文字行逐渐模糊 |
| **模板条纹太窄** | `template_ratio=0.20` → 模板仅 ~100px 高。在周期性列表（如文件列表每行 45px）中，100px 模板在 d、d±45 处得分接近 | 匹配跳到隔壁行 → 行重复/错位 |
| **帧间比较而非帧-画布比较** | 当前是 `last_frame` vs `curr_frame`。如果某帧被丢弃（静止/低置信度），下一帧的 `last_frame` 是旧的 → 位移突变 | 丢帧后紧跟一帧大位移拼接 → 内容缺失或重叠 |

### 1.3 日志数据佐证

```
delta=424 tpl_h=114 new_h=36   ← 匹配太靠下，只追加 36px
delta=412 tpl_h=114 new_h=48   ← 下一帧位移突然变了 12px
delta=265 tpl_h=114 new_h=191  ← 突然跳到完全不同的位置
```

delta 在 265-455 之间剧烈跳动，说明模板在周期性内容上锁不稳定。

---

## 二、业界成熟方案调研

### 2.1 ScrollSnap（macOS 开源，Swift）

**来源**: [Brkgng/ScrollSnap](https://github.com/Brkgng/ScrollSnap) — macOS 上最接近的开源滚动截屏

**核心技术**: `VNTranslationalImageRegistrationRequest`（Apple Vision Framework）

```swift
let request = VNTranslationalImageRegistrationRequest(targetedCGImage: previousCG)
let handler = VNImageRequestHandler(cgImage: currentCG)
handler.perform([request])
// observation.alignmentTransform.ty → 亚像素级垂直位移
```

**关键设计**：
- 帧间比较（`current` vs `previous`），不是帧-画布比较
- Vision 框架内部用 **FFT 相位相关 + 亚像素插值**，精度达 0.01px
- 向下滚动 → `offset > 0` → 从新帧底部追加 `offset` 高度的内容
- 向上滚动 → `offset < 0` → 从画布底部裁剪 `|offset|` 高度
- offset == 0 → 静止，跳过

**优势**: 利用 Apple 硬件加速（Metal/GPU），无需手写匹配算法，亚像素精度。

### 2.2 ShareX（Windows 开源，C#）

**核心技术**: 纯像素行差异比较

**流程**:
1. 后端模拟滚轮 `SendInput(WM_MOUSEWHEEL)`
2. 等待 `scroll_delay`（默认 100ms）让应用渲染
3. 截图 → 与上一张比较
4. 从上一张底部逐行往下找最大相似度位置 → 确定重叠区域
5. 裁掉重叠部分，追加新内容

**关键参数**:
- `scroll_delay`: 等待渲染完成（太短→截到半渲染画面/撕裂）
- 重叠区域固定比例（~30%）

**优势**: 简单可靠，不依赖频率域计算。**劣势**: 整数像素精度，无亚像素。

### 2.3 Picsew / nocoo/image-stitch（离线拼接类）

**核心技术**: **ORB 特征点匹配**（Oriented FAST and Rotated BRIEF）

**流程**:
1. 对两张截图提取 ORB 特征点
2. BFMatcher 或 FLANN 匹配特征点对
3. RANSAC 过滤误匹配
4. 估计刚性变换矩阵（纯垂直平移）
5. 按平移量裁剪拼接

**优势**: 对亮度变化、轻微缩放鲁棒。**劣势**: 计算量大，实时性差（适合离线后处理）。

### 2.4 相位相关（Phase Correlation，频率域方法）

**数学原理**:

两张只差平移 `(dx, dy)` 的图像，其互相关功率谱的相位等于线性相位：

```
R = F₁ · F₂* / |F₁ · F₂*|
IFFT(R) → 在 (dx, dy) 处有一个 δ 峰值
```

**优势**:
- **亚像素精度**：对峰值做抛物线拟合，精度 0.01-0.1px
- **全局最优**：不依赖模板位置，不会卡在周期性假峰
- **O(N log N)**：FFT 比逐行 NCC 滑窗快
- 对周期性内容鲁棒（频率域中周期性体现为多个峰，但主峰仍是真实位移）

**Rust 生态**: `rustfft` crate 成熟稳定。

---

## 三、推荐方案

### 方案 A：FFT 相位相关（推荐）

> **更新（2026-06-12）**：本方案为调研结论，**实际未采纳**。最终实现采用 2D SAD 空间模板匹配 + 软速度罚分（见 commit `4b94215`），在实测中已能精准工作。后续性能优化（整数化 + 模板预取 + 画布增量追加）见 [`2026-06-12-capx-optimization-design.md`](./2026-06-12-capx-optimization-design.md)。

**适用场景**: 我们是纯垂直 1D 平移，FFT 相位相关是最优数学工具。

**算法**:

```rust
// 1. 取两帧的灰度图（或 Sobel 边缘图）
let gray_a = grayscale(&frame_a);
let gray_b = grayscale(&frame_b);

// 2. 2D FFT（只算垂直方向即可，可降为 1D）
let fft_a = fft2d(&gray_a);
let fft_b = fft2d(&gray_b);

// 3. 互相关功率谱的归一化相位
let cross_power = conj(&fft_b) * &fft_a;
let normalized = cross_power / cross_power.norm();

// 4. IFFT → 峰值位置即位移
let correlation = ifft2d(&normalized);
let (dy, _) = find_peak(&correlation);  // 亚像素：抛物线拟合

// 5. dy > 0 → 向下滚了 dy 像素
//    从 frame_b 的 dy 位置开始到底部，追加到画布
```

**亚像素精化**:

```rust
// 在整数峰值 (px, py) 附近做抛物线拟合
let left  = corr[px - 1];
let peak  = corr[px];
let right = corr[px + 1];
let subpixel = px as f32 + 0.5 * (left - right) / (left - 2.0 * peak + right);
```

**性能估算**:
- 选区 ~2000×500px，1D FFT（沿垂直方向）: O(W × H log H) ≈ 2000 × 500 × 9 ≈ 9M ops
- RustFFT 在 release 模式下约 **2-5ms/帧**
- 对比当前 NCC：100 次滑窗 × 每次 ~200×100 像素块 ≈ 2M ops，但无亚像素

**实现路径**:
1. `Cargo.toml` 加 `rustfft = "3.0"` + `realfft = "3.0"`（实数 FFT 封装）
2. 在 `stitch.rs` 中用 `fft_phase_correlation(frame_a, frame_b) -> f32` 替换 `match_template`
3. 返回 `dy: f32`（亚像素），裁剪时 `round(dy)` 取整，但用 `dy` 的累积值跟踪偏移

### 方案 B：macOS Vision Framework（macOS 专属）

通过 `objc2` 调用 `VNTranslationalImageRegistrationRequest`：

```rust
// 伪代码
let request = VNTranslationalImageRegistrationRequest::new(&prev_cgimage);
let handler = VNImageRequestHandler::new(&curr_cgimage);
handler.perform(&[request]);
let observation = request.results()[0];
let dy = observation.alignment_transform.ty;  // 亚像素
```

**优势**: 零算法实现，Apple GPU 加速。
**劣势**: macOS 专属，Windows/Linux 需另写。但滚动截屏本身各平台的焦点/截屏机制已经平台分支了，拼接算法分支也可接受。

### 方案对比

| 维度 | 当前 NCC | FFT 相位相关 | Vision Framework |
|---|---|---|---|
| 亚像素精度 | 无（整数 px） | 0.01-0.1px | 0.01px |
| 周期性鲁棒 | 差（假匹配） | 好（频率域全局峰） | 好 |
| 计算速度 | ~15ms | ~3-5ms | ~1ms（GPU） |
| 跨平台 | ✅ | ✅ | macOS only |
| 实现复杂度 | 已有 | 中（FFT + 峰值拟合） | 低（调 API） |
| 丢帧处理 | 帧间比较→突变 | 帧间比较→突变 | 帧间比较→突变 |

---

## 四、丢帧问题的独立解决方案

无论用哪种匹配算法，**帧间比较** 都有一个致命问题：如果帧 N 被丢弃（静止/低置信度），帧 N+1 的 `previous` 还是帧 N-1 → 位移突变。

### 4.1 方案：画布底部比较（Canvas-Anchored Matching）

**核心改变**：不要比较 `frame[n]` vs `frame[n-1]`，而是比较 `frame[n]` vs **canvas 的底部 strip**。

```
Canvas:     [................... strip_bottom]
Frame N:    [strip_bottom候选位置 ... 帧底部]

匹配 strip_bottom 在 frame N 中的位置 → 确定新内容
```

**优势**：
- 无论中间多少帧被丢弃，canvas 底部始终是"已确认的最新内容"
- 不会因为丢帧产生位移突变

**实现**：
```rust
// 每次匹配时：
let canvas_bottom = crop_bottom_strip(&self.canvas, tpl_h);
let dy = phase_correlation(&canvas_bottom, &frame);
// 或 NCC: 在 frame 中搜索 canvas_bottom 的位置
```

**注意**：canvas 底部 strip 是 RGBA，frame 也是 RGBA，无需边缘转换。但如果页面底部有动态内容（如加载动画），strip 内容会变 → 需要更新 strip。

### 4.2 方案：动态 strip 更新

匹配成功后，用 **frame 底部的 `tpl_h` 行替换 canvas 底部 strip**（而非追加后的 canvas 底部）：

```rust
// 匹配成功后
let new_strip = crop_bottom(&frame, tpl_h);
self.reference_strip = new_strip;  // 下一帧用这个做比较
```

这样每帧的参考 strip 始终是"上一帧实际看到的内容底部"，即使某些帧被跳过也不影响。

---

## 五、综合推荐实施路径

### 第一优先级：Canvas-Anchored + FFT 相位相关

```
Phase 1: Canvas-Anchored 匹配（解决丢帧）
  ├─ 从 canvas 底部裁 strip 作为参考
  ├─ 每帧与 canvas strip 比较（不再帧间比较）
  └─ 匹配成功后更新 reference strip

Phase 2: FFT 相位相关替换 NCC（解决精度+周期性）
  ├─ 加 rustfft / realfft 依赖
  ├─ 实现 fft_phase_correlation(a, b) -> f32（亚像素 dy）
  └─ 裁剪用 round(dy)，累积偏移用 dy 浮点

Phase 3: 混合策略（鲁棒性兜底）
  ├─ FFT 主匹配
  ├─ 如果 FFT 峰值不明显（score < 阈值）→ fallback NCC 全局搜索
  └─ 如果 NCC 也不够好 → 跳过帧（等用户继续滚动）
```

### 可选：macOS 走 Vision Framework 捷径

如果只考虑 macOS，直接用 `VNTranslationalImageRegistrationRequest`：
- Phase 1（Canvas-Anchored）仍然需要
- Phase 2 用 Vision API 替代 FFT（更简单 + GPU 加速）
- Windows/Linux 后续再补 FFT 实现

### 预期效果

| 问题 | 当前 | 改进后 |
|---|---|---|
| 重叠 | NCC 整数误差累积 | FFT 亚像素 → 误差 < 0.1px |
| 丢帧 | 帧间比较→位移突变 | Canvas-Anchored → 无突变 |
| 周期性假匹配 | 模板窄→假峰 | FFT 全局主峰→鲁棒 |
| 模糊 | 亚像素误差→半像素错位 | 亚像素精度→清晰 |

---

## 六、参考资料

- [ScrollSnap 源码](https://github.com/Brkgng/ScrollSnap) — macOS Vision Framework 滚动截屏
- [Phase Correlation - Wikipedia](https://en.wikipedia.org/wiki/Phase_correlation) — FFT 位移估计原理
- [VNTranslationalImageRegistrationRequest](https://developer.apple.com/documentation/vision/vntranslationalimageregistrationrequest) — Apple Vision 图像配准
- [nocoo/image-stitch](https://github.com/nocoo/image-stitch) — ORB 特征匹配拼接
- [ShareX 滚动截屏文档](https://getsharex.com/docs/scrolling-screenshot) — 像素行差异比较
- [Subpixel Phase Correlation Methods](https://apps.dtic.mil/sti/tr/pdf/ADA519383.pdf) — 亚像素精度方法对比


---

## 来自原文件 `2026-07-01-image-preview-design.md`

# 图片预览 / 标注窗（Image Preview）设计

> 日期：2026-07-01
> 状态：✅ 实施完成（图片预览窗 + 标注工具栏 + OCR + 复制/保存）
> 关联：`docs/superpowers/specs/2026-06-30-compact-editor-design.md`（窗口/命令/PENDING 模式模板）、`docs/superpowers/specs/2026-06-30-notepad-design.md`。
> 分支：`worktree-feature-notepad`。**功能完整完成前不往 main 同步。**

## 1. 背景与目标

剪贴板文本条目已能用精简编辑器（compact editor）打开编辑。图片条目目前只能看 8×8 缩略图 + 尺寸，**无法看原图、无法在图上做标注**。本期补一个「图片预览 / 标注窗」：

- 从剪贴板图片条目**单击缩略图**唤起，打开原图。
- **浮动白卡工具栏**（对齐截图主工具栏的漂浮白卡形态）：标注工具（选择/矩形/椭圆/直线/箭头/画笔/文字）+ 颜色·粗细属性浮窗 + 保存 + 复制 + OCR + 缩放 + 置顶（关窗走原生 × / Esc，工具栏不放关闭按钮）。
- 标注能力**复用截图工具栏已有的标注引擎**（抽取成共享模块，截图与预览共用）。

用户视野里有**两种图片展示形态**，本需求只做第一种，但为第二种留好基础：

| 形态 | 触发 | 长相 | 本需求 |
|---|---|---|---|
| **① 轻工具栏预览** | 剪贴板条目缩略图单击 | 原生窗口 + 灯箱画布 + 浮动白卡工具栏，可标注 | ✅ 做 |
| **② 贴图模式** | 按需（主要给**截图钉住**用） | 无工具栏、就一张图、钉屏置顶（Snipaste 风格，hover 浮出关闭/图钉按钮） | ❌ 不做，留基础 |

## 2. 范围

**做：**
- 新建独立窗口 `image_preview_window`（原生标题栏、可调大小、单例销毁、macOS 激活策略切换）。
- 后端命令：`open_image_preview` / `get_pending_image` / `close_image_preview` / `get_image_full`。
- **抽取共享标注核心** `frontend/src/lib/annotation.ts`：类型（`Tool`/`Annotation`）+ 纯绘制/命中函数。`Screenshot/index.tsx` 改为 import（行为零变化），`ImagePreview` 复用。
- `ImagePreview` 组件：全图画布（fit-to-window、无选区），点击拖拽画标注、文字点选输入、撤销、选中移动。
- 轻工具栏组件（标注工具 + 属性浮窗 + 保存/复制/OCR/置顶）。
- 入口：剪贴板图片条目缩略图单击 → 唤起预览。

**不做（YAGNI / 留给未来）：**
- **贴图模式（形态②）**：无边框置顶、hover 工具栏、多实例——本需求不建窗口、不写交互，仅在 §9 文档化基础。
- 标注**持久化到剪贴板条目**：本次标注是「按需预览」的临时操作，关窗即失；保存走「导出带标注的新图到文件」。未来可再持久化。
- ~~缩放控件（滚轮/按钮放大）~~：**已做**（见 §3.4）——默认 1:1 自然分辨率打开，工具栏放大/缩小按钮调 `zoom`（0.1×–8×），超出窗口自动滚动条 + 选择工具下抓手拖拽平移。标注用自然像素坐标空间，缩放/调窗不错位。
- ~~箭头/画笔/序号工具~~：**箭头/画笔已加**（用户后续要求补齐截图已有的 arrow/pen，共享核心本就支持）；序号（number）仍不暴露（预览场景少用）。

## 3. 架构

### 3.1 窗口与生命周期（`crates/desktop/src/image_preview_window.rs`）

镜像 `compact_editor_window.rs`：

- `WINDOW_LABEL = "image_preview_window"`。
- `create_image_preview_window(app)`：`.title("图片预览")`、`.inner_size(880, 620)`、`.min_inner_size(400, 320)`、`.decorations(true)`、`.resizable(true)`、`.center()`、`.visible(true)`。
- 单例：`get_webview_window` 命中已存在则 `show + set_focus` 并 emit load 推新 imageId（并发再开）；否则创建。
- macOS 激活策略：开窗切 `Regular`（Dock 显图标），关窗切回 `Accessory`。新增 `on_image_preview_closed(app)`，在 `main.rs` 的 `RunEvent::WindowEvent { Destroyed }` 按 label 挂载（紧邻 `on_compact_editor_closed`）。
- 生命周期：**关窗即销毁**（destroy-on-close）。

> **ACL（必做，踩过的坑）**：动态窗口 label 必须加进 `capabilities/default.json` 的 `windows` 数组，否则该窗口前端 `emit`/`invoke`/`listen` 全被静默拦。当前数组补 `image_preview_window`：
> `["main","result_window","settings_window","clipboard_window","notepad_window","compact_editor_window","image_preview_window","screenshot_*"]`。

### 3.2 后端命令（`crates/desktop/src/image_preview_commands.rs`，薄层）

镜像 `compact_editor_commands.rs` 的「写 PENDING → 建窗/聚焦 → 前端 mount 拉取」：

```rust
// 静态 PENDING：open 时写，前端 mount 时 take。
static PENDING: Mutex<Option<PendingImage>> = Mutex::new(None);

// mode 字段为贴图模式（形态②）预留：本需求仅 "preview"，pin 分支将来再加。
// 现在带上 mode 是「打好基础」的显式标记——窗口创建暂只走 preview 分支。
struct PendingImage { image_id: i64, mode: String }  // mode: "preview"（now）/ "pin"（future）

#[tauri::command]
pub fn open_image_preview(image_id: i64, mode: Option<String>, app_handle: AppHandle);
//   → *PENDING = Some({ image_id, mode: mode.unwrap_or("preview".into()) })
//   → 窗口已存在：emit("image-preview://load", { imageId, mode }) + show + focus
//   → 否则：建窗

#[tauri::command]
pub fn get_pending_image() -> Option<PendingImage>;  // 前端 mount 时 take

#[tauri::command]
pub fn close_image_preview(app_handle: AppHandle);   // close() → Destroyed → macOS 切 Accessory
```

三个命令在 `main.rs` 的 `generate_handler!` 注册（紧邻 compact_editor 三命令）。

> `PendingImage` 经 IPC 序列化为 camelCase：`{ imageId, mode }`。

**取原图命令（`crates/desktop/src/clipboard_commands.rs`，紧邻 `get_image_thumb`）：**

```rust
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<String, String> {
    // 镜像 get_image_thumb，但读 blob（原图）而非 thumb，复用 store::get_image_blob
    // 返回 "data:image/webp;base64,..."
}
```

store 层 `get_image_blob(conn, hash)` 已存在（`crates/clipboard/src/store.rs:467`），无需新增 store 函数。

### 3.3 共享标注核心抽取（`frontend/src/lib/annotation.ts`）= 「打好基础」

把 `Screenshot/index.tsx` 里**纯函数 / 纯类型**抽到 `lib/annotation.ts`，截图改为 import（行为零变化）：

```ts
// lib/annotation.ts
export type Tool = "none" | "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
export interface Annotation {
  type: "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
  x1: number; y1: number; x2: number; y2: number;
  text?: string; points?: number[][]; color?: string;
  lineWidth?: number; fontSize?: number; number?: number; circleSize?: number;
}

// 坐标系约定（重要）：Annotation 的坐标统一用「图片原始像素空间」（natural px）。
// - 显示时：scale = 显示宽 / naturalWidth，调 drawAnnotationScaled(ctx, ann, scale)。
// - 导出时：在 natural 尺寸 canvas 上 scale=1 直接画 drawAnnotation(ctx, ann)。

export function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Annotation): void;
export function drawAnnotationScaled(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number): void;
export function drawMultilineText(ctx, text, x, y, maxWidth, fontSize): void;
export function annBounds(ann: Annotation): { x: number; y: number; w: number; h: number };
export function hitTestAnnotationPrecise(anns: Annotation[], mx: number, my: number): number | null;
export function pointToSegmentDist(px, py, x1, y1, x2, y2): number;
```

> **截图坐标系的注意点**：截图现有代码里 `Annotation` 坐标是「窗口显示空间」（`window.innerWidth` 系），导出时 `scale = bg.naturalWidth / window.innerWidth` 放大。抽取时**不改截图的行为**——截图继续用自己的显示空间坐标。`ImagePreview` 则采用**原始像素空间**坐标（见 §3.4）。两套坐标都用同一组纯函数（`drawAnnotationScaled` 的 `scale` 参数天然适配两套），函数本身不绑定任何坐标约定，只是「按 ann 里存的坐标 + 给定 scale 画」。抽取只搬函数体，不语义改动，截图回归风险低。

**截图侧改动（最小）**：`Screenshot/index.tsx` 删除这 6 个函数/2 个类型的本地定义，改为 `import { Tool, Annotation, drawAnnotation, drawAnnotationScaled, drawMultilineText, annBounds, hitTestAnnotationPrecise, pointToSegmentDist } from "@/lib/annotation"`。`hitTestAnnotationPrecise` 抽取时把内部对 `annotations` 闭包的依赖改成接收 `anns: Annotation[]` 参数（截图调用处传 `annotations`）。

### 3.4 ImagePreview 组件（`frontend/src/pages/ImagePreview/index.tsx`）

**布局**（灯箱形态——深暗场让图片本身发光，工具卡与状态条均浮动其上）：

```
    ┌─ 浮动白卡工具栏（fixed 居中贴顶，见 §3.5）──┐    ← position:fixed, top:8, translateX(-50%)
    └──────────────────────────────────────────┘
┌──────────────────────────────────────────────────────┐
│  灯箱暗场 #1c1917（absolute inset-0 overflow-auto）  │
│      ┌────────────────────────────────┐              │
│      │ <canvas> 棋盘格底（透明区可见）  │             │  ← dispW=nw*zoom, dispH=nh*zoom
│      │ <img> display:none（仅作解码源） │             │     居中，超出视口自动滚动条
│      │ <textarea> 文字草稿（相对 canvas）│            │
│      └────────────────────────────────┘              │
│                  ┌─────────────────┐                 │
│                  │ 1920 × 1080 · PNG│                │  ← 底部 EXIF 状态条（fixed 居中贴底）
│                  └─────────────────┘                 │     半透+blur，等宽 tabular-nums
└──────────────────────────────────────────────────────┘
```

- 外层 `relative h-screen overflow-hidden`，背景 `#1c1917`（灯箱暗场）。
- 滚动画布容器 `absolute inset-0 overflow-auto`（图片大于视口自动出上下/左右滚动条；小于则 `flex items-center justify-center p-12` 居中）。
- 画布外层 `relative` div，尺寸 = 显示尺寸（`dispW × dispH`）。`<canvas>` 像素尺寸 `dispW*dpr × dispH*dpr`，`ctx.setTransform(dpr,…)` 后 `ctx.scale(zoom, zoom)` 画标注；`<img>` `display:none` 仅作解码源（onload 取 naturalWidth/Height）。
- **棋盘格底**（canvas CSS 背景）：`#292524` + 20px 棋盘格纹 —— 透明 PNG 的透明区可见，专业看图工具信号；不透明区自然盖住。合成导出时另用自然尺寸画布（默认透明背景），PNG 保留透明。
- **显示尺寸 = 1:1 × zoom**：默认 `zoom=1`（自然分辨率），工具栏放大/缩小按钮调 `zoom`（0.1×–8×，步长 1.25×）。`dispW = nw*zoom, dispH = nh*zoom`。选 `tool==="none"` 时鼠标在画布上呈抓手，按住拖拽平移视口（window 级 mousemove/up，鼠标出画布仍跟随），免拖滚动条。

**坐标转换**（标注存自然像素坐标，与 zoom 解耦）：
- 鼠标事件 `e.clientX/Y` → 相对 canvas 的 CSS 坐标 `(cssX, cssY)` → 自然坐标 `nx = cssX / zoom`，`ny = cssY / zoom`。
- 显示用 `ctx.scale(zoom, zoom)` 后 `drawAnnotation(ctx, ann)`（自然坐标）；命中用自然坐标调 `hitTestAnnotationPrecise(nx, ny, annotations)`。

**交互（无选区，比截图简单）**：
- `tool === "none"`：点中标注 → 选中（可拖动移动，复用截图的拖动平移逻辑思路）；空白点击 → 取消选中。
- `tool ∈ {rect, oval, line}`：mousedown 记起点（原始坐标）→ mousemove 更新 `drawingRef` → mouseup 过滤太小的后入 `annotations`。
- `tool === "text"`：click → 在该点浮一个 `<textarea>`（样式同截图文字浮层：透明背景、虚线边、200 宽）；blur/Esc 确认或取消 → 入 `annotations`。
- 撤销：删最后一个标注（工具栏按钮 / Cmd+Z）。
- 选中删除：Delete/Backspace 删选中标注。
- 双击文字标注：进入编辑（复用截图 `editTextOrigRef` 思路）。

**导出（保存 / 复制用）**：

```ts
function composeAnnotated(): string | null {
  // 在 natural 尺寸 canvas 上 drawImage(bg) + 逐个 drawAnnotation(ann)（scale=1）→ toDataURL("image/png").split(",")[1]
}
```

- 保存：`composeAnnotated()` → base64 PNG → `invoke("save_image_dialog", { pngBase64 })`（新增薄命令，见 §3.6）。
- OCR：`invoke<string>("ocr_image", { id: imageId })`（整图识别，无裁剪）→ 文本非空则 `save_ocr_to_note(text)`（存为 `source=ocr` 笔记，返回 noteId）→ `open_notepad_with_note(noteId)` 打开记事本并选中该笔记，供用户在笔记里编辑；同时 `ocrCopied=true` 1.5s 反馈（工具栏 OCR 按钮换绿勾）。识别结果落进记事本（笔记系统），不再写系统剪贴板。

**mount / 事件**：
- mount：`get_pending_image()` → `{ imageId }` → `invoke<string>("get_image_full", { id: imageId })` → `new Image()` onload 后存 `bgImgRef`、计算显示尺寸、`setReady(true)`。
- `listen("image-preview://load")`：并发再开时载入新 imageId。
- Esc → 关窗（`invoke("close_image_preview")`，键盘快捷）。鼠标关窗走原生标题栏 ×（两者都触发 Destroyed → macOS 切回 Accessory）。

### 3.5 轻工具栏组件（`frontend/src/pages/ImagePreview/Toolbar.tsx`）

> 用 frontend-design skill 设计（2026-07-01 重做）：用户要求「浮窗的出现方式跟使用方式与截图的工具栏浮窗保持一致」，故工具栏复刻截图主工具栏的**浮动白卡**形态（非贴顶深色横条），属性浮窗 1:1 复刻截图 `ToolPropsPopover`。视觉 Vernacular 来自「看图工具/灯箱」：深暗场 + 棋盘格画布底 + 底部 EXIF 状态条，是与截图区分、且体现专业看图工具气质的 signature。

**风格基准**（对齐截图主工具栏 + `ToolPropsPopover`，内联 style 与截图同出处以便两处同步微调）：
- **工具卡**：`position:fixed; left:50%; top:8px; translateX(-50%)`，`padding:6px 8px`，`background:#fff`，`border-radius:8`，`box-shadow:0 4px 16px rgba(0,0,0,0.3)`（截图同款）。
- **按钮 `ToolButton`**：32×32，`border-radius:6`，激活 `background:#3b82f6; color:#fff`、否则透明 `color:#44403c` hover `rgba(0,0,0,0.06)`。图标 18px（`h-[18px] w-[18px]`）。与截图 `ToolButton` 完全一致。
- **分隔线**：`width:1; height:20; background:rgba(0,0,0,0.08); margin:0 4px`（截图 0.1，微调柔和）。
- **缩放百分比**：等宽 `SF Mono/Menlo` + `tabular-nums`，点击重置 100%。
- 图标统一 lucide-react（不走截图的本地 SVG，减少依赖）。

**布局（左→右，同一张白卡内分组）**：

| 组 | 按钮（lucide） | 行为 |
|---|---|---|
| 操作 | `Download` 保存 / `Copy` 复制 / `ScanText` OCR（成功后换 `Check` 绿勾 1.5s） | 保存→`composePngBase64`+`save_image_dialog`；复制→`composePngBase64`+`copy_image_to_clipboard`（写系统剪贴板 + 主动入库，见下）；OCR→`ocr_image`→`save_ocr_to_note`→`open_notepad_with_note`（存笔记并开记事本）+ `ocrCopied` 反馈 |
| ｜ | 分隔线 | |
| 标注工具 | `MousePointer2` 选择 / `Square` 矩形 / `Circle` 椭圆 / `Minus` 直线 / `ArrowUpRight` 箭头 / `Pen` 画笔（自由曲线） / `Type` 文字 | 单选互斥（再点已激活 = 回选择，浮窗收起）；激活 `#3b82f6` 蓝底白图标；选中任一标注工具即自动浮出属性浮窗（见下） |
| | `Undo2` 撤销 | 删最后标注；`canUndo=false` 时图标 opacity 0.3 |
| ｜ | 分隔线 | |
| 缩放 | `ZoomOut` 缩小 / 百分比（点击重置 100%）/ `ZoomIn` 放大 | `zoom` 0.1×–8×，步长 1.25×；百分比等宽 tabular-nums |
| ｜ | 分隔线 | |
| 置顶 | `Pin`/`PinOff` | toggle always-on-top；激活 `#3b82f6` |

> **不放「关闭」按钮**：窗口有原生标题栏（右上角 ×）已能关窗，工具栏再放一个冗余。关窗走原生 ×（鼠标）或 Esc（键盘）。

**属性浮窗**（1:1 复刻截图 `ToolPropsPopover`）：选中任一标注工具（`tool !== "none"`）时，从工具卡左下方 `absolute; left:0; top:calc(100%+6px)` 自动浮出 —— 白卡 `border-radius:10` + `box-shadow:0 8px 24px -4px…`，宽 240，两行：
- 行 1：label（粗细/字号，10px `#a8a29e`）+ `<input type="range">`（`accentColor` 跟当前色）+ 数值（等宽 tabular-nums）+ 当前色圆（20px，3px 白边 + 阴影，与下方预设色区分）。
- 分隔线 `rgba(0,0,0,0.06)`。
- 行 2：8 预设色（`["#ef4444","#f97316","#eab308","#22c55e","#3b82f6","#8b5cf6","#000000","#ffffff"]`），18×18 r5，全 opacity；active 用蓝 ring（`box-shadow:0 0 0 2px #fff, 0 0 0 3.5px #3b82f6`）—— 比截图（靠上方当前色圆反映）更清晰，白色加细边。文字工具显「字号 10–48」、其余显「粗细 1–10」。

> 不放截图原版的「调色板 `<input type="color">`」——8 预设色已覆盖常用，保持简单（YAGNI）。

**组件 API**：

```tsx
type PreviewTool = "none" | "rect" | "oval" | "line" | "text";

interface ToolbarProps {
  tool: PreviewTool; onTool: (t: PreviewTool) => void;
  color: string; onColor: (c: string) => void;
  size: number; onSize: (n: number) => void;   // 粗细 or 字号（按 tool 切含义/范围）
  onUndo: () => void; canUndo: boolean;
  onSave: () => void;
  onCopy: () => Promise<void>;   // 复制带标注图到系统剪贴板（composeAnnotated → copy_image_to_clipboard）
  onOcr: () => void; ocrLoading: boolean;
  pinned: boolean; onTogglePin: () => void;
}
```

### 3.6 新增薄命令：保存对话框 + 复制到剪贴板

`crates/desktop/src/clipboard_commands.rs` 新增两条薄命令：

```rust
#[tauri::command]
pub async fn save_image_dialog(png_base64: String, app_handle: AppHandle) -> Result<(), String> {
    // tauri_plugin_dialog::DialogExt save dialog → 写 PNG 字节。
    // 实现与 screenshot_commands::save_screenshot_dialog 等价（可抽公共 fn write_png_dialog(ah, base64)）。
}

#[tauri::command]
pub async fn copy_image_to_clipboard(
    png_base64: String,
    handle: State<'_, Arc<ClipboardHandle>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // base64 decode → handle.write_image（写系统剪贴板，置 suppress flag）。
    // 与条目行 copy_clipboard_item 区别：这里写的是 composePngBase64 合成的「带标注图」，非原图。
    // write_image 置 suppress 致 watcher 跳过自身写入（防回环），但预览的「复制」
    // 期望这条图进入剪贴板历史 → 主动调 octopus_clipboard::watcher::handle_clipboard_change
    // 入库（与系统复制同路径：去重 hash + WebP + 缩略图 + image_data BLOB），
    // 再 emit clipboard://changed 刷新浮窗/设置页。
}
```

两条都注册到 `generate_handler!`。

## 4. 数据流

```
ClipboardItem 缩略图 click
  │ invoke open_image_preview(imageId)          // mode 默认 "preview"
  ▼
image_preview_commands::open_image_preview
  │ PENDING = { imageId, mode:"preview" }
  │ 建窗(首次) 或 emit load + focus(已存在)
  ▼
ImagePreview mount ──get_pending_image──► PENDING.take()
  │ invoke get_image_full(imageId) → webp dataUrl → bgImg
  │ 计算显示尺寸 → canvas 就绪
  │ 工具栏选工具 → 画标注（存自然像素坐标）→ 撤销/选中移动
  │ 保存: composePngBase64() → save_image_dialog
  │ 复制: composePngBase64() → copy_image_to_clipboard（写系统剪贴板 + 主动入库历史）
  │ OCR:  ocr_image(imageId) → save_ocr_to_note(text) → open_notepad_with_note(noteId)
  │ 置顶: getCurrentWindow().setAlwaysOnTop(b)
  ▼
关闭 → close_image_preview → 销毁窗（macOS 切回 Accessory）
```

## 5. 入口（`ClipboardItem.tsx`）

剪贴板图片条目（`item.item_type === "image"`）的缩略图改为可点击：

- 当前缩略图 `<img>`（ClipboardItem.tsx:196）外包一层 `button`（或直接给 img 加 onClick），`onClick` → `e.stopPropagation()` + `invoke("open_image_preview", { imageId: item.id })`。
- 光标 `cursor-zoom-in`，`title="点击预览"`，hover 轻微放大（`hover:scale-105 transition`）。
- 单击预览**不**与「双击粘贴」「单击选中」冲突：缩略图是独立点击目标，`stopPropagation` 拦住行级 click/dblclick。

> Settings 剪贴板页（`ClipboardPanel.tsx`）若同样展示图片缩略图，后续按相同模式接入（次要，可在 plan 末尾追加）。

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 原图缺失（`get_image_blob` 返回 None） | `get_image_full` `Err("图片数据不存在")` → 前端画布区显示「图片数据缺失」+ 关闭按钮 |
| 并发再开（A 预览中，B 再开） | 后端 emit load 推 B 的 `{imageId}`；A 的内容被替换（单例，预览是短时操作，可接受） |
| ACL 未授权 | `image_preview_window` 必须在 capability `windows` 数组（§3.1）；前端跨窗口 emit/invoke 一律 `await`+`try/catch`，拒绝时显式日志（不静默吞） |
| 大图 base64 经 IPC | 原图已存 WebP（有压），可接受；缩略图仍用于列表，仅预览时拉原图 |
| WebP 解码 | 浏览器原生支持 WebP，dataUrl 直接渲染 |
| 窗口缩放 | 显示尺寸随 resize 重算，标注按原始像素坐标自动跟随缩放（不变形） |
| 文字标注 IME | 用 `<textarea>` 浮层（IME 安全），与截图文字标注一致 |
| 标注未持久化 | 关窗即失；保存走导出新图（已在 §2 声明） |

## 7. 测试

**后端单测（`image_preview_commands.rs`）：**
- `open_image_preview` 写 PENDING → `get_pending_image` 读回正确 `{imageId, mode}` 并 take 清空（镜像 compact_editor 测试）。
- `get_image_full`：内存 DB 插 image_data → 取回正确 base64（镜像 `get_image_thumb` 测试）。

**共享核心单测（`lib/annotation.ts`，纯函数易测）：**
- `drawAnnotationScaled` 的 scale 正确缩放坐标/线宽/字号（用 mock ctx 断言调用参数）。
- `annBounds` 各类型返回正确包围盒；`hitTestAnnotationPrecise` 空心标注命中边、不命中内部。
- 回归保障：截图改 import 后，截图既有行为不变（靠 e2e 兜，见下）。

**前端组件（ImagePreview / Toolbar）：**
- Toolbar：工具单选互斥、置顶 toggle 切换、OCR loading 态、属性浮窗显隐（mock invoke/emit）。

**e2e（手动，跨窗口 + canvas + IME，单测覆盖不到）：**
1. 剪贴板图片条目 → 单击缩略图 → 预览窗打开、显示原图（默认 1:1，超窗滚动条 + 抓手平移）。
2. 画矩形/椭圆/直线/箭头/画笔/文字标注 → 显示正确 → 撤销 → 选中移动 → 双击改文字。
3. 保存 → 对话框 → 文件内容含标注；复制 → **剪贴板历史出现这条带标注图**（浮窗刷新）+ 系统剪贴板可粘贴。
4. OCR → 识别文本 → 记事本打开并选中新建的 source=ocr 笔记，可在笔记里编辑。
5. 置顶 → 窗口浮于其他应用之上；缩放按钮放大/缩小/重置，标注不错位。
6. Esc / 原生 × → 窗销毁，macOS Dock 图标切回。
7. 回归：截图工具栏标注功能仍正常（抽取未破坏截图）。

## 8. 文档同步

- `docs/architecture.md`：窗口列表 + `image_preview_window`；命令清单 + `open_image_preview` / `get_pending_image` / `close_image_preview` / `get_image_full` / `save_image_dialog` / `copy_image_to_clipboard`；前端模块树 + `lib/annotation.ts`（共享标注核心）、`pages/ImagePreview/`。
- 本 spec → `docs/superpowers/plans/2026-07-01-image-preview.md`（writing-plans 产出）。
- `docs/superpowers/specs/2026-06-30-compact-editor-design.md`：可在末尾加一行「图片预览窗复用同款窗口/PENDING 模式」交叉引用（可选）。

## 9. 贴图模式（形态②）—— 未来，仅留基础

**目标长相**（用户参考图，Snipaste 风格）：无边框、透明、置顶；就一张带阴影的图；hover 时右上角浮出「×关闭 / 📌钉住」、右下角缩放比例。

**本需求已为其留的基础：**
- `lib/annotation.ts` 共享标注核心——贴图窗将来可直接复用（贴图上也能标注）。
- `get_image_full` 命令——贴图窗加载图片用同一个。
- `PendingImage.mode` 字段——`"pin"` 分支预留（窗口创建按 mode 切 `decorations(false)/transparent/always_on_top`，现仅 preview 分支）。
- 项目已有的透明无边框置顶 + 点击穿透机器（`result_window.rs` 的 `start_click_through_poller` / `setIgnoresMouseEvents` / CSS 伪装尺寸）——贴图窗的「钉屏 + hover 显隐工具栏」可复用这套。

**本需求不做**：不建 `image_pin_window`、不写贴图交互、不接入截图「钉住」按钮。截图工具栏的「钉住」入口属截图功能域，未来单独立项。

**多实例说明**：预览是单例（短时操作）；贴图将来需多实例（每张钉图一个窗），届时用「label 加后缀」或独立 label 方案，不影响本期预览。


---

## 来自原文件 `2026-07-01-pin-screenshot-design.md`

# 贴图功能（Pin to Desktop）设计

**日期**: 2026-07-01
**状态**: ✅ 实施完成（一期 macOS：MacPinWindow + pin_screenshot 命令）
**分支**: `feature/clipboard-research`
**分期**: 一期 macOS，二期 Windows/Linux

## 0. 概述

截图工具栏新增"钉子"图标按钮。用户框选区域后点击钉子，选区图片以原生窗口形式钉在桌面选区当前位置。支持拖拽移动、滚轮缩放、右键菜单关闭。可同时存在多个贴图。

### 核心架构

**原生 NSWindow + NSImageView**（不创建 WebView，内存 ~3MB/个）

## 1. 触发入口

截图工具栏中，OCR 按钮旁新增钉子图标按钮（`icons/pin.svg`）。点击后：
1. `invoke("pin_screenshot", { label, x, y, w, h })` 只传坐标，不传图片数据
2. 后端从 `ALL_CAPTURES` 裁剪选区 → PNG bytes → NSImage
3. 关闭截图窗口
4. 在选区当前位置创建贴图窗口

## 2. 架构

### 2.1 文件结构

```
crates/desktop/src/pin_window.rs          # PinWindow trait + macOS 实现
crates/desktop/src/screenshot_commands.rs  # pin_screenshot 命令
crates/desktop/frontend/.../Screenshot/index.tsx  # 工具栏钉子按钮
crates/desktop/frontend/public/icons/pin.svg     # 钉子图标
```

### 2.2 PinWindow trait（跨平台抽象）

```rust
pub trait PinWindow {
    /// 创建贴图窗口
    /// png_data: PNG 字节（从 ALL_CAPTURES 裁剪，无 base64）
    /// x, y: 选区全局逻辑坐标（Quartz 坐标系）
    /// width, height: 逻辑像素尺寸
    fn create(png_data: Vec<u8>, x: f64, y: f64, width: f64, height: f64);
}
```

### 2.3 数据流

```
用户点击钉子按钮
  → invoke("pin_screenshot", { label, x, y, w, h })
  → 后端：从 ALL_CAPTURES 匹配 label → crop_region → PNG bytes
  → PinWindow::create(png_bytes, sel_global_x, sel_global_y, w, h)
  → close_all_screenshot_windows()
```

零 base64、零 WebP、最小 CPU。

## 3. macOS 实现（NSWindow + NSImageView）

### 3.1 窗口创建

```objc
NSWindow:
  - styleMask: .borderless
  - level: .floating（置顶不抢焦点）
  - hasShadow: true（桌面贴附质感）
  - isMovable: false（自行处理拖拽）
  - acceptsMouseMovedEvents: true
  - backgroundColor: clear（透明）

NSImageView:
  - image: NSImage(data: png_data)
  - frame: 填满 contentView
```

### 3.2 交互

| 操作 | 实现 | 行为 |
|---|---|---|
| 左键拖拽 | `PinNSImageView.mouseDown` → `window.performWindowDragWithEvent` | 系统原生拖拽，零抖动 |
| 滚轮 | `PinNSWindow.scrollWheel` → delta × 0.01 缩放因子 → 以鼠标为中心 `setFrame_display` | 等比缩放 0.2×~5× |
| 右键 | `PinNSWindow.rightMouseDown` → 弹出 NSMenu（单项「关闭」）→ `close` | 关闭贴图 |

### 3.3 事件处理

**双类架构**（职责分离）：
- `PinNSImageView`（继承 NSImageView）— 只处理 `mouseDown`，委托给 `window.performWindowDragWithEvent` 触发系统原生拖拽
- `PinNSWindow`（继承 NSWindow）— 处理 `scrollWheel`（缩放）和 `rightMouseDown`（右键菜单）

拖拽用 `performWindowDragWithEvent` 而非手动 `mouseDragged` + `setFrameOrigin`：
- 系统内部处理，跨屏正确，无抖动
- 不需要记录初始坐标，不依赖 `locationInWindow`/`mouseLocation`（避免坐标计算 bug）

缩放细节：
```
scrollWheel:
  scale = 1.0 + deltaY * 0.01
  newWidth/Height = current × scale（限制 20~10000）
  ratio = mouseInWindow / frameSize
  newOrigin = frameOrigin + mouseInWindow - ratio × newSize
  setFrame_display(newFrame, true)
```

### 3.4 右键菜单

```
rightMouseDown:
  menu = NSMenu()
  item = NSMenuItem("关闭", action: close, keyEquivalent: "")
  item.setTarget(self)
  menu.addItem(item)
  NSMenu.popUpContextMenu(menu, event, contentView)
```

## 4. 多实例管理

- 不维护全局列表（NSWindow 被 ARC 持有，close 后自动释放）
- 每次创建独立 NSWindow + 独立事件处理
- 关闭一个不影响其他

## 5. 坐标系

贴图窗口的位置用选区的**全局 Quartz 逻辑坐标**（与截图选区一致）：
- 前端传 `x, y, w, h`（CSS 逻辑像素，窗口局部）
- 后端转全局：`sel_global_x = win_origin_x + x`（同截图坐标映射）
- NSWindow 的 frame 用 Quartz 坐标（原点左下），需翻转 Y

## 6. 跨平台规划

| 平台 | 方案 | 内存/个 | 分期 |
|---|---|---|---|
| macOS | NSWindow + NSImageView（objc2） | ~3MB | 一期 |
| Windows | HWND + DirectComposition（windows-rs） | ~5MB | 二期 |
| Linux/X11 | override-redirect + XPutImage（x11rb） | ~2MB | 二期 |
| Linux/Wayland | gtk-layer-shell | ~5MB | 二期 |

二期实现替换 `PinWindow` trait 的 macOS 实现，上层调用不变。

## 7. 限制（一期）

- macOS only
- 不支持鼠标穿透
- 不支持标注编辑（贴图是最终图片）
- 不支持复制到剪贴板（截图时已入库）


---

## 来自原文件 `2026-07-02-capx-canvas-anchored-design.md`

# Canvas-Anchored 匹配设计

**日期**: 2026-07-02
**状态**: ✅ 实施完成（Canvas-Anchored 匹配落地，18 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-07-02-capx-stitch-robustness-design.md`](./2026-07-02-capx-stitch-robustness-design.md)（健壮性优化前置）、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)（调研第四节首次提出）

---

## 一、背景与根因

健壮性优化（时序平滑 + 动态阈值 + 三级降级链）实施后，丢内容问题改善但仍存在。

### 根因（systematic-debugging Phase 1 确认）

**帧间比较的累积漂移**：`self.reference` 只在匹配成功时更新。匹配失败时 reference 不前进，后续帧与过时的 reference 比较 → 真实位移逐帧累积 → 最终超出搜索范围 → 内容永久丢失。

```
帧 N-1: 匹配成功，reference = N-1，dy=-30
帧 N:   匹配失败（模糊/回弹），reference 仍 = N-1
帧 N+1: 与 N-1 比较 → 真实位移 = 60px，可能还能匹配
帧 N+K: 位移 > 440px（降级 1 上限）→ 永远匹配不上 → 内容永久丢失
```

### 业界验证

| 工具 | 策略 | 结论 |
|------|------|------|
| **ShareX** | 帧-画布比较（每帧与 `ResultImage` 底部 strip 对齐） | 无累积漂移，接缝最干净 |
| **ScrollSnap** | 帧间比较，但失败时不前进参考帧 | 轻量级近似，下帧自动追赶 |
| **Scrollshot** | 帧间 + 时序平滑中位数 | 用统计补偿，非根治 |

ShareX 的帧-画布比较是最彻底的解决方案。[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md) 第四节早在调研阶段就提出了此方案（"Canvas-Anchored Matching"），但当时未实施。

---

## 二、目标与非目标

### 目标

1. **根治丢内容**：匹配输入源从 `self.reference`（上一帧）改为画布底部 strip，消除累积漂移
2. **对外 API 零改动**
3. **保留现有健壮性优化**：时序平滑、动态阈值、三级降级链不受影响

### 非目标

- 不做 Sobel 梯度特征（阶段二，验证后按需）
- 不做多 band 投票（阶段二）
- 不做文本主体区域检测（阶段二）

---

## 三、设计

### 3.1 核心改造：匹配输入源从 reference 帧改为画布底部 strip

#### 当前（帧间比较）

```
reference（上一帧完整灰度）↔ curr_gray（当前帧完整灰度）
匹配成功后：self.reference = curr_gray
匹配失败：self.reference 不变 → 下一帧与过时 reference 比较 → 位移突变
```

#### 改为（Canvas-Anchored）

```
canvas_bottom_gray（画布底部 STRIP_H 行灰度）↔ curr_gray（当前帧完整灰度）
每帧重新从 canvas_buf 提取 → 无论多少帧失败，画布底部始终是最新已确认内容
```

### 3.2 数据流变化

**移除 `self.reference: GrayBuf` 字段**。新增每帧即时提取的画布底部灰度。

```rust
// 每帧 process_frame 开始时，从 canvas_buf 提取底部 strip 转灰度
fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
    let row_bytes = self.canvas_w as usize * 4;
    let start_row = self.canvas_h.saturating_sub(strip_h);
    // 直接从 canvas_buf 底部 strip_h 行 RGBA 转灰度
    let mut data = Vec::with_capacity(strip_h as usize * self.canvas_w as usize);
    for y in start_row..self.canvas_h {
        let row_start = y as usize * row_bytes;
        for x in 0..self.canvas_w as usize {
            let off = row_start + x * 4;
            let r = self.canvas_buf[off] as u32;
            let g = self.canvas_buf[off + 1] as u32;
            let b = self.canvas_buf[off + 2] as u32;
            let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
            data.push(luma as u8);
        }
    }
    GrayBuf { data, width: self.canvas_w as usize }
}
```

**关键区别**：这个 GrayBuf 的 height = strip_h（不是完整帧高度），因为只提取画布底部。匹配逻辑需要适配：模板条和搜索空间都基于这个"短"灰度图。

### 3.3 匹配逻辑调整

当前 `find_overlap_spatial_ext` 假设 ref_buf 和 curr_buf 是同样大小的完整帧。Canvas-Anchored 后，ref_buf 只有 strip_h 行（画布底部），curr_buf 是完整帧。

**调整搜索逻辑**：ref_buf 的全部 strip_h 行就是模板，在 curr_buf 的 `[eff_top, eff_bottom]` 范围内搜索 ref_buf 的最佳对齐位置。

```rust
// ref_buf: 画布底部 strip（strip_h 行）
// curr_buf: 当前帧完整灰度（h 行）
// 在 curr_buf 中搜索 ref_buf 的对齐位置
// y_offset = ref_buf 顶部在 curr_buf 中的 y 坐标
// dy = y_offset - eff_top（ref_buf 顶部 vs 有效区顶部）
//   → dy < 0 表示 curr 在 ref 下方有新内容（用户向下滚了）
```

具体来说，`search_best_offset` 的搜索范围从 `[eff_top, eff_bottom - strip_h]` 变为 `[eff_top, eff_bottom - strip_h]`，模板就是整个 ref_buf（strip_h 行），不再需要 `extract_template` 单独提取（ref_buf 本身就是模板）。

### 3.4 对现有改造的影响

| 组件 | 变化 |
|------|------|
| `self.reference` 字段 | **移除**，每帧从 canvas_buf 提取 |
| `GrayBuf::from_rgba(frame)` | 保留，仍用于 curr_buf |
| `find_overlap_spatial_ext` | ref_buf 高度 = strip_h（非完整帧）；搜索逻辑微调 |
| `extract_template` | 简化——ref_buf 本身就是模板，直接传 ref_buf.data |
| `search_best_offset` | 模板来源从 extract_template 变为 ref_buf 全量 |
| `try_match_1d_projection` | ref_proj 从画布底部提取 |
| `apply_fallback_match` | 不再 `self.reference = curr_buf.clone()` |
| `decide_match` | 不变 |
| `is_stationary` | 不变 |
| `dynamic_sad_accept` | 不变 |
| `estimate_texture_density` | 输入改为画布底部灰度 |
| `finalize` | ref_buf 也改为画布底部 |

### 3.5 初始化处理

首帧 `process_frame` 初始化时，canvas 就是首帧裁剪后内容。此时 `extract_canvas_bottom_gray` 从 canvas 底部提取，作为下一帧的匹配模板。**无需特殊初始化逻辑**——第二帧直接与画布底部比较，完全正确。

### 3.6 性能影响

- 每帧提取画布底部 STRIP_H=80 行 RGBA 转灰度：80 × canvas_w × (4 读 + 1 写) ≈ 80×2000×5 = 800K ops ≈ 0.1ms
- 相比之前 `GrayBuf::from_rgba(frame)` 转整帧灰度：600×2000×5 = 6M ops ≈ 0.6ms
- **反而更快**（只转 80 行而非整帧）

---

## 四、API 兼容性

对外 API 零改动。`reference` 字段是私有的，移除不影响调用方。

---

## 五、测试策略

### 现有 16 测试必须保持全绿

### 新增测试

1. **Canvas-Anchored 不丢内容**：构造 5 帧序列，第 3 帧是模糊帧（匹配失败），验证第 4 帧能与画布底部正确对齐（而非与第 3 帧比较）
2. **画布底部提取正确性**：构造已知画布内容，验证 `extract_canvas_bottom_gray` 提取的灰度与手动计算一致
3. **连续失败后恢复**：构造 3 帧连续匹配失败后，第 4 帧恢复正常，验证能正确拼接（不位移突变）

---

## 六、风险与缓解

| 风险 | 缓解 |
|------|------|
| Canvas-Anchored 后 ref_buf 高度变化导致搜索逻辑 bug | 新增"画布底部提取正确性"测试；搜索范围严格基于 ref_buf 高度 |
| 画布底部正好是 sticky 区域导致匹配异常 | 画布首帧已裁掉 sticky_bottom，底部始终是有效内容 |
| finalize 时画布可能很大，提取底部仍只取 STRIP_H 行 | 只取 strip_h 行，不随画布增长 |


---

## 来自原文件 `2026-07-02-capx-ncc-sobel-design.md`

# NCC + Sobel 梯度匹配引擎重写

**日期**: 2026-07-02
**状态**: ✅ 实施完成（NCC + Sobel 匹配落地，19 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-07-02-capx-canvas-anchored-design.md`](./2026-07-02-capx-canvas-anchored-design.md)、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)

---

## 一、背景与根因

经过多轮 SAD + 灰度框架下的调参（动态阈值、三级降级链、best-guess、亚像素插值），拼接质量仍不稳定。核心问题是 **SAD 在周期性内容（文件列表、表格线）中产生多峰假匹配**，而我们的置信度/阈值/best-guess 机制本质是在补这个算法缺陷的洞。

### 业界对照

Scrollshot（Rust 开源滚动截屏）的源码级分析揭示了一条成熟的匹配管线：

| 维度 | 我们（SAD + 灰度） | Scrollshot（NCC + Sobel） |
|------|-------------------|---------------------------|
| 特征源 | 原始灰度值 | Sobel 边缘梯度（对渲染差异免疫） |
| 匹配准则 | 手写整数 SAD | `imageproc::template_matching::match_template`（NCC） |
| 周期内容 | 多峰 → 需要置信度/降级/best-guess 补丁 | NCC 峰更锐利，自然区分真假匹配 |
| 模板 | 固定 80px | 5 种高度并行（`{1,2,3,5,8} × min_overlap`） |
| 验证 | 单一 conf > 0.5 | 5 道独立检查 |

### 为什么 NCC 更好

SAD 在"差一个周期"的位置 SAD 差异很小（都是 ~20），而 NCC 在正确位置给出 0.95+，错误位置给出 0.3——数学上的归一化天然区分真假匹配。

### 为什么 Sobel 梯度更好

原始灰度值受抗锯齿、Retina 子像素渲染、JPEG 压缩影响。Sobel 梯度只保留结构性边缘特征，对这些像素级差异免疫。

---

## 二、目标与非目标

### 目标

1. **替换匹配核心**：用 `imageproc` 的 NCC + Sobel 梯度替代手写 SAD + 灰度
2. **保留 Canvas-Anchored 架构**：每帧从画布底部提取模板
3. **保留健壮性设计**：dy_history 时序平滑、best-guess 熔断（但简化——NCC 更准后大部分降级可移除）
4. **API 零改动**

### 非目标

- 不做多模板并行（rayon）——保持单模板但高度自适应
- 不做文本主体检测（Otsu/墨水密度）——当前固定 10%/80% 裁剪已够用
- 不做滚动条排除——固定裁剪覆盖
- 不替换抛物线插值——我们的实现比 Scrollshot 的更完整

---

## 三、设计

### 3.1 匹配管线

```
Canvas-Anchored（画布底部 80px strip）
  → Sobel 梯度（imageproc::gradients::sobel_gradients）
  → 归一化（mean + 3σ，纯色退化）
  ↓
当前帧 ROI 灰度
  → Sobel 梯度
  → 归一化（同上）
  ↓
imageproc NCC 模板匹配（CrossCorrelationNormalized）
  → 最佳 y 偏移 + NCC 分数
  → 抛物线亚像素插值（已有实现）
  → 多道验证（分数 + 局部 delta + 全局 delta）
```

### 3.2 特征图生成（Sobel + 归一化 + 纯色退化）

```rust
use imageproc::gradients::sobel_gradients;
use imageproc::stats::histogram;

/// 将 GrayBuf 转为 Sobel 梯度特征图 + 归一化。
/// 纯色区域（max_gradient=0）返回 (空白, false)，调用方退回灰度。
fn to_feature_map(gray: &GrayBuf) -> (GrayImage, bool) {
    let luma_img = gray.to_gray_image();  // GrayBuf → image::GrayImage
    let gradients = sobel_gradients(&luma_img);

    let max_gradient = gradients.iter().map(|p| p[0]).max().unwrap_or(0);
    if max_gradient == 0 {
        return (GrayImage::new(luma_img.width(), luma_img.height()), false);
    }

    // 归一化：mean + 3σ
    let mean = mean_of(&gradients);
    let stddev = stddev_of(&gradients, mean);
    let normalizer = (mean + 3.0 * stddev).max(1.0);

    let normalized = GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
        let g = gradients.get_pixel(x, y)[0] as f32;
        let scaled = (g / normalizer) * 255.0;
        image::Luma([scaled.round().clamp(0.0, 255.0) as u8])
    });
    (normalized, true)
}
```

### 3.3 NCC 匹配

```rust
use imageproc::template_matching::{match_template, find_extremes, MatchTemplateMethod};

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
/// 返回 (best_y_offset, ncc_score, response_map)
fn ncc_match(
    template: &GrayImage,  // 画布底部 strip 的特征图（模板）
    search_region: &GrayImage,  // 当前帧 ROI 的特征图（搜索区域）
) -> (f64, f64, ImageBuffer<Luma<f32>, Vec<f32>>) {
    let response = match_template(
        search_region,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1 as f64;
    let best_score = extremes.max_value as f64;
    (best_y, best_score, response)
}
```

### 3.4 多道验证（替代单一 conf > 0.5）

Scrollshot 的 5 道验证，我们精简为 3 道：

```rust
const NCC_SCORE_THRESHOLD: f32 = 0.75;       // 最低 NCC 分数
const LOCAL_CONFIDENCE_DELTA: f32 = 0.005;    // best vs 次优差值
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.002;   // best vs 远处差值（≥4px）

fn validate_match(
    response: &ImageBuffer<Luma<f32>, Vec<f32>>,
    best_y: usize,
    best_score: f32,
) -> bool {
    // 1. 最低分数
    if best_score < NCC_SCORE_THRESHOLD { return false; }

    // 2. 局部置信度：best vs best±1 的最大值差
    let local_alt = max_adjacent(response, best_y);
    if best_score - local_alt < LOCAL_CONFIDENCE_DELTA { return false; }

    // 3. 全局置信度：best vs 距离≥4px 的最大值差
    let distant_alt = max_distant(response, best_y, 4);
    if best_score - distant_alt < GLOBAL_CONFIDENCE_DELTA { return false; }

    true
}
```

### 3.5 process_frame 核心流程

```rust
pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
    // ... 宽度校验、eff_top/eff_bottom 计算（不变）...

    // 1. Canvas-Anchored：从画布底部提取 strip
    let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);
    let canvas_ref_map = to_feature_map(&canvas_gray);

    // 2. 当前帧 ROI 灰度 + 特征图
    let roi_top = ...;
    let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, eff_bottom);
    let curr_map = to_feature_map(&curr_gray);

    // 3. 纯色退化：任一帧无特征 → 回退灰度
    let (template, search_region) = if canvas_ref_map.1 && curr_map.1 {
        (canvas_ref_map.0, curr_map.0)
    } else {
        (canvas_gray.to_gray_image(), curr_gray.to_gray_image())
    };

    // 4. NCC 匹配
    let (best_y, ncc_score, response) = ncc_match(&template, &search_region);

    // 5. 多道验证
    if !validate_match(&response, best_y as usize, ncc_score as f32) {
        // 降级链（简化为 best-guess only）
        return self.try_best_guess(frame, ...);
    }

    // 6. 抛物线亚像素插值（已有实现，复用 response map）
    let refined_y = parabolic_refine(&response, best_y);

    // 7. 追加 + 状态更新（dy_history 等，不变）
    ...
}
```

### 3.6 降级链简化

NCC + Sobel 更准后，大幅简化降级链：

- **移除降级 1**（扩大搜索范围）：NCC 失败通常意味着真的没对齐内容
- **移除降级 2**（缩小模板）：固定模板 + 纯色退化已覆盖
- **保留降级 3**（1D 投影）：作为最后的图像匹配尝试
- **保留降级 4**（best-guess）：带熔断的历史估算

### 3.7 GrayBuf 增强

```rust
impl GrayBuf {
    /// 转为 image::GrayImage（供 imageproc 使用）
    fn to_gray_image(&self) -> image::GrayImage {
        image::GrayImage::from_raw(self.width as u32, (self.data.len() / self.width) as u32, self.data.clone())
            .expect("GrayBuf → GrayImage 失败")
    }
}
```

### 3.8 移除的代码

- `search_best_offset`（整数 SAD 主搜索）→ 替换为 `ncc_match`
- `extract_template`（模板预提取）→ NCC 直接用 GrayImage
- `estimate_confidence`（稀疏采样置信度）→ 替换为多道验证
- `sparse_sad_at_offset` → 删除
- `estimate_texture_density`（纹理密度评估）→ Sobel 梯度天然提供
- `dynamic_sad_accept`（动态 SAD 阈值）→ NCC 固定阈值 0.75
- `SAD_ACCEPT`、`MIN_CONFIDENCE`、`SPEED_PENALTY` 等常量 → 删除或替换

### 3.9 保留的代码

- Canvas-Anchored 架构（`extract_canvas_bottom_gray`）
- `dy_history` 时序平滑 + `is_stationary`
- `estimate_dy_hint` + best-guess 熔断
- `try_match_1d_projection`（降级 3）
- 抛物线插值（`parabolic_refine`，从 response map 提取 ±1 分数拟合）
- ROI 灰度转换（`from_rgba_roi` + `y_offset`）
- 画布 `Vec<u8>` + 惰性缓存
- `quick_stationary_check`（best-guess 前静止检测）

---

## 四、依赖

`Cargo.toml` 已有 `imageproc = "0.25"`。需确认：
- `imageproc::template_matching::match_template` — ✅ 0.25 有
- `imageproc::gradients::sobel_gradients` — ✅ 0.25 有
- `imageproc::definitions::Image`（response map 类型）— ✅

可能需要升级 `imageproc` 到 `0.26`（Scrollshot 用的版本）以获得最新 API。

---

## 五、API 兼容性

对外 API 零改动。所有替换在私有函数内部。

---

## 六、测试策略

### 现有 18 测试

- 合成图测试（`make_frame`）必须保持全绿
- 注意：`make_frame` 生成的是灰度渐变 + 周期条纹，Sobel 梯度对其的特征提取可能与原始灰度不同——需要验证 NCC 在这些合成图上也能正确匹配

### 新增测试

1. **Sobel 特征图生成**：纯色输入返回 `(blank, false)`，正常输入返回有特征的图
2. **NCC 匹配精度**：已知位移的合成帧，NCC 应返回正确偏移 + 高分数
3. **纯色退化**：两帧纯色输入，匹配应回退灰度
4. **多道验证**：构造低 NCC 分数 / 低 delta 的响应图，验证被拒绝

---

## 七、风险与缓解

| 风险 | 缓解 |
|------|------|
| `imageproc::match_template` 计算量大于手写 SAD | NCC 用 FFT 优化（imageproc 内部），且我们只搜索单列（垂直一维），实际计算量可控。若超 30ms 可降采样 |
| Sobel 预处理增加每帧开销 | `sobel_gradients` 是 O(W×H) 的简单卷积，比 SAD 主搜索本身快 |
| NCC 在我们的合成测试帧上表现不同 | 先验证现有 18 测试全绿，失败则调整 `make_frame` 特征密度 |
| `imageproc` 0.25 vs 0.26 API 差异 | 先 check 0.25，不够再升级 |

---

## 八、验收标准

1. `cargo test -p octopus-capx` 全绿（现有 + 新增）
2. `cargo check -p octopus-desktop` 无错误
3. API 零改动
4. **e2e 实测**：滚动截屏在文件列表、代码编辑器、网页（含纯色区域）场景下无重复/丢内容/断裂（需人工实测确认后才同步 main）


---

## 来自原文件 `2026-07-02-capx-stitch-robustness-design.md`

# 滚动拼接健壮性优化设计

**日期**: 2026-07-02
**状态**: ✅ 实施完成（3 改造 + 16 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-06-12-capx-optimization-design.md`](./2026-06-12-capx-optimization-design.md)（性能优化，已完成）、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)（算法调研）

---

## 一、背景与动机

性能优化（P1-P5）完成后，拼接引擎的核心瓶颈从性能转向**健壮性**。用户实际使用中三个主要痛点：

| 症状 | 严重度 | 频率 |
|------|--------|------|
| **B 错位/重叠** — 文字行接不上，内容错位 | 高 | 常见 |
| **C 丢内容** — 某段画面缺失 | 高 | 常见 |
| **A 容易断** — 滚到一半拼接停止，长图不完整 | 中 | 偶发 |

### 根因分析（对照当前 stitch.rs）

| 症状 | 根因 | 位置 |
|------|------|------|
| **C 丢内容** | 回弹/模糊帧整体 SAD 抬高，`stationary_sad_avg` 与 `best_sad_avg` 差距缩小，触发 `stationary < best + 1.0` → **误判静止** → 真实滚动内容被丢弃 | `decide_match` `stitch.rs` |
| **B 错位** | 周期性列表中，差一个周期的假匹配 SAD 与真值接近；硬阈值 `SAD_ACCEPT=7.5` 无法区分"纹理丰富但真实"与"纹理丰富但假匹配" | `search_best_offset` 无周期校验 |
| **A 容易断** | `find_overlap_spatial_ext` 返回 `None` → `process_frame` 直接 `return Ok(false)`，**无降级重试** | `process_frame` |

---

## 二、目标与非目标

### 目标

1. **解决 C 丢内容**：时序平滑替代静态校验硬覆盖，单帧抖动不误判静止
2. **解决 B 边界**：动态自适应 SAD 阈值，根据纹理密度 + 历史基线调整接受门槛
3. **解决 A 容易断**：三级兜底降级链，单次匹配失败时依次尝试备选策略
4. **对外 API 零改动**：`Stitcher::new/process_frame/finalize/canvas/height` 签名不变

### 非目标（留待"全面"阶段）

- **不做分层粗精搜索**（降采样粗搜）— 当前暴力搜索 + 动态阈值已能解决 BC；分层引入新复杂度
- **不做动态模板高度** — 降级链中的"缩小到 40px"已覆盖空白页场景
- **不做预处理均值滤波/降采样** — 灰度转换已有，当前噪声不是主要问题
- **不做帧率自适应采集** — manual 模式用户自控滚动，固定 30ms 采样合理

---

## 三、设计

### 3.1 改造 1：时序平滑替代静态校验硬覆盖（解决 C 丢内容）

#### 当前问题

`decide_match` 中的静态校验是"硬覆盖"——一次 `stationary_sad < best_sad + 1.0` 即强制返回 `dy=0`：

```rust
// 当前 decide_match
if stationary_sad_avg < STATIONARY_SAD || stationary_sad_avg < best_sad_avg + 1.0 {
    return Some((0.0, 1.0));  // 强制判静止，哪怕真实在滚动
}
```

回弹场景：画面轻微拉伸，整体 SAD 抬高。stationary_sad（dy=0 处）与 best_sad（搜索到的最佳）差距从正常的 5+ 缩小到 < 1.0 → 触发误判静止 → 真实滚动被丢弃。

#### 新方案：dy 时序历史 + 滑动均值判静止

**Stitcher 新增字段**：

```rust
/// 最近若干帧的 dy 历史，用于时序平滑判断静止。
dy_history: VecDeque<f64>,
```

`new()` 初始化为空 `VecDeque::with_capacity(8)`。

**静止判断改为时序平滑**：

```rust
/// 判断当前是否为静止状态（基于历史 dy 均值）。
/// 回弹帧 dy 可能抖动到 -3，但历史 [-15,-12,-10,-3] 均值 -10，不判静止。
fn is_stationary(&self) -> bool {
    if self.dy_history.len() < 3 {
        return false;  // 不足 3 帧，不判静止（让 SAD 主匹配决定）
    }
    let n = self.dy_history.len().min(5);
    let recent: f64 = self.dy_history.iter().rev().take(n).sum::<f64>() / n as f64;
    recent.abs() < STATIONARY_DY_THRESHOLD  // 均值 |dy| < 2.0 视为静止
}
```

**`decide_match` 移除静态校验硬覆盖**，改为只返回搜索结果：

```rust
fn decide_match(
    best_y_offset: u32, best_sad_avg: f64, stationary_sad_avg: f64,
    confidence: f64, template_y: u32, dynamic_threshold: f64,
) -> Option<(f64, f64)> {
    // 保留静止 SAD 锚点作为"绝对静止"快速路径
    // （画面完全没动时 stationary_sad 极低，这是安全的）
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0));
    }
    // 移除 stationary < best + 1.0 的硬覆盖——交由 is_stationary() 时序判断
    if best_sad_avg < dynamic_threshold && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

**`process_frame` 中静止判断上移**：

```rust
// 主匹配后
let result = find_overlap_spatial_ext(...);
match result {
    Some((dy, conf)) => {
        // 双重静止校验：
        // ① SAD 主匹配返回 dy ≈ 0（绝对静止 SAD 锚点极低时）
        // ② 时序历史均值也接近 0（is_stationary）
        // 两者都满足才判静止跳过——防止单帧 SAD 误判
        if dy.abs() < 0.5 && self.is_stationary() {
            return Ok(false);  // 确认静止，跳过
        }
        // 正常追加 + 更新 dy_history
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
        // ...append...
    }
    None => { /* 进入降级链（改造 3）*/ }
}
```

> **关键变化**：原来 `decide_match` 内 `stationary_sad < best + 1.0` 单帧即硬覆盖为静止；现在需要 **dy≈0 且时序也确认**才判静止。回弹帧 SAD 可能返回 dy≈0（误匹配），但时序均值 -10 否决 → 不丢内容。

**效果**：回弹帧 dy 抖动到 -3，但历史均值 -10 → 不判静止 → 内容不丢。

### 3.2 改造 2：动态自适应 SAD 阈值（解决 B 边界 + C 模糊帧被拒）

#### 当前问题

`SAD_ACCEPT=7.5` 是硬编码，空白页 SAD 天然低（纹理少）、密集列表天然高（纹理多），同一阈值不适合所有场景。

#### 新方案：纹理密度 + 历史 EMA 基线

**纹理密度评估**（Sobel 式水平梯度阈值计数）：

```rust
/// 评估模板条区域的纹理密度（边缘像素占比）。
/// 复用 sample_cols 的相邻列对做水平差分，O(strip_h × n_cols)，开销极低。
fn estimate_texture_density(buf: &GrayBuf, sample_cols: &[usize], template_y: u32) -> f64 {
    let mut edge_count = 0u32;
    let mut total = 0u32;
    for dy in 0..STRIP_H {
        let row = buf.row((template_y + dy) as usize);
        for w in sample_cols.windows(2) {
            total += 1;
            if (row[w[0]] as i32 - row[w[1]] as i32).abs() > TEXTURE_EDGE_THRESHOLD {
                edge_count += 1;
            }
        }
    }
    if total == 0 { return 0.0; }
    edge_count as f64 / total as f64
}
```

**Stitcher 新增字段**：

```rust
/// 历史成功匹配的 SAD 均值（EMA，指数移动平均）。
sad_baseline: f64,
```

`new()` 初始化为 `0.0`。每次成功匹配后用 EMA 更新：

```rust
const SAD_BASELINE_ALPHA: f64 = 0.3;  // EMA 平滑系数
self.sad_baseline = SAD_BASELINE_ALPHA * best_sad_avg + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
```

**动态阈值计算**：

```rust
/// 根据当前帧纹理密度 + 历史 SAD 基线动态计算 SAD 接受阈值。
fn dynamic_sad_accept(&self, texture: f64) -> f64 {
    // 纹理越丰富 → 绝对 SAD 天然更高 → 允许更高阈值
    let texture_bonus = texture * TEXTURE_BONUS_FACTOR;  // texture ∈ [0,1], factor=30
    // 历史基线浮动：EMA 均值的 1.5 倍 + 5 作为上界
    let baseline_cap = self.sad_baseline * SAD_BASELINE_MULTIPLIER + SAD_BASELINE_PADDING;
    (SAD_ACCEPT + texture_bonus).min(baseline_cap).max(SAD_ACCEPT)
}
```

**`find_overlap_spatial_ext` 接受动态阈值参数**：

```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32, x_end: u32,
    eff_top: u32, eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,  // 新增：动态阈值
) -> Option<(f64, f64)>
```

`decide_match` 用传入的 `dynamic_threshold` 替代硬编码 `SAD_ACCEPT`。

**效果**：
- 密集列表（纹理密度 0.3）→ 阈值 ~16.5，但 baseline_cap 可能限制到 ~12
- 空白页（纹理密度 0.05）→ 阈值 ~9.0
- 回弹帧（历史 baseline 6）→ 阈值上限 ~14
- 周期列表假匹配（best_sad 可能 5.0）→ 仍在阈值内，但改造 1 的时序平滑 + 改造 3 的周期校验兜底

### 3.3 改造 3：多级兜底降级（解决 A 容易断 + B 假匹配）

#### 当前问题

`find_overlap_spatial_ext` 返回 `None` → `process_frame` 直接 `return Ok(false)`。

#### 新方案：三级降级链

```rust
pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
    // ...（初始化/eff 计算不变）...

    let curr_buf = GrayBuf::from_rgba(frame);
    let texture = estimate_texture_density(&curr_buf, &sample_cols, eff_bottom - STRIP_H);
    let sad_accept = self.dynamic_sad_accept(texture);

    // 主匹配（动态阈值）
    if let Some(result) = self.try_match(&curr_buf, &sample_cols, eff_top, eff_bottom, MAX_SCROLL, sad_accept) {
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 1：扩大搜索范围 ×2（快速滚动可能超出 MAX_SCROLL）
    if let Some(result) = self.try_match(&curr_buf, &sample_cols, eff_top, eff_bottom, MAX_SCROLL * 2, sad_accept) {
        log::info!("[stitch] fallback 1: expanded search range");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 2：缩小模板到 40px + 放宽阈值 ×1.5（空白页/低纹理场景）
    if let Some(result) = self.try_match_strip(&curr_buf, &sample_cols, eff_top, eff_bottom, 40, sad_accept * 1.5) {
        log::info!("[stitch] fallback 2: reduced strip height");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 3：1D 灰度投影匹配（对纹理极少的纯色场景鲁棒）
    if let Some(result) = self.try_match_1d_projection(&curr_buf, eff_top, eff_bottom, sad_accept) {
        log::info!("[stitch] fallback 3: 1D projection match");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 全部失败：不停止，等下一帧（desktop 层 250 帧兜底处理）
    log::info!("[stitch] all fallbacks exhausted, skipping frame");
    Ok(false)
}
```

**内部方法**：

```rust
/// 主匹配（封装 find_overlap_spatial_ext 调用）
fn try_match(&self, curr: &GrayBuf, cols: &[usize], eff_top: u32, eff_bottom: u32, max_scroll: u32, sad_accept: f64) -> Option<(f64, f64)>;

/// 缩小模板匹配（strip_h 可变版本）
fn try_match_strip(&self, curr: &GrayBuf, cols: &[usize], eff_top: u32, eff_bottom: u32, strip_h: u32, sad_accept: f64) -> Option<(f64, f64)>;

/// 1D 灰度投影匹配（行均值序列 SAD）
fn try_match_1d_projection(&self, curr: &GrayBuf, eff_top: u32, eff_bottom: u32, sad_accept: f64) -> Option<(f64, f64)>;
```

#### 1D 灰度投影匹配算法

将每行像素按 `sample_cols` 取均值，降为一维信号，对一维信号做 SAD 搜索。对纯色/低纹理场景（2D SAD 缺乏特征）反而更鲁棒，因为行均值对横向噪声做了平均。

```rust
fn try_match_1d_projection(&self, curr: &GrayBuf, eff_top: u32, eff_bottom: u32, sad_accept: f64) -> Option<(f64, f64)> {
    // 参考帧行均值信号
    let ref_proj = row_means(&self.reference, eff_top, eff_bottom);
    // 当前帧行均值信号
    let curr_proj = row_means(curr, eff_top, eff_bottom);
    // 在 ref_proj 底部 strip 范围搜索 curr_proj 的最佳对齐位置
    // ...类似 search_best_offset 但在一维信号上...
}
```

#### `find_overlap_spatial_ext` 参数化 strip_h

当前 `STRIP_H=80` 是常量，降级 2 需要可变。改为参数化：

```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32, x_end: u32,
    eff_top: u32, eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,    // 新增：动态阈值
    strip_h: u32,       // 新增：可变模板高度（默认 STRIP_H）
) -> Option<(f64, f64)>
```

---

## 四、API 兼容性

**对外 API 零改动**。所有新增字段（`dy_history`、`sad_baseline`）和新增方法（`is_stationary`、`dynamic_sad_accept`、`try_match*`、`apply_match`）均为私有。`new/process_frame/finalize/canvas/height` 签名不变。

---

## 五、新增常量

```rust
/// 时序平滑：静止判断的 dy 均值阈值（近 N 帧 |dy| 均值 < 此值 → 静止）
const STATIONARY_DY_THRESHOLD: f64 = 2.0;
/// dy 历史长度
const DY_HISTORY_LEN: usize = 8;
/// 纹理密度评估：水平梯度阈值
const TEXTURE_EDGE_THRESHOLD: i32 = 20;
/// 动态阈值：纹理密度奖励系数（texture ∈ [0,1] × 30 → 最多加 30）
const TEXTURE_BONUS_FACTOR: f64 = 30.0;
/// 动态阈值：历史基线倍数（sad_baseline × 1.5 + 5）
const SAD_BASELINE_MULTIPLIER: f64 = 1.5;
const SAD_BASELINE_PADDING: f64 = 5.0;
/// 动态阈值：EMA 平滑系数
const SAD_BASELINE_ALPHA: f64 = 0.3;
/// 降级 2：缩小模板高度
const FALLBACK_STRIP_H: u32 = 40;
/// 降级 2：阈值放宽倍数
const FALLBACK_SAD_MULTIPLIER: f64 = 1.5;
```

---

## 六、测试策略

### 新增测试用例（合成图 + 不变量）

1. **时序平滑不误判回弹**：
   - 构造 4 帧序列：dy=[-15,-12,-10,-3]（最后帧模拟回弹 dy 变小）
   - 验证：第 4 帧不被 `is_stationary()` 判为静止（均值 -10 > 阈值）

2. **真实静止被时序识别**：
   - 构造 5 帧序列：全部相同（dy=0）
   - 验证：`is_stationary()` 返回 true

3. **动态阈值随纹理变化**：
   - 构造高纹理帧（密集条纹）+ 低纹理帧（纯色 + 少量文字）
   - 验证：`dynamic_sad_accept()` 对高纹理返回更高阈值

4. **降级链触发**：
   - 构造一个超出 MAX_SCROLL 的快速滚动帧
   - 验证：主匹配失败但降级 1（扩大范围）成功

5. **1D 投影匹配**：
   - 构造纯色背景 + 少量文字的帧（2D SAD 纹理不足）
   - 验证：降级 3 的 1D 投影能匹配

6. **baseline EMA 更新**：
   - 连续匹配后 `sad_baseline` 收敛到合理值

7. **回归测试**：现有 12 个测试必须保持全绿

### `make_frame` 工具增强

现有 `make_frame` 需扩展支持：
- 可控纹理密度（稀疏/密集条纹）
- 可控噪点水平（模拟拖影/模糊）
- 回弹序列构造（dy 先大后小）

---

## 七、风险与缓解

| 风险 | 缓解 |
|------|------|
| 时序平滑引入延迟：前 2-3 帧 `dy_history` 不足，不判静止 | `is_stationary()` 在 `len < 3` 时返回 false，让 SAD 主匹配决定 |
| 动态阈值放过坏帧（纹理丰富时阈值放宽） | baseline_cap 上界限制（EMA × 1.5 + 5）；改造 1 的时序平滑兜底（假匹配 dy 与历史差距大 → 不追加） |
| 1D 投影匹配在强周期列表中也有多峰 | 作为最后降级手段；置信度要求更严（`confidence > 0.25` 而非 0.15） |
| `find_overlap_spatial_ext` 签名变化（新增 2 参数） | 内部私有函数，调用方都在 stitch.rs 内；`try_match` 封装统一调用 |
| 降级链增加每帧计算量（最坏 4 次匹配） | 正常情况主匹配一次通过，降级仅在边缘场景触发；每级降级都有日志便于调优 |

---

## 八、验收标准

1. `cargo test -p octopus-capx` 全绿（现有 12 + 新增 ≥6 = ≥18 个测试）
2. `cargo check -p octopus-capx -p octopus-desktop` 无错误
3. API 零改动（`lib.rs` 无变化，公开签名不变）
4. 源码无新增裸魔法数字（全部命名常量）
5. 手动验证：滚动截屏在密集列表、空白页、快速滚动场景下不再出现错位/丢内容/断裂（需人工实测，测试覆盖算法层）


---

## 来自原文件 `2026-07-02-ipc-binary-design.md`

# IPC 二进制传输改造

**日期**: 2026-07-02
**状态**: ✅ 实施完成（3 层全部落地：scroll://done 双向往返消除 + 前端→Rust Raw body + Rust→前端 ipc::Response）
**分支**: `optimize-capx`

---

## 一、改造范围

### 层 1：消除 scroll://done 双向往返

Rust 端已有 `png_bytes`，保存模式下直接弹对话框，不经过前端。

- `scroll://done` payload 移除 `png_base64`，只传 `{ id }`
- 前端保存按钮改为：先 `invoke("stop_scroll_capture", { mode: "save" })`，保存对话框由 Rust 端在停止后直接弹出

### 层 2：前端→Rust 改用 Raw body

| 命令 | 当前 | 改后 |
|------|------|------|
| `copy_image_to_clipboard` | `{ pngBase64: String }` | Raw body + headers |
| `save_image_dialog` | `{ pngBase64: String }` | Raw body + headers |
| `confirm_screenshot_with_data` | `{ pngBase64, label, width, height }` | Raw body + headers（元数据走 headers） |
| `ocr_screenshot` | `{ pngBase64, label }` | Raw body + headers |

前端：`canvas.toDataURL("image/png")` → `canvas.toBlob()` → `arrayBuffer()` → `new Uint8Array()` → `invoke(cmd, uint8array, { headers: { ... } })`

### 层 3：Rust→前端改用二进制返回

| 命令 | 当前 | 改后 |
|------|------|------|
| `get_image_full` | `data:image/webp;base64,...` | `ipc::Response::new(webp_bytes)` |
| `get_screenshot_image` | `{ image: b64 }` | `ipc::Response::new(jpeg_bytes)` + headers 传 width/height |
| `scroll://done` | `{ id, png_base64 }` | `{ id }`（移除 base64） |

保留 base64：`get_image_thumb`（<50KB）、`scroll://frame`（前端需要 data: URL）。

---

## 二、API 兼容性

这是内部 IPC 改造，前端和后端同步修改，无外部 API 变化。


---

## 来自原文件 `2026-07-02-notepad-type-migration-design.md`

# 记事本 type 迁移：content_text + type 双类型（text / markdown）

> **状态**：已实现、e2e 通过、合并 main（merge `6e004ac`，2026-07-02）。本文为最终设计；架构概览见 `docs/architecture.md` §octopus-notepad。

## 0. 设计演进：为何是双类型而非三类型

本方案最初把 `type` 放开到 `html` / `text` / `markdown` 三态（html = TipTap 富文本为主）。三类型落地后，富文本在 macOS WKWebView 下历经多轮踩坑才勉强可用——输入时序 `immediatelyRender`、Tailwind preflight 重置 `h1`–`h6` 需手补 `.prose` 样式、`window.prompt` 被禁需改内联输入框、图片需 `note-img:` 协议 + ACL 桥接——收益却不匹配：记事本定位是 ASR / OCR / 剪贴板的「内容收集箱」，纯文本 + Markdown 已足够。

用户判定「富文本对本应用无用、还不好控制」，遂彻底移除：`NoteType` 收窄为 `text` / `markdown`，TipTap 依赖全删，历史 `type=html` 笔记由 DB 迁移 v11→v12 删除。**不要再为记事本重新提议富文本编辑器 / TipTap / 恢复 `NoteType::Html`。**

> `content_html` 列在 schema 保留但恒空（`split_body` 永远写 `""`），无需删列——保留列避免再走一次 ALTER，且不占语义。

## 1. 目标与非目标

### 目标
1. notes 表采纳 `content_text + type` 结构（保留 `content_html` 列但恒空）。
2. `type` 双态：`text`（纯文本，默认）/ `markdown`（md 源码）。
3. 安全迁移链 v9 → v12（幂等、不丢历史数据；v12 删 html 笔记为预期行为）。
4. 前端双类型编辑器：`text` = textarea / `markdown` = 源码 + 可折叠预览（`marked`）。
5. `type` 端到端透传：DB → `Note` struct → IPC 命令 → 前端类型 → 编辑器分发。
6. 非手动来源（剪贴板 / OCR / ASR）存入默认 `type=text`。

### 非目标
- **不**恢复 egui UI（egui 方案暂停，记事本维持 webview）。
- **不**改 `source` 语义（source = 来源 asr/ocr/clipboard/manual，与 type = 内容格式正交）。
- **不**改 FTS5 索引结构（仍索引 `content_text`）。
- **不**支持富文本 / 图片嵌入（已移除）。

## 2. 最终 Schema（`crates/infra/src/db.sql`，新库 INIT_SQL）

```sql
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',   -- 纯文本/md源码（FTS + 预览 + 编辑 source of truth）
    content_html  TEXT    NOT NULL DEFAULT '',   -- 保留列，恒空（富文本已移除）
    type          TEXT    NOT NULL DEFAULT 'text',  -- text | markdown
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);
```
- `content_text` 是 source of truth（text 存原文、markdown 存源码）。
- `content_html` 保留恒空，**不删列**。
- FTS5 表 + 触发器 + 索引不变（索引 `content_text`，`type` 不进 FTS）。

## 3. 迁移链（v9 → v12，幂等）

| 版本跃迁 | 操作 | 说明 |
|----------|------|------|
| v9 → v10 | `ALTER TABLE notes ADD COLUMN type TEXT NOT NULL DEFAULT 'html'`（先查列存在再 ALTER） | 引入 `type` 列；历史笔记默认 `html` |
| v10 → v11 | `ALTER TABLE notes ADD COLUMN content_html ...`（先查列存在） | 兼容曾被 egui 分支重建过（无 `content_html`）的库 |
| v11 → v12 | `DELETE FROM notes WHERE type='html'` | 富文本下线，删除历史 html 笔记 |

- 全新安装（v0/v1）执行 INIT_SQL 后直接 `user_version=12`。
- 每个 ALTER 分支先用 `PRAGMA table_info(notes)` 查列是否存在，幂等保护，避免重复迁移崩溃。
- v12 删除前 `SELECT COUNT(*)` 计数并 log，无 html 笔记时为 noop。

## 4. 后端（Rust）

### 4.1 `NoteType` enum（`crates/notepad/src/model.rs`）
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Text,
    Markdown,
}
// as_str:  Text => "text", Markdown => "markdown"
// from_str: "markdown" => Markdown；"text" / 已下线的 "html" / 未知值 => Text（容错）
```
- `from_str` 未知值回退 `Text`（富文本已移除，历史 `html` 值安全降级为纯文本，不丢内容）。

### 4.2 `Note` struct
含 `note_type: NoteType` 字段（在 `content_text` 之后）。

### 4.3 `store.rs` 读写
- `create_note_at(conn, source, source_ref_id, body, note_type)` / `update_note_at(conn, id, title, body, note_type)`：签名带 `note_type`，INSERT/UPDATE 写 `type` 列。
- `split_body(body, _note_type) -> (String, String)`：返回 `("", body.to_string())`——`content_html` 恒空，`content_text` = body 原文。**无 html 抽取**（`extract_text` 已随 `serialize.rs` 删除）。
- `row_to_note` + 所有 SELECT 列表（`list_notes_at` / `query_with_search` 两分支 / `get_note_at`）在 `updated_at` 后加 `, type`。

### 4.4 IPC 命令（`crates/desktop/src/note_commands.rs`）
- `create_note` / `update_note`：加 `note_type: String` 参数 → `NoteType::from_str` → 透传 store。
- `save_transcription_to_note` / `save_ocr_to_note`：内部固定 `NoteType::Text`（纯文本来源），不再 `<p>` 包裹。
- **已删** `get_note_image` / `insert_note_image`（图片桥接随富文本移除），`main.rs` invoke_handler 同步注销。

## 5. 前端（React + TypeScript，无 TipTap）

- `types/note.ts`：`export type NoteType = "text" | "markdown";`
- `lib/notepad.ts`：`createNote` / `updateNote` 透传 `noteType`；**已删** `getNoteImage` / `insertNoteImage`。
- `NoteEditor.tsx`：按 `note_type` 分发——`markdown` → `<MarkdownEditor>`，`text` → `<textarea>`；标题 + 正文 800ms debounce 保存（同走 `textBody`）；导入（`.md`/`.txt`）→ `textBody`；导出（md→`.md` / text→`.txt`）。顶部仅类型标签 + 导入/导出/收藏/置顶。
- `MarkdownEditor.tsx`：源码 textarea + 轻量工具栏（标题/粗体/斜体/列表/引用/代码/链接）+ `marked` 可折叠预览。
- `NoteList.tsx`：`TYPE_TABS`（全部 / 纯文本 / Markdown）+ 行内 type 角标（`MD` / `T`）+ 行内删除。
- **已删**：`extensions.tsx`（TipTap 编辑器 + Image NodeView）、`index.css` 的 `.ProseMirror` 样式、`@tiptap/*` + `tiptap-markdown` 依赖（bundle 1.2M → 410K）。

## 6. 数据兼容

| 场景 | 处理 |
|------|------|
| 历史 v9 笔记（无 type） | v10 给 `type='html'`，v12 删除（富文本下线，预期行为） |
| 剪贴板 / OCR / ASR 存入 | `type='text'`（纯文本来源） |
| 手动新建 | 默认 `type='text'`，可选 markdown；**已建锁定**不可改 type |
| FTS 搜索 | 不变（索引 `content_text`：text=原文，markdown=源码） |

> 已建笔记 type 锁定：新建时选 type，一旦创建固定。想换格式 → 复制内容新建。理由：跨格式转换有损且复杂，锁定避免数据损坏。

## 7. 测试策略

- `NoteType` roundtrip（`as_str`/`from_str`）+ 未知值 / 已下线 `"html"` → `Text`。
- `create_note_at` / `update_note_at`：text / markdown 直存原文（`content_html` 空，无抽取）。
- 迁移：`migrate_v11_to_v12_deletes_html_keeps_text_markdown`（插 html×2 + text + markdown → 仅留 text/markdown，v=12）；`migrate_v11_to_v12_no_html_is_noop`。
- 现状：infra 48 / notepad 19 / desktop 56 单测全绿。

## 8. 影响面清单（最终）

| 文件 | 改动 |
|------|------|
| `crates/infra/src/db.sql` | notes 建表 `type DEFAULT 'text'`；`content_html` 保留恒空 |
| `crates/infra/src/db.rs` | INIT_SQL → v12；v9→v10 / v10→v11 / v11→v12 迁移分支 + 测试 |
| `crates/notepad/src/model.rs` | `NoteType`（text/markdown）+ `Note.note_type` |
| `crates/notepad/src/store.rs` | create/update 带 `note_type`；`split_body` 恒空；row/SELECT 加 type |
| `crates/notepad/src/serialize.rs` | **已删**（+ `Cargo.toml` 去 `scraper`） |
| `crates/desktop/src/note_commands.rs` | create/update 透传 type；save_* 固定 text；删图片桥接命令 |
| `crates/desktop/src/main.rs` | invoke_handler 注销 `get_note_image` / `insert_note_image` |
| `crates/desktop/frontend/src/types/note.ts` | `NoteType = "text" \| "markdown"` |
| `crates/desktop/frontend/src/lib/notepad.ts` | create/update 透传 noteType；删 image 封装 |
| `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx` | 按 type 分发 textarea / MarkdownEditor |
| `crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx` | md 编辑器（源码 + 工具栏 + marked 预览） |
| `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx` | TYPE_TABS + type 角标 + 行内删除 |
| `crates/desktop/frontend/src/pages/Notepad/extensions.tsx` | **已删**（TipTap 编辑器） |
| `crates/desktop/frontend/src/index.css` | 删 `.ProseMirror` 样式 |
| `crates/desktop/frontend/package.json` | 加 `marked`；删 `@tiptap/*` + `tiptap-markdown` |

