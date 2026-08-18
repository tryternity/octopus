# ActionBar 转 Markdown 命令设计

- 日期：2026-08-18
- 类型：增量功能（新 crate + ActionBar 新 action_type）
- 依赖：ActionBar 现有链路（`detect_selection` / `PENDING_CONTEXT` / `execute_action_bar_inner` / `action_bar_show_result`）

## 1. 需求

召唤 ActionBar（Alt+D / 菜单项独立热键）后，对当前上下文执行「转 Markdown」命令：

| 输入 | 转换语义 |
|---|---|
| 选中的网页文本 | 读剪贴板 **HTML flavor**，保留粗体/链接/列表/表格转 Markdown |
| 选中的纯文本 | 无格式可转，直通原文展示 |
| Finder 选中文件（单/多） | 按格式矩阵逐文件转换，多文件合并为单文档（带文件树头） |
| Finder 选中文件夹 | 递归遍历（跳过隐藏/垃圾目录），合并为单文档（带文件树头） |

用途（brainstorm 确认，三者兼容）：喂给 AI 处理、存笔记/归档（保留结构与图片链接）、随手查阅（快）。

**输出**：CompactEditor 临时 tab 展示 Markdown 预览（复用 `action_bar_show_result` 模板），`write_output_to_clipboard=1` 时同时写剪贴板。

### 范围外（v1 明确不做）

- **URL 抓取**：选中 URL 不做本地 GET + html→md，留作后续独立 task（含静态抓取 vs WKWebView 渲染的取舍）
- **图片 → OCR → Markdown**：现有 `octopus-ocr::layout::to_markdown` 链路可作后续扩展点，v1 图片一律 skipped
- **zip/tar 压缩包递归**：v1 视为 Binary 跳过
- **剪贴板恢复时保留 HTML flavor**：模拟 Cmd+C 覆盖剪贴板后恢复写回，原剪贴板自带的 HTML flavor 会丢失（少见场景，接受）

## 2. 方案选型

| 方案 | 结论 |
|---|---|
| **A. anydoc + htmd，新建 octopus-convert crate（选定）** | 纯 Rust 自包含（DMG 分发无外部依赖）、格式最全、质量速度双优 |
| B. script 菜单项 shell out markitdown CLI | 依赖用户 Python 环境、能力受限、大文本走环境变量有风险 |
| C. markdownify crate | 无 PDF/doc/RTF/EPUB（归档硬伤）；其 `convert_files` 合并能力自己实现更薄 |

### 2.1 引擎调研结论（2026-08-18）

- **anydoc**（firecrawl/anydoc，v0.1.9）：14 种格式（doc/docx/docm/ppt/pptx/pptm/pps/ppsx/ppsm/pot/xls/xlsx/xlsm/xlsb/odt/ods/odp/rtf/epub/pdf/csv），纯 Rust 无外部依赖（PDF 用自研 pdf-inspector，无需 poppler/LibreOffice），官方 benchmark 中位 4.4ms、LLM 盲评 81 分（markitdown 65）。MIT，公司维护。**0.1.x API 可能变动，Cargo.toml 锁死 `=0.1.9`**。已知限制：扫描版（纯图片）PDF 无法本地转换。
- **htmd**（v0.5.5）：turndown.js 血统的 HTML→Markdown，275 万下载，活跃维护（对比 html2md 2025-01 起停更，弃用）。

## 3. 总体架构与数据流

```
召唤 ActionBar（Alt+D / 菜单项独立热键）
   │
   ├─ detect_selection（现有，扩展）
   │    Finder 文件/文件夹 → ActionBarContext { kind: Files, files }（现有）
   │    选中文本 → 模拟 Cmd+C 读剪贴板文本（现有）
   │             + 新增：读 pasteboard public.html flavor → ctx.html: Option<String>
   │
   ├─ 前端菜单过滤：「转 Markdown」accepts="any"（文本/文件/无选中都显示）
   │
   └─ 执行（两路径汇合于 execute_action_bar_inner 的 "markdown" 分支）
        ├─ 前端点击：invoke("execute_action_bar", { itemId, text, html, files })
        └─ Quick Execute 热键：handle_files_selection 写 PENDING_CONTEXT → inner 读 files（现有机制）
   │
   ├─ spawn_blocking → octopus_convert 按输入分派：
   │    html 有值 → html_to_markdown（htmd）
   │    纯文本   → 直通原文
   │    单/多文件 → convert_files（anydoc/htmd/codeblock/md 按矩阵分派 + 合并）
   │    文件夹   → convert_folder（walkdir 过滤 + 上限守卫 + 合并单文档）
   │
   └─ action_bar_show_result（现有）
        → CompactEditor 临时 tab Markdown 预览
        → write_output_to_clipboard=1 时写剪贴板
```

**inner 输入优先级**：显式 `files` 参数 > `PENDING_CONTEXT.files` > `html` > `text`；全空报「没有可转换的内容」。文件路径按 `is_dir` 分流 `convert_folder` / `convert_files`。

### 3.1 格式分派矩阵（octopus_convert::dispatch）

| FormatKind | 扩展名 | 策略 |
|---|---|---|
| Anydoc | doc/docx/docm/ppt/pptx/pptm/pps/ppsx/ppsm/pot/xls/xlsx/xlsm/xlsb/odt/ods/odp/rtf/epub/pdf/csv | `anydoc::to_markdown_bytes` |
| Html | html/htm | htmd |
| Md | md/markdown | 原样嵌入 |
| Code | py/rs/ts/tsx/js/jsx/json/yml/yaml/toml/xml/sh/bash/zsh/txt/log（**封闭清单**） | fenced code block + 按扩展名标语言 |
| Binary | 图片、zip/tar 及一切不在上述清单的扩展/无扩展 | 单文件报「暂不支持」；文件夹场景 skipped 标注 |

**合并形态规则**：单文件 → 直接输出转换内容（无树头，便于直接复制）；多文件/文件夹 → §4.3 合并文档形态，标题用公共父目录名。

## 4. octopus-convert crate

**零项目内依赖**（对齐 infra 惯例，纯转换域），workspace `members` + `default-members` 注册。外部依赖：`anydoc = "=0.1.9"`、`htmd`、`walkdir`（均走 workspace 依赖版本管理）。

```
crates/convert/
├── Cargo.toml
├── assets/       # 测试用真实小文件：sample.docx / sample.xlsx / sample.csv / sample.pdf（Cargo.toml include）
└── src/
    ├── lib.rs      # pub use 具名 re-export（对齐 octopus-translation 惯例）
    ├── dispatch.rs # 纯函数：扩展名 → FormatKind
    ├── convert.rs  # convert_one(path) -> FileSection（单文件转换核心）
    ├── folder.rs   # walkdir 遍历 + 过滤 + 上限 + 树头 + merge_sections 合并核心
    └── error.rs    # ConvertError
```

### 4.1 复用设计——一条转换核心，三种入口

```rust
pub struct FileSection {
    pub rel_path: String,
    pub content: Result<String, ConvertError>,
}

fn convert_one(path: &Path) -> FileSection            // 唯一转换单元
fn merge_sections(sections: Vec<FileSection>, title: &str) -> String  // 唯一合并单元

pub fn convert_files(paths: &[PathBuf]) -> Result<String, ConvertError>  // N × convert_one + merge
pub fn convert_folder(root: &Path) -> Result<String, ConvertError>       // walkdir → 同一条 convert_files
pub fn html_to_markdown(html: &str) -> String                            // htmd 包装（含配置）
```

单文件、多文件、文件夹不写三套逻辑：folder 只是「产生路径列表」的方式不同，转换与合并完全共享。`FileSection.content` 用 `Result` 服务两种错误语义——单文件上抛，文件夹降级为 skipped 标注不中断。

### 4.2 守卫常量（pub const，变更需同步本 spec）

- `MAX_FILES = 200`
- `MAX_TOTAL_BYTES = 50 * 1024 * 1024`（50MB）
- 忽略目录：`.git` / `node_modules` / `target` / `__pycache__` / `.venv` / `dist` / `build`，及所有隐藏文件（`.` 开头）

### 4.3 文件夹输出文档形态

```markdown
# <folder-name>/

## 文件树

​```
<ascii tree，路径相对根，排序确定>
​```

## path/to/a.docx

<anydoc 转出内容>

## path/to/main.py

​```python
...
​```

> ⚠️ skipped: img/logo.png（暂不支持）
```

## 5. desktop / 前端集成

### 5.1 选区采集扩展（唯一动现有代码处，保持薄）

- `ActionBarContext`（`context.rs`）加 `html: Option<String>`，serde camelCase；前端 `types.ts` Context 加 `html?: string | null`（casing 一一对应）
- `ClipboardHandle`（`crates/clipboard/src/handle.rs`）加 `read_html()`：pasteboard 含 `public.html` flavor 时读取（浏览器/WKWebView app 提供；TextEdit 等返回 None）
- 剪贴板三态恢复逻辑不动

### 5.2 执行链路

- `execute_action_bar` 命令（`script.rs:590`）加可选参数 `html: Option<String>`、`files: Option<Vec<String>>`（Tauri camelCase 自动映射），前端 `executeItem` 透传 `ctx?.html` / `ctx?.files`
- `execute_action_bar_inner`（`script.rs:359`）match 加 `"markdown"` 分支，按 §3 优先级分派；`spawn_blocking` 包裹（对齐 ai 分支）；本地毫秒级不设超时
- `ConvertError` → `Err(String)` 走现有前端 toast；成功走 `action_bar_show_result(md, text, item.title, app, item.write_output_to_clipboard)`——剪贴板写入由 show_result 内部统一处理（对齐 ai 分支模式）

### 5.3 action_type 与 seed

- `action_type = 'markdown'`（TEXT 列，零 schema 变更）
- seed 系统项「转 Markdown」：`is_system=1, action_type='markdown', accepts='any', write_output_to_clipboard=1`，位置在 AI 子菜单后；`schema.sql` 新装 + 存量 DB 迁移 INSERT IF NOT EXISTS（schema v60 → v61，细节在 plan 落）
- 系统项禁删/禁改类型由现有机制自动覆盖
- 前端 `constants.tsx`：`ACTION_TYPES` 加 markdown（图标 + label），`deriveAccepts('markdown') → 'any'`；用户也可自建 markdown 型菜单项（行为同内置）

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| 单文件 Binary 格式 | toast「暂不支持 .xxx」 |
| anydoc 转换失败（损坏文件） | 透传错误信息 |
| 扫描版 PDF | 错误文案附「扫描版 PDF 暂不支持，可截图走 OCR」 |
| 文件夹超限 | 「N 个文件 / X MB 超出上限（200 文件 / 50MB），请缩小范围」 |
| 文件夹内不支持文件 | skipped 标注，不中断整体 |
| 无选中且无参数 | 「没有可转换的内容」 |
| 纯文本（无 HTML flavor） | 直通展示原文 |

## 7. 测试计划（TDD，每模块红→绿先行）

| 模块 | 测试 |
|---|---|
| dispatch.rs | 表驱动：每个扩展名 → 期望 FormatKind；未知/无扩展 → Binary |
| folder.rs | 临时目录 fixture：遍历排序确定性、隐藏文件排除、忽略目录排除、上限触发（201 文件报错）、树头渲染、skipped 标注、混合格式合并输出 |
| convert.rs | md 原样嵌入；codeblock 语言标注（.py → python）；assets/ 真实 docx/xlsx/csv/pdf 过 anydoc 接线测试（毫秒级，无需 ignore 标记） |
| error.rs | Display 文案断言 |
| desktop inner 分支 | 构造 markdown 型 ActionBarItem + 临时文件 → `execute_action_bar_inner` 出 md 的内联测试（照抄 script 分支测试模式） |
| 前端 | `deriveAccepts('markdown') === 'any'` 断言 |

NSPasteboard HTML flavor 读取是唯一不可单测的 macOS 胶水，保持单行薄封装。

## 8. 文档同步清单

- `docs/architecture.md`：crate 树 + 依赖图加 octopus-convert
- `docs/features/desktop-app.md` §12/§14：「转 Markdown」命令说明
- `AGENTS.md`：Cargo Workspace 结构列表加 `convert/`
- 本 spec 为设计真相源；实施偏差回写
