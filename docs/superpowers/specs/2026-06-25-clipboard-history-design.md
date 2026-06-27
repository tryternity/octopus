# 剪贴板历史管理功能设计

**日期**: 2026-06-25
**状态**: ✅ Phase 0-3 已实现 + Phase 3 后迭代（图片保存格式选择、FTS5 自动 rebuild、toolbar 精简、窗口高度调整）
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 0. 概述

为 octopus desktop 新增剪贴板历史管理功能：监听系统剪贴板变化，持久化历史记录，提供列表/搜索/过滤/收藏/清理能力。同时管理两类来源——普通剪贴板复制（`source=clipboard`）和语音识别输出（`source=asr`）。

现有 ASR 结果输出（paste.rs → tauri-plugin-clipboard-manager）一并迁移到新方案，统一底层库。

### 参考项目

| 项目 | 借鉴点 |
|---|---|
| [EcoPaste](https://github.com/EcoPasteHub/EcoPaste) | UI 交互模式（tab 过滤器、虚拟滚动列表、失焦隐藏窗口）、数据模型（type+group 双字段、图片文件存储）、窗口管理（NSPanel） |
| [Ortu](https://github.com/abhijith-p-subash/ortu) | FTS5 全文搜索、图片 SHA-256 去重 blob 表、清理策略（pin/分组豁免） |
| [clipboard-rs](https://github.com/ChurchTao/clipboard-rs) | 底层库——读写 + 监听一体，跨平台 |
| [wl-clipboard-rs](https://github.com/YaLTeR/wl-clipboard-rs) | Wayland 支持（clipboard-rs 的底层依赖，不直接使用） |

## 1. 架构

### 1.1 新增 crate：`octopus-clipboard`

独立 crate 承载剪贴板核心能力，仅依赖 `octopus-infra`（复用 DB 单例和路径约定），不依赖 Tauri。

```
crates/
├── infra/         ← octopus-infra（DB 单例、paths）
├── clipboard/     ← octopus-clipboard（新增）
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs           # 公开 API：start/stop/insert/query/delete/clear
│       ├── handle.rs        # ClipboardHandle: Mutex<ClipboardContext> + SUPPRESS_FLAG
│       ├── watcher.rs       # clipboard-rs 监听封装（后台线程 + callback）
│       ├── store.rs         # SQLite 读写（clipboard_history 表）
│       ├── model.rs         # ClipboardItem, ItemType, Source 等数据结构
│       ├── image.rs         # 图片处理: PNG 编码 + SHA-256 去重 + 缩略图 + blob 回收
│       └── cleanup.rs       # 自动清理（条数 + 天数 + 收藏豁免）
├── desktop/       ← octopus-desktop（UI 层调用 octopus-clipboard）
```

**依赖流向**：
```
infra ← clipboard ← desktop（默认启用）
```

### 1.2 库选型：clipboard-rs

替代原来的 `tauri-plugin-clipboard-manager` 和用户最初考虑的 `arboard`。

**选择理由**：
- 读写 + 监听一体——arboard 只有读写无监听，选 arboard 必须自建三平台监听层
- 跨平台监听机制完整覆盖（与方案 C 一致）：

| 平台 | 监听方式 | 延迟 | 底层 API |
|---|---|---|---|
| macOS | 轮询 changeCount | ~500ms | `NSPasteboard.changeCount`（整数对比，零拷贝） |
| Windows | 事件驱动 | 实时 | `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE` |
| Linux X11 | 事件驱动 | 实时 | XFixes `XfixesSelectionNotify` |
| Linux Wayland | 两级轮询 | ~500ms | ① MIME 类型列表变化 ② text 内容变化 |

- Wayland 支持：通过 `wayland` feature 启用 `wl-clipboard-rs`（ext-data-control / wlr-data-control 协议），运行时检测 `WAYLAND_DISPLAY`，失败自动 fallback X11
- EcoPaste 底层也是 clipboard-rs（经 tauri-plugin-clipboard-x 封装），验证了可行性

**与 arboard 的功能差异**：

| 特性 | clipboard-rs | arboard |
|---|---|---|
| Text / HTML / Image 读写 | ✅ | ✅ |
| RTF 读写 | ✅ | ❌ |
| Files 读写 | ✅ | ❌ |
| 变化监听 | ✅ 全平台 | ❌ 无 |
| Wayland | ✅ v0.3.4（feature） | ✅（feature） |

### 1.3 tauri-plugin-clipboard-manager 处理：完全替换

移除 `tauri-plugin-clipboard-manager`，paste.rs 迁移到 clipboard-rs。理由：
- 共存有 Windows 剪贴板锁竞争风险
- 功能被 clipboard-rs 完全覆盖（paste.rs 只用了 `write_text()` / `read_text()`）
- 消除重复依赖，统一 Wayland 行为

前端 `navigator.clipboard`（WebView 原生 API）不受影响。

### 1.4 前端技术栈：React + shadcn/ui

现有三个页面（overlay/result/settings）是纯 HTML + 内联 JS（共 1777 行 HTML + ~1375 行 JS），大量使用 `innerHTML` 字符串拼接 DOM。迁移到 React 统一所有四个页面（含新增剪贴板历史）。

| 项 | 选择 | 理由 |
|---|---|---|
| 框架 | React 18 + TypeScript | 生态最大，与 EcoPaste 一致便于参考 |
| 构建 | Vite 6 | Tauri 官方推荐 |
| 样式 | Tailwind CSS 4 | 跨平台样式一致性，CSS reset 消除平台默认样式差异 |
| UI 组件 | shadcn/ui（源码 copy-in） | 定制性和辨识度——组件源码在项目内，深度可改，不锁定运行时依赖 |
| 图标 | Lucide React | shadcn/ui 默认搭配 |
| 虚拟滚动 | `@tanstack/react-virtual` | ~3KB，轻量 |
| 表单 | react-hook-form + zod | settings 页面表单迁移用 |
| 暗色模式 | Tailwind `dark:` + CSS 变量 | 系统跟随 / 手动切换 |
| 状态管理 | React 内置（useState/useReducer） | 不引入额外状态库 |

**不选 Ant Design**：偏管理后台风格；**不选 Mantine**：开发效率有优势但定制性和辨识度不如 shadcn/ui。

React 编译为纯静态 HTML/CSS/JS，WebView 直接加载，**运行时不需要 Node，不增加打包体积**。

## 2. 数据模型与存储

### 2.1 类型判定优先级

clipboard-rs 监听只通知"有变化"，需主动判断类型。按固定优先级（与 EcoPaste 一致），只取最高优先级的一种：

```
on_clipboard_change()
  → available_formats() 判断类型
  → 按优先级处理：
      1. Files  (has ContentFormat::Files)   → file 类型
      2. Image  (has ContentFormat::Image)   → image 类型
      3. Text   (has ContentFormat::Text)    → text 类型，额外检测 is_rich
```

优先级 files > image > text：从文件管理器复制图片文件时，剪贴板同时有文件路径和图片预览，但用户意图是操作文件。截图（Cmd+Shift+3/4）只有内存像素数据，无文件 URL，落入 image 分支。

### 2.2 各类型数据流

**text**
```
变化 → has(Text) → get_text() → SHA-256(content) 去重
  → 已存在：更新 created_at（可选 autoSort）
  → 新内容：DB insert content=文本, search_text=文本, is_rich=has(Html||Rtf)
```

**image（含截图）**
```
变化 → has(Image) → get_image() → RustImageData { width, height }
  → to_png() → PNG bytes
  → 超过 40MB 跳过（内存保护）
  → SHA-256(png_bytes) → hash 去重
  → 已存在：更新 created_at，不重存文件
  → 新内容：
      a. 存原图：~/.octopus/clipboard_images/<hash>.png
      b. 生成缩略图 240×240（image crate resize Lanczos）：~/.octopus/clipboard_images/<hash>_thumb.png
      c. DB insert: content=hash, blob_hash=hash, width, height, has_thumbnail=1
```

截图场景：剪贴板持有内存像素（`NSImage`/`CF_DIB`），磁盘无文件 → `get_image()` 拿到像素 → `to_png()` 编码 → 落盘。与"复制图片文件"走不同分支（后者有 file URL，优先级命中 file）。

**file**
```
变化 → has(Files) → get_files() → Vec<String> (file:// URI)
  → 解析为 Vec<PathBuf>，过滤非法路径
  → 超过 50 个只记前 50 + file_count=实际数量
  → JSON.stringify(paths) → SHA-256 去重
  → DB insert: content=JSON(paths), file_count=N, search_text=paths.join(" ")
```

文件被删除/移动 → 渲染时 `Path::exists()` 检测，失效条目灰显，不自动删除记录。

### 2.3 DB Schema（一次性定完，覆盖所有类型）

```sql
CREATE TABLE IF NOT EXISTS clipboard_history (
    id                INTEGER PRIMARY KEY,       -- 毫秒戳（同 transcriptions 约定）
    item_type         TEXT    NOT NULL,          -- 'text' | 'image' | 'file'
    source            TEXT    NOT NULL,          -- 'clipboard' | 'asr'
    content           TEXT    NOT NULL,          -- text:文本; image:blob_hash; file:JSON路径数组
    search_text       TEXT,                      -- FTS5 索引文本（text=内容; image=NULL; file=路径拼接）
    is_favorite       INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT    NOT NULL,

    -- image 元数据
    blob_hash         TEXT,                      -- SHA-256(PNG bytes)，去重 + 文件引用
    width             INTEGER,
    height            INTEGER,
    has_thumbnail     INTEGER NOT NULL DEFAULT 0,

    -- file 元数据
    file_count        INTEGER,

    -- 富文本标记（text 类型，二期扩展用）
    is_rich           INTEGER NOT NULL DEFAULT 0,

    -- ASR 元数据（source='asr' 时填充）
    transcription_id  INTEGER,                   -- 外键 → transcriptions.id
    polish_status     TEXT,                      -- 'off' | 'applied' | 'edited'
    engine            TEXT,
    model             TEXT,

    FOREIGN KEY (transcription_id) REFERENCES transcriptions(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_clip_created   ON clipboard_history(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clip_type      ON clipboard_history(item_type);
CREATE INDEX IF NOT EXISTS idx_clip_source    ON clipboard_history(source);
CREATE INDEX IF NOT EXISTS idx_clip_hash      ON clipboard_history(blob_hash);
CREATE INDEX IF NOT EXISTS idx_clip_favorite  ON clipboard_history(is_favorite);

-- FTS5 全文索引（借鉴 Ortu）
CREATE VIRTUAL TABLE IF NOT EXISTS clipboard_history_fts USING fts5(
    search_text,
    content='clipboard_history',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, search_text) VALUES (new.id, new.search_text);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_ad AFTER DELETE ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, search_text)
    VALUES('delete', old.id, old.search_text);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_au AFTER UPDATE ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, search_text)
    VALUES('delete', old.id, old.search_text);
    INSERT INTO clipboard_history_fts(rowid, search_text) VALUES (new.id, new.search_text);
END;
```

**app_config 新增 seed**：
```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
  ('clipboard_enabled',      'true',  '是否启用剪贴板历史监听'),
  ('clipboard_shortcut',     'Alt+V', '剪贴板历史窗口快捷键'),
  ('clipboard_max_items',    '1000',  '最大保留条数（不含收藏）'),
  ('clipboard_max_age_days', '30',    '自动清理天数（不含收藏）'),
  ('clipboard_auto_paste',   'double','列表项点击行为: single(复制) | double(粘贴)');
```

### 2.4 Rust 数据结构

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType { Text, Image, File }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source { Clipboard, Asr }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub source: Source,
    pub content: String,
    pub is_favorite: bool,
    pub created_at: String,
    pub image_meta: Option<ImageMeta>,
    pub file_meta: Option<FileMeta>,
    pub asr_meta: Option<AsrMeta>,
    pub is_rich: bool,
}

pub struct ImageMeta {
    pub blob_hash: String,
    pub width: u32,
    pub height: u32,
    pub has_thumbnail: bool,
}

pub struct FileMeta {
    pub file_count: usize,
    pub paths: Vec<String>,
}

pub struct AsrMeta {
    pub transcription_id: i64,
    pub polish_status: String,
    pub engine: String,
    pub model: String,
}
```

### 2.5 文件系统布局

```
~/.octopus/
├── octopus.db                         # 现有，新增 clipboard_history 表
└── clipboard_images/                  # 新增
    ├── <sha256>.png                   # 原图
    └── <sha256>_thumb.png             # 240×240 缩略图
```

### 2.6 不返工保障

| 维度 | 一期实现 | 二期扩展 | 为什么不返工 |
|---|---|---|---|
| DB schema | 全字段建表 | 不加字段 | blob_hash/width/height/file_count/is_rich 都在 |
| Rust 结构 | 全 enum 变体定义 | 不改结构 | ItemType/FileMeta/ImageMeta/AsrMeta 都在 |
| 存储路径 | `clipboard_images/` 目录建好 | 不改路径 | |
| 监听逻辑 | files>image>text 优先级定死 | 加 rich text 处理 | 优先级链预留 rtf/html 位置 |
| 清理逻辑 | image blob 回收 | file 无特殊处理 | |
| 前端过滤 | 四类型 toggle 过滤 | 不改组件结构 | file 图标二期才渲染 |

## 3. 监听线程架构与 ASR 写入时机

### 3.1 核心并发问题

ASR 识别完成时 `do_paste()` → `paste::paste()` → `write_to_clipboard()` 会写入剪贴板。监听线程会把这当作"用户复制了文本"，插入 `source=clipboard` 记录——实际应记为 `source=asr`。需区分"我们自己写的"和"外部复制的"。

### 3.2 架构：两个路径 + 一个 suppress flag

```
ClipboardHandle（Mutex<ClipboardContext>）
  ├─ write_text()      ← paste.rs / ASR 输出路径调用
  ├─ read_text()       ← paste.rs 恢复原剪贴板用
  └─ SUPPRESS_FLAG: AtomicBool

Watcher（独立线程，ClipboardWatcherContext）
  └─ on_clipboard_change() →
       if SUPPRESS_FLAG { 清 flag，跳过 }  ← 自己写的不记录
       else { 读内容 → 去重 → insert DB source=clipboard }

Store（DB 层）
  └─ insert_asr_item(text, transcription_id, ...)
       source=asr，写 DB，不动剪贴板
```

**suppress flag 机制**：
- `write_text()` 写剪贴板前 `SUPPRESS_FLAG.store(true)`
- watcher 的 `on_clipboard_change` 回调先检查 flag，若 true 则清 false 并跳过本轮
- 平台无关，不依赖"剪贴板来源应用"判断

### 3.3 两条写入路径

**路径 1：外部复制（clipboard 来源）**
```
用户在浏览器/编辑器复制
  → 剪贴板变化
  → watcher on_clipboard_change()
  → SUPPRESS_FLAG = false
  → handle.read_text() / read_image() / read_files()
  → 去重
  → store.insert_clipboard_item(type, content, source=clipboard)
  → Tauri emit("clipboard://changed") → 前端列表刷新
```

**路径 2：ASR 识别完成（asr 来源）**
```
coordinator.rs do_paste()
  → 1. insert_asr_item(text, transcription_id, meta)
       → store 直接写 DB, source=asr（不经过剪贴板）
  → 2. paste::paste() → write_text(text)
       → SUPPRESS_FLAG.store(true) → clipboard.set_text(text)
  → watcher 检测到变化 → SUPPRESS_FLAG=true → 跳过（不重复记录）
  → enigo 模拟 Cmd+V（现有逻辑不变）
```

ASR 历史记录 DB 写失败只记日志不阻断粘贴——ASR 粘贴优先级 > 历史记录。

### 3.4 监听线程生命周期

```
desktop 启动
  → octopus_clipboard::start_watcher(callback)
  → spawn 独立 std::thread：
      ClipboardWatcherContext::new()
      .add_handler(ClipboardChangeHandler { callback, suppress_flag })
      .start_watch()  ← 阻塞，直到 stop()

desktop 退出
  → watcher_shutdown.stop()  ← drop shutdown handle
```

### 3.5 关键设计不变量

1. **同一时刻只有一个 ClipboardContext 实例**（`Mutex` 保护）——Windows 防锁竞争
2. **Watcher 不共享 ClipboardContext**——用独立的 `ClipboardWatcherContext`
3. **SUPPRESS_FLAG 是 AtomicBool**——无锁，跨线程安全
4. **ASR 历史记录不经 watcher 路径**——走 `insert_asr_item` 直达 DB
5. **paste.rs 恢复原剪贴板逻辑不变**——两次 `write_text` 都设 suppress flag

## 4. UI 架构

### 4.1 前端目录结构

```
crates/desktop/frontend/
├── package.json
├── vite.config.ts
├── tailwind.config.ts
├── components.json              ← shadcn/ui 配置
├── index.html                   ← Vite 入口
└── src/
    ├── main.tsx
    ├── App.tsx                  ← 按 window label 路由
    ├── lib/
    │   ├── utils.ts             ← cn() shadcn 工具
    │   └── tauri.ts             ← invoke/listen 封装
    ├── components/
    │   └── ui/                  ← shadcn 组件源码
    ├── hooks/
    │   ├── useTauriEvent.ts
    │   ├── useClipboardHistory.ts
    │   └── useDarkMode.ts
    └── pages/
        ├── Overlay.tsx
        ├── Result.tsx
        ├── Settings/
        └── Clipboard/
            ├── index.tsx        ← 页面主组件
            ├── FilterTabs.tsx   ← tab 过滤器
            ├── HistoryList.tsx  ← 虚拟滚动列表
            ├── ClipboardItem.tsx
            └── SearchBar.tsx
```

### 4.2 路由策略

每个 Tauri 窗口加载同一个 `index.html`，React 根据 window label 渲染对应页面：

```typescript
function App() {
  const label = getCurrentWindow().label;
  switch (label) {
    case 'overlay':   return <Overlay />;
    case 'result':    return <Result />;
    case 'settings':  return <Settings />;
    case 'clipboard': return <Clipboard />;
  }
}
```

### 4.3 剪贴板历史页面布局

```
┌──────────────────────────────────────────────┐
│  🔍 [搜索框__________________]          📌   │  ← SearchBar + 窗口置顶
│                                                │
│  [全部] [🎤] [📝] [🖼️] [📄] [⭐]              │  ← FilterTabs（单选）
│                                                │
│  ┌──────────────────────────────────────────┐ │
│  │ 📝  这是一段复制的文本内容...      [⭐]  │ │  ← 虚拟滚动列表
│  ├──────────────────────────────────────────┤ │
│  │ 🎤  昨天是 monday today is...      [⭐]  │ │
│  ├──────────────────────────────────────────┤ │
│  │ 🖼️  [缩略图]                    [⭐]  │ │
│  ├──────────────────────────────────────────┤ │
│  │ 📄  file1.txt, file2.png (2)       [⭐]  │ │
│  └──────────────────────────────────────────┘ │
│                                                │
│  共 156 条                         [清空历史]  │
└──────────────────────────────────────────────┘
```

### 4.4 FilterTabs

单选互斥（EcoPaste 风格），`Tab` 键循环切换：

| tab | filter 值 | SQL 条件 | 图标（Lucide） |
|---|---|---|---|
| 全部 | `all` | 无过滤 | `LayoutGrid` |
| ASR | `asr` | `source = 'asr'` | `Mic` |
| 文本 | `text` | `item_type='text' AND source='clipboard'` | `Type` |
| 图片 | `image` | `item_type = 'image'` | `Image` |
| 文件 | `file` | `item_type = 'file'` | `FileText` |
| 收藏 | `favorite` | `is_favorite = 1` | `Star` |

### 4.5 HistoryList（虚拟滚动）

`@tanstack/react-virtual` 实现——只渲染可见区域 + 上下各 5 个 buffer。`estimateSize: 80`，分页加载每页 20 条。

### 4.6 ClipboardItem 按类型渲染

| item_type + source | 渲染 | 图标 |
|---|---|---|
| text + clipboard | 文本预览（前 2 行 + 省略号） | `Type` |
| text + asr | 文本预览 + 引擎名 Badge | `Mic` |
| image | 缩略图 + 尺寸标注 | `Image` |
| file | 文件名列表 + 数量，失效灰显 | `FileText` |

每项右侧收藏按钮（toggle `is_favorite`）。右键菜单（shadcn ContextMenu）：复制/粘贴/删除/收藏/备注。

### 4.7 操作行为

| 动作 | 行为 |
|---|---|
| 单击项 | 选中条目（不复制） |
| 双击项 | 复制到剪贴板（不关闭窗口，用户手动 Cmd+V） |
| 右键菜单 | 复制/粘贴/删除/收藏/备注 |
| 收藏 | toggle `is_favorite`（乐观更新） |
| 删除 | 两步确认（首次点击红色高亮 1.5s，再次点击执行） |
| 图片保存 | 点击下载图标 → 格式选择浮层 → 选 JPEG/WebP/PNG + 质量 → 直接落盘到 `~/Downloads/octopus/` |
| 文件打开 | 点击打开图标 → 系统默认应用打开 |
| 搜索 | FTS5 全文搜索，debounce 300ms |
| Tab 键 | 循环切换过滤器 |
| Esc | 关闭窗口 |

### 4.8 窗口属性

```json
{
  "label": "clipboard",
  "title": "剪贴板历史",
  "width": 420,
  "height": 600,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "resizable": true
}
```

失焦自动隐藏（除非 pinned）。macOS 后续可用 `tauri-nspanel` 做 NSPanel。

两个入口：全局快捷键浮窗（默认 `Alt+V`）+ 主窗口内访问按钮。

### 4.9 图片保存浮层（SaveImagePopover）

点击图片条目的下载图标弹出格式选择浮层（非系统对话框）：

```
┌────────────────────────────┐
│  格式                       │
│  ┌──────┬──────┬──────┐    │
│  │ JPEG │ WebP │ PNG  │    │  ← 分段控件，默认 JPEG
│  └──────┴──────┴──────┘    │
│                             │
│  质量               85      │  ← 细线滑轨 10-100，默认 85（仅 JPEG/WebP）
│  ●━━━━━━━━━━━━○────────    │
│  小                大       │
│                             │
│  保存后打开文件夹      [○]  │  ← toggle 开关，默认关
│ ─────────────────────────── │
│  [    保存到下载    ]       │  ← 墨色按钮，成功后变翡翠绿
└────────────────────────────┘
```

**保存逻辑**：
- 直接写入 `~/Downloads/octopus/<hash前8位>.<ext>`，文件名冲突自动加 `-1`、`-2`
- JPEG/WebP 使用用户选的质量值；PNG 无损（隐藏质量控件）
- 保存后写文件路径到剪贴板
- 勾选「打开文件夹」时，用系统文件管理器定位到该文件（macOS `open -R`、Windows `explorer /select,`、Linux `xdg-open`）

**视觉设计**（frontend-design skill）：纯白不透明卡片 + 双层强阴影，与剪贴板透明底形成明确层次。分段控件激活项白底微阴影，滑轨 3px 深墨色填充进度。

### 4.9 Tauri 命令层

```rust
#[tauri::command]
async fn query_clipboard_history(filter: String, search: Option<String>,
                                  page: u32, size: u32) -> Result<Vec<ClipboardItem>, String>;

#[tauri::command]
async fn toggle_clipboard_favorite(id: i64) -> Result<(), String>;

#[tauri::command]
async fn delete_clipboard_item(id: i64) -> Result<(), String>;

#[tauri::command]
async fn clear_clipboard_history(keep_favorite: bool) -> Result<(), String>;

#[tauri::command]
async fn copy_clipboard_item(id: i64) -> Result<(), String>;
```

## 5. 清理策略、错误处理与边界

### 5.1 自动清理

> ⚠️ `run_cleanup`（按天数/数量清理 + blob 回收）已实现但**尚未接入定时调用**。当前仅 FTS5 索引维护已接入（见下）。

**FTS5 索引维护**（已实现，防止影子表膨胀）：

```
触发时机：
  a) 应用启动时——rebuild 一次，清理上次运行遗留的空洞
  b) 运行中删除计数器（AtomicU32）累计达 10——rebuild + 清零

原理：FTS5 external content table 的 DELETE 触发器只移除逻辑索引，
      _data 表 b-tree 页不自动收缩，删除越多空洞越大。
      rebuild 重建索引结构，回收空洞。

边界：计数器是进程内存态，重启归零（启动时已 rebuild，逻辑自洽）。
```

**完整清理（run_cleanup，待接入）**：

```
执行步骤：
  1. DELETE 超过 max_age_days（默认 30）且 is_favorite=0
  2. DELETE 超出 max_items（默认 1000）且 is_favorite=0（按 created_at ASC）
  3. 孤立 blob 回收：对比 DB blob_hash 与磁盘文件，无引用的删除
  4. FTS5 索引重建

豁免：is_favorite=1 永不被删
```

### 5.2 错误处理

**监听路径**：所有错误（`available_formats` / `get_*` / DB INSERT / 磁盘满）均跳过本轮 + 记日志，不中断监听线程。文本 >50MB / 图片 >40MB 跳过。

**ASR 写入路径**：`insert_asr_item` 失败记 warn 不阻断粘贴。`write_text` 失败传播给 coordinator（现有行为）。

**paste.rs 迁移后**：Windows `set_text` 偶发 `ClipboardOccupied` → 重试 3 次（间隔 50ms）。恢复原剪贴板失败静默忽略。

### 5.3 边界 case

| 边界 | 处理 |
|---|---|
| suppress flag 竞态 | paste 期间外部恰好复制 → 被吞掉。可接受（< 300ms 窗口，极小概率） |
| 相同内容反复复制 | hash 去重，只更新 created_at |
| 空内容 | 跳过（空文本/空图片/空文件列表） |
| 应用自身写入 | 所有 `ClipboardHandle.write_text` 统一设 suppress flag |
| Wayland 协议不支持 | clipboard-rs fallback X11；纯 Wayland 无 XWayland → 功能不可用，记 error，其他功能不受影响 |
| 文件被删除/移动 | 渲染时 `Path::exists()` 检测，灰显不自动删 |

### 5.4 并发安全

| 资源 | 保护 |
|---|---|
| ClipboardContext（读写） | `Mutex`，全局单例 |
| ClipboardWatcherContext（监听） | 独立线程，不共享 Mutex |
| SQLite 连接 | 复用 infra `Mutex<Connection>` |
| SUPPRESS_FLAG | `AtomicBool` |
| 图片文件写入 | 文件名 = SHA-256 hash，无冲突 |

### 5.5 DB 迁移

现有 DB user_version=4。`init_schema` 加 v4→v5 分支，`db.sql` 追加 clipboard_history 建表（幂等 `CREATE TABLE IF NOT EXISTS`）。新用户 v0 一次性建全部，老用户 v4→v5 增量迁移。

FTS5 可用性：`rusqlie` with `bundled` feature 默认启用 FTS5，需验证。

## 6. 实施分期

| Phase | 范围 | 依赖 |
|---|---|---|
| **Phase 0** | React + Vite + Tailwind + shadcn/ui 项目骨架 + Tauri 构建链 | 无 |
| **Phase 1** | 现有三页面迁移到 React（overlay/result/settings） | Phase 0 |
| **Phase 2** | octopus-clipboard crate（model/store/handle/watcher/image/cleanup）+ paste.rs 迁移 + 移除旧插件 | 无（可与 Phase 0/1 并行） |
| **Phase 3** | 剪贴板历史 UI（FilterTabs/HistoryList/ClipboardItem/SearchBar）+ Tauri 集成 + ASR 写入 | Phase 1 + 2 |
| **Phase 4** | file 类型渲染 + 富文本原文存储 | Phase 3 |
| **Phase 5**（可选） | macOS NSPanel + Paste Stack + 内容变换 | Phase 3 |

## 7. 依赖变更

**移除**：`tauri-plugin-clipboard-manager = "2"`

**新增（Rust）**：
- `clipboard-rs = { version = "0.3", features = ["image", "wayland"] }`
- `image = { version = "0.25", features = ["png", "webp", "jpeg"] }`
- `webp = "0.3"`（WebP 编码）
- `sha2 = "0.10"`
- `dirs = "5"`（定位 Downloads 目录）

**新增（前端 package.json）**：
- react / react-dom / vite / @vitejs/plugin-react
- tailwindcss / @tailwindcss/vite
- typescript / @types/react
- @tauri-apps/api
- @tanstack/react-virtual
- lucide-react
- react-hook-form / zod / @hookform/resolvers
- shadcn/ui 依赖：class-variance-authority / clsx / tailwind-merge / @radix-ui/react-*

## 8. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| clipboard-rs Wayland 监听在某些合成器不工作 | 中 | Linux Wayland 监听失效 | fallback X11；纯 Wayland 无 XWayland 降级为只读 |
| FTS5 在 bundled SQLite 未启用 | 低 | 搜索降级为 LIKE | 验证 rusqlite bundled；不含则加编译选项 |
| React 迁移引入回归 | 中 | 现有功能异常 | Phase 1 后全面回归测试；旧 HTML 在 git 历史可回退 |
| suppress flag 极窄竞态 | 低 | 偶发丢失一条外部复制 | 可接受（< 300ms 窗口） |
| Windows 剪贴板锁竞争 | 中 | `set_text` 偶发失败 | 重试 3 次 + 50ms 间隔 |
