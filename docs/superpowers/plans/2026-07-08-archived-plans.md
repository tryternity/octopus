# 归档实施计划（archived-plans，第二批）

> 归档日期：2026-07-08（实际整理 2026-07-12）
> 范围：2026-07-05 ~ 2026-07-08 的 15 篇实施 plan，按主题同类合并。
> 上批归档：[`2026-07-05-archived-plans.md`](./2026-07-05-archived-plans.md)（覆盖至 2026-07-05，06-12~07-05 的 14 篇）。
> 配套设计：各篇对应 `specs/2026-07-0X-*.md`（架构决策/数据结构/接口的权威来源）。

## 归档原则

- **plan = 实施执行记录**：本归档提炼每篇 plan 的目标 / 状态+commit / 关键偏差与教训（plan 独有、design/architecture/feature 未覆盖的部分），按主题分组。逐步骤的代码细节已被代码本身取代，不再赘述。
- **现行 vs 下线**：15 篇均已落地、对应功能现行存活（已对照代码核实：`detectUrl`/`clear_history_by_filter`/`fileMeta` 等符号均在；仅个别 checkbox 因「依赖真实复现/真机回归」未勾，代码+单测已落地）。

## 目录

- [一、剪贴板浮窗交互（4 篇合并）](#一剪贴板浮窗交互)
  - 1.1 无协议域名链接识别（原 `2026-07-06-clipboard-domain-link-detection.md`）
  - 1.2 历史条目两行布局（原 `2026-07-06-clipboard-row-layout.md`）
  - 1.3 浮窗键盘导航（原 `2026-07-07-clipboard-keyboard-nav.md`）
  - 1.4 一键清理（原 `2026-07-08-clipboard-clear-all.md`）
- [二、滚动截图拼接（4 篇合并）](#二滚动截图拼接)
  - 2.1 借鉴改造 A+B（原 `2026-07-06-scroll-stitch-borrow-A-B.md`）
  - 2.2 F 配置外置 + D 两阶段 refine（原 `2026-07-06-scroll-stitch-config-downsample.md`）
  - 2.3 内容突变鲁棒性（原 `2026-07-06-scroll-stitch-transition-robustness.md`）
  - 2.4 截帧性能优化（原 `2026-07-06-scrolling-screenshot-performance-optimization-plan.md`）
- [三、跨平台贴图窗口](#三跨平台贴图窗口)（原 `2026-07-06-cross-platform-pin-window-design-plan.md`）
- [四、流式 ASR 引擎复用 StreamingSessionManager](#四流式-asr-引擎复用)（原 `2026-07-06-streaming-session-manager.md`）
- [五、vendor paddle-ocr-rs](#五vendor-paddle-ocr-rs)（原 `2026-07-06-vendor-paddle-ocr.md`）
- [六、VadSegmented LSTM 跨段漂移修复](#六vadsegmented-lstm-跨段漂移修复)（原 `2026-07-07-vad-segmented-lstm-drift-fix.md`）
- [七、AI 命令面板 action bar](#七ai-命令面板-action-bar)（原 `2026-07-08-action-bar.md`）
- [八、系统状态页 + model_probe 依赖反转](#八系统状态页--model_probe-依赖反转)（原 `2026-07-08-system-status-page.md`）

---

## 一、剪贴板浮窗交互

### 1.1 无协议域名链接识别

> 原 plan：`2026-07-06-clipboard-domain-link-detection.md` ｜ 状态：✅ 已合 main ｜ design：[`specs/2026-07-06-clipboard-domain-link-detection-design.md`](../specs/2026-07-06-clipboard-domain-link-detection-design.md) ｜ feature：[clipboard.md](../../features/clipboard.md) §12

**目标**：剪贴板文本条目无协议时也识别为可点击链接——常用域名后缀（补 `https://`）与 localhost/IPv4+端口（补 `http://`），消除两处重复内联正则。

**已落地**：`types/clipboard.ts::detectUrl(raw)`（纯函数 + 34 单测），`ClipboardItem.tsx` / `ClipboardPanel.tsx` 两处消费点替换原 `/^https?:\/\//` 内联判定。

**关键约束（plan 独有）**：
- 路径 A（常用后缀 `.com/.cn/.com.cn/.net/.org`）补 `https://`；路径 B（localhost/IPv4 + **必带端口** 1–65535）补 `http://`。后缀自带前导 `.` 做 dot 对齐防子串误命中（`foocom ≠ .com`）。
- **句中片段不识别**：含空白直接判非链接（`看这个 github.com/foo` 不识别）；括号致 label 非法也拒（`（github.com/foo）`）。
- 纯 IP/localhost **无端口**不识别（`127.0.0.1` / `localhost` 不识别，需 `:port`）——避免把 IP 字面量当链接。

### 1.2 历史条目两行布局

> 原 plan：`2026-07-06-clipboard-row-layout.md` ｜ 状态：✅ 已合 main（`c454db1`）｜ design：[`specs/2026-07-06-clipboard-row-layout-design.md`](../specs/2026-07-06-clipboard-row-layout-design.md) ｜ feature：[clipboard.md](../../features/clipboard.md) §12

**目标**：剪贴板浮窗历史行从「单行 + 右侧 hover 操作占位空白」改为两行——第一行铺满内容 + 行尾元数据，第二行时间戳 + 操作按钮；「复制」操作置顶。

**关键决策（plan 独有，与 [[clipboard-timestamp-below-content]] 一致）**：
- **时间戳固定在第二行（内容下）**，不在内容上方——时间戳在上会视觉归属到上一条（2026-07-07 实测否决）。
- 类型图标提为跨两行垂直居中的「头像」列，兼单击复制入口（`w-5 h-5`，copied 时 `scale-125 text-emerald-500` + 「已复制」气泡）。
- 操作组顺序：复制 → 打开链接 → 编辑/预览/保存/打开文件 → 删除 → 收藏（复制居首）。
- 新增 `fileMeta(item)` helper（file 类型显「类型」或「N个 · 类型」）。
- 浮窗与设置页管理页 `ClipboardRow` 两处消费点同步重构。

### 1.3 浮窗键盘导航

> 原 plan：`2026-07-07-clipboard-keyboard-nav.md`（5 Task + 7 项验收）｜ 状态：✅ 已合 main ｜ design：[`specs/2026-07-07-clipboard-keyboard-nav-design.md`](../specs/2026-07-07-clipboard-keyboard-nav-design.md) ｜ feature：[clipboard.md](../../features/clipboard.md) §12

**目标**：剪贴板浮窗脱离鼠标可用，对齐 Wox/Raycast 的搜索框持焦 + 方向键导航范式。

**已落地**：
- `lib/clipboardNav.ts` 纯函数：`moveIndex`（列表选中移动，边界夹紧不循环）/ `moveTab`（tab 循环切换）+ 单测。
- `Clipboard/index.tsx`：`selectedIndex` 索引驱动 + window 级 keydown handler（7 分支）+ 选中行 `scrollIntoView`。
- 行加 `data-clip-index`、tab 加 `data-tab-index` 供 DOM 定位。

**关键约束（plan 独有）**：
- **TABS 顺序**：all(1) / favorite(2) / asr(3) / text(4) / ocr(5) / image(6) / file(7)——favorite 提到第 2（main `254a4a2`）。
- `←→` 仅搜索框**为空**时切 tab（有内容让出给光标移动）；`Tab/Shift+Tab` 无论搜索框是否有内容都切 tab；`Cmd+1..7` 直接跳 tab。
- `Esc`：有搜索内容清空、已空隐藏浮窗。
- **闭包陷阱**：window keydown handler 用 `itemsRef`/`selectedIndexRef`/`searchRef`/`filterRef` 存最新值，避免 handler 注册时闭包过期。
- `selectedIndex` 索引为第一性 citizen（非 `selectedId`）；items 变化（过滤/搜索/刷新）时 useEffect 夹紧越界索引。

### 1.4 一键清理

> 原 plan：`2026-07-08-clipboard-clear-all.md`（3 Task）｜ 状态：✅ 已合 main ｜ design：[`specs/2026-07-08-clipboard-clear-all-design.md`](../specs/2026-07-08-clipboard-clear-all-design.md) ｜ feature：[clipboard.md](../../features/clipboard.md) §12

**目标**：剪贴板浮窗底栏增「清理」按钮，一键删除当前 tab 类别下所有非收藏条目，两步 inline 确认防误触。

**已落地**：`store::clear_history_by_filter(conn, filter, keep_favorite)` + 5 单测；Tauri 命令 `clear_clipboard_history_by_filter`（emit `clipboard://changed`）；前端两步状态机。

**关键设计（plan 独有）**：
- **「查询看到的 = 清理删除的」语义一致**：复用现有 `build_where`（filter→SQL 单一权威）拼 WHERE + `AND is_favorite = 0`，与 `clear_history` 对称（含 `cleanup_unreferenced_images`）。
- **收藏 tab 自然删 0 条**：`filter="favorite"` + `keep_favorite=true` → `is_favorite = 1 AND is_favorite = 0` 恒假，后端无需特判，前端 `disabled` 按钮即可。
- **两步 inline 确认**：点 1 次 → `confirming=true`（变红「再点确认」+ 3s 超时回退），再点才执行。filter 切换/卸载清 timer（避免 A tab 点了第一步、切 B tab 后第二次点击误清 B）。
- **与搜索框正交**：清理删整个 tab 类别非收藏，与搜索词无关。

---

## 二、滚动截图拼接

> 4 篇均改 `crates/capx/src/stitch.rs` / `screenshot_commands.rs`，`Stitcher` 公共接口零变更。算法层全貌见 [screenshot.md](../../features/screenshot.md) §4-§5（已是权威来源，本节仅记 plan 独有的执行偏差）。

### 2.1 借鉴改造 A+B

> 原 plan：`2026-07-06-scroll-stitch-borrow-A-B.md` ｜ design：[`specs/2026-07-06-scroll-stitch-borrow-A-B-design.md`](../specs/2026-07-06-scroll-stitch-borrow-A-B-design.md)

**A（队列解耦）✅ 已合 main（`574f53b`+`107278c`）**：`start_scroll_recording` 把 capture/process 拆成生产-消费两 task + `tokio::sync::watch` 通道（丢旧保新），快速滚动不卡顿、不丢大段。

**B（NCC 主次比硬过滤）⚠️ 已回退（`13b450d`）**：主次比对 NCC 连续 response 不成立（response 相邻 y 高度相关，无独立「次峰」概念），代码已从 main 移除。突变鲁棒性改由方向 1（相邻帧参考 fallback，见 2.3）解决。

### 2.2 F 配置外置 + D 两阶段 refine

> 原 plan：`2026-07-06-scroll-stitch-config-downsample.md` ｜ 状态：✅ Task 1-4 全完成（`e53b5fe`+`8053665`+`f1477be`，合 main `133ea22`）｜ design：[`specs/2026-07-06-scroll-stitch-config-downsample-design.md`](../specs/2026-07-06-scroll-stitch-config-downsample-design.md)

- **F 字段化**：`STRIP_H`/`MAX_SCROLL`/`NCC_SCORE_THRESHOLD` 由 const 纳入 `StitchConfig`（+`ncc_downsample_width`），默认值不变行为零变化。
- **D 大屏两阶段 refine**：帧宽 > `ncc_downsample_width`(1920) 时 Triangle 降采样域粗定位 dy → 原分辨率 ±2px 邻域 `ncc_match_range` refine 恢复亚像素（降采样 Nearest 锯齿会破坏 response 峰值，必须 Triangle）。`primary_ncc` + `PrimaryOutcome`（Matched/Mismatch/SizeError）封装验证 + 失配语义。

### 2.3 内容突变鲁棒性（方向 1 相邻帧参考）

> 原 plan：`2026-07-06-scroll-stitch-transition-robustness.md` ｜ 状态：✅ Task 1-5 全完成（`7cb9bb6` 合 main）｜ design：[`specs/2026-07-06-scroll-stitch-transition-robustness-design.md`](../specs/2026-07-06-scroll-stitch-transition-robustness-design.md)

**目标**：给 `Stitcher` 加相邻帧参考 fallback——突变帧（如「文字→图片」）主 NCC 失配时，用**前一帧**有效区匹配当前帧求正确 dy，消除 best-guess 盲 append 污染画布 + 熔断永久卡死。

**关键设计（plan 独有）**：`+prev_gray` 字段（每帧 `process_frame` 末尾统一更新，避免散落 8 个 return 点）+ 提取 `process_frame_inner` + `try_match_prev_frame` 方法。`try_fallback` 在 1D 投影降级前插入相邻帧层。前一帧与当前帧只差一个 dy、突变边界是两帧共同特征、重叠最大 → 能求出正确 dy。

### 2.4 截帧性能优化

> 原 plan：`2026-07-06-scrolling-screenshot-performance-optimization-plan.md` ｜ 状态：✅ 已完成

Windows/Linux 截帧热路径性能：新增 `capture::capture_single_monitor(mon_x, mon_y)`（仅捕获指定显示器，找不到降级主屏）+ `crop_region_rgba_direct(rgba_bytes: &[u8])`（只读 Slice 行优先越界安全直接裁剪，避免裁剪前复制全屏大图，**内存拷贝降 98%+**）；热路径 `log::info!` 降 `debug!` 移除高频 I/O。消除 30ms 滚屏截图在 Windows/Linux 的多屏捕获耗时 + 33MB 双重拷贝。

---

## 三、跨平台贴图窗口

> 原 plan：`2026-07-06-cross-platform-pin-window-design-plan.md` ｜ 状态：✅ 全部完成 ｜ feature：[screenshot.md](../../features/screenshot.md) §8（三平台 PinWindow trait 实现表已是权威来源）

**目标**：`pin_window.rs` + `screenshot_commands.rs` 实现 Windows (Win32) / Linux (GTK3) 原生贴图，对齐已有 macOS 实现。

**关键踩坑（plan 独有，代码已修）**：
- **macOS 右键退出 Segfault**：历史代码 `std::thread::spawn` 后台线程访问 `NSWindow::isVisible` 违反 Cocoa 主线程限制 + `isReleasedWhenClosed=YES` 致 `Retained` 悬空。修：`setReleasedWhenClosed(false)` + `cleanup` 经 `performSelector:withObject:afterDelay:` 主线程延迟 0.1s 调度。
- **带标注/马赛克贴图**：原 `pin_screenshot` 从后端 `ALL_CAPTURES` 裁原始截图（不含 Canvas 涂鸦）。改前端 `composeAndCropBytes()` 合成完整 Buffer → `FileReader.readAsDataURL` 转 base64（替代 8192 分块 + apply，避免大图内存/堆栈风险）→ 后端解码。`isPinningRef` 防重复点击锁。
- **Windows GDI 资源泄漏**：`run_gdi_calls` 局部闭包模式杜绝 HDC/HBITMAP 中间泄漏；修正 `GetMessageW` 把 `-1` 错误返值当 true 的缺陷；`WM_CREATE` lparam 空指针防御。
- **Linux GTK 内存泄漏**：重构菜单事件闭包切断 `win_menu` 循环引用。
- **交叉编译**：macOS 宿主缺 MSVC 链 + Linux GTK/GDK sysroot，交叉编译在 C 库层报错；Rust 绑定已人工对齐，待原生宿主/Docker CI 完整编译。

---

## 四、流式 ASR 引擎复用

> 原 plan：`2026-07-06-streaming-session-manager.md`（4 Task + 5 轮审查修复）｜ 状态：✅ 全部合 main（`d2964f0..237df45`）｜ design：[`specs/2026-07-06-streaming-session-manager-design.md`](../specs/2026-07-06-streaming-session-manager-design.md) ｜ feature：[asr-engine.md](../../features/asr-engine.md)

**目标**：给流式 ASR 引擎补对齐离线 `AsrEngineManager` 的 `StreamingSessionManager`，desktop 录音复用常驻引擎（reset 而非 new），消除每次录音秒级重载。

**关键设计（plan 独有）**：
- **靠 reset 复用、非并发共享**：ort `Session::run` 是 `&mut`，Session 本就不能跨连接并发；流式 `StreamingSession` 又有连接级状态（punct_prefix/decoder_caches…），故 reset 复用而非并发共享。
- **`StreamingRunner.engine` 由 `Box` 改 `Arc`**：让 pipeline drop 时仅释放 Arc clone、manager 原 Arc 仍持有 → 引擎不销毁、下次复用。连带 `WsStreamSession` 同步改 Arc（编译要求）。
- 仅 desktop 接入；server（每连接独立状态、连接结束即 drop）与 cloud（独立路径）不动。

**审查修复（2026-07-09，`d6c2d71`）**：
- **max_cache=2 驱逐**：原「不设上限」假设未覆盖用户配置多流式引擎反复切换（每个 Session 数百 MB、无上限致 OOM）。`set_active` 入缓存前淘汰非 active + `probe(Unload)`。
- **model_probe 接入**：`switch_model` 加 `probe(Before/After)`，状态页统计流式引擎内存。
- **paraformer drain 停滞 bug**：e2e 暴露 `raw_samples` drain 与绝对帧索引 `fi*SHIFT` 不兼容，`237df45` 移除 drain + 新增 fbank 持续增长回归测试。

**server per-connection 维持现状（复查确认不改）**：server 是桌面端辅助 sidecar、非大并发（连接数个位数、结束即 drop），共享 ort Session 需拆动静字段而 ort 推理持 `&mut` 串行无并发收益——正是 spec §9 YAGNI 项。

---

## 五、vendor paddle-ocr-rs

> 原 plan：`2026-07-06-vendor-paddle-ocr.md`（11 Task）｜ 状态：✅ 全部完成 ｜ feature：[ocr.md](../../features/ocr.md) §7（推理后端迁移已是权威来源）

**目标**：从 ocr-rs（MNN C++ 推理）迁移到 vendored paddle-ocr-rs（ONNX Runtime），消除 MNN cmake + bindgen + libclang 依赖，ort 与 ASR 引擎共用推理后端。

**关键踩坑与后处理（plan 独有，见 [[paddle-ocr-sort-boxes-python-j-plus-1]]）**：
- `read_character_file` 原 `trim()` 误删全角空格 U+3000（字典首行）→ CTC 索引偏移 1 位，改 `strip_suffix('\r')`。
- `sort_boxes_like_python` 复刻 PaddleOCR `predict_system.py` 的 **j+1 相邻交换**（非 i+1）——前三轮误判 i+1 为「保真」是循环论证（验证脚本复刻了自己的 bug），第四轮据官方源码纠偏（`d5c07a5`）。
- 长图切分去重由「文本逐字相等」改为**坐标去重** `drop_overlapped_blocks(covered_until_y)`（`510d475`）。
- ort rc.10→rc.12 API 适配（outputs/inputs 方法、Builder map_err、inputs! 宏、ndarray 0.17）+ `download-binaries` feature（不能用 `default-features = false`）。
- **opencv 死代码清理 ~1000 行**：删全部 `#[cfg(feature = "opencv-backend")]` 门控 + `VisionBackend` enum 本身（ocr/desktop 零引用确认完全内部类型）+ 合并所有 `_with_backend` 变体为单一 pure rust 实现。
- 词库精简 `words_alpha.txt`(370K 词/4MB) → `words_common.txt`(17.7K 词/168KB)。

---

## 六、VadSegmented LSTM 跨段漂移修复

> 原 plan：`2026-07-07-vad-segmented-lstm-drift-fix.md` ｜ 状态：✅ A+B 已落地（代码+单测，e2e 依赖漂移复现靠运气未勾）｜ design：[`specs/2026-07-08-vad-segmented-pipeline-design.md`](../specs/2026-07-08-vad-segmented-pipeline-design.md) §5 ｜ feature：[asr-engine.md](../../features/asr-engine.md) §VAD 分段切分策略（已是权威来源）

**症状**：SenseVoice 引擎录音几段后偶发「再说话不吐字」，重启恢复。

**根因**：`VadSegmentedPipeline.detect_vad`（Silero LSTM，`h`/`c` 有状态）在一个录音会话内**跨段累积、从不 reset** → 几段后 LSTM 漂移 → 真实语音持续 `prob<0.5` → `has_speech` 卡 false → `silence_cut`/`force_cut` 都因 `&& has_speech` 永不触发 → `audio_buffer` 无限增长不切段。

**方案（A 治本 + B 兜底，仅 `pipeline.rs::run_tick`）**：
- **A**：切段清理后插入 `detect_vad.reset() + vad_preroll`（切段点天然是 LSTM 安全重置点，preroll 与构造时对称防段首丢字）。
- **B**：`force_cut` 去掉 `&& has_speech`——达上限必切，由 `filter_vad`（每段 reset、不受漂移污染）独立兜底判定有无语音；`silence_cut` 保持 `&& has_speech`（原意：检测到停顿才切）。

**关联（非本根因）**：`finish` 末段丢失是另一独立 bug（finish 不转码剩余 `audio_buffer`，stop 时末段未达切段条件则丢），修复 `2373d58` 在 spec §4.1 单独记录，与 LSTM 漂移无关。详见 [[vad-segmented-finish-tail-flush]]。

---

## 七、AI 命令面板 action bar

> 原 plan：`2026-07-08-action-bar.md`（7 核心 + 27 调试优化 + 9 审查修复）｜ 状态：✅ 已合 main ｜ design：[`specs/2026-07-08-action-bar-design.md`](../specs/2026-07-08-action-bar-design.md) ｜ feature：[desktop-app.md](../../features/desktop-app.md) §12

**目标**：新建 `action_bar_window` 迷你浮窗，用户选中文本 → 热键 → 模拟 Cmd+C → 弹出动作栏 → AI/搜索/翻译/网页 → CompactEditor 展示结果。**仅 macOS**（依赖 CGEvent 鼠标坐标 + osascript 模拟 Cmd+C）。

**全局约束（plan 独有，反复踩坑）**：
- **物理/逻辑坐标**：`CGEvent::location()` 返回**逻辑坐标（points）**，Tauri `LogicalPosition` 是逻辑像素，两者一致**不除 scale**；`Monitor::position()/size()` 返回物理像素**需除 scale**。曾误把 CGEvent 当物理坐标除 scale → 浮窗偏到无关位置。
- **trigger 后台线程**：模拟 Cmd+C 后 200ms sleep 不能在主线程（阻塞事件循环 → 窗口无焦点 → Esc/按钮无响应），必须 `std::thread::spawn`。
- **NSWindow 操作必须在主线程**：`show_action_bar_window` 用 `app.run_on_main_thread()`，不能用 `tauri::async_runtime::spawn`（tokio worker 线程）。见 [[tauri-async-cmd-window-main-thread]]。
- **capabilities 白名单**：`action_bar_window` 必须加入 `capabilities/default.json` 的 `windows` 数组，否则 listen/invoke 全被 ACL 静默拦。见 [[tauri-dynamic-window-capability]]。
- **mousedown capture 陷阱**：`addEventListener("mousedown", fn, true)` capture 模式在 onClick 前触发 → 按钮点击被拦截；外部点击检测用 `click` 事件冒泡阶段（`false`）。
- **剪贴板生命周期**：trigger 阶段 suppress_next → Cmd+C → 读选中 → 立即 write_text 恢复原始剪贴板（选中文本不入库），存 `PENDING_CONTEXT` 供后续操作。

**与原 plan 的偏差**：
1. **AI 结果不做 Run And Paste**——浏览器安全策略阻止模拟粘贴 + 焦点时序问题，改为 CompactEditor 展示（isTemp 临时 tab 不写 DB）。
2. **搜索改为子菜单**（Google/百度/Bing），`action_bar_search_engine` 配置项控制默认高亮。
3. **system_prompt 全局污染修复（P0）**：`run_ai_action` 不再用 `set_system_prompt`/`polish`（会污染并发 ASR 润色），新增 `octopus_llm::chat_text_with_prompt(system, user, config)` 走参数注入。
4. **trigger 重入 guard**：`TRIGGER_IN_PROGRESS: AtomicBool` 防热键连按，`finalize_action_bar` 统一收口。
5. **AI 超时**：翻译 5s + 润色/摘要/解释 10s；前端 `timedOutRef` 丢弃超时后到达的结果。

---

## 八、系统状态页 + model_probe 依赖反转

> 原 plan：`2026-07-08-system-status-page.md`（16 Task 含 5 轮审查修复）｜ 状态：✅ 全部合 main ｜ design：[`specs/2026-07-08-system-status-page-design.md`](../specs/2026-07-08-system-status-page-design.md) ｜ feature：[desktop-app.md](../../features/desktop-app.md) §13

**目标**：设置窗新增「系统状态」tab，实时展示 octopus 进程内存/CPU + 各本地模型估算内存 + 短时趋势（sparkline）。

**架构（依赖反转）**：
- `crates/infra/src/model_probe.rs`：全局加载探针 `set_probe`/`probe(LoadPhase, id)`，infra 不依赖 sysinfo/desktop，只持有闭包。
- asr-local（`load_engine_into_cache`/`SileroVad::new`）+ ocr（`OcrEngine::instance`）+ 流式（`StreamingSessionManager::switch_model`）在加载点埋点 Before/After/Unload。
- `crates/desktop/src/system_status_commands.rs`：`SystemStatusSampler`（tokio 后台每 2s sysinfo 采样进 ring buffer 60 点=2 分钟 + emit）+ `ModelMemoryRegistry`（加载前后 RSS 差值估算）+ `get_system_status` 命令。desktop 启动注入 probe 闭包（读 RSS 差写 registry）。

**关键踩坑（plan 独有）**：
- **RSS vs phys_footprint 双指标**：状态页 RSS（sysinfo `resident_size`）含 mmap 的 file-backed 模型权重，比活动监视器「内存」（`phys_footprint`，不计可回收 file-backed 页）长期高 ~450M。macOS 用 `proc_pid_rusage` FFI 读 `ri_phys_footprint`（flavor `RUSAGE_INFO_V0=0`，**非 16**；字节偏移 72）→ `ProcessStats.real_bytes`；非 macOS 返回 None 退 RSS。
- **模型内存「估算」**：同进程 ort 无法 OS 级 per-model 拆分，用加载前后 RSS 差值近似；**仅首次记录不覆盖**（ort arena 复用致后续差值偏低/为负）。`estimated` 首次值持久缓存，跨 unload/reload 保留（reload 复用首次值，不算偏低差）。
- **probe race ThreadId 隔离**：多线程并发加载同一未缓存模型时 `before_map` key 覆盖/错拿，key 从 `String` 改 `(ThreadId, String)`，Before/After 同线程配对。
- **probe 持锁调用户闭包**：持 PROBE 锁调 f，fallback 路径 sysinfo 扫全部进程慢、阻塞其他线程；改 clone `Option<ProbeFn>`（Arc+1）释放锁后再调 f。
- **OCR idle 60s 自动释放**：`OcrEngine.inner: Mutex<Option<RapidOcr>>` + std::thread 守护线程（ocr crate 共享 cli/server、无 tokio runtime 假设，`tokio::spawn` 在 Tauri sync setup panic「no reactor running」，改 std::thread，见 [[tauri-async-runtime-spawn-not-tokio]]）。详见 [ocr.md](../../features/ocr.md) §3.1-§3.2。
- **释放后进程内存数值不降（接受现状）**：RapidOcr drop 后 ort session 走 `malloc/free`，macOS libmalloc 不主动 `munmap` 归还物理页；真实收益是「下次重载复用 free list」+「内存压力时 OS 可压缩回收」，非立即降数值。

---

> **归档边界说明**：本文件覆盖 2026-07-05 ~ 2026-07-08 的实施 plan（第二批）。2026-07-06 起活跃 plan 至此全部归档完毕。2026-07-09 起的 plan（action-bar-menu-db / ocr-layout-aware / models-tab / clipboard-dock / asr-hotword / hotword-sets / markdown-editor / extension-package / input-source-switch / action-bar-script-enhancement 等）仍在 `plans/` 独立维护。配套设计层面内容见 `specs/` 对应文件。
