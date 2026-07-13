# Action Bar 应用上下文获取 实施计划

> **状态**：已实现

**Goal:** 在 action bar 触发时通过 macOS Accessibility API + Browser AppleScript JS 获取选中文本的来源应用与前后文，让 LLM 动作具备情境感知。

**Architecture:** `app_context/` 模块，`ContextProvider` trait + cfg 分发。macOS 双路径：AX（原生 App）+ AppleScript execute javascript（浏览器）。`trigger_action_bar` 调 `gather()` 采集上下文，`build_enriched_text()` 注入 LLM prompt。失败降级到「仅 text」零回归。

**Spec:** [`2026-07-12-actionbar-app-context-design.md`](../specs/2026-07-12-actionbar-app-context-design.md)

## Global Constraints

- **平台**：macOS（AX + Browser JS）+ Windows（UIAutomation）；Linux 暂不支持（NullProvider）
- **降级铁律**：上下文获取任何失败都不得阻塞浮窗显示
- **Terminal scrollback**：30 行 / 1000 字截断
- **Editor before/after**：各 1000 字截断
- **AX 超时**：500ms 上限（deadline 透传到每层递归）
- **classify_app**：内部 `to_ascii_lowercase()` 统一大小写
- **测试命令**：`cargo test -p octopus-desktop --bin octopus-desktop`

---

## Task 执行记录

### Task 1: app_context 模块骨架 ✅
- [x] 创建 mod.rs（类型 + trait + classify_app + extract_surrounding + 纯函数测试）
- [x] 创建 macos_ax.rs stub + ffi.rs stub
- [x] main.rs 注册 `mod app_context;`
- **偏差**：plan 写 lib.rs，实际入口是 main.rs（binary-only crate）

### Task 2: macOS AX 实现 ✅
- [x] ffi.rs AXUIElement C FFI 声明
- [x] macos_ax.rs AxProvider 实现（NSWorkspace + AXUIElement）
- **偏差**：AX 属性名不能用 extern static（链接器不可见），改用 `CFString::new("AXFocusedUIElement")`
- **偏差**：objc2-app-kit 版本 0.3（项目已有），非 plan 写的 0.2

### Task 3: ActionBarContext 升级 + trigger 集成 ✅
- [x] ActionBarContext 加 source/surrounding 字段
- [x] trigger_action_bar 调 gather_context

### Task 4: LLM prompt 上下文注入 ✅
- [x] build_enriched_text 拼接来源/窗口/上文/下文到 prompt
- [x] LLM 翻译 + 润色/摘要/解释注入；本地翻译不注入

### Task 5: 前端类型更新 ✅
- [x] Context 接口加 source/surrounding 类型
- [~] 来源标签：已实现后移除（挤压菜单布局）

### Task 6: 手动验证 + 文档同步 ✅
- [x] architecture.md 更新
- [x] plan review 回写

---

## 实现过程中追加的修复（plan 外）

### CF 类型安全（防崩溃）
- [x] `get_attribute_string` 加 `is_cf_string` CFTypeID 检查（CFNumber 被 CFString 解转崩溃）
- [x] `is_cf_array` CFTypeID 检查（AXChildren 返回 CFBoolean 崩溃）
- [x] `find_text_element_depth` CFRetain 返回的子元素（CFArray Drop 后 use-after-free）
- [x] `find_text_element_depth` AXValue 检查后 CFRelease（内存泄漏）
- [x] 11 个单测覆盖类型安全

### AX 子树遍历
- [x] `find_text_element` 递归遍历找文本元素（Sublime 焦点元素不是文本区）
- [x] 优先匹配包含 selected_text 的文本元素（Chrome AXWebArea）
- [x] max_depth 5→8、max_breadth 50→100（Chrome 深层 AX 树）
- [x] AXFocusedUIElement `-25212` 时 200ms 重试 + AXIsProcessTrusted 诊断

### 内容校验与降级
- [x] full_text 不含选中文本时降级返回 None（Sublime/WPS 自绘编辑器）
- [x] `ax_error_desc` 错误码翻译（-25212 → kAXErrorAPIDisabled）

### Terminal 坐标系修复
- [x] AXSelectedTextRange 与 AXValue 不在同一坐标系 → 改用 selected_text 在 full_text 中搜索定位

### 控制字符过滤
- [x] `strip_control_chars` 过滤终端 scrollback 的 `\0` 等 C0 控制字符

### Browser AppleScript JS（方案演进）
- [x] `gather_browser_via_applescript` 通过 execute javascript 读 DOM
- [x] JS 写临时文件 + `read POSIX file`（避免引号转义地狱）
- [x] Chrome 正确语法：`execute (active tab of window 1) javascript jsCode`
- [x] JS 用 selected_text 搜索 DOM（不依赖 window.getSelection，执行时选区已清空）
- [x] JS 向上溯源 parentNode 直到 textContent ≥ 2000 + sel.length
- [x] `findIn` 模糊匹配（选中文字与 DOM 可能不完全一致）

### 上下文日志
- [x] 采集结果写入 `~/.octopus/logs/action-bar.log`
- [x] AX 诊断信息写入日志文件（focused_role/child_role/ax_value_err/selected_range_err/full_text_preview）
- [x] 日志文件路径修复（`create_dir_all(path.parent())` 而非 `create_dir_all(path)`）
- [x] 12 个单测覆盖格式化/写入/目录创建

### 参数调整
- [x] 上下文截断 2000→1000 字；Terminal scrollback 50 行/2000 字→30 行/1000 字
- [x] WPS Office bundle id 加入 Editor 分类

### 杂项
- [x] 移除 ActionBar 来源标签（挤压菜单布局）
- [x] 移除废弃的 `truncate_terminal_scrollback`（被 `truncate_text_tail` 替代）
- [x] 清理 dead_code warning
- [x] `open_temp_compact_editor` 投递主线程修复 Dock 图标（main 上也存在的 bug）

### 三轮代码审查修复
- [x] **严重 1**：gather 改异步——浮窗先弹（仅 text），后台线程采集完成后回填（不阻塞浮窗 + guard 不泄漏）
- [x] **严重 2**：deadline 透传到 `gather_surrounding` → `find_text_element_depth`，每层递归入口检查超时
- [x] **中 3**：JS 临时文件 RAII guard（Drop 删文件）+ 唯一文件名（纳秒时间戳）
- [x] **低 4**：`get_selected_range` 加 `is_cf_value`（`AXValueGetTypeID`）类型守卫
- [x] **低 6**：`extract_surrounding` 归一化 range（`end = end.max(start)`）
- [x] **二轮 新-1**：gather 回填前校验 `ctx.text == text_for_gather` 防跨触发污染
- [x] **二轮 新-2**：osascript 改 `spawn` + `Stdio::piped()` + `try_wait` 轮询超时 + `kill`
- [x] **二轮 新-3**：临时文件名加纳秒时间戳 + spawn 失败路径 RAII 清理
- [x] **二轮 新-4**：移除死事件 `emit("action-bar://context-updated")`
- [x] **三轮**：osascript spawn 补 `Stdio::piped()` + 并发读线程（防 child.stdout=None 功能回归）
- [x] **三轮卫生**：超时路径显式 join 读线程

---

## 回顾检查项

1. ✅ `ActionBarContext` 有 `source` + `surrounding` 字段
2. ✅ macOS 走 NSWorkspace + AXUIElement（原生 App）/ AppleScript JS（浏览器）
3. ✅ `gather()` 异步——浮窗先弹，不阻塞；回填校验防污染
4. ✅ Terminal：selected_text 搜索定位切 before/after（不依赖 AX range）
5. ✅ Editor：before/after 各 1000 字截断
6. ✅ AX 超时：deadline 透传到每层递归 + osascript spawn 超时 kill
7. ✅ AI 动作 prompt 注入上下文；本地翻译不注入
8. ✅ 前端类型升级（来源标签已移除）
9. ✅ CF 类型安全：`is_cf_string`/`is_cf_array`/`is_cf_value` + range 归一化 + RAII 文件清理
10. ✅ 纯函数 + 类型安全 + 日志 单测共 41 个
11. ✅ 手动验证：TextEdit / Terminal / iTerm2 / Chrome / Sublime / WPS / Safari
