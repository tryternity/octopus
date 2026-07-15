# ActionBar 搜索功能设计

> 2026-07-15 · ActionBar 集成搜索输入框 + 应用启动 + 文件搜索 + Quicklinks + 书签搜索 + Run And Paste
>
> **实现完成** — 本文档已同步实际代码（2026-07-15，含 9 轮 code review 修复）

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
│ [a 全部] [p 应用] [f 文件] [s Shell] [b 书签] │ ← Tab 栏（有搜索时显示）
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
│ [a 全部] [p 应用] [f 文件] [s Shell] [b 书签] │ ← Tab 栏
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
- 应用索引缓存在 DB `app_index` 表（v34），启动时 <1ms 加载；DB 为空时扫文件系统 + 写缓存
- `reindex_apps` 命令供安装/卸载应用后刷新缓存

### 3.4 结果分组与 Tab

Tab 页：`[a 全部] [p 应用] [f 文件] [s Shell] [b 书签]`

- `[a 全部]`：混合展示所有来源结果，按优先级排序
- `[p 应用]`：仅 source === "app"
- `[f 文件]`：仅 source === "file"
- `[s Shell]`：`>` 前缀路由的 shell 命令
- `[b 书签]`：仅 source === "bookmark"

**菜单项 accepts 过滤**：搜索结果中的 menu/quicklink 来源按 `context.accepts` 过滤。无选中（context=null）时仅显示 `accepts="any"` 的项。

### 3.5 键盘导航

| 当前焦点 | 按键 | 行为 |
|---------|------|------|
| 搜索框 | `Tab` | 焦点跳到结果区，选中第一个 |
| 搜索框 | `↑↓` | 焦点跳到结果区首/末项 |
| 搜索框 | `Enter` | 执行第一个结果 |
| 结果区 | `Tab` | 循环：Tab 页之间 → 最后一个 Tab 正向 Tab 回搜索框 |
| 结果区 | `Shift+Tab` | 反向：Tab 页之间 → 第一个 Tab 反向 Tab 回搜索框 |
| 结果区 | `a` `p` `f` `s` `b` | 跳到对应 Tab |
| 结果区 | `i` | 焦点回搜索框（input focus 恢复） |
| 结果区 | `↑↓` | 在结果项间导航 |
| 结果区 | `Enter` | 执行选中项 |
| 任意 | `Escape` | 有查询→清空；无查询→dismiss；**loading 视图也生效** |
| 快捷键 | 再按热键 | 窗口已可见 → 隐藏（toggle 语义） |

**IME 处理（Enter 键）**：
- macOS 事件序列：IME 选词 = `keydown(keyCode=229)` → `compositionend` → `keydown(Enter, 13)`
- 纯英文 Enter = `keydown(Enter, 13)`，前面没有 229
- 实现：window keydown handler 记录 keyCode 229 的时间戳，Enter(13) 在 229 后 500ms 内 → 跳过（选词确认），否则正常执行
- **不依赖** `isComposing`（window 级时序不可靠）和 `compositionend`（WKWebView 空组合会误触发）
- **DOM focus 策略**：input 永远不 blur——`searchFocusZone` 通过同步 ref（非 useEffect 异步更新）控制 window handler 路由。结果区时设 `input.readOnly = true` 防 IME 捕获字母键；回 input 时解除 readOnly。这样 Tab 循环和字母快捷键都能正常工作

## 4. 搜索结果执行

### 4.0 UI 交互规则

**鼠标 hover 选中**：
- `onMouseMove` 内比较 `clientX/clientY` 与上次记录（<1px 容差）+ 结果/Tab 变化后 200ms 抑制窗口
- 只有鼠标**真的移动了**才改变选中——React 重渲染导致 DOM 重建时浏览器自动触发的 mousemove 不影响选中
- 结果列表变化时 `searchSelectedIdx` 重置为 0（用户输入时焦点在搜索框，选中应始终是第一个）

**窗口 resize 序列化**：
- `setSize` + `setPosition` 用 generation token 保证只有最新一次 resize 生效
- 两者串行 await——防快速 Tab 切换时异步乱序导致输入框被覆盖
- TabBar 固定 `h-[30px]`，Tab 按钮用 `transition-colors`（非 `transition-all`）——防 active 切换时尺寸微变导致布局晃动

**快捷键 toggle**：
- 窗口已可见时再按热键 → 隐藏（等同 Escape）
- 不可见时按热键 → 正常触发

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
  └─ 非 Finder → Cmd+C + changeCount → Selection::Text {text, mouse} / None
  返回：Selection 枚举（携带选中内容 + 鼠标坐标 + meta）

trigger_action_bar() ── 纯路由
  match &sel:
    None   → centered（主屏 1/5）
    Text   → at_mouse(sel.mouse) + context gather
    File   → at_mouse(sel.mouse)
    Folder → at_mouse(sel.mouse)
```

### 6.2 前端模块

| 文件 | 职责 |
|------|------|
| `searchTypes.ts` | SearchResult/TabId/TABS/ExpandDirection 等类型 + 常量 |
| `searchLogic.ts` | 15 个纯逻辑函数（展开方向/Tab 循环/结果合并/过滤/高度计算等） |
| `searchLogic.test.ts` | 56 个单元测试 |
| `SearchPanel.tsx` | Tab 栏 + 结果列表组件 |
| `index.tsx` | 集成：搜索输入框 + 条件渲染 + 窗口动态调整 + 键盘导航 |

### 6.3 后端 Tauri 命令

```rust
search_all(query, tab) → Vec<SearchResult>    // 综合搜索
launch_app(path) → ()                          // status (非 spawn)
open_file(path) → ()                           // status
open_url(url) → ()                             // status
execute_shell(command) → String                // 30s 超时 + kill_on_drop + 100KB 截断
set_auto_paste(id, auto_paste) → ()            // 零行检查
```

### 6.4 独立 crate `octopus-search`

不依赖 Tauri，可独立测试。模块：`matcher` + `app_index` + `file_search` + `bookmark` + `engine`。

**Chromium 书签解析**：遍历 `roots` 对象的每个 folder value（bookmark_bar/other/synced）递归 walk children——非直接 walk(roots)。

**osascript 超时**：所有 osascript 调用通过子线程 + `recv_timeout(5s)`，防首次自动化权限对话框永久挂起。

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
- `reindex_apps` Tauri 命令供安装/卸载应用后刷新
- `scan()` 检测旧缓存 icon 全空时自动重扫

## 8. 性能预算

| 操作 | 预算 |
|------|------|
| 按键到结果显示 | <16ms（即时搜索） |
| 文件搜索（mdfind） | <200ms（防抖后），10s 超时 |
| 应用索引扫描（启动时） | <500ms |
| 书签解析（启动时） | <100ms |

## 9. 不变量

1. 无选中（context=null）→ 仅搜索框，不显示菜单条
2. 有选中 + 输入框为空 → 菜单条显示
3. 输入框有内容 → 搜索结果替代菜单条
4. 搜索结果最多 10 行可见
5. 无结果时保留 1 行高度（显示"无结果"提示，不被 0 高度裁剪）
6. Run And Paste 结果直接粘贴到原光标位置
7. LLM 调用用 `spawn_blocking`（防阻塞 tokio worker）
8. 所有子进程（open/mdfind/sips/osascript）有超时 + kill/防孤儿
9. 结果列表 hover 选中只在鼠标真正移动时生效（不受 React 重渲染影响）
10. 窗口 resize 串行化（generation token），防 Tab 快速切换异步乱序
11. URL scheme 仅放行 http/https（选中文本即 URL 时）

## 10. 降级

- mdfind 不可用 → 文件搜索返回空
- 书签文件读取失败 → 书签搜索返回空
- Safari 书签解析暂未实现（plist 解析需额外依赖）
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
