# 归档实施计划（archived-plans）

> 归档日期：2026-07-05（实际整理 2026-07-12）
> 范围：2026-06-12 ~ 2026-07-05 的 14 篇实施 plan，按主题同类合并。
> 配套设计归档：[`specs/2026-07-05-archived-design.md`](../specs/2026-07-05-archived-design.md)（架构决策/数据结构/接口的权威来源）。

## 归档原则

- **plan = 实施执行记录**：本归档提炼每篇 plan 的目标 / 状态+commit / 关键偏差与教训（plan 独有、design 未覆盖的部分），按主题分组。逐步骤的代码细节已被代码本身取代，不再赘述。
- **架构细节查 design 归档**：数据结构、接口签名、不变量、边界用例的设计层面内容，统一在配套的 `archived-design.md` 对应章节，本文件给出交叉链接。
- **现行 vs 下线**：除 `2026-06-26-focus-tracker-todo`（⏸️ 暂缓/下线，自动粘贴不可靠已回滚为手动 Cmd+V）外，其余 13 篇均已完成、对应功能现行存活（已对照代码核实）。

## 目录

- [一、CAPX 优化](#一capx-优化)（原 `2026-06-12-capx-optimization.md`）
- [二、死代码/重复/超长重构](#二死代码重复超长重构)（原 `2026-06-12-refactor-deadcode-dup-long.md`）
- [三、窗口焦点追踪 + 自动粘贴 ⏸️ 暂缓](#三窗口焦点追踪--自动粘贴-暂缓)（原 `2026-06-26-focus-tracker-todo.md`，已下线）
- [四、ASR 光标中插/选中替换](#四asr-光标中插选中替换)（原 `2026-07-03-asr-cursor-insert.md`）
- [五、记事本移除 + 多 tab CompactEditor + OCR 统一](#五记事本移除--多-tab-compacteditor--ocr-统一)（原 `2026-07-03-clean-used-feature.md`）
- [六、图片查看器性能优化](#六图片查看器性能优化)（原 `2026-07-03-image-viewer-perf.md`）
- [七、剪贴板管理整合](#七剪贴板管理整合)（原 `2026-07-04-clipboard-consolidation.md`）
- [八、图片查看器（OCR 文本块 + 统一查看器）](#八图片查看器ocr-文本块--统一查看器)（合并 2 篇）
- [九、2026-07-05 代码审查修复 P0/P1/P2 + rust-patterns](#九2026-07-05-代码审查修复)（合并 4 篇）
- [十、DB 合并 + FTS5](#十db-合并--fts5)（合并 2 篇）

---

## 一、CAPX 优化

> 原 plan：`2026-06-12-capx-optimization.md` ｜ 状态：✅ 重构目标基本达成（实现路线已演进，以代码为准）｜ design：[archived-design §一](../specs/2026-07-05-archived-design.md#一capx-滚动截屏模块优化)

**已落地**：Task 1（`cgimage_to_rgba` 去重）、Task 2（魔法数字提取为命名常量）、Task 3（`bgra_to_rgba` 纯函数 + 内联测试）、Task 4/5（stitch 测试网 `make_frame`/`make_frame_with_sticky`，现 33 处测试标记）、Task 6（`GrayBuf` 连续灰度 buffer + 与 `image::grayscale` 逐像素相等验证）、Task 8（画布 `canvas_buf: Vec<u8>` + `canvas_cache` 惰性缓存）、Task 9（文档同步）。

**⚠️ Task 7 未按本 plan 落地**：`find_overlap_spatial_ext` SAD 整数化重写——该函数已整体删除（`a27ee39`），算法路线从 SAD 改为 **NCC + `row_projection_means` 行投影**，后续 20+ 个 `fix(capx)`/`feat(capx)` commit 围绕新路线迭代（NCC 假匹配 / 周期性假匹配 / 滚动断裂）。

**关键约束（所有任务）**：API 零改动（`Stitcher::new/process_frame/finalize/canvas/height` 与 `capture::*` 签名不变，desktop 零改动）；灰度公式不变（`(2126*R+7152*G+722*B)/10000` 整数除法）；dy 符号约定（`dy<0` = 用户向下滚动）。

**惰性缓存实现选择**：`canvas(&self)` 下写 `canvas_cache` 用 `unsafe`（`&self` 内部可变性，函数式惰性求值标准模式，单线程安全）；备选 `RefCell`，API 不变。

---

## 二、死代码/重复/超长重构

> 原 plan：`2026-06-12-refactor-deadcode-dup-long.md` ｜ 状态：✅ 已完成（merge `903a66a` + 扩展，84+42 tests + tsc 0 error，0 warning）｜ design：[archived-design §二](../specs/2026-07-05-archived-design.md#二死代码重复超长代码重构)

**执行方式变更**：原定 subagent-driven，实际 inline execution（当时 agent 工具为只读）。保留 TDD（Task 1/2 先写失败测试 → RED → 实现 → GREEN）。

**实施偏差**：
- Task 1 `saturate_cast_i16_from_f32` 测试期望值修正：统一版本含 `is_finite` 检查，Inf → 0 非 i16::MAX（行为无变化，调用点输出不会是 Infinity）。
- Task 2 `find_monitor_for_point` 返回 `Option<usize>`（plan 原写索引供兜底），实际用 `.or_else(|| (!monitors.is_empty()).then_some(0))`（clippy 更地道）。
- Task 3 `runtime_config` 参数名去掉下划线（spawn 闭包内 4 处引用，加下划线编译错误）。
- Task 4 `prepare_streaming_session` 的 `engine` 参数加 `_` 前缀（streaming 分支不直接用）；cloud 分支提取后 `return` 语义变化，`begin_recording` 调用后显式 `return` 保持原语义。

**desktop worktree 编译验证变通**：desktop crate 在 worktree 无法编译（前端 dist 缺失），变通为在主仓库 `git merge --no-commit --no-ff` 临时合并验证 + `git merge --abort` 回退，每个 task 结束验证 desktop 编译 + 84/84 测试。

**commit 表**：Task 1 `1dd12f9`（paddle-ocr 42 测）/ Task 2 `4c6ce7c`（desktop 84 测）/ Task 3 `75b09a0` / Task 4 `66027bd` / 最终 merge `903a66a` / 后续扩展 `9bc0b53`/`0c0ae93`/`680e31d`/`1c473a4`（postprocess 拆分 + 前端组件拆分 + db_queue 提取 + compute_physical_crop 防御）。

---

## 三、窗口焦点追踪 + 自动粘贴 ⏸️ 暂缓

> 原 plan：`2026-06-26-focus-tracker-todo.md` ｜ 状态：⏸️ **暂缓/下线** ｜ design：`specs/2026-06-26-focus-tracker-design-todo.md`

**目标（未实现）**：三平台（macOS/Windows/Linux）全局焦点追踪，双击剪贴板条目时自动恢复焦点到上一个前台应用并粘贴内容。架构：新增 `focus_tracker.rs` 封装平台 FFI（macOS NSWorkspace 通知 + osascript；Windows SetWinEventHook + enigo；Linux X11 focus event + enigo）。Wayland/权限不足降级为只复制不粘贴。

**暂缓原因**：macOS 自动粘贴方案不可靠（Sublime Text/微信等应用不工作），已回滚为「**双击复制到剪贴板，用户手动 Cmd+V**」。Windows/Linux 焦点追踪不再实施。

**当前替代实现**：双击剪贴板条目 → `paste_clipboard_item` 命令（读 content → write_text → hide 窗口），用户自行粘贴。无 `focus_tracker.rs` 模块。

**后续重启条件**：见 `specs/2026-06-26-focus-tracker-design-todo.md`。除非找到可靠的三平台焦点恢复 + 粘贴模拟方案，否则不重启。

---

## 四、ASR 光标中插/选中替换

> 原 plan：`2026-07-03-asr-cursor-insert.md`（53 步 checkbox 全勾）｜ 状态：✅ 全部合入 main ｜ design：[archived-design §三](../specs/2026-07-05-archived-design.md#三asr-光标定位与中间插入选中替换)

**任务结构**（8 任务 + 多轮追加修复）：Task 1（Transcript 段模型重写 + 全调用点编译适配，**基石，零回归**）→ Task 2（pipeline insertion 标志）/ Task 3（llm 多段润色）/ Task 4（coordinator set_caret + 润色新协议）/ Task 5（result_window insertion）/ Task 6（DB v14 迁移 + CRUD）/ Task 7（前端光标）/ Task 8（e2e）。Task 1 必须一并做「最小编译适配」（旧调用点改新等价方法，不引入新功能），核心数据结构替换无法纯增量，内部步骤连续统一 commit。

**关键 commit**：段模型基石 `c20eb35` / `set_caret` `f2ca142` / 选中替换 `b961f8e` / `append_segment` 对称消费 `9d4a654` / 编辑后光标归末尾 `f32f1a9` / vitest 基建 `e797e0f` / 跨会话方案 C `a79ab97` / Bug C cloud 对称 `1f3e162` / CancelEdit 清 pending_delete `c92955e` / editingRef 同步 `797e7f3`。

**实施关键点**：
- Task 1 临时桥接：`spawn_polish_thread` 改接收 `PolishInput` 但内部把 segments 折回旧 `(preserved, to_polish)`（Task 4 才改 `polish_regions` 多段）。临时桥接在「仅 0/1 个 Edited 段 + 非 edited 连续」时与旧逻辑等价（零回归）。
- Task 2/5 执行顺序调整：Task 5（result_window insertion）紧随 Task 2，让 insertion 链路后端贯通，避免跨任务破坏编译。
- Task 6 DB 迁移：v13→v14 旧三列按 `edited≻polished≻raw` 映射单段；用 Rust + serde_json 构造 segments（纯 SQL 拼 JSON 无法转义换行/控制字符）；保留旧三列 nullable（SQLite 删列需重建表），后续 v15 用 `ALTER TABLE DROP COLUMN`（bundled SQLite ≥3.45）删掉。

**追加特性执行记录**：
- **选中替换（§11）**：`pending_delete` 运行时态；初版仅 `apply_engine_full` 消费漏 `append_segment` → e2e 后 `9d4a654` 补对称（离线/cloud 引擎才生效）。
- **跨会话选中替换（方案 C，§11.8）**：移除 `idle_selection` 长期缓存（三类 bug）改前端 `currentSelectionRef` + 两阶段 Toggle（`prepare-record` + 200ms 看门狗）；Bug C cloud 分支对称植入 selection 种子（`1f3e162`）。
- **前端渲染 4 bug + 性能优化（§12）**：contentEditable 不 reconcile（imperative textContent）、光标滚动错位（scroll 监听）、滚底跟随间隙（onScroll 立即滚底）、换行符（whitespace-pre-wrap）；stickToBottom、rAF 合并渲染、DB 落库节流。
- **vitest 基建（§追加）**：`measureCaretPx`/`codePointOffsetTo`/`codePointOffsetBefore` 抽到 `caret.ts` 纯函数可测；`caret.test.ts` 9→14 测锁住 code-point→UTF-16 offset 对齐；jsdom 未实现 `Range.getBoundingClientRect` 用 `defineProperty` 补零矩形。
- **代码审查多轮（§13-§15）**：最终文本被 diverted 延迟覆盖（clearTimeout）、layout thrashing（measure 推 rAF）、caret 多节点（locateCpOffset）、enterEdit 光标恢复（caretPosRef）、editingRef 同步更新（防 update-result 覆盖编辑态）、拖选三重陷阱（clampRangeToContainer + document mouseup + 坐标边界 + mousedown offset 缓存）。

**e2e 验证**：默认录音逐字末尾追加（零回归）/ 非编辑态闪烁光标 / 点击中间新词从光标处冒出原文本右推 / 显示=落库=复制一致 / mode=2 中插态自动润色 / 编辑态 + 编辑后再劈开 Edited 段 / DB v14 迁移 / emoji code-point 对齐 / 精简态 + 长篇态点击均生效。

---

## 五、记事本移除 + 多 tab CompactEditor + OCR 统一

> 原 plan：`2026-07-03-clean-used-feature.md`（Task 1-14）｜ 状态：✅ 已合 main ｜ design：[archived-design §四](../specs/2026-07-05-archived-design.md#四记事本移除--多-tab-compacteditor--ocr-统一)

**执行顺序**：Task 1→2（OCR 类别后端）→ 3（DB 迁移 v12→v13）→ 4→5（清记事本）→ 6→7（多 tab）→ 8→9（OCR 统一）→ 10（OCR 类别前端）→ 11（文档+全量验证）→ 12-14（e2e 修复 + 审查追加）。

**Task 12 e2e 阶段关键修复**（plan 独有的踩坑）：
- **with_db 重入死锁**：`insert_ocr_clipboard_item` 把 `current_ocr_meta()` 移出 `with_db` 闭包（`std::Mutex` 非递归，闭包内调 `current_ocr_meta`→`load_config_key`→`with_db` = 同线程重入死锁；症状：async `await` 卡住不报错 + DB 查询全阻塞 + 应用不全僵死）。
- **CompactEditor 保存同步**：`set_clipboard_item_text` 成功后 `emit("clipboard://changed")`，编辑器是独立窗口，剪贴板列表窗口靠此事件感知刷新，FTS5 经 `clip_fts_au AFTER UPDATE OF search_text` 触发器自动同步。
- **超长图 OCR 切分**：`recognize_long_image`/`plan_chunks`（`SPLIT_HEIGHT_THRESHOLD=1600`/`CHUNK_HEIGHT=1280`/`CHUNK_OVERLAP=200`），解决 2032×15796 长图 det 等比缩放后短边过小 text_len=0。
- **OCR 全局并发互斥**：`OcrLockGuard`（`OCR_BUSY: AtomicBool` + `compare_exchange` RAII）在 `ocr_image`/`ocr_screenshot` 入口 `try_acquire`，忙则 `Err("前一个 OCR 还未完成，请稍后")`；前端 4 入口 catch 给可见提示。

**OCR 僵死归因（技术债，用户定调不深究）**：e2e 期间 OCR 曾僵死，多轮归因（建窗 worker 线程 / 并发首次加载 / MNN C++ 包）均被独立进程 smoke test 证伪，真因未坐实；当前版本（DCL + with_db 修 + emit + 互斥）稳定。`INIT_LOCK` DCL 保留为无害串行化优化。

**Task 13/14 CompactEditor 审查修复**：replaceOne 焦点跳转（替换后重新 collectMatches）、replaceAll 大小写（RegExp gi）、mount 监听泄露（cancelled 标志）、keydown 监听器每键重建（doSaveRef）、键盘 undo/redo（execCommand 统一）。

---

## 六、图片查看器性能优化

> 原 plan：`2026-07-03-image-viewer-perf.md` ｜ 状态：✅ 已合 main（实现路线演进）｜ design：[archived-design §五](../specs/2026-07-05-archived-design.md#五图片查看器)

**3 个 Task**：双 canvas 分层 + draw 拆分（drawBg/drawActive）→ createImageBitmap 异步预缩放 → 先 thumb 再 full 渐进加载 + fit-to-window。

**⚠️ 架构演进（plan 独有的迭代记录）**：原设计双 canvas（bgCanvas + drawCanvas）。实施后用户测 2032×15796 超长图仍卡顿，经历三轮迭代：① RAF 节流 + 跳过无变化 canvas 尺寸重设（`203be9d`，稍好仍慢）→ ② pen 增量线段 + shape 脏区域重绘（`6379614`，drawCanvas GPU 合成 45M 像素本身有固定成本）→ ③ **canvas + SVG overlay**（`237713a`，最终方案，标注完全不参与 canvas 操作）。后续又叠加视口渲染 v2（`9bca0de`+`a9faa39`，canvas 恒定窗口大小，GPU 合成 174MB→~8MB 降 20×）。（⚠️ 此方案后续曾偏离走偏——canvas 被移入 wrapper、物理变 `dispW*dpr×dispH*dpr` 致长图超 32767 崩；2026-07-07 修复恢复视口固定、改 content 内 sticky 实现 + `viewportMath.ts` 纯函数单测，详见 [archived-design §5.2 演进订正](../specs/2026-07-05-archived-design.md)（「视口渲染 v2（最终方案）」节末））

**Task 3 race condition 修复**：plan 原用 `if (imgRef.current?.src === thumbDataUrl) return` 防重复，快速切图时旧 promise onload 仍覆盖新图 → effect 内 `let cancelled = false` + cleanup，thumb/full 的 `.then`/`onload` 均检查。thumb→full 同尺寸未缩放时 `setNatW/H`+`setZoomSync` 无状态变化 → drawBg useCallback 不重建 → canvas 保留已 close 旧 bitmap → full onload 末尾补 `drawBg()` 显式重绘。

**fit padding 修正**：plan 原写 `-24`，实际容器 `p-12`（48px×2=96px），改 `FIT_PADDING=96`。

**dist 不提交**：plan 原写「dist 已纳入 git」有误，实际 `.gitignore` 排除 `/crates/desktop/dist/`，只提交源文件。

**后续追加**（同分支累积）：自适应宽度按钮 + ResizeObserver（`bdfa8fa`→`ad65e02`）、灯箱暗场重构（`2dc2ccf`）、序号/马赛克/菱形工具、实心填充、redo、属性浮窗交互、图标统一截图 SVG 风格（多个 commit）。

**多轮代码审查修复**：无标注复制失效、textarea Esc 关窗、scroll 触发全组件重渲染（删 scrollPos state RAF 直接调 drawBg）、createImageBitmap 卸载泄漏、文本折行硬编码、thumb→full 尺寸跳变、createImageBitmap debounce、Screenshot 文本 textWidth、composePngBytes 防御、缩略图竞态降级（fullLoadedRef）等。

---

## 七、剪贴板管理整合

> 原 plan：`2026-07-04-clipboard-consolidation.md` ｜ 状态：✅ 完成（`668300d` z-sync 回写，已合 main）｜ 分支 `image-viewer-perf`

**需求**：废弃独立识别记录管理，整合到剪贴板管理。所有文本截断 200 字。加链接检测/复制图标/级联删除。

**7 Task 全完成**：① 文本截断 200 字（浮窗 + 管理页 text/ocr/asr）② 左侧导航改名「剪贴板」→「剪贴管理」+ 废弃识别记录管理 ③ 剪贴板条目元信息（浮窗语音条目显时间戳、管理页显「时间戳·引擎」）④ 语音条目可编辑（CompactEditor 已支持）⑤ 级联删除 transcriptions ⑥ 链接检测 + 打开（tauri-plugin-opener）⑦ 复制图标合并到类型图标 + 列表项重设计（frontend-design）。

**commit 表**：截断+改名+废弃 `0dc7c97` / 级联删除 `b968aef` / 链接检测+复制图标 `71cc941` / 浮窗时间戳 `8e1fa38` / 管理页元数据顺序 `694f185` / 链接 openUrl 修复（Rust 插件+ACL）`d0a2935` / 复制图标常显 `7af2cb2` / 列表项重设计 `a01cee4`。

---

## 八、图片查看器（OCR 文本块 + 统一查看器）

> 合并 2 篇 plan（均分支 `image-viewer-perf`，✅ 已合 main）｜ design：[archived-design §五 5.5-5.6](../specs/2026-07-05-archived-design.md#五图片查看器)

### 8.1 OCR 文本块可视化（原 `2026-07-04-ocr-text-blocks.md`，状态 ✅ 全部完成）

**4 Task**：① 后端 engine `recognize_with_blocks` 返回带坐标文本块（`recognize_long_image_with_blocks` 坐标 offset 合并 + 末行去重）② `ocr_image` 返回结构化 `OcrResult{text,blocks}` ③ 前端 SVG 叠加层 + 按钮三态 toggle（overlay/mask/off）+ 双击复制 ④ 文档同步。

**commit 表**：engine `284e797` / 命令 `bd6c3a6` / 前端叠加层+双态 `84417d1` / 三态 toggle `f085264` / 遮罩两遍渲染 `81b7dbe` / 双击复制 `e356071` / 截图关窗→开预览 `b5a9975`（废弃全屏窗叠加 `9487af5`）/ 三态图标 `bbdfe83` / 图标切换修复 `0e82607`。

**架构决策**：截图 OCR 不在全屏透明窗叠加（信息过载 + 遮挡其他窗口），改为关截图窗 → 开图片预览展示叠加层（图片预览是完善的 OCR 叠加载体）。

### 8.2 统一内容查看器（原 `2026-07-04-unified-viewer.md`，状态 ✅ Task 1-6 全完成）

**6 Task**：① 后端窗口尺寸 880×620 + 记忆 + `get_transcription_text` ② 前端 Tab 模型升级（source/itemType/图片≤5/hidden 挂载）③ ImagePreview 改可控组件（props imageId）④ 入口统一（ocr_screenshot + ClipboardItem 预览 → openCompactEditorTab）⑤ 废弃 ImagePreview 窗口（命令注册/ACL/activation 移除）⑥ 文档同步。

**commit 表**：后端 `c1c4d8f` / Tab 模型 `776bf74` / ImagePreview 组件 `684e156`+`1e9288a` / 入口统一 `3286885`+`83a6548` / 截图 OCR 防重复 `bf8fc92` / 工具栏黑边 `11ff198`+`e10f846` / 废弃窗口 `1928e62` / 窗口记忆 `3d27e53`+`0e9a042`+`c4eca38` / 语音管理查看入口 + 截图 OCR tab 顺序 `0266534`。

**架构决策**：图片 tab 背景从暗灯箱 `#18181b` 改为 `bg-background`（与文本 tab 白底统一）；ImagePreview 不再用 fixed/Esc/暗区关闭；剪贴板图片条目删独立 OCR 按钮（统一在图片预览 tab 工具栏）。

**后续优化（z-sync 补记）**：识别记录文本截断 200 字（`dde0529`）；管理页全选 header sticky（`41c35b1`）；VAD 驱动波纹（`PipelineEvent::Speaking(bool)` + 前端 200ms 防抖 `b8fe71f`+`fd71550`+`7d0fdb6`+`3257ddc`+`9d6071d`）。**Tauri bool 事件 payload 被 wrap 教训**：后端 `emit("event", true)` → WKWebView 序列化多包一层 `{event,payload,id}`，前端 `e.payload` 拿到整个 Event 对象非裸 bool → 防御性提取 `typeof payload === "boolean" ? payload : payload?.payload ?? false`。

**代码审查修复（第二~五轮）**：CompactEditor 查找模式打字光标拽回（`54c508f`）、查找匹配 debounce（`56af793`）、CancelEdit 清 pending_delete（`c92955e`）、多屏不同缩放率穿透失效（`2f4690b`）、查找跳转 soft wrap 滚动偏差（`0b8c622`）、查找跳转读旧 matches + debounce 循环（`8de65fd`）、查找栏输入抢焦点/Enter 删匹配/失焦无高亮（`509b460`）。

---

## 九、2026-07-05 代码审查修复

> 合并 4 篇 plan（均 2026-07-05，✅ 已完成）｜ design：[archived-design §七](../specs/2026-07-05-archived-design.md#七2026-07-05-代码审查批次)

### 9.1 P0（原 `2026-07-05-code-review-fix-p0.md`，7 Task，8 commit `2394e34..381d75c`，257 passed）

**修复**：asr-local mel filterbank bug（抽 feature.rs）+ whisper Box::leak（全局 Lazy）+ whisper 归一化/audio unwrap/moonshine 下溢（Task A1-A3）；download 多段 200 截断（Task E1）；dlp stderr JSON 首行 + tempfile 死依赖（Task E2）；dlp 下载超时 + 200MB 限制（Task E3）；desktop AppKit 主线程 UB（`run_on_main_thread`）+ CloudStreaming 看门狗（Task F1/F2）。

**实施偏差**：① Task A1 范围收窄——`compute_fbank` 未统一抽取（fbank.rs 无 DC removal/pre-emphasis 与 paraformer 不同算法），feature.rs 只统一 mel_filterbank + apply_lfr + window + hz_to_mel/mel_to_hz；`apply_lfr` 公式保持原始（plan 新公式会改 streaming 行为）；fbank.rs 用 `high_freq=8000`（Nyquist）、paraformer 用 `-400`。② Task E2 元数据 JSON 前的 `eprintln!` 改 `println!`（stdout），比加 `[log]` 前缀更简洁。③ subagent 工具只读，切 inline execution。④ server `ws_stream_session` 测试 base commit 即失败（pre-existing）。

**移至 P2**：I-E1（download .part 清理，保留续传策略）、I-E2（download 416 死代码删除）。

### 9.2 P1（原 `2026-07-05-code-review-fix-p1.md`，13 Task，12 commit `7ae031c..87a49a6`，256+ passed）

**修复**：infra `net.rs` 统一超时常量（Task C0）；asr-cloud 4 provider WS 全链路超时（C1）；llm 共享 Client + 超时（C2）；desktop gRPC connect 移入 timeout（C3）；server spawn_blocking + 引擎锁（D1）+ 默认 127.0.0.1 + body limit + serde_json 转义 + graceful shutdown（D2）；引入 parking_lot + 全 crate 锁迁移（B1-B5）；desktop unreachable! 降级 + 启动 expect 降级（F3）；streaming_paraformer raw_samples drain（A4）。

**实施偏差**：① B4 qwen3 三锁——parking_lot 无毒化后同时持三锁无级联风险，跳过分阶段加锁。② B5 desktop coordinator `if let Ok(tx) = self.tx.lock()`（11 处）需手动改 `let tx = self.tx.lock()`（parking_lot 返回 guard 非 Result），Python 正则批量修。③ D2 C9——API token 校验跳过（绑定 localhost 已足够），CORS 改 `CorsLayer::new()`（空层=同源）。④ F3 只做 main.rs config fallback + coordinator unreachable! 降级，tray/clipboard expect 待 P2。

**⚠️ D1 已被 arch-fixes（2026-07-06）取代**：`inference_lock` 方案后经 `AsrEngineManager::get_engine`（只读 Arc 不改 active）+ `new_with_capacity(8)` 取代——同模型并发受引擎内 `Mutex<Session>` 串行化、跨模型天然并行，不再需要全局 `inference_lock`；仅 `spawn_blocking` 仍生效。

**移至 P2/follow-up**：I-F2 截图门控（AtomicBool 待 P2）、I-F3 tray/clipboard expect（P2）、save_app_config 事务（I-D4 P2）、DB WAL/busy_timeout（P2）。

### 9.3 P2 + Follow-up（原 `2026-07-05-code-review-fix-p2.md`，✅ 完成）

**4 Task**：G1 删死代码 + 死依赖（infra/image_util.rs 全文件、desktop/download/capx/asr-cloud 死代码、dlp tempfile/llm serde_yaml）；G2 capx 跨平台编译（测试块加 `#[cfg(target_os="macos")]`）；G3 清理生产路径调试输出（eprintln→log::debug!、console.log 删除、aliyun info!→debug!）；G4 clippy 全量修复（auto-fix 118 → 手动 70 → 清零 + 各 crate 加 `#![warn(clippy::all)]`）。

**G1 偏差**：Pipeline trait 删除需同步删 impl 块 + `VadSegmentedPipeline` 的 finish/reset 补为 inherent 方法；feature-gated 死代码（`current_partial`/`is_cloud`/`reset`）保留加 `#[allow(dead_code)]`；downloader.rs 416 分支注释有误（416 被 classify_status 归 Fatal 早已 return），重构为 `if 200 → seg.begin else → start`。

**G4 偏差**：clippy 总数原「118」实际 auto-fix 后剩 70 手动；CIF 热路径 `streaming_paraformer.rs` 的 `run_cif`/`run_cif_final` `needless_range_loop` 加函数级 `#[allow]`（改 iterator 可读性下降）；`too_many_arguments`（4 处内部函数）+ `type_complexity`（2 处 KV cache 4-tuple/DB 10-tuple）加 `#[allow]`。

**P2-Final 偏差**：server `ws_stream_session_feed_partial_then_empty_finish_final` 失败（512 静音样本有 VAD 模型时被门控）→ follow-up 加 `StreamingRunner::new_no_vad`（vad=None 跳过门控）server 测试用此验证纯 relay。

**Follow-up（Important + Minor + FTS5 精选）**：I-H1 save_app_config 事务、I-H2 DB WAL + busy_timeout、I-H3 FTS5 搜索切换（见第十节）、I-F2 截图 AtomicBool CAS 门控 + BusyGuard、I-F3 create_tray 返回 Result + 11 处 expect→map_err?、M-4 collect_rows helper、M-5 baidu Close 空结果发 Failed、M-6 capx from_raw 降级、M-7 JoinHandle 丢弃**评估不修**；Bug B 跨会话选中替换方案 C（移除 idle_selection 改前端推选区两阶段 Toggle）；从右往左拖选三重陷阱修复（见 [§四](#四asr-光标中插选中替换) / design §15）。

### 9.4 Rust-Patterns 专项（原 `2026-07-05-rust-patterns-review.md`，✅ 完成）

**5 Task + 1 评估**：P1-1 Mutex lock().unwrap() → unwrap_or_else(|e| e.into_inner())（cli/main.rs 10 处 + downloader.rs）；P1-2 HeaderValue parse unwrap → map_err?（settings_commands.rs:449）；P1-3 ndarray as_slice().unwrap() → ok_or_else（streaming_paraformer.rs 3 处）；P2-1 14 个零调用 pub fn 收窄 pub(crate)（clipboard 3 + download 8 + llm 3）；P2-2/P2-3 评估无需改动（paste sleep 在 spawn_blocking 内、cloud block_on 需架构级重构）。工作目录 `.worktrees/rust-review`。

---

## 十、DB 合并 + FTS5

> 合并 2 篇 plan（均 2026-07-05，✅ 已实现，DB 现行 v18）｜ design：[archived-design §八](../specs/2026-07-05-archived-design.md#八db-表合并--fts5-搜索)

### 10.1 DB 表合并（原 `2026-07-05-db-merge.md`，5 Task，✅ 完成）

**目标**：clipboard_history 吞并 transcriptions，精简为 content + ref_data + meta_info 三层。DROP + CREATE 不迁移历史数据（用户确认可丢弃）。v16→v17。

**实施记录**：
- Task 3 实际修改超出原计划：coordinator.rs `insert_asr_item` 改 4 参数签名删 AsrMeta 构造；clipboard_commands.rs 删 cascade_delete_transcriptions + 3 处调用、write_item_to_clipboard 改 ref_data、current_ocr_meta 返回元组、insert_ocr_clipboard_item 适配；settings_commands.rs `delete_by_transcription_ids`→`delete_items`；screenshot_commands.rs 4 处 NewClipboardItem 适配；compact_editor_commands.rs get_transcription_text 改读 clipboard_history；watcher.rs 删无用变量。**编译 31 错→0**，clipboard 11/11 + compact_editor 1/1 pass。
- Task 4 前端：`types/clipboard.ts` 完全重写（删 5 旧类型改统一 MetaInfo）；`formatFilePaths` 签名 `(content,count?)`→`(refData?)`；`image_meta.size`（number bytes）→`meta_info.size`（string）删 formatSize。**tsc+vite 全通过**。未改动 FilterTabs（"asr" 由后端 build_where 映射）/ CompactEditor（source 是 tab 标识非 DB 列）。
- **后续简化**：迁移落地后因无 v<17 旧库，`init_schema` 移除历史迁移链 + DROP 兜底——`user_version >= 17` 跳过，其他跑 db.sql 一次性到 v17，`ensure_db` 不再 loop。
- **遗留（非阻塞）**：`octopus_asr_local::db` 和 `octopus_infra::db` 仍有 transcriptions 表 CRUD 函数（`list_transcriptions`/`delete_transcriptions`/`finalize_transcription`），运行时调用会失败（表已 DROP），编译不报错（SQL 字符串不检查），彻底清理属更大范围重构。
- **Task 4 后续 UI 迭代**：统一元数据行 `metaParts()`、类型色编码 typeAccent、image `WxH·size` 移 content 区 `imageMeta()`、MetaInfo `skip_serializing_if` 跳 None、`engine_mode`→`asr_mode` 重命名。

### 10.2 FTS5 搜索切换（原 plan 在 P2 follow-up 内 + design `2026-07-05-fts5-search-design.md`，✅ 完成，v17→v18）

**目标**：voice 历史搜索走 FTS5 MATCH（>=3 字符），<3 字符回退 LIKE；历史行 backfill。

**实施**：v17→v18 backfill（`INSERT INTO clipboard_history_fts(rowid,content) SELECT id,content FROM clipboard_history WHERE content!='' AND id NOT IN (SELECT rowid FROM clipboard_history_fts)`，幂等）；`list_transcriptions_search_at` 按字符数分流（>=3 走 `clipboard_history_fts MATCH ?1` 子查询，<3 回退 LIKE）；`escape_fts5_match` 双引号包裹 phrase 内部双引号双写。6 个新测试全过。

**验证**：`cargo test -p octopus-infra -- fts5` / `-- list_transcriptions` / `cargo clippy -p octopus-infra` 全绿。

---

> **归档边界说明**：本文件覆盖至 2026-07-05 的实施 plan。2026-07-06 起的活跃 plan（scroll-stitch 系列、clipboard 系列、streaming-session-manager、vendor-paddle-ocr、clipboard-keyboard-nav 等）仍在 `plans/` 独立维护。配套设计层面内容见 [`specs/2026-07-05-archived-design.md`](../specs/2026-07-05-archived-design.md)。
