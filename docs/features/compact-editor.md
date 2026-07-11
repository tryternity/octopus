# 统一编辑器

> `compact_editor_window`——统一内容查看器窗口（多 tab），取代独立的 ImagePreview 窗口和已移除的 Notepad。tab 切换文本（可编辑）/ 图片（嵌入 ImagePreview）/ 语音（只读），与剪贴板历史联动。

源文件：`crates/desktop/src/compact_editor_commands.rs`、`crates/desktop/src/compact_editor_window.rs`、`frontend/src/pages/CompactEditor/`。

---

## 1. 窗口属性

- 原生标题栏、**880×620 可调 + 记忆**、居中、min 400×320
- **窗口记忆**：`WindowState` 存 `app_config`，`CloseRequested` 存位置/大小到 `app_config`（物理像素÷`scale_factor` 存逻辑像素），开窗读记忆无记忆用默认居中
- 关窗即销毁
- macOS 开窗切 Regular、关窗 `Destroyed` 经 `on_compact_editor_closed` 切回 Accessory（与 settings 对称）

---

## 2. Tab 模型

```typescript
type Tab = {
  key: string;       // `${source}:${itemId}`；temp tab 用 `temp:${ts}_${rand}` 避免冲突
  source: 'clipboard' | 'transcription' | 'temp';
  itemId: number;    // temp tab 为 0，保存入库后升级为真 id
  itemType?: 'text' | 'image';
  text?: string;
  imgWidth?: number;
  imgHeight?: number;
  isTemp?: boolean;  // 临时 tab（不写 DB，图文编辑空白入口）；保存后升级为 clipboard tab
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
| `open_compact_editor_tabs(items)` | 批量开（避免连续单开在「窗口刚 build、React 未 mount」中间态丢 tab）；每项 push PENDING_TABS + 一次 create/emit |
| `get_pending_compact_tabs() -> Vec<PendingTabFull>` | 前端 mount take 全部（含 itemType/text/图片尺寸，合并到一次 IPC） |
| `get_clipboard_item_text(item_id)` | 读 content 供文本 tab 载入 |
| `get_clipboard_item_type(item_id) -> 'text'\|'image'\|'file'` | 前端据此渲染 CodeMirror 或 ImagePreview |
| `get_transcription_text(id) -> String` | 读 clipboard_history voice 条目的 content，供语音只读 tab |
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

## 5. undo/redo

Markdown 改造（2026-07-11）后文本 tab 用 CodeMirror 6，undo/redo 走 CM6 `history()` 扩展（替代旧 textarea 时代的 `document.execCommand` 方案——彼时受控 textarea 每次 value 同步清空 WebKit 原生 undo 栈致 Cmd+Z 失灵）。

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

---

## 7. ImagePreview 组件

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

## 8. 已移除

- ~~`image_preview_commands.rs` + `image_preview_window.rs`~~（统一查看器 Task 5）——独立图片预览窗口废弃，功能合并入 CompactEditor 图片 tab
- ~~`pages/Notepad/`~~（随 `octopus-notepad` crate 一并删除）
- ~~`image_preview_window`~~ 窗口、capabilities ACL + activation `REGULAR_WINDOWS` + main.rs 命令注册、前端 App.tsx 路由
