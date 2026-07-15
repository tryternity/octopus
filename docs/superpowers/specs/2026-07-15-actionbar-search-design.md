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
│ [? 全部] [a 应用] [f 文件] [> Shell] [b 书签] │ ← Tab 栏（有搜索时显示）
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
│ [? 全部] [a 应用] [f 文件] [> Shell] [b 书签] │ ← Tab 栏
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
| **应用** | `/Applications/`、`~/Applications/`、`/System/Applications/`、`/Applications/Utilities/` 下的 `.app` | 启动时 | 内存索引，即时（跨目录同名去重） |
| **菜单项** | DB `action_bar_items` 表 | 实时读 DB | 内存索引，即时 |
| **Quicklinks** | DB `action_bar_items` WHERE `action_type='url'` AND 有 `trigger_keyword` | 实时读 DB | 内存索引，即时 |
| **文件** | mdfind（Spotlight metadata） | 实时查 | 防抖 150ms，10s 超时 + `kill_on_drop` |
| **书签** | Chrome/Edge（JSON）| 启动时 | 防抖 150ms |

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

引入 `nucleo-matcher` crate。

**匹配优先级**（取最高分）：
1. **精确匹配**（query == target，忽略大小写）→ 10000
2. **前缀匹配**（target 以 query 开头）→ `5000 - remaining`（越短分越高；remaining 按 **char count** 计算，非 byte len——CJK 3 字节/char，byte 算法会系统性压低中文排名）
3. **拼音首字母**（query 全 ASCII 时匹配中文菜单项硬编码首字母）→ `3000 - remaining`（remaining 同样按 char count）
4. **模糊匹配**（nucleo）→ 按匹配度评分

**结果合并排序**：即时结果与延迟结果合并后全局按 score 降序排序（`mergeResults` 含 `source:title:subtitle` 去重）。

### 3.4 结果分组与 Tab

Tab 页：`[? 全部] [a 应用] [f 文件] [> Shell] [b 书签]`

- `[? 全部]`：混合展示所有来源结果，按优先级排序
- `[a 应用]`：仅 source === "app"
- `[f 文件]`：仅 source === "file"
- `[> Shell]`：`>` 前缀路由的 shell 命令
- `[b 书签]`：仅 source === "bookmark"

**菜单项 accepts 过滤**：搜索结果中的 menu/quicklink 来源按 `context.accepts` 过滤。无选中（context=null）时仅显示 `accepts="any"` 的项。

### 3.5 键盘导航

| 当前焦点 | 按键 | 行为 |
|---------|------|------|
| 搜索框 | `Tab` | 焦点跳到结果区，选中第一个 |
| 搜索框 | `↑↓` | 焦点跳到结果区首/末项 |
| 搜索框 | `Enter` | 执行第一个结果 |
| 结果区 | `Tab` | 循环切换 Tab 页 |
| 结果区 | `Shift+Tab` | 反向循环 Tab 页 |
| 结果区 | `?` `a` `f` `>` `b` | 跳到对应 Tab |
| 结果区 | `i` | 焦点回搜索框 |
| 结果区 | `↑↓` | 在结果项间导航 |
| 结果区 | `Enter` | 执行选中项 |
| 任意 | `Escape` | 有查询→清空；无查询→dismiss |

> **IME 组合**（`e.isComposing`）期间放行所有按键——Enter 是确认候选词，不应触发搜索执行。
> **loading 视图** Escape 仍生效（auto_translate 无超时，防卡住困死用户）；其他导航键在 loading 时屏蔽。

## 4. 搜索结果执行

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
5. Run And Paste 结果直接粘贴到原光标位置
6. LLM 调用用 `spawn_blocking`（防阻塞 tokio worker）

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
