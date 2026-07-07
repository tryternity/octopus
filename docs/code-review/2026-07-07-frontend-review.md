# 前端 Webview 代码审查报告 — 2026-07-07

> Worktree: `.claude/worktrees/scroll-stitch-debug`（基于 main）
> Scope: 前端 Webview 模块（React + Vite + Tailwind v4 + Tauri 2）静态行为审计
> 工具: 源码逐行核对（file:line + 时序/边界分析）
> 关联: memory `imagepreview-canvas-32767-limit`（问题4 待办）

## 总体结论

**7 项发现全部核实成立，无误报。** 6 项已修复（`tsc -b --force` EXIT=0），1 项（Canvas 超 Chromium 32767 上限）因需 GUI 视觉验证、留作独立任务。

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

### 4. 长图 Canvas 超 Chromium 32767 上限（留作独立任务）
- 文件：`frontend/src/pages/ImagePreview/index.tsx:285-286`
- 问题：`drawBg` 设 `canvas.width=dispW*dpr / height=dispH*dpr`（dispW=natW×zoom）。长图 `natH×zoom×dpr > 32767`（Chromium 单边硬限；总面积 268M 像素另有限）→ Canvas 空白崩。触发：长图 `zoomFitWidth`（按宽适配→高度仍大）或手动放大；macOS Retina dpr=2 时 natH≈16383 即触发，滚 10+ 屏常见。注释 L268「canvas 固定窗口大小，只画可见区域」是设计意图，但实现成了「大画布随滚 + drawBg 只重绘可见切片」，意图与实现不符。
- 方案（实施时）：canvas 物理尺寸改视口固定 `viewport×dpr`（永不超限）+ `position: sticky` 钉 scrollContainer 视口；drawBg 按 `scrollLeft/Top` 算可见 src 切片、drawImage 到视口坐标 `(0,0)`（dst 从「图片空间」改「视口空间」）；标注 SVG overlay 仍随 wrapper 滚（SVG 无 canvas 尺寸上限）；标注坐标系 `canvasCoords`/`toNatural` 已手算 scroll 偏移不需动。
- 风险：需 GUI 验证滚动/缩放/标注视觉对齐，盲改有回归风险（改错会让所有长图预览坏，比现状仅超长图崩更糟）。详见 memory `imagepreview-canvas-32767-limit`。

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
| 4 | Canvas 超 32767 | ImagePreview/index.tsx | ⏳ 留独立任务 | memory 待办 |
| 5 | 暗色 popup 白字 | Result/index.tsx | ✅ bg-background | tsc + 暗色实测 |
| 6 | 暗色删除对比 | ClipboardItem.tsx | ✅ bg-red-500/15 | tsc + 暗色实测 |
| 7 | hit-testing 不一致 | annotation.ts | ✅ filled 内部 + diamond 分支 | tsc + 实测选中行为 |

`tsc -b --force` EXIT=0。

---

## 附：与 2026-06-12 审查的关系

2026-06-12 审查聚焦 Rust（死代码/重复/超长），其 §3.3 列出前端 3 个超长组件（Screenshot 1011 / ImagePreview 807 / Result 739 行）建议拆分。本轮在其基础上做**行为正确性**审计（内存泄露/暗色/命中测试），两者互补：前者结构、后者行为。
