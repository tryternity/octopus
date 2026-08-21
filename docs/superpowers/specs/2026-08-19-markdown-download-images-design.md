# Markdown 图片下载到同名目录 设计

- 日期：2026-08-19（同日修订：**替换** base64 内嵌方案——未进 main 前原地改向）
- 类型：增量功能（md 后处理 pass + 第二菜单项 + asset protocol 预览）
- 依赖：`extract_image_links` / `download_image` / `mime_from_ext` / `absolutize`（URL 路径）、schema v62 迁移链（未发布可原地改 seed）

## 0. 修订注记（base64 → 目录）

本 spec 初版（`0e41e603..4de80445`，已实现未合并）为 base64 内嵌方案。用户复审后改为**下载到同名目录**（Obsidian/Typora assets 模式）——互操作性更好（相对路径任何编辑器原生支持）、md 不膨胀、无 40MB 剪贴板/CM6 卡顿。替换范围：删除 `embed_images`/`embed_images_with` 与 base64 依赖；**保留** `extract_image_links`（含转义括号修复）、`download_image`、`mime_from_ext`、守卫常量（改名 DOWNLOAD_*）、outermost 后处理架构。

## 1. 需求（brainstorm 确认）

| 维度 | 决定 |
|---|---|
| 命令形态 | **双命令**：原「转 Markdown」（链接）+「转 Markdown（下载图片）」（id=13，`action_type="markdown"`，`action_data="download_images"`，icon `folder-down`） |
| 图片去向 | 下载到 **md 同名目录**（`<stem>_<ts>/`），md 内替换为**相对路径**引用 |
| 替换目标 | 仅图片 `![alt](http/https://...)`；文本链接不动 |
| 预览 | CompactEditor 经 asset protocol 显示相对路径图片；**md 源与保存写回保持相对路径**（互操作灵魂） |

**范围外**：anydoc 文件路径（图片是 alt text）；非 http(s) 协议；md 删除时目录不联动清理（v1 不管，文档注明）；重命名 md 不联动目录。

## 2. 数据流与目录布局

```
「转 Markdown（下载图片）」→ markdown 分支 item.action_data == "download_images"
  → convert_and_save(download: bool) 拿到 (path, md)
  → download_images_pass(md, dir = path.parent())：

markitdown/
├── 我的文章_20260819-143000.md      → ![alt](我的文章_20260819-143000/cover.png)
└── 我的文章_20260819-143000/        ← 与 md 同名（去 .md），自动创建
    ├── cover.png
    └── Foo_(bar)-1.png              ← 命名：URL 末段去 query → unescape → sanitize；
                                        冲突 -N；无扩展名按 MIME 补（.png/.jpg/.gif/.webp/.svg）

守卫（改名 DOWNLOAD_*，值不变）：20 张 / 5MB 单张 / 30MB 总量 / 10s 每张
失败/超帽 → 保留原远程链接；统计注释 `<!-- 下载图片 N/M 张 -->`（仅 0<N<M）
落盘后重写 md 文件（md' != md 时）→ CompactEditor
```

## 3. octopus-convert::web 接口

```rust
/// 内嵌 pass 删除；新增下载 pass（下载器注入，网络零进单测）。
/// 返回 (md', downloaded, total)。dir = 图片目标目录（desktop 传 md 同名子目录）。
pub fn download_images_with(
    md: &str,
    dir: &std::path::Path,
    download: impl Fn(&str) -> Result<(String, Vec<u8>), String>,
) -> (String, usize, usize)

/// 生产绑定：download_image（复用，EMBED_TIMEOUT_SECS→DOWNLOAD_TIMEOUT_SECS 改名）。
pub fn download_images(md: &str, dir: &Path) -> (String, usize, usize)
```

链接形态：md 中替换为 `![alt](<dir.file_name()>/<filename>)`（子目录前缀——2026-08-20 修复：裸文件名相对 md 解析会指向不存在路径，与图片落 `dir` 子目录不一致；dir 无末段名（根/`.`）时退回裸文件名）。

文件名规则（纯函数 `image_filename(url, mime, existing: &HashSet<String>) -> String`，可单测）：末段去 query → `unescape_md_url` → sanitize（白名单字符集同 sanitize_stem）→ 无扩展名/未知扩展按 MIME 补 → 冲突 `-N`。

## 4. desktop 集成

- `markdown.rs`：`apply_embed` 删除 → `apply_download_images(md, dir) -> String`（pass + 统计注释）；`convert_and_save` 参数 `embed` → `download`，拿到 (path, md) 后调 pass + 重写。
- `script.rs`：`item.action_data == "download_images"`。
- schema **v62 原地改**（未发布不升 v63）：迁移臂/schema.sql 的 id=13 改 title「转 Markdown（下载图片）」/action_data `download_images`/icon `folder-down`；迁移测试同步改断言。
- 前端：LUCIDE_PATHS 删 `image-plus` 加 `folder-down`。

## 5. 预览链路（相对路径 → 可显示）

1. `tauri.conf.json`：`app.security.assetProtocol = { enable: true, scope: ["$HOME/Documents/octopus/**"] }`——WKWebView 经 asset 协议读磁盘（scope 最小面到 octopus 输出树）。
2. `MarkdownPreview` 新增 `baseUrl?: string`（file tab 传 filePath 父目录）；innerHTML 注入后 DOM 层替换 `<img src>`：**仅**非 `http/data/asset` 前缀的相对 src → 绝对 join → `convertFileSrc()`。纯函数 `resolveImgSrc(src, baseUrl)` 抽出（convertFileSrc 注入点可 mock）供 vitest。md 源/保存零影响。

## 6. 错误处理

同 base64 方案的降级表：下载失败/超时/非 2xx/超单图帽/未知 MIME → 保留原链接继续；数量/总量帽停止后续；全部失败 = 原样无注释。新增：目录创建失败/写文件失败 → 整体走错误 temp tab 通道。

## 7. 测试计划（TDD）

| 模块 | 测试 |
|---|---|
| `image_filename` | sanitize/去 query/unescape/MIME 补扩展/冲突 -N 表驱动 |
| `download_images_with`（fake） | 落盘 + md 相对引用形态；失败保留；守卫截断；计数 |
| `apply_download_images` | 无图 noop；注释规则 |
| 前端 `resolveImgSrc` | http/data/asset 跳过；相对 join；baseUrl 缺省原样 |
| schema | v62 迁移断言改新字段 + 幂等 |
| 手动 e2e | 下载后预览图可见 + VSCode/Obsidian 打开同款可显示（互操作验证） |

## 8. 文档同步清单

- `desktop-app.md` §14：条目改为下载方案（目录布局/预览机制/守卫）
- `architecture.md`：v62 描述改 + assetProtocol 一句
- 旧 plan（embed 版）整体作废重写
- 本 spec §0 保留替换决策记录
