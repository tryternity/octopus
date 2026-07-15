# ActionBar 搜索功能 实施计划

> **状态：全部完成 ✅**（含 14 轮修复 + 搜索增强 / 键盘导航重构 / 焦点时序修复 / S1-L6 系统性审查）
>
> 本文档为实施记录而非一次性待办——反映实际实现。

**Goal:** 将 ActionBar 从纯菜单条升级为搜索驱动的命令面板：搜索输入框 + Tab 分组结果（应用/文件/Shell/书签）+ Run And Paste。

**Architecture:** 前端 ActionBar 组件重构（输入框 + Tab 栏 + 结果列表 + 展开/收起逻辑），后端新增独立 crate `octopus-search`（应用索引 + mdfind 文件搜索 + 书签解析 + nucleo-matcher 模糊匹配），DB v32 加 `trigger_keyword` + `auto_paste` 字段。

**Tech Stack:** Rust, Tauri 2, React/TypeScript, nucleo-matcher

**Spec:** `docs/superpowers/specs/2026-07-15-actionbar-search-design.md`

## Global Constraints

- 无选中 → ActionBar 居中弹出（主屏 1/5 处），仅搜索框，无菜单条
- 有选中 + 输入框为空 → 菜单条显示
- 输入框有内容 → Tab + 搜索结果替代菜单条
- 即时搜索（应用+菜单+Quicklinks）无防抖，延迟搜索（文件+书签）150ms 防抖
- 选中检测用 `NSPasteboard.changeCount`（非剪贴板内容比较）

---

### Task 1: DB v32 — trigger_keyword + auto_paste 字段 ✅

- [x] db.sql + db.rs v31→v32 迁移（`trigger_keyword` + `auto_paste`）
- [x] `insert_action_bar_item` / `update_action_bar_item` 全链路传入新字段
- [x] `set_auto_paste` 命令（零行检查）
- [x] 新建项 `is_enabled` 参数化（不再硬编码 1）

### Task 2: 搜索引擎独立 crate `octopus-search` ✅

- [x] `matcher.rs` — exact/prefix/pinyin/fuzzy 四级匹配（打分方向：短目标分更高）
- [x] `app_index.rs` — 扫描 macOS 应用目录（跨目录同名去重）
- [x] `file_search.rs` — mdfind + 10s 超时 + kill_on_drop
- [x] `bookmark.rs` — Chrome/Edge JSON 解析（遍历 roots 的 folder values）
- [x] `engine.rs` — 统一 SearchEngine + tab 参数（quick/files_bookmarks/all）
- [x] 25 个单元测试

### Task 3: Tauri 命令 ✅

- [x] `search_all` / `launch_app` / `open_file` / `open_url` / `execute_shell` / `set_auto_paste`
- [x] `launch_app` / `open_file` / `open_url` 用 `status()`（非 spawn，防僵尸）
- [x] `execute_shell` 30s 超时 + kill_on_drop + 100KB 字符截断

### Task 4: 前端搜索面板 ✅

- [x] `searchTypes.ts` — 类型 + 常量
- [x] `searchLogic.ts` — 15 个纯函数
- [x] `searchLogic.test.ts` — 56 个单元测试
- [x] `SearchPanel.tsx` — Tab 栏 + 结果列表
- [x] `index.tsx` 集成 — 搜索输入框 + 条件渲染 + 窗口动态调整 + 展开方向
- [x] 键盘导航完整实现（Tab 循环 / ↑↓ / ?af>b / i / Escape）
- [x] 窗口 resize 用 generation token 防 Tab 快速切换异步乱序
- [x] 延迟搜索 cancelled 守卫
- [x] selectedIdx / subSelectedIdx clamp effect
- [x] 菜单项 accepts 过滤（无选中时仅 accepts=any）

### Task 5: Quicklink 关键词触发 ✅

- [x] 后端 `search_quicklink_keywords` 检测 `<keyword> <rest>` 模式
- [x] `url_encode_param` 百分比编码
- [x] 前端 URL 模板 `{query}`/`{text}` 替换
- [x] 设置页 trigger_keyword 输入框（URL 类型）

### Task 6: Run And Paste ✅

- [x] `action_bar_run_and_paste` → 写剪贴板 + 模拟 ⌘V
- [x] execute_action_bar_inner 三处调用点检查 auto_paste
- [x] LLM 翻译用纯 text（非 enriched_text）
- [x] LLM 调用用 spawn_blocking
- [x] 设置页 autoPaste 开关（AI/Script 类型）
- [x] 麦克风不可用时 emit mic-error + 红色气泡

### Task 7: 无选中居中搜索 ✅

- [x] `detect_selection` + `Selection` 枚举（None/Text/File/Folder + mouse）
- [x] `NSPasteboard.changeCount` 判断有无选中
- [x] 无选中 → 主屏居中弹出（1/5 位置）
- [x] `show_action_bar_centered` 强制用主显示器
- [x] osascript 5s 超时（防权限对话框永久挂起）

---

## Code Review 修复记录

5 轮 code review，修复 30+ 项问题，关键的：

### 第一轮（8 项）
- mergeResults 合并后全局排序（原仅拼接）
- 非关键词 Quicklink URL 模板前端替换
- Files/Bookmarks tab 延迟搜索加防抖
- 新建项 autoPaste 不丢失
- execute_shell 加超时 + 输出截断
- DB 读合并为一次
- i18n 硬编码中文修正
- ref-before-declaration 修正

### 第二轮（5 项）
- 延迟搜索 cancelled 守卫
- Enter 与视觉高亮一致
- stale baseWinPosRef 清空
- 搜索菜单项按 accepts 过滤
- prefix_match 打分方向

### 第三轮（3 项）
- execute_shell 截断 panic（字节切片→字符截断）
- execute_shell kill_on_drop
- pinyin_match 打分方向

### 第四轮（6 项）
- LLM 翻译用纯 text
- mergeResults 去重 key 加 subtitle
- mdfind 加 10s 超时
- 改 actionType 时重算 accepts
- 新建项 is_enabled 参数化
- 无选中时 menu accepts 过滤

### 第五轮（2 项 + 观察项）
- mdfind kill_on_drop
- subSelectedIdx clamp
- osascript 超时副作用（可接受取舍）

### 后续修复
- Chromium 书签 roots 遍历修正
- 快速 Tab 切换窗口尺寸异步乱序（generation token）
- 无选中时不显示菜单条
- AI/LLM spawn_blocking

### 第六-九轮（7 项）

- prefix_match / pinyin_match 剩余惩罚改 **char count**（非 byte len，CJK 3 字节/char 公平）
- `accepts="any"` 项无选中时可执行（去掉 `executeItem` 的 `if(!ctx) return`，text 用空串；`contextFilteredResults` 保证无选中仅 any 项入结果）
- loading 视图 Escape 仍生效（auto_translate 无超时，防卡住困死）
- 无结果时 `calcResultsHeight(0)` 返回 1 行高度 36px（保证「无结果」提示可见）
- IME 组合中（`e.isComposing`）Enter 不触发搜索执行（确认候选词）
- URL scheme 白名单：选中文本即 URL 仅放行 http/https，其余补 https://（防 smb/file/vnc 触发系统操作）
- `search_all` 入口 `query.trim()`（防前导/尾部空格致 exact/prefix 匹配失败）

### 第十轮（实施 P0-P2 建议修复）

- **P0 extension 编辑死锁链**：`install_extension` 加 `shortcut`/`is_enabled`/`replace_id`（编辑重选走 update 保持位置）；`import_extension` 去掉 `dest.exists()` 拒绝；`saveEdit` 重构去顶部误拦 + 补 `set_auto_paste`；autoPaste toggle 显示 extension 类型
- **P1 剪贴板恢复**：`detect_selection` 恢复逻辑前移到读 `clipboard_after` 之后（`_` 分支选中图片/文件也恢复原剪贴板）
- **P2 urlDetect 边界**：文件扩展名黑名单 `FILE_EXT_RE` + IPv4 `isValidIpv4Host` 0-255 校验
- **P2 性能/可维护性**：`fuzzy_match` Matcher `thread_local` 复用；`app_index` 递归子目录（深度 2，覆盖嵌套 .app）；`search_bookmarks` 按 url 去重；`now_iso8601` → `now_epoch_secs` 重命名（名实一致）

### 第十一轮（搜索增强 + 键盘导航重构 + IME/UX 收尾）

**搜索增强（feature）**：
- 应用**本地化别名**：扫描读 `zh-Hans.lproj/InfoPlist.strings`（UTF-16 LE 解码 + OpenStep/XML plist 解析）取 CFBundleDisplayName/Name 作 alias（WeChat→微信）；`AppIndex::search` 对 name+aliases 都匹配取最高分
- 应用**图标**：`sips -s format png -z 32 32` 提取 icon → base64 PNG 存 DB；`SearchResult` 携带 `icon` 字段（应用为 base64 PNG，其余 source 为 None 由前端用默认图标）
- 应用索引 **DB 缓存**（`app_index` 表，v33 建表 / v34 补 icon 列）：启动 <1ms 加载，DB 空则扫盘写回，旧缓存 icon 全空时自动重扫；`reindex_apps` Tauri 命令供装卸应用后强制刷新
- **拼音通用化**：`pinyin` crate 覆盖全部 CJK 汉字首字母（替代硬编码菜单表），pinyin 分数 3000→4000；app 结果额外 +2000 权重，使拼音匹配 app（4000+2000=6000）排在文件 prefix match（~5000）之前

**键盘导航重构（refactor 127877fc + 6d22df27，搜索模式键盘）**：
- 去掉 `searchFocusZone`（输入区/结果区焦点切换）概念——输入框始终保持 DOM focus
- `Tab`/`Shift+Tab` 只在 Tab 页间循环（all→apps→files→shell→bookmarks→all），不回搜索框
- Tab 页定位：此轮为 `Cmd+A`/`D`/`F`/`S`/`B`（**第十三轮统一改为 `Alt+字母`**——Alt 统一承担定位/切换，Cmd/Ctrl 让给执行）；Tab 栏按钮显示「全部 ⌥A」等（字母大写、文字后、`text-[9px] font-mono` 弱化）
- ↑↓ 导航结果时输入框保留 focus（字母键直接进输入框参与过滤，不触发 IME）

**IME Enter 最终方案（多轮反复后收敛 10e20727）**：
- macOS 序列：选词 = `keydown(keyCode=229)` → `compositionend` → `keydown(Enter,13)`；纯英文 Enter 前无 229
- window keydown 记录 keyCode 229 时间戳；Enter(13) 在 229 后 500ms 内 → 跳过（选词确认），否则正常执行
- **不依赖** `isComposing`（window 级时序不可靠）和 `compositionend`（WKWebView 空组合误触发）；早期 compositionstart/end + skipNextEnterRef 方案因「Enter 按两次」「纯英文被吞」等问题已废弃

**UX 收尾**：
- hover 选中：`onMouseMove` 坐标比较（<1px 容差）+ 结果/Tab 变化后 200ms 抑制——React 重渲染致 DOM 重建触发的 mousemove 不影响选中
- 窗口 resize generation token 串行化；Tab 按钮 `transition-colors`（非 `transition-all`）防 active 切换尺寸晃动；TabBar 固定 `h-[30px]`
- 全局快捷键 toggle：窗口可见时再按 → 隐藏（**第十三轮改为走统一收口 `hide_action_bar_window`**，非裸 `win.hide()`——否则 show 时切的 Regular policy 残留、Dock 图标常驻）；不可见 → 正常触发
- 设置页 `GeneralPanel` 内部加水平 sub-tab（一般/快捷键/语音，纯 UI 重组，无新配置项 / 无 Rust 改动）

### 第十二轮（测试竞态修复 + 预存警告清理）

- **TRIGGER guard 测试竞态**：4 个 `test_reset_trigger_guard*` 测试共享全局 `TRIGGER_IN_PROGRESS`/`TRIGGER_TIMESTAMP` 静态量，Rust 默认并行跑 → 互相覆盖导致 `test_reset_trigger_guard_if_stale_keeps_recent` 间歇性失败。加 `TRIGGER_TEST_LOCK: Mutex<()>` 序列化这 4 个测试
- **预存警告清理**：`macos_ax.rs:2366` 测试中 `if let Some((before, after))` 的 `after` 未使用 → 改 `_after`

### 第十三轮（菜单模式键盘模型重构 + 焦点时序修复）

**菜单模式键盘模型重构（双模式确立）**：
旧菜单模式键盘（左右键移项 / 无修饰 1-9+a-z 定位 / Alt+字符执行）整体重设计，与搜索模式键盘并立为**双模式**。

- **输入框始终聚焦**：去掉「有选中文本时不聚焦 input（让方向键导航菜单）」的旧逻辑——input 始终持 DOM focus，键盘处理器放行规则依赖 `activeElement===input`
- **修饰键重映射**：`Alt+数字/字母`=定位菜单项（`labelToIndex`，选中不执行）；`Cmd/Ctrl+字符`=执行（按菜单项 `shortcut` 匹配）；无修饰字符=进输入框触发搜索过滤。统一 **Alt=定位/切换、Cmd/Ctrl=执行**，避免与执行键打架
- **Tab 替代左右键**：`Tab`/`Shift+Tab` 在主菜单项 / 子菜单项间循环移动（submenu 项自动展开预览），原 `ArrowLeft/ArrowRight` 移项职责移交 Tab
- **↑↓ 切焦点层**：主↔子菜单层切换不变（注释更新为「展开/收起由 Tab 控制而非左右键」）
- **input 无多行/回车**：Enter 交给处理器执行菜单项，不放行给输入框；放行规则 `navKeys=["ArrowUp","ArrowDown","Tab","Enter"," "]` + `!e.altKey && !e.metaKey && !e.ctrlKey`（补 `!e.ctrlKey` 防 Ctrl+字符误入输入框）
- UI：IconBtn `title` 显示「`Alt+{indexLabel}` 定位 · `⌘{shortcut}` 执行」；快捷键徽章 `⌥`→`⌘`

**焦点时序修复（Sublime 间歇性失焦根因）**：
- **gather_context 同步化**：`trigger_action_bar` 文本分支 `gather_context` 从 show 后异步改为 **show 前同步**——gather 调前台 app（Sublime `subl --command` / Browser osascript）会激活前台 app、异步在 show 之后抢走 ActionBar 焦点（对照实验铁证：无选中不 gather → 正常获焦；有选中 gather → 失焦）。附带收益：show 前前台确定是源 app，`frontmost_app()` 读上下文更准（原异步方案 ActionBar 获焦后 frontmost 可能变成 octopus 自己）。代价：热键到弹出增加 gather 耗时（Sublime ~50-150ms）
- **show 后焦点探针 + 巩固**：`show_action_bar_window` 后起诊断线程 150/350ms 记录 `isKeyWindow`（objc2 `msg_send!`，比 `NSApplication::isActive` 准——isActive 是 app 级，isKeyWindow 是窗口级）；150ms 若已失焦则 `set_focus` 巩固夺回——覆盖 Sublime 延迟激活窗口
- **onFocusChanged 宽限**：show 后 500ms 内的 spurious focus-lost 不触发 dismiss（`showTimeRef` 记录 show 时刻，app 激活 / 窗口成 key 的时序抖动期不误关）
- **resize 后重新 focus**：`apply` effect 在 `setSize`/`setPosition` 后（macOS 调 NSWindow frame 触发 webview blur，致「打第一个字母即失焦」）若 `activeElement !== inputRef` 则重新 focus，保证连续输入不中断

**诊断增强 + 收口统一**：
- `action_bar_dismiss` 加 `reason: Option<String>`（`focus-lost` / `click-outside` / 操作后）+ `[action-bar][dismiss]` 日志；前端 `onFocusChanged` 加 console.log
- `trigger_agent_voice` 隐藏浮窗从裸 `win.hide()` 改走统一收口 `hide_action_bar_window`（切回 Accessory + 焦点协调）；热键 toggle 同改（防 show 时切的 Regular policy 残留、Dock 图标常驻）

**React #300 修复（ab51a283 → d3daeda9）**：
- 初版 ab51a283 仍有 hooks 顺序问题；d3daeda9 将 useEffect + subItems 声明移到 early return 之前，彻底修复 hooks 数量不一致

**TRIGGER guard 收尾（092e4dbb + b91af242）**：
- toggle 重置 + 超时保护：上次触发超 10s（092e4dbb 初 30s → b91af242 改 10s）仍未 finalize 则强制重置，防 webview 崩溃后 guard 永久卡死

### 第十四轮（系统性代码审查 S1-S4 / M1-M5 / L1-L6 修复）

承第十三轮键盘/焦点修复之后的系统性代码审查，逐项修复 + 配套回归测试：

- **S1 reindex 无效 → 后台 mtime 自动扫描**：`SearchEngine.app_index` 改 `RwLock<AppIndex>`（对齐 `hotword.rs` 的 `OnceLock<RwLock<T>>` 先例），后台线程启动 30s 后每 10 分钟检测 `/Applications` 等目录 mtime，变化时 `refresh_app_index` 刷新内存 + DB。`reindex_apps` 命令改为调 `refresh_app_index`（修复"只更 DB 不更内存"）。新增 `refresh_app_index_replaces_in_memory_index` + `app_index_rwlock_concurrent_safe` 2 个并发测试
- **S2 save_app_index 非原子**：DELETE 移入 `unchecked_transaction` 内，中途 INSERT 失败回滚 DELETE。新增 `save_app_index_atomic_on_failure` 测试（UNIQUE 冲突验证原数据保留）
- **S3 submenu Enter 失灵**：`executeItem` 展开 submenu 时 `setFocusLayer(nextFocusLayerAfterExecute(...))`——executeItem 是终结性动作，展开后焦点进 sub，Enter 执行子项。抽 `nextFocusLayerAfterExecute` 纯函数到 searchLogic.ts + 3 个单测。Tab/Alt 预览路径不改（保持"预览不抢焦点"契约）
- **S4 Safari 书签声明不符**：`load_all_bookmarks` 不再假装"尝试读失败跳过"，改为 `log::debug!("未实现，跳过")`。`load_safari_bookmarks` 标注 `#[allow(dead_code)]` + 注释明确未实现。新增 `load_safari_bookmarks_unimplemented_returns_empty` 测试锁定语义
- **M1 trigger 漏 finalize**：`finalize_action_bar` 从 None 分支提取到 match 后统一调用，覆盖 Text/File/Folder 分支
- **M2 gather 子进程无超时**：抽 `run_command_with_deadline(cmd, deadline)` 到 `app_context/mod.rs`（spawn + 轮询到 deadline + 超时 kill/wait），替换 Pages osascript / lsof / pdftotext / officecli / mdfind / subl 共 7 处裸 `.output()`。fallback 函数加 `deadline: Instant` 参数透传。新增 `run_command_with_deadline_kills_on_timeout` + `returns_output_on_success` 2 个测试
- **M4 dismiss reason 补全**：5 处裸 `invoke("action_bar_dismiss")` 补 reason 参数（launch-app/open-file/open-url/execute-shell/escape）
- **M5 focus-lost 宽限文档化**：移除生产 `console.log`；spec §4.0 补 500ms 宽限不变量
- **L1 hover 抑制覆盖键盘选中**：SearchPanel suppress effect 依赖加 `selectedIdx`
- **L3 sips 临时文件残留**：失败分支也 `remove_file`；文件名加纳秒时间戳防跨进程冲突
- **L4 matcher 热路径分配**：`pinyin_match` 复用单次 `query.to_lowercase()`
- **L5 多音字**：`pinyin_initials` 注释标注 `first_letter()` 只取常用读音的限制（不改实现，避免组合爆炸）
- **L6 v33/v34 死代码**：删除 v34 块的冗余 icon 补丁（v33 内部补丁已覆盖），合并为单次 v32→v34 迁移。新增 `migration_v32_to_v34_creates_app_index_with_icon` 测试

**文档同步**：spec §3.3（后台扫描策略）/ §3.5（submenu focusLayer 契约）/ §4.0（dismiss reason + focus-lost 宽限）/ §6.3（reindex 降级 + gather 超时 + Safari 未实现）/ §6.4 / §8（性能预算拆分）/ §9（+4 条不变量）；plan 第十四轮记录

### 第十四轮（Sublime 选中检测两个现象修复）

用户报告 Sublime 下两个现象：
- **现象 1**：选中文本召唤 ActionBar，有时只显示输入框（无菜单条），输入文字删除后菜单才出现
- **现象 2**：未选中文本召唤 ActionBar，有时显示菜单模式（应只显示搜索框），弹出位置在鼠标位置（应居中）

**现象 1 根因（前端首屏竞态，非 context=null）**：
- 用户观察推翻了 context=null 假设——窗口在鼠标位置弹出说明后端正确判定有选中（PENDING_CONTEXT=Some），但前端只显示输入框
- 根因：`refresh()` 的 `invoke("action_bar_get_context")` 是异步 Promise。窗口已 show + React 已渲染首屏，但 ctx Promise 还在 pending → `context` state 仍是初始/陈旧 null → 只渲染输入框（`context ? menuContent : null`）
- 用户输入触发搜索（inSearch=true）再删除后，期间 ctx Promise 已 resolve，菜单才出现
- **修复**：`show_action_bar_window` emit `action-bar://show` 时携带 `snapshot_pending_context()` 的 context payload；前端 refresh 优先用事件 payload（零延迟），mount 首次走 invoke 兜底。改动：`action_bar_commands.rs` 加 `snapshot_pending_context`；`action_bar_window.rs` emit 带 `&ctx`；`index.tsx` refresh 接收 `showPayload` 参数

**现象 2 根因（后端 changeCount 污染，弹出在鼠标位置=误判 Some）**：
- 用户观察弹出在鼠标位置 → 后端误判有选中（PENDING_CONTEXT 错误写了 Some），非前端残留
- 根因：detect_selection 恢复剪贴板（write_files/set_image/write_text，原 169-177 行）**自身递增 changeCount**。下次 detect 的 `change_count_before` 实时读时，若上次恢复写入尚未完成或本次 200ms sleep 期间有异步剪贴板写入，changeCount"假递增"→ 误判有选中 → 读残留文本 → Selection::Text
- **修复**：全局 `CHANGE_COUNT_BASELINE: AtomicI64` 记录上次 detect 结束时的 changeCount，下次 detect 的 `before = max(实时读, baseline)`；所有退出路径（None/Text）恢复后更新 baseline = 当前 changeCount。隔离恢复写入对下次检测的污染
