# ActionBar 搜索功能 实施计划

> **状态：全部完成 ✅**（含 11 轮 code review 修复 + 搜索增强 / 键盘导航重构）
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

**键盘导航重构（refactor 127877fc + 6d22df27）**：
- 去掉 `searchFocusZone`（输入区/结果区焦点切换）概念——输入框始终保持 DOM focus
- `Tab`/`Shift+Tab` 只在 Tab 页间循环（all→apps→files→shell→bookmarks→all），不回搜索框
- Tab 定位改 **Cmd+字母**：`Cmd+A`/`D`/`F`/`S`/`B` = 全部/应用/文件/Shell/书签；Tab 栏按钮显示「全部 ⌘A」等（字母大写、文字后、`text-[9px] font-mono` 弱化）
- ↑↓ 导航结果时输入框保留 focus（字母键直接进输入框参与过滤，不触发 IME）

**IME Enter 最终方案（多轮反复后收敛 10e20727）**：
- macOS 序列：选词 = `keydown(keyCode=229)` → `compositionend` → `keydown(Enter,13)`；纯英文 Enter 前无 229
- window keydown 记录 keyCode 229 时间戳；Enter(13) 在 229 后 500ms 内 → 跳过（选词确认），否则正常执行
- **不依赖** `isComposing`（window 级时序不可靠）和 `compositionend`（WKWebView 空组合误触发）；早期 compositionstart/end + skipNextEnterRef 方案因「Enter 按两次」「纯英文被吞」等问题已废弃

**UX 收尾**：
- hover 选中：`onMouseMove` 坐标比较（<1px 容差）+ 结果/Tab 变化后 200ms 抑制——React 重渲染致 DOM 重建触发的 mousemove 不影响选中
- 窗口 resize generation token 串行化；Tab 按钮 `transition-colors`（非 `transition-all`）防 active 切换尺寸晃动；TabBar 固定 `h-[30px]`
- 全局快捷键 toggle：窗口可见时再按 → 后端内联 `win.hide()`（不经前端 dismiss）；不可见 → 正常触发
- 设置页 `GeneralPanel` 内部加水平 sub-tab（一般/快捷键/语音，纯 UI 重组，无新配置项 / 无 Rust 改动）
