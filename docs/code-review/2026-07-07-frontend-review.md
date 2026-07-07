# 前端 Webview 代码审查报告 — 2026-07-07

> Worktree: `.claude/worktrees/scroll-stitch-debug`（基于 main）
> Scope: 前端 Webview 模块（React + Vite + Tailwind v4 + Tauri 2）静态行为审计
> 工具: 源码逐行核对（file:line + 时序/边界分析）
> 关联: memory `imagepreview-canvas-32767-limit`（问题4 已实施，GUI 核心已验证：长图不崩+缩放）

## 总体结论

**7 项发现全部核实成立，无误报。** 7 项均已实施（`tsc -b --force` EXIT=0 + 前端 79 单测通过）；问题 4（Canvas 超 Chromium 32767 上限）代码已修并已合 main（7f8d4d1），GUI 核心项已验证（超大图不崩 + 缩放正常，2026-07-07 实测）；DOM/sticky 其余对齐项（标注贴图/滚动/resize/dpr）可选补充（纯几何换算已抽 `viewportMath.ts` 单测覆盖）。

---

## 一、逻辑缺陷与内存泄露

### 1. 快捷键录入全局监听器泄露（已修）
- 文件：`frontend/src/pages/Settings/GeneralPanel.tsx:115-139`
- 问题：`startShortcutCapture` 在 `document` 注册 capture 阶段 keydown 监听器，cleanup 仅在 handler 内部调用（按键/Esc）。用户点「按下快捷键…」后不按键直接切 Tab/关窗 → 组件卸载，监听器永久泄露。stale handler 持续 `preventDefault/stopPropagation` 劫持全局键盘（其他输入框打不出字），并 `setVal` 写已卸载组件。
- 处置：监听器生命周期改绑 `capturingKey` state，用 `useEffect([capturingKey])` 管理，卸载自动 `removeEventListener`。

### 2. Tauri listen 异步事件泄露（已修）
- 文件：`frontend/src/pages/Screenshot/index.tsx:99-116`
- 问题：`listen` 返回 `Promise<UnlistenFn>`，cleanup 直调局部变量。组件在 Promise resolve 前卸载（用户快速 ESC/右键取消截图）→ cleanup 时变量仍 undefined、注销失效 → 监听器永久留在 Tauri 事件总线，每快速取消一次泄露一对。
- 处置：加 `cancelled` 哨兵，resolve 时若已卸载立即 `fn()` 自注销（对齐 ImagePreview `ocr-screenshot` 监听范式 `unlisten.then(f=>f())`）。

---

## 二、性能与资源

### 3. 多 Tab 并发挂载 ImagePreview 致内存暴涨（已修）
- 文件：`frontend/src/pages/CompactEditor/index.tsx:443-470`
- 问题：所有 Tab `display:none` 全挂载。隐藏的图片 Tab 仍跑 ImagePreview 的 `useEffect([imageId])`：并发 `get_image_full` + `createImageBitmap(resizeQuality:"high")` + Canvas 撑整图物理尺寸。5 图全分辨率位图常驻 → 内存×Tab 数，低端机 OOM。
- 处置：图片 Tab 懒加载——仅活跃 Tab 挂载 ImagePreview，非活跃显示占位。textarea 保持全挂载（保编辑状态）。切回重新加载（标注/缩放重置可接受）。

### 4. 长图 Canvas 超 Chromium 32767 上限（已实施，GUI 核心已验证）
- 文件：`frontend/src/pages/ImagePreview/index.tsx`（drawBg 重写 + canvas DOM 移位）、新增 `ImagePreview/viewportMath.ts` + `.test.ts`
- 问题：`drawBg` 设 `canvas.width=dispW*dpr / height=dispH*dpr`（dispW=natW×zoom）。长图 `natH×zoom×dpr > 32767`（Chromium 单边硬限；总面积 268M 像素另有限）→ Canvas 空白崩。触发：长图 `zoomFitWidth`（按宽适配→高度仍大）或手动放大；macOS Retina dpr=2 时 natH≈16383 即触发，滚 10+ 屏常见。注释 L268「canvas 固定窗口大小，只画可见区域」是设计意图，但实现成了「大画布随滚 + drawBg 只重绘可见切片」，意图与实现不符。
- 处置（已实施）：① canvas 物理尺寸改视口固定 `vw×dpr × vh×dpr`（`sc.clientWidth/clientHeight`，钳制 ≤32767 兜底，永不超限）；② canvas 从 wrapper 内移到 content 首子（wrapper 之前，DOM 顺序决定 stack 下层），`position:sticky; top:0; left:0` 钉 scrollContainer 视口，`pointer-events:none` 让鼠标穿透到 wrapper；③ drawBg 改视口坐标绘制——算图片矩形∩视口矩形的露出区（content 空间），drawImage 到视口坐标 `(dstL,dstT)`，dst 从「图片空间」改「视口空间」；④ 几何换算抽 `viewportMath.ts` 三纯函数（`computeVisibleRect`/`visibleToViewport`/`computeSrcSlice`）+ 17 单测；⑤ 不改：wrapper、SVG/OCR overlay（仍随滚，SVG 无 canvas 尺寸上限）、`canvasCoords`/`toNatural`（手算 scroll 偏移）、bitmap 预缩放（失败静默 fallback 原图，渐进降级）、composePngBytes（独立 canvas）。
- 对齐原理：canvas sticky 画到视口坐标 `(dstL,dstT)` = 图片露出区在视口的屏幕位置；wrapper 内 SVG overlay 随滚、viewBox 自然坐标映射到 wrapper CSS，露出区的标注与 canvas 画的图片重合 → 标注精准贴图。
- 验证状态：GUI 核心项已通过（超大图不崩 + 缩放正常，2026-07-07 实测）；DOM/sticky 其余对齐项（标注贴图/滚动/抓手平移/resize/dpr）可选补充，清单见下文「问题 4 GUI 验证清单」（核心回归即清单第 1、6 项已 ✅）。

---

## 三、主题与 UI

### 5. 暗色下拉弹出层白字白底（已修）
- 文件：`frontend/src/pages/Result/index.tsx:759`
- 问题：降噪/润色模式下拉 popup 硬编码 `bg-white`，项内文字用 `text-foreground`。暗色下 foreground 浅白 → 白底浅字，选项不可读。
- 处置：`bg-white` → `bg-background`，跟随主题翻转。

### 6. 暗色删除确认对比度不足（已修）
- 文件：`frontend/src/pages/Clipboard/ClipboardItem.tsx:160`
- 问题：删除二次确认行 `bg-red-50`（接近白的浅粉）+ `text-foreground/90` 浅字，暗色对比度极低。
- 处置：`bg-red-50` → `bg-red-500/15`（半透明红，亮暗均可见）。

### 7. 标注命中测试行为不一致（已修）
- 文件：`frontend/src/lib/annotation.ts:343-387`
- 问题：`hitTestAnnotationPrecise`（选择工具下点选/拖动标注）两缺陷：
  ① 实心 rect/oval 仅查边缘（onEdge），不查内部 → 实心大矩形/椭圆必须精准点几像素宽的边框才能选中拖动；
  ② if-else 链漏 `diamond`，落到 else 用 bounding box → 空心菱形四角外透明区/中心误中，而空心 rect/oval 中心不中，行为割裂。
- 处置：rect/oval `filled` 时判定鼠标在图形内部（rect 矩形包含；oval 归一半径 `r≤1`）；新增 diamond 分支（filled 用 L1 范数 `nd≤1` 内部判定；空心 `|nd-1|×min(halfW,halfH)≤HIT_DIST` 四边距离），与 rect/oval 一致。

---

## 处置汇总

| # | 问题 | 文件 | 处置 | 验证 |
|---|---|---|---|---|
| 1 | 快捷键监听泄露 | GeneralPanel.tsx | ✅ useEffect 管理 | tsc + 实测切 Tab 不劫持 |
| 2 | listen 异步泄露 | Screenshot/index.tsx | ✅ cancelled 哨兵 | tsc + 实测快速取消 |
| 3 | 图片 Tab 并发挂载 | CompactEditor/index.tsx | ✅ 懒加载 | tsc + 实测多图 Tab 内存 |
| 4 | Canvas 超 32767 | ImagePreview/index.tsx + viewportMath.ts | ✅ 视口固定 canvas + sticky | tsc + 17 单测；GUI 核心已验证（长图+缩放） |
| 5 | 暗色 popup 白字 | Result/index.tsx | ✅ bg-background | tsc + 暗色实测 |
| 6 | 暗色删除对比 | ClipboardItem.tsx | ✅ bg-red-500/15 | tsc + 暗色实测 |
| 7 | hit-testing 不一致 | annotation.ts | ✅ filled 内部 + diamond 分支 | tsc + 实测选中行为 |

`tsc -b --force` EXIT=0；`vitest run` 79 passed（含 viewportMath 17 新单测）。

---

## 问题 4 GUI 验证清单（核心项已 ✅，其余可选补充）

**核心回归已通过（2026-07-07 实测）：第 1 项（超大图不崩）+ 第 6 项（缩放正常）。** 其余 DOM/sticky 对齐项 jsdom 测不出（无真实布局/canvas/sticky），如需可于真实 Tauri 窗口（CompactEditor 图片 tab）补充验证——**任一项错位即视为未解决，须回调**：

| # | 场景 | 验证点 |
|---|---|---|
| 1 | **长图**（natH×zoom×dpr > 32767，如滚 10+ 屏） | ✅ canvas 不再空白崩，正常显示（核心回归，已验证） |
| 2 | 短图（小于视口） | 图片居中，canvas 不超出视口 |
| 3 | 高图垂直滚动 | canvas 钉视口，只画露出区，滚动流畅无闪烁 |
| 4 | 宽图水平滚动 | canvas 钉视口左，横向露出区正确 |
| 5 | 双向滚动（宽×高图） | canvas 双向钉视口，无错位 |
| 6 | zoom 连续放大/缩小（防抖 150ms 期间） | ✅ 占位帧（原图）正常，bitmap 替换无闪（已验证） |
| 7 | zoom 到 MAX_ZOOM=8 | bitmap 可能失败→fallback 原图仍能画 |
| 8 | 画标注（矩形/箭头/文字/画笔）后滚动 | **标注精准贴图**（canvas 底图与 SVG overlay 同步） |
| 9 | 文字草稿 textarea 输入中滚动 | textarea 随 wrapper 滚（不随 canvas），位置正确 |
| 10 | OCR overlay/mask 切换后滚动 | OCR 块精准贴图 |
| 11 | 抓手平移（tool=none 拖拽）到边界 | canvas 持续钉视口，平移流畅 |
| 12 | 窗口 resize（拖大拖小） | canvas 物理尺寸同步，居中重算（fit 模式） |
| 13 | dpr 变化（多显示器拖窗 1↔2） | canvas 物理尺寸重算，不模糊 |
| 14 | 切换图片（imageId 变） | canvas 立即清旧画新，无残留 |
| 15 | 保存（composePngBytes） | 合成 PNG 不受影响（独立 canvas） |

---

## 附：与 2026-06-12 审查的关系

2026-06-12 审查聚焦 Rust（死代码/重复/超长），其 §3.3 列出前端 3 个超长组件（Screenshot 1011 / ImagePreview 807 / Result 739 行）建议拆分。本轮在其基础上做**行为正确性**审计（内存泄露/暗色/命中测试），两者互补：前者结构、后者行为。
