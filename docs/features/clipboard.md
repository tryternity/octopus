# 剪贴板管理

> 独立的剪贴板历史核心库（`octopus-clipboard` crate），仅依赖 `octopus-infra`，基于 `clipboard-rs` 跨平台读写 + 监听。统一存储 text/voice/ocr/image/file 五类条目，FTS5 全文搜索，图片原图文件系统存储 + 缩略图入 DB，自动清理 + 收藏跨设备同步。

源文件：`crates/clipboard/src/`。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `model` | 数据结构：`ItemType`（Text/Voice/Ocr/Image/File 5 变体）、`ClipboardItem`（含 `ImageMeta`/`FileMeta`/`AsrMeta`；无 source 字段）、`QueryFilter`（9 种 filter 值 + 分页 + 搜索） |
| `handle` | `ClipboardHandle`：`Mutex<ClipboardContext>` 全局单例（Windows 防锁竞争）+ `AtomicBool` suppress flag + `AtomicBool` recording_enabled gate |
| `watcher` | `ClipboardWatcher`：后台线程跑 `ClipboardWatcherContext::start_watch()`（阻塞），`on_clipboard_change` 回调链 |
| `store` | DB CRUD：通用插入 / ASR 插入 / OCR 插入 / FTS5 JOIN 搜索 / 分页 / 去重 / 引用计数清理 / voice 软删分流 |
| `favorite` | **2026-08-05 新增**：收藏业务逻辑——维护 `clipboard_favorites` 表三态（active / tombstone epoch / 无记录），`toggle_favorite` 幂等切换（2026-08-14 事务化：单连接 `unchecked_transaction`）；跨设备 sync 详见 architecture.md §octopus-sync |
| `image` | RGBA → MD5 去重 → JPEG q100 原图存文件系统 + 缩略图 240×240 JPEG q5 入 DB |
| `cleanup` | 自动清理：按天数（默认 30）+ 按数量（默认 1000）删除非收藏 + 孤立 blob 回收 + FTS5 索引重建 |

---

## 2. 数据结构

```rust
enum ItemType { Text, Voice, Ocr, Image, File }

struct ClipboardItem {
    id: String,             // UUID v4（v59 起，作跨设备 sync 锚点；原 i64 毫秒戳已废）
    item_type: ItemType,
    content: String,        // voice/ocr/text 全文；image/file 为空串
    ref_data: String,       // image=文件 hash；file=JSON 路径数组
    meta_info: Value,       // JSON，按 item_type 存不同 schema（见 §10）
    segments: Option<String>, // 仅 voice 段 JSON
    is_deleted: i64,        // 软删标志（仅 voice 用，见 §9.1）
    is_favorite: bool,
    is_rich: bool,
    has_thumbnail: bool,
    created_at: String,
}

struct QueryFilter {
    filter: String,         // 9 种：all / asr / ocr / text / image / file / favorite / unfavorite / trash
    search: Option<String>, // >=3 字符 FTS5 MATCH；<3 字符 LIKE
    page: i64,
    size: i64,
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
3. **concealed hint 检测**（2026-08-05，跨平台）：粘贴板含密码管理器 concealed 标记（macOS `org.nspasteboard.ConcealedType` / Windows `ExcludeClipboardContentFromMonitorProcessing` / Linux `x-kde-passwordManagerHint`）→ 静默 return，防密码明文入库 / FTS 索引 / 跨设备 sync。详见 [spec](../superpowers/specs/archived/2026-08-05-macos-concealed-type-skip.md)
4. **类型判断**（优先级 `files > image > text`，非三者则静默跳过避免 `read_text` 失败日志污染）
5. **enqueue 信号到 `desktop/clipboard_queue.rs`**（2026-07-21 P0-5）——watcher 线程只发信号，去重 / 存 DB / emit 由 worker 线程串行处理（原直接在 watcher 线程同步处理会阻塞 NSPasteboard 下一次通知 → 连续复制延迟入库）

worker 线程：**去重**（文本/文件按 `find_by_text` 匹配；图片按 `find_by_content_hash`）→ **存 DB**（`insert_clipboard_item`）→ **通知前端**（`emit("clipboard://changed")`）。

---

## 6. DB schema v60

### clipboard_history 表

统一存储 text/voice/ocr/image/file——`item_type` 枚举区分。

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | `TEXT PRIMARY KEY` | **UUID v4**（v59 改，原 INTEGER 毫秒戳——作跨设备 sync 锚点） |
| `item_type` | `TEXT` | `text` / `voice` / `ocr` / `image` / `file` |
| `content` | `TEXT` | voice/ocr/text 全文；image/file 为空串 |
| `ref_data` | `TEXT` | image=文件 hash；file=JSON 路径数组；text/voice/ocr 为空 |
| `meta_info` | `TEXT` | JSON，按 item_type 存不同 schema（见 §10） |
| `segments` | `TEXT` | 仅 voice 段 JSON `[{kind:raw\|polished\|edited, text}]` |
| `is_deleted` | `INTEGER` | 软删标志（仅 voice 用，见 §9.1；v53） |
| `is_favorite` | `INTEGER` | 0/1 |
| `is_rich` | `INTEGER` | 0/1 |
| `has_thumbnail` | `INTEGER` | 0/1 |
| `created_at` | `TEXT` | iso 时间戳 |

### clipboard_favorites 表（v59 新增）

收藏的跨设备同步锚点（4 字段极简）：`history_id`（PK = clipboard_history.id，一对一，**无 FK**——物理删 history 后 favorite tombstone 须存活到 sync 传播）+ `is_deleted`（0=active / >0=epoch 秒 tombstone）+ `updated_at` + `sync_md5`。超期 tombstone 由 scheduler 每日 GC（30 天，`CLIPBOARD_TOMBSTONE_RETENTION_SECS`）。加密存储于 `.sync/clipboard/`（AES-256-GCM），经 `SyncEntity` trait 的 `merge_three_way` 与 vault/热词统一 merge。

### clipboard_history_fts 虚表

FTS5 trigram tokenizer，索引 `content` 列。voice/ocr/text 被搜索，image/file content 为空串自动跳过。

### image_data 表

图片缩略图存储（原图存文件系统，2026-07-30 改造）：

| 列 | 类型 | 说明 |
|---|---|---|
| `hash` | `TEXT PRIMARY KEY` | MD5(RGBA 像素)，去重键 + 文件名 |
| `thumb` | `BLOB` | 240×240 缩略图（JPEG q5，几 KB） |
| `width` / `height` | `INTEGER` | 原图尺寸 |
| `created_at` | `TEXT` | 创建时间 |

原图存文件系统 `~/Documents/octopus/screens/<hash>.jpg`（可配 `screen_output_dir`）。`clipboard_history.ref_data` = hash；删除条目时引用计数为 0 才删文件 + DB 行（`cleanup_unreferenced_images`）。

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

- 9 种 filter 值过滤 + 分页（`LIMIT ? OFFSET ?`）
- `ORDER BY created_at DESC, id DESC` 二级排序——消除秒级 `iso_now` 同秒不稳定（同秒内按 id 保证确定顺序）

FTS5 索引一致性由 db.sql 触发器（`clip_fts_ai`/`clip_fts_ad`/`clip_fts_au`）事务内增量同步，无需周期 rebuild（原 `rebuild_fts_index` 已于 2026-07-29 删除——死代码）。

---

## 8. 图片存储

`crates/clipboard/src/image.rs`——RGBA → MD5 去重 → JPEG q100 原图存文件系统 + 缩略图 JPEG q5 存 DB。

**存储架构**（2026-07-30 从 DB BLOB 改文件系统）：
- **原图**：`~/Documents/octopus/screens/<hash>.jpg`（JPEG q100，可配 `screen_output_dir`）
- **缩略图**：DB `image_data.thumb`（240×240 JPEG q5，几 KB，列表加载快）
- **hash**：MD5(RGBA 像素) 或 MD5(PNG bytes)，作文件名天然去重

**编码流程**：
1. 从剪贴板读取 RGBA 像素
2. `hash_rgba`：MD5 计算内容哈希（去重用，2026-07-29 SHA-256→MD5）
3. 去重：`find_by_content_hash` 命中则复用已有文件 + DB 行
4. JPEG 编码（按 IMAGE_SAVE_QUALITY 配置链，默认 q100）
5. 缩略图 240×240（nearest-neighbor resize + q5 编码）
6. 原图 `save_image_to_file` + 缩略图 `insert_image_data`（DB）

`ImageMeta.size` 经文件大小 `fs::metadata` 算，供列表显示存储大小。

**文件丢失处理**（2026-07-30）：原图文件可能被用户删除——`check_image_file_exists(id)` 检查存在性；复制/粘贴失败时前端设 `fileMissing=true`（缩略图变感叹号 + 红色"原图丢失"气泡 2s）；不自动删条目（保留 DB 记录）。

---

## 9. 自动清理

`crates/clipboard/src/cleanup.rs`：

- **按天数**（默认 30 天）+ **按数量**（默认 1000 条）删除非收藏记录
- **孤立图片回收**：删除条目后引用计数为 0 的图片文件 + image_data 行（`cleanup_unreferenced_images`：删文件 `delete_image_file` + DELETE DB 行）
- **FTS5 索引重建**：仅在有删除/回收时 rebuild，避免定时清理无删除时无谓全表重建

**接入定时调用**（scheduler 任务 `clipboard_cleanup`，每 10 分钟）：
- 启动时跑一次
- 之后每 10 分钟从 DB 重读 `clipboard_max_items` / `clipboard_max_age_days` 跑一次（用户运行时改限额 10 分钟内自动生效）

另有 scheduler 任务 `clipboard_favorite_gc`（每日）：硬删超期（>30 天）favorite tombstone + export 重建 `.sync`。

## 9.1 软删（voice 内部机制，用户不可见）（2026-07-29 重构）

`clipboard_history` 加 `is_deleted` 列（v53，INTEGER 0/1）。**回收站概念不暴露给用户**——无 trash tab、无还原命令、无清空回收站。`is_deleted` 仅作为 voice 的内部软删标记。

**删除分流（2026-07-29 策略反转）**：
- **voice**：软删（`UPDATE is_deleted = 1`），数据保留在 DB 但列表不显示。软删内容主要用于**热词挖掘**（INV-C1：`list_recent_text` 不过滤 `is_deleted`，软删内容仍是热词来源）及后续优化语音识别准确性。
- **text/ocr/image/file**：物理 DELETE（image 另做 blob 引用计数清理）。

**voice 软删 500 条上限（INV-VT，2026-08-02 从 100 提升）**：voice 软删后实时保证 `is_deleted=1` 的 voice ≤ 500 条（`VOICE_TRASH_MAX`）。任何入口（`delete_item` / `delete_items` / `clear_history` / `clear_history_by_filter`）软删 voice 后，立即把最老的（`created_at ASC`）voice 物理删到恰好 500 条（`enforce_voice_trash_limit`）。

**短 voice 物理删**（2026-08-02）：content < 5 字符的 voice 直接物理删不进软删（对 bigram 语料无价值），`is_voice_worth_keeping` 判别；`clear_voice_aware` 分流——长语音软删带过滤 / 短语音物理删带过滤（2026-08-14 补齐 filter：原短语音分支无 filter 致跨 tab 清空误删全库短 voice）。

**设置页删除联动**（2026-08-14）：设置页「删除语音记录」走 `clipboard::store::delete_items` voice-aware 分流（软删保语料）；三个 voice 列表查询统一加 `AND is_deleted = 0` 过滤软删行。

**不变量**：收藏条目（`is_favorite=1`）任何删除入口都跳过。详见 [spec](../superpowers/specs/archived/2026-07-29-clipboard-softdelete-voice-only.md)。

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

**删除已统一**：transcriptions 表已废弃（v17 DROP），所有 ASR 数据在 clipboard_history（item_type='voice'）；`delete_history` 走 `clipboard::store::delete_items` voice-aware 分流（2026-08-14——长语音软删保 bigram 语料 / 短语音物理删），删除行数 >0 时主动 `emit("clipboard://changed")` 广播浮窗/设置页双端刷新。

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
- TABS 顺序：all/favorite/asr/text/ocr/image/file/**queue**（8 个，favorite 第 2，queue 第 8——数字快捷键 Ctrl+1..7 仅覆盖前 7 个，queue tab 无数字键）。tab 数 >6 时只显图标（`COMPACT_THRESHOLD` 动态阈值，title 出 tooltip）。
- **写死 `Ctrl` 非 `Cmd`**：octopus 激活策略为 Accessory，浮窗显示时不切 Regular，前一 app 的菜单栏 key equivalent 会拦截 `Cmd+digit`；`Ctrl` 不产生特殊字符、非标准 menu equivalent、跨平台一致。固定 Ctrl 不再开放配置（原 `clipboard_tab_modifier` 配置项已移除）。
- **闭包陷阱**：window keydown handler 用 `itemsRef`/`selectedIndexRef`/`searchRef`/`filterRef` 存最新值，避免注册时闭包过期。
- `moveIndex`（边界夹紧，null 初态按方向落到首/末）/ `moveTab`（循环 `(cur+delta+len)%len`）抽纯函数单测。

### 12.4 一键清理（`clear_history_by_filter`）

`store::clear_history_by_filter(conn, filter, keep_favorite)` + Tauri 命令 `clear_clipboard_history_by_filter`——浮窗底栏「清理」按钮一键删当前 tab 类别下所有非收藏条目。

- **「查询看到的 = 清理删除的」语义一致**：复用 `build_where`（filter→SQL 单一权威）拼 WHERE + `AND is_favorite = 0`，与 `clear_history` 对称（含 `cleanup_unreferenced_images`）。
- **收藏 tab 自然删 0 条**：`filter="favorite"` + `keep_favorite=true` → `is_favorite = 1 AND is_favorite = 0` 恒假，后端无需特判，前端 `disabled` 按钮。
- **两步 inline 确认**：点 1 次 → `confirming=true`（变红「再点确认」+ 3s 超时回退），再点才执行。filter 切换/卸载清 timer（防 A tab 点了第一步、切 B tab 误清 B）。
- **与搜索框正交**：删整个 tab 类别非收藏，与搜索词无关。

### 12.5 粘贴队列（Paste Stack，2026-08-05）

批量粘贴场景的 FIFO 队列：

- **入栈**：浮窗 `Cmd+点击` 多选条目（绿色高亮 + 序号 badge ①②③）→ 底部「入栈 N 条」→ `push_to_paste_stack(ids)`
- **出栈**：切到目标应用按全局热键 `paste_stack_shortcut`（默认 `⌘⇧V`）逐条弹出粘贴（`pop_and_paste`：pop front → 写剪贴板 + suppress flag → restore_focus → simulate_paste → emit `paste-stack://updated` 更新计数）
- **队列 tab**：渲染栈内容，序号 = 出栈顺序（index 0 = 下一个弹出）；拖拽排序用 `@dnd-kit/sortable`（WKWebView HTML5 DnD 不可靠）；单条删除 / 清空全部
- 栈不持久化（内存 `Mutex<VecDeque>`，重启清空）
- **命令**：`push_to_paste_stack` / `pop_and_paste` / `paste_stack_status` / `peek_paste_stack` / `remove_from_paste_stack` / `move_paste_stack_item` / `clear_paste_stack`

---

## 13. 边缘吸附（dock）

> 拖到屏幕边缘 ≤10px 自动吸附收缩为 8px voice 色细条，hover/点击展开，失焦收缩，拖离边缘恢复。

**窗口物理尺寸不变**（300×600），收缩靠 CSS 隐藏 + `cursor_position()` 轮询穿透（与 result_window 统一）。
- `DOCK_EXPANDED: AtomicBool` Rust 侧状态真相源。
- dock 恢复用**保存坐标**定位显示器（非 `current_monitor()`，窗口刚创建在默认位置返回主屏导致副屏 dock 跑到主屏）。
- 仅 macOS（`#[cfg(target_os="macos")]` gate）。

## 14. hover 预览 overlay

> 预览开关（标题栏 Eye/EyeOff，默认开启，localStorage 记住）开启时，hover / 键盘 ↑↓ 选中条目后列表右侧弹出 200×200 absolute overlay。

- 文本→等宽可滚动 `text-[11px]`（>500 字截断防卡顿）、图片→缩略图（cancelled 竞态守卫）、文件→路径。
- 智能定位：选中在上半→预览在下方（底边与条目重叠 2px）；下半→上方。clamp 上下界含 `scrollTop`（abs 子元素随内容滚动）。
- 键盘/hover 防冲突：键盘 ↑↓ 时 `keyboardNavRef=true` 屏蔽 mouseEnter 300ms（`onHover` prop 独立于 `onSelect`）。
- 浮窗失焦时隐藏预览（`onFocusChanged`）。
