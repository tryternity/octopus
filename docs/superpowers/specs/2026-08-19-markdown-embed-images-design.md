# Markdown 图片 base64 内嵌设计

- 日期：2026-08-19
- 类型：增量功能（md 后处理 pass + 第二菜单项）
- 依赖：`absolutize_md_links`（URL/HTML 路径产物）、`fetch_page` 同款 HTTP client 构造、schema v61 迁移链

## 1. 需求（brainstorm 确认）

转换 Markdown 时把**远程图片链接**（`![alt](https://...)`)下载后替换为 base64 data URI，产出自包含 md（存笔记/离线查看场景）；原「转 Markdown」命令不变（链接形式——喂 AI 场景 base64 是 token 毒药）。

| 维度 | 决定 |
|---|---|
| 作用路径 | URL 抓取产物 + HTML 选区产物（两条远程图片链接来源） |
| 开关 | **双命令并存**：新菜单项「转 Markdown（内嵌图片）」（id=13，`action_type="markdown"`，`action_data="embed_images"`）；原命令不变，零配置 UI |
| 替换目标 | 仅图片 `![alt](http/https://...)`；文本链接 `[text](url)` 不动 |

**范围外（v1 不做）**：anydoc 文件路径（文档内嵌图片是 alt text，无链接可嵌）；非 http(s) 协议（file:/cid:/data: 保留原样）；部分内嵌的 UI 提示（失败静默保留链接 + 结果头部统计注释）。

## 2. 数据流

```
「转 Markdown（内嵌图片）」→ markdown 分支读 item.action_data == "embed_images"
  → embed: bool 传入 convert_and_save 系
URL 路径：fetch → absolutize → htmd → md ─┐
HTML 选区路径：html_to_markdown → md（无 absolutize——选区无 base URL，相对图片链接非 http 被内嵌扫描跳过）─┤
                                                    ↓ embed=true
                                    embed_images_pass(md) → md'
  ├─ 扫描图片链接 → 逐张 GET（EMBED_TIMEOUT_SECS、同 DESKTOP_UA）
  ├─ MIME：Content-Type 优先，fallback 扩展名映射（png/jpg/jpeg/gif/webp/svg）
  ├─ 替换 `![alt](data:<mime>;base64,<b64>)`（SVG 统一 base64，免转义心智）
  ├─ 失败/超帽 → 该张保留原链接，继续其余
  └─ 结果头部统计注释：`<!-- 内嵌图片 N/M 张 -->`（N<M 时才有）
落盘/编辑器/剪贴板链路零变化（md' 只是更长的 md）
```

## 3. octopus-convert::web 扩展

```rust
/// 提取 md 中可内嵌的远程图片链接（纯函数）。返回 (alt, url) 列表——
/// 仅 ![alt](http/https://...)；data:/file:/cid:/相对路径（未绝对化时）跳过。
pub fn extract_image_links(md: &str) -> Vec<(String, String)>

/// 内嵌 pass（下载器注入，网络零进单测）。返回 (md', embedded, total)。
pub fn embed_images_with(
    md: &str,
    download: impl Fn(&str) -> Result<(String /*mime*/, Vec<u8>), String>,
) -> (String, usize, usize)

/// 生产绑定：真下载（fetch_page 同款 client：DESKTOP_UA + EMBED_TIMEOUT_SECS）。
pub fn embed_images(md: &str) -> (String, usize, usize)
```

**守卫常量**（spec 记录，变更需回写）：

| 常量 | 值 | 理由 |
|---|---|---|
| `EMBED_MAX_IMAGES` | 20 | 页面动辄几十图，全嵌撑爆 md |
| `EMBED_MAX_IMAGE_BYTES` | 5MB/张 | base64 膨胀 33% 后 ~6.7MB，超大图保留链接 |
| `EMBED_MAX_TOTAL_BYTES` | 30MB（累计原 bytes） | md 总量帽——预览截断只护 preview，编辑栏全量加载 |
| `EMBED_TIMEOUT_SECS` | 10/张 | 同站快跨站慢的折中 |

MIME fallback 映射：png→image/png、jpg/jpeg→image/jpeg、gif→image/gif、webp→image/webp、svg→image/svg+xml；未知扩展名 → 保留原链接（不瞎猜）。

## 4. desktop 集成

- `markdown.rs`：`convert_and_save_url_with` 与 HTML/text 分支各加 `embed: bool`（编排函数参数 +1）；`convert_and_save` 系从 `execute_action_bar_inner` 传入（item.action_data == "embed_images"）；统计注释在落盘前拼（N<M 时）。
- `script.rs` markdown 分支：读 `item.action_data` → `embed` bool → 传参。
- schema v61→v62 迁移臂：seed id=13「转 Markdown（内嵌图片）」（icon `image-plus`，accepts=any，write_output_to_clipboard=1，sort_order 5）+ schema.sql 新装同款 INSERT + `CURRENT_SCHEMA_VERSION=62`。
- 前端：ACTION_TYPES/deriveAccepts 不变（同 markdown 型）；LUCIDE_PATHS 加 `image-plus`；i18n zh/en 各 1 条（菜单 title 走 DB seed 不需 i18n，但设置页类型描述沿用 typeMarkdownDesc——无需新 key；如需区分文案再加）。

## 5. 错误处理

| 场景 | 行为 |
|---|---|
| 单张下载失败/超时/非 2xx | 保留原链接，继续其余 |
| 单张 > 5MB | 保留原链接 |
| 累计达 30MB（per-item 帽保证永不超额）或总数达 20 | 停止后续（已替换的保留），统计注释反映 N/M |
| MIME 未知（无 Content-Type 且扩展名不识） | 保留原链接 |
| 全部失败 | md 原样（等价 embed=false），无统计注释（N=0 时也省略） |

## 6. 测试计划（TDD）

| 模块 | 测试 |
|---|---|
| `extract_image_links` | 表驱动：图片/链接区分、http/https 收、data:/file:/cid: 跳过、alt 含空格/括号、markdown 嵌套形态 |
| `embed_images_with`（fake） | 成功替换 data URI + MIME；失败保留；单图帽；总帽截断；数量帽；统计计数；`<!-- N/M -->` 拼接规则（N<M 时/N=M 时省略） |
| desktop 编排（fake） | embed=true 经内嵌（fake downloader 断言被调）；embed=false md 原样不经 pass |
| schema | v61→v62 迁移测试（seed id=13 存在 + 幂等） |
| 生产下载绑定 | 编译级 + 手动 e2e |

## 7. 文档同步清单

- `docs/features/desktop-app.md` §14：双命令条目 + 内嵌行为/守卫
- `docs/architecture.md`：v62 一句
- 本 spec 实施注记回写

## 8. 实施注记（2026-08-19 实施后回写）

实现与 spec/plan/brief 的偏差与补充（全部对照源码核实，commits 5d784717 → 62640f49 → docs 同步）：

1. **`fn img_re()` 共享 static（DRY）**：brief 中 extract 与 embed 各持一个同 pattern 的 `OnceLock<Regex>`——实抽为单例 `fn img_re()`（`crates/convert/src/web.rs`），两处共用（plan 自审风险②的落地）。
2. **loop 简化 + `contains_key` 去重**：同 URL 出现多次只下载一次（replacements 映射覆盖全部出现）；计数语义写入 doc comment——`total` 按 md 中**出现次数**计（extract 计所有匹配），`embedded` 按**成功下载的不同 URL** 计（同一 URL 替换两处也只计 1）。
3. **brief 总帽测试 bug 修正**：brief 原测试 `size = TOTAL/2+1`（15MB）超 5MB 单图帽，永远到不了总帽判定（不可达路径）——改为 7 张各恰 5MB（单图帽 `>` 判定、等号放行）：前 6 张累计 30MB=总帽，第 7 张触发停止，总帽语义验证不变。
4. **brief raw-string 编译修正**：regex 含内嵌双引号，brief 的 `\"` 转义写法落成 `r#"..."#` 原始字符串（语义等价，编译通过为准）。
5. **测试计数笔误**：plan「33 既有 + 8 新 = 41」实为 **7 个新测试**（extract 1 + embed 6）→ octopus-convert 共 **40 passed**（33 既有 + 7 新）。
6. **`mime_from_ext` 冗余行删除**：brief 里第一段 `let ext`（未 strip query 的死代码）已删，只留 `path` 分支；`unwrap_or` 不可达分支（`split` 永不返回空迭代）按 brief 原文保留。
7. **fragment 未剥离**：URL `#fragment` 不剥（只 strip `?query`）——`x.png#frag` 扩展名匹配失败 → 保守退化保留链接（宁可少嵌不瞎猜，与未知 MIME 同策略）。
8. **v60→v61 断言泛化**：该迁移测试的版本断言从字面量 `61` 改 `CURRENT_SCHEMA_VERSION`（v62 起迁移链会继续过 v61→v62 seed 臂）+ v59→v60 测试注释同步（「升到 61」→「升到 CURRENT」）。
9. **desktop 增补（spec §4 dispatch 层）**：`convert_and_save` embed 后处理加 `md2 != md` 守卫（内嵌无变化不重写文件）；script.rs 的 `embed` bool 在 spawn 闭包**前**计算（保持 moved set 最小）。

**§5 措辞更正**（Task 1 审查建议，已回写上表）：总帽行「累计 > 30MB」→「累计达 30MB（per-item 帽保证永不超额）」——实现是循环顶 `accumulated >= TOTAL` 先停 + 入图前 `accumulated + len > TOTAL` 拒绝，累计字节永不超额。

**验证数字（2026-08-19 全量）**：workspace `cargo build` 0 error 0 warning；`cargo test` 全过（唯一失败为 pre-existing flake `test_collect_open_tabs_oversized_image_rejected`，隔离复跑通过）；convert 40 / desktop markdown 21（20 既有 + 1 新）/ infra 197；前端 tsc 0 error + vitest 532 passed（32 文件）+ vite build 成功。
