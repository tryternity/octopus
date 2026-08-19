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
| 累计 > 30MB 或总数 > 20 | 停止后续（已替换的保留），统计注释反映 N/M |
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
