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
| 识别位置 | **后端**（`convert_and_save` 入口 `route_input`，⑬ 修订后 files > url > html > text）——前端零改动，Quick Execute 自动覆盖 |

**范围外（v1 不做）**：URL 直连 PDF/办公文档转换（Content-Type 非 HTML 直接报错；后续可用 anydoc 扩展）；登录墙/个性化内容（WKWebView 干净会话，不共享 cookie）；渲染后页面的滚动触发懒加载（settle 固定 2s）。

## 2. 数据流与决策树

> 输入优先级（2026-08-18 终审 ⑬ 修订，与 §9 注记一致）：**files > url > html > text**。

```
convert_and_save(files, html, text)——route_input 纯函数分流：
  files 非空 → 现有 run_markdown_convert + output_file_stem（零变化）
  is_explicit_url(text)（单行显式 URL——意图优先于 html flavor：浏览器对纯文本
    选区也写 html，原「files/html 空」gate 会遮蔽页内选中的 URL）
    → URL 路径：
       1. 静态抓取 fetch_page（octopus-convert::web）
       2. md = absolutize + htmd(html)
       3. 空壳判定：md.trim() < SPA_SHELL_THRESHOLD(200 字符)
          → 非空壳：(md, stem) 直接用
          → 空壳：web_render::render_html(url)（§4）
              → outerHTML → absolutize + htmd → (md, stem)
              → 渲染超时/失败 → Err（不落半成品）
    → 落盘 <stem>_<时间戳>.md（复用碰撞后缀）→ CompactEditor file tab
  html / text → 现有 run_markdown_convert + output_file_stem（零变化）
```

`run_markdown_convert` 签名与语义不变（URL 编排在 `convert_and_save` 层经 `route_input` 分流，其既有测试零波及）。

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
| 请求头 | Safari macOS UA（Version/17.4，与 WKWebView 渲染 fallback 同族——⑮ 修正，原误写 Chrome）+ `Accept-Language: zh-CN`（默认 reqwest UA 会被 403） |
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

## 9. 实施注记（2026-08-18 实施完成，Task 5 回写）

按 plan（`2026-08-18-url-to-markdown.md`）5 个 task 全部落地，commits：T1 `e520e48c` / T2 `a44d44d2` / T3 `dda1b583` / T4 `c366c2ad` / T5 见 plan 实施记录。与 spec 的偏差：

1. **dispatch_after 等效替代**（§4）：spec 的 `dispatch_after(250ms)` 重试用「监控线程 `recv_timeout(250ms)` 超时节拍 + `run_on_main_thread` 回投探针」等效替代（plan 预声明）——零 GCD 依赖；settle 延时同理（监控线程 sleep 后回投 final evaluate）。
2. **`render_html` 签名携 `AppHandle`**（§4）：spec 签名 `render_html(url)` 无 app——Tauri 无全局 handle API，改为 `render_html(app: &AppHandle, url: &str)`；连带 `convert_and_save` 生产入口加 `app` 参数（`script.rs` markdown 分支调用点补传 `&ah`）。
3. **URL 检测分支位置**（§2）：spec 画在 `convert_and_save_to`，实际位于 `convert_and_save`（生产入口，`markitdown_dir()` 解析处）——`convert_and_save_to` 保持无 app 依赖可单测，语义等价（files/html 空 + 显式 URL 的判定条件不变）。
4. **reqwest features**（§3）：除 `gzip` 外补 `blocking`（fetch_page 用 `reqwest::blocking::Client`，调用方在 spawn_blocking 上下文）。
5. **`Accept-Language` 挂 RequestBuilder**（§3）：reqwest 0.12 blocking ClientBuilder 无 header 方法，移到 `.get(url)` 后的 RequestBuilder（语义等价）。
6. **sniff_charset 两处修正**（§3）：`<meta charset="` 长度恰 15 字节（plan 骨架误写 s+17 会截掉前 2 字符）；`meta_equiv` 值可能被引号包裹（`charset='x'` 单引号变体）——跳过开头引号、按同引号截断。
7. **`sanitize_stem` 兜底**（§5）：sanitize+trim 后**为空或不含任何字母数字**（如 `///` → `___`）→ 兜底 `"markitdown"`（spec 未明说；TDD 中 brief 测试期望即规格）。
8. **objc2-web-kit features**（§4）：实际需 `WKWebView` + `WKNavigation` + `block2` 三 feature（spec 只写了 WKWebView/WKWebViewConfiguration；`loadRequest` 返回 `Retained<WKNavigation>` 需要 WKNavigation feature，未用 WKWebViewConfiguration）。
9. **block2 API 形态**：用 `RcBlock<dyn Fn(*mut AnyObject, *mut NSError)>`（block2 0.6 无 `Block2` 类型；completion 是 async 回调需堆分配 RcBlock，`Some(&block)` 传参）。
10. **completion 参数 +0 借用**：block 回调的 `*mut AnyObject` / `*mut NSError` 按 ObjC block 惯例为 +0 借用（调用方 WKWebView 保活），实现仅以 `&*ptr` 引用形态使用、**禁止 `Retained::from_raw` 接管**（会过度释放）。
11. **absolutize 已知限制**（§3）：代码块内的示例相对路径同样被正则改写（md 后处理不理解代码块语义，接受）。
12. **plan brief 两处笔误修正**：sniff_charset 偏移（见 6）；Task 1 期望计数「24 既有 + 7 新」——实际新增 **6** 个测试 fn（24+6=30，最终 30 passed 与期望值吻合，「+7」为笔误）。

终审修复追加（2026-08-18 终审，同一 worktree 单 commit）：

13. **输入优先级修订**（§2）：**url 提升到 html 之前**（files > url > html > text）——意图优先。终审发现浏览器对纯文本选区也写 html flavor，原「files/html 空 && 显式 URL」gate 把页内选中的 URL 遮蔽进 html-conversion（产出垃圾 `[url](url)`），§1 核心场景「选中 URL → 抓取」失效。路由抽为纯函数 `route_input`（markdown.rs，表驱动单测 2 个）。
14. **错误前缀去重**（§6）：`ConvertError` 新增 `Web(String)` **裸变体**（Display 无前缀）替换 `Html` 变体——原「HTML 转换失败: 」与编排层「抓取失败: 」叠加成双重前缀（「抓取失败: HTML 转换失败: HTTP 404」）；fetch_page 是 `Html` 唯一使用者，变体已删。`render_html` 同步改返**裸消息**（「渲染超时（SPA 页面）」/「通道关闭」/ JS 错误原文，去掉自带的「渲染失败: 」前缀）——两条链的前缀均由编排层（`convert_and_save_url_with`）唯一叠加 → 「抓取失败: HTTP 404」/「渲染失败: 渲染超时（SPA 页面）」各单前缀。
15. **UA 标签修正**（§3）：`DESKTOP_UA` 实为 **Safari macOS UA**（Version/17.4 Safari/605.1.15，与 WKWebView 渲染 fallback 同族）——原 spec 表格与 fetch_page 注释误写 Chrome。
16. **web_render 加固**（§4）：① **stale probe guard**——monitor 循环收到 `Signal::Failed` 时若已 settled（final evaluate 在途）则忽略并 continue（旧探针迟到失败不是权威结果；代价：final evaluate 自身失败时以 deadline 超时文案兜底）；② **create fail-fast**——`run_on_main_thread` 派发失败（app 退出中）立即发 `Failed("主线程派发失败")`，避免监控循环空转 20s。

**验证**（2026-08-18，worktree `markdown-conversion`）：`cargo build` 0 error 0 warning；`cargo test` 全 workspace 902 passed / 1 failed / 17 ignored——唯一失败 `test_collect_open_tabs_oversized_image_rejected` 为 **pre-existing flake**（Task 4 已在干净 HEAD `dda1b583` 验证全量跑失败、单跑通过；Task 5 复跑单跑同样通过，测试隔离问题，与本 feature 无关）；前端 `tsc --noEmit` 0 error + `vite build` 通过。手动 e2e（spec §7）待用户侧执行。

**终审修复验证**（2026-08-18）：`cargo build -p octopus-convert -p octopus-desktop` 0 error 0 warning；`cargo test -p octopus-convert --lib` **33 passed**（32 + `Web` Display 裸前缀测试）；`cargo test -p octopus-desktop markdown` **20 passed**（18 既有含 404/渲染超时传播两测试 + 2 新 route_input 表驱动）。
