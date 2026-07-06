# 已归档设计规格（2026-06-25 ~ 2026-06-28）

> 以下功能均已实现并合并 main。本文件由 10 份原独立 spec 文件合并归档，原文件已删除。
> spec↔plan 旧路径交叉引用随归档失效，按主题在 plans/2026-06-28-archived-plans.md 内查同名章节。

## 目录

- 2026-06-25-clipboard-history-design.md
- 2026-06-26-asr-rename-to-local-design.md
- 2026-06-26-asr-separator-i18n-design.md
- 2026-06-27-asr-streaming-token-diagnostic-design.md
- 2026-06-27-global-edit-shortcut-design.md
- 2026-06-27-image-storage-blob-design.md
- 2026-06-27-ocr-module-design.md
- 2026-06-28-polish-global-shortcut-design.md
- 2026-06-28-screenshot-design.md
- 2026-06-28-settings-model-selection-design.md


---

## 来自原文件 `2026-06-25-clipboard-history-design.md`

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


---

## 来自原文件 `2026-06-26-asr-rename-to-local-design.md`

# octopus-asr → octopus-asr-local 重命名设计

> **目标**：把 `octopus-asr` crate 改名为 `octopus-asr-local`，与 `octopus-asr-cloud` 命名对称。
> **性质**：纯机械重命名，**零行为变更、零接口变更**。
> **范围决策（用户 2026-06-26）**：① 彻底对称（package + lib + 目录全改，lib → `octopus_asr_local`，11+ 文件 `use` 一并改）；② docs 全改含 archived（33 文件）。

## 1. 背景与现状

`crates/asr`（package `octopus-asr`，lib `octopus_asr`）是本地 ASR 零件库 + 无端 helper（`transcribe_batch` / `StreamingRunner` / `StreamingEngine`/`OfflineEngine`/`AudioSource` trait / `TranscriptEvent` / `PipelineConfig`）。与云端 `octopus-asr-cloud`（`crates/asr-cloud`，lib `octopus_asr_cloud`）职责对称（本地 vs 云端），但命名不对称：本地缺 `-local` 后缀。

被 `asr-cloud` / `cli` / `desktop` / `server` / `llm` 依赖（`asr-cloud` 也依赖它拿本地零件 trait）。三端（cli/desktop/server）已统一走 asr helper（阶段1/2/3 收官，2026-06-26）。

## 2. 改名映射

| 项 | 现 | 新 |
|---|---|---|
| package name | `octopus-asr` | `octopus-asr-local` |
| lib name（派生） | `octopus_asr` | `octopus_asr_local` |
| 目录 | `crates/asr` | `crates/asr-local` |

与 `octopus-asr-cloud`（package `octopus-asr-cloud` / lib `octopus_asr_cloud` / `crates/asr-cloud`）完全对称。**不**加显式 `[lib] name`——lib name 由 package 派生，与 asr-cloud 一致。

## 3. 影响清单

### 3.1 代码 / Cargo（必须改，编译器验证）

- `crates/asr-local/Cargo.toml`：`name = "octopus-asr"` → `"octopus-asr-local"`
- workspace `Cargo.toml`：members `"crates/asr"` → `"crates/asr-local"`
- **5 个依赖 Cargo.toml**（依赖名 + `path = "../asr"` → `"../asr-local"`）：`asr-cloud` / `cli` / `desktop`（含 feature `embedded = ["octopus-asr"]` → `["octopus-asr-local"]`） / `server` / `llm`
- **~17 源文件** `octopus_asr` → `octopus_asr_local`：desktop(13) / asr-cloud(2-3) / server(2) / cli(2) / llm(1) / asr-local/lib.rs(1)
- `Cargo.lock`：自动重新生成（**不手改**）

### 3.2 无向后兼容风险

cli/main.rs 的 46 处引用**全为下划线 `octopus_asr` 代码路径**（`use` / `octopus_asr::path`），**0 处连字符、0 处 `"octopus-asr"` 字符串字面量**。即没有用户可见的 `octopus-asr` 配置 key / engine source 名 / CLI 参数——改名不影响任何 config/脚本/外部调用。

desktop feature 名 `embedded` / `cloud` 本身不变（仅 `embedded` 启用项从 `octopus-asr` 改为 `octopus-asr-local`），`--features embedded cloud` 用法不受影响。

### 3.3 docs（全改含 archived，33 文件）

sed 全局替换，含 `architecture.md` / `AGENTS.md` / `usage.md` / `docs/superpowers/specs/*` / `docs/superpowers/plans/*`（含 `*-archived-*`）/ `crates/dlp/docs/architecture.md` / `docs/asr_archiveture_opt.md`。理由：代码里将无 `octopus-asr`，archived 保留旧名会误导 vibecoding；archived 保留的是设计决策/动机，非当时的 crate 名。

## 4. 执行顺序

1. `git mv crates/asr crates/asr-local`（保 rename history）
2. 改 `asr-local/Cargo.toml` `name` + workspace `members`
3. 改 5 依赖 Cargo.toml（name + path）
4. 替换源码 `octopus_asr` → `octopus_asr_local`
5. `cargo check --workspace --all-targets`（验证编译 + 自动更新 Cargo.lock）
6. `cargo test --workspace` + clippy（0 新 warning）
7. 替换 docs（33 文件）
8. commit

## 5. 关键执行细节：替换必须排除 `-cloud`

朴素 `s/octopus-asr/octopus-asr-local/g` 会把 `octopus-asr-cloud` 误改成 `octopus-asr-local-cloud`（下划线同理：`octopus_asr_cloud` → `octopus_asr_local_cloud`）。

**用 perl 负向 lookahead 精确排除**：
- 连字符：`perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g'`（后不跟 `-`，排除 `-cloud`；已改的 `-local` 后跟 `-` 也不重复匹配）
- 下划线：`perl -pi -e 's/octopus_asr(?!_)/octopus_asr_local/g'`（后不跟 `_`，排除 `_cloud`；`::`/空白/引号前的 `octopus_asr` 全改）

macOS BSD sed 不支持 `\b`，故用 perl（负向 lookahead 可靠）。plan 给精确命令 + 每步 grep 复核。

## 6. 验证

- `cargo check --workspace --all-targets`：0 error
- `cargo test --workspace`：全绿（lib + 各 crate 单测）
- `cargo clippy --workspace --all-targets`：0 新 warning
- grep 复核：仓库内（排除 `target`/`node_modules`/`.git`）无残留 `octopus-asr`/`octopus_asr`（非 cloud）—— `grep -rnE 'octopus[-_]asr(?![-_])'` 应只剩 `*-cloud`/`*_cloud`

## 7. 风险

- **低**：纯机械替换，编译器抓所有代码遗漏；docs 遗漏由 grep 复核。
- perl 负向 lookahead 排除 cloud——执行后 grep 复核确认无 `*_local_cloud` 误伤。
- git history：`git mv` 保目录 rename；源文件内容仅 `use` 行变，git 识别为 modify（非 rename）。
- `asr-cloud` 依赖 `asr-local`（云端依赖本地零件 trait），语义不变。


---

## 来自原文件 `2026-06-26-asr-separator-i18n-design.md`

# ASR 句间分隔符 i18n 统一

> **状态**：已实施（2026-06-26）。源自 bug 报告审查 → 用户「统一处理」。

## 背景

ASR 多句/多段文本拼接的句间分隔符，全 workspace 原硬编码中文逗号 `'，'`（U+FF0C）。
`language=en` 时，英文文本被插入中文全角逗号，不规范（如 `"Hello world，How are you"`）。

审查发现共约 16 处同类拼接，分布在 3 个 crate、多条路径：

| crate | 文件 | 处 | 路径语义 |
|---|---|---|---|
| asr-cloud | `aliyun_stream.rs` | 4 | 云端流式 Fun-ASR/Qwen 句间拼接 |
| asr-local | `streaming_engine.rs` | 9 | 本地流式静音分句（7 `push('，')` + 2 `format!("{}，{}")`） |
| desktop | `engine_aliyun.rs` | 4 | 桌面分块云端 `collect_results` |
| desktop | `coordinator.rs` | 1 | `finalize_cloud` 跨 utterance |
| desktop | `pipeline.rs` | 1 | `consume_completed_results_vad` 段间 |
| desktop | `cloud_pipeline.rs` | 1 | `drain_cloud_session` 提交 |

> asr-cloud `aliyun_stream.rs` 已先于本次统一单独修复（commit `e487aaf`，私有 helper）；
> 本次将其提升为公共 helper 并推广到全 workspace。

## 设计

### 共享 helper

`sentence_separator(language: &str) -> &'static str`，落点 **`asr-local/src/paraformer.rs`**
（紧邻 `smart_append`），`lib.rs` re-export 为 `octopus_asr_local::sentence_separator`。

- **落点理由**：`asr-local` 是 asr-cloud / desktop / server / cli 的共同依赖底座
  （asr-cloud → asr-local，desktop → asr-local + asr-cloud），放此处零循环依赖、零新依赖边。
- **取值**：`en`（大小写不敏感）→ `" "`（空格）；其他（`zh`/`auto`/空）→ `"，"`。
- **英文用空格的理由**：英文 ASR 句子常自带句末标点（`.`/`!`/`?`），空格连接最自然且
  不与之冲突；若用英文逗号 `,` 或句号 `.` 会与服务端标点打架。中文/auto 保持 `，`（口语
  连续叙述的连贯感）。

### 接口变化

| 接口 | 变化 |
|---|---|
| `StreamingSession::new` | 加 `language: &str` 参数；三 variant（Paraformer/ZipformerCtc/ZipformerTransducer）各存 `separator: &'static str` 字段，构造时由 `sentence_separator(language)` 算出 |
| `consume_completed_results_vad` | 加 `language: &str` 参数（pub(crate) fn，3 调用方：1 生产 + 2 测试） |
| `collect_results`（desktop engine_aliyun） | 加 `language: &str` 参数 |
| `drain_cloud_session` / `CloudDrainState` | `CloudDrainState` 加 `language: &'a str` 字段（借用结构），构造处（`CloudPipelineEngine::tick`）传 `&self.language` |

### language 可达性

各拼接点均已能拿到 language（本次确认）：

- **本地流式**：两调用方（`coordinator.rs:613/620`、`server/main.rs:202`）创建时已持有
  `config.language` / query `language`（server 原 `_language` 未用，改回 `language`）。
  从 DB `ModelEntry.language` 推断不可靠（字段可能空 + 模型本质单语言，与用户意图冲突），
  故走「调用方传 user-intent language」路径。
- **desktop cloud/段间**：`coordinator` 构造各 pipeline 时已快照 `config.language.clone()`
  给 `CloudPipelineEngine.language` / `VadSegmentedPipeline.language`；`finalize_cloud`
  直接读 `config.language`。
- **engine_aliyun**：`transcribe` 已收 `language`，透传给 `collect_results`。

## 验证

- `cargo test -p octopus-asr-local -p octopus-asr-cloud -p octopus-server -p octopus-desktop`：全绿
  （含新增 `paraformer::tests::sentence_separator_by_language`）。
- `cargo clippy`（desktop `--features cloud`）：改动文件零新 warning（命中的 `*acc` deref /
  collapsible-if 均为既有周边结构，separator 改动不引入新类型）。

## 不在范围

- coordinator/desktop 侧历史 plan（已归档到 `2026-06-25-archived-plan.md` 的 `asr-pipeline-stage2c2` / `vad-segmented-rehome`）里的「逗号拼接」
  描述为已合并的历史实施记录，按惯例不回溯修改；本 spec 作为现状设计文档。


---

## 来自原文件 `2026-06-27-asr-streaming-token-diagnostic-design.md`

# 流式 ASR 首/尾字诊断与修复设计

> **目标**：修复流式 ASR（`StreamingPipeline`）的首字缺失、启动 spurious token、停顿后丢字、尾字/中段重复问题。
> **性质**：行为修复（bug fix），**无接口破坏**（`StreamingEngine` trait 的 `has_speech` 参数是本期内新增，所有实现已同步）。
> **范围**：`crates/asr-local`（`streaming_runner` / `streaming_engine` / `streaming_paraformer` / `streaming_zipformer`），server `pipeline.rs` mock 同步签名。
> **诊断方法**：`[asr-diag]` 字级 / frame 级 `log::debug!`（验证完成后已清理，见 plan Phase 4）。

## 1. 背景

用户实测流式 ASR（paraformer 为主，中文识别率高）：

1. **启动 spurious「嗯」**：开始录音、尚未说话时 final 就以「嗯，…」开头。
2. **首字缺失**：「开始语音识别」→「始语音识别」。
3. **停顿后丢字**：连续说多句，停顿后的下一句首字丢失（段 2 丢「开」、段 4 丢「始」）。
4. **尾字 / 中段重复**：「开始语音识别」→「始语音**识识**别」/「开**语语**音识别」/「**开开**语音识别」。

zipformer 同样有首字缺失（机理独立，见 §3.1），但用户重点用 paraformer，zipformer 尾字诊断搁置（从未复现）。

> **优先级（用户 2026-06-27）**：**丢字 > 叠字**。丢字使模型不可用；叠字可由 LLM 润色后处理兜底。故修复重心在「不丢字」，叠字尽力而为、剩余交 LLM。

## 2. 诊断方法：`[asr-diag]` 日志（诊断期临时，已清理）

在流式 token 生成路径插入 frame 级 / chunk 级 / token 级 `log::debug!`（统一 `[asr-diag]` 前缀），观测：

- paraformer：`process_chunk_at` 的 mask 决策（`is_first`/`enc_len`/`mask 前 alpha_sum`/`mask_left`/`mask_right`/`fresh`）、CIF `fired`/`alpha_cache`、force-fire 条件、跨边界去重命中、fresh_segment 消费。
- zipformer：reset 前段文本快照、CTC/Transducer token emit。

辅助：`diag_text_dup_sentinel`（文本层重复哨兵）——decode 后扫描相邻 CJK 叠字，验证 token 层跨边界去重是否漏网。

**这些日志是诊断工具，验证完成后已全部删除**（17 处 `log!` + 哨兵函数及其单测），见 plan Phase 4。

## 3. 根因分析

| # | 症状 | 引擎 | 根因 | 置信度 |
|---|---|---|---|---|
| 3.1 | 首字缺失 | **zipformer** | `accept_samples` Zipformer 分支 `if was_silent { finish+reset }`；`was_silent` 取 `step_silence` 的 `prev`（**更新前** silence_duration）。开口前静音 > 0.5s、开口瞬间能量低 `has_speech=false` → silence 持续 > 0.5s → 每 tick `was_silent` 恒真 → 反复 `finish+reset`，`reset` 清空 `token_ids` 冲掉首字音头 | 确凿 |
| 3.2 | 首字缺失 | **paraformer** | `mask_alphas_selective` 把首 chunk 的 left 5 帧 + right 3 帧 alpha 置 0；但首 chunk 的 left 帧（`feat_cache` 初始全 0 = padding）与 right 帧（alpha 集中在 right 3 帧）恰恰承载首字能量 → mask 后累积不到阈值 → fired=0 → 首字丢失 | 确凿（e2e） |
| 3.3 | 启动「嗯」 | **paraformer** | #3.2 修复后首 chunk 不 mask，副作用：首 chunk 是 ~0.6s 启动噪声（用户尚未说话），`alpha_sum≈1.3` 误 fire 出「嗯」，被首次静音 flush commit 成首段。**非**真首字近音，是启动噪声的 spurious fire | 确凿（e2e） |
| 3.4 | 停顿后丢字 | **paraformer** | 停顿 flush 后下一句首 chunk **非 `is_first`**（`num_processed_frames > 0`）→ `mask_left=true` 砍新句音头。会话首句首字（#3.2）已修，但段间首字仍丢 | 确凿（e2e） |
| 3.5 | 尾字/中段重复 | **paraformer** | **跨边界**：CIF 在音节跨 chunk 时相邻两 chunk 各 fire 一次同一 token（如「别」跨 chunk → 「识别别」）。**chunk 内**：高 alpha（~2.0）单 chunk 积分两次过阈值 → decoder 两次输出同 token（「语语」「开开」），模型层偏差，mask 无关 | 跨边界确凿；chunk 内为模型层 |

paraformer 的 `was_silent` 只插逗号、**不** `finish+reset`（`streaming_paraformer.rs` 流式不 reset），故不复用 zipformer 的首字症结。

## 4. 修复方案与设计决策

### 4.1 zipformer 首字（#3.1）— `041e678`

把「本轮是否有语音」传进 engine，让 `finish+reset` 只在**持续静音**（真·段边界）触发，排除「静音→语音过渡」tick：

- `step_silence` / `detect_silence_gap` 额外返回 `has_speech` → `(was_silent_for_punct, should_flush, has_speech)`。
- `StreamingEngine::accept_samples(samples, was_silent, has_speech)` trait 加 `has_speech` 参数（所有实现同步：local `StreamingSession`、`streaming_runner` 的 `FakeStreamingEngine`、server `pipeline.rs` 的 `FakeEngine`）。
- `streaming_engine.rs` ZipformerCtc / ZipformerTransducer 分支条件改 `if was_silent && !has_speech { finish+reset }`；Paraformer 分支忽略 `has_speech`（标点逻辑不变）。

### 4.2 paraformer 首字（#3.2）— mask 策略迭代

`process_chunk_at` 的 mask 决策，经三轮 e2e 迭代收敛：

| commit | 改动 | 效果 |
|---|---|---|
| `8f32f2f` | 首 chunk 也置零 left → 改为**首 chunk 不 mask left** | frame0 fired=0→1，首字 fire；但 right 仍 mask |
| `e802e98` | 首 chunk 也**不 mask right** | 首字能量保住；但关了**所有** chunk 的 right → 中段退化（中段 right 是 overlap 边界帧，不 mask → fired 增多 → 叠字/错字涨） |
| `3930dbf` | `mask_right = !(is_first \|\| is_final)`，**仅中段** mask right | 首字改善保留 + 中段质量回稳 |

**最终 mask 策略**（`process_chunk_at`）：

```rust
let is_first_chunk = self.num_processed_frames == 0;
let fresh = self.fresh_segment;          // 见 §4.4
let mask_left  = !(is_first_chunk || fresh);
let mask_right = !is_first_chunk && !is_final;
```

- **mask_left**：首 chunk 与 fresh 段首 chunk 关（保音头）；中段/final 开（去上 chunk overlap）。
- **mask_right**：仅中段开（去下 chunk overlap 边界帧，acoustic 不准）；首/final 关（保首字能量 + 尾音 fire）。

### 4.3 paraformer 启动「嗯」（#3.3）— `seen_speech` 门控 `968b7e5`

`StreamingRunner` 加 `seen_speech: bool` 锁存：

- VAD 在场时，首个 `has_speech` tick 锁存 `seen_speech=true`；**未锁存前不喂 engine**（丢弃启动噪声）。
- 首个 `has_speech` tick 整体喂入（含该 tick 内开头静音），故**不丢真实首字音头**；与 #4.2 配合：首 speech chunk → `is_first=true` → 真首字 fire。
- VAD=None（无 silero 模型）**不门控**，退回原行为喂全部，兼容测试 / 模型缺失环境。
- `finish_with_tail` 同步门控：纯噪声会话（`seen_speech=false`）不喂 tail → finish 返回空。
- `reset()` 清零。

### 4.4 paraformer 停顿后丢字（#3.4）— `fresh_segment` `a9b55ab`

`StreamingParaformer` 加 `fresh_segment: bool`：

- `flush()` 末尾置 `fresh_segment=true`。**关键安全性**：flush 用零 padding 收尾，结束后 `feat_cache` 已被冲成静音（非上段语音尾巴）→ 新段首 chunk 不 mask left **安全**——静音 alpha≈0 不会重 fire 上段尾，却保住新句音头。
- `process_chunk_at` 对 `fresh` 段首 chunk `mask_left=false`（即 §4.2 的 `mask_left = !(is_first || fresh)`）。
- **锁存语义**：`fresh_segment` 锁存到**新段首个 fire 的 chunk** 才清（若首 chunk 静音没 fire，`num_tokens=0`，保留 `fresh` 给下个 chunk），确保音头不错过；fire 后 `self.fresh_segment = false` 恢复正常 mask。
- `flush()` 开头先清 `fresh_segment=false`，避免上段 unconsumed 的 fresh 误 mask 当前段尾 chunk。
- `reset()` 清零。

### 4.5 paraformer 跨边界重复（#3.5 跨边界）— token 层去重 `1105798`

`process_chunk_at` step 8 累积 token 时跨边界去重：

```rust
if !seen_first_valid && (tid as i64) == self.last_emitted_token {
    seen_first_valid = true;
    continue;   // 本 chunk 首个有效 token == 上 chunk 末 token → CIF 双 fire，跳过
}
```

- 命中条件：本 chunk **首个有效** token == 上 chunk **末** token（CIF 双 fire 的特征）。
- **不影响**单 chunk 内合法重复（「爸爸」「常常」：两相同字在同一 chunk fire，不跨边界）。

## 5. 已知限制（接受，不进一步修）

| 现象 | 根因 | 决策 |
|---|---|---|
| chunk 边界中段音节偶发丢失（「始」） | 分块 CIF 固有限制：音节横跨 chunk 边界时被切 | 接受（彻底治需 flush 后全量 reset，风险大，违背「丢字优先已解」的现状） |
| chunk 内 CIF 双 fire 叠字（「语语」「开开」） | 模型层 alpha 偏差，单 chunk 积分两次过阈值 | 交 LLM 润色后处理；代码层文本去重有「爸爸」误杀风险，不做 |

## 6. 验证

- **单测**：`cargo test -p octopus-asr-local`——`streaming_runner`（10 例，含 `push_samples_gates_silence_until_speech_when_vad_present`）、`streaming_paraformer`（84 例，含真实模型 flush→accept 路径）全绿。
- **e2e**（paraformer，连说 6 句「开始语音识别」）：首字「开」稳定保留，启动「嗯」消失，停顿后段间首字不再丢；剩余「开开」/偶发「始」丢属 §5 已知限制。final 形如 `开始语音了识别，开语语音识别，开始语音识别，开开语音识别，开始语音识别，`（2/5 完全正确，其余可 LLM 兜底）。
- **回归**：长静音分段、停顿标点（逗号）行为不变；zipformer 首字（`was_silent && !has_speech`）逻辑正确。

## 7. 涉及 commit

| commit | 内容 |
|---|---|
| `041e678` | zipformer 首字：`has_speech` 区分段边界与开口过渡 |
| `c73af9c` / `54a0636` | 加 `[asr-diag]` 流式 token 诊断日志（诊断期） |
| `1105798` | paraformer 跨 chunk 边界 token 去重 |
| `73e350d` | paraformer 文本层重复哨兵（验证用，诊断期） |
| `8f32f2f` / `e802e98` / `3930dbf` | paraformer mask 策略迭代收敛 |
| `968b7e5` | paraformer 启动「嗯」：`seen_speech` 开口前门控 |
| `a9b55ab` | paraformer 停顿后丢字：`fresh_segment` |


---

## 来自原文件 `2026-06-27-global-edit-shortcut-design.md`

# 全局编辑快捷键（edit_global_shortcut）设计

> 日期：2026-06-27
> 状态：已实施（代码 + 编译验证通过；e2e 待用户桌面环境验证）
> 关联：clipboard-history-design（窗口管理）、coordinator 编辑态、asr-streaming-token-diagnostic（无关，仅同日）

## 1. 背景 / 动机

用户使用反馈：识别结果落在 `result_window`（always-on-top 浮窗），但用户常在**别的应用**里工作。要编辑识别文本必须先把 `result_window` 切到前台聚焦，再按窗口内 `edit_shortcut`（默认 Cmd+Enter）。这个「先聚焦窗口」的步骤在跨应用场景下很割裂——用户在浏览器/编辑器里，想改刚识别的一句话，得先点出 `result_window`。

现有 `edit_shortcut` 是**窗口内**快捷键：前端 `Result/index.tsx` 的 keydown 监听器匹配 `parseShortcut(edit_shortcut)` → `toggleEdit()`，仅当 `result_window` 聚焦时生效，**无法跨应用**触发。

用户要求：新增一个**全局**快捷键，任意应用聚焦时按一下就能编辑识别区；同时**保留**现有窗口内 Cmd+Enter 不动（用户明确约束：「保留焦点状态下的 CMD + Enter 进入编辑/保存不动」）。

## 2. 设计目标

- **跨应用**：任意应用聚焦时按全局键 → 唤起 `result_window` 到前台 + 进入编辑态。
- **与窗口内 Cmd+Enter 并存**（不替换、不冲突）：Cmd+Enter 继续管「结果窗已聚焦时的编辑 toggle」，全局键管「跨应用唤起 + toggle」。
- **toggle 语义一致**：全局键也是「进入/保存同键」（复用 `toggleEdit`）。
- **可配置 + 热重载 + 冲突检测**，复用现有 `asr_shortcut` 的设置基础设施。
- **空文本保护**：无识别结果时全局键只唤起窗口、不进空编辑。

## 3. 设计

### 3.1 新配置字段 `edit_global_shortcut`

- `AppConfig.edit_global_shortcut: String`（`crates/infra/src/config.rs`），`#[serde(default = "default_edit_global_shortcut")]`，默认 `"CmdOrCtrl+Shift+E"`。
- 默认值选择：与 `asr_shortcut`（`CmdOrCtrl+Shift+Z`）同系列（`CmdOrCtrl+Shift+<字母>`），`E` = Edit 易记；不与 `clipboard_shortcut`（`Alt+V`）/ `edit_shortcut`（`Cmd+Enter`）冲突。
- `db.sql` `app_config` seed 加一行（新安装用户）；老 DB 缺该行时 serde default 兜底（`load_config` 反序列化容错）。
- `impl Default for AppConfig` 同步加字段初始化。

### 3.2 后端：全局快捷键 handler

新增两个函数（`crates/desktop/src/result_window.rs`）：

- `trigger_global_edit(app)`：`show` + `set_focus` `result_window` + `emit("global-edit-toggle", ())`。
- `register_edit_global_shortcut(app, shortcut_str)`：解析 Accelerator → `on_shortcut` 注册，handler（Pressed 时）调 `trigger_global_edit`。与 `shortcut::register_shortcut`（handler 调 `coordinator.toggle()`）的区别仅在此 handler。

注册时机：`main.rs` setup 阶段，紧跟 `asr_shortcut` 注册之后（读 `config.edit_global_shortcut`）。

### 3.3 前端：事件 → toggleEdit

`Result/index.tsx` 在 `toggleEdit` 声明**之后**加独立 `useEffect`，`listen("global-edit-toggle", () => toggleEdit())`。

- **为什么独立 useEffect 而非并入主事件数组**：`toggleEdit` 是 `const`（TDZ），主事件 useEffect（L129）在 `toggleEdit` 声明（L247）之前，前置引用触发 TS2448。独立 useEffect 放声明之后规避。
- 复用 `toggleEdit` = `editingRef.current ? commitEdit() : enterEdit()`：未编辑→进入，已编辑→保存，与窗口内 Cmd+Enter 同语义。
- `enterEdit` 已自带 `if (!displayedRef.current.trim()) return`：无识别结果时全局键只唤起窗口、不进空编辑。

### 3.4 热重载 + 冲突检测（复用 asr_shortcut 模式）

- `settings_commands.rs::set_config`：`edit_global_shortcut` 变更时 `unregister` 旧的 + `register_edit_global_shortcut` 新的，注册成功才持久化，失败恢复旧值并返回 Err（同 `asr_shortcut` 2026-06-21 审查修复）。
- `apply_config_value` 加 `edit_global_shortcut` 分支（字符串校验）。
- `check_shortcut` 通用冲突检测（注册 → 立即注销），设置 UI 键盘捕获时自动复用。

### 3.5 设置 UI

`GeneralPanel.tsx`「快捷键」卡片加「全局编辑」行（`ShortcutButton` 组件），复用 `startShortcutCapture("edit_global_shortcut")` → `check_shortcut` + `setVal`。窗口内「编辑模式」（`edit_shortcut`）配置行已**移除**——Cmd+Enter 固定默认值，不再在设置页管理（功能靠字段 default + 前端 keydown 保留）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 无识别结果（Idle / 空文本） | 全局键 show+focus 窗口；`enterEdit` 空文本 return，不进编辑 |
| 录音中（Streaming stage） | 全局键进编辑 → `handle_enter_edit_mode` 硬暂停 ASR（与窗口内 Cmd+Enter 录音中按下同行为） |
| 编辑中再按全局键 | `toggleEdit` → `commitEdit` 保存（toggle 语义） |
| 失焦后按全局键 | show+set_focus 重新激活；若在编辑态则 commit（toggle）——用户失焦后想继续编辑可点回编辑区 |
| 与 asr_shortcut 撞键 | Tauri `on_shortcut` 后注册覆盖/报错；设置 UI 改键时 `check_shortcut` + 注册失败恢复旧值兜底 |
| 编辑态按 ESC（窗口内） | `cancelEdit`：退出编辑 + 还原原文快照 + 不写 DB（放弃编辑）；非编辑态 ESC 仍放弃录音——编辑态需按 2 次 ESC 才放弃录音。保存走 Cmd+Enter / 工具栏「保存编辑」按钮 |

**为什么全局键也 toggle（而非只进入）**：与窗口内 Cmd+Enter 语义一致（用户心智模型统一「编辑键 = 进入/保存」），且复用 `toggleEdit` 零额外代码。

## 5. 不改动 / 持久化

- 窗口内 `edit_shortcut`（Cmd+Enter）**功能**完全保留（前端 keydown + 字段 default），但**设置页配置行已移除**——固定 Cmd+Enter，不再可改（用户要求）。
- `enter_edit_mode` / `commit_edit` 后端命令、`handle_enter_edit_mode` / `commit_edit_apply` 逻辑不变——全局键复用现有编辑态链路。

### 5.1 DB 持久化（`crates/infra/src/db.rs`）

`load_app_config_at` / `save_app_config_at` 是**显式字段列表**（非 serde 全量），每加一个 `AppConfig` 字段必须同步在这两处补行，否则：`set_config` 改了不写 DB（热重载内存生效但重启回退）+ `get_config` 从 DB load 不到该字段 → 回退 serde default（设置页显示默认值，正是本次报告的 bug）。

`edit_global_shortcut` 必须在两处补：
- `load_app_config_at`：字符串字段区 `"edit_global_shortcut" => cfg.edit_global_shortcut = value`
- `save_app_config_at`：`fields` 数组 `("edit_global_shortcut", cfg.edit_global_shortcut.clone())` + 数组长度 `25 → 26`

**隐藏前提（2026-06-28 踩坑）**：`load_app_config_at` 用 `WHERE category='setting'` 过滤，而该行的 `category` 必须真是 `'setting'`。老库 schema 的 `category` 列 DEFAULT 曾为 `'default'`（`db.sql` 后改 `'setting'`，但 `CREATE TABLE IF NOT EXISTS` 不更新老表列 DEFAULT），导致新字段首次 `set_config` 写入时拿到 `'default'` → load 漏读 → 设置页回退 serde 默认值（现象：「改键生效但显示错」）。修复 = 确保 DB 列 DEFAULT=`'setting'` + 既有 `default` 行改回 `setting`。代码层无需改动。


---

## 来自原文件 `2026-06-27-image-storage-blob-design.md`

# 图片存储迁移：文件系统 → DB BLOB

**日期**: 2026-06-27
**状态**: ✅ 实施完成（image_data 表 + insert/get API + image_migration 自动迁移，DB v7+）
**分支**: `feature/clipboard-research`

## 0. 概述

将剪贴板图片从文件系统（`~/.octopus/clipboard_images/`）迁移到 SQLite DB BLOB 存储。消除文件与 DB 不一致风险，防止用户误删，简化回收逻辑。

## 1. 新增表

```sql
CREATE TABLE IF NOT EXISTS image_data (
    hash       TEXT PRIMARY KEY,     -- SHA-256(PNG bytes)，去重键
    blob       BLOB NOT NULL,        -- 图片原图 BLOB（格式见 image_type）
    thumb      BLOB NOT NULL,        -- 缩略图 BLOB（240×240 resize）
    image_type TEXT NOT NULL DEFAULT 'webp',  -- BLOB 格式：webp（预留 png/jpeg 扩展）
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
```

`clipboard_history.blob_hash` 通过应用层引用 `image_data.hash`（不用外键约束——SQLite 外键默认关闭，且跨表 CASCADE 在 PRAGMA foreign_keys=OFF 下不生效，改用应用层引用计数）。

## 2. 存储策略

| 存储项 | 格式 | 用途 |
|---|---|---|
| `blob` | WebP 100% 无损 | OCR 识别 + 用户导出（转 JPEG/PNG） |
| `thumb` | WebP 20%，240×240 Lanczos resize | 前端列表内联展示 |

**体积估算**：典型截图 PNG 200-500KB → WebP 无损 100-300KB；缩略图 3-8KB。500 张约 50-150MB + 缩略图 2-4MB。

**为什么 WebP 无损而非有损**：
- 剪贴板截图含文字/图标，有损压缩会产生伪影影响 OCR 精度
- WebP 无损对截图类图片压缩率优于 PNG（约 20-30% 体积缩减）
- 最大 500 条，体积可控

## 3. 编码流程（watcher.rs 修改）

```
剪贴板图片事件
  │
  ├─ clipboard-rs get_image() → RGBA pixels
  ├─ encode_and_hash() → PNG bytes（仅用于 SHA-256 去重 hash）
  ├─ 去重：hash 已在 image_data 表？→ 跳过编码，只插入 clipboard_history 行
  ├─ 复用原始 RGBA 构造 DynamicImage → WebP 100% 无损（webp crate Encoder::encode_lossless）
  │  （encode_to_webp 接 &DynamicImage，不再二次解码上一步的 PNG）
  ├─ resize 240×240 (image crate Lanczos3) → WebP 20%（Encoder::encode(20.0)）
  └─ INSERT INTO image_data (hash, blob, thumb, width, height, created_at)
```

## 4. 读取流程

### 4.1 OCR 识别

```
ocr_image 命令
  → clipboard_history.blob_hash
  → SELECT blob FROM image_data WHERE hash = ?
  → image::load_from_memory(webp_bytes) → DynamicImage
  → OcrEngine::recognize(&DynamicImage)
```

### 4.2 前端缩略图展示

Tauri 命令 `get_image_thumb(id: i64) -> String`（返回完整 data URL，非裸 `Vec<u8>`）：
```
→ clipboard_history.blob_hash
→ SELECT thumb FROM image_data WHERE hash = ?
→ 后端 base64 编码 → 返回 "data:image/webp;base64,..."
```

后端一次编码成 data URL，避免 Tauri IPC 把 `Vec<u8>` 序列化成 JSON 数字数组（4-5x 膨胀）+ 前端 `map/join/btoa` 转换。前端 `<img src={dataUrl}>` 直接展示。

### 4.3 导出保存

`save_image_item` 修改：
```
→ SELECT blob FROM image_data WHERE hash = ?  （WebP 无损 bytes）
→ 用户选格式（JPEG/WebP/PNG）
  ├─ PNG: image crate 解码 WebP → 重新编码 PNG
  ├─ JPEG: image crate 解码 WebP → JpegEncoder
  └─ WebP: 直接写原始 bytes（已是无损 WebP）
→ 写入 ~/Downloads/octopus/
```

## 5. 删除流程（引用计数）

```
delete_item(id)
  → 读 clipboard_history.blob_hash
  → DELETE FROM clipboard_history WHERE id = ?
  → SELECT COUNT(*) FROM clipboard_history WHERE blob_hash = ?
  → count == 0 ? DELETE FROM image_data WHERE hash = ? : 跳过
```

`clear_history` 同理：批量删 clipboard_history 后，清理无引用的 image_data 行。

**不再需要**：`cleanup_orphaned_blobs`、`delete_blob_files`、`clipboard_images/` 目录。

## 6. 删除清单

| 删除项 | 原因 |
|---|---|
| `clipboard/src/image.rs::clipboard_images_dir()` | 不再使用文件系统 |
| `clipboard/src/image.rs::save_image()` | 替换为 DB 写入 |
| `clipboard/src/image.rs::generate_thumbnail()` | 缩略图在编码时一步生成 |
| `clipboard/src/image.rs::cleanup_orphaned_blobs()` | 改为 DB 引用计数 |
| `clipboard/src/image.rs::delete_blob_files()` | 改为 DB DELETE |
| `clipboard/src/cleanup.rs::run_cleanup` 中 blob 回收步骤 | 改为 DB 清理 |
| `~/.octopus/clipboard_images/` 目录 | 历史数据，迁移后删除 |

## 7. 迁移策略

一次性迁移（应用启动时检测）：
```
if clipboard_images/ 目录存在:
  for each <hash>.png in 目录:
    if image_data 中无此 hash:
      PNG → WebP 无损 + 缩略图 → INSERT image_data
  迁移完成后删除 clipboard_images/ 目录
```

迁移幂等——已存在的 hash 跳过。

## 8. 依赖

已有依赖，无需新增：
- `webp = "0.3"`（infra 已有）— WebP 编码
- `image = "0.25"`（clipboard 已有）— resize + 格式转换

## 9. DB 版本

v6 → v7 迁移：新增 `image_data` 表。

## 10. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| DB 膨胀影响查询性能 | 低 | Mutex 阻塞 | SQLite BLOB 读写不经 Mutex 行锁，页级锁；实测 200KB BLOB 读写 <1ms |
| 迁移中断 | 低 | 部分图片未迁移 | 幂等设计，下次启动继续 |
| WebP 编码性能 | 低 | 监听延迟 | webp crate 编码 200KB PNG 约 5-10ms，不阻塞 |
| 旧 clipboard_images 目录残留 | 低 | 磁盘浪费 | 迁移成功后删除目录 |


---

## 来自原文件 `2026-06-27-ocr-module-design.md`

# OCR 模块设计

**日期**: 2026-06-27
**状态**: ✅ 实施完成（OcrEngine + 模型管理 + 3 个 Tauri 命令：ocr_image/ocr_screenshot/save_ocr_to_note）
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 0. 概述

为 octopus 新增 OCR（光学字符识别）能力，基于 PaddleOCR（PP-OCRv6）模型 + `ocr-rs` Rust 库（MNN 推理后端）。一期仅用于剪贴板图片识别——用户在剪贴板浮窗或管理页对图片条目点「OCR」按钮，识别文本写入 `search_text`（FTS5 可搜索）+ 写入系统剪贴板 + 用系统文本编辑器新建无标题文档展示结果。

OCR 作为独立 crate（`octopus-ocr`），与 `octopus-asr-local` 平级，一期仅剪贴板场景调用，未来可被 CLI/Server 复用。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── ocr/              # octopus-ocr — 新增，依赖 infra
│   ├── Cargo.toml    # ocr-rs = "2.3", image = "0.25"
│   └── src/
│       ├── lib.rs    # pub use，模块入口
│       ├── engine.rs # OcrEngine 封装：模型加载 + recognize() + 单例缓存
│       └── model.rs  # 模型路径管理 + 就绪检测
├── infra/            # octopus-infra — 复用 models 表 + app_config
├── clipboard/        # octopus-clipboard — 不直接依赖 ocr（由 desktop 调用）
└── desktop/          # octopus-desktop — Tauri 命令 + 前端按钮
```

**依赖关系**：`infra ← ocr ← desktop`（clipboard 不依赖 ocr，desktop 作为编排层调用 ocr + clipboard）

### 1.2 为什么不用 ort（ONNX Runtime）

项目 ASR 用 `ort` 做 ONNX 推理。OCR 选择 `ocr-rs`（MNN 后端）而非 ort 手动实现，原因：

- `ocr-rs` 封装了完整 pipeline（det → crop → cls → rec），API 干净
- `ort` 路线需自己实现 DBNet 后处理（expand/shrink boxes）、CRNN CTC 解码、图片预处理——工作量大且调参痛苦
- OCR 是独立 crate，推理后端隔离合理（MNN 比 ONNX Runtime 更轻量）
- HF 上的 PaddlePaddle 模型是 ONNX 格式，但 `ocr-rs` 官方仓库提供对应的 MNN 转换模型 + 字典文件

### 1.3 engine.rs 核心接口

```rust
pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

impl OcrEngine {
    /// 全局单例，首次调用时懒加载模型（OnceLock<Arc<OcrEngine>>）。
    /// model_name 从 app_config.ocr_model 读取。
    pub fn instance() -> Result<Arc<OcrEngine>>;

    /// 识别图片字节（PNG），返回识别文本（多行用 \n 连接）。
    pub fn recognize(&self, png_bytes: &[u8]) -> Result<String>;
}
```

### 1.4 model.rs

```rust
/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf;

/// 检查模型组三件套是否就绪（det.mnn + rec.mnn + keys.txt）
pub fn is_model_ready(model_name: &str) -> bool;

/// 默认模型名
pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv6-small";
```

## 2. 模型管理

### 2.1 det/rec 分离的存储设计

HuggingFace 上 det 和 rec 是独立 repo（`PaddlePaddle/PP-OCRv6_small_det_onnx` / `_rec_onnx`），但 `ocr-rs` 用的是 MNN 格式，从 ocr-rs 官方 GitHub 仓库下载。

**一个 OCR 模型组 = det.mnn + rec.mnn + keys.txt**，对用户呈现为一个可选项（如「PP-OCRv6-small」）。

### 2.2 models 表复用（零 schema 变更）

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, ...)
VALUES
  ('ocr', 'paddleocr', 'ocr', 'PP-OCRv6-small',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',  -- source: det 下载地址
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',  -- secret_key: rec 下载地址（本地模型复用此字段存下载 URL）
   ...);
```

**字段复用语义**：

| 字段 | ASR local 用途 | OCR 用途 |
|---|---|---|
| `domain` | `'asr'` | `'ocr'` |
| `source` | HF repo | det 模型下载 URL |
| `secret_key` | 空（本地模型） | rec 模型下载 URL |
| `category` | 引擎族 | `'ocr'`（统一） |
| `is_local` | 1 | 1 |
| `is_streaming` | 0/1 | 0 |

### 2.3 app_config

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
  ('ocr_model', 'PP-OCRv6-small', 'setting');
```

### 2.4 文件布局

```
~/.octopus/models/ocr/
└── PP-OCRv6-small/
    ├── det.mnn       4.7M   文本检测模型
    ├── rec.mnn       10M    文本识别模型
    └── keys.txt      73K    字符字典
```

keys.txt 与 rec 模型配套（同仓库下载，固定 URL）。

### 2.5 下载流程

一期手动放置（已就绪）。后续接入模型管理页时：
1. 用户点「下载」
2. 读 `models` 表 source（det URL）→ 下载 det.mnn
3. 读 secret_key（rec URL）→ 下载 rec.mnn
4. keys.txt 从固定 URL 或 rec URL 同目录下载
5. 三件套就位 → `is_enabled` 置 1

## 3. OCR 触发流程

### 3.1 完整流程

```
用户点击图片条目「OCR」按钮
         │
         ▼
┌──────────────────────────────────┐
│  前端 invoke("ocr_image", { id }) │
│  按钮 → loading（spin + 不可点）   │
└──────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────┐
│  后端 ocr_image 命令              │
│  1. 查 DB 拿 blob_hash            │
│  2. 读 ~/.octopus/clipboard_      │
│     images/<hash>.png             │
│  3. 读 app_config.ocr_model       │
│     → 无配置？Err                 │
│  4. is_model_ready(model_name)    │
│     → 否？Err("需下载模型")        │
│  5. OcrEngine::instance()         │
│     → recognize(png_bytes)        │
│  6. 识别文本写入 search_text       │
│  7. 文本写入系统剪贴板             │
│  8. 系统文本编辑器新建无标题文档    │
│  9. 返回识别文本                   │
└──────────────────────────────────┘
```

### 3.2 结果处理（三步，无临时文件）

1. 识别文本写入 `clipboard_history.search_text`（FTS5 触发器自动更新索引，图片变可搜索）
2. 文本写入系统剪贴板（`ClipboardHandle::write_text`，用户可直接 Cmd+V）
3. 系统文本编辑器新建无标题文档，内容为 OCR 文本（用户可编辑/保存/丢弃）

**新建文档方式**：
- **macOS**：osascript 让 TextEdit 新建文档 + 设置文本
  ```applescript
  tell application "TextEdit"
    activate
    make new document with properties {text:"OCR文本"}
  end tell
  ```
- **Windows**：启动 notepad，剪贴板已有文本（用户 Ctrl+V 或后续 SendInput）
- **Linux**：启动 gedit/文本编辑器，剪贴板已有文本

**不落盘临时文件**——避免系统污染和遗忘清理。

### 3.3 模型下载检测

首次点击 OCR 时：
```
is_model_ready？ → false → toast「请先在设置中下载 OCR 模型」
```

一期模型已手动放置，下载流程后续接入。

## 4. 前端集成

### 4.1 入口位置

剪贴板浮窗（`ClipboardItem.tsx`）+ Settings 剪贴板管理页（`ClipboardPanel.tsx`），仅 `item_type === "image"` 的条目显示 OCR 按钮（`ScanText` 图标，lucide-react）。

### 4.2 按钮状态机（三态 + 过程提示）

```
idle ──→ loading ──→ done（✓ 0.7s）──→ idle
              │
              └─→ error（toast）──→ idle
```

**loading 态分阶段反馈**：
- 按钮 → `Loader2` spin，`disabled`（不可重复触发）
- 模型下载中（如需）→ toast「正在下载 OCR 模型…」
- OCR 识别中 → 按钮旁浮现「识别中…」小字（`animate-pulse`，stone-400，10px）

**done 态**：
- 成功 → ✓ emerald-600（0.7s）→ toast「已识别」+ 系统 .txt 弹出
- 无文字 → ✓ amber-500（0.7s）→ toast「未识别到文本」

**error 态**：
- toast「OCR 失败：…」→ 按钮恢复 idle

### 4.3 与现有按钮的关系

- OCR 与「保存图片」独立操作，不互斥
- OCR 不改变图片条目视觉（只更新 `search_text`）
- 已 OCR 过的图片可重复点击（覆盖 `search_text`）

### 4.4 Settings 模型管理页

后续迭代接入。在 ModelsPanel 新增 OCR 分区，与 ASR 引擎并列，支持下载/切换/删除。一期不做。

## 5. 数据流与存储

### 5.1 DB（零 schema 变更）

OCR 只写 `clipboard_history.search_text`：
```
OCR 前：search_text = ""（空，图片条目一直是空的）
OCR 后：search_text = "识别出的文本" → FTS5 触发器 clip_fts_au 自动更新索引
```

**不变量**：
- OCR 只写 `search_text`，不改 `content`（content 始终是 blob_hash）
- OCR 不碰 `transcriptions` 表（ASR 专属）
- 重复 OCR 覆盖 `search_text`，不做版本历史
- `item_type` 保持 `image`，不因 OCR 变成 text

### 5.2 models 表 seed

`db.sql` 新增 OCR 模型 seed：
```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
  ('ocr','paddleocr','ocr','PP-OCRv6-small',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',
   'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',
   'auto','PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日',
   1,1,0);
```

`is_enabled=1`（一期手动放置模型，标记为已就绪）。

### 5.3 app_config seed

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
  ('ocr_model', 'PP-OCRv6-small', 'setting');
```

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 模型未下载 | `is_model_ready` 返回 false → toast「请先在设置中下载 OCR 模型」 |
| 模型加载失败 | MNN 初始化异常 → toast「OCR 模型加载失败」+ 记 error 日志 |
| 图片读取失败 | blob 文件丢失/损坏 → toast「图片文件读取失败」 |
| OCR 结果为空 | 纯图片无文字 → toast「未识别到文本」 |
| OCR 结果含特殊字符 | 原样写入 search_text，FTS5 trigram 正常索引 |
| 重复 OCR | 覆盖 search_text，不做版本 |
| 多语言文本 | PP-OCRv6 small 支持中/英/繁体/日，无需切换 |
| 超大图片 | 无限制（det 阶段内部 resize），>50MB PNG 可能内存紧张 → 记日志继续 |

**并发安全**：`OcrEngine::instance()` 用 `OnceLock<Arc<OcrEngine>>`，全局单例，无锁读取。模型加载只在首次调用时发生一次。

**降级**：OCR 是可选功能，不影响剪贴板核心流程。OCR 失败只 toast 报错，图片条目仍可正常复制/保存/收藏。

## 7. 依赖变更

**新增（Rust）**：
- `ocr-rs = "2.3"`（MNN 推理 + PaddleOCR pipeline 封装）
- `image = "0.25"`（图片解码，infra 已有）

**构建依赖**：
- `cmake` + `cc`（ocr-rs 的 MNN FFI 编译需要）

**新增（前端）**：
- `ScanText` 图标（lucide-react，已有）

## 8. 实施分期

| 阶段 | 范围 | 依赖 |
|---|---|---|
| **Step 1** | octopus-ocr crate（engine.rs + model.rs + 集成测试） | 模型已就绪 |
| **Step 2** | desktop ocr_image Tauri 命令 + DB seed（models + app_config） | Step 1 |
| **Step 3** | 前端 OCR 按钮（ClipboardItem + ClipboardPanel）+ 状态机 | Step 2 |
| **Step 4**（后续） | Settings 模型管理页 OCR 分区 + 下载流程 | Step 2 |

## 9. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| ocr-rs MNN 编译问题（macOS arm64） | 中 | 无法构建 | 提前验证 cargo build；MNN 预编译库覆盖主流平台 |
| ocr-rs API 变更（v2.x 仍在快速迭代） | 低 | 接口不兼容 | 锁定版本 `= "2.3.1"` |
| macOS osascript TextEdit 权限 | 低 | 新建文档失败 | 降级为只写剪贴板 + toast |
| MNN 与 ort 共存内存冲突 | 极低 | 推理崩溃 | 独立 crate 隔离 + 测试验证 |

## 10. 实施偏差与补充

### 10.1 图片存储迁移影响

OCR spec 原设计从文件系统 `~/.octopus/clipboard_images/<hash>.png` 读取原图。实施过程中图片存储迁移到 DB BLOB（详见 `2026-06-27-image-storage-blob-design.md`），OCR 读取路径相应调整：
- `ocr_image` 命令从 `image_data` 表读 WebP BLOB（不再读文件）
- `OcrEngine::recognize` 改用 `image::load_from_memory`（自动检测格式，支持 WebP）

### 10.2 ocr-rs 实际 API

- `OcrEngine::new(det_path, rec_path, charset_path, config)` — 接受 `impl AsRef<Path>`
- `recognize(&DynamicImage)` → `OcrResult<Vec<OcrResult_>>`，`OcrResult_` 有 `.text` 字段
- MNN 预编译库从 GitHub 自动下载（build script），release 构建需手动放置预编译包

### 10.3 osascript 输出静默

osascript 创建 TextEdit 文档时会在 stderr 输出「document 未命名」，需 `.stdout(Stdio::null()).stderr(Stdio::null())` 静默。

> **注**：§3.2 的「写 search_text + 系统剪贴板 + osascript TextEdit」三步落库流程已被后续 `clean-used-feature` 改造取代——OCR 现为纯识别 → `insert_ocr_clipboard_item` 入 `source=ocr` 剪贴板条目 → CompactEditor 多 tab 打开编辑（详见 `2026-07-03-clean-used-feature-design.md`）。本节保留作历史决策记录。

### 10.4 超长图切分（2026-07-03）

`recognize` 对 `height > 1600`px 的长图（如长截图 2032×15796）按块切分逐块识别：常量 `SPLIT_HEIGHT_THRESHOLD=1600` / `CHUNK_HEIGHT=1280` / `CHUNK_OVERLAP=200`；`plan_chunks(h)` 纯函数规划 `(top, chunk_h)` 列表（步长 = 块高 − 重叠，末块补齐到 h），`recognize_long_image` 逐块 `crop_imm` + 识别 + 跳过与上一块末行相同的起始连续行去重。动机：det `max_side_len=960` 会等比缩放长边，超长图短边过小致检测不到文本（曾返回 text_len=0）。算法 + 单测见 `crates/ocr/src/engine.rs`。详见 `architecture.md` octopus-ocr 节。

### 10.5 全局并发互斥（2026-07-03）

同一时刻仅允许一个 OCR 任务：`OcrLockGuard`（`static OCR_BUSY: AtomicBool` + `compare_exchange(false, true, Acquire, Acquire)` 的 RAII guard，`Drop` 时 `store(false, Release)`）在 `ocr_image` / `ocr_screenshot` 入口 `try_acquire`，忙则立即 `Err("前一个 OCR 还未完成，请稍后")`。guard 绑定命令返回值生命周期，async future 被 cancel 时正常 drop 释放（不会泄漏锁）。前端 4 入口（ClipboardItem / ImagePreview / Screenshot / ClipboardPanel）catch 该错误给出可见提示。详见 `architecture.md`。


---

## 来自原文件 `2026-06-28-polish-global-shortcut-design.md`

# 全局立即润色快捷键（polish_global_shortcut）设计

> 日期：2026-06-28
> 状态：✅ 实施完成
> 关联：[global-edit-shortcut-design](./2026-06-27-global-edit-shortcut-design.md)（同模式复刻）、coordinator `PolishNow`、result_window 窗口管理

## 1. 背景 / 动机

用户已有三个全局快捷键：`asr_shortcut`（语音识别）、`edit_global_shortcut`（语音编辑）、`clipboard_shortcut`（剪贴板浮窗）——任意应用聚焦时跨应用触发。

现在缺一个「立即润色」的全局入口。现有立即润色只能通过结果窗工具栏的「立即润色」按钮（前端 `invoke("polish_now")`）触发，**必须结果窗聚焦**。用户在浏览器/编辑器工作时，想对刚识别的文本立即润色（不等 `polish_mode` 自动润色、也不切到结果窗点按钮），得先把结果窗切到前台——跨应用场景下割裂。

用户要求：新增第四个全局快捷键，任意应用聚焦时按一下就对当前识别结果立即润色。**复刻 `edit_global_shortcut` 的模式**（config 字段 + result_window handler + 热重载 + 前端事件 + 设置 UI 行）。

## 2. 设计目标

- **跨应用**：任意应用聚焦时按全局键 → 对当前识别结果立即润色（等价工具栏「立即润色」/ `polish_now`）。
- **与工具栏按钮一致**：复用同一润色逻辑（loading 状态 + toast 反馈 + 幂等门控），不另起一套。
- **仅显示不聚焦**：唤起结果窗 `show` 让用户看到润色结果，但**不抢键盘焦点**（不 `set_focus`）——不打断当前应用输入。
- **空文本保护 + 幂等**：无识别结果时静默无操作；润色进行中再按幂等忽略。
- **可配置 + 热重载 + 冲突检测**，复用现有快捷键设置基础设施。

## 3. 设计

### 3.1 新配置字段 `polish_global_shortcut`

- `AppConfig.polish_global_shortcut: String`（`crates/infra/src/config.rs`），`#[serde(default = "default_polish_global_shortcut")]`，默认 `"CmdOrCtrl+Shift+S"`。
- 默认值选择：与同批调整的 `asr_shortcut`（`CmdOrCtrl+Shift+A`）/ `edit_global_shortcut`（`CmdOrCtrl+Shift+E`）/ `clipboard_shortcut`（`CmdOrCtrl+Shift+D`）统一为 `CmdOrCtrl+Shift+<字母>` 系列（S=润色、A=ASR、E=Edit、D=剪贴板），窗口内 `edit_shortcut`（`Cmd+Enter`）不冲突。
- `db.sql` `app_config` seed 加一行（新安装用户）；老 DB 缺该行时 serde default 兜底。
- `impl Default for AppConfig` 同步加字段初始化 + 单测断言默认值。

### 3.2 后端：全局快捷键 handler

新增两个函数（`crates/desktop/src/result_window.rs`），复刻 `trigger_global_edit` / `register_edit_global_shortcut`：

- `trigger_global_polish(app)`：`show` 结果窗（**不 `set_focus`**，区别于 edit_global）+ `emit("global-polish-trigger", ())`。
- `register_polish_global_shortcut(app, shortcut_str)`：解析 Accelerator → `on_shortcut` 注册，handler（Pressed 时）调 `trigger_global_polish`。

注册时机：`main.rs` setup 阶段，紧跟 `register_edit_global_shortcut` 之后（读 `config.polish_global_shortcut`）。

**为什么 show 不 set_focus**：润色是后端动作（`polish_now` 发 `Command::PolishNow`），不需要窗口聚焦接收键盘输入（编辑才需要）。用户在别的应用输入时按润色键，`show` 让其看到结果，不 `set_focus` 避免抢走当前输入焦点。

### 3.3 前端：事件 → polishNow

`Result/index.tsx` 把现有 polish-now 按钮 onClick 的逻辑抽成 `polishNow`（`useCallback`），按钮与全局事件共用：

```ts
const polishNow = useCallback(async () => {
  if (polishLoading) return;                    // 进行中幂等忽略
  if (!displayedRef.current.trim()) return;     // 无结果静默
  setPolishLoading(true);
  try { await invoke("polish_now"); showToast("润色中…"); }
  catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
}, [polishLoading, showToast]);
```

- polish-now 工具按钮 `onClick` 改用 `polishNow`（行为零差异）。
- 新增独立 `useEffect`（在 `polishNow` 声明之后）：`listen("global-polish-trigger", () => polishNow())`。
  - **独立 useEffect 规避 TDZ**：与 `global-edit-toggle` 同理（`polishNow` 是 `const`，主事件 useEffect 在其声明之前，前置引用触发 TS2448）。

**无结果时的窗口行为（方案 a，已选定）**：后端 `trigger_global_polish` 无条件 `show + emit`；前端 `polishNow` 判空 return 不润色。即无结果时结果窗被 show（`#container` opacity:0，视觉几乎无害），但不触发润色——与 `edit_global`（无结果 show 窗不进编辑）完全对称。文本在前端 `displayedRef`，后端判不了空，故采用「后端无条件 show + 前端判空」的对称模式，不另加 show command（舍去方案 b 的额外往返）。

### 3.4 热重载 + 冲突检测（复用 asr/edit_global 模式）

- `settings_commands.rs::set_config`：`polish_global_shortcut` 变更时 `unregister` 旧的 + `register_polish_global_shortcut` 新的，注册成功才持久化，失败恢复旧值并返回 Err（同 `asr_shortcut` / `edit_global_shortcut`）。`old_shortcut` 拆分为 `old_asr` + `old_edit_global` + `old_polish_global`。
- `apply_config_value` 加 `polish_global_shortcut` 分支（字符串校验）。
- `check_shortcut` 通用冲突检测（注册 → 立即注销），设置 UI 键盘捕获时复用。

### 3.5 设置 UI

`GeneralPanel.tsx`「快捷键」卡片加「立即润色」行（`ShortcutButton` 组件），复用 `startShortcutCapture("polish_global_shortcut")` → `check_shortcut` + `setVal`。快捷键卡片现有行：语音识别 / 剪贴板浮窗 / 语音编辑，新增「立即润色」（位置紧跟语音编辑之后）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 无识别结果（Idle / 空文本） | 后端 show 结果窗（透明，opacity:0 视觉无害）+ emit；前端 `polishNow` `trim()` 判空 return，不润色（与 edit_global 对称） |
| 润色进行中（polishLoading）再按 | 前端 `polishLoading` 门控 return，幂等忽略（与工具栏按钮 disabled 一致） |
| 录音中（Streaming）按 | `polish_now` 对当前 transcript 触发润色（与工具栏按钮录音中按下同行为） |
| 与其它快捷键撞键 | Tauri `on_shortcut` 后注册覆盖/报错；设置 UI 改键时 `check_shortcut` + 注册失败恢复旧值兜底 |
| 结果窗当前隐藏 | `show` 让窗口可见，润色完成后 `update-result` 显示润色文本 |

**为什么只 show 不 set_focus**：润色不需窗口接收键盘（区别于编辑），不抢焦点 = 不打断用户当前应用输入。

## 5. 不改动 / 持久化

- 工具栏「立即润色」按钮**功能不变**，仅 onClick 改抽出的 `polishNow`（行为零差异）。
- `polish_now` 后端命令、`Command::PolishNow`、coordinator 润色逻辑不变——全局键复用现有润色链路。
- `polish_mode`（自动润色）不受影响——全局键是手动立即润色入口，与自动模式独立。

### 5.1 DB 持久化（`crates/infra/src/db.rs`）

`load_app_config_at` / `save_app_config_at` 是显式字段列表，每加一个 `AppConfig` 字段必须同步补行（漏则 `set_config` 不写 DB + `get_config` load 不到 → 设置页回退 serde default）。

`polish_global_shortcut` 必须在两处补：
- `load_app_config_at`：`"polish_global_shortcut" => cfg.polish_global_shortcut = value`
- `save_app_config_at`：`fields` 数组 `("polish_global_shortcut", cfg.polish_global_shortcut.clone())` + 数组长度 `26 → 27`

**隐藏前提**（`edit_global_shortcut` 2026-06-28 踩过的坑，详见 [global-edit-shortcut-design §5.1](./2026-06-27-global-edit-shortcut-design.md)）：`load_app_config_at` 用 `WHERE category='setting'` 过滤，该行 `category` 必须真是 `'setting'`。老库 `category` 列 DEFAULT 曾为 `'default'`，新字段首次 `set_config` 可能拿到 `'default'` → load 漏读 → 设置页回退默认（「改键生效但显示错」）。当前 db.sql DEFAULT=`'setting'` + 既有 migration（`db.rs` `UPDATE ... 'default'→'setting'`）对新装/已迁移库正确；若老库列 DEFAULT 仍是 `'default'`，新字段需确保 seed/migration 写 `category='setting'`。


---

## 来自原文件 `2026-06-28-screenshot-design.md`

# 屏幕截图功能设计

**日期**: 2026-06-28
**状态**: ✅ 一期 + 1.1 期（多显示器）+ 二期（标注工具栏）已实现，e2e 验证通过
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 0. 概述

为 octopus 新增屏幕截图能力。一期实现基础截图：全局快捷键/托盘菜单触发 → 全屏遮罩 → 鼠标框选 + 8 手柄调整 + 拖拽平移 → Enter 确认 → 自动进剪贴板历史。1.1 期多显示器支持（每屏独立窗口）。二期标注工具栏（矩形/椭圆/直线/箭头/画笔/文字/序号 + 颜色/粗细/大小浮窗 + OCR/保存/确认/取消）。三期滚动截图。

基于 xcap（跨平台截图引擎）+ Tauri 全屏窗口 + React Canvas 选区 UI。

独立 crate `octopus-capx`（目录 `crates/capx/`），封装 xcap 截图 + 裁剪。截图结果作为图片条目进入剪贴板历史，可 OCR / 保存 / 收藏 / 删除。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── capx/               # octopus-capx — 新增，依赖 infra
│   ├── Cargo.toml      # xcap (path 引用本地), image
│   └── src/
│       ├── lib.rs      # pub use
│       └── capture.rs  # 截图核心：截全屏 → 裁剪选区
└── desktop/            # Tauri 命令 + 前端
    ├── src/
    │   ├── screenshot_commands.rs  # start/confirm/cancel 截图命令
    │   └── screenshot_window.rs    # 全屏透明窗口管理
    └── frontend/src/pages/
        └── Screenshot/index.tsx    # 选区 Canvas UI
```

**依赖关系**：`infra ← capx ← desktop`

### 1.2 为什么直接引用 xcap

xcap 是纯粹的截图引擎（跨平台底层 API 封装），只负责"把屏幕拍下来返回图片"。我们需要的所有功能（框选/裁剪/标注/滚动拼接）都不需要改 xcap：

| 功能 | 实现位置 | 改 xcap？ |
|---|---|---|
| 截全屏 | xcap `Monitor::capture_image()` | 否 |
| 框选矩形 | 前端 Canvas + 鼠标事件 | 否 |
| 裁剪选区 | `image` crate 裁剪 | 否 |
| 标注工具栏（二期） | 前端 Canvas 绘制 | 否 |
| 滚动截图（三期） | 多次调 xcap 截图 → 像素匹配拼接 | 否 |

依赖方式：`xcap = { path = "../../xcap" }`（本地路径引用）。

### 1.3 capture.rs 核心接口

```rust
pub struct ScreenCapture {
    pub png_bytes: Vec<u8>,   // 全屏 PNG
    pub width: u32,
    pub height: u32,
}

/// 截取主显示器全屏
pub fn capture_full_screen() -> Result<ScreenCapture>;

/// 从全屏图中裁剪指定矩形区域（物理像素坐标）
pub fn crop_region(full: &ScreenCapture, x: u32, y: u32, w: u32, h: u32) -> Result<Vec<u8>>;
```

## 2. 截图触发流程

```
用户按快捷键 / 点托盘菜单「截图」
         │
         ▼
┌─────────────────────────────────────┐
│  1. capx::capture_full_screen()     │  xcap 截全屏 → PNG bytes
│     返回 (png_bytes, width, height) │
├─────────────────────────────────────┤
│  2. 创建全屏透明窗口（screenshot）   │  无边框、置顶、透明
│     窗口大小 = 屏幕尺寸              │
├─────────────────────────────────────┤
│  3. emit("screenshot://ready", {    │  传 PNG base64 + 尺寸给前端
│       image, width, height })       │
├─────────────────────────────────────┤
│  4. 前端 Canvas 渲染全屏图           │  作为背景，整体加暗色遮罩
├─────────────────────────────────────┤
│  5. 用户鼠标拖拽框选                 │  mousedown → mousemove → mouseup
│     Canvas 实时更新选区              │  选区内亮 + 选区外暗 + 边框
├─────────────────────────────────────┤
│  6. 8 手柄调整 + 拖拽平移            │  角点/边中点 resize + 内部 move
├─────────────────────────────────────┤
│  7. ESC/右键 取消 → 关窗口           │
│     Enter 确认 → invoke 截图确认     │
├─────────────────────────────────────┤
│  8. 后端裁剪选区                     │  从全屏 PNG 裁剪 (x,y,w,h)
│     → capx::crop_region()            │
├─────────────────────────────────────┤
│  9. 写入剪贴板历史                   │  手动编码 WebP BLOB → DB
│     + 写系统剪贴板                    │  + write_image (suppress flag)
├─────────────────────────────────────┤
│  10. 关闭截图窗口                     │
└─────────────────────────────────────┘
```

### 2.1 选区交互状态机

```
idle（等待框选）→ selecting（拖拽中）→ selected（已确定，可调整）
                                           ↓
                                    resize（拖拽手柄/移动选区）
                                           ↓
                                    selected ←────┘
                                           ↓
                                    Enter → 确认截图
```

- `idle`：鼠标点击任意位置 → 进入 `selecting`
- `selecting`：鼠标拖拽实时更新选区 → mouseup 进入 `selected`
- `selected`：8 手柄可见，鼠标按手柄进入 `resize`，按选区内进入 `move`，按选区外重新 `selecting`
- `resize`/`move`：实时更新 → mouseup 回到 `selected`
- 最小选区 10×10，不超出屏幕边界

### 2.2 选区调整手柄

```
拖拽选区 → mouseup 确定初始选区
         │
         ▼
┌───────────────────────────┐
│  选区四角 + 四边中点显示    │
│  8 个拖拽手柄（小方块）     │
├───────────────────────────┤
│  鼠标移到边框 → 双向箭头   │  cursor: ew-resize / ns-resize
│  鼠标移到角点 → 斜向箭头   │  cursor: nwse-resize / nesw-resize
│  鼠标移到选区内 → 可拖动   │  cursor: move（拖动整个选区位置）
├───────────────────────────┤
│  拖拽手柄 → 实时更新选区   │  最小 10×10
│  拖拽选区内部 → 平移选区   │  不超出屏幕边界
├───────────────────────────┤
│  Enter 确认 / ESC 取消     │
└───────────────────────────┘
```

## 3. 前端选区 Canvas 设计

### 3.1 双层 Canvas

- **底层 Canvas**：全屏原图（xcap 截图）全尺寸渲染
- **上层 Canvas**：遮罩 + 选区框 + 8 手柄（`clearRect` 挖出选区）

```
┌──────────────────────────────────────────────────┐
│  ████████████████████████████████████████████████ │  ← 暗遮罩（选区外）
│  ████████████┌─────────────────────┐████████████ │
│  ████████████│                     │████████████ │
│  ████████████│    选区（清晰）      │████████████ │
│  ████████████│              1280×720│████████████ │  ← 尺寸标注
│  ████████████└─────────────────────┘████████████ │
│  ████████████████████████████████████████████████ │
└──────────────────────────────────────────────────┘
```

### 3.2 选区坐标

```typescript
interface Selection {
  x: number; y: number;   // 左上角
  w: number; h: number;   // 宽高
}
```

归一化处理——支持任意方向拖拽（右下→左上时自动 min/max 换算）。

### 3.3 鼠标状态判定（按优先级）

1. 点在手柄上 → `resize`（按手柄方向调整）
2. 点在选区内 → `move`（平移选区）
3. 点在选区外 → 重新框选（清空旧选区，开始新的 `selecting`）

### 3.4 尺寸标注

选区右下角实时显示像素尺寸（如 `1280 × 720`），半透明白底黑字。

### 3.5 全局快捷键（截图窗口内）

- `Enter` → 确认，调 `invoke("confirm_screenshot", { x, y, w, h })`
- `Esc` / 右键 → 取消，调 `invoke("cancel_screenshot")`

### 3.6 Retina/HiDPI 适配

- xcap 截图尺寸 = 物理像素（如 2880×1800）
- 前端 Canvas / 鼠标坐标 = CSS 像素（如 1440×900）
- 前端按 `devicePixelRatio` 自行换算后传物理坐标给后端（后端无需感知缩放）

## 4. 数据流与存储

### 4.1 截图结果处理（手动写入剪贴板历史）

不走 watcher 自动捕获（避免重复截屏 + 截到截图窗口本身）：

1. 后端从全屏 PNG 裁剪选区 → `capx::crop_region()` → PNG bytes
2. `encode_and_hash()` → SHA-256 去重
3. `encode_to_webp()` → 无损 + 缩略图
4. `insert_image_data()` + `insert_clipboard_item()`（source=clipboard, item_type=image）
5. `ClipboardHandle::write_image()`（设置 suppress flag）

### 4.2 不变量

- 截图在剪贴板历史中表现为普通图片条目（可 OCR / 可保存 / 收藏 / 删除）
- `content` = blob_hash（与 watcher 产生的一致）
- `source` = `clipboard`（截图不是 ASR）

### 4.3 截图配置

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
  ('screenshot_shortcut', 'Alt+S', '截图快捷键');
```

AppConfig 新增 `screenshot_shortcut` 字段，设置页快捷键 section 新增「截图」行（ShortcutButton 热重载，与 ASR/剪贴板快捷键一致）。

## 5. Tauri 命令与窗口管理

### 5.1 命令

```rust
/// 启动截图：截全屏 → 创建截图窗口 → emit 图片给前端
#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String>;

/// 确认截图：从全屏图裁剪选区 → 写剪贴板历史 → 关窗口
#[tauri::command]
pub async fn confirm_screenshot(
    x: u32, y: u32, w: u32, h: u32,
    app_handle: tauri::AppHandle,
) -> Result<(), String>;

/// 取消截图：关窗口
#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String>;
```

### 5.2 截图窗口属性

```json
{
  "label": "screenshot_window",
  "title": "",
  "fullscreen": true,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "resizable": false,
  "transparent": true
}
```

### 5.3 start_screenshot 流程

1. `capx::capture_full_screen()` → 截全屏 PNG + 尺寸
2. 创建截图窗口（如已存在则 destroy 重建）
3. 暂存全屏 PNG 到静态变量 `SCREENSHOT_DATA: Mutex<Option<ScreenCapture>>`
4. 窗口 ready 后 emit `screenshot://ready`（PNG base64 + 尺寸）
5. 前端收到后渲染 Canvas

### 5.4 confirm_screenshot 流程

1. 从 `SCREENSHOT_DATA` 取全屏 PNG
2. `capx::crop_region(full, x, y, w, h)` → 选区 PNG
3. `encode_and_hash` → SHA-256 去重
4. `encode_to_webp` → 无损 + 缩略图
5. `insert_image_data` + `insert_clipboard_item`
6. `ClipboardHandle::write_image`（设置 suppress flag）
7. 关闭截图窗口 + 清空 `SCREENSHOT_DATA`

### 5.5 快捷键注册

main.rs setup 从 config 读 `screenshot_shortcut` 注册全局快捷键。`set_config` 中热重载（与 `clipboard_shortcut` 一致：unregister 旧 + register 新）。

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| macOS 屏幕录制权限未授权 | xcap 返回空/黑屏 → toast「请授予屏幕录制权限」 |
| 多显示器 | ✅ 已实现：每个显示器独立窗口（screenshot_window / screenshot_window_N），用 Tauri monitor API 获取逻辑坐标 + 尺寸，鼠标在哪屏截哪屏 |
| 选区太小（< 10×10） | Enter 无效，不确认 |
| 选区超出屏幕边界 | clamp 到屏幕尺寸内 |
| Retina/HiDPI 缩放 | 前端按 `devicePixelRatio` 换算后传物理坐标 |
| 截图窗口被遮挡 | `alwaysOnTop: true` + 创建后 `set_focus()` |
| 连续触发截图 | 检测窗口已存在 → 先 destroy 重建 |
| 应用崩溃 | `SCREENSHOT_DATA` 内存态，重启自动清空 |
| 快捷键与系统冲突 | `check_shortcut` 冲突检测（与 ASR/剪贴板一致） |

**并发安全**：`SCREENSHOT_DATA` 用 `Mutex<Option<ScreenCapture>>`，互斥保护。

**降级**：截图失败不影响 octopus 其他功能。start_screenshot 返回 Err → 快捷键/托盘回调静默忽略 + 记 error 日志。

## 7. 依赖变更

**新增（Rust）**：
- `xcap = { path = "../../xcap" }`（截图引擎，本地路径引用）
- `image = "0.25"`（裁剪/编码，已有）

**新增（前端）**：无额外依赖（React Canvas 原生 API）。

## 8. 实施分期

| 阶段 | 范围 | 依赖 |
|---|---|---|
| **一期** | 基础截图：xcap 截主屏 + Canvas 框选 + 8 手柄调整 + Enter 确认 → 剪贴板历史 | 无 |
| **1.1 期** | 多显示器支持：每个显示器独立窗口，鼠标在哪屏截哪屏 | ✅ 已实现 |
| **二期** | 标注工具栏（矩形/箭头/文字/撤销），选区内 Canvas 绘制 | ✅ 已实现 |

### 二期实现详情

**标注工具**（8 个标注 + 4 个操作，按工具栏顺序）：
1. **矩形** — 拖拽绘制彩色矩形框
2. **椭圆** — 拖拽绘制彩色椭圆轮廓（`ctx.ellipse`）
3. **直线** — 拖拽绘制彩色直线（无箭头）
4. **箭头** — 拖拽绘制彩色箭头（含三角头部）
5. **画笔**（自由曲线）— 跟随鼠标轨迹，追加点序列
6. **文字** — 点击弹 textarea 输入，失焦/点击其他位置确认
7. **序号** — 点击递增（实心彩色圆圈 + 白色加粗数字 1→2→3...）
8. **撤销** — Cmd+Z / 工具栏按钮，删除最后一个标注

**操作按钮**：
- **OCR** — 合成选区 → 入库 → OCR 识别 → 写 search_text + 剪贴板 + osascript 新建文档（`ocr_screenshot` 命令）
- **保存** — 弹系统保存对话框（`save_screenshot_dialog`）
- **确认** — 合成选区 → 入库 → 剪贴板历史（`confirm_screenshot_with_data`）
- **取消** — 关闭所有截图窗口

**工具属性浮窗**（ToolPropsPopover，两行布局）：
- 第一行：粗细/字号/圆圈大小滑轨 + 数值 + **当前色圆形指示器**（20px 圆形，3px 白边 + 双层阴影，和预设色形状区分）
- 分隔线
- 第二行：8 个预设色（方形圆角，选中态 scale 1.1× + 不透明，未选中 0.45）+ 彩虹调色板（conic-gradient 圆形，弹原生 color picker）
- 三种模式：粗细（1-10）/ 字号（10-48）/ 圆圈（16-60）

**标注属性**：每个标注独立记忆 color + lineWidth（或 fontSize / circleSize），已画的不受新设置影响。

**标注交互**：
- **选择工具**（arrow-pointer，工具栏首位）激活时用精确命中检测已有标注（`hitTestAnnotationPrecise`：空心形状检查到线条距离 ≤8px，内部空白不命中）
- **选区手柄优先**：手柄 hitTest 在所有逻辑之前，任何工具状态下可调整选区大小
- 其他标注工具激活时不检测已有标注（优先绘制）
- 选中标注蓝色虚线高亮，可拖动移动（delta 偏移所有坐标 + pen points 数组）
- 悬停标注显示 move 光标（仅 tool=none）
- 选中后按 Delete/Backspace 可删除单个标注
- 标注绘制限制在选区内（Canvas `clip()`）
- 右键仅 `preventDefault`，不执行任何操作（不模拟左键、不取消截图）

**确认合成**（`confirm_screenshot_with_data`）：
- 前端在临时 Canvas 上合成：原图 1:1（`naturalWidth × naturalHeight`）+ 标注
- 标注坐标/线宽/字号按 `scale = naturalWidth / innerWidth` 放大（`drawAnnotationScaled`）
- 裁剪选区后 `toDataURL("image/png")` → base64 传给后端
- 后端跳过裁剪，直接解码 → SHA-256 去重 → WebP BLOB → DB + 剪贴板
- 保证截图全分辨率 + 标注比例正确

**保存到文件**（`save_screenshot_dialog`）：
- 弹系统保存对话框（`tauri_plugin_dialog`），用户选路径 + 文件名
- 不进剪贴板历史

**工具栏图标**：全部使用自定义 SVG（square/oval-vertical/straight-line/arrow-line/sketching/text/sequence-note/restore/ocr-ai/save/copy/close）

**前端合成重构**：`composeAndCrop()` 公共函数（doOcr/doSaveFile/doConfirm 共用），消除重复代码。

**多显示器崩溃修复**：串行创建窗口（150ms 间隔），避免 macOS WKWebView 同时创建崩溃

**多显示器同步显示**：READY_COUNT + TOTAL_WINDOWS barrier——所有窗口前端渲染完后统一 show，避免逐个弹出。3s 超时 fallback 强制显示防死锁。窗口 label 用 session ID（`screenshot_{timestamp}_{i}`）保证唯一，无需 sleep 等待旧窗口销毁。主显示器聚焦通过匹配 `_0` 结尾的 label。

**背景图编码优化**：RGBA → JPEG 85%（比 PNG 快 10×+，4K 从 ~200ms → ~20ms），Base64 IPC 数据量从 ~20MB → ~3MB。

**避免重复解码**：`encode_to_webp_from_image(&DynamicImage)` 直接接收已解码图像引用，跳过第二次 `load_from_memory`。

**Canvas 性能**：Canvas 尺寸仅初始化一次（`canvasInitedRef`），draw 中用 `clearRect` 替代每帧重设 width/height，避免高频 GPU 缓冲区重分配。

**选区外点击忽略**：选区确定后（`mode === "selected"`），选区外左右键均忽略——避免误操作丢失标注。取消仅通过 ESC 或工具栏取消按钮。

**鼠标行为**：
| 位置 | 左键 | 右键 |
|---|---|---|
| 选区内 | 绘制/选中（工具决定） | 仅 preventDefault（无操作） |
| 选区外 | 忽略（已确定选区时） | 仅 preventDefault（无操作） |

**文字标注编辑**：双击已有文字标注进入编辑（保留原颜色/字号，ESC 恢复，不修改全局工具状态）。`drawMultilineText` 支持 `\n` 换行 + 自动折行。

**工具栏位置**：默认选区下方水平居中，下方空间不够时移到上方。工具栏纯白不透明背景，遮挡后面 Canvas 的选区边框/手柄。

| **三期** | 滚动截图（手动滚动 + Canvas-Anchored NCC + Sobel 拼接） | ✅ 实施完成 |

## 9. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| macOS 屏幕录制权限拒绝 | 中 | 无法截图 | 首次弹窗引导 + toast 提示 |
| xcap Linux Wayland 不完善 | 中 | Linux 截图失效 | 检测 Wayland → fallback `xdg-desktop-portal` 截图 |
| Retina 坐标偏移导致裁剪错位 | 高 | 选区内容偏移 | 前端统一按 `devicePixelRatio` 换算 + 测试验证 |
| 全屏透明窗口在部分 WM 闪烁 | 低 | UX 差 | 测试 macOS/Windows/Linux 三端 |
| xcap 本地路径引用在 CI 环境 | 低 | 构建失败 | CI clone 时包含 xcap 仓库或改为 git 依赖 |


---

## 来自原文件 `2026-06-28-settings-model-selection-design.md`

# 设置页「模型选择」Card 设计

> 日期：2026-06-28
> 状态：✅ 实施完成（ASR引擎/LLM模型/OCR模型选择 + 热重载预热 + 前端 select 下拉）
> 关联：[model-mgmt-ui GUI 模型管理页](./)（侧栏「模型管理」Tab，下载/校验）、[ocr 引擎](../../../crates/ocr/src/engine.rs)（`OcrEngine::instance` OnceLock 单例）

## 1. 背景 / 动机

系统设置页（`GeneralPanel.tsx`）当前有 5 个 Card：交互 / 快捷键 / 语音识别 / 语音识别润色 / 剪贴板。其中「模型选择」分散在两个 Card：

- **语音识别模型**（`asr_engine`）嵌在「语音识别」Card（与语言/硬件加速/纠错/简繁/停顿混在一起）。
- **润色模型**（`polish_llm`）嵌在「语音识别润色」Card（与模式/提示词/间隔/停顿阈值混在一起）。
- **OCR 模型**（`ocr_model`）**完全不在设置页**——它走 `ocr/engine.rs::OcrEngine::instance()` 里的 `load_config_key("ocr_model")` 旁路读取，用户无法在 GUI 切换。

更隐蔽的问题：`ocr_model` 在 `app_config` 表**已有 seed**（`db.sql:286`，值 `PP-OCRv6-small`），但 `AppConfig` 结构体（`config.rs`）**漏了该字段**。所以它不参与 `load_app_config_at` / `save_app_config_at` 的统一读写，无法通过 `set_config` 持久化——这是历史遗漏。

用户要求：在「交互」Card 正下方新增独立的「模型选择」Card，把语音识别模型、润色模型、OCR 模型**集中**在一起。

## 2. 设计目标

- **集中**：三类模型选择（ASR / 润色 / OCR）归拢到单一「模型选择」Card，从原所在 Card 移走（不重复）。
- **补漏**：`ocr_model` 纳入 `AppConfig`，走统一的 DB load/save + `set_config` 持久化链路，与 `asr_engine` / `polish_llm` 对齐。
- **OCR 可选**：OCR 下拉选项从 `models` 表 `domain='ocr'` 查（对齐 asr/llm 的 DB 查询模式），当前仅 1 项（`PP-OCRv6-small`），未来加模型不改 UI。
- **生效语义清晰**：每行标注真实生效时机（asr 下次录音 / polish 立即 / ocr 下次启动）。

## 3. 设计

### 3.1 UI：新增「模型选择」Card（`GeneralPanel.tsx`）

Card 顺序（新 Card 紧跟「交互」之后，符合"在交互下面"）：

1. 交互（Mic，不变）
2. **模型选择（新，`Layers` 图标）**
3. 快捷键（Keyboard）
4. 语音识别（Volume2）—— **删「识别引擎」行**
5. 语音识别润色（Sparkles）—— **删「润色模型」行**
6. 剪贴板（ClipboardList）

新「模型选择」Card 三行（复用现有 `Row` + `select` + `selectClass`）：

| 行 label | config key | 选项来源 | effect |
|---|---|---|---|
| 语音识别模型 | `asr_engine` | `asr_engines`（已有） | 下次录音 |
| 润色模型 | `polish_llm` | `llm_models`（已有） | 立即 |
| OCR 模型 | `ocr_model` | `ocr_models`（**新增**） | 下次启动 |

- ASR / 润色行的下拉逻辑**原样搬运**（`asr_engines`/`llm_models` 的 `value`/`onChange` 不变），仅换所属 Card。
- OCR 行：`<select value={cfg.ocr_model}>` 选项来自 `ocr_models`（`m.name` / `m.label`），`onChange` 调 `setVal("ocr_model", e.target.value)`。
- 新 Card 用 `Layers` 图标，区别于侧栏「模型管理」Tab 已用的 `Box`。

### 3.2 `ocr_model` 纳入 AppConfig（补漏，核心）

`ocr_model` 当前是 `load_config_key("ocr_model")` 旁路读，不进 `AppConfig`。补齐使其走统一链路：

- **`config.rs`**：加字段 `pub ocr_model: String`（`#[serde(default = "default_ocr_model")]`，默认 `"PP-OCRv6-small"`）+ `fn default_ocr_model()` + `Default` impl 初始化 + 单测断言 `cfg.ocr_model == "PP-OCRv6-small"`。
  - 字段位置：紧邻 `asr_engine`（同为"模型选择"语义）或字段区末尾均可；放末尾减少对既有字段顺序的扰动。
- **`db.rs::load_app_config_at`**：字符串区分支加 `"ocr_model" => cfg.ocr_model = value,`。
- **`db.rs::save_app_config_at`**：`fields` 数组加 `("ocr_model", cfg.ocr_model.clone())`，长度 `27 → 28`；注释 `27 字段` → `28 字段`。
- **`db.sql`**：`app_config` seed 行（L286 `('ocr_model', 'PP-OCRv6-small', ...)`）**已存在，不动**；新装用户有 seed，老库缺行时 serde default 兜底。

补齐后，`ocr/engine.rs::OcrEngine::instance()` 仍用 `load_config_key("ocr_model")` 读取（行为不变）——本设计**不改 OCR 引擎读取入口**，只补 AppConfig 持久化链路，让设置页能写。

### 3.3 OCR 下拉数据源（新增）

对齐 `list_llm_models` 模式：

- **`db.rs`**：新增 `OcrModelInfo { model_name, description }`（`#[derive(Debug, Clone, serde::Serialize)]`）+ `list_ocr_models_at(conn)`（`SELECT model_name, description FROM models WHERE domain='ocr' AND is_enabled=1`）+ `pub fn list_ocr_models()`（经 `with_db`）。
- **`runtime_config.rs`**：新增 `OcrOption { name, label, current }`（`#[derive(Serialize)]`）+ `build_ocr_options(current, ocrs)` + `pub fn build_ocr_options_public(current, ocrs)`。
  - **不做「不选择模型」首项**（区别于 `build_llm_options`）：OCR 必须有一个模型，空值无意义。列表即 DB 启用的 OCR 模型，`current` 按 `m.model_name == current` 标记。
  - `label`：优先 `description`（如 "PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日"），description 空时回退 `model_name`。
- **`settings_commands.rs`**：`ConfigResponse` 加 `pub ocr_models: Vec<crate::runtime_config::OcrOption>`；`get_config` 加 `let ocrs = octopus_infra::db::list_ocr_models()...; let ocr_models = build_ocr_options_public(&g.ocr_model, ocrs);` 并填入返回。
- **前端 `ConfigResponse`**（`Settings/index.tsx`）：接口加 `ocr_models: { name: string; label: string; current: boolean }[]`。

### 3.4 OCR 生效时机 = 下次启动（有意取舍）

`ocr/engine.rs::OcrEngine::instance()` 用 `OnceLock` 缓存，首次加载后整个进程固定。改 `ocr_model` 写入 DB 后，**当前会话不热替换**——原因：

- OCR 引擎实例化需反序列化 3 个 `.mnn` 文件 + 建 session，成本高。
- OCR 使用频率远低于 ASR（截图识别才触发），不值得为热切换加 `OnceLock` 清空 + 重载逻辑。
- 重启后 `instance()` 重新读 DB，自然生效。

故 OCR 行 effect 标「下次启动」。`set_config` 处理 `ocr_model` 时**只持久化、不热重载**（对比 `asr_shortcut` 等的热重载块，OCR 无对应块）。

### 3.5 `apply_config_value` 加 `ocr_model` 分支

`ocr_model` 是裸 `model_name`（非 3-part spec，OCR 引擎直接拿来当目录名），简单字符串校验即可（照 `asr_shortcut` 字符串分支模板）：

```rust
"ocr_model" => {
    cfg.ocr_model = value.as_str().ok_or("ocr_model 需要字符串")?.to_string();
}
```

**不**调 `build_*_spec` 构造（OCR 无 spec 解析），**不**校验模型是否在 DB（当前仅 1 个，且未启用时报错反而碍事；持久化即可，`instance()` 加载时 `is_model_ready` 兜底）。

## 4. 边界与权衡

| 场景 | 行为 |
|------|------|
| 切 OCR 模型（当前仅 PP-OCRv6-small） | 写 DB 持久化；当前会话 OCR 仍用旧实例（OnceLock）；重启后生效 |
| OCR 模型未下载（`is_model_ready=false`） | 下拉仍显示（DB `is_enabled=1` 即列）；选了重启后 `instance()` bail「OCR 模型未就绪」（既有兜底，不在本设计改） |
| `ocr_models` 为空（DB 无 domain='ocr' 启用行） | 下拉空，`cfg.ocr_model` 保持当前值；不崩（前端 map 空数组） |
| 老库 `app_config` 无 `ocr_model` 行 | `load_app_config_at` 无匹配分支走 serde default `PP-OCRv6-small`；首次 `set_config` 触发 `save_app_config_at` 写入该行 |
| ASR/润色行从原 Card 移走 | 原「语音识别」Card 剩语言/硬件加速/纠错/简繁/停顿；原「语音识别润色」Card 剩模式/提示词/间隔/停顿阈值——职责更清晰 |

## 5. 不改动 / 持久化

- **不改** `ocr/engine.rs::OcrEngine::instance()` 读取入口（仍 `load_config_key("ocr_model")`）、`OcrEngine::recognize`、OCR 单例缓存机制。
- **不改** `asr_engine` / `polish_llm` 的后端逻辑（仅前端换 Card 归属）。
- **不改** 侧栏「模型管理」Tab（`ModelsPanel.tsx`，下载/校验 ASR 模型）——本设计是设置页内 Card 重组，不涉及模型下载。
- **不改** `db.sql`（OCR 的 models seed L101-107 + app_config seed L286 均已存在）。

### 5.1 DB 持久化（隐藏前提，同 polish_global_shortcut）

`load_app_config_at` / `save_app_config_at` 是显式字段列表，每加一个 `AppConfig` 字段必须同步补行（漏则 `set_config` 不写 DB + `get_config` load 不到 → 设置页回退 serde default）。`ocr_model` 必须在两处补（见 §3.2）。

`load_app_config_at` 用 `WHERE category='setting'` 过滤。`db.sql` 的 `ocr_model` seed 行 `category` 走列 DEFAULT（当前 `='setting'`），新装/已迁移库正确。老库若列 DEFAULT 仍是 `'default'`，该行 load 漏读 → 回退 serde default（功能可用，仅显示值非 DB 值）；既有 migration（`UPDATE ... 'default'→'setting'`）已覆盖。

