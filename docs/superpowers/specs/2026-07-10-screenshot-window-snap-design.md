# 截图智能窗口识别（区域截图自动吸附窗口边界）— 设计

- 日期：2026-07-10
- 分支：asr-wordbook（worktree）
- 状态：设计中（已过脑暴澄清，待写实施计划）

## 背景

octopus 截图（`crates/capx` + `crates/desktop/src/screenshot_commands.rs`）当前为**纯手动拖拽框选**：前端 Canvas 上鼠标拖拽画矩形选区（暗遮罩 + 选区框 + 8 手柄），下游接标注工具栏 / `ocr_screenshot` / `pin_screenshot` / 复制。

调研文档 `docs/superpowers/specs/2026-07-09-action-bar-related-tools-survey.md` §6.2（Snow Shot，Tauri 同栈截图工具，octopus 已大量借鉴）指出：「智能窗口识别（区域截图自动吸附窗口/元素边界）——octopus 截图为纯手动框选。加窗口识别可减少选区操作，尤其 OCR fallback 时精准取窗口内容（鼠标悬停的元素直接 OCR）。」

Snow Shot 的做法（[mg-chao/snow-shot](https://github.com/mg-chao/snow-shot) + [官方 guide](https://snowshot.top/guide/)）：区域截图模式下鼠标移动时自动高亮悬停的窗口/元素边界，点击即选中该区域，自动吸附窗口边缘无黑边，仍可拖拽手画。

本设计在 octopus 区域截图模式加入**窗口边界自动吸附**：悬停高亮 + 单击即选整窗，减少手动框选操作；OCR fallback 时一键取整窗内容。

## 目标

- 区域截图模式：鼠标悬停自动高亮候选窗口，**单击即选中整窗**（Snow Shot 式）
- 拖拽手画仍可用——吸附与手画共存，零学习成本
- macOS 先行，架构留跨平台 trait 抽象（仿 `PinWindow` trait 模式）
- v1 **零额外权限**（`CGWindowList`，复用已有屏幕录制权限）；v2 可选 AX 元素级（**仅浏览器**）

## 非目标

- 元素级 AX 对**所有**原生应用——v2 仅对浏览器 bundle id 启用（避开原生/Electron AX 的复杂与易错，survey §4.4）
- 拖拽起止点磁吸辅助线（不做——单击取整窗已足够，磁吸辅助线收益小）
- Windows / Linux 实现（后续，trait 已预留扩展位）
- 悬停实时 OCR（v2 元素级落地后再加快路径，如双击/快捷键直送 OCR）
- 跨屏窗口吸附（跳过，见关键决策 5）

## 关键决策

1. **平台范围**：macOS 先行 + `WindowDetector` trait 抽象（仿 `PinWindow` trait）。Windows（EnumWindows + UI Automation）/ Linux（X11 `_NET_WM`）后续补 impl，不改 trait。
2. **识别粒度**：v1 `Granularity::Window`（`CGWindowListCopyWindowInfo`，零额外权限即开即用）；v2 `Element`（AX，仅浏览器 bundle id，用户授权辅助功能后启用）。
3. **交互模型**：悬停高亮 + 单击即选；`mousedown` 后 `move > 4px` 进拖拽手画（吸附高亮灭，走现有选区逻辑）；`mouseup` 且未超阈值 = 单击，选中吸附候选 `SnapRect`；单击落空（无候选）选区**不动**；按住 **Cmd** 临时禁用吸附（纯手动精框）。
4. **`hit_test` 后端实时查**（方案 A）：前端 `mousemove` 节流 **50Hz** 调后端 `hit_test_window`，后端每次实时 `CGWindowListCopyWindowInfo`。该 API 只取窗口元数据（不抓像素），微秒级；窗口新增/移动/菜单展开**始终最新**；前端零状态。本地 IPC 往返 ~1ms，50Hz 可接受。备选（前端缓存窗口列表本地命中）被否：截图期间窗口变化会 stale，z-order/多显示器坐标系搬到前端易错。
5. **跨屏窗口跳过**：窗口 bounds 不完全在鼠标所在显示器（monitor rect）内 → 不作为吸附候选，回退纯手画。跨屏截图极少，跳过最简，避免"截到半个窗口"的困惑。
6. **trait 一次到位**：v1 即带 `Granularity` 参数与 `Element` 变体，macOS 的 `Element` 分支在 v1 返回 `None`/fallback `Window`。v2 只补 impl，不改 trait 与调用方。

## 架构

```
                 ┌──────────────────────────────────────────┐
  截图覆盖窗前端  │  mousemove (50Hz 节流) → 全局逻辑坐标     │
  (Canvas 吸附层) │  → invoke('hit_test_window', gx, gy)     │
                 └──────────────────┬───────────────────────┘
                                    │
              ┌─────────────────────▼──────────────────────┐
              │ screenshot_commands::hit_test_window(x, y)  │
              │  → WindowDetector::hit_test(gx, gy, Window) │
              └─────────────────────┬──────────────────────┘
                                    │
              ┌─────────────────────▼──────────────────────┐
              │ crates/capx/src/window_detect/              │
              │  trait WindowDetector                       │
              │  └─ MacOsDetector                           │
              │      · Window: CGWindowListCopyWindowInfo   │
              │      · Element(v2): AXUIElement...AtPosition│
              │        （仅浏览器 bundle id）                │
              └─────────────────────┬──────────────────────┘
                                    │ Option<SnapRect{全局逻辑坐标}>
                                    ▼
              前端 global_to_local(rect, monitor) → 本窗 CSS
              → 画半透明高亮描边 + 5% 填充
```

## 组件

| 组件 | 职责 | 位置 |
|---|---|---|
| `WindowDetector` trait | `fn hit_test(&self, gx, gy, Granularity) -> Option<SnapRect>`；跨平台抽象 | `crates/capx/src/window_detect/mod.rs`（新增） |
| `enum Granularity` | `Window` / `Element` | 同上 |
| `struct SnapRect` | `{ x, y, w, h: f64（全局逻辑 points）, title: Option<String> }` | 同上 |
| `MacOsDetector` | `Window` = `CGWindowListCopyWindowInfo` + 命中算法；`Element`(v2) = AX（仅浏览器） | `crates/capx/src/window_detect/macos.rs`（新增） |
| `hit_test_window` 命令 | 截图覆盖窗前端调；granularity 暂固定 `Window` | `crates/desktop/src/screenshot_commands.rs`（修改） |
| `global_to_local` helper | 全局逻辑 rect → 本窗 CSS（多显示器原点反算） | `crates/desktop/src/screenshot_geometry.rs`（新增对称函数） |
| 前端吸附层 + 状态机 | 悬停高亮 / 单击即选 / Cmd 禁用 / 拖拽手画 | 截图 Canvas 组件（修改） |

## macOS 窗口命中算法（v1 Window 级）

1. `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)` 取全部 on-screen 窗口元数据
2. **滤除**：
   - `kCGWindowOwnerPID == octopus 自身 PID`（排除 octopus 截图覆盖窗，否则鼠标总命中自己的全屏窗）
   - `layer < 0`（桌面/壁纸层）
   - 无 bounds / bounds 退化（w 或 h ≤ 0）
   - **跨屏**：bounds 不完全包含在鼠标所在显示器（monitor rect）内 → 跳过
3. **命中**：候选 = bounds 含 `(gx, gy)` 的窗口；按 `layer` **降序**（数字大 = 更上层，菜单/弹窗 > 普通窗）取最上层
4. 返回其 bounds（已是全局显示逻辑 points）+ `kCGWindowName`

命中算法抽出纯函数 `pick_top_window(windows: &[WinInfo], point, monitor_rect, self_pid) -> Option<Rect>`，便于单测（FFI 部分隔离）。

## 数据流（每次 mouse-move）

1. 前端 `mousemove` → rAF + 时间戳节流 **50Hz** → 全局逻辑坐标 `(gx, gy)`（复用 `screenshot_geometry` 的窗口原点 + CSS 偏移换算）
2. `invoke('hit_test_window', gx, gy)` → 后端 `WindowDetector::hit_test` → `SnapRect`（全局逻辑）
3. 前端 `global_to_local(rect, monitor)` → 本窗 CSS → 画半透明高亮描边 + 5% 填充
4. **in-flight 去重**：新请求覆盖旧的，旧响应到达即丢弃（以最新鼠标位置为准）；避免拖动快时高亮闪烁滞后

## 前端交互状态机

```
IDLE --mousemove--> HOVERING(显吸附高亮)
HOVERING --mousedown--> ARMED(记起点+时间戳)
ARMED --move>4px--> DRAGGING(手画选区,吸附灭,走现有选区逻辑)
ARMED --mouseup(≤4px)--> CLICK_SELECT: 选区=吸附SnapRect(若有候选) --> IDLE
任意态 按住 Cmd --> 吸附禁用(不调 hit_test/不高亮),拖拽手画照常
```

- 单击落空（鼠标在纯桌面、无候选）→ 选区**不动**（与现状单击一致）
- 拖拽手画（move > 4px）期间吸附高亮隐藏，完全复用现有选区绘制/手柄/尺寸标注逻辑

## v1 / v2 边界

- **v1（本 spec 实施范围）**：仅 `Granularity::Window`，`CGWindowList`，**零额外权限**即开即用。trait 已带 `Element` 变体，macOS impl 返回 `None`（未实现）。
- **v2（后续）**：`Element` = AX。`AXUIElementCopyElementAtPosition(system_wide, gx, gy, &el)` → 读 `kAXPositionAttribute` + `kAXSizeAttribute` → `SnapRect`。**仅对浏览器 bundle id 启用**，其他应用 fallback `Window`。只补 impl，不改 trait / 命令签名 / 调用方。

## AX 权限引导（v2 才触发）

- 首次启用元素级（设置项开关）调 `AXIsProcessTrusted()`，未授权 → 打开 `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility` 并提示勾选 octopus；状态页显示「辅助功能：已/未授权」。
- **仅浏览器 bundle id** 启用元素级（`com.google.Chrome` / `com.apple.Safari` / `com.microsoft.edgemac` / `org.mozilla.firefox` / `com.brave.Browser` 等）。其他应用 owner → 元素级返回 `None`，自然 fallback 窗口级。
- Chrome 系（Chromium）元素识别需 `--force-renderer-accessibility`（survey §4.4），未开启时 AX 返回粗粒度/窗口级，自然 fallback。
- TCC 重编译失效：开发文档加 troubleshooting 重置流程（survey §4.7 Moly 教训）。

## 错误处理与边界

| 情况 | 处理 |
|---|---|
| `CGWindowList` 调用失败（极少） | `hit_test` 返回 `None` → 无高亮，回退纯手动（=现状） |
| 命中 octopus 自身窗口 | 已按 `OwnerPID == 自身` 滤除；仍命中则返回 `None` |
| 全屏应用 / Stage Manager | `CGWindowList` 照常返回，正常工作 |
| 跨屏窗口 | 跳过（不作候选）；若所有最上层都跨屏 → 无候选 → 回退手画 |
| 单击落空（纯桌面） | 选区不动（=现状） |
| v2 AX 调用慢/卡（主线程同步） | 加 ~50ms 超时，超时/失败 fallback `Window`；调用走 `spawn_blocking` 不阻塞 UI |
| 性能（窗口数极多） | `CGWindowList` 只取元数据不抓像素，微秒级；50Hz 节流兜底 |

## 测试策略

- **单测（纯逻辑，核心）**：
  - `pick_top_window`：喂构造窗口列表，断言过滤（自身 PID / layer<0 / 退化 bounds / 跨屏）+ layer 降序取最上层
  - `global_to_local`：多显示器原点偏移换算
  - `Granularity::Element` 未授权 / 非浏览器 → fallback `Window`
  - FFI 部分（`CGWindowList` / AX）隔离，不进单测
- **手动验收**（macOS GUI，无法 headless）：悬停高亮、单击选中、Cmd 禁用、拖拽手画覆盖吸附、多显示器跨屏跳过、纯桌面落空不动
- **v2 AX**：授权后命中浏览器内按钮/文本块；未授权 fallback；Electron/非浏览器退化窗口级；Chrome 未开 `--force-renderer-accessibility` 退化窗口级
- 本功能非 ASR/pipeline，截图吸附靠 GUI 交互，**手动验收为主 + 单测覆盖纯逻辑**（不套 e2e 真实录音断言那套）

## 与现有代码的关系

- **新增** `crates/capx/src/window_detect/`（`mod.rs` trait + `macos.rs` 实现）
- **修改** `crates/desktop/src/screenshot_commands.rs`：加 `hit_test_window` 命令 + `generate_handler!` 注册
- **修改** `crates/desktop/src/screenshot_geometry.rs`：加 `global_to_local(rect, monitor)` 对称 helper
- **修改** 前端截图 Canvas 组件：加吸附层 + 交互状态机（在现有「原图 + 暗遮罩 + 选区框 + 8 手柄」之上）
- **下游零改动**：吸附只改"选区怎么来"，`ocr_screenshot` / `pin_screenshot` / 标注工具栏 / 复制链路完全不变

## 调研来源

- 调研文案 `docs/superpowers/specs/2026-07-09-action-bar-related-tools-survey.md` §6.2 Snow Shot
- [mg-chao/snow-shot](https://github.com/mg-chao/snow-shot)（Tauri 同栈截图工具，11.4K★）+ [官方 guide](https://snowshot.top/guide/)
- survey §4 Moly Appshots（AX 元素树路径 + Electron `--force-renderer-accessibility` 警示 §4.4 + TCC 重编译 §4.7）
