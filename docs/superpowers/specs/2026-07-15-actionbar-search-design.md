# ActionBar 搜索功能设计

> 2026-07-15 · ActionBar 集成搜索输入框 + 应用启动 + 文件搜索 + Quicklinks + 书签搜索 + Silent Hotkey + Run And Paste

## 1. 设计目标

将 ActionBar 从纯菜单条升级为**搜索驱动的命令面板**：用户可在搜索输入框中搜索应用、文件、菜单项、Quicklinks、书签，直接触达执行。

## 2. 面板布局

### 2.1 状态切换

| 状态 | 输入框 | Tab+结果 | 菜单条 |
|------|--------|---------|--------|
| 未选中（无文本/文件） | ✅ 显示，聚焦 | ✅ 有输入时显示 | ❌ 隐藏 |
| 选中，搜索框为空 | ✅ 显示，未聚焦 | ❌ 隐藏 | ✅ 显示 |
| 选中，搜索框有输入 | ✅ 显示，聚焦 | ✅ 显示 | ❌ 隐藏（被 Tab+结果覆盖） |

### 2.2 向下展开（输入框下方空间充足）

```
┌─────────────────────────────────────────┐
│ [搜索输入框]                              │ ← 始终在顶部
├─────────────────────────────────────────┤
│ [全部] [应用 ℓ] [文件 ⌘] [> Shell] [? 书签] │ ← Tab 栏（有搜索时显示）
│  结果1                                    │
│  结果2                                    │ ← 最多10行
│  ...                                     │    无结果区域透明+穿透
│  结果N                                    │
├─────────────────────────────────────────┤
│ [翻译] [搜索] [AI] [脚本] [复制路径]     │ ← 菜单条（搜索框为空时显示）
└─────────────────────────────────────────┘
```

### 2.3 向上展开（输入框下方空间不足）

```
│  结果N                                    │
│  ...                                     │
│  结果2                                    │ ← 最多10行
│  结果1                                    │    无结果区域透明+穿透
│ [全部] [应用 ℓ] [文件 ⌘] [> Shell] [? 书签] │ ← Tab 栏
├─────────────────────────────────────────┤
│ [搜索输入框]                              │ ← 始终在核心位置
├─────────────────────────────────────────┤
│ [翻译] [搜索] [AI] [脚本] [复制路径]     │ ← 菜单条
└─────────────────────────────────────────┘
```

### 2.4 展开方向判定

- 获取 ActionBar 输入框的屏幕 Y 坐标
- 屏幕高度 - Y > 阈值（如 400px）→ 向下展开
- 否则 → 向上展开
- 无搜索结果时，结果区域透明 + 鼠标穿透（不遮挡下方内容）

## 3. 搜索引擎

### 3.1 搜索来源

| 来源 | 数据 | 索引时机 | 搜索方式 |
|------|------|---------|---------|
| **应用** | `/Applications/`、`~/Applications/`、`/System/Applications/` 下的 `.app` | 启动时 + 增量 | 内存索引，即时 |
| **菜单项** | DB `action_bar_items` 表 | 实时读 DB | 内存索引，即时 |
| **Quicklinks** | DB `action_bar_items` WHERE `action_type='url'` AND 有 `trigger_keyword` | 实时读 DB | 内存索引，即时 |
| **文件** | mdfind（Spotlight metadata） | 实时查 | 防抖 150ms |
| **书签** | Safari/Chrome/Edge/Firefox 书签文件 | 启动时 | 防抖 150ms |

### 3.2 搜索策略

```
输入框内容变化（防抖 50ms）
  ├── 即时搜索（<1ms）：应用 + 菜单项 + Quicklinks
  └── 延迟搜索（150ms 防抖）：文件（mdfind）+ 书签
       └── 输入长度 ≥ 2 字符时才触发延迟搜索
```

### 3.3 匹配算法

引入 `nucleo-matcher` crate（Rust 高性能模糊匹配库）。

**匹配优先级**：
1. **精确匹配**（query == title）→ 最高分
2. **前缀匹配**（title starts with query）→ 高分
3. **模糊匹配**（nucleo fuzzy match）→ 按匹配度评分
4. **拼音首字母**（query 为 ASCII 时匹配中文菜单项的拼音首字母）

**同级别排序**：应用 > 文件 > Shell > 其他（菜单/Quicklinks/书签）

### 3.4 结果分组与 Tab

Tab 页：`[全部] [应用] [文件] [> Shell] [? 书签]`

- `[全部]`：混合展示所有来源结果，按优先级排序
- `[应用]`：仅应用结果
- `[文件]`：仅文件结果
- `[> Shell]`：`>` 前缀路由的 shell 命令（输入 `>` 时自动切到此 Tab）
- `[? 书签]`：仅书签结果

**Tab 切换**：
- `Tab` 键循环切换
- 输入 `>` 自动切换到 Shell Tab
- Tab 栏上标注快捷键提示（如 `ℓ`、`⌘`、`>`、`?`）

### 3.5 结果项格式

```
┌──────────────────────────────────────────────┐
│ [图标] Chrome                    Google Chrome │
│        [路径/副标题/类型标签]                   │
└──────────────────────────────────────────────┘
```

- 左：图标（应用图标 / 文件类型图标 / 菜单项 emoji）
- 中：标题（应用名 / 文件名 / 菜单标题）
- 右：副标题（bundle name / 文件路径 / action_type 标签）

## 4. 搜索结果执行

### 4.1 应用启动

- 选中应用结果 → `Enter` → `NSWorkspace::launchApplication` 或 `open -a <app>`
- `⌘↵` → 在 Finder 中显示

### 4.2 菜单项触发

- 选中菜单项结果 → `Enter` → 执行该 `action_bar_items` 行（等价于点菜单项）
- 与现有 action bar 菜单执行逻辑完全一致

### 4.3 Quicklinks

- 输入 `tr hello` → 匹配 Quicklink `tr`（trigger_keyword）→ 替换 `{query}` → 浏览器打开 `https://translate.google.com/?text=hello`
- 也可从 `[全部]` Tab 选中 Quicklink 结果 → `Enter` 打开

### 4.4 文件

- 选中文件结果 → `Enter` → `open <file>`
- `⌘↵` → 在 Finder 中显示

### 4.5 书签

- 选中书签结果 → `Enter` → 默认浏览器打开 URL

### 4.6 Shell

- Shell Tab 中输入命令 → `Enter` → 执行 `sh -c "<command>"`
- 结果显示在结果列表区域（临时替换为终端输出）

## 5. Silent Query Hotkey + Run And Paste

### 5.1 设计

菜单项绑全局热键后，支持两种模式：
- **默认模式**：选中内容 → 按热键 → 弹面板显示结果 → 用户确认
- **Run And Paste 模式**：选中内容 → 按热键 → 直接执行 → 结果粘贴回光标位置

### 5.2 确认机制

首次使用某个 Silent Hotkey 时：
1. 执行命令（如翻译）
2. 结果显示在浮窗面板
3. 面板底部提示：「下次直接粘贴，不再确认？」+ `[记住选择]` 按钮
4. 用户点「记住选择」→ DB 记录该菜单项 `auto_paste=true`
5. 后续按该热键 → 直接执行 + 粘贴，不弹面板

### 5.3 实现

- `action_bar_items` 表加 `auto_paste INTEGER DEFAULT 0` 字段
- 全局热键 handler：
  ```rust
  if item.auto_paste {
      // Run And Paste：执行 → 写剪贴板 → 模拟 ⌘V
  } else {
      // 弹面板显示结果 + 确认按钮
  }
  ```
- 粘贴逻辑复用现有 coordinator 的 paste 实现（写剪贴板 + CGEvent post ⌘V）

## 6. 技术实现

### 6.1 前端

- ActionBar 组件重构：加搜索输入框 + Tab 栏 + 结果列表
- 结果列表虚拟滚动（最多 10 行可见，更多滚动）
- 输入框防抖 → Tauri command 搜索 → 结果渲染
- Tab 切换键盘导航

### 6.2 后端（Rust）

新增 Tauri 命令：

```rust
#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<SearchResults, String>
// 返回 { apps: [...], files: [...], menus: [...], bookmarks: [...], quicklinks: [...] }

#[tauri::command]
pub fn launch_app(app_path: String) -> Result<(), String>

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String>

#[tauri::command]
pub fn execute_shell(command: String) -> Result<String, String>
```

### 6.3 应用索引

```rust
pub struct AppIndex {
    apps: Vec<AppEntry>,  // 启动时扫描，增量更新
}

pub struct AppEntry {
    name: String,          // "Google Chrome"
    bundle_name: String,   // "Chrome"
    path: PathBuf,         // /Applications/Google Chrome.app
    icon: Vec<u8>,         // 预提取的图标 PNG
}
```

扫描路径：
- `/Applications/`
- `~/Applications/`
- `/System/Applications/`
- `/Applications/Utilities/`

### 6.4 文件搜索

```rust
pub async fn search_files(query: &str) -> Vec<FileEntry> {
    // mdfind -name <query> -onlyin ~ 2>/dev/null
    // 限制结果数 10 条
}
```

### 6.5 书签搜索

```rust
pub struct BookmarkIndex {
    bookmarks: Vec<BookmarkEntry>,
}

pub struct BookmarkEntry {
    title: String,
    url: String,
    browser: String,  // "safari" / "chrome" / "edge" / "firefox"
}
```

读取路径：
- Safari: `~/Library/Safari/Bookmarks.plist`（需 Full Disk Access）
- Chrome: `~/Library/Application Support/Google/Chrome/Default/Bookmarks`
- Edge: `~/Library/Application Support/Microsoft Edge/Default/Bookmarks`
- Firefox: `~/Library/Application Support/Firefox/Profiles/*/places.sqlite`

### 6.6 模糊匹配

使用 `nucleo-matcher` crate：
```rust
use nucleo_matcher::{Matcher, Config, pattern::{Pattern, CaseMatching, Normalization}};

let mut matcher = Matcher::new(Config::DEFAULT);
let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
let score = pattern.score(Utf32Str::Ascii(title.into()), &mut matcher);
```

## 7. DB 变更

### 7.1 action_bar_items 新增字段

```sql
ALTER TABLE action_bar_items ADD COLUMN trigger_keyword TEXT NOT NULL DEFAULT '';
-- Quicklinks 关键词触发（如 'tr'），空=不参与搜索框关键词匹配
ALTER TABLE action_bar_items ADD COLUMN auto_paste INTEGER NOT NULL DEFAULT 0;
-- Silent Hotkey Run And Paste 模式（0=弹面板确认，1=直接粘贴）
```

### 7.2 搜索索引缓存表（可选）

```sql
CREATE TABLE IF NOT EXISTS search_index (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT NOT NULL,   -- 'app' | 'file' | 'bookmark' | 'menu'
    title       TEXT NOT NULL,
    subtitle    TEXT NOT NULL DEFAULT '',
    action_data TEXT NOT NULL,   -- JSON: { path/url/action_type/... }
    pinyin      TEXT NOT NULL DEFAULT '',  -- 预计算拼音首字母
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

应用/书签索引可缓存到此表，避免每次启动重新扫描。文件搜索走 mdfind 不缓存。

## 8. 性能预算

| 操作 | 预算 |
|------|------|
| 按键到结果显示 | <16ms（即时搜索） |
| 文件搜索（mdfind） | <200ms（防抖后） |
| 应用索引扫描（启动时） | <500ms |
| 书签解析（启动时） | <100ms |
| 内存占用（索引） | <5MB |

## 9. 不变量

1. 输入框为空时 → 菜单条显示（与现有行为一致）
2. 输入框有内容时 → 搜索结果替代菜单条
3. 搜索结果最多 10 行可见，超出滚动
4. 无搜索结果时结果区域透明 + 穿透
5. Silent Hotkey 不弹 ActionBar 面板（除非需要确认）
6. Run And Paste 结果直接粘贴到原光标位置

## 10. 降级

- mdfind 不可用（非 macOS / 权限不足）→ 文件搜索 Tab 隐藏
- 书签文件读取失败 → 书签 Tab 隐藏
- 应用索引扫描失败 → 应用 Tab 隐藏，不影响其他搜索
- nucleo-matcher 编译失败 → 回退到简单的 `contains` 匹配
