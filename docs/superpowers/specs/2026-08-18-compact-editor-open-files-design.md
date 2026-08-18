# CompactEditor 打开已存在文件 设计

- 日期：2026-08-18
- 类型：增量功能（复用现有 file tab / 图片入库 / 批量开 tab 三条链路）
- 依赖：`open_disk_file_in_compact_editor`（2026-08-18 抽取）、clipboard 图片体系（`encode_image` / `insert_image_data` / `insert_clipboard_item`）、`open_compact_editor_tabs` 批量开窗机制

## 1. 需求

允许 CompactEditor 打开磁盘上已存在的文件（brainstorm 确认）：

| 维度 | 决定 |
|---|---|
| 入口 | ① 工具栏「打开」按钮（plugin-dialog 文件选择器，多选）；② 拖文件到窗口（Tauri `onDragDropEvent`，Terminal 同模式） |
| 文件类型 | 文本（任意扩展名按 UTF-8 读，失败拒绝）+ 图片（png/jpg/jpeg/gif/webp/bmp/tiff/tif） |
| 图片策略 | **入库复用全能力**：文件图片写入剪贴板图片体系得 imageId，ImagePreview 零改造（OCR/二维码/复制/缩放全可用）；代价是剪贴板历史多一条可删记录 |
| 多选 | 选择器与拖拽都支持多文件，逐个开 tab |

**范围外（v1 不做）**：文件夹拖入（只收文件，目录路径进 errors）；图片重复打开的 history 去重（`insert_image_data` 文件级 hash 去重已有，history 行可删）；非 UTF-8 编码文本转码（报错拒绝）；新建文件（只打开已存在）。

## 2. 总体架构与数据流

```
入口 A：工具栏「打开」按钮 → plugin-dialog open({ multiple: true, filters })
入口 B：拖文件入窗口 → getCurrentWebview().onDragDropEvent（paths）
   │
   └─ 收敛 invoke("open_files_in_editor", { paths: Vec<String> })
        │
        ├─ 逐路径分流 is_image_ext（纯函数，封闭清单）：
        │    图片 → fs::read → image::load_from_memory → hash_rgba + encode_image
        │           → insert_image_data + insert_clipboard_item(type=image, ref_data=hash)
        │           → imageId + 宽高 → 图片 tab（source="clipboard"，前端 loadAndAddTab 识别）
        │    其余 → fs::read_to_string（非 UTF-8/IO 失败 → errors）
        │           → file tab（text + filePath，md5(路径) itemId 去重聚焦）
        │
        └─ open_tabs_batched(Vec<PendingTabFull>)：mounted 检测 → 逐个 emit 或
           全部 push PENDING_TABS + 一次 create_compact_editor_window
        返回 { errors: Vec<String> } → 前端非空时聚合 toast
```

## 3. 后端设计

**位置**：`crates/desktop/src/commands/compact_editor_commands.rs`。

### 3.1 分流纯函数（TDD 表驱动）

```rust
/// 图片扩展名封闭清单（spec §1）；其余一律尝试 UTF-8 文本读。
fn is_image_ext(ext: &str) -> bool
// png jpg jpeg gif webp bmp tiff tif（大小写不敏感，容忍前导点）
```

### 3.2 `open_tabs_batched` 泛化（复用核心）

把 `open_compact_editor_tabs` 的「`PENDING_TABS.is_empty()` 判 React mounted → mounted 逐个 emit / 未 mounted 全部 push + 一次建窗」机制泛化为：

```rust
/// 批量开 tab（完整 payload 直传，不查 DB）。两条调用方：
/// - open_compact_editor_tabs（原 (id, source) 元组路径，经 push_pending_tab 组装后转入）
/// - open_files_in_editor（file tab 全文 + 图片 tab imageId 直传）
fn open_tabs_batched(tabs: Vec<PendingTabFull>, app: &AppHandle)
```

原 `open_compact_editor_tabs` 保持签名不变（内部转调），行为零变化。file tab 的 emit payload 与 `open_disk_file_in_compact_editor` 一致（`{itemId, source:"file", text, filePath}`，camelCase）。

### 3.3 `open_files_in_editor` 命令

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFilesResult { pub errors: Vec<String> }

#[tauri::command]
pub async fn open_files_in_editor(paths: Vec<String>, app: AppHandle) -> Result<OpenFilesResult, String>
```

- `spawn_blocking` 包裹（图片解码 + 多文件 IO）
- 图片入库：`octopus_clipboard::image::encode_image`（现有：DynamicImage → 主图 blob + 缩略图）+ `hash_rgba` + `store::insert_image_data` + `store::insert_clipboard_item`（`NewClipboardItem { item_type: "image", ref_data: hash, ... }`）；拿 meta_info 宽高（入库时写）——具体 NewClipboardItem 必填字段在 plan 期对照 store.rs 现有调用点
- 目录路径 / 不存在 / 非 UTF-8 / 图片解码失败 → 逐个进 `errors`（`"<文件名>（<原因>）"`），不中断其他文件
- 空图片尺寸或超 `MAX_IMAGE_TABS` 由前端既有逻辑处理（见 §4）

## 4. 前端设计（`CompactEditor/index.tsx`）

1. **「打开」按钮**：tab 栏尾部 `FolderOpen` 图标 → plugin-dialog `open({ multiple: true, filters: [{ name: "文本与图片", extensions: [...文本+图片] }] })`（RecordConfig 同款 `open as openDialog`）→ invoke
2. **拖拽**：`useEffect([])` 挂 `getCurrentWebview().onDragDropEvent`——**listener 稳定化规范**（跨窗口 listener 踩坑 2 次）：回调经 ref 持有、deps `[]` 只注册一次；drop → `invoke("open_files_in_editor", { paths })`；`drag-over` 期间容器加 `ring` 边框高亮
3. **错误 toast**：返回 `errors` 非空 → toast 聚合（`editor.openFailed`：`N 个文件打开失败：a.bin（非 UTF-8 文本）、c.png（图片解码失败）`；具体 toast 组件用 CompactEditor 现有反馈机制，plan 期核实，兜底 useToast）
4. **图片 tab 上限**：遵守现有 `MAX_IMAGE_TABS = 5`（loadAndAddTab/建 tab 既有约束同路径生效）
5. **大文件**：文本自动受益预览截断防护（spec 2026-08-18-actionbar-markdown-conversion §9.2），无需处理
6. **i18n**（zh/en）：`editor.openFile`（打开文件）、`editor.openFailed`、拖拽提示 `editor.dropHint`（如需）

## 5. 错误处理汇总

| 场景 | 行为 |
|---|---|
| 目录路径（拖入文件夹） | errors：「<名>（暂不支持文件夹）」 |
| 文件不存在/无权限 | errors：IO 错误原因 |
| 非 UTF-8 文本 | errors：「<名>（非 UTF-8 文本）」 |
| 图片解码失败 | errors：「<名>（图片解码失败）」 |
| 空 paths / 全部失败 | Ok({errors})，前端 toast；命令本身不 Err |

## 6. 测试计划（TDD）

| 模块 | 测试 |
|---|---|
| `is_image_ext` | 表驱动：清单内全部 → true；大小写/前导点；md/txt/pdf/空 → false |
| `open_files_in_editor` | tmp 文件集成：文本开 file tab（返回 errors 空）；非 UTF-8 bytes → errors 含「非 UTF-8」；目录 → errors 含「文件夹」；混合批次部分成功 |
| 前端纯函数 | paths 过滤（空/目录剔除策略如有）、错误文案拼接 |
| 既有回归 | `open_compact_editor_tabs` 转调后既有行为（批量开 tab 测试）不回归 |

拖拽 listener 与 dialog 为 macOS 胶水，编译级验证 + 手动 e2e。

## 7. 文档同步清单

- `docs/features/compact-editor.md`：「打开已存在文件」段（双入口/类型/图片入库语义/错误反馈）
- `docs/architecture.md`：CompactEditor 段落补一句（如有对应章节）
- 本 spec 实施注记回写
