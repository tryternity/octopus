# 结果窗口

> ASR 识别结果展示浮窗——透明无边框置顶、流式追加文字、闪烁光标定位、选中替换、编辑态、停顿/立即润色集成、快捷键。这是用户与语音输入交互的核心界面。

源文件：`crates/desktop/src/result_window.rs`、`frontend/src/pages/Result/`。

---

## 1. 窗口属性

- 物理窗口固定 **720×480**（创建即定死，**不再运行时 setSize**——`transparent`+`decorations(false)` 悬浮窗上 NSWindow 拒绝 setFrame）
- 前端 CSS 按模式切「可见容器」尺寸：
  - **默认精简态**：只渲染顶部居中 520×116 小条（文本区 `max-h-[63px]`），容器外靠 `body{background:transparent}` 透明
  - **工具栏「放大」**：切换为完整 720×480 容器
- 可拖拽、多行滚动、透明无边框、置顶
- `#container` 默认 `opacity:0`，提前 show 不产生空窗闪烁

---

## 2. 工具栏

顶部悬停工具栏：鼠标移入展开（窗口高度 116→148px），移出收起。

工具（精简为 5+ 个）：

| 工具 | 说明 |
|------|------|
| **关闭**（首位） | 放弃内容保留 DB 记录（= Discard） |
| 系统设置 | 打开设置窗口 |
| 降噪模式 | 运行时切换（`set_denoise_mode`） |
| 润色模式 | 运行时切换（`set_polish_mode`：0 关闭/1 仅最终/2 中间+最终） |
| 立即润色 | 忽略 mode 立即润色（`polish_now`） |
| 编辑 | 编辑态追加取消/保存 |

由 `app_config.hide_toolbar`（默认 `false`）控制：`true`=hover 显隐，`false`=始终显示。**运行时切换立即生效**：设置窗口改 `hide_toolbar` → emit `config-changed` 事件 → result window 的 `refreshActive()` 双向切换。

语音模型和润色模型入口已移至 Settings 页面（模型太多，下拉空间有限）。

---

## 3. 流式追加渲染

前端 `update-result` listener 接收后端 `emit("show-result", {text, caret})` / `emit("update-result", {text, caret})` 渲染。

**单调渲染**：
- 新文本是已显示内容的前缀（`startsWith`）→ 立即渲染并清待处理跳变
- 跳变 / 段切换延迟合并（`DIVERTED_DELAY_MS=300`）只渲染最新，连续跳变不闪烁

**`renderResultNow`（imperative DOM 同步）**：
- `textRef` 是 contentEditable div，React 19 对其 children 的 commit 不写 DOM（保护用户编辑）→ `renderResultNow` 须 imperative `textRef.textContent = newText`（非编辑态）
- `measureCaretPx` 长度读 DOM `firstText.nodeValue`（非 state text，否则 state 新 text 算 target、DOM 旧文本 clamp 到旧末尾 → 光标错位 + 新文字空白）
- `flushSync(setText)` 驱动 state 让 `CaretBlink` effect 触发重测

**whitespace-pre-wrap**：textRef div 加 `whitespace-pre-wrap`，否则编辑态 `innerText` 的 `\n` 在 `white-space:normal` 下折叠成空格。

---

## 4. 闪烁光标（CaretBlink）

非编辑态显示自定义闪烁光标（`CaretBlink` 组件，纯定位指示器，非 contentEditable）。

- 前端点击算 char offset（code-point 计数，与 Rust `char` 对齐）→ `invoke("set_caret", {offset})` → `set_caret` 劈段定位 `caret_gap`
- 后续 delta 从该处生长，光标后文本右推
- 光标经 caret 透传链跟随（`Emit{caret}`→`update_result(..,caret)`→前端 `setCaretPos`）

**CaretBlink 踩坑**：
- 须接收 `RefObject`（effect 内读 `.current`），不可传 `container={textRef.current}` 作 prop——editing 切换致 textRef 重挂载时 render 阶段读到 detached 旧 div，光标错落首位
- 须监听 scroll（passive + rAF 节流）重测 px（视口相对，随 `scrollTop` 变）+ 视口外（`px.top` 超 `[0, clientHeight]`）隐藏
- 初始 `measure()` 改 rAF（同帧 `flushSync`+`textContent` 写后同步读 `getBoundingClientRect` = 强制回流，高频 ASR 每帧叠加；代价 1 帧光标滞后）

---

## 5. 编辑态

coordinator 主循环 `editing` 标志置位时 tick 跳过喂引擎（**硬暂停**）。

| 操作 | 行为 |
|------|------|
| 进入编辑 | `enterEdit`：用 `caretPosRef` 捕获点击位、`placeCaretAtCodePoint` 恢复光标（纯点击可恢复，拖选 caretPos=null 仍落末尾） |
| 编辑中 | `UpdateEditBuffer { text }` 更新 `edit_buffer` |
| 提交 | `commit_edit(flat)` 整篇压成单 `Edited` 段（raw/polished 清零）+ UPDATE DB（segments JSON + text 列） |
| 取消 | `cancelEdit`：退出编辑态，恢复编辑前文本 |

编辑入口两条：
- 窗口内 `edit_shortcut` 默认 CmdOrCtrl+Enter（跨平台=⌘/Ctrl，结果窗聚焦时 toggle 进入/保存，不在设置页管理）
- 全局 `edit_global_shortcut` 默认 CmdOrCtrl+Shift+E（任意应用聚焦时唤起结果窗 show+set_focus 并 toggle 编辑，复用同一 toggleEdit，空文本只唤起不进编辑）

---

## 6. 润色集成

详见 [coordinator.md](./coordinator.md) §7-§9。

| 模式 | 触发 | 行为 |
|------|------|------|
| 停顿润色（mode=2） | 静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 段边界 | 全篇一次润色，不重置流式引擎 |
| 立即润色（PolishNow） | 工具栏「立即润色」按钮 / `polish_global_shortcut`（默认 CmdOrCtrl+Shift+S） | 忽略 mode，全部活跃 stage 支持 |
| 最终润色 | 停止后（mode=1/2） | `Stage::Polishing` 异步线程跑 LLM |

全局润色入口：`polish_global_shortcut` handler 调 `trigger_global_polish`：show 结果窗**不聚焦** + emit `global-polish-trigger` → 前端 `polishNow`（空文本静默、polishLoading 幂等）。

润色完成后 emit `polish-done` 通知前端恢复按钮。

---

## 7. 选中替换（产品核心特色）

详见 [coordinator.md](./coordinator.md) §8。

前端拖选流程：
1. `onMouseDown` 时在 `document` 上注册一次性 `mouseup` listener（鼠标移出 textRef/浮窗时 React `onMouseUp` 不触发）
2. handler 内按 `isCollapsed` 分流：
   - 折叠 → `set_caret` 中插
   - 非折叠 → `clampRangeToContainer` 裁剪后 `set_selection` + 写 `currentSelectionRef`

**前端拖选三重陷阱**（WKWebView 中拖选到容器边界的高频踩坑区）：
1. **`Range.startContainer` 飘移到父容器**——从右往左选到开头时鼠标划出 textRef 左边界 → `clampRangeToContainer` 用 `compareBoundaryPoints` 强制裁剪到容器内
2. **React `onMouseUp` 不在 textRef 外触发**——`onMouseDown` 时在 `document` 上注册一次性 `mouseup` listener
3. **mouseup 时鼠标在容器外**——用 `el.getBoundingClientRect()` 判断鼠标 X 坐标（`< rect.left` → offset=0，`> rect.right` → 末尾）

**通用教训**：WKWebView 中拖选到容器边界是一个高频踩坑区——浏览器选区 API（`Selection`/`Range`）在边界处行为不稳定。三重防御（Range clamping + document mouseup + 坐标边界判断）缺一不可。

---

## 8. 快捷键

| 快捷键 | 默认 | 作用域 | 功能 |
|--------|------|--------|------|
| `asr_shortcut` | — | 全局 | 开始/停止录音（Toggle） |
| `edit_shortcut` | CmdOrCtrl+Enter | 结果窗聚焦 | toggle 进入/保存编辑 |
| `edit_global_shortcut` | CmdOrCtrl+Shift+E | 全局 | 唤起结果窗 + toggle 编辑 |
| `polish_global_shortcut` | CmdOrCtrl+Shift+S | 全局 | 唤起结果窗（不聚焦）+ 立即润色 |
| Esc | — | 结果窗 | 取消录音（Cancel） |

---

## 9. 窗口加载就绪（ready）机制

结果窗 webview 首次加载有延迟，若后端在页面就绪前 `emit('show-result')`，事件丢失导致「文本不显示 / 不弹窗」。

- `WINDOW_READY`（AtomicBool）+ `PENDING_TEXT`（Mutex<Option<String>>）兜底——未 ready 时暂存文本
- 前端 `index.html` 加载完成后发起 `result_window_ready` Tauri command → 后端置 ready 并冲刷积压文本
- `show_result` / `update_result` 把「判 ready + 写 pending」收进同一把 `PENDING_TEXT` 锁，与 `result_window_ready` 的 store(true)+take 互斥，消除启动首帧 TOCTOU 文本滞留
- **`show_result` 的物理 `window.show()` 无条件执行**（不受 ready 门控，仅 `emit('show-result')` 受门控）——冷启动首启 webview 未 ready 时按快捷键也能立即弹窗

**前端 `result_window_ready` 时还主动调一次 `refreshActive()`** 拉取首帧工具栏配置（`edit_shortcut` / `polish_mode` / `denoise_mode` 等），避免冷启动到首次 `show-result`（录音）/ `config-changed`（设置改动）之间，窗口内 keydown 监听器读到 `edit_shortcut` 初始默认值。

---

## 10. 前端单测基建

引入 vitest 4 + jsdom 29：
- `measureCaretPx` / `codePointOffsetTo` / `codePointOffsetBefore` / `placeCaretAtCodePoint` 抽到 `Result/caret.ts`（纯函数可测，`locateCpOffset` 为 measure/place 共享 helper）
- `caret.test.ts`（14 测）锁 code-point → UTF-16 offset 对齐 + null/空容器/多文本节点分支
- jsdom 无 `Range.getBoundingClientRect`，defineProperty 补零矩形
