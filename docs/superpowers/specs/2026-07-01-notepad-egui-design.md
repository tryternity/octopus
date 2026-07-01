# 记事本 egui 迁移设计

> 日期：2026-07-01
> 状态：**已实现**（见 `plans/2026-07-01-notepad-egui.md`；e2e 待 Task 12 用户验证）。
> 关联：`docs/superpowers/specs/2026-06-30-notepad-design.md`（原 webview 记事本）、`docs/superpowers/specs/2026-07-01-image-preview-design.md`（窗口/PENDING/ACL 模板）。
> 分支：`worktree-feature-notepad`。**功能完整完成前不往 main 同步。**

## 1. 背景与目标

桌面端所有窗口（main / notepad / clipboard / compact_editor / image_preview / settings / result / screenshot 共 8 类）都是独立 webview，单个 macOS WKWebView 约 80–150MB。叠加 ASR/OCR/LLM 模型常驻，**多开时内存劣势大**。

记事本是**长开**窗口（用户可能一直开着收集），内存影响最大。本期把记事本从 webview 迁到 **egui 原生进程**（immediate-mode，进程基线约 30–50MB），验证降内存路径；**走通后再迁剪贴板浮窗 / compact editor / 语音识别框**（同一 egui 进程加 view，不开新进程，摊薄基线）。

**记事本定位明确**：轻量编辑框（应用内操作和预览），**不是全功能 WYSIWYG**（富文本创作找专业工具）。因此 egui 的富文本短板对本需求不成立——用 markdown 源码 + 实时预览（`egui_commonmark`）即可，参考 Ferrite / md-echo 等成熟形态。

**非目标**：不动主干（Tauri 主进程的模型加载、剪贴板监听、截图、OCR/ASR、托盘、快捷键不变）；不追求富文本创作能力。

## 2. 范围

**做（第一阶段）：**
- 新二进制 crate `octopus-egui`（eframe）：单进程 + view 路由，第一阶段只实现 `NotepadView`（md 源码 + 预览分屏）。
- **WAL 迁移**（前置）：共享 DB 迁 `journal_mode=WAL` + `busy_timeout`，允许多进程并发（全应用受益）。
- **notes 表重建**：去 `content_html`，核心字段 = `content_text` + `type('text'|'markdown')`；drop + recreate，旧数据不迁移。
- **IPC**（Tauri↔egui）：本地 TCP socket + JSON line + 单实例锁（port 文件 + pid）。
- **macOS 集成**：egui 进程作 Accessory agent（无 Dock 图标）优先；**解决不了则接受 2 个 Dock 图标**（兜底）。
- Tauri 侧：`open_notepad*` 改走 IPC、`notepad_window` webview 删除、`notepad://changed` → IPC `notes_changed`、notepad 命令薄层废弃。
- spike 4 项（见 §7）。

**不做（YAGNI / 后续阶段）：**
- compact editor / 剪贴板浮窗 / 语音识别框迁移（第一阶段后，单 egui 进程加 view）。
- **note-img 图片渲染**（暂不考虑；md 里的图片语法第一阶段不渲染为纹理）。
- WYSIWYG 富文本（轻量定位，不要）。
- html 导出（砍掉，只留 md/txt 导出）。
- 一次性数据迁移（表重建，旧数据接受丢失）。
- egui→Tauri 反向 IPC（egui 直连 DB，第一阶段不需要）。

## 3. 架构

### 3.1 进程拓扑

```
┌─ Tauri 主进程（现状，不动主干）─────────────────────┐
│  模型加载(ASR/OCR/LLM) · 剪贴板监听 · 截图 ·        │
│  托盘 · 全局快捷键 · 其余 webview 窗口              │
│        │ std::process::Command spawn + IPC         │
└────────┼───────────────────────────────────────────┘
         │  本地 TCP socket（127.0.0.1，JSON line）
         ▼
┌─ egui 进程（新二进制 octopus-egui）─────────────────┐
│  eframe 单进程 · view 路由                          │
│   └ 第一阶段：NotepadView（md 源码 + 预览分屏）     │
│   └ 预留：CompactEditor / Clipboard / AsrResult     │ ← 后续阶段不开新进程
└─────────────────────────────────────────────────────┘
         │  直连（各自 Connection，WAL）
         ▼
   ~/.octopus/octopus.db  （octopus-notepad / octopus-clipboard store 两端复用）
```

**单 egui 进程 + 多视图**：egui 进程有固定基线（wgpu/winit/字体），多视图共享一份才划算。第一阶段 view 路由只 `NotepadView`，为后续 compact editor / 剪贴板 / ASR 结果框留口子。多窗口用 egui `ViewportBuilder`（0.28+），第一阶段单窗口不纠结。

**crate 边界**：`octopus-egui` 二进制**不依赖 tauri**，依赖 `eframe` + `egui_commonmark` + `octopus_notepad`（store）+ `octopus_clipboard`（store，note-img 暂不用但保留可选）+ `octopus_infra`（db）+ `serde`/`serde_json`。Tauri 侧用 `std::process::Command` spawn 它，IPC 用 `std::net::TcpListener`/`TcpStream`。

### 3.2 数据层

**3.2.1 WAL 迁移（前置，全应用受益）**

`infra/src/db.rs` 的 `ensure_db` 打开连接后补：
```rust
conn.pragma_update(None, "journal_mode", "WAL")?;     // DB 级持久，设一次
conn.pragma_update(None, "busy_timeout", 5000)?;       // 连接级，每连接设
conn.pragma_update(None, "synchronous", "NORMAL")?;    // WAL 下安全且更快
```
- 产生 `-wal`/`-shm` 副文件（正常，备份一起带）。
- 这步动 infra 层，Tauri 单进程也立即受益（并发更好），可独立先合。
- **验收**：egui 保存笔记 + Tauri 同时 OCR 入库，压测不 `database is locked`。

**3.2.2 notes 表重建（content_text + type）**

去掉 `content_html`，新 schema：
```sql
-- notes 表 drop + recreate（user_version bump）
id, title, content_text, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at
-- type: 'text' | 'markdown'（默认 'text'）
```
- egui 编辑 → `type='markdown', content_text=md 源码`。
- `save_ocr_to_note` / `save_transcription_to_note` → 去掉 `<p>` 包裹，直接 `type='text', content_text=纯文本`。
- 导入 md → `type='markdown', content_text=md`。
- FTS5 仍索引 `content_text`（md 源码可被搜到）。
- **旧数据不迁移**（表重建丢失，已确认接受）。

**3.2.3 store 复用 + 连接持有**

- egui 进程调 `octopus_infra::db::ensure_db(path)` 初始化**自己的** Connection（WAL），复用 `octopus_notepad::store::*_at(conn, ...)` 全部逻辑——store 层零改动。
- egui **不每帧查 DB**：笔记列表/当前笔记载入内存，编辑走 800ms 防抖保存（对齐现状）。`with_db`（进程内单连接）egui 进程直接用。
- 两进程各持一个 Connection 指向同一 DB 文件，WAL 保证并发读写。

### 3.3 编辑器 UI（NotepadView）

**三栏布局**（沿用原 webview 记事本，不重新发明）：

```
┌─────────────┬──────────────────────────────┐
│ 列表（左）   │ 标题 input                    │
│  搜索框      ├──────────────┬───────────────┤
│  来源 tab    │ md 源码编辑   │ 预览           │
│  收藏过滤    │ (TextEdit    │ (egui_commonmark│
│  分页        │  multiline)  │  渲染)         │
│  笔记条目    │              │               │
└─────────────┴──────────────┴───────────────┘
```

- **编辑形态**：md 源码 + 实时预览**分屏**（左编辑右预览，`egui_commonmark` 渲染当前 `content_text`）；窄窗退化为编辑/预览切换 tab。
- **工具栏**（极简 5 按钮，选中文本→包 md 语法）：`**粗体**` / `*斜体*` / `# 标题` / `- 列表` / `` `代码` ``。
- **其余对齐现状**：标题 input、800ms 防抖自动保存、收藏/置顶 toggle、来源 tab（all/asr/ocr/clipboard）、搜索（FTS）、分页。
- **egui 注意**：
  - 列表 `ScrollArea`；笔记量上千要虚拟化（按可见区裁剪），先全量、量大了再优化。
  - `TextEdit::multiline` 大文本（1MB+）会卡（egui 已知 issue #3086）；单条笔记不会那么大，可接受。
  - `egui_commonmark` 预览**只在 `content_text` 变更时重解析**（缓存解析结果），不每帧 parse。

### 3.4 IPC 协议（Tauri↔egui，本地 TCP + JSON line）

- egui 进程启动 bind `127.0.0.1:0`（OS 分配端口）→ 把 `{pid, port}` 写入 `~/.octopus/egui-ipc.port`。
- Tauri「打开记事本」：读 port 文件 → 连 → **连得上**发消息；**连不上 / pid 已死**（`kill(pid,0)` 检测）→ 删 port 文件 → `spawn octopus-egui`（命令行带初始 `note_id`），新进程起来后重写 port 文件。port 文件 + pid 存活 = 单实例锁。
- 协议：JSON line（每行一条 JSON）。
- 消息（Tauri → egui）：
  - `{"type":"open","note_id":N}` — 打开并选中（OCR/ASR→notepad 场景）。
  - `{"type":"notes_changed"}` — Tauri 侧写笔记后通知 egui 刷新列表（**替代原 `notepad://changed`**）。
  - `{"type":"show"}` — 托盘唤起 / show+focus。
- egui → Tauri：第一阶段不需要（egui 直连 DB）。

### 3.5 macOS 窗口集成（⚠️ 最大未知，spike 重点）

egui 独立进程 = 两个 app，macOS 要处理好「一个 Dock 图标」语义：

- **目标**：egui 进程作 **Accessory agent**（`LSUIElement=true` / 运行时 `setActivationPolicy(.accessory)`）—— **不占 Dock 图标**，窗口仍能弹（helper app 标准做法，像 Spotlight/输入法）。Tauri 主应用独占 Dock。
- 托盘「记事本」留在 Tauri 主进程 → 发 IPC `show` → egui 窗口 `set_focus`/unminimize/raise。
- **兜底**：若 Accessory 配置/双进程 focus 抢夺搞不定，**接受 2 个 Dock 图标**（egui 进程 Regular 策略）。功能不阻断，仅体验降级。
- **风险**：eframe/tao 能否配 Accessory 策略 + 两进程 focus/菜单栏怪象——spike 实测。

### 3.6 Tauri 侧改动

- `open_notepad` / `open_notepad_with_note`：改实现——不再建 webview，走 IPC client（连不上则 spawn）。
- 新增 **IPC client 模块**（本地 TCP，发 JSON line，spawn `octopus-egui` 二进制）。
- `notepad_window.rs`：webview 创建路径**删除**。
- `note_commands.rs` 里所有 `emit("notepad://changed")` → 改 IPC 发 `notes_changed`。
- 托盘「记事本」→ IPC `show`。
- **notepad 的 Tauri 命令薄层**：`note_commands.rs` 共 14 个命令，其中 **12 个废弃**（egui 直连 `octopus_notepad::store`，不走 invoke）——`list_notes`/`count_notes`/`get_note`/`create_note`/`update_note`/`delete_notes`/`toggle_note_pinned`/`toggle_note_favorite`/`export_note`/`import_note_from_file`/`get_note_image`/`insert_note_image`。store 层保留（egui 用）。
  - **保留 2 个**（Tauri 主进程集成入口，OCR/ASR 识别后调）：`save_ocr_to_note` / `save_transcription_to_note`，内部改写 `type='text'` + 发 IPC `notes_changed`（原 `emit("notepad://changed")` 改 IPC）。

## 4. 数据流

**OCR → 记事本（端到端）：**
```
image_preview OCR 按钮
  → invoke ocr_image(id) → 文本
  → save_ocr_to_note(text)：写 notes(type='text', content_text=text) → IPC 发 {open, note_id} + {notes_changed}
  ▼
egui 进程收 {open, note_id}：选中该笔记 + focus 窗口；收 {notes_changed}：刷新列表
```

**编辑保存：**
```
egui NotepadView 编辑 md（800ms 防抖）
  → octopus_notepad::store::update_note_at(conn, id, title, content_text, type='markdown')
  → WAL 写入（Tauri 进程并发读不阻塞）
```

**托盘唤起：**
```
Tauri 托盘「记事本」
  → IPC client：连 egui 进程 → 发 {show}
  → egui 窗口 set_focus / unminimize / raise（连不上则 spawn）
```

## 5. 错误处理与边界

| 场景 | 处理 |
|---|---|
| IPC 连不上 / pid 已死 | 删 port 文件 → spawn 新 egui 进程（命令行带初始 note_id） |
| WAL 偶发 busy | `busy_timeout=5000` 自动重试，不立即 `database is locked` |
| egui 进程崩溃 | port 文件残留但 pid 死 → Tauri 下次打开检测到 → 重建 |
| `TextEdit` 大文本卡顿 | 单条笔记不会到 1MB+，可接受；未来按需优化 |
| `egui_commonmark` 每帧重解析 | 只在 `content_text` 变更时重解析（缓存） |
| macOS 双图标（Accessory 搞不定） | 兜底接受 2 个 Dock 图标，功能不阻断 |
| 删表丢旧笔记 | 已确认接受（重建 notes 表） |
| egui 进程被杀但 port 文件在 | pid 存活检测（`kill(pid,0)`）识别并清理 |

## 6. 测试

- **store 层**：`octopus_notepad` store 单测适配 `content_text` + `type`（create/update/list/FTS）。
- **WAL**：两进程（或两线程各持 Connection）并发读写压测，不 locked。
- **IPC**：client/server 往返单测（本地 TCP，mock `open`/`notes_changed`/`show`）。
- **egui UI + macOS 集成**：布局逻辑用 `egui::__run_test_ctx` 单测固化（如侧栏 `exact_size(260)` 实际宽度、panel 间 gap=0——可在无 GUI 下确定性断言，曾据此证伪「线上诊断出的 324.28 黑区」实为旧二进制）；macOS Accessory / focus / 托盘唤起走手动 e2e。
- **端到端**：Tauri spawn egui → OCR→notepad 打开选中 → 编辑保存 → 托盘唤起。

## 7. spike（写代码前先过 4 项）

1. **WAL**：迁移 + 两进程并发读写压测不锁死。
2. **IPC**：空 egui 窗口被 Tauri spawn + TCP 收 `{open, note_id}` 并打印。
3. **macOS Accessory**（最难，但有兜底）：egui 进程无 Dock 图标、窗口能 show/focus，主应用仍一个 Dock 图标。**不过则接受双图标继续。**
4. **egui_commonmark 预览**：只在 `content_text` 变更时重解析（性能 sanity）。

## 8. 分阶段路线

- **阶段 1（本 spec）**：记事本 → egui 独立进程（md 源码 + 预览）+ WAL + 表重建 + IPC + macOS 集成。
- **阶段 2+**：compact editor / 剪贴板浮窗 / 语音识别框（result_window）→ **同一 egui 进程加 view**（不开新进程，摊薄基线）。每阶段独立 spec → plan。

## 9. 风险提示

- **macOS Accessory 集成**：最大未知；兜底双图标已确认可接受，不阻断方案。
- **WAL 迁移影响全应用**：低风险（WAL 比 DELETE 更并发友好），但是 infra 层改动，需回归现有 clipboard/note 读写。
- **egui 全新依赖**：workspace 首次引入 wgpu/eframe，编译时间增加；CI 构建调整。
- **删表丢数据**：已确认接受；若后续需保留，加一次性 md 迁移脚本（本 spec 不做）。
- **egui 列表大数据**：先全量渲染，上千条再虚拟化（egui 无现成虚拟列表）。

## 10. 文档同步

- `docs/architecture.md`：加 `octopus-egui` crate 说明、进程拓扑、IPC、WAL；记事本窗口从 webview 改注为 egui 进程。
- 本 spec → `docs/superpowers/plans/2026-07-01-notepad-egui.md`（writing-plans 产出）。
- `docs/superpowers/specs/2026-06-30-notepad-design.md`（原 webview 记事本）：顶部标注「已被 egui 方案替代，见 2026-07-01-notepad-egui-design.md」。
