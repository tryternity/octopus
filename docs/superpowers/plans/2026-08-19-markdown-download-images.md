# Markdown 图片下载到同名目录 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 「转 Markdown（下载图片）」替换 base64 内嵌——图片下载到 md 同名目录、md 内相对路径引用、CompactEditor 经 asset protocol 预览。

**Architecture:** convert 层删 embed pass 换 `download_images_with(md, dir, download)`（落盘 + 相对路径替换，文件名纯函数可测）；desktop `apply_download_images(md, dir)` outermost 后处理；v62 seed 原地改（未发布）；预览链路 assetProtocol + MarkdownPreview DOM 层 convertFileSrc。

**Tech Stack:** 既有 reqwest/regex；删除 base64 依赖；`@tauri-apps/api/core::convertFileSrc`。

**Spec:** `docs/superpowers/specs/2026-08-19-markdown-download-images-design.md`（§0 替换范围：保留 extract_image_links/download_image/mime_from_ext/unescape_md_url，删 embed_images/embed_images_with/base64）

## Global Constraints

- **开发隔离**：`.worktree/markdown-conversion` 分支，未经明确指令不进 main。
- **TDD**：Task 1/2/4 测试先行；生产下载绑定与 asset 预览编译级 + 手动 e2e。
- **签名变更面**：`convert_and_save` 参数 `embed`→`download`（改名非新增）；`MarkdownPane`/`MarkdownPreview` 各 +1 可选 prop——其余既有函数与测试不动。
- **守卫改名**：`EMBED_*` → `DOWNLOAD_*`（值不变：20 张 / 5MB / 30MB / 10s）。
- **v62 原地改**：不升 v63——迁移臂/schema.sql/测试三处同步改 id=13 字段。
- **md 源零污染**：预览的 convertFileSrc 只在 DOM 层，保存写回相对路径原样。
- 0 warning；casing；删 base64 后 Cargo.toml/Cargo.lock 同步。

---

### Task 1: convert 层——删 embed 换 download（TDD）

**Files:**
- Modify: `crates/convert/Cargo.toml`（删 `base64`）
- Modify: `crates/convert/src/web.rs`

**Interfaces:**
- Keeps: `extract_image_links` / `download_image` / `mime_from_ext` / `unescape_md_url`
- Deletes: `embed_images` / `embed_images_with` + 其 8 个测试 + `EMBED_*` 常量名
- Produces: `pub const DOWNLOAD_MAX_IMAGES/DOWNLOAD_MAX_IMAGE_BYTES/DOWNLOAD_MAX_TOTAL_BYTES/DOWNLOAD_TIMEOUT_SECS`、`pub fn image_filename(url: &str, mime: &str, existing: &std::collections::HashSet<String>) -> String`、`pub fn download_images_with(md, dir, download) -> (String, usize, usize)`、`pub fn download_images(md, dir) -> (String, usize, usize)`

- [ ] **Step 1: 写失败测试（替换 embed 的 8 个测试位）**

```rust
    // ── 图片下载到同名目录（spec 2026-08-19-markdown-download-images，替换 base64 内嵌）──

    #[test]
    fn test_image_filename_rules() {
        let mut used = std::collections::HashSet::new();
        // 基本形态：末段去 query + unescape + sanitize（白名单同 sanitize_stem）
        assert_eq!(image_filename("https://a.com/x/cover.png?w=100", "image/png", &used), "cover.png");
        assert_eq!(image_filename("https://a.com/x/Foo_\\(bar\\).png", "image/png", &used), "Foo_(bar).png");
        // 无扩展名 → 按 MIME 补
        assert_eq!(image_filename("https://a.com/x/photo", "image/jpeg", &used), "photo.jpg");
        // 冲突 -N
        used.insert("cover.png".into());
        assert_eq!(image_filename("https://a.com/x/cover.png?w=2", "image/png", &used), "cover-1.png");
        // 未知 MIME 且无扩展 → img.bin 兜底（保守可显示性差但可用）
        assert_eq!(image_filename("https://a.com/x/file", "application/octet-stream", &used), "file.bin");
        // URL 末段为空（尾斜杠）→ image-N 兜底（existing 计数防重复）
        assert_eq!(image_filename("https://a.com/x/", "image/png", &used), "image-1.png");
    }

    #[test]
    fn test_download_images_with_success() {
        let dir = std::env::temp_dir().join(format!("octopus-dl-img-{}-a", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let md = "# t\n\n![cover](https://a.com/x/cover.png)\n";
        let (out, n, total) = download_images_with(md, &dir, |_u| Ok(("image/png".into(), vec![1u8, 2])));
        assert_eq!((n, total), (1, 1));
        assert!(out.contains("![cover](cover.png)"), "相对路径引用（md 同目录），out={}", out);
        assert!(!out.contains("https://a.com"));
        // 目录与 md 同名逻辑：pass 只写图片文件到 dir 本身？——不：spec §2 图片落 <stem>_<ts>/ 子目录由 desktop 层拼；
        // convert 层 download_images_with 的 dir 即图片目录（desktop 传入 md 同名目录）。此处直接断言 dir 下文件
        let written = std::fs::read(dir.join("cover.png")).unwrap();
        assert_eq!(written, vec![1u8, 2]);
    }

    #[test]
    fn test_download_images_with_failure_keeps_link() {
        let dir = std::env::temp_dir().join(format!("octopus-dl-img-{}-b", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        let md = "![a](https://a.com/x.png) ![b](https://b.com/y.png)";
        let (out, n, total) = download_images_with(md, &dir, |u| {
            if u.contains("a.com") { Err("timeout".into()) } else { Ok(("image/png".into(), vec![9u8])) }
        });
        assert_eq!((n, total), (1, 2));
        assert!(out.contains("![a](https://a.com/x.png)"));
        assert!(out.contains("![b](y.png)"));
    }

    #[test]
    fn test_download_images_with_guards() {
        let dir = std::env::temp_dir().join(format!("octopus-dl-img-{}-c", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); std::fs::create_dir_all(&dir).unwrap();
        // 单图帽：超 5MB 保留链接
        let md = "![big](https://a.com/x.png)";
        let (out, n, _) = download_images_with(md, &dir, |_u| Ok(("image/png".into(), vec![0u8; DOWNLOAD_MAX_IMAGE_BYTES + 1])));
        assert_eq!(n, 0);
        assert!(out.contains("https://a.com/x.png"));
        // 数量帽：21+ 张第 21 张起保留
        let md2: String = (0..22).map(|i| format!("![i{}](https://a.com/{}.png)\n", i, i)).collect();
        let (_, n2, total2) = download_images_with(&md2, &dir, |_u| Ok(("image/png".into(), vec![1u8])));
        assert_eq!((n2, total2), (DOWNLOAD_MAX_IMAGES, 22));
    }

    #[test]
    fn test_download_images_with_no_images_noop() {
        let (out, n, total) = download_images_with("纯文本 [链接](https://a.com)", std::path::Path::new("/nonexistent"), |_u| panic!("无图不应下载"));
        assert_eq!((n, total), (0, 0));
        assert_eq!(out, "纯文本 [链接](https://a.com)");
    }
```

- [ ] **Step 2: 跑红 → 实现**

删 `embed_images`/`embed_images_with` 与其测试/`EMBED_*` 常量；`Cargo.toml` 删 base64。常量改名 `DOWNLOAD_*`（值不变）。新增：

```rust
/// 图片文件名（spec §3）：URL 末段去 query → unescape → sanitize（白名单同 sanitize_stem
/// 的字符集，另允许 `/` 已剥、首段空时 image-N 兜底）→ 无扩展名按 MIME 补 → 冲突 -N。
/// existing 为该目录已用名集合（调用方跨张维护——同一 URL 两处出现共享同一下载文件）。
pub fn image_filename(url: &str, mime: &str, existing: &std::collections::HashSet<String>) -> String {
    let unescaped = unescape_md_url(url);
    let raw = unescaped.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("");
    let ext_by_mime = || match mime {
        "image/png" => Some("png"), "image/jpeg" => Some("jpg"), "image/gif" => Some("gif"),
        "image/webp" => Some("webp"), "image/svg+xml" => Some("svg"), _ => None,
    };
    let has_known_ext = raw.rsplit('.').next()
        .map(|e| ["png","jpg","jpeg","gif","webp","svg"].contains(&e)).unwrap_or(false);
    let base: String = if raw.is_empty() {
        "image".into()
    } else {
        raw.chars().map(|c| if c.is_alphanumeric() || " -_.()[]".contains(c) { c } else { '_' })
            .take(80).collect::<String>().trim().trim_matches('.').to_string()
    };
    let base = if base.is_empty() { "image".into() } else { base };
    let ext = if has_known_ext { raw.rsplit('.').next().unwrap().to_string() }
              else { ext_by_mime().unwrap_or("bin").to_string() };
    let mut candidate = format!("{}.{}", base, ext);
    let mut n = 0;
    while existing.contains(&candidate) {
        n += 1;
        candidate = format!("{}-{}.{}", base, n, ext);
    }
    candidate
}

/// 下载 pass（spec §2）：dir = 图片目标目录（desktop 传 md 同名目录）。
/// 复用 extract_image_links（含转义括号）+ 守卫语义（数量/单张/总量，值同 DOWNLOAD_*）。
pub fn download_images_with(
    md: &str,
    dir: &std::path::Path,
    download: impl Fn(&str) -> Result<(String, Vec<u8>), String>,
) -> (String, usize, usize) {
    // 结构同 embed_images_with：targets → 逐张守卫+下载 → replacements(url→相对文件名)
    // + created dir（首张成功时 create_dir_all）+ 写文件；全部失败返回原样 md。
    // md 替换形态：![alt](<filename>)——filename 相对（图片与 md 同目录由调用方保证）。
    // 同 URL 两处出现：existing 含已定名 → 同名复用（一张文件、两处引用）。
    // …（实现按上述骨架落全，测试为准）
}

/// 生产绑定。pub fn download_images(md, dir) -> (String, usize, usize) = download_images_with(md, dir, download_image)
```

- [ ] **Step 3: 跑绿**

```bash
cargo test -p octopus-convert --lib 2>&1 | tail -3   # 预期：33 基础（41 - 8 embed 测试）+ 5 新 ≈ 38（以实跑为准）
cargo build -p octopus-convert 2>&1 | grep -cE "^(error|warning)"
```

- [ ] **Step 4: Commit**

```bash
git add crates/convert Cargo.lock
git commit -m "feat(convert): 图片下载 pass 替换 base64 内嵌（image_filename/download_images_with TDD）"
```

---

### Task 2: desktop 接线改名（TDD）

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/markdown.rs`
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs`

- [ ] **Step 1: 测试改写**——`test_apply_embed_no_remote_images_noop` 改为 `test_apply_download_images_noop`（无远程图 → 原样无注释，dir 用 temp）；注释规则断言不变（`<!-- 下载图片 N/M 张 -->`）。

- [ ] **Step 2: 实现**——`apply_embed` 删；`apply_download_images(md: &str, dir: &Path) -> String`（`octopus_convert::web::download_images` + 注释规则）；`convert_and_save` 参数 `embed: bool` → `download: bool`；后处理段：`dir = path.parent()`，**图片子目录** `img_dir = path 文件名去 .md`（`path.file_stem()`），调 `apply_download_images(md, &img_dir)`（该函数内部 create_dir_all）+ `md2 != md` 时重写 md 文件（先写 md 后建目录写图——顺序：download pass 落图 + 返回 md' → 重写 md）。`script.rs`：`item.action_data == "download_images"`。

- [ ] **Step 3: 验证**——`cargo test -p octopus-desktop markdown` 全绿（20 + 1 改写）；`cargo build -p octopus-desktop` 0 warning。

- [ ] **Step 4: Commit**——`feat(action-bar): 下载图片接线替换 embed（action_data=download_images）`

---

### Task 3: schema v62 原地改 + icon 换（TDD）

**Files:**
- Modify: `crates/infra/src/db/mod.rs`（61 臂 + 测试断言 + 注释）
- Modify: `crates/infra/resources/sql/schema.sql`
- Modify: `crates/desktop/frontend/src/components/ActionBarIcon.tsx`

- [ ] **Step 1: 测试断言改**——`migrate_v61_to_v62_seeds_embed_images_item` 改名 `..._seeds_download_images_item`，断言 `action_data == "download_images"`、title 改「转 Markdown（下载图片）」（跑红）。
- [ ] **Step 2: 实现**——迁移臂/schema.sql 的 id=13 INSERT 三字段改（title/icon `folder-down`/action_data）；icon map：删 `image-plus` 加 folder-down：
  `'<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><path d="M12 10v6"/><path d="m15 13-3 3-3-3"/>'`
- [ ] **Step 3: 验证**——`cargo test -p octopus-infra --lib` 全绿；前端 vitest components + tsc。
- [ ] **Step 4: Commit**——`feat(infra): v62 seed 改下载图片命令（folder-down）`

---

### Task 4: 预览链路 assetProtocol + convertFileSrc（TDD）

**Files:**
- Modify: `crates/desktop/tauri.conf.json`（app.security.assetProtocol）
- Create: `crates/desktop/frontend/src/pages/CompactEditor/resolveImgSrc.ts` + `.test.ts`
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPreview.tsx`（baseUrl prop + DOM 替换）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx`（透传 baseUrl）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`（file tab 传 baseUrl = filePath 父目录）

- [ ] **Step 1: resolveImgSrc 纯函数 TDD（红→绿）**

```ts
/**
 * 预览图片 src 解析（spec §5）：仅相对路径（非 http/data/asset/绝对 scheme）经 baseUrl join
 * 后交 convert 转换（生产 convert = convertFileSrc；测试注入 identity）。
 * md 源与保存零影响——本函数只用于渲染 DOM。
 */
export function resolveImgSrc(src: string, baseUrl: string | undefined, convert: (abs: string) => string): string {
  if (!baseUrl) return src;
  if (/^(https?:|data:|asset:|blob:|tci:)/i.test(src)) return src;
  if (src.startsWith("/")) return src; // 绝对路径不经 join（站点语义，保留）
  const joined = baseUrl.replace(/\/+$/, "") + "/" + src.replace(/^\.?\//, "");
  return convert(joined);
}
```

测试：http/data 跳过、无 baseUrl 原样、相对 join（`./a.png` 与 `dir/a.png` 形态）、convert 被调/不被调、尾部斜杠归一。

- [ ] **Step 2: tauri.conf.json + MarkdownPreview 接线**

```json
"security": { "assetProtocol": { "enable": true, "scope": ["$HOME/Documents/octopus/**"] } }
```
（app 段下；scope 覆盖 markitdown 与 screens/recordings 树。）

MarkdownPreview：`baseUrl?: string` prop；`useEffect`（html 变化后、与 innerHTML 注入同一 effect 内）：`article.querySelectorAll("img")` → `img.getAttribute("src")` 经 `resolveImgSrc(src, baseUrl, convertFileSrc)` 替换（setAttribute，避免浏览器先解析相对失败缓存）。MarkdownPane `baseUrl?: string` 透传；index.tsx MarkdownPane 调用点：`baseUrl={tab.source === "file" && tab.filePath ? tab.filePath.replace(/\/[^/]*$/, "") : undefined}`。

- [ ] **Step 3: 验证**——vitest CompactEditor 全绿 + tsc 0 + npm run build。
- [ ] **Step 4: Commit**——`feat(compact-editor): 相对路径图片预览（assetProtocol + convertFileSrc 渲染层）`

---

### Task 5: 全量验证 + 文档同步

- [ ] **Step 1**: `cargo build` 0w / `cargo test`（flake 如实注明）/ 前端 tsc + vitest 全量 + build
- [ ] **Step 2**: 手动 e2e（用户侧）：①双命令（folder-down 图标）②文章页下载 → md 相对引用 + 同名目录图片 + **预览图可见** ③VSCode/Obsidian 打开同款可显示（互操作）④坏图保留链接 + `<!-- N/M -->` ⑤原命令零回归
- [ ] **Step 3**: 文档——desktop-app §14 改下载方案；architecture v62 描述 + assetProtocol 一句；spec 实施注记；plan（旧 embed plan 文件删除，本文件即记录）
- [ ] **Step 4**: Commit `docs: 同步下载图片命令`

---

## Self-Review 记录

- **Spec coverage**：§2 布局/命名→Task 1；§3 接口→Task 1；§4 desktop→Task 2/3；§5 预览→Task 4；§6 错误→Task 1 测试；§7 测试矩阵→各 task；§8 文档→Task 5。无缺口。
- **签名变更面受控**：`convert_and_save` embed→download 改名；MarkdownPane/Preview +1 可选 prop；其余不动。
- **类型一致性**：`image_filename(url, mime, existing)`、`download_images_with(md, dir, download) -> (String, usize, usize)`、`apply_download_images(md, dir)`、`resolveImgSrc(src, baseUrl, convert)` 跨 task 一致。
- **实现期风险**：① Task 1 测试基线 = 41 - 8 embed 测试 + 5 新（估算 38，以实跑为准）；② Task 2 的 img_dir 用 `path.file_stem()`——md 同名目录（spec §2）；③ assetProtocol scope 变量 `$HOME` 语法按 Tauri 2 文档（`$HOME/...`）——若 dev 模式不生效查 `assetProtocol` dev 配置；④ jsdom 无 convertFileSrc——渲染层测试全部经注入 convert（identity），生产 import 仅在组件内。
