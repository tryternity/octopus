# 统一编辑器

> `compact_editor_window`——统一内容查看器窗口（多 tab），取代独立的 ImagePreview 窗口和已移除的 Notepad。tab 切换文本（可编辑）/ 图片（嵌入 ImagePreview）/ 语音（只读），与剪贴板历史联动。

源文件：`crates/desktop/src/compact_editor_commands.rs`、`crates/desktop/src/compact_editor_window.rs`、`frontend/src/pages/CompactEditor/`。

---

## 1. 窗口属性

- 原生标题栏、**1100×680 可调 + 记忆**、居中、min 600×360
- **窗口记忆**：`WindowState` 存 `app_config`，`CloseRequested` 存位置/大小到 `app_config`（物理像素÷`scale_factor` 存逻辑像素），开窗读记忆无记忆用默认居中
- 关窗即销毁
- macOS 开窗切 Regular、关窗 `Destroyed` 经 `on_compact_editor_closed` 切回 Accessory（与 settings 对称）

---

## 2. Tab 模型

```typescript
type Tab = {
  key: string;       // `${source}:${itemId}`；temp tab 用 `temp:${ts}_${rand}` 避免冲突
  source: 'clipboard' | 'transcription' | 'temp' | 'file';
  itemId: string;    // clipboard 条目为 UUID；file tab 为 md5(路径) 前 16 hex（同路径去重）；temp tab 保存入库后升级为真 id
  itemType?: 'text' | 'image';
  text?: string;
  imgWidth?: number;
  imgHeight?: number;
  isTemp?: boolean;  // 临时 tab（不写 DB，图文编辑空白入口）；保存后升级为 clipboard tab
  filePath?: string; // file source tab 的磁盘路径（Cmd+S 写回用）
}
```

- 文本 tab 标题 = 文本前 5 字 + `-` + id hex 后 5 位
- 图片 tab 嵌入 `ImagePreview` 组件（≤5，超 5 替换最旧）。**图片懒加载**（2026-07-07）：仅活跃 Tab 挂载 ImagePreview，非活跃显示占位——避免隐藏 Tab 仍并发拉全图+建 `createImageBitmap` 致内存×Tab 数暴涨；切回重新加载（标注/缩放重置可接受）。文本/语音 tab 仅活跃 tab 挂载 `MarkdownPane`（CodeMirror 6），与图片懒加载一致
- 语音 tab 只读（MarkdownPane 预览模式）

工具栏：撤销 / 重做 / 字号 / 查找替换 / 清空 / 保存

---

## 3. 命令

| 命令 | 说明 |
|------|------|
| `open_compact_editor_tab(item_id, source?)` | 单开（转调批量版）；已开则 emit `compact-editor://open-tab` 推送并聚焦、未开建窗 |
| `open_compact_editor_tabs(items)` | 批量开（避免连续单开在「窗口刚 build、React 未 mount」中间态丢 tab）；逐项查 DB 组装后经 `open_tabs_batched` emit-or-pending + 一次建窗 |
| `get_pending_compact_tabs() -> Vec<PendingTabFull>` | 前端 mount take 全部（含 itemType/text/图片尺寸，合并到一次 IPC） |
| `get_clipboard_item_text(item_id)` | 读 content 供文本 tab 载入 |
| `get_clipboard_item_type(item_id) -> 'text'\|'image'\|'file'` | 前端据此渲染 CodeMirror 或 ImagePreview |
| `get_transcription_text(id) -> String` | 读 clipboard_history voice 条目的 content，供语音只读 tab |
| `open_files_in_editor(paths) -> { errors }` | 打开磁盘文件（2026-08-18）：图片入库开图片 tab、文本按 UTF-8 开 file tab，失败逐个进 errors（见 §7） |
| `insert_clipboard_text_item(text) -> i64` | 插入新文本条目（temp tab 保存用）：入库 + 同步系统剪贴板 + emit `clipboard://changed`，返回新 id |
| `close_compact_editor` | 关窗 |

辅助函数（非命令）：`store_pending_temp_tab(text, source)` 写 temp pending；`open_temp_compact_editor(app, text)` 打开 temp tab（窗口存在 emit / 不存在 store+建窗），供托盘「图文编辑」与 action_bar 结果展示共用。

单例：open 时已存在则 show+focus，否则创建。

---

## 4. 编辑保存

**既有文本 tab**（source=clipboard）Ctrl+S / Cmd+↵ 经 `set_clipboard_item_text` 回写 DB + 系统剪贴板：
- 同写 `content` + `search_text`（保 FTS 命中，`clip_fts_au AFTER UPDATE OF search_text` 触发器自动同步 FTS5 索引）
- 同步系统剪贴板
- **成功后 `emit("clipboard://changed")`**（编辑器是独立窗口，剪贴板列表窗口靠此事件感知条目变化并 `fetchItems()` 重新拉取，否则编辑后列表仍显示旧文本）

**临时 tab**（source=temp，isTemp=true——托盘「图文编辑」/ action_bar 结果入口）保存走 insert：
- 非空 → `insert_clipboard_text_item(text)` 入库新剪贴板条目（返回新 id）→ `promoteTempTab` 把 tab 升级为正式 clipboard tab（`key=clipboard:${id}`、source/itemId/isTemp 同步），后续编辑走上文 update 路径
- 空 → 关闭该 tab（不入库）

**清空后保存 = 删除条目**（既有 tab 调 `delete_clipboard_item`：仅剩该 tab 则关窗，否则关该 tab）。

关 tab 不删条目（仅关视图）。

---

## 5. Markdown 编辑器（CodeMirror 6 + markdown-it）

> 2026-07-11 改造：textarea 替换为 CM6 编辑器 + markdown-it 实时预览。组件 `MarkdownPane`（工具栏 + grid 布局 + Splitter 内联）。

**视图模式** `split | editor | preview`，智能默认 = `readOnly ? 'preview' : 'split'`。

**关键约束**：CM6 + Preview **始终挂载**，用 CSS grid + `display:none` 切换可见性（零 mount/unmount）。卸载 CM6 会导致 flexbox 高度塌陷 + 光标丢失。

**markdown-it 配置**：`html:false, linkify:true, typographer:true, breaks:false`。插件 `markdown-it-task-lists`（enabled:false）+ `markdown-it-mark`。Mermaid 代码块输出 `md-mermaid-pending` 占位类（未来 SVG 渲染钩子）。代码块复制按钮通过声明式 `code_block`/`fence` 渲染规则（非命令式 DOM 注入）。

**预览 debounce 150ms**；CM6 `updateListener` 同步触发 `onChange`（无 debounce）。

**大文档防护**（2026-08-18，z_perf baseline 见 `largeDocPerf.test.ts`）：
- **预览截断**：source > 256KB 时 `sliceForPreview`（`previewTruncate.ts` 纯函数）按行边界截到 256KB 再渲染，顶部提示条显示「前 X KB / 共 Y KB」——修复打开 MB 级转 Markdown 产物时整篇 markdown-it 渲染 + 数 MB `innerHTML` 的 WKWebView 布局冻结（实测 2MB：JS 渲染 212ms→22ms，HTML 4.8MB→~600KB；DOM 布局成本随之有界）。截断可能切在 fence 内部——markdown-it 把未闭合 fence 渲染为代码块到截断末尾，无结构破坏。
- **每键 O(N) 消除**：`CodeMirrorEditor` 的 value 回流先比对 `lastEmittedRef`（自己 emit 的回声）O(1) 跳过，仅外部真实变更才做全量 diff 替换——大文档打字不再每键 `doc.toString()` 全量序列化。
- CM6 建态实测 1MB 仅 ~14ms（viewport 渲染 + Lezer 增量解析），无需大文档语言降级。

**滚动同步**（`useSyncScroll`）：双向比例同步，rAF 节流 + echo count 防回环。

**i18n 基础设施**（`lib/i18n.ts`，~60 行，无 i18next）：`initI18n()` 读 `ui_language` config → `setLocale()` 通知 `useT()` 订阅者 → `${name}` 插值。详见 [architecture.md](../architecture.md) i18n 全面覆盖段。

**三层只读保护**：(1) `EditorState.readOnly`，(2) `disableSave = isTemp || readOnly` 隐藏 Clear/Save，(3) `doSave` 4 守卫。

---

## 6. 入口

| 入口 | source | 行为 |
|------|--------|------|
| 剪贴板文本「编辑」 | `clipboard` | 可编辑文本 tab |
| 剪贴板图片「预览」 | `clipboard` | 图片 tab（Maximize2 图标） |
| OCR 识别后 | `clipboard` | 统一 `insert_ocr_clipboard_item` → `openCompactEditorTab` |
| 截图 OCR | `clipboard` | 图片 tab + 文本双 tab |
| 语音识别记录「查看」 | `transcription` | 只读 tab |
| 托盘菜单「图文编辑」 | `temp` | 空白 temp tab（可编辑，保存入库为新剪贴板条目） |
| action_bar 翻译/润色等结果 | `temp` | temp tab 展示结果（`open_temp_compact_editor`） |

语音结果窗**不用**独立编辑器——改为原地尺寸双模式（见 [result-window.md](./result-window.md)）。

上面入口之外，CompactEditor 自身可直接**打开磁盘上已存在的文件**（见下节）。

---

## 7. 打开已存在文件（2026-08-18）

> spec：[2026-08-18-compact-editor-open-files-design](../superpowers/specs/2026-08-18-compact-editor-open-files-design.md)。双入口收敛到同一命令 `open_files_in_editor(paths)`，多选/多文件逐个开 tab。

### 双入口

| 入口 | 实现 |
|------|------|
| 工具栏「打开」按钮 | tab 栏**最前**（所有 tab 之前，`flex-shrink-0` 不随横向滚动移位）`FolderOpen` 图标 → plugin-dialog `open({ multiple: true })`，filter 提示文本+图片扩展名（`openFilesUtils.ts::TEXT_IMAGE_EXTS`——**仅选择器提示用**，后端才是真相源：拖拽不限扩展名）。tab 栏因此**常驻**（0 tab 也能打开文件） |
| 菜单栏 File「打开文件…」（⌘O） | macOS 系统菜单（`ui/app_menu.rs`，2026-08-18）→ Rust 侧 plugin-dialog 同款选择器 → 复用 `collect_open_tabs` → `open_tabs_batched` 同一后端管线；失败经 `open-files://errors` emit 给 CompactEditor toast（编辑器未开仅日志）。系统菜单全量 i18n 并随 `locale-changed` 重建 |
| 拖文件入窗口 | `getCurrentWebview().onDragDropEvent`（WKWebView 下 HTML5 DnD 不可靠，Terminal 同模式）；listener 回调经 ref 稳定化 + `useEffect([])` 只挂一次（跨窗口 listener 踩坑规范）；drag-over 期间容器加 `ring` 高亮 |

### 类型分流（后端 `collect_open_tabs`，spawn_blocking 防卡 runtime）

- **图片**（png/jpg/jpeg/gif/webp/bmp/tiff/tif，大小写不敏感、容忍前导点）→ **入库复用全能力**：镜像剪贴板 watcher 的 ingest 组合——读 bytes → `image::load_from_memory` → `hash_rgba` → **历史级去重**（`find_by_content_hash` 命中同图则 `touch_created_at` 复用已有行 id，**不新增历史条目**；未命中才 `insert_image_data` + `insert_clipboard_item(type=image)`）→ imageId + 宽高 → 图片 tab（`source="clipboard"`，ImagePreview 零改造，OCR/二维码/复制/缩放全可用）
- **其余**（含 svg——按文本读可编辑）→ `fs::read_to_string` 按 UTF-8 读 → file tab（itemId = md5(路径) 前 16 hex，同路径重复打开去重聚焦；Cmd+S 写回磁盘）

### 约束与错误反馈

- **图片 tab 上限** `MAX_IMAGE_TABS = 5`（`mergePendingTabs.ts` 导出，index.tsx 复用）——emit 路径经 loadAndAddTab 逐事件强制、**pending 批量路径由 `mergePendingTabs` 合并时强制**（超 5 挤掉最旧图片 tab，文本不受影响；P2 2026-08-18 补齐——此前关窗状态拖 N 张图可绕过上限）
- **文本 50MB 帽**（P2 2026-08-18，watcher.rs:219 同款）：`collect_open_tabs` 读文本前按 metadata 拦截——超大文本全量进 CM6 会秒级建态 + 双倍内存（预览截断只护 preview 不护 editor）
- 失败**不中断批次**，逐个收集进返回值 `errors`（`<文件名>（<原因>）`）；前端非空时聚合 warning toast（不自动消失——需看清失败清单）。原因：文件夹 / 文件不存在 / 非 UTF-8 / 图片解码失败 / 图片过大（~40MB）/ 文本过大（50MB）。空 paths / 全部失败命令本身不 Err
- **错误文案全 i18n**（P1 2026-08-18）：后端原因串经 `ui::i18n::t("editor.openErr*")`（与前端共用 locales yaml），dialog filter 名（`editor.openFilesFilter`）与 errors 分隔符（`editor.errorsSep`，zh `、` / en `; `）同源——en 用户不再见中文
- 大文本自动受益**预览截断防护**（>256KB 行边界截断，见 §5 大文档防护），无需专门处理

### 批量送出（`open_tabs_batched`）

从 `open_compact_editor_tabs` 泛化的共用出口：窗口在且 React 已 mount（pending 队列空）→ 逐个 emit + show/focus；在但未 mount → 全部 push `PENDING_TABS`（emit 会丢）；不在 → 先 `take_pending_tabs()` 清 stale 残留再 push + 一次建窗（防幽灵 tab）。

---

## 8. ImagePreview 组件

`frontend/src/pages/ImagePreview/index.tsx`（**组件，非路由**）

- props `imageId: number`（去掉 `get_pending_image` / `listen("image-preview://load")`，由父 tab 控制 imageId）
- 保留 `listen("ocr-screenshot://result")` 接收截图 OCR blocks

**性能优化**（2026-07-03）：
1. **canvas 视口固定 + 可见区切片重绘**——canvas `position:sticky` 钉 scrollContainer 视口，物理尺寸 = 视口×dpr（永不超 Chromium 32767 单边硬限，长图不再崩）；drawBg 滚动/缩放时只 drawImage 图片露出视口的 src 切片到视口坐标（不全量重绘）。几何换算抽 `viewportMath.ts` 纯函数（17 单测）。GUI 核心已验证（超大图不崩 + 缩放正常 2026-07-07）；其余 DOM/sticky 对齐项见 code-review 2026-07-07 问题4 清单可选补充。bitmap 预缩放超 GPU 纹理上限时静默 fallback 原图（渐进降级）
2. **底图 canvas + SVG overlay**——标注用 SVG 元素（标注变化零 canvas 操作）
3. **zoom 走 `createImageBitmap` 异步预缩放**（`zoomVersionRef` 防过时帧）
4. **先 thumb 再 full 渐进加载**（`cancelled` 防竞态 + ResizeObserver 自动重算）
5. **thumb→full 期间 `loadingFullRef` 门控禁止标注**

标注核心 `frontend/src/lib/annotation.ts` + `AnnotationSvg.tsx`（SVG overlay）。

---

## 9. 已移除

- ~~`image_preview_commands.rs` + `image_preview_window.rs`~~（统一查看器 Task 5）——独立图片预览窗口废弃，功能合并入 CompactEditor 图片 tab
- ~~`pages/Notepad/`~~（随 `octopus-notepad` crate 一并删除）
- ~~`image_preview_window`~~ 窗口、capabilities ACL + activation `REGULAR_WINDOWS` + main.rs 命令注册、前端 App.tsx 路由
