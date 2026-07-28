# 剪贴板管理

> 独立的剪贴板历史核心库（`octopus-clipboard` crate），仅依赖 `octopus-infra`，基于 `clipboard-rs` 跨平台读写 + 监听。统一存储 text/voice/ocr/image/file 五类条目，FTS5 全文搜索，图片 JPEG q85 BLOB 压缩，自动清理。

源文件：`crates/clipboard/src/`。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `model` | 数据结构：`ItemType`（Text/Image/File）、`Source`（Clipboard/Asr）、`ClipboardItem`（含 `ImageMeta`/`FileMeta`/`AsrMeta`）、`QueryFilter`（6 种过滤 + 分页 + 搜索） |
| `handle` | `ClipboardHandle`：`Mutex<ClipboardContext>` 全局单例（Windows 防锁竞争）+ `AtomicBool` suppress flag + `AtomicBool` recording_enabled gate |
| `watcher` | `ClipboardWatcher`：后台线程跑 `ClipboardWatcherContext::start_watch()`（阻塞），`on_clipboard_change` 回调链 |
| `store` | DB CRUD：通用插入 / ASR 插入 / OCR 插入 / FTS5 JOIN 搜索 / 分页 / 去重 / 引用计数清理 |
| `image` | RGBA → PNG → SHA-256 去重 → JPEG q85 编码 → 缩略图 240×240 |
| `cleanup` | 自动清理：按天数（默认 30）+ 按数量（默认 1000）删除非收藏 + 孤立 blob 回收 + FTS5 索引重建 |

---

## 2. 数据结构

```rust
enum ItemType { Text, Image, File }
enum Source   { Clipboard, Asr }

struct ClipboardItem {
    id: i64,
    item_type: ItemType,
    source: Source,
    content: String,       // voice/ocr/text 全文；image/file 为空串
    ref_data: String,      // image=blob_hash；file=JSON 路径数组
    meta_info: Value,      // JSON，按 item_type 存不同 schema（见 §10）
    segments: Option<String>, // 仅 voice 段 JSON
    is_favorite: bool,
    is_rich: bool,
    has_thumbnail: bool,
    created_at: String,
}

struct QueryFilter {
    item_type: Option<ItemType>,  // 6 种过滤（全部/语音/文本/图片/文件/收藏）
    search: Option<String>,       // >=3 字符 FTS5 MATCH；<3 字符 LIKE
    limit: i64,
    offset: i64,
}
```

`AsrMeta`：`{engine, asr_mode, char_count, polished, polish_model, duration_ms}`（voice 条目）
`ImageMeta`：`{w, h, size}`
`OcrMeta`：`{engine, model, char_count}`
`FileMeta`：`{files: [{size, type}]}`

---

## 3. 监听机制（clipboard-rs 内置）

| 平台 | 机制 | 间隔 |
|------|------|------|
| macOS | 轮询 `NSPasteboard.changeCount` | 500ms |
| Windows | 事件驱动 `AddClipboardFormatListener` | 即时 |
| Linux X11 | `XFixes` 事件驱动 | 即时 |
| Linux Wayland | 两级轮询（MIME 类型 + text 内容） | 500ms |

---

## 4. ClipboardHandle

- **`Mutex<ClipboardContext>` 全局单例**：Windows 上 `ClipboardContext::new()` 与 `empty()` 竞争锁，故全局化避免重复建实例
- **`AtomicBool` suppress flag**：区分 ASR 写入与外部复制。ASR `do_paste` 写剪贴板前置 suppress=true，watcher 的 `on_clipboard_change` 命中 suppress 后直接 return、不执行 `on_change` 闭包（不入库不 emit），paste 完成后置 false
- **`AtomicBool` recording_enabled**：`clipboard_enabled` 运行时镜像。false 时 `on_clipboard_change` 不入库（直接 return）；`set_config` 热重载翻转；`main.rs` setup 启动时按 DB 值 `set_recording_enabled` 一次性同步——`new()` 默认 true，否则重启会复活已关闭的监听

---

## 5. watcher 回调链

`on_clipboard_change` 依次执行：

1. **suppress 检查**：suppress=true → return（ASR 自身写入，跳过）
2. **recording_enabled gate**：recording_enabled=false → return（用户暂停了监听）
3. **类型判断**（优先级 `files > image > text`，非三者则静默跳过避免 `read_text` 失败日志污染）
4. **去重**：文本/文件按 `find_by_text(text, ItemType)` 匹配；图片按 `find_by_content_hash`
5. **存 DB**：`insert_clipboard_item`
6. **通知前端**：`emit("clipboard://changed")`

---

## 6. DB schema v18

### clipboard_history 表

统一存储 text/voice/ocr/image/file——`item_type` 枚举区分。

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | `INTEGER PRIMARY KEY` | voice = 识别开始毫秒时间戳；其他 = 自增 |
| `item_type` | `TEXT` | `text` / `voice` / `ocr` / `image` / `file` |
| `source` | `TEXT` | `clipboard` / `asr` |
| `content` | `TEXT` | voice/ocr/text 全文；image/file 为空串 |
| `ref_data` | `TEXT` | image=blob_hash；file=JSON 路径数组；text/voice/ocr 为空 |
| `meta_info` | `TEXT` | JSON，按 item_type 存不同 schema（见 §10） |
| `segments` | `TEXT` | 仅 voice 段 JSON `[{kind:raw\|polished\|edited, text}]` |
| `is_favorite` | `INTEGER` | 0/1 |
| `is_rich` | `INTEGER` | 0/1 |
| `has_thumbnail` | `INTEGER` | 0/1 |
| `created_at` | `TEXT` | iso 时间戳 |

### clipboard_history_fts 虚表

FTS5 trigram tokenizer，索引 `content` 列。voice/ocr/text 被搜索，image/file content 为空串自动跳过。

### image_data 表

图片 BLOB 存储：

| 列 | 类型 | 说明 |
|---|---|---|
| `hash` | `TEXT PRIMARY KEY` | SHA-256 内容哈希 |
| `blob` | `BLOB` | JPEG q85 编码的图片数据 |
| `thumb` | `BLOB` | 240×240 缩略图 |
| `image_type` | `TEXT` | 编码格式（jpeg/webp，默认 jpeg） |
| `width` / `height` | `INTEGER` | 原图尺寸 |
| `created_at` | `TEXT` | 创建时间 |

`clipboard_history.ref_data` 引用 `image_data.hash`；删除条目时引用计数为 0 才删 image_data 行（`cleanup_unreferenced_images`）。

### 触发器

3 个触发器自动同步 FTS5 索引：`clip_fts_ai`（INSERT）、`clip_fts_ad`（DELETE）、`clip_fts_au`（UPDATE OF content）。

v17 废弃原 `transcriptions` 表（db.sql 不再含此表）。

---

## 7. FTS5 搜索

`query_history` 搜索逻辑：

| 查询长度 | 路径 |
|----------|------|
| ≥3 字符 | FTS5 MATCH phrase（trigram 倒排索引，包成 `"phrase"` 走 phrase query） |
| <3 字符 | LIKE `%text%` 子串 fallback（trigram 无法生成 3-gram） |

- 6 种 item_type 过滤 + 分页（`LIMIT ? OFFSET ?`）
- `ORDER BY created_at DESC, id DESC` 二级排序——消除秒级 `iso_now` 同秒不稳定（同秒内按 id 保证确定顺序）

`rebuild_fts_index`：FTS5 索引重建，仅启动时一次性 populate；删除路径由 db.sql 触发器 `clip_fts_ad` 增量同步，无需周期 rebuild。运行中删除计数器达 10 自动 rebuild。

---

## 8. 图片存储

`crates/clipboard/src/image.rs`——RGBA → PNG → SHA-256 去重 → JPEG q85 编码 → 缩略图。

**编码流程**：
1. 从剪贴板读取 RGBA 像素
2. `encode_and_hash`：PNG 编码 + SHA-256 计算内容哈希
3. 去重：`find_by_content_hash` 命中则复用已有 image_data 行
4. JPEG 编码（按 IMAGE_SAVE_QUALITY 配置链，默认 q85）
5. 缩略图 240×240（Triangle 滤波降采样）

**`encode_to_webp`**（函数名历史遗留，实际按 `IMAGE_SAVE_QUALITY` 配置链编码，默认 JPEG q85）接 `&DynamicImage`，复用调用方已解码的像素（watcher 从 RGBA 构造 DynamicImage；screenshot/migration 复用 `load_from_memory`）。

**编码降级链**（`consts::IMAGE_SAVE_QUALITY` = `"jpeg:85"`，`;` 分割、`:` 解析格式与质量）：
1. 正常尺寸先 lossless WebP
2. 按序尝试首个成功（默认 jpeg:85）
3. 超尺寸（>16383px，VP8 上限）跳过 lossless 直接进降级链
4. 每次编码 `catch_unwind` 兜底防超大图 panic

`ImageMeta.size` 经 `(SELECT length(blob) FROM image_data WHERE hash = blob_hash)` 子查询算（query_history / get_item_by_id / LIKE / FTS5 四处 SELECT 同步），供列表显示存储大小。

---

## 9. 自动清理

`crates/clipboard/src/cleanup.rs`：

- **按天数**（默认 30 天）+ **按数量**（默认 1000 条）删除非收藏记录
- **孤立 blob 回收**：删除条目后引用计数为 0 的 image_data 行
- **FTS5 索引重建**：仅在有删除/回收时 rebuild，避免定时清理无删除时无谓全表重建

**接入定时调用**：
- `main.rs` setup 启动时跑一次（image_migration 迁入旧图片后）
- 后台线程每小时从 DB 重读 `clipboard_max_items` / `clipboard_max_age_days` 跑一次（让设置页「最大保留条数 / 自动清理天数」真正生效；用户运行时改限额 1 小时内自动生效）

## 9.1 软删 / 回收站（v47）

`clipboard_history` 加 `is_deleted` 列（v53，INTEGER 0/1）。删除走两阶段：

- **软删**：`UPDATE ... SET is_deleted = 1`（条目仍在 DB，列表不显示，进「回收站」tab）
- **还原**：`UPDATE ... SET is_deleted = 0`（回收站 tab → 还原按钮）
- **永久删**：`DELETE FROM`（回收站 tab → 永久删除按钮，或 TTL 自动触发）

**图片物理删**：软删文本条目只设 is_deleted=1；但图片条目软删时立即物理删 image_data blob（图片占空间大，软删留 blob 无意义）。

**回收站自动清**（scheduler `trash_purge` 任务）：
- TTL 3 天（`created_at` 超过 3 天的永久删——is_deleted 是 0/1 标志不是时间戳）
- 容量上限 500 条（排除收藏，超限时先永久删回收站最老的）
- 与 `clipboard_cleanup`（§9 按 天数/数量）互补：cleanup 管活跃区，trash_purge 管回收站

**不变量**：收藏条目（`is_favorite=1`）即使软删也不被自动清理/TTL 永久删（用户显式永久删才行）。详见 [spec](../superpowers/specs/archived/2026-07-22-clipboard-soft-delete.md)。

---

## 10. meta_info schema（按 item_type 分）

| item_type | meta_info JSON | 说明 |
|-----------|----------------|------|
| `text` | `{char_count}` | 字符数 |
| `voice` | `{engine, asr_mode, char_count, polished, polish_model, duration_ms}` | ASR 引擎/模式/润色状态/时长 |
| `ocr` | `{engine, model, char_count}` | OCR 引擎/模型（默认 paddle / PP-OCRv5） |
| `image` | `{w, h, size}` | 宽高 + 存储字节数 |
| `file` | `{files: [{size, type}]}` | 文件列表（大小 + MIME 类型） |

序列化时 None 字段跳过不写 null。

---

## 11. ASR 集成

`coordinator.rs::do_paste` 中剪贴板联动：

1. `store::touch_created_at`：录音过程 `insert_transcription_at_id` 已在 clipboard_history 创建 voice 条目（item_type='voice'），paste 时只需 touch `created_at` 顶到列表顶部（不重复创建）
2. **成功后主动 `emit("clipboard://changed")`**：`paste::paste` 写剪贴板设 suppress flag，watcher 的 `on_clipboard_change` 命中后直接 return、不执行含 emit 的 `on_change` 闭包，故 ASR 记录需主动广播前端才能即时渲染
3. 调 `paste::paste`（写系统剪贴板，suppress flag 阻止 watcher 重复记录）

**删除已统一**：transcriptions 表已废弃（v17 DROP），所有 ASR 数据在 clipboard_history（item_type='voice'）；`delete_history` 直接调 `delete_transcriptions`（已写 clipboard_history），删除行数 >0 时主动 `emit("clipboard://changed")` 广播浮窗/设置页双端刷新。

---

## 12. 剪贴板浮窗交互（前端）

> 浮窗容器（`clipboard_window`）见 [desktop-app.md](./desktop-app.md) §5。本节是浮窗内列表项渲染、链接识别、键盘导航、一键清理的纯前端特性。源文件：`crates/desktop/frontend/src/pages/Clipboard/`、`types/clipboard.ts`、`lib/clipboardNav.ts`。

### 12.1 无协议链接识别（`detectUrl`）

`types/clipboard.ts::detectUrl(raw): { isLink, url }`——剪贴板文本条目无协议时也识别为可点击链接，浮窗/设置页两处消费点共用（消除重复内联正则）。

| 路径 | 识别条件 | 补全 |
|------|----------|------|
| 带协议 | `^https?://` | 原样（含 trim） |
| 常用后缀域名 | labels 合法 + 后缀在 `.com/.cn/.com.cn/.net/.org` | `https://` |
| localhost/IPv4 + 端口 | 主机名是 localhost 或合法 IPv4，端口 1–65535 | `http://` |

**不识别**：句中片段（含空白）、纯 IP/localhost（无端口）、括号包裹（label 非法）。后缀自带前导 `.` 做 dot 对齐（`foocom ≠ .com`）。34 单测锁定。

### 12.2 两行布局 + `fileMeta`

历史行从「单行 + 右侧 hover 占位空白」改为两行：第一行铺满内容 + 行尾元数据，第二行时间戳 + 操作按钮。**时间戳固定在内容下方**（在上会视觉归属上一条，实测否决，见 [[clipboard-timestamp-below-content]]）。

- 类型图标提为跨两行垂直居中的「头像」列，兼单击复制入口（copied 时 `scale-125` + 闪绿 + 「已复制」气泡）。
- 操作组：复制 → 打开链接 → 编辑/预览/保存/打开文件 → 删除 → 收藏（复制居首）。
- 行尾元数据三选一 helper：image→`imageMeta`（`W×H·size`）/ file→`fileMeta`（类型或「N个·类型」）/ 其余→`metaParts`（`N字` / `N字·Xs`）。

### 12.3 键盘导航

`lib/clipboardNav.ts` 纯函数 + `index.tsx` window 级 keydown handler，脱离鼠标可用（对齐 Wox/Raycast）。

| 键 | 行为 |
|----|------|
| `↑↓` | 移动选中（边界夹紧不循环）+ 自动滚动到可见 |
| `Enter` | 对选中条目执行粘贴（复用 `paste_clipboard_item`，与双击一致） |
| `Esc` | 有搜索内容清空、已空隐藏浮窗 |
| `←→` | **仅搜索框为空时**循环切 tab（有内容让出给光标移动） |
| `Tab/Shift+Tab` | 无论搜索框是否有内容都循环切 tab |
| `Ctrl+1..7` | 直接跳 tab（写死，不可配置；不用 cmd 防 Accessory 策略下菜单栏拦截），用 `e.code`（物理键位 `Digit[1-7]`）匹配非 `e.key`（macOS Option+数字产生特殊字符如 `¡`） |

- `selectedIndex` 索引驱动（非 `selectedId`）；items 变化（过滤/搜索/刷新）时 useEffect 夹紧越界索引。
- TABS 顺序：all/favorite/asr/text/ocr/image/file（favorite 第 2）。
- **写死 `Ctrl` 非 `Cmd`**：octopus 激活策略为 Accessory，浮窗显示时不切 Regular，前一 app 的菜单栏 key equivalent 会拦截 `Cmd+digit`；`Ctrl` 不产生特殊字符、非标准 menu equivalent、跨平台一致。固定 Ctrl 不再开放配置（原 `clipboard_tab_modifier` 配置项已移除）。
- **闭包陷阱**：window keydown handler 用 `itemsRef`/`selectedIndexRef`/`searchRef`/`filterRef` 存最新值，避免注册时闭包过期。
- `moveIndex`（边界夹紧，null 初态按方向落到首/末）/ `moveTab`（循环 `(cur+delta+len)%len`）抽纯函数单测。

### 12.4 一键清理（`clear_history_by_filter`）

`store::clear_history_by_filter(conn, filter, keep_favorite)` + Tauri 命令 `clear_clipboard_history_by_filter`——浮窗底栏「清理」按钮一键删当前 tab 类别下所有非收藏条目。

- **「查询看到的 = 清理删除的」语义一致**：复用 `build_where`（filter→SQL 单一权威）拼 WHERE + `AND is_favorite = 0`，与 `clear_history` 对称（含 `cleanup_unreferenced_images`）。
- **收藏 tab 自然删 0 条**：`filter="favorite"` + `keep_favorite=true` → `is_favorite = 1 AND is_favorite = 0` 恒假，后端无需特判，前端 `disabled` 按钮。
- **两步 inline 确认**：点 1 次 → `confirming=true`（变红「再点确认」+ 3s 超时回退），再点才执行。filter 切换/卸载清 timer（防 A tab 点了第一步、切 B tab 误清 B）。
- **与搜索框正交**：删整个 tab 类别非收藏，与搜索词无关。

---

## 13. 边缘吸附（dock）

> 拖到屏幕边缘 ≤10px 自动吸附收缩为 8px voice 色细条，hover/点击展开，失焦收缩，拖离边缘恢复。

**窗口物理尺寸不变**（300×600），收缩靠 CSS 隐藏 + `cursor_position()` 轮询穿透（与 result_window 统一）。
- `DOCK_EXPANDED: AtomicBool` Rust 侧状态真相源。
- dock 恢复用**保存坐标**定位显示器（非 `current_monitor()`，窗口刚创建在默认位置返回主屏导致副屏 dock 跑到主屏）。
- 仅 macOS（`#[cfg(target_os="macos")]` gate）。

## 14. hover 预览 overlay

> 预览开关（标题栏 Eye/EyeOff，默认关闭，localStorage 记住）开启时，hover / 键盘 ↑↓ 选中条目后列表右侧弹出 200×200 absolute overlay。

- 文本→等宽可滚动 `text-[11px]`（>500 字截断防卡顿）、图片→缩略图（cancelled 竞态守卫）、文件→路径。
- 智能定位：选中在上半→预览在下方（底边与条目重叠 2px）；下半→上方。clamp 上下界含 `scrollTop`（abs 子元素随内容滚动）。
- 键盘/hover 防冲突：键盘 ↑↓ 时 `keyboardNavRef=true` 屏蔽 mouseEnter 300ms（`onHover` prop 独立于 `onSelect`）。
- 浮窗失焦时隐藏预览（`onFocusChanged`）。
