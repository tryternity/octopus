# 搜索多 Provider 架构重构设计

> 2026-07-16 · 借鉴 wox 多源广播思想，重构 octopus 搜索为 Provider trait + 并发扇出 + 流式渐进渲染 + 频次加权。修复 bookmark 不显示问题，新增 calculator/url 源。
>
> **状态**：实现完成（2026-07-16，含用户手测后反馈驱动修复）。实际偏差见下方各节"实现注"。

## 0. 背景与动机

### 0.1 用户反馈

> "现在搜到的内容还比较少，只有应用和文件。另外 shell 和标签都没有匹配到的内容。"

经代码 + 环境排查，根因有三层：

1. **shell**：原是匹配逻辑 bug（要求 `>` 前缀 + 前端未接入辅助函数）。**手测后用户判定 launcher 场景下 shell 是伪需求**（无终端上下文 / 输出无展示区），**已移除整个 shell 功能**——详见 §4.2「ShellProvider 已移除」。
2. **"标签"实为浏览器收藏（bookmark）覆盖缺失**：用户主要用 Chrome（书签在 `Profile 1`，非 `Default`），另有 Safari。原代码只读 Chrome/Edge 的 `Default/Bookmarks`，漏掉多 profile + Safari + Firefox。→ bookmark tab 永远空。
3. **架构串行**：当前 `search()` 是 6 个 source 串行 `results.extend`，慢源（mdfind）拖慢整体，且无频次加权，常用项不会排前。

### 0.2 借鉴 wox 的核心思想（不照搬实现）

wox 是 38 插件、3 语言运行时、Flutter UI 的完整体系，直接照搬工作量极大且与 octopus Tauri 单进程架构不匹配。**借鉴的是架构思想**：

| wox 思想 | octopus 采纳方式 |
|---|---|
| `*` 触发词 = 全局搜索源注册 | Provider 声明 `matches_tab` |
| 并发扇出所有插件 | `FuturesUnordered` 单 task 并发（不 spawn，避免 ctx 生命周期问题） |
| `FallbackSearcher` 接口 | `is_fallback()` trait 方法（本期暂不启用 fallback provider，预留） |
| 频次加权（斐波那契衰减） | 简化为 7 天滑窗 + 当次 query 加分 |
| 渐进式渲染（resultDebouncer） | Tauri 事件 + 前端 listen 增量 |
| `IgnoreAutoScore` 特性 | `uses_frequency()` trait 方法 |
| ScoreKey 稳定标识 | `source + "\|"` + action_data 稳定字段 |

**不采纳**：wox 的插件 SDK、外部进程通信、SQLite FTS5 文件引擎（octopus 继续用 mdfind）、分组（Group）显示、拼音独立 FTS 表。

## 1. 设计目标

1. **广覆盖**：从 6 个 source 重构为 6 个 Provider（app/file/menu/bookmark/calculator/url），修复 bookmark 让现有源真正可用，新增 calculator/url。**shell 手测后移除**（详见 §4.2）。
2. **搜得准**：频次加权让常用项排前（借鉴 wox 斐波那契衰减思想，简化实现）+ **word-prefix 匹配**让 "Google Chrome" 搜 "chrome" 走分词匹配拿高分（详见 §4.7）。
3. **搜得快**：并发扇出 + 流式渐进渲染，首屏 < 30ms，全量 < 200ms。
4. **可扩展**：Provider trait 让新增搜索源变成"实现一个 trait"，不动搜索主流程。
5. **健壮**：单个 Provider 失败绝不拖垮整个搜索。

### 1.1 非目标（YAGNI）

- ❌ **shell 命令搜索**（手测后新增）：launcher 场景下无终端上下文（无 cwd/环境继承）+ 输出无展示区，伪需求。已移除。
- ❌ Finder 文件标签（tag）搜索：macOS 独有，跨平台性差，用户实际不用。取消。
- ❌ 浏览器标签页（tabs）搜索：需浏览器扩展，复杂度高，不在本期。
- ❌ websearch provider（联网搜索）：fallback 场景，本期不做。
- ❌ clipboard provider：独立大功能，另立 spec。
- ❌ AI 问答 fallback：另立 spec。
- ❌ Arc/Brave/Vivaldi 浏览器：长期支持，本期只做 Chrome/Edge（含多 profile）+ Safari + Firefox。
- ❌ 前端 Tab/结果渲染重构为 wox 式分组：保持现有 Tab 栏 + 列表，只改数据流。

## 2. 架构总览

### 2.1 Provider trait

```rust
// crates/search/src/provider.rs（新增）
use async_trait::async_trait;

#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Provider 唯一标识，对应 SearchResult.source
    fn id(&self) -> &'static str;

    /// 该 Provider 响应哪些 tab。"all" 总是包含（由调用方保证）。
    fn matches_tab(&self, tab: &str) -> bool;

    /// 执行搜索。契约：绝不返回 Err，失败时返回空 vec。
    async fn search(&self, query: &str, ctx: &SearchContext) -> Vec<SearchResult>;

    /// 是否参与频次加权。calculator/url 等确定性/无频次意义的返回 false。
    fn uses_frequency(&self) -> bool { true }

    /// 是否作为 fallback（无结果时兜底）。本期预留，无 Provider 启用。
    fn is_fallback(&self) -> bool { false }
}

/// 各 Provider 共享的只读上下文。
pub struct SearchContext<'a> {
    pub app_index: &'a parking_lot::RwLock<AppIndex>,
    pub bookmarks: &'a parking_lot::RwLock<Vec<BookmarkEntry>>,
    pub frequency: &'a FrequencyScorer,
    pub tab: &'a str,  // 当前 tab（Provider 可据此调整行为，如 ShellProvider 已移除但保留设计）
}
```

### 2.2 SearchEngine 重构

从"持有 6 个 source 的散字段"变为"持有 `Vec<Box<dyn SearchProvider>>`"：

```rust
pub struct SearchEngine {
    providers: Vec<Box<dyn SearchProvider>>,
    app_index: parking_lot::RwLock<AppIndex>,       // 仍保留供后台刷新
    bookmarks: parking_lot::RwLock<Vec<BookmarkEntry>>,
    frequency: FrequencyScorer,
}

/// 单次 search 返回的最大**总**结果数（跨所有 Provider 合并后）。
/// 这是"可滚动浏览的总量"，不是"一屏可视行数"——前端窗口高度由前端的
/// MAX_VISIBLE_RESULTS（10 行）+ overflow-y-auto 滚动容器控制，与本常量无关。
/// 设 30：足够滚动浏览多个 Provider 的结果，又不过载。
const MAX_TOTAL_RESULTS: usize = 30;

impl SearchEngine {
    /// 旧 API 保留（诊断/测试）：聚合所有 Provider 一次返回。
    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        let ctx = SearchContext { app_index: &self.app_index, bookmarks: &self.bookmarks,
                                  frequency: &self.frequency, tab };
        let futures = self.providers.iter()
            .filter(|p| tab == "all" || p.matches_tab(tab))
            .map(|p| p.search(query, &ctx));
        let batches = futures::future::join_all(futures).await;
        let mut all: Vec<SearchResult> = batches.into_iter().flatten().collect();
        self.frequency.boost(&mut all, query);
        all.sort_by(|a, b| b.score.cmp(&a.score));
        all.truncate(MAX_TOTAL_RESULTS);
        all
    }

    /// 新 API：流式。每个 Provider 完成立即 emit 一批全局 top-N。
    pub async fn search_streaming<F>(
        &self, query: &str, tab: &str, run_id: &str, mut emit: F,
    ) where F: FnMut(SearchBatch) {
        let ctx = SearchContext { /* ... tab ... */ };
        let active: Vec<_> = self.providers.iter()
            .filter(|p| tab == "all" || p.matches_tab(tab)).collect();
        // FuturesUnordered：单 task 并发，先完成先 yield（不 spawn，避免 ctx 生命周期问题）
        let mut futs = active.into_iter()
            .map(|p| p.search(query, &ctx))
            .collect::<futures::stream::FuturesUnordered<_>>();
        let mut collected: Vec<SearchResult> = Vec::new();
        while let Some(batch) = futs.next().await {
            // ⚠️ 关键：boost 只对新 batch 加权一次，不对累积的 collected 重复 boost
            // （boost 是加法性的 score += X，对已 boost 的再 boost 会重复加权）
            self.frequency.boost(&mut batch, query);  // ← per-batch boost
            collected.extend(batch);
            collected.sort_by(|a, b| b.score.cmp(&a.score));
            collected.truncate(MAX_TOTAL_RESULTS);
            emit(SearchBatch { run_id: run_id.to_string(), results: collected.clone() });
        }
    }
}
```

**实现注（已验证）**：`FuturesUnordered` 在单 task 内轮询多个 future，ctx 借用 `&self.{app_index,bookmarks,frequency}`，生命周期覆盖整个 while 循环，borrow checker 接受——无需 Arc / spawn。**采纳此方案**。

**流式 boost 修正（review 抓到的 Critical）**：原设计 `boost(&mut collected, query)` 对累积 vec 重复 boost——boost 是 `score += X` 加法性，先完成的 Provider 结果被多次加权。**修正为 per-batch boost**：每个新 batch 进来先独立 boost 一次，再 extend 到 collected。这样每条结果只加权一次，流式最终排序与非流式 `search()` 一致（回归测试 `streaming_boost_applied_once_not_per_round` 锁住）。

### 2.3 数据流

```
前端 invoke("search_stream", {query, tab, runId})
    ↓
search_stream 命令（search_commands.rs）
    ↓
engine.search_streaming(query, tab, runId, |batch| app.emit("search://batch", batch))
    ↓
FuturesUnordered 并发跑各 Provider
    ↓ 每个 Provider 完成
    ↓
收集 → per-batch frequency.boost → sort → truncate(30) → emit("search://batch", {runId, results})
    ↓ 所有 Provider 完成
emit("search://done", {runId})
    ↓
前端 listen("search://batch") → runId 匹配则 setSearchResults(payload.results)
前端 listen("search://done") → unlisten + 清理
```

## 3. 流式渐进渲染契约

### 3.1 后端 Tauri 命令

```rust
// crates/desktop/src/search_commands.rs
#[tauri::command]
pub async fn search_stream(
    query: String,
    tab: String,
    run_id: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let engine = octopus_search::get_engine().ok_or("engine not init")?;
    let emit_batch = move |batch: SearchBatch| {
        let _ = app.emit("search://batch", &batch);
    };
    engine.search_streaming(&query, &tab, &run_id, emit_batch).await;
    let _ = app.emit("search://done", &serde_json::json!({"runId": run_id}));
    Ok(())
}

// search_all 保留（诊断用，内部仍调 engine.search 聚合版）
```

事件 payload：
```rust
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBatch {
    pub run_id: String,
    pub results: Vec<SearchResult>,  // 全局 top-30（后端已加权+排序+截断）
}
```

**排序在后端**：每次 emit 的 `results` 是"截至当前所有已完成 Provider 的全局 top-30"，前端零排序逻辑，直接 `setSearchResults(payload.results)` 整体替换。

### 3.2 前端契约

```ts
// crates/desktop/frontend/src/pages/ActionBar/searchStream.ts（新增）
let currentRunId: string | null = null;
let unlistenBatch: UnlistenFn | null = null;
let unlistenDone: UnlistenFn | null = null;

export async function executeSearchStream(
  query: string, tab: TabId,
  onResults: (results: SearchResult[]) => void,
) {
  // 取消旧监听（防串扰）
  unlistenBatch?.(); unlistenDone?.();
  currentRunId = crypto.randomUUID();
  const myRunId = currentRunId;

  unlistenBatch = await listen<SearchBatch>("search://batch", (e) => {
    if (e.payload.runId !== myRunId) return;  // 旧批次丢弃
    onResults(e.payload.results);
  });
  // done 事件也校验 runId——否则旧搜索的 done 会 tear down 新搜索的 batch listener
  unlistenDone = await listen<{ runId: string }>("search://done", (e) => {
    if (e.payload.runId !== myRunId) return;
    unlistenBatch?.(); unlistenDone?.();
    unlistenBatch = null; unlistenDone = null;
  });
  await invoke("search_stream", { query, tab, runId: myRunId });
}
```

`index.tsx` 改造：原 `executeSearch`（调 `search_all`）替换为 `executeSearchStream`，`onResults` 回调直接 `setSearchResults`。

### 3.3 防串扰不变量

- **run_id 唯一**：每次搜索 `crypto.randomUUID()`。
- **旧监听即弃**：新搜索发起时立即 `unlisten()` 旧的 + 用闭包捕获的 `myRunId` 二次校验 payload。
- **batch + done 双校验**：`search://batch` 和 `search://done` 的 listener 都校验 `payload.runId !== myRunId`（review 抓到的竞态：旧搜索的 done 事件会 tear down 新搜索的 batch listener，导致慢 Provider 结果被丢弃）。
- **单事件名**：`search://batch`（不带 run_id 后缀，避免 run_id 含特殊字符），payload 字段区分。
- **完成即清理**：`search://done` 触发 unlisten，防内存泄漏。

## 4. Provider 设计

### 4.1 Provider 清单

| Provider | source | matches_tab | uses_frequency | 本期改动 |
|---|---|---|:---:|---|
| AppProvider | `app` | all/apps/quick | ✅ | 从 engine.rs 搬出为独立 Provider，+2000 权重保留 |
| FileProvider | `file` | all/files/files_bookmarks | ✅ | mdfind，无大改，包成 Provider |
| MenuProvider | `menu`+`quicklink` | all/quick/actions | ✅ | 合并现有 `search_menus_and_quicklinks` + `search_quicklink_keywords` |
| BookmarkProvider | `bookmark` | all/bookmarks/files_bookmarks | ✅ | **新增 Safari (plist) + Firefox (places.sqlite) + Chrome/Edge 多 profile 扫描** |
| CalculatorProvider | `calculator` | all | ❌ | 新增：evalexpr 求值（整数字面量升 Float 让 `10/4=2.5`） |
| UrlProvider | `url` | all | ❌ | 新增：检测合法 URL |

### 4.2 ShellProvider 已移除（2026-07-16 手测后）

**原设计**：曾实现 ShellProvider（裸命令透传 + 55 条命令补全 + zsh_history 历史匹配）。**手测后用户判定 launcher 场景下 shell 是伪需求**：
- 无终端上下文（`sh -c` 从 home 起步，无 cwd / 环境继承，`cd` 无意义）
- 输出无展示区（execute_shell 执行了但结果被丢弃 + 窗口立刻 dismiss，用户看不到输出）
- 唯一"能用"的无输出命令（`open`/`pbcopy`）用户更习惯在终端做

**移除范围**：search crate 的 `shell.rs`/`shell_commands.rs`/`shell_history.rs`（git rm）+ engine.rs 的 ShellProvider 装配 + 2 个 shell 测试；desktop 的 `execute_shell` 命令 + 注册；前端的 shell Tab + `case "shell"` 分支（保留防御性空 case 兜底历史频次残留）+ `isShellMode`/`extractShellCommand` 辅助函数 + 相关测试。Tab 从 6 个减为 5 个（all/apps/files/bookmarks/actions）。

### 4.3 BookmarkProvider（重点）

```rust
pub struct BookmarkProvider;
impl BookmarkProvider {
    fn load_all(&self, bookmarks: &[BookmarkEntry]) -> Vec<SearchResult> { ... }
}
```
`load_all_bookmarks()`（`bookmark.rs`）扩展支持多 profile + Safari + Firefox：

**Chrome/Edge 多 profile 扫描**（用户反馈触发的修复，Task 8 遗漏）：
旧代码硬编码读 `Default/Bookmarks`，但多账号/迁移用户的书签常在 `Profile 1`（用户实测主 profile，261 条书签在 `Profile 1/Bookmarks`，无 `Default`）。`load_chromium_all_profiles` 扫 User Data 下所有 profile 目录（`Default`/`Profile 1`/`Profile 2`/...）的 `Bookmarks`，跳过 `Guest Profile`/`System Profile`，合并后跨 profile 按 url 去重。

**Safari**（plist 二进制，需 `plist` crate + Full Disk Access）：
```rust
fn load_safari_bookmarks() -> Vec<BookmarkEntry> {
    let path = dirs::home_dir()?.join("Library/Safari/Bookmarks.plist");
    let plist = plist::Value::from_file(&path).ok()?;  // 失败（无权限）返回 None
    let mut result = vec![];
    walk_safari(&plist, &mut result);  // 递归 Children，type=="url" 取 URIDictionary.urlString + Title
    result
}
```
Safari plist 结构（`WebBookmarkType` 系）：
- 根 dict 含 `Children` 数组
- 每个 child：`WebBookmarkType == "WebBookmarkTypeLeaf"` 时有 `URIDictionary.urlString` + `title`
- `WebBookmarkType == "WebBookmarkTypeList"` 时有 `Children` 递归

**Firefox**（SQLite，不依赖 plist）：
```rust
fn load_firefox_bookmarks() -> Vec<BookmarkEntry> {
    // 1. 找 profile：~/Library/Application Support/Firefox/Profiles/*.default-release/
    let profiles_dir = dirs::home_dir()?.join("Library/Application Support/Firefox/Profiles");
    let profile = fs::read_dir(&profiles_dir).ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().ends_with(".default-release"))?;
    let places = profile.path().join("places.sqlite");

    // 2. 拷到临时文件（避免锁 Firefox 运行中的 DB）
    let tmp = std::env::temp_dir().join(format!("ff_places_{}.db", std::process::id()));
    fs::copy(&places, &tmp).ok()?;

    // 3. 只读打开 + 查询
    let conn = rusqlite::Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT b.title, p.url FROM moz_bookmarks b
         JOIN moz_places p ON b.fk = p.id
         WHERE b.type = 1 AND p.url NOT LIKE 'place:%'"
    )?;
    // type=1 = bookmark，过滤 place:% 内部 schema URL
    let rows = stmt.query_map([], |row| BookmarkEntry {
        title: row.get::<_, String>(0)?,
        url: row.get::<_, String>(1)?,
        browser: "Firefox".into(),
    })?;
    // 拷贝的临时文件不主动删（OS tmp 会清；进程退出即失效）
    rows.filter_map(|r| r.ok()).collect()
}
```

**新增依赖**：`plist` crate（search crate）+ `rusqlite`（search crate，与 infra 同版本，避免重复链接）。

### 4.4 CalculatorProvider

```rust
// crates/search/src/providers/calculator.rs
pub struct CalculatorProvider;

#[async_trait]
impl SearchProvider for CalculatorProvider {
    fn id(&self) -> &'static str { "calculator" }
    fn matches_tab(&self, tab: &str) -> bool { tab == "all" }  // 仅 all tab
    fn uses_frequency(&self) -> bool { false }

    async fn search(&self, query: &str, _) -> Vec<SearchResult> {
        let q = query.trim();
        if !looks_like_expression(q) { return vec![]; }
        // evalexpr 求值（safe，无自定义函数）
        match evalexpr::eval(q) {
            Ok(val) => {
                let num_str = format_value(&val);  // 1+2 → "3"，1/3 → "0.3333"
                vec![SearchResult {
                    source: "calculator".into(),
                    title: format!("= {}", num_str),
                    subtitle: "计算结果".into(),
                    icon: None,
                    action_type: "copy".into(),  // 新 action_type
                    action_data: json!({ "text": num_str }).to_string(),
                    score: 10000,
                }]
            }
            Err(_) => vec![],
        }
    }
}

fn looks_like_expression(s: &str) -> bool {
    // 至少含一个运算符，且全部字符是数字/运算符/括号/空格/小数点
    let has_op = s.chars().any(|c| matches!(c, '+' | '-' | '*' | '/' | '%'));
    let all_valid = s.chars().all(|c|
        c.is_ascii_digit() || matches!(c, '+' | '-' | '*' | '/' | '%' | '(' | ')' | '.' | ' ')
    );
    has_op && all_valid && !s.ends_with(|c: char| matches!(c, '+' | '-' | '*' | '/'))
}
```

**新 action_type: "copy"**：calculator 结果回车 = 复制到剪贴板。前端 `executeSearchResult` 加分支。

**浮点除法修正（review 驱动）**：evalexpr 11 对 `Int/Int` 做整数除法（`10/4 → 2`），用户期望 JS/Python3 风格（`10/4 → 2.5`）。原方案 `1.0*(expr)` 对 `1.0*(10/4)=2.0` 仍错（括号内先整数除法）。**实际采用"整数字面量升 Float"**：`promote_int_literals_to_float` 扫描表达式把整数字面量 `10` → `10.0`，对所有算式一致正确（`5-10/4=2.5`、`2*3/4=1.5`）。额外处理浮点化后 `1/0 → inf` 被 `is_finite()` 过滤，保持除零返回空的原语义。

### 4.5 UrlProvider

```rust
// crates/search/src/providers/url.rs
pub struct UrlProvider;

#[async_trait]
impl SearchProvider for UrlProvider {
    fn id(&self) -> &'static str { "url" }
    fn matches_tab(&self, tab: &str) -> bool { tab == "all" }
    fn uses_frequency(&self) -> bool { false }

    async fn search(&self, query: &str, _) -> Vec<SearchResult> {
        let q = query.trim();
        if !looks_like_url(q) { return vec![]; }
        let url = if q.starts_with("http") { q.to_string() }
                  else { format!("https://{}", q) };
        vec![SearchResult {
            source: "url".into(),
            title: format!("打开 {}", q),
            subtitle: "网址".into(),
            icon: None,
            action_type: "url".into(),
            action_data: json!({ "url": url }).to_string(),
            score: 9000,
        }]
    }
}

fn looks_like_url(s: &str) -> bool {
    // 含点 + 顶级域名片段，或 http(s):// 开头
    // 例：github.com / api.example.co.jp / http://localhost:3000
    (s.starts_with("http://") || s.starts_with("https://"))
    || (s.contains('.') && {
        let last = s.rsplit('.').next().unwrap_or("");
        last.len() >= 2 && last.chars().all(|c| c.is_ascii_alphabetic())
    })
}

// **已知假阳性**（本期接受）：`hello.world.txt`/`report.final.pdf` 这类"看起来像域名"
// 的输入也会触发 URL 项。launcher 场景下用户输了就当想打开，且 URL 项 score=9000 低于
// app/file 精确匹配，不会抢占主结果位。如需更严，可加公共 TLD 白名单，但 YAGNI。
```

### 4.6 MenuProvider（合并现有两个 fn）

现有 `search_menus_and_quicklinks` + `search_quicklink_keywords` 合并进 `MenuProvider::search`，逻辑不变（一次 DB 读，两个分支产出结果）。source 仍区分 `menu`/`quicklink`（前端按 source 显示 badge）。

### 4.7 word_prefix_match 匹配增强（用户反馈修复 2）

**问题**：用户反馈"Chrome 在列表中但跌出前 10"。根因：`prefix_match` 只检查 target **整体**前缀，"Google Chrome" 搜 "chrome" 命中失败（不以 "chrome" 开头）→ 落 fuzzy（~500）+ 2000 加权 ≈ 2500，被书签 prefix（~5000）压下挤出 top-10。

**修复**：新增 `word_prefix_match`——按空格/连字符/斜杠/点分词后，检查 query 是否匹配**任意一个词**的开头。打分 base 4500（介于 prefix 5000 和 pinyin 4000 之间）。"Google Chrome" → "Chrome" 词匹配 → 4495 + 2000 加权 = 6495 稳压书签。

```rust
pub fn word_prefix_match(query: &str, target: &str) -> Option<Score> {
    let query_lower = query.to_lowercase();
    target.split(|c: char| !c.is_alphanumeric())  // 按非字母数字分词
        .filter(|w| !w.is_empty())
        .filter_map(|word| {
            let word_lower = word.to_lowercase();
            if word_lower.starts_with(&query_lower) {
                let remaining = word.chars().count().saturating_sub(query.chars().count());
                Some(4500 - remaining as Score)
            } else { None }
        })
        .max()
}
```

`match_score` 升级为 **exact > prefix > word-prefix > pinyin > fuzzy** 五级。

## 5. 频次加权

### 5.1 DB schema（v35）

```sql
CREATE TABLE IF NOT EXISTS search_frequency (
    score_key TEXT NOT NULL,            -- 稳定标识
    query TEXT NOT NULL DEFAULT '',     -- 触发查询（完全匹配加分）
    hit_count INTEGER NOT NULL DEFAULT 0,
    last_hit_ts INTEGER NOT NULL DEFAULT 0,  -- unix 秒
    PRIMARY KEY (score_key)
);
```
迁移：`db.rs` 加 v35 分支（当前最新 v34），`CREATE TABLE IF NOT EXISTS`。

### 5.2 ScoreKey 设计

借鉴 wox：用 `source + "|" + 稳定字段`，**不用 title**（title 随本地化/动态文案变）。

| source | score_key |
|---|---|
| app | `app\|/Applications/Chrome.app`（用 path） |
| file | `file\|/path/to/file.txt`（用路径） |
| menu/quicklink | `menu\|<db_id>` / `quicklink\|<db_id>` |
| bookmark | `bookmark\|<url>` |
| calculator/url | 不加权（`uses_frequency()=false`） |

### 5.3 加分公式（`FrequencyScorer::boost`，每次 emit 前）

```rust
fn boost(&self, results: &mut [SearchResult], query: &str) {
    let freqs = self.db.load_all_frequency().unwrap_or_default();  // 启动时加载内存
    let now = unix_now();
    for r in results.iter_mut() {
        if !provider_uses_frequency(&r.source) { continue; }
        let key = make_score_key(r);
        if let Some(f) = freqs.get(&key) {
            let days_ago = (now - f.last_hit_ts) / 86400;
            let base = match days_ago {
                0 => 3000,
                1 => 2000,
                2..=3 => 1000,
                4..=7 => 500,
                _ => 0,
            };
            let count_factor = f.hit_count.min(5) as i32;
            r.score += base * count_factor;
            if f.query.eq_ignore_ascii_case(query) && !query.is_empty() {
                r.score += 500;
            }
        }
    }
}
```
- 简化 wox 斐波那契 `[5,8,13,21,34,55,89]` 为 4 档（0/1/2-3/4-7 天）。
- `count_factor` 上限 5 防刷。
- 频次表内存缓存（启动加载），写时同步刷 DB。

### 5.4 记录时机

**执行动作时记录**（前端 `executeSearchResult` 触发）：

```rust
#[tauri::command]
pub async fn record_search_hit(
    source: String, action_type: String, action_data: String, query: String,
) -> Result<(), String> {
    let engine = get_engine().ok_or("not init")?;
    let result = SearchResult { source, action_type, action_data, /* title 等不用 */ };
    engine.record_frequency(&result, &query);  // 后端内部 make_score_key
    Ok(())
}
```
前端 `executeSearchResult`（`index.tsx`）在 switch 之前 fire-and-forget：
```ts
invoke("record_search_hit", {
  source: result.source, actionType: result.actionType,
  actionData: result.actionData, query: queryRef.current,
}).catch(() => {});
```
**ScoreKey 前后端一致性**：前端传整个 result 对象，后端 `make_score_key` 算 key 并 record——避免前后端重复实现导致不一致。

**已知技术债（Minor）**：quicklink 关键词触发的 score_key 含 url（含替换后的 query），不同 query 产生不同 key，频次不累积——低频场景，后续可优先 id 字段。`record` 每次 upsert 后全表 reload 内存 map——性能优化空间。

## 6. 降级路径与错误处理

### 6.1 Provider 契约不变量

> `Provider::search` 的契约是**绝不返回 Err**——只返回 `Vec<SearchResult>`（空 vec = 失败/无匹配）。

这样 `FuturesUnordered` 并发不会因一个 Provider 的 `?` 提前返回。每个 Provider 内部 `match`/`map_err(|_| vec![])` 吞掉错误，log warn。

### 6.2 各 Provider 降级

| Provider | 失败场景 | 降级 |
|---|---|---|
| FileProvider | mdfind 超时/不存在 | 返回空，log warn |
| BookmarkProvider-Safari | 无 Full Disk Access / plist 解析失败 | 返回空，log warn（不 crash、不弹窗） |
| BookmarkProvider-Firefox | places.sqlite 被锁 / 无 profile / 拷贝失败 | 返回空 |
| BookmarkProvider-Chromium | 无 profile / Bookmarks 文件损坏 | 返回空 |
| CalculatorProvider | 表达式不合法 | 返回空（静默） |
| UrlProvider | 非 URL | 返回空 |
| **search_streaming 整体** | 某 Provider future panic | `FuturesUnordered` 的 future 内用 `catch_unwind` 包装（或 AssertUnwindSafe），panic 时该 Provider 贡献空 vec，不影响其他 |

### 6.3 频次加权降级

频次是"锦上添花"，非必需：
- DB 读 `search_frequency` 失败 → 跳过加权（用原始 score）
- DB 写（record 失败） → log warn，不影响搜索
- 加权纯算术，不 panic

### 6.4 流式降级

- search_stream 命令 panic → Tauri 命令返回 Err，前端 catch 显示"搜索失败"（或回退到空结果）
- Provider 全部失败 → emit 空批次 → 前端显示空列表（与当前空结果体验一致）

## 7. 前端改动

### 7.1 searchTypes.ts

```ts
// SearchResult.source
source: "app" | "file" | "menu" | "quicklink" | "bookmark" | "calculator" | "url";
// action_type
actionType: "launch_app" | "open_file" | "menu" | "url" | "copy";
// TabId（shell 已移除）
type TabId = "all" | "apps" | "files" | "bookmarks" | "actions";
// SearchBatch（流式批次事件 payload）
interface SearchBatch { runId: string; results: SearchResult[]; }
```

### 7.2 index.tsx

- `executeSearch`（调 search_all）→ `executeSearchStream`（调 search_stream + listen）
- **防抖按 tab 分流**：files/bookmarks/files_bookmarks tab（走 mdfind，慢）150ms 防抖；其他 tab（含 all）0ms 即时——all tab 虽跑 mdfind 但后端流式扇出，快 Provider 先 emit，mdfind 慢的后追加，首屏 < 30ms。
- `executeSearchResult` 加 `"copy"` 分支：`navigator.clipboard.writeText(actionData.text)`
- `executeSearchResult` switch 之前 fire-and-forget `record_search_hit`（频次加权记录）
- 组件卸载 `useEffect(() => () => cleanupSearchStream(), [])` 防 listener 泄漏

### 7.3 Tab 栏

5 个：all/apps/files/bookmarks/actions（shell tab 已随 shell 功能移除）。calculator/url 只在 "all" tab 出现（后端 `matches_tab` 返回 false，仅由 search() 的 `tab=="all"` 兜底），不新增 Tab。

### 7.4 capabilities/default.json

无新窗口，复用 ActionBar 窗口。无需改 capabilities（事件 listen 已允许）。

## 8. 测试策略

### 8.1 Provider 单测（每个 Provider 独立）

```rust
// calculator
#[tokio::test] async fn calc_basic_arithmetic()  // "1+2" → "= 3"
#[tokio::test] async fn calc_division_by_zero_returns_empty()
#[tokio::test] async fn calc_non_expression_returns_empty()  // "abc" → empty
#[tokio::test] async fn calc_float_result()  // "10/4" → "= 2.5"（整数字面量升 Float）
#[tokio::test] async fn calc_integer_result_no_decimal()  // "1+2" → "= 3" 不是 "3.0"

// url
#[tokio::test] async fn url_domain_detected()  // "github.com"
#[tokio::test] async fn url_non_domain_rejected()  // "hello" / "中文" → empty
#[tokio::test] async fn url_known_false_positive_accepted()  // "report.pdf" → 出 URL 项（已知假阳性，本期接受）

// matcher（word-prefix 增强）
#[test] fn word_prefix_match_non_first_word()  // "chrome" 匹配 "Google Chrome"
#[test] fn word_prefix_match_partial_word()    // "chr" 匹配 "Google Chrome"
#[test] fn word_prefix_match_rejects_non_prefix()  // "hrome" 不匹配
#[test] fn match_score_google_chrome_chrome_outranks_bookmark_fuzzy()  // 核心场景回归

// bookmark-safari（fixture plist）
#[test] fn safari_plist_parsed_from_fixture()
#[test] fn safari_nonexistent_returns_empty()

// bookmark-firefox（fixture places.sqlite）
#[test] fn firefox_places_query_from_fixture()

// bookmark-chromium 多 profile
#[test] fn chromium_multiple_profiles_scanned()  // Default + Profile 1 + 跳过 Guest
#[test] fn chromium_only_profile1_no_default()    // 用户实测场景：只有 Profile 1
```

### 8.2 SearchEngine 集成测试

```rust
#[tokio::test]
async fn streaming_boost_applied_once_not_per_round() {
    // review 抓到的 Critical：流式 boost 重复加权回归测试。
    // 两个 MockAppProvider + 非空 frequency 数据，断言流式最后 emit 的每条 score
    // == 非流式 search() 的对应 score（boost 只加一次）。
    // buggy 状态下流式分数 > 非流式（先完成的被重复 boost）。
}

#[tokio::test]
async fn streaming_emits_at_least_once() { /* 至少 emit 一次 */ }
#[tokio::test]
async fn streaming_empty_query_emits_once_empty() { /* empty query → emit 一次空 batch */ }
```

### 8.3 频次加权回归

```rust
#[test]
fn frequency_today_outranks_week_ago() {
    // 同 score_key，last_hit 今天 vs 7 天前，加权后今天的高
}
#[test]
fn frequency_query_exact_match_bonus() {}
```

### 8.4 现有测试保留

`engine.rs` 现有测试（empty/quick_tab/files_bookmarks_tab 等）适配新架构，shell 相关测试已随 shell 功能移除。重构不应改变非 shell 行为。

### 8.5 编译验证

```bash
cargo build -p octopus-search
cargo build -p octopus-desktop
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
cargo test -p octopus-search --lib
cargo test -p octopus-desktop --lib
```

## 9. 性能基线

- **首屏目标**：即时 Provider（app/menu/calc/url/bookmark）< 30ms 完成第一批 emit
- **全量目标**：含 file（mdfind）< 200ms 全部完成
- mdfind 10s 超时保留，FileProvider 独立 future 不阻塞其他源
- `FuturesUnordered` 单 task 轮询，无线程开销
- 频次表内存缓存，boost 是 O(N) 纯算术（N=MAX_TOTAL_RESULTS=30）
- **前端可视**：窗口高度 clamp 到 MAX_VISIBLE_RESULTS=10 行 + overflow-y-auto 滚动容器，30 条结果可上下键/滚轮浏览

## 10. 实施顺序（高层）

1. **infra**：DB schema v35（search_frequency 表）
2. **search crate**：Provider trait + SearchContext（含 tab 字段）+ 重构 SearchEngine（providers Vec）
3. **search crate**：FrequencyScorer + make_score_key
4. **search crate**：搬移现有 source 为 Provider（app/file/menu/bookmark）
5. **search crate**：BookmarkProvider 加 Safari + Firefox + **Chrome/Edge 多 profile 扫描**
6. **search crate**：CalculatorProvider（整数字面量升 Float）+ UrlProvider（+ evalexpr 依赖）
7. **search crate**：search_streaming（FuturesUnordered + per-batch boost + emit）
8. **desktop**：search_stream + record_search_hit Tauri 命令
9. **前端**：executeSearchStream + listen（batch + done 双校验）+ "copy" action + record_search_hit 接入
10. **matcher 增强**：word_prefix_match（exact > prefix > word-prefix > pinyin > fuzzy 五级）
11. **用户手测后修复**：record_search_hit 前端接入（Critical）+ shell all-tab 污染（后直接移除 shell）+ 结果数 10→30（滚动）+ 删 delayedResults 死状态 + 防抖按 tab 分流
12. **测试**：各 Provider 单测 + 集成 + streaming boost 回归 + multi-profile + word-prefix

详细任务分解见实施 plan。

## 11. 新增依赖

| crate | 用途 | 加到 |
|---|---|---|
| `async-trait` | Provider trait 的 async fn | search |
| `futures` | FuturesUnordered | search（已有则跳过） |
| `evalexpr` | calculator 表达式求值 | search |
| `plist` | Safari 书签解析 | search |

`rusqlite`（Firefox places）infra 已有，search 可复用或经 infra 暴露。

## 12. 不变量速查（实现时对照）

1. `Provider::search` 绝不返回 Err，失败返空 vec
2. `search_streaming` 每次 emit 的是**全局 top-30**（per-batch boost + 排序 + 截断）
3. **per-batch boost**：每个新 batch 独立 boost 一次，不对累积 collected 重复 boost（boost 加法性，重复加权是 bug）
4. run_id 防串扰：batch + done listener 都校验 payload.runId（旧 done 会 tear down 新 batch listener）
5. ScoreKey 后端算：前端传 result 对象，后端 make_score_key
6. calculator/url `uses_frequency()=false`，其他默认 true
7. calculator/url 仅 "all" tab（matches_tab 返回 false，靠 search() 的 tab=="all" 兜底）
8. Safari 无 Full Disk Access 时返回空，不 crash
9. Firefox places 必须拷临时文件读，不锁运行中 DB
10. Chrome/Edge 扫所有 profile（Default/Profile 1/...），跳过 Guest/System，跨 profile 按 url 去重
11. `MAX_TOTAL_RESULTS=30` 是可滚动总量，不是可视行数——前端窗口高度 clamp 到 `MAX_VISIBLE_RESULTS=10` 行 + overflow-y-auto
12. 现有 search_all 命令保留（诊断/测试）
13. shell 功能已移除（launcher 伪需求）——前端 case "shell" 保留防御性空 case 兜底历史频次残留
