# Markdown 图片 base64 内嵌 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 「转 Markdown（内嵌图片）」第二命令——md 中远程图片链接下载转 base64 data URI，失败降级保留链接；原命令不变。

**Architecture:** 内嵌是**最终 md 的后处理 pass**（outermost，`convert_and_save` 层）——不动任何现有编排函数签名与测试；`octopus-convert::web` 提供 `extract_image_links`（纯函数）+ `embed_images_with`（下载器注入可测）+ `embed_images`（生产绑定）；desktop 侧 `apply_embed` 薄封装（统计注释拼接）。

**Tech Stack:** base64 0.22（convert 新依赖，workspace 三 crate 已用同版本）、既有 regex/reqwest。

**Spec:** `docs/superpowers/specs/2026-08-19-markdown-embed-images-design.md`

## Global Constraints

- **开发隔离**：`.worktree/markdown-conversion` 分支，未经明确指令不进 main。
- **TDD**：Task 1/2/3 测试先行；生产下载绑定编译级 + 手动 e2e。
- **签名零波及**：`convert_and_save_url_with` / `convert_and_save_to` / `run_markdown_convert` 与其全部既有测试不动（outermost 后处理设计的关键约束）。
- **守卫常量**（spec §3）：`EMBED_MAX_IMAGES=20`、`EMBED_MAX_IMAGE_BYTES=5MB`、`EMBED_MAX_TOTAL_BYTES=30MB`、`EMBED_TIMEOUT_SECS=10`。
- **注释规则**：`<!-- 内嵌图片 N/M 张 -->` 仅 N>0 且 N<M 时前缀；全部失败=原样 md（等价 embed=false）。
- 0 warning；casing；schema v61→v62 幂等迁移。

---

### Task 1: extract_image_links + embed_images_with（TDD）

**Files:**
- Modify: `crates/convert/Cargo.toml`（`base64 = "0.22"`）
- Modify: `crates/convert/src/web.rs`（追加）

**Interfaces:**
- Produces: `pub const EMBED_MAX_IMAGES/EMBED_MAX_IMAGE_BYTES/EMBED_MAX_TOTAL_BYTES/EMBED_TIMEOUT_SECS`、`pub fn extract_image_links(md: &str) -> Vec<(String, String)>`、`pub fn embed_images_with(md: &str, download: impl Fn(&str) -> Result<(String, Vec<u8>), String>) -> (String, usize, usize)`

- [ ] **Step 1: 写失败测试（web.rs tests 追加）**

```rust
    // ── 图片内嵌（spec 2026-08-19-markdown-embed-images）──

    #[test]
    fn test_extract_image_links() {
        let md = "![图一](https://a.com/x.png) [链接](https://a.com/page) \
![图二](http://b.com/y.jpg \"title\") ![data](data:image/png;base64,xx) \
![file](file:///tmp/z.png) ![rel](img/w.png) ![无alt](https://c.com/w.svg)";
        let links = extract_image_links(md);
        let urls: Vec<&str> = links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, vec!["https://a.com/x.png", "http://b.com/y.jpg", "https://c.com/w.svg"]);
        assert_eq!(links[0].0, "图一");
        assert_eq!(links[2].0, "无alt");
    }

    #[test]
    fn test_embed_images_with_success() {
        let md = "前文 ![alt](https://a.com/x.png) 后文";
        let (out, n, total) = embed_images_with(md, |_u| Ok(("image/png".into(), vec![1u8, 2, 3])));
        assert_eq!((n, total), (1, 1));
        let b64 = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        assert!(out.contains(&format!("![alt](data:image/png;base64,{})", b64)), "out={}", out);
        assert!(!out.contains("https://a.com/x.png"));
    }

    #[test]
    fn test_embed_images_with_failure_keeps_link() {
        let md = "![a](https://a.com/x.png) ![b](https://b.com/y.png)";
        let (out, n, total) = embed_images_with(md, |u| {
            if u.contains("a.com") { Err("timeout".into()) } else { Ok(("image/png".into(), vec![9u8])) }
        });
        assert_eq!((n, total), (1, 2));
        assert!(out.contains("![a](https://a.com/x.png)"), "失败保留原链接");
        assert!(out.contains("data:image/png;base64,"));
    }

    #[test]
    fn test_embed_images_with_per_image_cap() {
        let md = "![big](https://a.com/x.png)";
        let (out, n, _) = embed_images_with(md, |_u| Ok(("image/png".into(), vec![0u8; EMBED_MAX_IMAGE_BYTES + 1])));
        assert_eq!(n, 0);
        assert!(out.contains("https://a.com/x.png"), "超单图帽保留链接");
    }

    #[test]
    fn test_embed_images_with_total_cap_stops_later() {
        let md = "![a](https://a.com/1.png) ![b](https://a.com/2.png) ![c](https://a.com/3.png)";
        // 每张 4MB：第一张后累计 4MB，第二张后 8MB……构造使第 3 张超总帽
        let size = EMBED_MAX_TOTAL_BYTES / 2 + 1; // 两张即超
        let (out, n, total) = embed_images_with(md, |_u| Ok(("image/png".into(), vec![0u8; size])));
        assert_eq!(total, 3);
        assert_eq!(n, 2, "第三张超总帽停止");
        assert!(out.contains("![c](https://a.com/3.png)"));
    }

    #[test]
    fn test_embed_images_with_count_cap() {
        let md: String = (0..25).map(|i| format!("![i{}]({}/{}.png)\n", i, "https://a.com", i)).collect();
        let (out, n, total) = embed_images_with(&md, |_u| Ok(("image/png".into(), vec![1u8])));
        assert_eq!(total, 25);
        assert_eq!(n, EMBED_MAX_IMAGES);
        assert!(out.contains("![i20](https://a.com/20.png)"), "第 21 张起保留链接");
        assert!(out.contains("data:image/png;base64,"));
    }

    #[test]
    fn test_embed_images_with_no_images_noop() {
        let (out, n, total) = embed_images_with("纯文本 [链接](https://a.com)", |_u| panic!("无图不应下载"));
        assert_eq!((n, total), (0, 0));
        assert_eq!(out, "纯文本 [链接](https://a.com)");
    }
```

- [ ] **Step 2: 跑红 → 实现（web.rs 追加）**

```rust
// ── 图片 base64 内嵌（spec 2026-08-19-markdown-embed-images §3）──

/// 内嵌守卫（spec §3，变更需回写 spec）。
pub const EMBED_MAX_IMAGES: usize = 20;
pub const EMBED_MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const EMBED_MAX_TOTAL_BYTES: usize = 30 * 1024 * 1024;
pub const EMBED_TIMEOUT_SECS: u64 = 10;

/// 提取 md 中可内嵌的远程图片链接：仅 `![alt](http/https://...)`；
/// 文本链接 / data: / file: / 相对路径跳过。
pub fn extract_image_links(md: &str) -> Vec<(String, String)> {
    static IMG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = IMG_RE.get_or_init(|| {
        regex::Regex::new(r"!\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)").unwrap()
    });
    re.captures_iter(md)
        .filter_map(|c| {
            let alt = c.get(1)?.as_str().to_string();
            let url = c.get(2)?.as_str().to_string();
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Some((alt, url))
            } else {
                None
            }
        })
        .collect()
}

/// 内嵌 pass（spec §2）：逐张经 download 下载 → 守卫 → data URI 替换。
/// 返回 (md', embedded, total)。失败/超帽保留原链接继续其余（spec §5）。
pub fn embed_images_with(
    md: &str,
    download: impl Fn(&str) -> Result<(String, Vec<u8>), String>,
) -> (String, usize, usize) {
    static IMG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = IMG_RE.get_or_init(|| {
        regex::Regex::new(r"!\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)").unwrap()
    });
    let targets = extract_image_links(md);
    let total = targets.len();
    let mut embedded = 0usize;
    let mut accumulated = 0usize;
    // 预解析成功的 URL 集 + data URI 映射（守卫决定谁进集合）
    let mut replacements: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (url, _) in targets.into_iter().map(|(alt, url)| (url, alt)).collect::<Vec<_>>().iter().map(|(u, _)| (u.clone(), ())) {
        if replacements.len() >= EMBED_MAX_IMAGES || accumulated >= EMBED_MAX_TOTAL_BYTES {
            break; // 数量/总量帽：停止后续（spec §5）
        }
        let Ok((mime, bytes)) = download(&url) else { continue };
        if bytes.len() > EMBED_MAX_IMAGE_BYTES { continue; }
        if accumulated + bytes.len() > EMBED_MAX_TOTAL_BYTES { continue; }
        accumulated += bytes.len();
        let b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        replacements.insert(url, format!("data:{};base64,{}", mime, b64));
        embedded += 1;
    }
    if embedded == 0 {
        return (md.to_string(), 0, total); // 全部失败=原样（spec §5）
    }
    let out = re
        .replace_all(md, |caps: &regex::Captures| {
            let url = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            match replacements.get(url) {
                Some(data) => format!("![{}]({})", caps.get(1).map(|m| m.as_str()).unwrap_or(""), data),
                None => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .into_owned();
    (out, embedded, total)
}
```

（实现允许微调——如 targets 去重：同一 URL 出现多次只下载一次且 replacement 覆盖全部出现——`replacements` HashMap 天然支持；测试未覆盖去重场景，实现按 HashMap 语义即可，勿额外引入「同 URL 只计一次 total」复杂度。）

- [ ] **Step 3: Cargo.toml + 跑绿**

`crates/convert/Cargo.toml` 加 `base64 = "0.22"`。

```bash
cargo test -p octopus-convert --lib 2>&1 | tail -3
```

Expected: 41 passed（33 既有 + 7 新 + extract 1 = 实际 33+8=41；以实跑为准记录）。

- [ ] **Step 4: Commit**

```bash
git add crates/convert Cargo.lock
git commit -m "feat(convert): extract_image_links + embed_images_with 内嵌 pass（TDD）"
```

---

### Task 2: 生产下载绑定 embed_images + MIME 映射（编译级）

**Files:**
- Modify: `crates/convert/src/web.rs`

**Interfaces:**
- Produces: `pub fn embed_images(md: &str) -> (String, usize, usize)`（真下载：EMBED_TIMEOUT_SECS + DESKTOP_UA + MIME fallback 扩展名映射）

- [ ] **Step 1: 实现（追加）**

```rust
/// MIME fallback：扩展名映射（spec §3）。未知扩展名 → Err（保留原链接，不瞎猜）。
fn mime_from_ext(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    let path = lower.split('?').next().unwrap_or(&lower);
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// 生产下载绑定：GET（EMBED_TIMEOUT_SECS、DESKTOP_UA）→ (mime, bytes)。
/// mime：Content-Type（strip ;charset）优先，fallback 扩展名映射；都不明 → Err。
fn download_image(url: &str) -> Result<(String, Vec<u8>), String> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(EMBED_TIMEOUT_SECS))
        .user_agent(DESKTOP_UA)
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| format!("下载失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let bytes = resp.bytes().map_err(|e| format!("读取失败: {}", e))?.to_vec();
    let mime = ct
        .filter(|m| m.starts_with("image/"))
        .or_else(|| mime_from_ext(url).map(str::to_string))
        .ok_or_else(|| "未知图片类型".to_string())?;
    Ok((mime, bytes))
}

/// 生产入口（spec §2）：embed_images_with + 真下载。编译级验证 + 手动 e2e。
pub fn embed_images(md: &str) -> (String, usize, usize) {
    embed_images_with(md, download_image)
}
```

（`mime_from_ext` 里第一段 `let ext` 是冗余行——删掉只留 `path` 分支；以编译 0 warning 为准。）

- [ ] **Step 2: 验证**

```bash
cargo build -p octopus-convert 2>&1 | grep -cE "^(error|warning)"
cargo test -p octopus-convert --lib 2>&1 | tail -2
```

- [ ] **Step 3: Commit**

```bash
git add crates/convert
git commit -m "feat(convert): embed_images 生产下载绑定（MIME 双源映射）"
```

---

### Task 3: desktop apply_embed + convert_and_save 后处理接线 + 测试

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/markdown.rs`
- Modify: `crates/desktop/src/action_bar/action_bar_commands/script.rs`（markdown 分支读 action_data 传 embed）

**Interfaces:**
- Consumes: `octopus_convert::web::embed_images`
- Produces: `fn apply_embed(md: &str) -> String`（纯函数：内嵌 + `<!-- N/M -->` 注释规则）；`convert_and_save` 增加 `embed: bool` 参数（**唯一**签名变更，消费点 script.rs 一处同步）

- [ ] **Step 1: 写失败测试（markdown.rs tests 追加）**

```rust
    // ── 图片内嵌接线（spec 2026-08-19）──

    /// apply_embed 是生产绑定直调真下载——网络不进单测。此处测注释规则：
    /// 用 embed 全失败形态（无 http 图片）断言原样 + 无注释。
    #[test]
    fn test_apply_embed_no_remote_images_noop() {
        let md = "纯文本 ![本地](img/x.png) [链接](https://a.com)";
        assert_eq!(apply_embed(md), md);
    }
```

（注释规则 `<!-- N/M -->` 的拼接逻辑在 apply_embed 内联三行，配合 convert 层的 embed_images_with 已测计数——desktop 侧以无图 noop 为代表性断言 + 编译级。）

- [ ] **Step 2: 实现**

`markdown.rs`：

```rust
/// 内嵌后处理（spec §2）：真下载 pass + 统计注释（仅 0 < N < M 时前缀）。
fn apply_embed(md: &str) -> String {
    let (embedded, n, m) = { let (out, n, m) = octopus_convert::web::embed_images(md); (out, n, m) };
    let _ = embedded; // 见下——直接返回处理值
    unreachable!()
}
```

（以上为示意——实际落成：）

```rust
/// 内嵌后处理（spec §2）：真下载 pass + 统计注释（仅 0 < N < M 时前缀）。
fn apply_embed(md: &str) -> String {
    let (out, n, m) = octopus_convert::web::embed_images(md);
    if n > 0 && n < m {
        format!("<!-- 内嵌图片 {}/{} 张 -->\n\n{}", n, m, out)
    } else {
        out
    }
}
```

`convert_and_save`（生产入口）改造——获得 (path, md) 后按 embed 后处理并**重写文件**（outermost 设计：不动内部编排签名/测试）：

```rust
pub(crate) fn convert_and_save(
    app: &tauri::AppHandle,
    files: Vec<String>,
    html: Option<String>,
    text: String,
    embed: bool, // 新增（spec §4）
) -> Result<(std::path::PathBuf, String), String> {
    let (path, md) = match route_input(&files, html.as_deref(), &text) {
        // …现有 Url/其余分支不变…
    }?;
    if embed {
        let md2 = apply_embed(&md);
        std::fs::write(&path, &md2).map_err(|e| format!("写入文件失败: {}", e))?;
        return Ok((path, md2));
    }
    Ok((path, md))
}
```

`script.rs` markdown 分支：

```rust
let embed = item.action_data == "embed_images"; // spec §1 双命令
// convert_and_save 调用补第 5 参 embed（grep 定位唯一调用点）
```

- [ ] **Step 3: 跑绿 + 回归**

```bash
cargo test -p octopus-desktop markdown 2>&1 | grep "test result"
cargo build -p octopus-desktop 2>&1 | grep -cE "^(error|warning)"
```

Expected: 21 passed（20 既有 + 1 新）；0 warning。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop
git commit -m "feat(action-bar): convert_and_save embed 后处理接线 + apply_embed 注释规则"
```

---

### Task 4: schema v62 seed + 前端 icon（TDD）

**Files:**
- Modify: `crates/infra/src/db/mod.rs`（CURRENT_SCHEMA_VERSION=62 + `61 =>` 迁移臂 + 测试）
- Modify: `crates/infra/resources/sql/schema.sql`（id=13 seed）
- Modify: `crates/desktop/frontend/src/components/ActionBarIcon.tsx`（LUCIDE_PATHS 加 image-plus）

**Interfaces:**
- Produces: seed id=13「转 Markdown（内嵌图片）」（action_type=markdown、action_data=embed_images、icon=image-plus、accepts=any、write_output_to_clipboard=1、sort_order=5）、schema v62

- [ ] **Step 1: 写失败迁移测试（db/mod.rs tests，照抄 v60→v61 模式）**

```rust
    /// v61→v62 迁移：seed「转 Markdown（内嵌图片）」菜单项（spec 2026-08-19）。
    #[test]
    fn migrate_v61_to_v62_seeds_embed_images_item() {
        let conn = open_with_version(61, "true");
        conn.execute("DELETE FROM action_bar_items WHERE id = 13", []).unwrap();

        init_schema(&conn).expect("v61→v62 迁移应成功");

        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 62);

        let (action_type, action_data, accepts): (String, String, String) = conn
            .query_row(
                "SELECT action_type, action_data, accepts FROM action_bar_items WHERE id = 13",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(action_type, "markdown");
        assert_eq!(action_data, "embed_images");
        assert_eq!(accepts, "any");

        init_schema(&conn).unwrap(); // 幂等
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM action_bar_items WHERE id = 13", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
```

- [ ] **Step 2: 实现（常量 + 迁移臂 + schema.sql）**

`CURRENT_SCHEMA_VERSION: u32 = 62`；`61 =>` 臂（在 60 臂后）：

```rust
            61 => {
                // v61→v62：ActionBar seed「转 Markdown（内嵌图片）」（spec 2026-08-19）
                conn.execute_batch(
                    "INSERT OR IGNORE INTO action_bar_items
                        (id, parent_id, title, icon, action_type, action_data,
                         sort_order, is_system, accepts, write_output_to_clipboard)
                     VALUES
                        (13, NULL, '转 Markdown（内嵌图片）', 'image-plus', 'markdown', 'embed_images', 5, 1, 'any', 1);",
                )
                .context("迁移 v61→v62：seed 内嵌图片菜单项")?;
                log::info!("DB migrated v61→v62: seed 转 Markdown（内嵌图片）");
            }
```

schema.sql 在 id=12 INSERT 后加同款 INSERT（注释标 v62）。

- [ ] **Step 3: 前端 icon（ActionBarIcon.tsx LUCIDE_PATHS）**

```tsx
  "image-plus": '<path d="M16 5h6"/><path d="M19 2v6"/><path d="M21 11.5V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7.5"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
```

（lucide image-plus v1.31.0 实测 path。）

- [ ] **Step 4: 验证 + Commit**

```bash
cargo test -p octopus-infra --lib 2>&1 | grep -E "test result|FAILED" | head -2
cd crates/desktop/frontend && npx vitest run src/components/ 2>&1 | grep "Tests " | head -1
```

```bash
git add crates/infra crates/desktop/frontend
git commit -m "feat(infra): schema v62 seed 内嵌图片菜单项 + image-plus 图标"
```

---

### Task 5: 全量验证 + 文档同步

**Files:**
- Modify: `docs/features/desktop-app.md` §14（双命令条目）
- Modify: `docs/architecture.md`（v62 一句 + convert web.rs 描述补内嵌）
- Modify: `docs/superpowers/specs/2026-08-19-markdown-embed-images-design.md`（实施注记）
- Modify: plan（勾选 + 实施记录）

- [ ] **Step 1: 全量验证**

```bash
cargo build 2>&1 | grep -cE "^(error|warning)"
cargo test 2>&1 | grep -cE "FAILED|error\["
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
```

- [ ] **Step 2: 手动 e2e（用户侧）**

1. ActionBar 出现「转 Markdown（内嵌图片）」（image-plus 图标，排在「转 Markdown」后）
2. 选中带远程图片的文章 URL → 内嵌命令 → md 中图片为 data URI、头部有 `<!-- N/M -->` 注释、CompactEditor 预览图可见
3. 原命令（转 Markdown）→ 图片仍为链接（零变化回归）
4. 全坏图页面 → md 原样无注释
5. 喂 AI 场景用原命令照旧

- [ ] **Step 3: 文档同步 + Commit**

```bash
git add docs
git commit -m "docs: 同步内嵌图片命令（desktop-app/architecture/spec 注记）"
```

---

## Self-Review 记录

- **Spec coverage**：§1 双命令→Task 3/4；§2 数据流→Task 3（outermost 后处理）；§3 接口/守卫/MIME→Task 1/2；§4 desktop 集成→Task 3/4；§5 错误降级→Task 1 测试覆盖；§6 测试矩阵→各 task；§7 文档→Task 5。无缺口。
- **签名零波及验证**：唯一签名变更是 `convert_and_save` +embed（消费点 script.rs 一处，grep 可定位）；`convert_and_save_url_with`/`convert_and_save_to`/`run_markdown_convert` 及 24 个既有测试不动——outermost 后处理 + 文件重写设计保证。
- **占位符**：Task 1 实现注记允许按 HashMap 语义微调（同 URL 多次出现共享下载）；Task 2 示意代码含一行冗余已标注删除；Task 3 含一段「示意→实际」的替换说明——执行者落实际版。
- **类型一致性**：`embed_images_with(md, download) -> (String, usize, usize)`、`embed_images(md)` 同型、`apply_embed(md) -> String`、`convert_and_save(+embed)` 跨 task 一致。
- **实现期风险**：① 测试计数以实跑为准（plan 估算 41/21）；② `regex` 双 OnceLock（extract 与 embed 各一）同 pattern——可共享一个 static，实现者按 DRY 判断（同 pattern 常量提取为 `fn img_re()`）；③ v61→v62 测试若 `open_with_version` 在 61 版本走全迁移链需 62 为 CURRENT 才不 bail——照抄 v60→v61 模式无此问题。
