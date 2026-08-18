# URL 抓取转 Markdown 设计

- 日期：2026-08-18
- 类型：增量功能（主 spec `2026-08-18-actionbar-markdown-conversion-design.md` §1 范围外的遗留 TODO）
- 依赖：`octopus-convert::html_to_markdown`（htmd）、`convert_and_save` 落盘链路、objc2-web-kit 0.3（已在依赖树）

## 1. 需求（brainstorm 确认）

「转 Markdown」命令支持 URL 输入：选中 URL → Alt+D / Quick Execute → 静态抓取 → 转 md → 落盘 `markitdown` 目录 → CompactEditor 打开（与文件/文件夹/富文本同一条输出链路）。

| 维度 | 决定 |
|---|---|
| 抓取深度 | **静态 + 渲染 fallback**：静态抓到 SPA 空壳时用离屏 WKWebView 渲染后重转 |
| URL 识别 | **仅显式**：text trim 后单行且以 `http://` / `https://` / `www.` 开头（www. 补全 https://）；其余照旧纯文本直通——裸域名/IP 不识别（防误抓） |
| 识别位置 | **后端**（`convert_and_save_to` 入口）——前端零改动，Quick Execute 自动覆盖 |

**范围外（v1 不做）**：URL 直连 PDF/办公文档转换（Content-Type 非 HTML 直接报错；后续可用 anydoc 扩展）；登录墙/个性化内容（WKWebView 干净会话，不共享 cookie）；渲染后页面的滚动触发懒加载（settle 固定 2s）。

## 2. 数据流与决策树

```
convert_and_save_to(files, html, text)：
  files/html 空 && is_explicit_url(text)
    → URL 路径：
       1. 静态抓取 fetch_page（octopus-convert::web）
       2. md = absolutize + htmd(html)
       3. 空壳判定：md.trim() < SPA_SHELL_THRESHOLD(200 字符)
          → 非空壳：(md, stem) 直接用
          → 空壳：web_render::render_html(url)（§4）
              → outerHTML → absolutize + htmd → (md, stem)
              → 渲染超时/失败 → Err（不落半成品）
    → 落盘 <stem>_<时间戳>.md（复用碰撞后缀）→ CompactEditor file tab
  其余输入 → 现有 run_markdown_convert + output_file_stem（零变化）
```

`run_markdown_convert` 签名与语义不变（URL 编排在 `convert_and_save_to` 层，其 7 个测试零波及）。

## 3. 静态抓取（octopus-convert 新 `web.rs`）

```rust
pub fn is_explicit_url(text: &str) -> Option<String>  // 识别 + www. 补全
pub fn fetch_page(url: &str) -> Result<FetchedPage, ConvertError>
pub struct FetchedPage { pub html: String, pub final_url: String, pub title: Option<String> }
pub(crate) fn absolutize_md_links(md: &str, base: &str) -> String
pub fn extract_title(html: &str) -> Option<String>  // 宽容 <title> 解析
fn sniff_charset(headers: &[u8], body_head: &[u8]) -> &'static encoding_rs::Encoding  // 纯函数
```

| 项 | 决定 |
|---|---|
| 客户端 | reqwest 0.12（desktop 已依赖；**补 `gzip` feature**——部分站点强制 gzip） |
| 超时 | `WEB_FETCH_TIMEOUT_SECS = 15`（infra HTTP_TIMEOUT 120s 是长任务语义，不适用） |
| 请求头 | Chrome macOS UA + `Accept-Language: zh-CN`（默认 reqwest UA 会被 403） |
| 重定向 | reqwest 默认跟随；base 用 `response.url()`（重定向后最终 URL） |
| 非 2xx / 非 HTML / 大小 | 报错（含状态码）/「该 URL 不是 HTML 页面」/ HTML > `WEB_MAX_HTML_BYTES = 20MB` 拒绝 |
| charset | Content-Type charset → BOM → 前 2KB `<meta charset>` 嗅探 → 默认 UTF-8；非 UTF-8 经 `encoding_rs` 解码（GBK/GB18030/Shift_JIS/Big5/Latin1 常见表） |
| 绝对化 | **md 后处理**（非 HTML 预处理）：`[text](rel)` / `![alt](rel)` 经 `url::Url::join(base)` 绝对化；跳过 `#` / `mailto:` / `data:` / 已带 scheme。理由：`<base href>` 注入不改写 htmd 输出值；md 语法面窄可穷举测试 |
| 新依赖 | convert crate：`reqwest`（gzip）、`encoding_rs`、`url` |

## 4. 渲染 fallback（desktop 新 `ui/web_render.rs`，cfg macos）

分层：`octopus-convert::web` 保持平台纯净（纯逻辑 + reqwest）；WKWebView 胶水在 desktop（`objc2-web-kit` 0.3 直接启用——已在依赖树，desktop 加 features：`WKWebView`、`WKWebViewConfiguration`）。

```rust
pub fn render_html(url: &str) -> Result<String, String>  // 返回 outerHTML
```

**线程模型**（零 NavigationDelegate、零跨线程属性读）：

```
调用线程（spawn_blocking）：oneshot channel + RENDER_TIMEOUT_SECS(20) recv_timeout 守护
主线程（run_on_main_thread）：
  1. 创建离屏 WKWebView（不 attach window——headless，Tauri 主 runloop 驱动）
  2. load(URL)
  3. evaluateJavaScript("(document.readyState==='complete')
        ? document.documentElement.outerHTML : null")
     completion block 链式自轮询（block2）：null → dispatch_after(250ms) 重试；
     拿到 HTML → 等 RENDER_SETTLE_MS(2000)（SPA 懒加载/字体）→ 取最终 outerHTML
     → channel 送回 + 释放 webview
超时/失败：主线程清理 → Err（「渲染超时/失败」）
```

- completion-block 链式轮询替代 `WKNavigationDelegate`——避开 objc2 `declare_class` 委托生命周期管理
- 干净会话（非持久化 storage）——登录墙内容抓不到，已知限制

## 5. 命名与守卫

- **stem**：`sanitize(extract_title(html))`（路径非法字符→`_`、长度 ≤60）fallback URL host；文件名 `<stem>_<yyyymmdd-HHMMSS>.md`，同秒碰撞 `-N` 后缀（复用）
- 常量（spec 记录，变更需回写）：`SPA_SHELL_THRESHOLD = 200` 字符、`WEB_FETCH_TIMEOUT_SECS = 15`、`WEB_MAX_HTML_BYTES = 20MB`、`RENDER_SETTLE_MS = 2000`、`RENDER_TIMEOUT_SECS = 20`

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| 非 2xx | 「抓取失败: HTTP <code>」 |
| 非 HTML | 「该 URL 不是 HTML 页面」 |
| > 20MB | 「页面过大（上限 20MB）」 |
| 静态空壳 + 渲染超时/失败 | 「渲染超时/失败（SPA 页面）」——不落半成品 |
| 静态成功非空壳 | 快速路径，不触发渲染 |

错误经现有「转 Markdown 失败」temp tab 通道（`TempTabPayload`），无需新 UI。

## 7. 测试计划（网络调用不进单测——纯函数全覆盖 + 手动 e2e）

| 模块 | 测试 |
|---|---|
| `is_explicit_url` | 表驱动：http/https/www ✓（含补全）；裸域名/多行/空/带空格 ✗ |
| `absolutize_md_links` | 相对→绝对、`../` 回溯、`#`/mailto/data:/已绝对 跳过、图片/链接两形态 |
| `extract_title` + sanitize | 有/无/多行 title、非法字符、超长截断、fallback host |
| `sniff_charset` | header charset > BOM > meta 优先级；GBK bytes + meta 声明解码正确 |
| desktop 编排 / web_render | 编译级 + 手动 e2e（文章页快速路径 + SPA 样例 fallback） |

## 8. 文档同步清单

- `docs/features/desktop-app.md` §14：markdown 命令补 URL 输入行
- `docs/architecture.md`：desktop 模块清单补 `ui/web_render.rs`
- 主 spec `2026-08-18-actionbar-markdown-conversion-design.md` §1 范围外-URL 条目改为指向本 spec
- 本 spec 实施注记回写
