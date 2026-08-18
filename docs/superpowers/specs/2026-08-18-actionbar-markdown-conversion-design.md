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

**输出**（2026-08-18 修订）：命令**异步执行**——立即收口隐藏 ActionBar，后台转换完成后把结果写入 `~/Documents/octopus/markitdown/<源名>_<yyyymmdd-HHMMSS>.md`（可经 `markitdown_output_dir` 配置覆盖），并用 CompactEditor **file tab** 打开（编辑保存可写回磁盘；同路径重复打开聚焦同一 tab）；`write_output_to_clipboard=1` 时同时写剪贴板。异步失败时开 CompactEditor 错误 temp tab 反馈。

### 范围外（v1 明确不做）

- **URL 抓取**：v1 不做——已于 2026-08-18 独立 spec 落地设计：[url-to-markdown](2026-08-18-url-to-markdown-design.md)（静态 + WKWebView 渲染 fallback）
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
   └─ 异步收口（2026-08-18 修订）：inner 立即 Ok(false)（外层统一收口隐藏 ActionBar）
        → tokio::spawn 后台：convert_and_save 写 ~/Documents/octopus/markitdown/<stem>_<时间戳>.md
        → 主线程 open_disk_file_in_compact_editor（file tab，保存写回）
        → write_output_to_clipboard=1 时同时写剪贴板；失败开错误 temp tab
```

**inner 输入优先级**：显式 `files` 参数 > `PENDING_CONTEXT.files` > `url`（单行显式 URL，意图优先于 `html`——2026-08-18 终审修订，见 [URL spec §9⑬](2026-08-18-url-to-markdown-design.md)）> `html` > `text`；全空报「没有可转换的内容」。文件路径按 `is_dir` 分流 `convert_folder` / `convert_files`。

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
- `execute_action_bar_inner`（`script.rs:359`）match 加 `"markdown"` 分支，按 §3 优先级分派
- **异步执行（2026-08-18 修订）**：分支内 `tokio::spawn` 后台任务（内部 `spawn_blocking` 跑转换），立即返回 `Ok(false)` 走外层统一收口——ActionBar 即刻隐藏，转换完成后写文件 + 主线程 `open_disk_file_in_compact_editor`（从 `prompt_files.rs::open_file_in_editor` 抽取共用的 file tab 打开函数，md5(路径) 做 tab 去重 id）；失败开 `TempTabPayload` 错误 temp tab（`agent-task://error` 只有 Result 浮窗监听、不一定可见）
- **输出目录**：`infra/paths.rs::markitdown_dir()`——沿用 recordings/screens 约定（DB `markitdown_output_dir` 配置可覆盖，兜底 `~/Documents/octopus/markitdown`）；文件名 `<源 stem>_<yyyymmdd-HHMMSS>.md`（单文件=文件 stem，文件夹=目录名，多选=公共父目录名，text/html=`markitdown`），同秒碰撞追加 `-1/-2` 后缀
- `ConvertError` → 后台任务错误信息进错误 temp tab；`write_output_to_clipboard=1` 时后台直接写剪贴板

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

## 9. 实施注记（2026-08-18 实施回写）

实施于 worktree `.worktree/markdown-conversion`（分支 `markdown-conversion`），与设计的偏差与补充：

1. **anydoc/htmd API 无偏差**：`anydoc::to_markdown(path)` 与 `htmd::HtmlToMarkdown::builder().skip_tags(...)` 均按 §2.1 预期工作，错误类型实现 Display。
2. **desktop `markdown.rs` 注册方式微调**：子模块经 `mod markdown;` 声明（无 `pub use markdown::*` glob re-export）——`run_markdown_convert` 是 `pub(crate)`，glob re-export 会产生 warning（无 pub 项可导出），与兄弟模块模式略异但语义一致。
3. **既有测试预期更新**（v61 演进导致，非 bug）：`action_bar_non_submenu_accepts_default_text` 排除 markdown 型并单独断言其 `accepts='any'`；`migrate_v59_to_v60` 的版本断言改为 `CURRENT_SCHEMA_VERSION`（迁移链跑到最新）。
4. **assets fixture 精简**：xlsx/pdf fixture 未生成（textutil 只产 docx/rtf）——csv + docx 已覆盖 anydoc 接线路径，xlsx/pdf 同代码路径，风险极低。
5. **worktree 基线补充**：desktop 依赖 gitignore 的 `binaries/octopus-sck-helper`（tauri resource），新 worktree 须先跑 `./scripts/build-macos-helper.sh` 才能 `cargo build/test`。
6. **plan 测试计数笔误**：Task 7 的 `run_markdown_convert` 测试实际 7 个（plan 误写「8 个测试全过」预期）。

### 9.1 首轮交付后修订（2026-08-18，用户反馈）

用户要求变更输出链路，已实施：

1. **异步执行**：inner 的 markdown 分支由「await 转换 + `action_bar_show_result` 同步展示」改为 `tokio::spawn` 后台执行、立即 `Ok(false)` 收口。前端零改动（通用分支天然适配）。
2. **落盘**：`markitdown_dir()`（infra paths，DB 键可覆盖）+ `<stem>_<时间戳>.md` 命名 + 同秒 `-N` 后缀；`convert_and_save_to(dir)` 注入目录便于测试。
3. **CompactEditor file tab 打开**：从 `prompt_files.rs::open_file_in_editor` 抽取 `open_disk_file_in_compact_editor` 共用（prompt 文件查看与转 Markdown 输出同一条打开链路）；prompt_files 重构为薄包装。
4. **错误反馈**：后台失败开 CompactEditor 错误 temp tab（放弃 `agent-task://error`——其监听方 Result 浮窗不一定可见）。
5. 新增测试：`markitdown_dir` 兜底（含 `init_test_db` 隔离，防绑开发库）、`output_file_stem` ×4、`convert_and_save_to` ×3（写入/碰撞/错误透传）。

### 9.2 CompactEditor 大文件加载修复（2026-08-18，z_perf 流程）

落盘打开链路上线后发现 CompactEditor 打开大文件（MB 级转 Markdown 产物）冻结。按 z_perf measurement-first 修复：

**Baseline**（`largeDocPerf.test.ts`，可复现 workload 保留在测试套件）：
- `renderMarkdown`（markdown-it 全文）~100ms/MB 线性；2MB→212ms、HTML 4.8MB
- CM6 `EditorState.create` 1MB：plain 7.4ms / +markdown() 13.9ms——建态不是热点
- jsdom 测不到的主成本：整篇 `innerHTML` 的 WKWebView DOM 解析+布局（数 MB HTML，秒级冻结）
- 次要：`CodeMirrorEditor` value effect 每键 `doc.toString()` O(N) 比对（2MB 每键数 ms）

**修复**（只修证明过的热点）：
1. **预览截断**（`previewTruncate.ts`，TDD 4 测试）：>256KB 按行边界截断渲染 + 提示条（i18n `editor.previewTruncated`）。after 实测：2MB 预览 JS 渲染 212ms→22ms，HTML 4.8MB→~600KB，DOM 成本有界。编辑栏/保存仍承载全文（file tab 保存写回不受影响）。
2. **每键 O(N) 消除**：`lastEmittedRef` 回声快路径，外部变更才全量 diff。

**明确不做**（YAGNI，未证明热点）：大文档 CM6 语言降级（1MB +markdown() 仅 14ms）、IPC 分片传输（一次性 MB 级 IPC 数十 ms）、preview 虚拟滚动（截断已使 DOM 有界，复杂度不值）。

同步文档：`docs/features/compact-editor.md`「大文档防护」段。
