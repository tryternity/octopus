# ActionBar 搜索功能 实施计划

> **状态：全部完成 ✅**（含 5 轮 code review 修复）
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
