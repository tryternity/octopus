# ActionBar 转 Markdown 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ActionBar 新增「转 Markdown」命令——选中网页文本（HTML flavor）/文件/文件夹 → 转 Markdown → CompactEditor 展示 + 写剪贴板。

**Architecture:** 新建零项目内依赖的 `octopus-convert` crate（anydoc 负责 14 种办公格式，htmd 负责 HTML→md），一条 `convert_one` + `merge_sections` 转换核心服务单文件/多文件/文件夹三种入口；desktop 层新增 `action_type="markdown"` 分支，复用 `action_bar_show_result` 收口链路。

**Tech Stack:** Rust（anydoc `=0.1.9`、htmd `0.5`、walkdir `2`）、Tauri 2、React + vitest。

**Spec:** `docs/superpowers/specs/2026-08-18-actionbar-markdown-conversion-design.md`（本 plan 的需求真相源）

## Global Constraints

- **开发隔离**：在 `.worktree/` 下新建 worktree 分支开发（如 `.worktree/markdown-conversion`），**未经用户明确指令不得合并/push 到 main**（AGENTS.md Git 同步纪律）。
- **TDD**：每个纯逻辑模块先写失败测试再实现（用户明确要求 TDD 驱动）。
- **代码复用**：不新造展示/收口链路——一律复用 `action_bar_show_result` / `PENDING_CONTEXT` / `detect_selection`；crate 内单文件/多文件/文件夹共用 `convert_one` + `merge_sections`（用户明确要求）。
- **依赖版本**：`anydoc = "=0.1.9"`（0.1.x 锁死，spec §2.1）、`htmd = "0.5"`、`walkdir = "2"`，全部走 `[workspace.dependencies]`。
- **casing**：`ActionBarContext.html` serde camelCase；前端 `types.ts` 字段一一对应（AGENTS.md 序列化 casing 规范）。
- **守卫常量**（spec §4.2）：`MAX_FILES = 200`、`MAX_TOTAL_BYTES = 50MB`、忽略目录 `.git/node_modules/target/__pycache__/.venv/dist/build` + 所有 `.` 开头文件。
- **0 warning 纪律**：每 task 编译必须 0 error 0 warning；改 struct 签名后 grep 全部构造点（AGENTS.md 改动验证纪律）。
- **验证命令**：Rust 用 `cargo test -p <crate> --lib`；前端用 `cd crates/desktop/frontend && npx vitest run <file>`；全量 `cargo test` + `npm run build`。

---

### Task 1: octopus-convert crate 骨架 + workspace 注册 + ConvertError

**Files:**
- Modify: `Cargo.toml`（根，members + default-members + workspace.dependencies）
- Create: `crates/convert/Cargo.toml`
- Create: `crates/convert/src/lib.rs`
- Create: `crates/convert/src/error.rs`

**Interfaces:**
- Produces: `octopus_convert::ConvertError`（enum：`UnsupportedFormat(String)` / `Anydoc(String)` / `Html(String)` / `Io(std::io::Error)` / `TooManyFiles{count,max}` / `TooLarge{bytes,max_bytes}` / `Empty`），实现 `Display` + `std::error::Error` + `From<io::Error>`。后续所有 task 的错误类型。

- [x] **Step 1: 写失败的 error Display 测试**

`crates/convert/src/error.rs`（测试先行——先只写 tests mod，实现部分留空跑红）：

```rust
use std::fmt;

/// 转换错误——Display 文案是用户直接看到的 toast（spec §6 错误处理表）。
#[derive(Debug)]
pub enum ConvertError {
    UnsupportedFormat(String),
    Anydoc(String),
    Html(String),
    Io(std::io::Error),
    TooManyFiles { count: usize, max: usize },
    TooLarge { bytes: u64, max_bytes: u64 },
    Empty,
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(ext) => write!(f, "暂不支持 .{} 格式", ext),
            Self::Anydoc(e) => write!(f, "文档转换失败: {}", e),
            Self::Html(e) => write!(f, "HTML 转换失败: {}", e),
            Self::Io(e) => write!(f, "文件读取失败: {}", e),
            Self::TooManyFiles { count, max } => {
                write!(f, "{} 个文件超出上限（最多 {} 个），请缩小范围", count, max)
            }
            Self::TooLarge { bytes, max_bytes } => write!(
                f,
                "{:.1}MB 超出上限（最多 {:.0}MB），请缩小范围",
                *bytes as f64 / 1024.0 / 1024.0,
                *max_bytes as f64 / 1024.0 / 1024.0
            ),
            Self::Empty => write!(f, "没有可转换的内容"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<std::io::Error> for ConvertError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_unsupported_format() {
        assert_eq!(
            ConvertError::UnsupportedFormat("png".into()).to_string(),
            "暂不支持 .png 格式"
        );
    }

    #[test]
    fn test_display_too_many_files() {
        let e = ConvertError::TooManyFiles { count: 233, max: 200 }.to_string();
        assert!(e.contains("233 个文件超出上限"));
        assert!(e.contains("200 个"));
    }

    #[test]
    fn test_display_too_large_mb() {
        let e = ConvertError::TooLarge { bytes: 60 * 1024 * 1024, max_bytes: 50 * 1024 * 1024 }.to_string();
        assert!(e.starts_with("60.0MB 超出上限"));
        assert!(e.contains("50MB"));
    }

    #[test]
    fn test_display_empty() {
        assert_eq!(ConvertError::Empty.to_string(), "没有可转换的内容");
    }

    #[test]
    fn test_from_io_error() {
        let e: ConvertError = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert!(e.to_string().contains("文件读取失败"));
    }
}
```

- [x] **Step 2: 建 crate 骨架（测试此时应编译失败——依赖未加）**

`crates/convert/Cargo.toml`：

```toml
[package]
name = "octopus-convert"
version = "0.1.0"
edition = "2021"

[dependencies]
anydoc = { workspace = true }
htmd = { workspace = true }
walkdir = { workspace = true }
```

`crates/convert/src/lib.rs`：

```rust
//! octopus-convert——文档转 Markdown 领域库（spec 2026-08-18-actionbar-markdown-conversion-design）。
//! 零项目内依赖（对齐 infra 惯例）。格式分派 / 单文件转换 / 多文件与文件夹合并。

pub mod error;

pub use error::ConvertError;
```

根 `Cargo.toml` 三处修改：
1. `members` 数组末尾（`"crates/pty"` 后）加 `"crates/convert"`
2. `default-members` 数组末尾（`"crates/record"` 后）加 `"crates/convert"`
3. `[workspace.dependencies]` 段加三行：

```toml
# 转 Markdown（ActionBar markdown 命令，spec 2026-08-18）。anydoc 0.1.x 锁死防 API 变动。
anydoc = "=0.1.9"
htmd = "0.5"
walkdir = "2"
```

- [x] **Step 3: 跑测试验证通过**

```bash
cargo test -p octopus-convert --lib
```

Expected: 5 passed（error.rs 的 5 个测试）。

- [x] **Step 4: Commit**

```bash
git add Cargo.toml crates/convert
git commit -m "feat(convert): octopus-convert crate 骨架 + ConvertError（TDD）"
```

---

### Task 2: dispatch.rs 格式分派（表驱动 TDD）

**Files:**
- Create: `crates/convert/src/dispatch.rs`
- Modify: `crates/convert/src/lib.rs`（加 `pub mod dispatch;`）

**Interfaces:**
- Produces: `FormatKind`（enum：`Anydoc/Html/Md/Code/Binary`）、`pub fn format_kind(ext: &str) -> FormatKind`、`pub fn code_language(ext: &str) -> &'static str`。Task 4 的 `convert_one` 消费。

- [x] **Step 1: 写失败的表驱动测试**

`crates/convert/src/dispatch.rs`：

```rust
//! 格式分派矩阵（spec §3.1）——扩展名 → 转换策略。纯函数，封闭清单。

/// 单文件转换策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    /// anydoc：14 类办公/文档格式
    Anydoc,
    /// htmd：HTML 文件
    Html,
    /// .md 原样嵌入
    Md,
    /// fenced code block + 语言标注
    Code,
    /// 一切不在清单内/无扩展——单文件报不支持，文件夹场景 skipped
    Binary,
}

/// anydoc 覆盖格式（封闭清单，spec §3.1）。
pub const ANYDOC_EXTS: &[&str] = &[
    "doc", "docx", "docm", "ppt", "pptx", "pptm", "pps", "ppsx", "ppsm", "pot",
    "xls", "xlsx", "xlsm", "xlsb", "odt", "ods", "odp", "rtf", "epub", "pdf", "csv",
];
pub const HTML_EXTS: &[&str] = &["html", "htm"];
pub const MD_EXTS: &[&str] = &["md", "markdown"];
/// Code 封闭清单（spec §3.1）——清单外一律 Binary。
pub const CODE_EXTS: &[&str] = &[
    "py", "rs", "ts", "tsx", "js", "jsx", "json", "yml", "yaml", "toml",
    "xml", "sh", "bash", "zsh", "txt", "log",
];

/// 扩展名 → FormatKind。输入带不带点、大小写均可。
pub fn format_kind(ext: &str) -> FormatKind {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    if ANYDOC_EXTS.contains(&e.as_str()) {
        FormatKind::Anydoc
    } else if HTML_EXTS.contains(&e.as_str()) {
        FormatKind::Html
    } else if MD_EXTS.contains(&e.as_str()) {
        FormatKind::Md
    } else if CODE_EXTS.contains(&e.as_str()) {
        FormatKind::Code
    } else {
        FormatKind::Binary
    }
}

/// Code 类扩展名 → fenced code block 语言标注。
pub fn code_language(ext: &str) -> &'static str {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "py" => "python",
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "yml" | "yaml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anydoc_full_matrix() {
        for ext in ANYDOC_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Anydoc, "ext={}", ext);
        }
    }

    #[test]
    fn test_html_md_code_matrix() {
        for ext in HTML_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Html);
        }
        for ext in MD_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Md);
        }
        for ext in CODE_EXTS {
            assert_eq!(format_kind(ext), FormatKind::Code);
        }
    }

    #[test]
    fn test_binary_and_case_insensitive() {
        assert_eq!(format_kind("png"), FormatKind::Binary);
        assert_eq!(format_kind(""), FormatKind::Binary);
        assert_eq!(format_kind("exe"), FormatKind::Binary);
        // 大小写 + 前导点
        assert_eq!(format_kind(".DOCX"), FormatKind::Anydoc);
        assert_eq!(format_kind("Py"), FormatKind::Code);
    }

    #[test]
    fn test_code_language_mapping() {
        assert_eq!(code_language("py"), "python");
        assert_eq!(code_language("rs"), "rust");
        assert_eq!(code_language("tsx"), "typescript");
        assert_eq!(code_language("jsx"), "javascript");
        assert_eq!(code_language("yml"), "yaml");
        assert_eq!(code_language("zsh"), "bash");
        assert_eq!(code_language("txt"), "text");
        assert_eq!(code_language("unknown"), "text");
    }
}
```

- [x] **Step 2: lib.rs 注册模块**

`crates/convert/src/lib.rs` 的 `pub mod error;` 下加：

```rust
pub mod dispatch;
```

- [x] **Step 3: 跑测试验证通过**

```bash
cargo test -p octopus-convert --lib
```

Expected: 9 passed（Task 1 的 5 + 本 task 4）。

- [x] **Step 4: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): dispatch 格式分派矩阵（表驱动 TDD）"
```

---

### Task 3: html_to_markdown（htmd 包装）

**Files:**
- Modify: `crates/convert/src/lib.rs`

**Interfaces:**
- Produces: `pub fn html_to_markdown(html: &str) -> String`。剪贴板 HTML flavor（Task 7 desktop）与 .html 文件（Task 4）共用。

- [x] **Step 1: 写失败测试（lib.rs 底部加 tests mod）**

```rust
static HTML_CONVERTER: std::sync::OnceLock<htmd::HtmlToMarkdown> = std::sync::OnceLock::new();

/// HTML → Markdown（剪贴板 HTML flavor / .html 文件共用，spec §4.1）。
/// skip script/style（浏览器复制的脚本噪声）；失败回退原文（比空串更有用）。
pub fn html_to_markdown(html: &str) -> String {
    let converter = HTML_CONVERTER.get_or_init(|| {
        htmd::HtmlToMarkdown::builder()
            .skip_tags(vec!["script", "style"])
            .build()
    });
    converter.convert(html).unwrap_or_else(|_| html.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_markdown_basic() {
        let md = html_to_markdown("<h1>标题</h1><p>段落<b>加粗</b></p>");
        assert!(md.contains("# 标题"), "md={}", md);
        assert!(md.contains("**加粗**"), "md={}", md);
    }

    #[test]
    fn test_html_to_markdown_skips_script() {
        let md = html_to_markdown("<p>ok</p><script>var x = 1;</script>");
        assert!(md.contains("ok"));
        assert!(!md.contains("var x"), "script 应被剔除，md={}", md);
    }
}
```

- [x] **Step 2: 跑测试验证通过**

```bash
cargo test -p octopus-convert --lib
```

Expected: 11 passed。若 `htmd::HtmlToMarkdown` builder API 与上述不符（编译错），查 `docs.rs/htmd` 修正调用方式——测试断言不变。

- [x] **Step 3: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): html_to_markdown（htmd + skip script/style）"
```

---

### Task 4: assets fixture + convert_one 单文件转换核心

**Files:**
- Create: `crates/convert/assets/sample.csv`、`crates/convert/assets/sample.docx`（textutil 生成）
- Create: `crates/convert/src/convert.rs`
- Modify: `crates/convert/src/lib.rs`（加 `pub mod convert;` + `pub use convert::FileSection;`）

**Interfaces:**
- Consumes: `format_kind` / `code_language`（Task 2）、`html_to_markdown`（Task 3）、`ConvertError`（Task 1）、`anydoc::to_markdown(path) -> Result<String, anydoc::ConvertError>`（docs.rs 实测签名）
- Produces: `pub struct FileSection { pub rel_path: String, pub content: Result<String, ConvertError> }`、`pub(crate) fn convert_one(abs: &Path, rel: &str) -> FileSection`。Task 5 消费。

- [x] **Step 1: 生成测试 fixture**

```bash
cd crates/convert
mkdir -p assets
printf 'name,age\nAlice,30\nBob,25\n' > assets/sample.csv
printf 'Octopus convert sample markdown heading\n' > /tmp/octopus-docx-src.txt
textutil -convert docx /tmp/octopus-docx-src.txt -output assets/sample.docx
ls -la assets/
```

Expected: `sample.csv`（~22 字节）与 `sample.docx`（数 KB）存在。textutil 是 macOS 自带，无额外依赖。

- [x] **Step 2: 写失败的 convert_one 测试 + 实现**

`crates/convert/src/convert.rs`：

```rust
//! 单文件转换核心（spec §4.1）——唯一的转换单元，多文件/文件夹共用。

use crate::dispatch::{code_language, format_kind, FormatKind};
use crate::error::ConvertError;
use std::path::Path;

/// 单文件转换结果。content 用 Result 服务两种错误语义（spec §4.1）：
/// 单文件场景上抛；文件夹场景降级为 skipped 标注不中断（Task 5 merge）。
#[derive(Debug)]
pub struct FileSection {
    pub rel_path: String,
    pub content: Result<String, ConvertError>,
}

/// 唯一转换单元：abs=磁盘路径（定扩展名+读内容），rel=展示用相对路径。
pub(crate) fn convert_one(abs: &Path, rel: &str) -> FileSection {
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    FileSection { rel_path: rel.to_string(), content: convert_content(abs, &ext) }
}

fn convert_content(abs: &Path, ext: &str) -> Result<String, ConvertError> {
    match format_kind(ext) {
        FormatKind::Anydoc => anydoc::to_markdown(abs).map_err(|e| {
            // 扫描版（纯图片）PDF anydoc 无法本地转换——文案附走 OCR 提示（spec §6）
            if ext.eq_ignore_ascii_case("pdf") {
                ConvertError::Anydoc(format!("{}（扫描版 PDF 暂不支持，可截图走 OCR）", e))
            } else {
                ConvertError::Anydoc(e.to_string())
            }
        }),
        FormatKind::Html => {
            let html = std::fs::read_to_string(abs)?;
            Ok(crate::html_to_markdown(&html))
        }
        FormatKind::Md => Ok(std::fs::read_to_string(abs)?),
        FormatKind::Code => {
            let body = std::fs::read_to_string(abs)?;
            Ok(format!("```{}\n{}\n```", code_language(ext), body))
        }
        FormatKind::Binary => Err(ConvertError::UnsupportedFormat(ext.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：临时目录下写文件（名字即测试内唯一键）。
    fn tmp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-convert-test-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn asset(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join(name)
    }

    #[test]
    fn test_convert_one_md_passthrough() {
        let p = tmp_file("a.md", b"hello md");
        let s = convert_one(&p, "a.md");
        assert_eq!(s.content.unwrap(), "hello md");
    }

    #[test]
    fn test_convert_one_code_block_with_language() {
        let p = tmp_file("a.py", b"print(1)");
        let s = convert_one(&p, "a.py");
        assert_eq!(s.content.unwrap(), "```python\nprint(1)\n```");
    }

    #[test]
    fn test_convert_one_html_file() {
        let p = tmp_file("a.html", b"<h1>T</h1>");
        let s = convert_one(&p, "a.html");
        assert!(s.content.unwrap().contains("# T"));
    }

    #[test]
    fn test_convert_one_binary_unsupported() {
        let p = tmp_file("a.png", b"\x89PNG fake");
        let s = convert_one(&p, "a.png");
        let err = s.content.unwrap_err();
        assert_eq!(err.to_string(), "暂不支持 .png 格式");
    }

    #[test]
    fn test_convert_one_csv_via_anydoc() {
        let dir = tmp_file("sample.csv", b"placeholder"); // 借目录
        let p = dir.parent().unwrap().join("sample.csv");
        std::fs::copy(asset("sample.csv"), &p).unwrap();
        let s = convert_one(&p, "sample.csv");
        let md = s.content.unwrap();
        assert!(md.contains("Alice"), "csv 应转出表格内容，md={}", md);
    }

    #[test]
    fn test_convert_one_docx_via_anydoc() {
        let p = tmp_file("sample.docx", b"placeholder");
        std::fs::copy(asset("sample.docx"), &p).unwrap();
        let s = convert_one(&p, "sample.docx");
        let md = s.content.unwrap();
        assert!(
            md.to_lowercase().contains("octopus"),
            "docx 应转出源文本，md={}",
            md
        );
    }

    #[test]
    fn test_file_section_fields() {
        let p = tmp_file("b.rs", b"fn main() {}");
        let s = convert_one(&p, "sub/dir/b.rs");
        assert_eq!(s.rel_path, "sub/dir/b.rs");
        assert!(s.content.unwrap().contains("```rust"));
    }
}
```

注意：若 `anydoc::to_markdown` 的错误未实现 `Display`（`format!("{}", e)` 编译报错），改用 `format!("{:?}", e)`；测试断言不受影响。

- [x] **Step 3: lib.rs 注册 + 跑测试**

`crates/convert/src/lib.rs` 的 `pub mod dispatch;` 下加：

```rust
pub mod convert;

pub use convert::FileSection;
```

```bash
cargo test -p octopus-convert --lib
```

Expected: 18 passed（含 2 个 anydoc 真文件接线测试，毫秒级无需 ignore）。

- [x] **Step 4: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): convert_one 单文件转换核心 + anydoc/csv/docx fixture 接线测试"
```

---

### Task 5: folder.rs 多文件/文件夹合并

**Files:**
- Create: `crates/convert/src/folder.rs`
- Modify: `crates/convert/src/lib.rs`

**Interfaces:**
- Consumes: `convert_one` / `FileSection`（Task 4）、`ConvertError`（Task 1）
- Produces: `pub fn convert_files(paths: &[PathBuf]) -> Result<String, ConvertError>`、`pub fn convert_folder(root: &Path) -> Result<String, ConvertError>`、`pub const MAX_FILES: usize = 200`、`pub const MAX_TOTAL_BYTES: u64 = 50*1024*1024`。Task 7 desktop 消费。

- [x] **Step 1: 写失败的合并/守卫测试 + 实现**

`crates/convert/src/folder.rs`：

```rust
//! 多文件/文件夹合并（spec §4.1 复用设计 + §4.2 守卫 + §4.3 文档形态）。
//! convert_folder 与 convert_files 共用 convert_one + merge_sections——一条转换核心。

use crate::convert::{convert_one, FileSection};
use crate::error::ConvertError;
use std::path::{Path, PathBuf};

/// 文件夹递归守卫（spec §4.2）。
pub const MAX_FILES: usize = 200;
pub const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
/// 忽略目录（隐藏文件/目录另行按 `.` 前缀跳过）。
pub const IGNORED_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "__pycache__", ".venv", "dist", "build",
];

fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        name.starts_with('.') || IGNORED_DIRS.contains(&name.as_ref())
    } else {
        name.starts_with('.')
    }
}

/// 递归收集文件（排序确定：路径字典序）+ 数量/体积守卫。
pub(crate) fn collect_files(root: &Path) -> Result<Vec<PathBuf>, ConvertError> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_ignored(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // 单个 entry 读取失败（权限等）跳过不中断
        };
        if entry.file_type().is_file() {
            out.push(entry.into_path());
        }
    }
    out.sort();
    if out.len() > MAX_FILES {
        return Err(ConvertError::TooManyFiles { count: out.len(), max: MAX_FILES });
    }
    let total: u64 = out
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    if total > MAX_TOTAL_BYTES {
        return Err(ConvertError::TooLarge { bytes: total, max_bytes: MAX_TOTAL_BYTES });
    }
    Ok(out)
}

/// 文件树（缩进路径形式，确定性输出，spec §4.3）。
fn render_tree(root_name: &str, rel_paths: &[String]) -> String {
    let mut out = format!("{}/\n", root_name);
    for rel in rel_paths {
        let depth = rel.matches('/').count();
        let name = rel.rsplit('/').next().unwrap_or(rel);
        out.push_str(&"  ".repeat(depth + 1));
        out.push_str(name);
        out.push('\n');
    }
    out
}

/// 唯一合并单元（spec §4.1）：树头 + 逐节输出 + 失败节降级 skipped 标注。
fn merge_sections(root_name: &str, sections: &[FileSection]) -> String {
    let rel_paths: Vec<String> = sections.iter().map(|s| s.rel_path.clone()).collect();
    let mut out = format!(
        "# {}\n\n## 文件树\n\n```\n{}```\n",
        root_name,
        render_tree(root_name, &rel_paths)
    );
    for s in sections {
        out.push_str(&format!("\n## {}\n\n", s.rel_path));
        match &s.content {
            Ok(c) => {
                out.push_str(c);
                out.push('\n');
            }
            Err(e) => out.push_str(&format!("> ⚠️ skipped（{}）\n", e)),
        }
    }
    out
}

fn common_parent_name(paths: &[PathBuf]) -> Option<String> {
    let parent = paths.first()?.parent()?;
    parent.file_name().map(|n| n.to_string_lossy().to_string())
}

/// 多文件入口（spec §4.1）：单文件直接输出内容（无树头）；多文件合并（标题=公共父目录名）。
pub fn convert_files(paths: &[PathBuf]) -> Result<String, ConvertError> {
    if paths.is_empty() {
        return Err(ConvertError::Empty);
    }
    if paths.len() == 1 {
        let p = &paths[0];
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        return convert_one(p, &name).content;
    }
    let root_name = common_parent_name(paths).unwrap_or_else(|| "文件".to_string());
    let sections: Vec<FileSection> = paths
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            convert_one(p, &name)
        })
        .collect();
    Ok(merge_sections(&root_name, &sections))
}

/// 文件夹入口：递归收集 → 与 convert_files 共用 convert_one/merge_sections。
pub fn convert_folder(root: &Path) -> Result<String, ConvertError> {
    let files = collect_files(root)?;
    if files.is_empty() {
        return Err(ConvertError::Empty);
    }
    let root_name = root.file_name().unwrap_or_default().to_string_lossy().to_string();
    let sections: Vec<FileSection> = files
        .iter()
        .map(|p| {
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
            convert_one(p, &rel)
        })
        .collect();
    Ok(merge_sections(&root_name, &sections))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-convert-folder-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(rel_root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = rel_root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn test_convert_files_single_no_tree_header() {
        let root = tmp_root("single");
        let p = write(&root, "a.md", b"only one");
        let md = convert_files(&[p]).unwrap();
        assert_eq!(md, "only one");
        assert!(!md.contains("文件树"), "单文件不应有树头");
    }

    #[test]
    fn test_convert_files_multi_merge_with_tree() {
        let root = tmp_root("multi");
        let a = write(&root, "a.py", b"print(1)");
        let b = write(&root, "b.md", b"doc");
        let md = convert_files(&[a, b]).unwrap();
        assert!(md.starts_with(&format!("# {}\n", root.file_name().unwrap().to_string_lossy())));
        assert!(md.contains("## 文件树"));
        assert!(md.contains("## a.py"));
        assert!(md.contains("```python"));
        assert!(md.contains("## b.md"));
        assert!(md.contains("doc"));
    }

    #[test]
    fn test_convert_folder_recursive_sorted_and_ignored() {
        let root = tmp_root("recur");
        write(&root, "sub/deep/note.txt", b"deep");
        write(&root, "z_last.py", b"x=1");
        write(&root, "a_first.md", b"first");
        write(&root, ".hidden.md", b"no");
        write(&root, "node_modules/junk.js", b"no");
        write(&root, ".git/config", b"no");
        let md = convert_folder(&root).unwrap();
        // 隐藏/垃圾目录不出现
        assert!(!md.contains("hidden"), "隐藏文件应被排除");
        assert!(!md.contains("junk"), "node_modules 应被排除");
        assert!(!md.contains(".git"), ".git 应被排除");
        // 递归 + 排序确定（a_first.md < sub/... < z_last.py）
        let i_first = md.find("## a_first.md").unwrap();
        let i_deep = md.find("## sub/deep/note.txt").unwrap();
        let i_last = md.find("## z_last.py").unwrap();
        assert!(i_first < i_deep && i_deep < i_last);
        assert!(md.contains("## 文件树"));
    }

    #[test]
    fn test_convert_folder_binary_skipped_not_fatal() {
        let root = tmp_root("skipped");
        write(&root, "ok.md", b"fine");
        write(&root, "img.bin", b"\x00\x01");
        let md = convert_folder(&root).unwrap();
        assert!(md.contains("## ok.md"));
        assert!(md.contains("> ⚠️ skipped"), "二进制应标注 skipped，md={}", md);
    }

    #[test]
    fn test_collect_files_max_files_guard() {
        let root = tmp_root("toomany");
        for i in 0..=MAX_FILES {
            write(&root, &format!("f{:03}.txt", i), b"x");
        }
        let err = convert_folder(&root).unwrap_err();
        assert!(err.to_string().contains(&format!("{} 个文件超出上限", MAX_FILES + 1)));
    }

    #[test]
    fn test_convert_empty_inputs() {
        assert!(matches!(
            convert_files(&[]).unwrap_err(),
            ConvertError::Empty
        ));
        let empty_root = tmp_root("emptydir");
        assert!(matches!(
            convert_folder(&empty_root).unwrap_err(),
            ConvertError::Empty
        ));
    }
}
```

- [x] **Step 2: lib.rs 注册 + 跑测试**

`crates/convert/src/lib.rs` 的 `pub use convert::FileSection;` 下加：

```rust
pub mod folder;

pub use folder::{convert_files, convert_folder, MAX_FILES, MAX_TOTAL_BYTES};
```

```bash
cargo test -p octopus-convert --lib
```

Expected: 24 passed。

- [x] **Step 3: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): 文件夹递归 + 多文件合并（树头/skipped 标注/上限守卫）"
```

---

### Task 6: 剪贴板 HTML flavor 采集 + ActionBarContext.html

**Files:**
- Modify: `crates/clipboard/src/handle.rs`（`read_files` 后加 `read_html`）
- Modify: `crates/desktop/src/action_bar/action_bar_commands/context.rs`（Selection::Text + ActionBarContext + detect_selection + serde 测试）
- Modify: `crates/desktop/src/action_bar/action_bar_commands/window.rs:49-57`（Text 分支透传 html）
- Modify: `crates/desktop/src/action_bar/action_hotkey.rs:104-105,123`（Text 分支透传 html）
- Modify: `crates/clipboard/Cargo.toml`（如 clipboard_rs feature 需要开启——先编译验证，默认无需改）

**Interfaces:**
- Produces: `ClipboardHandle::read_html(&self) -> anyhow::Result<String>`（clipboard_rs `get_html` 的薄包装）、`ActionBarContext.html: Option<String>`（serde camelCase）、`Selection::Text { text, html, mouse }`、`ActionBarContext::with_html(self, Option<String>) -> Self`。Task 7/9 消费。

**说明**：NSPasteboard HTML flavor 读取是 macOS 胶水（spec §7 唯一不可单测处），本 task 以编译验证 + serde 单测为主。

- [x] **Step 1: clipboard read_html**

`crates/clipboard/src/handle.rs` 的 `read_files` 方法后加：

```rust
    /// 读 HTML flavor（macOS public.html）——浏览器/WKWebView app 复制时提供；
    /// 无 HTML flavor 返回 Err（调用方 .ok() → None）。ActionBar 转 Markdown 用（spec §5.1）。
    pub fn read_html(&self) -> Result<String> {
        let ctx = self.ctx.lock();
        ctx.get_html()
            .map_err(|e| anyhow::anyhow!("Clipboard read html failed: {}", e))
    }
```

- [x] **Step 2: context.rs 三处修改**

1. `Selection::Text` 加 html 字体（`context.rs:83-86`）：

```rust
    /// 选中文本。html=Cmd+C 同时写入 pasteboard 的 HTML flavor（浏览器才有，
    /// 是「选中网页文本保留富文本结构」的数据源，spec §5.1）；无则 None。
    Text {
        text: String,
        html: Option<String>,
        mouse: (f64, f64),
    },
```

2. `ActionBarContext`（`context.rs:31-48`）加字段 + builder（构造器默认 None，不破坏现有调用）：

```rust
pub struct ActionBarContext {
    pub kind: ContextKind,
    pub text: Option<String>,
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::platform::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::platform::app_context::SurroundingText>,
}

impl ActionBarContext {
    pub fn text(text: String) -> Self {
        Self { kind: ContextKind::Text, text: Some(text), files: vec![], html: None, source: None, surrounding: None }
    }
    pub fn files(files: Vec<String>) -> Self {
        Self { kind: ContextKind::Files, text: None, files, html: None, source: None, surrounding: None }
    }
    /// 附带 HTML flavor（detect_selection 读到后调用）。
    pub fn with_html(mut self, html: Option<String>) -> Self {
        self.html = html;
        self
    }
}
```

3. `detect_selection`（`context.rs`）两处：
   - Cmd+C 分支 `let clipboard_after = read_clipboard_text(app);`（:247）之后、恢复剪贴板之前加：

```rust
    // HTML flavor 同窗口读取（恢复剪贴板前——restore 会覆盖 pasteboard）：
    // 浏览器 Cmd+C 同时写 text + HTML 两种 flavor（spec §5.1）。
    let html_after = clip_handle
        .read_html()
        .ok()
        .filter(|h| !h.trim().is_empty());
```

   - 末尾 `Selection::Text { text, mouse }`（:281）改 `Selection::Text { text, html: html_after, mouse }`；Sublime 分支 `Selection::Text { text, mouse }`（:178）改 `Selection::Text { text, html: None, mouse }`。

- [x] **Step 3: 消费点透传（影响面追踪：rg "Selection::Text" 全部构造/匹配点）**

`window.rs:49`（trigger_action_bar Text 分支）：

```rust
            crate::action_bar::action_bar_commands::Selection::Text { text, html, mouse } => {
```

`:57` 行改：

```rust
                let mut ctx = ActionBarContext::text(text.clone()).with_html(html.clone());
```

`action_hotkey.rs:104-105`：

```rust
        crate::action_bar::action_bar_commands::Selection::Text { text, html, .. } => {
            handle_text_selection(item_id, app, text, html);
        }
```

`action_hotkey.rs` 的 `handle_text_selection`（:123）签名 + :133 构造：

```rust
fn handle_text_selection(item_id: i64, app: &AppHandle, text: String, html: Option<String>) {
```

```rust
    let mut ctx = crate::action_bar::action_bar_commands::ActionBarContext::text(text.clone())
        .with_html(html);
```

- [x] **Step 4: serde casing 回归测试（context.rs tests mod 内加）**

```rust
    #[test]
    fn test_context_html_serialization_camel_case() {
        let ctx = crate::action_bar::action_bar_commands::ActionBarContext::text("hi".into())
            .with_html(Some("<h1>x</h1>".into()));
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"html\":\"<h1>x</h1>\""), "json={}", json);

        let ctx_no_html = crate::action_bar::action_bar_commands::ActionBarContext::text("hi".into());
        let json2 = serde_json::to_string(&ctx_no_html).unwrap();
        assert!(!json2.contains("\"html\""), "None 时省略，json={}", json2);
    }
```

- [x] **Step 5: 编译 + 测试（0 error 0 warning）**

```bash
cargo build -p octopus-clipboard -p octopus-desktop 2>&1 | tail -5
cargo test -p octopus-desktop --lib action_bar 2>&1 | tail -5
```

Expected: 编译 0 warning；action_bar 相关测试全过。若有遗漏的 `Selection::Text` 匹配点（编译器逐个报出），按同样模式补 `html` 字段。

- [x] **Step 6: Commit**

```bash
git add crates/clipboard crates/desktop
git commit -m "feat(action-bar): 选区采集 HTML flavor（read_html + ActionBarContext.html）"
```

---

### Task 7: desktop markdown 分派 + execute_action_bar 集成

**Files:**
- Create: `crates/desktop/src/action_bar/action_bar_commands/markdown.rs`
- Modify: `crates/desktop/src/action_bar/action_bar_commands/mod.rs`（`pub mod markdown;`）
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs:359,364-366,368,590-591`（inner 签名 + markdown 分支 + 命令参数）
- Modify: `crates/desktop/src/action_bar/action_hotkey.rs:177,258`（两处 inner 调用补参数）
- Modify: `crates/desktop/Cargo.toml`（加 octopus-convert 依赖）

**Interfaces:**
- Consumes: `octopus_convert::{convert_files, convert_folder, html_to_markdown}`（Task 5/3）、`PENDING_CONTEXT`（含 Task 6 的 html）
- Produces: `pub(crate) fn run_markdown_convert(files: Vec<String>, html: Option<String>, text: String) -> Result<String, String>`；`execute_action_bar` 命令新参数 `html: Option<String>, files: Option<Vec<String>>`（Task 9 前端 invoke 对应 camelCase 直传）

- [x] **Step 1: desktop 加依赖**

`crates/desktop/Cargo.toml` 的 `octopus-clipboard = { path = "../clipboard" }` 附近加：

```toml
octopus-convert = { path = "../convert" }
```

- [x] **Step 2: 写失败的 run_markdown_convert 测试 + 实现**

`crates/desktop/src/action_bar/action_bar_commands/markdown.rs`：

```rust
//! 「转 Markdown」命令（action_type = "markdown"）的纯分派逻辑（spec §3/§5.2）。
//! execute_action_bar_inner 的 markdown 分支是薄包装，本模块可单测。

/// 输入分派（优先级 spec §3）：files > html > text；全空 Err。
/// 混合文件夹+文件：文件夹逐个 convert_folder，与文件结果以 `---` 分隔拼接。
pub(crate) fn run_markdown_convert(
    files: Vec<String>,
    html: Option<String>,
    text: String,
) -> Result<String, String> {
    if !files.is_empty() {
        let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
        let mut parts: Vec<String> = Vec::new();
        for d in paths.iter().filter(|p| p.is_dir()) {
            parts.push(octopus_convert::convert_folder(d).map_err(|e| e.to_string())?);
        }
        let plain: Vec<std::path::PathBuf> =
            paths.iter().filter(|p| p.is_file()).cloned().collect();
        if !plain.is_empty() {
            parts.push(octopus_convert::convert_files(&plain).map_err(|e| e.to_string())?);
        }
        if parts.is_empty() {
            return Err("没有可转换的内容".to_string()); // 选中路径在执行前被删光
        }
        return Ok(parts.join("\n\n---\n\n"));
    }
    if let Some(h) = html.filter(|h| !h.trim().is_empty()) {
        return Ok(octopus_convert::html_to_markdown(&h));
    }
    if !text.trim().is_empty() {
        return Ok(text); // 纯文本直通（spec §6）
    }
    Err("没有可转换的内容".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str, bytes: &[u8]) -> String {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-cmd-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn test_text_passthrough() {
        assert_eq!(
            run_markdown_convert(vec![], None, "纯文本".into()).unwrap(),
            "纯文本"
        );
    }

    #[test]
    fn test_html_priority_over_text() {
        let md = run_markdown_convert(vec![], Some("<h1>H</h1>".into()), "plain".into()).unwrap();
        assert!(md.contains("# H"), "html 优先于纯文本，md={}", md);
    }

    #[test]
    fn test_empty_all_inputs_err() {
        let err = run_markdown_convert(vec![], None, "  ".into()).unwrap_err();
        assert_eq!(err, "没有可转换的内容");
    }

    #[test]
    fn test_single_file_no_tree() {
        let p = tmp_file("solo.md", b"solo content");
        let md = run_markdown_convert(vec![p], None, String::new()).unwrap();
        assert_eq!(md, "solo content");
    }

    #[test]
    fn test_binary_file_err_message() {
        let p = tmp_file("img.png", b"\x89PNG");
        let err = run_markdown_convert(vec![p], None, String::new()).unwrap_err();
        assert!(err.contains("暂不支持 .png"));
    }

    #[test]
    fn test_folder_merge_contains_tree() {
        let dir = std::env::temp_dir()
            .join(format!("octopus-md-cmd-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), b"aaa").unwrap();
        std::fs::write(dir.join("b.py"), b"x=1").unwrap();
        let md = run_markdown_convert(vec![dir.to_string_lossy().to_string()], None, String::new())
            .unwrap();
        assert!(md.contains("## 文件树"));
        assert!(md.contains("## a.md"));
        assert!(md.contains("```python"));
    }

    #[test]
    fn test_files_priority_over_html_and_text() {
        let p = tmp_file("prio.md", b"from file");
        let md = run_markdown_convert(
            vec![p],
            Some("<h1>from html</h1>".into()),
            "from text".into(),
        )
        .unwrap();
        assert_eq!(md, "from file");
    }
}
```

- [x] **Step 3: mod.rs 注册**

`crates/desktop/src/action_bar/action_bar_commands/mod.rs` 子模块声明区加：

```rust
pub mod markdown;
```

- [x] **Step 4: execute_action_bar_inner 签名 + markdown 分支（script.rs）**

签名（`:359`）与 PENDING 读取（`:364-366`）改为：

```rust
pub(crate) async fn execute_action_bar_inner(
    item_id: i64,
    text: String,
    html: Option<String>,
    files: Option<Vec<String>>,
    app: &AppHandle,
) -> Result<bool, String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(e2s)?
        .ok_or("菜单项不存在")?;

    // 从 PENDING_CONTEXT 取 files + html（Quick Execute 路径数据源，spec §3 优先级）
    let (app_state_files, pending_html) = {
        let guard = PENDING_CONTEXT.lock();
        guard
            .as_ref()
            .map(|c| (c.files.clone(), c.html.clone()))
            .unwrap_or_default()
    };
```

match（`:368`）的 `"copy_path" =>` 分支前加：

```rust
        "markdown" => {
            // 输入优先级（spec §3）：显式 files > PENDING files > html（显式 > PENDING）> text
            let files_in = files.filter(|f| !f.is_empty()).unwrap_or(app_state_files);
            let html_in = html.or(pending_html);
            let text_in = text.clone();
            let title = item.title.clone();
            let write_clipboard = item.write_output_to_clipboard;
            // 本地转换毫秒级但仍是同步 IO——spawn_blocking 防卡 runtime（对齐 ai 分支）
            let result = tokio::task::spawn_blocking(move || {
                crate::action_bar::action_bar_commands::markdown::run_markdown_convert(
                    files_in, html_in, text_in,
                )
            })
            .await
            .map_err(|e| format!("转换线程异常: {}", e))??;
            // 剪贴板写入由 show_result 内部统一处理（对齐 ai 分支模式，spec §5.2）
            action_bar_show_result(result, String::new(), title, app.clone(), write_clipboard);
            Ok(true)
        }
```

命令（`:590-591`）：

```rust
#[tauri::command]
pub async fn execute_action_bar(
    item_id: i64,
    text: String,
    html: Option<String>,
    files: Option<Vec<String>>,
    app: AppHandle,
) -> Result<(), String> {
    match execute_action_bar_inner(item_id, text, html, files, &app).await {
```

（其余 match 体不变。）

- [x] **Step 5: action_hotkey.rs 两处调用补参数（编译器会逐个报出）**

`:177`：

```rust
    let result = tauri::async_runtime::block_on(
        crate::action_bar::action_bar_commands::execute_action_bar_inner(
            item_id, text, None, None, &app_clone,
        ),
    );
```

`:258` 附近（handle_files_selection 内，files 经 PENDING_CONTEXT 传入——inner 回退读 pending）：

```rust
            crate::action_bar::action_bar_commands::execute_action_bar_inner(
                item_id, text, None, None, &app,
            )
```

（以实际代码上下文对齐换行；参数多出的 `None, None` 是关键。）

- [x] **Step 6: 编译 + 测试**

```bash
cargo build -p octopus-desktop 2>&1 | tail -3
cargo test -p octopus-desktop --lib markdown 2>&1 | tail -5
```

Expected: 0 warning；`run_markdown_convert` 8 个测试全过。

- [x] **Step 7: Commit**

```bash
git add crates/desktop Cargo.lock
git commit -m "feat(action-bar): markdown 命令分派 + execute_action_bar html/files 参数"
```

---

### Task 8: seed 菜单项 + schema v61

**Files:**
- Modify: `crates/infra/resources/sql/schema.sql`（主菜单 seed 区，`:545` 后）
- Modify: `crates/infra/src/db/mod.rs:508`（CURRENT_SCHEMA_VERSION 61）+ 迁移链 `60 =>` 分支 + tests

**Interfaces:**
- Produces: 系统菜单项 id=12「转 Markdown」（`action_type='markdown'`、`accepts='any'`、`write_output_to_clipboard=1`、icon=`file-code`）、`CURRENT_SCHEMA_VERSION = 61`。

- [x] **Step 1: 写失败的迁移测试（db/mod.rs tests mod，照抄 migrate_v59_to_v60 测试模式）**

```rust
    /// v60→v61 迁移：seed「转 Markdown」系统菜单项（spec 2026-08-18）。
    #[test]
    fn migrate_v60_to_v61_seeds_markdown_item() {
        let conn = open_with_version(60, "true");
        init_schema(&conn).expect("v60→v61 迁移应成功（纯 seed，无破坏性）");

        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 61);

        let (action_type, accepts, clipboard): (String, String, i64) = conn
            .query_row(
                "SELECT action_type, accepts, write_output_to_clipboard \
                 FROM action_bar_items WHERE id = 12",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(action_type, "markdown");
        assert_eq!(accepts, "any");
        assert_eq!(clipboard, 1);

        // 幂等：再跑一次不报错不重复
        init_schema(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM action_bar_items WHERE id = 12",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
```

- [x] **Step 2: 实现迁移 + seed**

`crates/infra/src/db/mod.rs`：
1. `:508` `pub const CURRENT_SCHEMA_VERSION: u32 = 60;` → `61`
2. 迁移链 `59 => {...}` 分支后加：

```rust
            60 => {
                // v60→v61：ActionBar 新增「转 Markdown」系统菜单项（spec 2026-08-18）
                conn.execute_batch(
                    "INSERT OR IGNORE INTO action_bar_items
                        (id, parent_id, title, icon, action_type, action_data,
                         sort_order, is_system, accepts, write_output_to_clipboard)
                     VALUES
                        (12, NULL, '转 Markdown', 'file-code', 'markdown', '', 4, 1, 'any', 1);",
                )
                .context("迁移 v60→v61：seed 转 Markdown 菜单项")?;
                log::info!("DB migrated v60→v61: seed 转 Markdown 菜单项");
            }
```

`crates/infra/resources/sql/schema.sql` 主菜单 INSERT 区（`:545` Github 行所在的搜索子菜单语句之后）加同样一条 INSERT：

```sql
-- 转 Markdown 命令（v61）——accepts=any（文本/文件/文件夹/无选中都可用），
-- 结果默认同时写剪贴板（喂 AI/存笔记场景，spec §5.3）
INSERT OR IGNORE INTO action_bar_items
    (id, parent_id, title, icon, action_type, action_data,
     sort_order, is_system, accepts, write_output_to_clipboard)
VALUES
    (12, NULL, '转 Markdown', 'file-code', 'markdown', '', 4, 1, 'any', 1);
```

- [x] **Step 3: 跑测试**

```bash
cargo test -p octopus-infra --lib migrate_v60_to_v61 2>&1 | tail -3
```

Expected: 1 passed。

- [x] **Step 4: Commit**

```bash
git add crates/infra
git commit -m "feat(infra): schema v61 seed 转 Markdown 系统菜单项"
```

---

### Task 9: 前端集成（types/constants/i18n/图标/传参）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/types.ts:19-25`（Context 加 html）
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx:531,541` 及斜杠路径 invoke（~:630）（透传 html/files）
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBar/constants.tsx:8-37`（TYPE_META/ACTION_TYPES/deriveAccepts）
- Modify: `crates/desktop/frontend/src/components/ActionBarIcon.tsx:8-12`（LUCIDE_PATHS 加 file-code）
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml:742` 区 + `en.yaml:735` 区（i18n）
- Create: `crates/desktop/frontend/src/pages/Settings/ActionBar/constants.test.ts`

**Interfaces:**
- Consumes: Task 7 的命令参数（camelCase 直传：`{ itemId, text, html, files }`）
- Produces: `Context.html?: string | null`、`TYPE_META.markdown`、`deriveAccepts("markdown") === "any"`

- [x] **Step 1: 写失败的 deriveAccepts 测试**

`crates/desktop/frontend/src/pages/Settings/ActionBar/constants.test.ts`：

```ts
import { describe, expect, it } from "vitest";
import { ACTION_TYPES, TYPE_META, deriveAccepts } from "./constants";

describe("markdown action type", () => {
  it("markdown 在 ACTION_TYPES 中", () => {
    expect(ACTION_TYPES.some((t) => t.value === "markdown")).toBe(true);
  });

  it("markdown 有 TYPE_META", () => {
    expect(TYPE_META.markdown).toBeDefined();
    expect(TYPE_META.markdown.label).toBe("MD");
  });

  it("deriveAccepts: markdown → any，explicit 优先", () => {
    expect(deriveAccepts("markdown")).toBe("any");
    expect(deriveAccepts("markdown", "text")).toBe("text");
    expect(deriveAccepts(undefined)).toBe("text");
  });
});
```

- [x] **Step 2: 跑测试验证失败**

```bash
cd crates/desktop/frontend && npx vitest run src/pages/Settings/ActionBar/constants.test.ts
```

Expected: FAIL（markdown 未定义）。

- [x] **Step 3: 实现 constants + i18n + 图标 + types + 传参**

`constants.tsx`——`TYPE_META` 的 `copy_path` 行后加：

```tsx
  markdown:   { bar: "bg-teal-500",        dot: "bg-teal-500",     label: "MD",      descKey: "settings.actionBar.typeMarkdownDesc",  placeholderKey: "" },
```

`ACTION_TYPES` 的 `copy_path` 行后加：

```tsx
  { value: "markdown",   labelKey: "settings.actionBar.typeMarkdown" },
```

`deriveAccepts` 的 `if (actionType === "submenu") return "any";` 后加：

```ts
  if (actionType === "markdown") return "any";
```

`ActionBarIcon.tsx` 的 `LUCIDE_PATHS` 加（lucide file-code v1.31.0 实测 path）：

```tsx
  "file-code": '<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M10 12.5 8 15l2 2.5"/><path d="m14 12.5 2 2.5-2 2.5"/>',
```

`types.ts` 的 `Context` 接口 `files: string[];` 后加：

```ts
  /** Cmd+C 同窗口读到的 HTML flavor（浏览器复制才有）；与后端 ActionBarContext.html camelCase 对应 */
  html?: string | null;
```

`index.tsx`——三处 invoke 补参数。`executeItem` 内 agent 分支（:531）与通用分支（:541）的 invoke 改为：

```ts
        await invoke("execute_action_bar", {
          itemId: item.id,
          text,
          html: ctx?.html ?? null,
          files: ctx?.files?.length ? ctx.files : null,
        });
```

斜杠路径的同一命令 invoke（`rg -n 'invoke\("execute_action_bar"' index.tsx` 找全，含 :630 附近）同样补 `html` / `files` 两参数（`ctx` 取 `contextRef.current`）。

i18n——`zh-CN.yaml` 的 `typeCopyPathDesc` 行后加：

```yaml
    typeMarkdown: 转 Markdown
    typeMarkdownDesc: 把选中的网页富文本/文件/文件夹转换为 Markdown，结果展示在浮窗并写入剪贴板
```

`en.yaml` 的 `typeCopyPathDesc` 行后加：

```yaml
    typeMarkdown: To Markdown
    typeMarkdownDesc: Convert selected rich text / files / folders to Markdown, shown in a floating window and copied to clipboard
```

- [x] **Step 4: 跑测试 + 类型检查 + 构建**

```bash
cd crates/desktop/frontend
npx vitest run src/pages/Settings/ActionBar/constants.test.ts
npx tsc --noEmit
npm run build
```

Expected: 测试 PASS；tsc 0 error；build 成功。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend
git commit -m "feat(action-bar): 前端 markdown 命令类型/图标/i18n + execute 传 html/files"
```

---

### Task 10: 全量验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`（crate 树 + 依赖图）
- Modify: `docs/features/desktop-app.md` §12/§14（命令说明）
- Modify: `AGENTS.md`（Cargo Workspace 结构列表 + 依赖关系）
- Modify: `docs/superpowers/specs/2026-08-18-actionbar-markdown-conversion-design.md`（实施偏差回写，如有）

- [x] **Step 1: 全量编译 + 测试（核心层）**

```bash
cargo build 2>&1 | tail -3          # default-members 全部（含 octopus-convert）
cargo test 2>&1 | tail -5           # 核心测试层
```

Expected: 0 error 0 warning；全部测试通过（含 octopus-convert 24 个 + desktop markdown 8 个 + infra 迁移 1 个 + 前端已单验）。

- [ ] **Step 2: 手动 e2e 冒烟（可选但推荐）**

```bash
./run-octopus.sh
```

验证清单：
1. Alt+D 召唤 ActionBar，主菜单出现「转 Markdown」
2. 浏览器选中一段带粗体/链接的文字 → Alt+D → 转 Markdown → CompactEditor 出现保留格式的 md + 剪贴板可粘贴
3. Finder 选一个 .docx → Alt+D → 转 Markdown → 内容正确
4. Finder 选一个含 .md/.py 的文件夹 → 转 Markdown → 文件树 + 各节内容
5. 设置页 ActionBar 列表看到「转 Markdown」系统项（禁删）

- [x] **Step 3: 文档同步**

`docs/architecture.md`：
- crate 列表（与 AGENTS.md 同步的位置）加：`├── convert/       # octopus-convert — 文档转 Markdown（anydoc/htmd，ActionBar markdown 命令）`
- 依赖关系图加：`convert ← (desktop)  — ActionBar 转 Markdown 命令`

`AGENTS.md` 的 Cargo Workspace 结构：`clipboard/` 行后加：

```
├── convert/      # octopus-convert — 文档转 Markdown（anydoc 14 格式 + htmd HTML→md + 文件夹合并）
```

依赖关系段加一行 `convert ← (desktop)`。

`docs/features/desktop-app.md` §14（命令面板菜单）加「转 Markdown」条目：action_type=markdown、accepts=any、输入优先级 files > html > text、结果 CompactEditor + 剪贴板、格式矩阵摘要、文件夹守卫 200 文件/50MB。

spec 回写：实现与 spec 的任何偏差（如 anydoc API 细节、htmd 配置）回写到 spec 的「实施注记」段。

- [x] **Step 4: Commit**

```bash
git add docs AGENTS.md
git commit -m "docs: 同步转 Markdown 命令（architecture/desktop-app/AGENTS）"
```

---

## Self-Review 记录

- **Spec coverage**：§3 数据流/优先级 → Task 6/7；§3.1 格式矩阵 → Task 2；§4 crate 结构/复用/守卫/文档形态 → Task 1/4/5；§5.1 采集 → Task 6；§5.2 执行链路 → Task 7；§5.3 seed → Task 8 + Task 9；§6 错误处理 → Task 1（文案）+ Task 4（PDF 提示）+ Task 5（skipped/上限）+ Task 7（空输入）；§7 测试计划 → 各 task TDD 步骤；§8 文档 → Task 10。无缺口。
- **占位符**：全部步骤含完整代码/命令，无 TBD。
- **类型一致性**：`ConvertError` 变体、`FileSection`、`convert_one(abs, rel)`、`convert_files(&[PathBuf])`、`convert_folder(&Path)`、`html_to_markdown(&str)`、`run_markdown_convert(files, html, text)`、`read_html()`、`with_html()`、`Selection::Text { text, html, mouse }` 跨 task 一致。
- **已知实现期风险**（实现者注意，不阻塞）：① anydoc/htmd 错误类型若未实现 Display，用 `{:?}` 替代（Task 4 注记）；② `open_with_version` helper 若签名不同，照抄 `migrate_v59_to_v60` 测试的实际调用方式；③ worktree 前端跑 `npm install` 勿软链主干 node_modules（AGENTS.md Gotcha）。

---

## 实施记录（plan-as-record，2026-08-18 回写）

**执行方式**：executing-plans inline（worktree `.worktree/markdown-conversion`，分支 `markdown-conversion`）。

**Task → commit 映射**：

| Task | Commit | 验证 |
|---|---|---|
| 1 骨架 + ConvertError | `afbc916b` | 5 测试过 |
| 2 dispatch | `47a8e6b4` | 9 过 |
| 3 html_to_markdown | `a53e453e` | 11 过 |
| 4 convert_one + fixtures | `8f3318b8` | 18 过（anydoc csv/docx 接线双绿） |
| 5 folder 合并 | `4a1f3fcc` | 24 过 |
| 6 HTML flavor 采集 | `c68f7350` | 81 action_bar 测试 + serde 测试过 |
| 7 markdown 分支 + 命令参数 | `8b898eb6` | 7 过（plan 预期「8 个」系笔误） |
| 8 schema v61 | `046b3a77` | 红灯→绿灯；infra 195 过（2 个既有测试预期随 v61 演进更新） |
| 9 前端 | `c3f1fa2c` | 3 新测试 + tsc 0 + build ✓ |
| 10 文档 | `5fc5b0f8` | 全量 cargo test 0 failed |

**交付后修订**（用户反馈，spec §9.1）：异步执行 + 落盘 `~/Documents/octopus/markitdown/` + CompactEditor file tab——commit `cf6e8b04`（markitdown_dir / open_disk_file_in_compact_editor 抽取 / run_markdown_convert 落盘 / tokio::spawn 异步）。

**性能修复**（spec §9.2，z_perf）：commit `a2f1651d`——预览 256KB 行边界截断（2MB 预览 212ms→22ms）+ CM6 每键 O(N) 回声快路径。

**遗留**：Task 10 Step 2 手动 e2e 冒烟未执行（用户侧验证项）；全部偏差详见 spec §9 实施注记。
