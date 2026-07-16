# 搜索多 Provider 架构重构设计

> 2026-07-16 · 借鉴 wox 多源广播思想，重构 octopus 搜索为 Provider trait + 并发扇出 + 流式渐进渲染 + 频次加权。修复 shell/bookmark 不显示问题，新增 calculator/url 源。
>
> **状态**：实现完成（2026-07-16）。实际偏差见下方各节"实现注"。

## 0. 背景与动机

### 0.1 用户反馈

> "现在搜到的内容还比较少，只有应用和文件。另外 shell 和标签都没有匹配到的内容。"

经代码 + 环境排查，根因有三层：

1. **shell 是匹配逻辑 bug**（非 provider 缺失）：`engine.rs:75` 要求 query 以 `>` 开头。前端切到 shell tab 输入裸命令（如 `ls`）不会自动补 `>`，`filterByTab("shell")` 又把非 shell 来源全过滤掉 → 空结果。`isShellMode`/`extractShellCommand` 辅助函数已写好且有单测，但 `index.tsx` 未接入。
2. **"标签"实为浏览器收藏（bookmark）覆盖缺失**：用户用 Safari + Arc，但 octopus 只读 Chrome/Edge（用户本机均未安装），Safari 解析是占位函数返回空（`bookmark.rs:88`），Arc 未覆盖。→ bookmark tab 永远空。
3. **架构串行**：当前 `search()` 是 6 个 source 串行 `results.extend`，慢源（mdfind）拖慢整体，且无频次加权，常用项不会排前。

### 0.2 借鉴 wox 的核心思想（不照搬实现）

wox 是 38 插件、3 语言运行时、Flutter UI 的完整体系，直接照搬工作量极大且与 octopus Tauri 单进程架构不匹配。**借鉴的是架构思想**：

| wox 思想 | octopus 采纳方式 |
|---|---|
| `*` 触发词 = 全局搜索源注册 | Provider 声明 `matches_tab` |
| 并发扇出所有插件 | `tokio::spawn` 每个 Provider 独立 task |
| `FallbackSearcher` 接口 | `is_fallback()` trait 方法（本期暂不启用 fallback provider，预留） |
| 频次加权（斐波那契衰减） | 简化为 7 天滑窗 + 当次 query 加分 |
| 渐进式渲染（resultDebouncer） | Tauri 事件 + 前端 listen 增量 |
| `IgnoreAutoScore` 特性 | `uses_frequency()` trait 方法 |
| ScoreKey 稳定标识 | `source + "\|"` + action_data 稳定字段 |

**不采纳**：wox 的插件 SDK、外部进程通信、SQLite FTS5 文件引擎（octopus 继续用 mdfind）、分组（Group）显示、拼音独立 FTS 表。

## 1. 设计目标

1. **广覆盖**：从 6 个 source 扩展到 7 个 Provider，修复 shell + bookmark 让现有源真正可用，新增 calculator/url。
2. **搜得准**：频次加权让常用项排前（借鉴 wox 斐波那契衰减思想，简化实现）。
3. **搜得快**：并发扇出 + 流式渐进渲染，首屏 < 30ms，全量 < 200ms。
4. **可扩展**：Provider trait 让新增搜索源变成"实现一个 trait"，不动搜索主流程。
5. **健壮**：单个 Provider 失败绝不拖垮整个搜索。

### 1.1 非目标（YAGNI）

- ❌ Finder 文件标签（tag）搜索：macOS 独有，跨平台性差，用户实际不用。取消。
- ❌ 浏览器标签页（tabs）搜索：需浏览器扩展，复杂度高，不在本期。
- ❌ websearch provider（联网搜索）：fallback 场景，本期不做。
- ❌ clipboard provider：独立大功能，另立 spec。
- ❌ AI 问答 fallback：另立 spec。
- ❌ Arc/Brave/Vivaldi 浏览器：长期支持，本期只做 Chrome/Edge（已有）+ Safari + Firefox。
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

    /// 是否参与频次加权。shell 等时间序/命令序的返回 false。
    fn uses_frequency(&self) -> bool { true }

    /// 是否作为 fallback（无结果时兜底）。本期预留，无 Provider 启用。
    fn is_fallback(&self) -> bool { false }
}

/// 各 Provider 共享的只读上下文。
pub struct SearchContext<'a> {
    pub app_index: &'a parking_lot::RwLock<AppIndex>,
    pub bookmarks: &'a parking_lot::RwLock<Vec<BookmarkEntry>>,
    pub shell_history: &'a ShellHistoryCache,
}
```

### 2.2 SearchEngine 重构

从"持有 6 个 source 的散字段"变为"持有 `Vec<Box<dyn SearchProvider>>`"：

```rust
pub struct SearchEngine {
    providers: Vec<Box<dyn SearchProvider>>,
    app_index: parking_lot::RwLock<AppIndex>,       // 仍保留供后台刷新
    bookmarks: parking_lot::RwLock<Vec<BookmarkEntry>>,
    shell_history: ShellHistoryCache,               // 进程内缓存
    frequency: FrequencyScorer,
}

impl SearchEngine {
    /// 旧 API 保留（诊断/测试）：聚合所有 Provider 一次返回。
    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        let ctx = self.make_ctx();
        let futures = self.providers.iter()
            .filter(|p| p.matches_tab(tab))
            .map(|p| async move { p.search(query, &ctx).await });
        let batches = futures::future::join_all(futures).await;
        let mut all: Vec<SearchResult> = batches.into_iter().flatten().collect();
        self.frequency.boost(&mut all, query);
        all.sort_by(|a, b| b.score.cmp(&a.score));
        all.truncate(MAX_RESULTS);
        all
    }

    /// 新 API：流式。每个 Provider 完成立即 emit 一批。
    pub async fn search_streaming(
        &self, query: &str, tab: &str, run_id: &str,
        emit: impl Fn(SearchBatch),
    ) {
        let ctx = self.make_ctx();
        let active: Vec<_> = self.providers.iter()
            .filter(|p| p.matches_tab(tab)).collect();
        let (tx, mut rx) = tokio::sync::mpsc::channel(active.len());
        for p in active {
            let tx = tx.clone();
            let q = query.to_string();
            let ctx_ref = &ctx;  // 见实现注：跨 task 需 Arc 或改设计
            tokio::spawn(async move {
                let batch = p.search(&q, /* ctx */).await;
                let _ = tx.send((p.id(), batch)).await;
            });
        }
        drop(tx);
        // 收一批：合并到全局已收集 → 频次加权 → 排序 → truncate → emit 整表
        let mut collected: Vec<SearchResult> = Vec::new();
        while let Some((_source, batch)) = rx.recv().await {
            collected.extend(batch);
            self.frequency.boost(&mut collected, query);
            collected.sort_by(|a, b| b.score.cmp(&a.score));
            collected.truncate(MAX_RESULTS);
            emit(SearchBatch {
                run_id: run_id.to_string(),
                results: collected.clone(),
            });
        }
    }
}
```

**实现注**：`SearchContext` 含引用，跨 `tokio::spawn` 生命周期受限。实际实现改为 `SearchContext` 持有 `Arc<RwLock<...>>`（而非 `&'a`），或 `search_streaming` 用 `join_all`（不 spawn，单 task 内并发 future，不跨越引用生命周期）。**推荐 `join_all` + `FuturesUnordered`**：既并发又不用 Arc 化，且 `FuturesUnordered` 可边完成边收（流式语义）。具体：

```rust
use futures::stream::{FuturesUnordered, StreamExt};
let mut futs = active.into_iter()
    .map(|p| async move { (p.id(), p.search(query, &ctx).await) })
    .collect::<FuturesUnordered<_>>();
while let Some((_id, batch)) = futs.next().await {
    collected.extend(batch);
    // ... boost + sort + truncate + emit
}
```

`FuturesUnordered` 在单 task 内轮询多个 future，先完成的先 yield，完美匹配"边出结果边 emit"。**采纳此方案，弃用 mpsc + spawn**。

### 2.3 数据流

```
前端 invoke("search_stream", {query, tab, runId})
    ↓
search_stream 命令（search_commands.rs）
    ↓
engine.search_streaming(query, tab, runId, |batch| app.emit("search://batch", batch))
    ↓
FuturesUnordererd 并发跑各 Provider
    ↓ 每个 Provider 完成
    ↓
收集 → frequency.boost → sort → truncate(10) → emit("search://batch", {runId, results})
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
    pub results: Vec<SearchResult>,  // 整表 top-10（后端已排序+加权）
}
```

**排序在后端**：每次 emit 的 `results` 是"截至当前所有已完成 Provider 的全局 top-10"，前端零排序逻辑，直接 `setSearchResults(payload.results)`。

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
  unlistenDone = await listen("search://done", (e) => {
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
- **单事件名**：`search://batch`（不带 run_id 后缀，避免 run_id 含特殊字符），payload 字段区分。
- **完成即清理**：`search://done` 触发 unlisten，防内存泄漏。

## 4. Provider 设计

### 4.1 Provider 清单

| Provider | source | matches_tab | uses_frequency | 本期改动 |
|---|---|---|:---:|---|
| AppProvider | `app` | all/apps/quick | ✅ | 从 engine.rs 搬出为独立 Provider，+2000 权重保留 |
| FileProvider | `file` | all/files/files_bookmarks | ✅ | mdfind，无大改，包成 Provider |
| MenuProvider | `menu`+`quicklink` | all/quick/actions | ✅ | 合并现有 `search_menus_and_quicklinks` + `search_quicklink_keywords` |
| BookmarkProvider | `bookmark` | all/bookmarks/files_bookmarks | ✅ | **新增 Safari (plist) + Firefox (places.sqlite)** |
| ShellProvider | `shell` | all/shell/quick | ❌ | **修复匹配 + 命令补全表 + 读 zsh_history** |
| CalculatorProvider | `calculator` | all | ❌ | 新增：evalexpr 求值 |
| UrlProvider | `url` | all | ❌ | 新增：检测合法 URL |

### 4.2 ShellProvider（重点）

```rust
// crates/search/src/providers/shell.rs
pub struct ShellProvider {
    history: ShellHistoryCache,  // 进程内缓存
}

#[async_trait]
impl SearchProvider for ShellProvider {
    fn id(&self) -> &'static str { "shell" }
    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "all" | "shell" | "quick")
    }
    fn uses_frequency(&self) -> bool { false }  // 命令透传不参与频次加权

    async fn search(&self, query: &str, _ctx) -> Vec<SearchResult> {
        // 修复核心：剥离可选的 > 前缀（兼容旧习惯），裸命令也处理
        let cmd = query.trim_start_matches('>').trim();
        if cmd.is_empty() { return vec![]; }

        let mut results = vec![];

        // (1) 透传执行项（原行为，最高分）
        results.push(SearchResult {
            source: "shell".into(),
            title: format!("▶ {}", cmd),
            subtitle: "Shell".into(),
            icon: None,
            action_type: "shell".into(),
            action_data: json!({ "command": cmd }).to_string(),
            score: 10000,
        });

        // (2) 内置命令补全：cmd 是某 builtin 前缀时，列出补全
        let mut completions = vec![];
        for cmd_def in BUILTIN_COMMANDS.iter() {
            if cmd_def.name.starts_with(cmd) && cmd_def.name != cmd {
                completions.push(SearchResult {
                    source: "shell".into(),
                    title: format!("▶ {}", cmd_def.name),
                    subtitle: cmd_def.desc.to_string(),
                    action_type: "shell".into(),
                    action_data: json!({ "command": cmd_def.name }).to_string(),
                    score: 8000,  // 补全低于透传
                });
            }
        }
        results.extend(completions.into_iter().take(5));

        // (3) 历史匹配
        let hist_matches: Vec<_> = self.history.search(cmd).into_iter().take(5).collect();
        for hist_cmd in hist_matches {
            results.push(SearchResult {
                source: "shell".into(),
                title: format!("▶ {}", hist_cmd),
                subtitle: "历史".into(),
                action_type: "shell".into(),
                action_data: json!({ "command": hist_cmd }).to_string(),
                score: 6000,  // 历史低于补全
            });
        }

        results
    }
}
```

**BUILTIN_COMMANDS**（`crates/search/src/providers/shell_commands.rs`，硬编码约 50 条）：
```rust
pub struct CmdDef { pub name: &'static str, pub desc: &'static str }
pub static BUILTIN_COMMANDS: &[CmdDef] = &[
    CmdDef { name: "ls", desc: "列出目录" },
    CmdDef { name: "cd", desc: "切换目录" },
    CmdDef { name: "pwd", desc: "当前路径" },
    CmdDef { name: "git", desc: "版本控制" },
    CmdDef { name: "git status", desc: "查看状态" },
    CmdDef { name: "git diff", desc: "查看差异" },
    CmdDef { name: "docker", desc: "容器" },
    CmdDef { name: "cargo", desc: "Rust 包管理" },
    CmdDef { name: "npm", desc: "Node 包管理" },
    CmdDef { name: "ping", desc: "网络连通" },
    CmdDef { name: "curl", desc: "HTTP 请求" },
    // ... 约 50 条，覆盖 90% 日常
];
```
支持多词命令（`git status`），cmd 以 `git ` 开头时补全 `git status`/`git diff` 等。

**ShellHistoryCache**（进程内缓存，`crates/search/src/providers/shell_history.rs`）：
```rust
pub struct ShellHistoryCache {
    entries: once_cell::sync::OnceCell<Vec<String>>,  // 首次查询惰性加载
}

impl ShellHistoryCache {
    pub fn search(&self, query: &str) -> Vec<String> {
        let entries = self.entries.get_or_init(|| load_history_files());
        entries.iter()
            .filter_map(|h| crate::matcher::fuzzy_match(query, h).map(|_| h.clone()))
            .take(20)
            .collect()
    }
}

fn load_history_files() -> Vec<String> {
    let mut all = vec![];
    for path in &["~/.zsh_history", "~/.bash_history"] {
        if let Ok(content) = std::fs::read_to_string(shellexpand::tilde(path)) {
            all.extend(parse_zsh_history(&content));  // 解析 `: ts:0;cmd` 格式
        }
    }
    all
}
```
zsh_history 格式：`: 1234567890:0;git status`，解析取 `;` 后的 cmd 部分。bash_history 是纯命令行。惰性加载：首次 shell 查询触发，之后进程内复用，重启重载。

### 4.3 BookmarkProvider（重点）

```rust
pub struct BookmarkProvider;
impl BookmarkProvider {
    fn load_all(&self, bookmarks: &[BookmarkEntry]) -> Vec<SearchResult> { ... }
}
```
`load_all_bookmarks()`（`bookmark.rs`）扩展支持 Safari + Firefox：

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

**新增依赖**：`plist` crate（infra 或 search crate）。`rusqlite` infra 已有。

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
| shell | 不加权（`uses_frequency()=false`） |
| calculator/url | 不加权 |

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
pub async fn record_search_hit(score_key: String, query: String) -> Result<(), String> {
    let engine = get_engine().ok_or("not init")?;
    engine.frequency.record(&score_key, &query);
    Ok(())
}
```
前端 `executeSearchResult`（`index.tsx`）在执行动作前：
```ts
const scoreKey = makeScoreKey(result);  // 与后端一致
invoke("record_search_hit", { scoreKey, query: currentQuery });
```
**ScoreKey 前后端一致性**：前端 `makeScoreKey` 必须与后端 `make_score_key` 产同样字符串（同 source + 同字段拼接）。在后端暴露一个 `compute_score_key` 命令，前端调一次拿到 key 再 record，避免前后端重复实现导致不一致。**或**：前端直接传整个 result 对象，后端算 key。**采纳后者**——前端传 result，后端算 key 并 record，保证一致性。

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
| ShellProvider-history | zsh_history 不存在/无权限 | 跳过历史，只返回补全+透传 |
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
// SearchResult.source 扩展
source: "app" | "file" | "menu" | "quicklink" | "bookmark" | "shell" | "calculator" | "url";
// action_type 扩展
actionType: "launch_app" | "open_file" | "menu" | "url" | "shell" | "copy";  // +copy
```

### 7.2 index.tsx

- `executeSearch`（调 search_all）→ `executeSearchStream`（调 search_stream + listen）
- 即时/延迟搜索统一为一次 stream（后端 Provider 并发，前端不再区分 quick/delayed）
- `executeSearchResult` 加 `"copy"` 分支：`navigator.clipboard.writeText(actionData.text)`

### 7.3 Tab 栏不变

保持现有 all/apps/files/shell/bookmarks/actions。calculator/url 只在 "all" tab 出现（后端 `matches_tab` 控制），不新增 Tab。

### 7.4 capabilities/default.json

无新窗口，复用 ActionBar 窗口。无需改 capabilities（事件 listen 已允许）。

## 8. 测试策略

### 8.1 Provider 单测（每个 Provider 独立）

```rust
// shell
#[tokio::test] async fn shell_naked_command_returns_transparent_result()
#[tokio::test] async fn shell_prefix_gt_stripped()  // ">ls" == "ls"
#[tokio::test] async fn shell_completion_for_partial()
#[tokio::test] async fn shell_history_match()

// calculator
#[tokio::test] async fn calc_basic_arithmetic()  // "1+2" → "= 3"
#[tokio::test] async fn calc_division_by_zero_returns_empty()
#[tokio::test] async fn calc_non_expression_returns_empty()  // "abc" → empty

// url
#[tokio::test] async fn url_domain_detected()  // "github.com"
#[tokio::test] async fn url_non_domain_rejected()  // "hello" / "中文" → empty
#[tokio::test] async fn url_known_false_positive_accepted()  // "report.pdf" → 出 URL 项（已知假阳性，本期接受）

// bookmark-safari
#[test] fn safari_plist_parsed()  // fixture plist 文件
#[test] fn safari_no_fda_returns_empty()

// bookmark-firefox
#[test] fn firefox_places_read()  // fixture places.sqlite
```

### 8.2 SearchEngine 集成测试

```rust
#[tokio::test]
async fn concurrent_providers_merge_by_score() {
    let engine = SearchEngine::new_for_test(vec![
        Box::new(MockProvider{ id:"a", scores:[100] }),
        Box::new(MockProvider{ id:"b", scores:[200] }),
    ]);
    let r = engine.search("x", "all").await;
    assert_eq!(r[0].score, 200);
}

#[tokio::test]
async fn streaming_emits_progressively() {
    // FuturesUnordered：快 Provider 先 emit，慢 Provider 后追加
    // 注入 sleep 不同时长的 mock provider
}
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

`engine.rs` 现有 9 个测试（empty/shell_mode/quick_tab 等）适配新架构，断言不变。重构不应改变行为。

### 8.5 编译验证

```bash
cargo build -p octopus-search
cargo build -p octopus-desktop
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
cargo test -p octopus-search --lib
cargo test -p octopus-desktop --lib
```

## 9. 性能基线

- **首屏目标**：即时 Provider（app/menu/shell/calc/url/bookmark）< 30ms 完成第一批 emit
- **全量目标**：含 file（mdfind）< 200ms 全部完成
- mdfind 10s 超时保留，FileProvider 独立 future 不阻塞其他源
- `FuturesUnordered` 单 task 轮询，无线程开销
- 频次表内存缓存，boost 是 O(N) 纯算术（N=top-10）

## 10. 实施顺序（高层）

1. **infra**：DB schema v35（search_frequency 表）
2. **search crate**：Provider trait + SearchContext + 重构 SearchEngine（providers Vec）
3. **search crate**：搬移现有 6 个 source 为 Provider（app/file/menu/bookmark-shell 现状）
4. **search crate**：ShellProvider 修复（裸命令/补全/历史）
5. **search crate**：BookmarkProvider 加 Safari + Firefox
6. **search crate**：CalculatorProvider + UrlProvider（+ evalexpr 依赖）
7. **search crate**：FrequencyScorer + record_search_hit 命令
8. **search crate**：search_streaming（FuturesUnordered + emit）
9. **desktop**：search_stream Tauri 命令 + capabilities
10. **前端**：executeSearchStream + listen + "copy" action 分支
11. **测试**：各 Provider 单测 + 集成 + 回归

详细任务分解见实施 plan（下一步 writing-plans）。

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
2. `search_streaming` 每次 emit 的是**全局 top-10**（已加权+排序+截断）
3. run_id 防串扰：新搜索即弃旧监听 + payload runId 二次校验
4. ScoreKey 前后端一致：前端传 result，后端算 key
5. shell `uses_frequency()=false`，其他默认 true
6. calculator/url 仅 "all" tab，不新增 Tab
7. Safari 无 Full Disk Access 时返回空，不 crash
8. Firefox places 必须拷临时文件读，不锁运行中 DB
9. 现有 search_all 命令保留（诊断/测试）
10. 现有 engine.rs 9 个测试断言不变（行为兼容）
