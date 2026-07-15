# ActionBar 搜索功能设计

> 2026-07-15 · ActionBar 集成搜索输入框 + 应用启动 + 文件搜索 + Quicklinks + 书签搜索 + Run And Paste
>
> **实现完成** — 本文档已同步实际代码（2026-07-16，含 14 轮修复 + 搜索增强 / 键盘导航重构 / 焦点时序修复 / S1-L6 系统性审查）

## 1. 设计目标

将 ActionBar 从纯菜单条升级为**搜索驱动的命令面板**：用户可在搜索输入框中搜索应用、文件、菜单项、Quicklinks、书签，直接触达执行。

### 1.1 无选中行为

无选中（文本/文件/文件夹）时，ActionBar 在主屏幕居中弹出（水平居中，垂直位于上方 1/5 位置，类似 Alfred/Wox），自动聚焦搜索框。选中检测使用 `NSPasteboard.changeCount`——Cmd+C 前后 changeCount 不变即为"无选中"。

## 2. 面板布局

### 2.1 状态切换

| 状态 | 输入框 | Tab+结果 | 菜单条 |
|------|--------|---------|--------|
| 无选中（context=null） | ✅ 显示，聚焦 | ✅ 有输入时显示 | ❌ **不显示** |
| 有选中，搜索框为空 | ✅ 显示 | ❌ 隐藏 | ✅ 显示 |
| 有选中，搜索框有输入 | ✅ 显示 | ✅ 显示 | ❌ 隐藏（被 Tab+结果覆盖） |

### 2.2 向下展开（输入框下方空间充足）

```
┌─────────────────────────────────────────┐
│ [搜索输入框]                              │ ← 始终在顶部
├─────────────────────────────────────────┤
│ [全部 ⌥A] [应用 ⌥D] [文件 ⌥F] [Shell ⌥S] [书签 ⌥B] │ ← Tab 栏（有搜索时显示）
│  结果1                                    │
│  结果2                                    │ ← 最多10行
│  ...                                     │
│  结果N                                    │
├─────────────────────────────────────────┤
│ [翻译] [搜索] [AI] [脚本] [复制路径]     │ ← 菜单条（搜索框为空且有选中时显示）
└─────────────────────────────────────────┘
```

### 2.3 向上展开（输入框下方空间不足）

```
│  结果N                                    │
│  ...                                     │
│  结果2                                    │ ← 最多10行
│  结果1                                    │
│ [全部 ⌥A] [应用 ⌥D] [文件 ⌥F] [Shell ⌥S] [书签 ⌥B] │ ← Tab 栏
├─────────────────────────────────────────┤
│ [搜索输入框]                              │ ← 始终在核心位置
├─────────────────────────────────────────┤
│ [翻译] [搜索] [AI] [脚本] [复制路径]     │ ← 菜单条
└─────────────────────────────────────────┘
```

### 2.4 展开方向判定

- show 时获取窗口 `outerPosition` + `window.screen.height`
- 屏幕高度 - Y > 400px → 向下展开
- 否则 → 向上展开
- 一次 show 中固定（不随结果变化重新判定）

### 2.5 窗口尺寸序列化

快速 Tab 切换时，`setSize` + `setPosition` 用 generation token 保证只有最新一次 resize 生效，且两者串行 await——防异步乱序导致输入框被覆盖。

## 3. 搜索引擎

### 3.1 搜索来源

| 来源 | 数据 | 索引时机 | 搜索方式 |
|------|------|---------|---------|
| **应用** | `/Applications/`、`~/Applications/`、`/System/Applications/`、`/Applications/Utilities/` 下的 `.app`（**递归子目录，深度 2**，覆盖 Adobe / JetBrains Toolbox 等嵌套） | 启动时 | 内存索引，即时（跨目录同名去重） |
| **菜单项** | DB `action_bar_items` 表 | 实时读 DB | 内存索引，即时 |
| **Quicklinks** | DB `action_bar_items` WHERE `action_type='url'` AND 有 `trigger_keyword` | 实时读 DB | 内存索引，即时 |
| **文件** | mdfind（Spotlight metadata） | 实时查 | 防抖 150ms，10s 超时 + `kill_on_drop` |
| **书签** | Chrome/Edge（JSON）| 启动时 | 防抖 150ms（**按 url 去重**） |

### 3.2 搜索策略

```
输入框内容变化
  ├── 即时搜索（无防抖）：应用 + 菜单项 + Quicklinks（tab="quick"）
  └── 延迟搜索（150ms 防抖）：文件（mdfind）+ 书签（tab="files_bookmarks"）
       └── 输入长度 ≥ 2 字符时才触发延迟搜索
       └── 延迟搜索有 cancelled 守卫——防慢请求覆盖新结果
```

后端 tab 参数：`"all"` 合并即时+延迟，`"quick"` 仅即时，`"files_bookmarks"` 仅延迟，`"apps"`/`"files"`/`"bookmarks"`/`"shell"` 单来源。

### 3.3 匹配算法

引入 `nucleo-matcher` crate。`fuzzy_match` 的 Matcher 经 `thread_local` 复用（避免大书签列表逐条匹配时重复分配 score table）。

**匹配优先级**（取最高分）：
1. **精确匹配**（query == target，忽略大小写）→ 10000
2. **前缀匹配**（target 以 query 开头）→ `5000 - remaining`（越短分越高；remaining 按 **char count** 计算，非 byte len——CJK 3 字节/char，byte 算法会系统性压低中文排名）
3. **拼音首字母**（query 全 ASCII 时匹配中文文本的拼音首字母）→ `4000 - remaining`（remaining 按 char count；用 `pinyin` crate 覆盖全部 CJK 汉字，非硬编码菜单名表）
4. **模糊匹配**（nucleo）→ 按匹配度评分

**应用 source 加权**：应用结果 `score += 2000`——应用启动是 launcher 核心场景，确保拼音匹配的 app（如 `wx`→微信 = 4000+2000=6000）排在文件 prefix 匹配（~5000）之前。

**结果合并排序**：即时结果与延迟结果合并后全局按 score 降序排序（`mergeResults` 按 `source:title:subtitle` 去重——同名文件/重复 quicklink 不互丢）。

**应用别名 + 图标**：
- 扫描时读 `zh-Hans.lproj/InfoPlist.strings`（UTF-16 LE 解码 + OpenStep/XML plist 解析）获取本地化名（如 WeChat→微信）作为 alias
- 用 `sips -s format png -z 32 32` 提取 icon → base64 PNG 存 DB
- `AppIndex::search` 对 name + aliases 都匹配取最高分
- 应用索引缓存在 DB `app_index` 表（v34），启动时 <1ms 加载（DB 非空）；DB 为空时扫文件系统 + 写缓存
- **后台自动刷新**：运行期间后台线程每 10 分钟检测 `/Applications` 等目录 mtime，变化时（用户装卸应用）自动触发后台重扫并刷新内存索引 + DB 缓存，用户无感知。内存索引通过 `SearchEngine.app_index` 的 `RwLock` 热替换，搜索走读锁零阻塞
- `reindex_apps` 命令降级为诊断/强制重扫 fallback（后台扫描失效时的手动兜底，正常情况下用户无需调用）

### 3.4 结果分组与 Tab

Tab 页：`[全部 ⌥A] [应用 ⌥D] [文件 ⌥F] [Shell ⌥S] [书签 ⌥B]`

- `全部 ⌥A`：混合展示所有来源结果，按优先级排序
- `应用 ⌥D`：仅 source === "app"
- `文件 ⌥F`：仅 source === "file"
- `Shell ⌥S`：`>` 前缀路由的 shell 命令
- `书签 ⌥B`：仅 source === "bookmark"

**菜单项 accepts 过滤**：搜索结果中的 menu/quicklink 来源按 `context.accepts` 过滤。无选中（context=null）时仅显示 `accepts="any"` 的项。

### 3.5 键盘导航

**设计原则**：输入框始终保持 DOM focus（双模式共用）。不使用 `searchFocusZone` 概念——没有"输入区"和"结果区"的焦点切换。**修饰键统一分工**：`Alt`=定位/切换（选中不执行），`Cmd/Ctrl`=执行；无修饰字符进输入框触发搜索过滤。window 级 keydown handler 据 `query` 是否为空分**搜索模式**与**菜单模式**两套行为。

> macOS Option 键改变 `e.key` 输出（Alt+H → "˙"），所有修饰键分支统一用 `codeToChar(e.code)` 取物理键字符再匹配。

#### 3.5.1 搜索模式（query 非空）

| 按键 | 行为 |
|------|------|
| `Alt+A`/`D`/`F`/`S`/`B` | 跳到对应 Tab 页（全部/应用/文件/Shell/书签） |
| `Tab` / `Shift+Tab` | 循环切换 Tab 页（all → apps → files → shell → bookmarks → all，不回搜索框） |
| `↑↓` | 导航结果列表（输入框始终保留 focus） |
| `Enter` | 执行选中项（无选中时执行第一个） |
| 其他键 | 交给输入框（参与搜索过滤） |

**Tab 栏按钮显示**：`全部 ⌥A`、`应用 ⌥D`、`文件 ⌥F`、`Shell ⌥S`、`书签 ⌥B`——快捷键字母大写，放在文字后面，样式弱化（`text-[9px]`、`font-mono`）。

#### 3.5.2 菜单模式（query 为空）

| 按键 | 行为 |
|------|------|
| `Cmd/Ctrl+数字/字母` | 执行菜单项（按菜单项配置的 `shortcut` 匹配，跨主/子菜单层级） |
| `Alt+数字/字母` | 定位菜单项（`labelToIndex`：1-9→索引 0-8，a-z→索引 9-34，最多 35 项；选中不执行） |
| `Tab` / `Shift+Tab` | 主菜单项 / 子菜单项间循环移动（submenu 项自动展开子菜单预览） |
| `↑↓` | 切换焦点层 main↔sub（不展开/收起子菜单，展开由 Tab 控制） |
| `Enter` / `Space` | 执行当前选中菜单项 |
| 无修饰字符 | 进输入框触发搜索过滤（切换到搜索模式） |

- **input 始终聚焦 + 无回车**：放行规则 `navKeys=["ArrowUp","ArrowDown","Tab","Enter"," "]` + `!e.altKey && !e.metaKey && !e.ctrlKey`——导航键与修饰键组合由 handler 拦截，其余字符放行进输入框。input 无多行/回车概念，Enter 执行菜单项而非提交输入框。
- **IconBtn 提示**：`title` 显示「`Alt+{indexLabel}` 定位 · `⌘{shortcut}` 执行」；徽章显示 `⌘{shortcut}`。

#### 3.5.3 通用按键

| 按键 | 行为 |
|------|------|
| `Escape` | 有查询→清空；无查询→dismiss；**loading 视图也生效** |
| 再按热键 | 窗口已可见 → 隐藏（toggle 语义，走 `hide_action_bar_window` 统一收口） |

**IME Enter 处理**（双模式共用）：
- macOS 事件序列：IME 选词 = `keydown(keyCode=229)` → `compositionend` → `keydown(Enter, 13)`
- 纯英文 Enter = `keydown(Enter, 13)`，前面没有 229
- 实现：window keydown handler 记录 keyCode 229 的时间戳，Enter(13) 在 229 后 500ms 内 → 跳过（选词确认），否则正常执行
- **不依赖** `isComposing`（window 级时序不可靠）和 `compositionend`（WKWebView 空组合会误触发）
- **DOM focus 策略**：input 永远不 blur 也不设 readOnly——因为没有"结果区焦点"概念，字母键直接进入输入框参与搜索过滤

**菜单模式 submenu 展开 × focusLayer 契约**（S3 修复）：
- `executeItem`（点击 / Cmd+字母 / Enter on main）展开 submenu 是**终结性动作**——展开后焦点层进 `sub`，后续 Enter 执行子项（`nextFocusLayerAfterExecute` 纯函数守护）
- Tab / Alt+字母 的**预览展开不抢焦点**——焦点层保持 `main`，用户可继续在主菜单移动（架构文档「预览不抢焦点」契约）
- ↑↓ 仍是从 main 进入 sub 的显式焦点层切换路径

## 4. 搜索结果执行

### 4.0 UI 交互规则

**鼠标 hover 选中**：
- `onMouseMove` 内比较 `clientX/clientY` 与上次记录（<1px 容差）+ 结果/Tab/**键盘选中**变化后 200ms 抑制窗口
- 只有鼠标**真的移动了**才改变选中——React 重渲染导致 DOM 重建时浏览器自动触发的 mousemove 不影响选中
- 键盘 ↑↓ 改变 `selectedIdx` 后也启动 200ms 抑制——防键盘选中后鼠标轻微移动覆盖选中（L1）
- 结果列表变化时 `searchSelectedIdx` 重置为 0（用户输入时焦点在搜索框，选中应始终是第一个）

**窗口 resize 序列化**：
- `setSize` + `setPosition` 用 generation token 保证只有最新一次 resize 生效
- 两者串行 await——防快速 Tab 切换时异步乱序导致输入框被覆盖
- TabBar 固定 `h-[30px]`，Tab 按钮用 `transition-colors`（非 `transition-all`）——防 active 切换时尺寸微变导致布局晃动
- **resize 后重新 focus**：`setSize`/`setPosition` 在 macOS 调整 NSWindow frame 会触发 webview blur（query 变化、搜索结果展开时尤其明显——「打第一个字母即失焦」）；`apply` effect 在 resize 完成后若 `activeElement !== inputRef` 则重新 `inputRef.focus()`，保证连续输入不中断

**快捷键 toggle**：
- 窗口已可见时再按热键 → 隐藏（等同 Escape，走 `hide_action_bar_window` 统一收口，非裸 `win.hide()`）
- 不可见时按热键 → 正常触发

**dismiss 触发路径 + reason 诊断**（M4）：
- `action_bar_dismiss(reason)` 命令的 `reason` 仅用于日志诊断（`click-outside` / `focus-lost` / `launch-app` / `open-file` / `open-url` / `execute-shell` / `escape`），后端对所有 reason 走相同 `hide_action_bar_window` + `finalize_action_bar`
- 所有 dismiss 调用点必须传 reason——否则日志记 `None`，无法区分触发来源

**focus-lost 500ms 宽限**（M5）：
- show 后 500ms 内的 `onFocusChanged(focused=false)` 被宽限跳过（防 app 激活/窗口成 key 的时序抖动产生的 spurious focus-lost）
- 此期间 Escape 仍可 dismiss（不经 onFocusChanged）；click-outside 走 document click 独立路径
- 代价：show 后 500ms 内点其他 app 无法立即关闭（需等 500ms 或按 Escape）

**焦点时序（Sublime 间歇性失焦根因修复）**：
- **gather_context 同步化**：`trigger_action_bar` 文本分支在 show **之前**同步采集上下文。原异步方案（浮窗先弹 → 后台 gather）中，gather 调用前台 app（Sublime `subl --command` / Browser osascript）会激活前台 app，异步在 show 之后抢走 ActionBar 焦点——对照实验铁证：无选中（不 gather）→ 正常获焦；有选中（gather）→ 失焦。移到 show 之前，由随后的 `show` + `set_focus` 统一夺回（最后 `set_focus` 者持有）。附带收益：show 前前台确定是源 app，`frontmost_app()` 读上下文更准。代价：热键到弹出增加 gather 耗时。
- **show 后焦点探针 + 巩固**：`show` 后起诊断线程 150/350ms 读 `isKeyWindow`（objc2 `msg_send!`，窗口级，比 app 级 `isActive` 准）；150ms 若已失焦则 `set_focus` 巩固夺回——覆盖 Sublime 延迟激活窗口。
- **onFocusChanged 宽限**：show 后 500ms 内的 spurious focus-lost 不触发 dismiss（`showTimeRef` 记录 show 时刻，app 激活 / 窗口成 key 的时序抖动期不误关）。
- **dismiss 诊断**：`action_bar_dismiss` 携带 `reason`（`focus-lost` / `click-outside` / 操作后）+ 日志，定位失焦触发来源。

### 4.1 应用启动
- `launch_app(path)` → `open <path>` (status，非 spawn)

### 4.2 菜单项触发
- 等价于点击菜单项，走现有 `executeItem` 逻辑

### 4.3 Quicklinks
- 关键词触发：输入 `tr hello` → 匹配 trigger_keyword="tr" → 替换 `{query}` → `open_url`
- 非关键词匹配：前端替换 `{query}`/`{text}` 占位符（用 context.text 或 query）
- `open_url(url)` → `open <url>` (status)

### 4.4 文件
- `open_file(path)` → `open <path>` (status)

### 4.5 书签
- `open_url(url)` → 默认浏览器打开

### 4.6 Shell
- `execute_shell(command)` → `sh -c "<command>"`
- 30s 超时 + `kill_on_drop(true)` + 100KB 字符截断（`chars().take()`，非字节切片）

## 5. Run And Paste

### 5.1 设计
`auto_paste=true` 的 AI/翻译/脚本项执行后跳过 CompactEditor，直接写剪贴板 + 模拟 ⌘V 粘贴到光标。

### 5.2 实现
- `action_bar_items.auto_paste INTEGER DEFAULT 0`
- `execute_action_bar_inner` 三处调用点（translate LLM / AI / script）检查 `item.auto_paste`
- `action_bar_run_and_paste(result, app)` → 写剪贴板 + 100ms 后 `paste::paste`
- 设置页 `autoPaste` 开关（仅 AI/Script 类型），新建和编辑时都通过 `set_auto_paste` 命令更新

### 5.3 麦克风不可用提示
`audio.start()` 失败时 emit `"mic-error"` 事件 + 弹结果窗 + 红色 toast 气泡（5s）。

## 6. 技术实现

### 6.1 架构：detect_selection 与路由分离

```
detect_selection() ── 唯一感知 changeCount 的地方
  ├─ Finder → AppleScript → Selection::File/Folder {files, parent_dir, mouse}
  ├─ Sublime → 插件 sel_start/sel_end → Selection::Text / None（绕过 Cmd+C）
  └─ 其他 → Cmd+C + changeCount → Selection::Text {text, mouse} / None
  返回：Selection 枚举（携带选中内容 + 鼠标坐标 + meta）

trigger_action_bar() ── 纯路由
  match &sel:
    None   → centered（主屏 1/5）
    Text   → at_mouse(sel.mouse) + context gather
    File   → at_mouse(sel.mouse)
    Folder → at_mouse(sel.mouse)
```

**Sublime 选区检测（绕过 Cmd+C 的 copy_with_empty_selection 陷阱）**：Sublime 4 默认 `copy_with_empty_selection: true`——无选中时 Cmd+C 复制当前行，导致 changeCount +1 且剪贴板有当前行内容，changeCount 方案误判"有选中"。detect 对 Sublime 走插件 `get_sublime_selection`：用 `subl --command octopus_export_context` 触发插件导出 `sel_start/sel_end`，`sel_start == sel_end` 即无选中（精确，不依赖 Cmd+C）。副作用：detect 阶段调 `subl --command` 会激活 Sublime，但 gather_context 对 Sublime 本就要调此命令，故无额外副作用。

**changeCount 污染防护**：detect 恢复剪贴板（write_files/set_image/write_text）自身会递增 changeCount。用全局 `CHANGE_COUNT_BASELINE: AtomicI64` 记录上次 detect 结束时的 changeCount，下次 detect 的 `before = max(实时读, baseline)`，所有退出路径（None/Text）恢复后更新 baseline。隔离恢复写入对下次检测的污染（防"无选中误判 Some"）。

**show 事件携带 context payload**：`show_action_bar_window` emit `action-bar://show` 时携带 `snapshot_pending_context()` 的 context。前端 refresh 优先用事件 payload（零延迟），mount 首次走 invoke 兜底。消除首屏竞态——原 invoke(get_context) 异步 Promise pending 期间，窗口已 show 但 context state 仍是陈旧值，导致"有选中却只显示输入框"。

### 6.2 前端模块

| 文件 | 职责 |
|------|------|
| `searchTypes.ts` | SearchResult/TabId/TABS/ExpandDirection 等类型 + 常量 |
| `searchLogic.ts` | 16 个纯逻辑函数（展开方向/Tab 循环/结果合并/过滤/高度计算/focusLayer 切换等） |
| `searchLogic.test.ts` | 59 个单元测试 |
| `SearchPanel.tsx` | Tab 栏 + 结果列表组件 |
| `index.tsx` | 集成：搜索输入框 + 条件渲染 + 窗口动态调整 + 键盘导航 |

### 6.3 后端 Tauri 命令

```rust
search_all(query, tab) → Vec<SearchResult>    // 综合搜索（入口 trim query）
launch_app(path) → ()                          // status (非 spawn)
open_file(path) → ()                           // status
open_url(url) → ()                             // status
execute_shell(command) → String                // 30s 超时 + kill_on_drop + 100KB 截断
set_auto_paste(id, auto_paste) → ()            // 零行检查
reindex_apps() → usize                         // 诊断命令：强制重扫 + 刷新内存索引 + DB（后台自动扫描的 fallback）
action_bar_dismiss(reason) → ()                // reason 仅诊断（click-outside/focus-lost/launch-app/...）
```

### 6.4 独立 crate `octopus-search`

不依赖 Tauri，可独立测试。模块：`matcher` + `app_index` + `file_search` + `bookmark` + `engine`。

**Chromium 书签解析**：遍历 `roots` 对象的每个 folder value（bookmark_bar/other/synced）递归 walk children——非直接 walk(roots)。

**Safari 书签**：`load_safari_bookmarks` **未实现**（返回空 Vec）——解析需引入 `plist` crate + Full Disk Access。接口占位已就绪，未来填充函数体即可。

**osascript / 子进程超时**：gather_context 的 fallback 链（Pages osascript / lsof / pdftotext / officecli / mdfind / subl）统一经 `run_command_with_deadline(cmd, deadline)` 执行——spawn 后轮询到 `AX_TIMEOUT`（500ms），超时 `kill` + `wait` 回收，防权限对话框/无响应进程永久卡死 trigger worker。Finder selection 检测的 osascript 经子线程 + `recv_timeout(5s)`。

## 7. DB 变更

### 7.1 v31→v32

```sql
ALTER TABLE action_bar_items ADD COLUMN trigger_keyword TEXT NOT NULL DEFAULT '';
ALTER TABLE action_bar_items ADD COLUMN auto_paste INTEGER NOT NULL DEFAULT 0;
```

`insert_action_bar_item` 和 `update_action_bar_item` 全链路传入这两个字段 + `is_enabled` 参数（新建不再硬编码 1）。

### 7.2 v32→v33→v34：app_index 缓存表

```sql
CREATE TABLE IF NOT EXISTS app_index (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,           -- file_stem（英文名，如 WeChat）
    alias      TEXT NOT NULL DEFAULT '', -- 本地化名（如 微信），空=无别名
    path       TEXT NOT NULL UNIQUE,    -- .app 绝对路径
    icon       TEXT NOT NULL DEFAULT '', -- base64 PNG 32×32，空=无图标
    indexed_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- 首次启动扫文件系统 + 写缓存；后续启动直接读 DB（<1ms）
- **后台 mtime 轮询**：启动 30s 后每 10 分钟检测 `/Applications` 等目录 mtime，变化时后台重扫 + 刷新内存索引（`SearchEngine.app_index` 的 `RwLock`）+ 写 DB
- `reindex_apps` 命令作诊断/强制重扫 fallback（后台扫描失效时手动兜底）
- `scan()` 检测旧缓存 icon 全空时自动重扫

## 8. 性能预算

| 操作 | 预算 |
|------|------|
| 按键到结果显示 | <16ms（即时搜索） |
| 文件搜索（mdfind） | <200ms（防抖后），受 gather deadline 约束 |
| 应用索引加载（启动，DB 缓存） | <1ms（DB 非空时） |
| 应用索引全量扫描（DB 空 / 后台 mtime 变化触发） | ~5-8s（~90 app × ~50-80ms/app，含 sips/defaults 子进程；后台异步不阻塞 UI） |
| 书签解析（启动时） | <100ms |
| gather_context（trigger 热键→show） | <500ms（`AX_TIMEOUT`；子进程超时 kill） |

## 9. 不变量

1. 无选中（context=null）→ 仅搜索框，不显示菜单条
2. 有选中 + 输入框为空 → 菜单条显示
3. 输入框有内容 → 搜索结果替代菜单条
4. 搜索结果最多 10 行可见
5. 无结果时保留 1 行高度（显示"无结果"提示，不被 0 高度裁剪）
6. Run And Paste 结果直接粘贴到原光标位置
7. LLM 调用用 `spawn_blocking`（防阻塞 tokio worker）
8. 所有子进程（open/mdfind/sips/osascript/pdftotext/lsof/officecli/subl）有超时 + kill/防孤儿——gather 链统一经 `run_command_with_deadline`
9. 结果列表 hover 选中只在鼠标真正移动时生效（不受 React 重渲染 / 键盘选中影响）
10. 窗口 resize 串行化（generation token），防 Tab 快速切换异步乱序
11. URL scheme 仅放行 http/https（选中文本即 URL 时）
12. **应用索引内存与 DB 一致性**：后台扫描刷新内存 `RwLock<AppIndex>` 与 DB 缓存同步，搜索走读锁零阻塞
13. **save_app_index 原子性**：DELETE + INSERT 在同一事务内，中途失败回滚 DELETE，不丢索引
14. **executeItem submenu 焦点契约**：展开 submenu 后 focusLayer 必为 `sub`（`nextFocusLayerAfterExecute` 守护），Enter 执行子项而非重复展开父项
15. **trigger guard 统一收口**：`trigger_action_bar` 的 None/Text/File/Folder 所有分支在 match 后统一 `finalize_action_bar`，不依赖用户后续操作清 guard
16. **changeCount 基准隔离**：`CHANGE_COUNT_BASELINE` 记录上次 detect 结束时的 changeCount，下次 `before = max(实时读, baseline)`——隔离恢复剪贴板写入对 changeCount 判定的污染
17. **show 事件携带 context**：`action-bar://show` emit 时携带 context payload，前端首屏渲染用 payload 而非异步 invoke（消除首屏竞态，防"有选中却只显示输入框"）
18. **Sublime 选区精确判定**：detect 对 Sublime 走插件 `sel_start/sel_end`（不靠 Cmd+C），绕过 Sublime 4 `copy_with_empty_selection` 导致的"无选中复制当前行"陷阱——Cmd+C 方案对该设置根本失效
- osascript 超时 → 返回 None（非 Finder 视为无选中）

## 11. 安全约束与边界处理

- **URL scheme 白名单**：选中文本即 URL 时（`item.action_data` 为空的内置「在浏览器打开」项）仅放行 `http://`/`https://`，其余 scheme 统一补 `https://`——防 `smb://`/`file:///`/`vnc://` 等通过选中不可信文本触发系统级操作（挂载共享致 NTLM 凭据泄露 / Finder 打开任意路径 / 屏幕共享）。用户配置的 url 模板（`action_data` 非空）走 `{text}` 替换 + `url_encode_param` 编码，不受此约束。
- **query trim**：`search_all` 入口 `query.trim()` 一次，覆盖所有 tab 路径——防前导/尾部空格（粘贴 / IME 残留）致 exact/prefix 匹配失败。
- **无结果保留高度**：`calcResultsHeight(0)` 返回 1 行高度（36px）而非 0——保证「无结果」提示可见，不被 0 高度容器 overflow 裁剪。
- **accepts=any 无选中可执行**：`accepts="any"` 的项无选中（context=null）时仍可执行（text 用空串）——`executeItem` 不再 `if(!ctx) return`；由 `contextFilteredResults` 保证无选中时仅 any 项进入搜索结果。

## 12. URL 检测（urlDetect.ts）

选中文本时前端 `detectActionUrl` 判定是否为 URL，是则显示「在浏览器打开」项（`actionType='url'`、`actionData=''`），执行时后端走 §11 的 URL scheme 白名单分支（`item.action_data` 为空 → 用选中文本作 URL）。

**三条检测路径**（宽松设计，源码注释明示「比剪贴板 detectUrl 更宽松」）：
1. `localhost`（`LOCALHOST_RE`）→ `http://localhost…`
2. IPv4（`IPV4_RE = /^\d{1,3}\.…/`）→ `http://<ip>`
3. 域名（含 `.` 且不以点开头/结尾，且点两侧至少一侧含字母）→ `https://<domain>`

**边界处理（已修）**：
- **文件名不误判**：`FILE_EXT_RE` 命中常见文件扩展名（`readme.md`/`photo.jpg`/`data.csv` 等）时不判为 URL。
- **IPv4 范围校验**：`isValidIpv4Host` 补 0-255 校验，剔 `999.999.999.999` 等无效 IP。

后端 §11 URL scheme 白名单双保险：即使检测漏过，非 http/https 统一补 `https://`，不会触发系统级操作。
