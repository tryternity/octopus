# 剪贴板历史管理功能设计

**日期**: 2026-06-25
**状态**: ✅ Phase 0-3 + Phase 3 后持续迭代（OCR / 图片存储迁移 / 设置页配置 / 管理页重设计 / 分页竞态修复 / 级联删除验证 / 快捷键热重载 / UI 优化 / FTS5 维护 / 启动同步修复 / 浮窗监听按钮样式）
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
变化 → has(Files) → get_files() → Vec<String>
  跨平台格式：Linux X11/Wayland = text/uri-list（file:// URI + 百分号编码）；
              macOS（clipboard-rs 用 NSURL.path）/ Windows（FileList）= 已解码普通路径
  → DB 存原始字符串（按平台原样，不在入库时解码——写回 write_files 各平台自洽）
  → 超过 50 个只记前 50 + file_count=实际数量
  → JSON.stringify(paths) → SHA-256 去重
  → DB insert: content=JSON(paths), file_count=N, search_text=paths.join(" ")
  出口解码（仅 file:// 开头才解码，避免误伤含字面 %XX 的普通路径）：
    前端 formatFilePaths 显示用 decodeURIComponent；后端 open_file_item 用 decode_file_uri（percent-encoding）
```

**`open_file_item` 打开命令按平台**：macOS `open` / Linux `xdg-open` / Windows `cmd /c start ""`（调默认关联程序）。Windows 不用 `explorer <file>`——它只在资源管理器「定位并选中」文件、不打开（旧实现曾用 explorer，v2 审计 2.2 改 cmd start）。Windows 剪贴板文件恒为普通路径（`CF_HDROP`），`decode_file_uri` 的 `file://` 分支不会触发，故无 `file:///C:/` 前导斜杠问题（v2 审计 2.1 核实为非实际 bug）。

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
  ('clipboard_max_age_days', '30',    '自动清理天数（不含收藏）');
```

> **`clipboard_enabled` 已落地（非 seed-only）**：纳入 `AppConfig`（bool，默认 `true`，`config.rs` 字段 + `db.rs` load/save 双向映射）。
> 运行时热重载——`ClipboardHandle` 持 `recording_enabled: AtomicBool` 镜像，`on_clipboard_change` 在 suppress 检查后加 gate（`false` 直接 return，不存库、不 emit `clipboard://changed`）；`set_config` 收到 `clipboard_enabled` 变更即翻转该 flag（无需 stop/restart watcher，watcher 线程始终运行）。
> 启动同步——`ClipboardHandle::new()` 默认 `recording_enabled = true`，若仅靠 `set_config` 热重载，用户关掉监听后重启会让 flag 复活（DB 仍 `false`，watcher 又开始记录）。故 `main.rs` setup 创建 handle 后、watcher 启动（同一 `Arc`）前，立即按 `config.clipboard_enabled` 调一次 `set_recording_enabled`，让运行时 flag 与 DB 持久值一致。
> 入口：设置页「交互」Card 开关 + 浮窗 title bar 快捷按钮（Pin 左侧，CircleCheck（绿圆勾=监听中）/ CircleX（红圆叉=已关闭）），双向经 `config-changed` 事件同步。
> **`clipboard_auto_paste` 已移除**：双击列表项固定 = 粘贴（`paste_clipboard_item`，见下交互表），不再可配。

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
  → 1.5 emit("clipboard://changed")  ← insert 成功后主动广播
  → 2. paste::paste() → write_text(text)
       → SUPPRESS_FLAG.store(true) → clipboard.set_text(text)
  → watcher 检测到变化 → on_clipboard_change 命中 SUPPRESS_FLAG=true
       → 直接 return（on_change 闭包整体不执行）→ 不重复记录、也不 emit
  → enigo 模拟 Cmd+V（现有逻辑不变）
```

**为何步骤 1.5 必须主动 emit**：watcher 的 `ChangeHandler::on_clipboard_change`（`watcher.rs`）在调用 `on_change` 闭包**之前**就 `check_and_clear_suppress`，命中即 return——而 emit 本就在 `on_change` 闭包内（`main.rs` 注入）。因此 ASR 粘贴触发的剪贴板变化**不会**自然产生 `clipboard://changed`。若不主动广播，前端浮窗（`useClipboardHistory`）/ 设置页（`ClipboardPanel`）收不到通知，ASR 记录虽已入库却无法即时渲染，需等用户手动复制外部内容触发 watcher、或重启窗口才刷新。主动 emit 仅在 `insert_asr_item` 成功后触发（失败无记录可显示，不广播）。

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
5. **paste.rs 恢复原剪贴板按类型**——`write_to_clipboard=false` 时 `paste_via_clipboard` 用 `ClipboardBackup` 备份原内容（files > image > text 优先级），ASR 文本粘贴后按类型还原（图片 `set_image` / 文件 `write_files` / 文本 `write_text`，均设 suppress flag；旧实现只 `read_text`，图片/文件被空串吞掉丢失）
6. **去重按 ItemType 匹配**——文本走 `find_by_text(text, ItemType::Text)`、文件走 `find_by_text(paths_json, ItemType::File)`（同一函数，`item_type` 参数化）、图片走 `find_by_content_hash`。旧实现 `find_by_text` 硬编码 `item_type='text'`，文件去重永远 miss → 连续复制同一文件源源不断写重复记录（已修，加 `test_find_by_text_file_dedup` 回归）。

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
│  共 156 条                      [管理]  │
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
| 双击项 | 写剪贴板 → 隐藏浮窗 → 恢复焦点 → 模拟 Cmd+V 自动粘贴（`paste_clipboard_item`，后端串起 hide `clipboard_window` + `focus_tracker.restore_focus` + `simulate_paste`）。**固定行为**（`clipboard_auto_paste` 可配项已移除） |
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

两个入口：全局快捷键浮窗（默认 `Alt+V`，toggle 按焦点判断：失焦状态按快捷键直接 `show`+`set_focus` 激活，仅「可见且有焦点」才收起——避免 always-on-top 窗口失焦后仍 visible 导致需按两次）+ 主窗口内访问按钮。

**管理入口**：浮窗底部「管理」按钮 → `open_settings({ initialPage: "clipboard" })` → 跳转设置窗口剪贴板管理页。`open_settings` 通过 `PENDING_PAGE`（`Mutex<Option<String>>`）暂存目标页面，前端 mount 后调 `get_initial_page` 拉取；窗口已打开时走 `settings://navigate` 事件即时切换。

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

### 4.10 Tauri 命令层

```rust
#[tauri::command]
async fn query_clipboard_history(filter: String, search: Option<String>,
                                  page: u32, size: u32) -> Result<Vec<ClipboardItem>, String>;

#[tauri::command]
async fn toggle_clipboard_favorite(id: i64) -> Result<(), String>;

#[tauri::command]
async fn delete_clipboard_item(id: i64) -> Result<(), String>;

#[tauri::command]
async fn delete_clipboard_items(ids: Vec<i64>) -> Result<usize, String>;

#[tauri::command]
async fn clear_clipboard_history(keep_favorite: bool) -> Result<(), String>;

#[tauri::command]
async fn copy_clipboard_item(id: i64) -> Result<(), String>;

#[tauri::command]
async fn clipboard_stats(filter: String, search: Option<String>) -> Result<i64, String>;
```

**修改命令广播同步**：`toggle_clipboard_favorite` / `delete_clipboard_item` / `delete_clipboard_items` / `clear_clipboard_history` 成功后 `emit("clipboard://changed")`——浮窗（`useClipboardHistory`）+ 设置页（`ClipboardPanel`）均监听此事件刷新，避免双端列表状态不一致（旧实现仅 watcher 外部复制时 emit，手动删除/收藏后另一窗口不刷新）。

**ASR 粘贴广播**：除上述手动修改命令外，`do_paste` 在 `insert_asr_item` 成功后也主动 `emit("clipboard://changed")`——ASR 粘贴写剪贴板时设 suppress flag，watcher 命中后跳过含 emit 的 `on_change` 闭包、不自然触发，故需主动广播前端才能即时渲染新 ASR 记录（详见 §3.3 路径 2）。

**级联删除广播**：设置页 `delete_history`（删转译记录）级联调 `delete_by_transcription_ids` 清理剪贴板 ASR 条目，**删除行数 >0 时主动 `emit("clipboard://changed")`**——否则浮窗 stale 显示已删条目，双击粘贴查不到 id 失效。这是 settings 命令间接影响剪贴板列表、需向浮窗/设置页双端广播的又一处（与上述 clipboard 命令同事件）。

**copy/paste 按类型还原**：`copy_clipboard_item` / `paste_clipboard_item` 经 `write_item_to_clipboard` 统一分发——文本 `write_text`、图片从 `image_data` 读 WebP 原图转 PNG 后 `write_image`、文件解析 JSON 路径后 `write_files`，还原真实内容（旧实现无视类型一律 `write_text`，导致图片粘出 blob_hash、文件粘出 JSON 字符串）。

**底部计数随筛选/搜索变化**：`clipboard_stats(filter, search)` 转调 store `count_history`——与 `query_history` 同一套 `build_where`（类型过滤）+ LIKE-fallback/FTS5-MATCH（搜索）逻辑，保证底部「共 N 条」随当前标签/搜索框变化，而非恒为全表总数（旧 `count_all` 无视筛选，浮窗/设置页切「图片」或输入搜索词后计数仍显全表）。前端 `useClipboardHistory` + `ClipboardPanel` 两处 invoke 均传 `filter` + `debouncedSearch`。

## 5. 清理策略、错误处理与边界

### 5.1 自动清理

> ✅ `run_cleanup`（按天数/数量清理 + blob 回收）已接入定时调用：`main.rs` setup 启动时跑一次（image_migration 迁入旧图片后）+ 后台线程每小时从 DB 重读 `clipboard_max_items` / `clipboard_max_age_days` 跑一次（用户运行时改限额 1 小时内自动生效）。`run_cleanup` 仅在有删除/回收时重建 FTS，定时清理无删除时只做几次 COUNT（很轻）。

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

**完整清理（run_cleanup，已接入）**：

```
触发：a) main.rs setup 启动一次（image_migration 之后）；b) 后台线程每小时（从 DB 重读最新限额）
执行步骤：
  1. DELETE 超过 max_age_days（默认 30）且 is_favorite=0
  2. DELETE 超出 max_items（默认 1000）且 is_favorite=0（按 created_at ASC）
  3. 孤立 blob 回收：cleanup_unreferenced_images（image_data 引用计数，无引用的删 DB 行）
  4. FTS5 索引重建（仅在第 1-3 有删除/回收时；无删除跳过，定时清理保持轻量）

豁免：is_favorite=1 永不被删
```

### 5.2 错误处理

**监听路径**：所有错误（`available_formats` / `get_*` / DB INSERT / 磁盘满）均跳过本轮 + 记日志，不中断监听线程。文本 >50MB / 图片 >40MB 跳过。**非 files/image/text 的自定义二进制格式**（Adobe/Office 等专有格式、空剪贴板）经 `else if has(Text)` 校验后静默跳过、不进 text 分支——避免 `read_text()` 失败触发 `error!` 日志污染。

**ASR 写入路径**：`insert_asr_item` 成功后主动 `emit("clipboard://changed")`（paste 的 suppress flag 使 watcher 不自然触发，见 §3.3）；失败记 warn 不阻断粘贴、也不广播（无记录可显示）。`write_text` 失败传播给 coordinator（现有行为）。

**paste.rs 迁移后**：Windows `set_text` 偶发 `ClipboardOccupied` → 重试 3 次（间隔 50ms）。`write_to_clipboard=false` 时 `paste_via_clipboard` 按 `files > image > text` 优先级用 `ClipboardBackup` 备份原内容，ASR 文本粘贴后按类型还原（图片 `set_image` / 文件 `write_files` / 文本 `write_text`），还原失败静默忽略（旧实现只 `read_text`，图片/文件被空串吞掉丢失）。

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

### 6.1 Settings 管理页设计（ClipboardPanel + HistoryPanel）

剪贴板浮窗适合快速查看/复制，但批量管理（多选删除、搜索过滤）需要更大的空间。Settings 窗口的「剪贴板」和「识别记录」两个 tab 提供完整管理能力。

**统一布局**（两页面共享风格）：
- **顶部**：搜索框 / 过滤标签（stone-50 底 + stone-200 描边）
- **列表 header**：全选 checkbox（hover 或有选中时显示「已选 N 项 / 全选」）
- **列表行**：checkbox + 类型图标 + 内容预览 + 元数据（时间/引擎/badge）+ hover 操作栏
- **底部**：状态栏（共 N 条 / 显示 N 条，N 随当前类型筛选 + 搜索框变化，由 `clipboard_stats(filter, search)` 返回），选中后浮现「删除选中」按钮（二次确认 3s）
- **分页**：手动「加载更多」按钮（每页 20/50 条），底部「— 没有更多了 —」提示

**ClipboardPanel 行操作**（ClipboardRow 子组件，与浮窗一致）：复制、收藏、保存图片（SaveImagePopover）、打开文件、单条二次确认删除（1.5s）

**HistoryPanel 行操作**（HistoryRow 子组件）：复制、单条二次确认删除（1.5s）、原始文本折叠展开（已润色条目左侧 amber-600 竖线）

**竞态说明**：曾尝试无限滚动（IntersectionObserver），但自动加载与删除刷新存在闭包陷阱（`pendingResetRef` 递归补执行捕获旧 `loading=true` 永远跳不出），最终回退为手动「加载更多」——手动点击不会与删除并发，`loadHistory`/`fetchData` 无需 loading 守卫，删除后直接刷新。

**级联删除**（单向）：删除识别记录（`transcriptions`）→ 同步删除引用该 `transcription_id` 的剪贴板条目（`delete_by_transcription_ids`）。反向不级联——删除剪贴板 ASR 条目只删 `clipboard_history` 行，`transcriptions` 源数据不受影响（外键 `ON DELETE SET NULL` 处理引用置空）。曾发现旧数据 `transcription_id` 为 NULL 导致级联失效，已清理旧数据并加测试断言验证写入/读回正确。

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

## 9. Phase 3 后持续迭代（2026-06-27）

### 9.1 OCR 模块（详见独立 spec: `2026-06-27-ocr-module-design.md`）

剪贴板图片条目新增 OCR 识别能力：
- 独立 crate `octopus-ocr`（ocr-rs/MNN + PP-OCRv6）
- 手动触发：图片条目点 OCR 按钮 → 识别文本写入 `search_text` + 系统剪贴板 + osascript 新建 TextEdit 文档
- DB `models` 表新增 `domain='ocr'` 模型条目（source=det URL, secret_key=rec URL）
- 详见 `docs/superpowers/specs/2026-06-27-ocr-module-design.md`

### 9.2 图片存储迁移（详见独立 spec: `2026-06-27-image-storage-blob-design.md`）

剪贴板图片从文件系统 `~/.octopus/clipboard_images/` 迁移到 DB BLOB：
- 新增 `image_data` 表（hash/blob/thumb/image_type/width/height/created_at）
- WebP 无损原图 + WebP 20% 缩略图
- 删除条目时引用计数为 0 才删 image_data 行
- 启动时 `image_migration` 模块自动迁移旧文件到 DB，迁移后删除目录
- 前端图片条目内联缩略图（`get_image_thumb` 命令 → 后端编码为 `data:image/webp;base64,...` data URL，前端直接 `<img src>`，避免 IPC 传 `Vec<u8>` 的 JSON 数字数组膨胀）
- 详见 `docs/superpowers/specs/2026-06-27-image-storage-blob-design.md`

### 9.3 设置页配置暴露

剪贴板相关配置暴露到系统设置页（GeneralPanel）：
- **快捷键 section** 新增「剪贴板浮窗」行（ShortcutButton kbd 标签风格，热重载）
- **剪贴板 section**（新增 Card）：最大保留条数（100/200/300/500/1000）+ 自动清理天数（1/3/7/15/30）
- AppConfig 新增 `clipboard_shortcut` / `clipboard_max_items` / `clipboard_max_age_days` 字段（+ `clipboard_enabled` bool，已落地，见 §2.3）
- `save_app_config` / `load_app_config` 同步更新（30 字段）

### 9.4 设置页 UI 优化

- 「引擎接入」section 移除（embedded 模式不需要用户配置）
- 润色 section label 加「润色」前缀
- 润色模型 select 用 `current` 匹配（修 3-part spec 与裸名不匹配问题）
- 快捷键捕获过滤纯修饰键（Alt/Shift/Control/Meta）

### 9.5 FTS5 索引维护

- 启动时 rebuild + 删除计数器达 10 自动 rebuild
- `delete_item` / `clear_history` 写回 search_text 时 FTS5 触发器自动更新索引

### 9.6 管理入口 + 管理页重设计

- 浮窗底部「清空」改为「管理」按钮，点击 `open_settings({ initialPage: "clipboard" })` 跳转设置页
- `open_settings` 支持初始页面参数（`PENDING_PAGE` 暂存 + `get_initial_page` 拉取）
- ClipboardPanel/HistoryPanel 按同风格重设计（stone 色系、行子组件、hover 操作）

### 9.7 分页与删除竞态

- 曾尝试无限滚动（IntersectionObserver）→ 闭包陷阱（`pendingResetRef` 递归捕获旧 loading）→ 回退手动「加载更多」
- 管理页手动加载更多不会与删除并发，`loadHistory`/`fetchData` 无需 loading 守卫

### 9.8 级联删除验证与 transcription_id 修复

- 发现旧数据 `transcription_id` 为 NULL 导致级联失效，清理后新记录正确关联
- 单向级联：删识别记录 → 同步删剪贴板引用；反向不级联
- 级联删除计入 FTS 维护：`delete_by_transcription_ids` 删除行数 >0 时同样调 `track_deletes`（与 `delete_items` / `clear_history` 一致），否则大批删转译记录后 FTS 影子表 `_data` 页空洞不回收、迟迟不触发阈值 rebuild；ASR 条目无 `blob_hash`，无需 `cleanup_unreferenced_images`

### 9.9 快捷键热重载

- `clipboard_shortcut` / `screenshot_shortcut` 在 `set_config` 中 unregister 旧 + register 新（经 `register_clipboard_shortcut` / `register_screenshot_shortcut` helper，与 `asr_shortcut` 一致：失败回滚旧值 + 返回 Err；旧实现 `let _ = on_shortcut(...)` 吞错——冲突时静默存入无效配置、重启后仍失败，已修复）
- `save_app_config` / `load_app_config` 补齐三个剪贴板配置字段（原 bug：AppConfig 有字段但 DB 读写漏了）
