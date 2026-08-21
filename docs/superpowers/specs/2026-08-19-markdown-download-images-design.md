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
  → download_images_pass(md, dir = parent/<stem>_<ts>/)：

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

## 9. 实施注记（2026-08-20 实施完成回写）

① **Task 1 brief 测试三处修正**：(a) `image_filename` 尾斜杠兜底断言——brief 原断言 `"image-1.png"` 与前置 `existing={"cover.png"}` 状态不一致（`image.png` 未被占用不应 -N），改为先断言 `"image.png"` 再插入 existing 补 `-N` 分支覆盖；(b) **双扩展名 bug 红跑实证**——brief 实现骨架的 base 含扩展名未先剥尾部 `.ext`，会拼出 `cover.png.png`，TDD 红灯实证后实现补 stem 剥离（有已知扩展名时 `rsplit_once('.')` 去尾）；(c) success 测试并入转义括号覆盖——承接被删 escaped-parens 回归测试（`4de80445`）的关键断言：下载器收到 unescaped URL、输出无 `\(` 残留。

② **Task 2 审查发现 plan 级链接前缀缺陷并修复**（`6a9753de`）：plan 原设计 md 中替换为裸文件名，但 md 落 dir **父**目录、图片落 dir **子**目录——裸文件名相对 md 解析指向不存在路径。修复：链接形态 `![alt](<dir.file_name()>/<filename>)` 子目录前缀（dir 无末段名——根/`.`/`..`——退回裸文件名），spec §3 链接形态段同步回写；desktop `img_dir` 的 `file_stem` 空串显式兜底 `"images"`（不可达防御）。

③ **flake 根因修复**（`cfb48702`，非本 feature 代码但阻塞全量验证）：`test_t_interpolation` 调 `init("en")` 污染全局 DICT 不恢复，cargo test 并行窗口内其他测试的中文 `t()` 断言挂（锚：`test_collect_open_tabs_oversized_image_rejected` 的「图片过大」）。修复：从 `t()` 抽纯插值核心 `interpolate(template, params)`，测试改用本地 dict + interpolate 直测（en/zh 双语 + 多参键），`test_t_missing_key` 去 init。全量 `cargo test -p octopus-desktop` 562 passed / 0 failed 连续 3 次。

④ **v62 原地改**：feature 未发布不升 v63，迁移臂 / `schema.sql` / 迁移测试三处同步改 id=13 三字段（title/action_data/icon）。**dev 库注意**：已 seed 旧 embed 行的开发库 `INSERT OR IGNORE` 不会更新，需手动 `DELETE FROM action_bar_items WHERE id=13` 后重启重 seed。

⑤ **protocol-asset feature 必要性**：Tauri 2 的 `asset://` 协议注册由 tauri crate 的 `protocol-asset` cargo feature 控制（cfg-gated），仅开 `tauri.conf.json` `assetProtocol` 运行时不生效——经 vendored 源码验证后补 `crates/desktop/Cargo.toml` tauri features（Cargo.lock 连带新增 `http-range` 0.1.5）。这是 plan Task 4 文件清单外的必要补充。

⑥ **file: scheme 设计注记（终审裁定）**：`resolveImgSrc` 跳过清单为 `http(s)/data/asset/blob/tci` + `/` 开头绝对路径，**不含 `file:`**——终审裁定范围外接受现状：本管线产出的 md 只含相对路径（下载 pass）与 http(s) 原链接两种形态，不产生 `file:`；外部 md 带 `file:` 图时渲染层误当相对路径 join（仅预览失败，md 源与保存零影响），v1 不扩清单。

⑦ **folder-down icon**：`ActionBarIcon.tsx` LUCIDE_PATHS 删 `image-plus` 加 `folder-down`（lucide 官方 path：folder 主体 + 内部下箭头），与命令「下载到目录」语义对齐。

⑧ **assetProtocol scope × `markitdown_output_dir` 覆盖失配**（终审发现）：DB 键 `markitdown_output_dir`（infra `markitdown_dir()`）可把输出目录覆盖到任意路径，而 assetProtocol scope 固定 `$HOME/Documents/octopus/**`——覆盖到范围外时下载流程正常、md/图片文件本身正常落盘，但 CompactEditor 图片预览静默失图（scope 拒绝）。裁定：**不放宽 scope**（文档警告方向），已在 `desktop-app.md` §14「markitdown_output_dir 配置可覆盖」处加 ⚠️ 警告。
