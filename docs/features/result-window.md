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

## 3. 流式渲染（CM6 AsrEditor）

> 2026-07-11 改造：contentEditable + 手写光标系统（`caret.ts` 122 行 + `CaretBlink.tsx` 49 行）替换为 CodeMirror 6 `AsrEditor.tsx`，实现**始终可编辑 + 随说随编**。~350 行旧代码删除。

前端 `update-result` listener 接收后端 `emit("show-result", {text, caret})` / `emit("update-result", {text, caret})` 渲染。

**单调渲染**（CM6 `updateListener` 区分用户编辑与程序写入）：
- `isUserEdit(update)`：`transactions.some(tr => tr.isUserEvent("input"|"delete"|"drop"|"paste"))`。程序 dispatch 无 userEvent 注解 → 不标记编辑态。
- 用户首次编辑 → `editingRef = true` + fire-and-forget `enter_edit_mode`（信号后端 `trim_buffer(5.0)`——麦克风不停，保留最后 5 秒音频，恢复后送 ASR 防"嘴比手快"丢字）。
- 编辑态下 `update-result` 事件被 `editingRef` 拦截（return early，不写入 CM6）。
- 非编辑态：新文本 `startsWith` 当前 → 立即写入 CM6；跳变/段切换延迟 300ms 合并（`DIVERTED_DELAY_MS`）。

**`set_caret` / `set_selection`** 保留：CM6 `updateListener` 在 `selectionSet && !docChanged && !editingRef` 时触发——折叠选区→`set_caret`，非折叠→`set_selection`。

---

## 4. Dirty Ranges 编辑提交

> 用户编辑 CM6 后，只标记实际改动的区域为 `Edited`，未编辑区域保留原始 SegmentKind。

**Dirty ranges 收集**：`Array<[from, to]>`，仅**插入**创建 range（`toB > fromB`）；纯删除不创建。每次 change → `iterChangedRanges` → push → sort + merge overlapping/adjacent。

**3 条提交路径**：Cmd+Enter / 保存按钮（`useImperativeHandle commit`）/ 空闲 2000ms 自动（`IDLE_TIMEOUT`）。

**第 4 条：停止录音 / 立即润色前强制 flush**（2026-08-02）：coordinator 在停止（Toggle/InstantStop/HandsFreeStop）和「立即润色」四条路径前先 emit `flush-edit` 事件，前端收到即 `commit()` 并回 `edit_flushed`（带 flush_id 校验），200ms 超时兜底继续——防 2s 防抖窗口内未提交的用户编辑被润色结果覆盖。

**Commit payload**：`{ text, dirtyRanges, hasEdited, caret?, selection? }`。

**后端 `commit_edit`**：
- 非空 dirty + `has_edited` → 全部标 `Edited`
- 空 dirty + `!has_edited` → `rebuild_segments`（纯删除保留原 kind）
- 非空 dirty → `rebuild_segments(old, flat, dirty_ranges)`：**字符级 walk**——构建 old 逐字符 kind 映射，dirty→Edited，clean→逐字符匹配保留原 kind。`push_or_merge` 合并相邻同 kind 段。

**3 态**：流式态 / 编辑态 / 空闲态（Idle = 停止后浏览，仍可自由编辑）。

**Reset**：clear/hide/show 时 `asrEditorResetKey++` → AsrEditor `key={resetKey}` 重挂。

---

## 5. 润色集成

详见 [coordinator.md](./coordinator.md) §7-§9。

| 模式 | 触发 | 行为 |
|------|------|------|
| 停顿润色（mode=2） | 静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 段边界 | 全篇一次润色，不重置流式引擎 |
| 立即润色（PolishNow） | 工具栏「立即润色」按钮 / toggle 中按单键短按 | 忽略 mode（2026-08-01：原 `polish_global_shortcut` 已删除） |
| 最终润色 | 停止后（mode=1/2） | `Stage::Polishing` 异步线程跑 LLM |

---

## 6. 选中替换

CM6 `selectionSet` 事件替代旧 contentEditable 拖选流程：
- 折叠选区 → `set_caret`（中插位置，后续 delta 从此生长）
- 非折叠选区 → `set_selection`（替换范围，后续 delta 替换此区）

> 旧 contentEditable 三重陷阱（Range clamping / document mouseup / 坐标边界判断）已废弃——CM6 选区 API 原生可靠。


## 8. 快捷键

| 快捷键 | 默认 | 作用域 | 功能 |
|--------|------|--------|------|
| `asr_shortcut` | — | 全局 | 开始/停止录音（Toggle） |
| `edit_shortcut` | CmdOrCtrl+Enter | 结果窗聚焦 | toggle 进入/保存编辑 |
| `edit_global_shortcut` | Alt+E | 全局 | 唤起结果窗 + toggle 编辑 |
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

---

## 11. Instant 视图（PTT/hands-free 模式）

2026-08-01 合并窗口重构后，`result_window`（720×480 透明窗）成为 ASR 唯一窗口实例——按录音模式在前端切换两套视图，`display:none` 不卸载组件（保留 CM6 编辑状态）：

- **toggle 视图**（上述 §1-10）：顶部居中精简小条 + 工具栏 + CM6 编辑器
- **instant 视图**（PTT/hands-free）：底部居中指示卡（`pages/Result/InstantView.tsx`，从原独立 `InstantOverlay` 搬入）

**模式切换**：后端在 show 前按 `INSTANT_MODE` 决定位置（toggle 顶部居中 / instant 贴底 `position_bottom_center`）+ emit `record-mode: "toggle"|"instant"`；前端 listener 设 `recordModeRef`（ref 持有，避免 React 闭包陷阱）→ 切换两视图的 `display`。穿透 poller 按模式切可交互区（顶部 `BAR_H` / 底部 `INSTANT_BAR_H=80`）。

**InstantView 四态**（listening/processing/polishing/done）：

- **listening 实时显示尾部最新内容**（2026-08-01）：流式识别期间 `update-result` 事件 payload.text 持续追加，listening 态显示**末尾 28 字符**（`LISTENING_TAIL_CHARS = 28`，`text.slice(-28)`）——用户看到的是「正在说的最新字」，不是从头开始累积的长文本。容器 `dir="rtl"` 让省略号出现在开头（视觉上"从右侧涌入"）。done 态保留完整文本（`truncate` 开头截断）。
- processing / polishing：spinner 动画 + 状态文案（如路由命中可视化携带「⏳ 润色中 · 模板名（app名）」，详见 [architecture.md §应用感知润色](../architecture.md)）
- done：文本展示 500ms 后 `hide_result` 回 Idle

详见 [merge-asr-windows spec](../superpowers/specs/archived/2026-08-01-merge-asr-windows-design.md) + [instant-live-text spec](../superpowers/specs/archived/2026-08-01-instant-live-text.md)。
