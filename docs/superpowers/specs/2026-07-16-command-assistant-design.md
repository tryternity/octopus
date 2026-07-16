# 命令查阅助手 + notify-rs 文件监听设计

> 2026-07-16 · 新增"命令"Tab 作为 CLI 命令查阅助手（LLM 生成中英文描述 + fuzzy 匹配），引入 notify-rs 实时监听 app 目录变化。
>
> **状态**：设计完成（配套实施计划见 `docs/superpowers/plans/2026-07-16-command-assistant.md`）

## 0. 背景与动机

### 0.1 用户需求

> "brew / npm / cargo 等安装方式的都需要支持"
> "命令名 + 描述都参与 fuzzy 匹配——搜 'find' 能命中 fd，搜'查找'也能命中 fd"
> "可执行命令行不进入 all（避免太多噪音），单独 tab，用来查阅有多少命令，都是干什么的"

之前移除了 shell 功能（launcher 场景下执行命令是伪需求），但**查阅命令**是真实需求——用户想知道系统里有哪些 CLI 工具、各自干什么，搜到后复制命令名。

### 0.2 与 shell 功能的区别

| | shell（已移除） | 命令查阅助手（本期） |
|---|---|---|
| 目的 | 执行命令 | 查阅 + 复制命令名 |
| 终端上下文 | 需要（无 cwd/环境，伪需求） | 不需要 |
| 输出展示 | 需要（无展示区，伪需求） | 不需要 |
| 噪音 | 污染 all tab | 独立 Tab，不进 all |

## 1. 设计目标

1. **广覆盖**：扫 PATH 上所有可执行文件（brew/cargo/npm/系统命令统一覆盖，不硬编码目录）
2. **搜得准**：命令名 + 英文描述 + LLM 生成的中英文关键字都参与 fuzzy 匹配（搜"查找"命中 `fd`）
3. **不吵**：独立 "命令" Tab，不进 all（240 个命令不污染主搜索）
4. **渐进可用**：启动即用（英文描述秒级填充），LLM 后台异步补中文关键字
5. **实时感知**：notify-rs 监听 app 目录，新装/卸载 app 秒级响应（保留轮询兜底）

### 1.1 非目标（YAGNI）

- ❌ 向量化语义搜索：本期 fuzzy 够用（240 条 + 中英文关键字），预留 DB schema 升级路径
- ❌ 文件内容索引：另立 spec
- ❌ 执行命令：查阅 + 复制，不执行（避坑 shell 的伪需求）
- ❌ 独立 LLM 配置：复用现有 polish LLM 配置

## 2. 架构

### 2.1 数据流

```
启动 → CommandIndex::scan()
  ├─ 解析 $PATH → 各目录 read_dir 收集可执行文件
  ├─ whats/brew desc 填英文描述（本地，秒级）
  ├─ 读 DB 缓存（已生成 keywords 的直接用）
  └─ 返回内存索引

后台 LLM 线程（启动后异步）
  ├─ 找 keywords 为空的命令
  ├─ 批量调 chat_text_with_prompt（spawn_blocking，每命令 1 次）
  ├─ 生成 "中英文关键字" 存 DB
  └─ 刷新内存索引

搜索 → CommandProvider (source="command")
  ├─ matches_tab: "commands"（不进 all）
  ├─ match_score(query, name) ∨ match_score(query, keywords) ∨ match_score(query, description)
  └─ action_type="copy"（回车复制命令名到剪贴板）

文件监听 → notify-rs watcher
  ├─ 监听 /Applications 等（Create/Remove 事件）
  ├─ debounce 3s → refresh_app_index()
  └─ fallback: 保留 2 分钟数量轮询
```

### 2.2 CommandIndex（仿 AppIndex 模式）

```rust
pub struct CommandEntry {
    pub name: String,           // "fd"
    pub path: String,           // "/opt/homebrew/bin/fd"
    pub source: String,         // "brew" | "cargo" | "system" | "path"
    pub description: String,    // "Simple, fast alternative to find"（英文，whatis/brew desc）
    pub keywords: String,       // "查找 文件 搜索 find files"（LLM 生成）
}

pub struct CommandIndex {
    pub commands: Vec<CommandEntry>,
}
```

**PATH 扫描**：`std::env::var("PATH")` → `split(':')` → 每个目录 `read_dir` 收集可执行文件（跳过目录/非执行权限）。source 判定：
- 路径含 `/homebrew/` 或 `/linuxbrew/` → `"brew"`
- 路径含 `.cargo/bin` → `"cargo"`
- 路径是 `/usr/bin` `/bin` `/usr/sbin` `/sbin` → `"system"`
- 其余 → `"path"`

**英文描述填充**（秒级，本地，无网络）：
- `whatis <cmd>`（读 man page 索引，`/usr/share/whatis`）—— 覆盖系统命令 + 部分 brew
- brew 命令额外 `brew desc <name>` —— brew 专属描述更准
- 都没有 → description 为空（靠命令名匹配）

**去重**：同名命令（如 `/usr/bin/python3` 和 `/opt/homebrew/bin/python3`）保留 PATH 顺序靠前的（`$PATH` 前面的优先级高）。

**DB 缓存**（统一 launcher_index 表）：读 `launcher_index WHERE type='command'` 拿 keywords 缓存；扫描后全量替换 `type='command'` 的行（`save_launcher_batch("command", ...)`）。app 走 `type='app'`，同一张表。

### 2.3 LLM 关键字生成

```
system: "你是命令行工具专家。为给定命令生成简短的中英文搜索关键字，用空格分隔。
         只输出关键字，不要解释。包含：命令功能、同义词、中文翻译。限 30 字以内。"

user: "命令: fd\n英文描述: Simple, fast alternative to find"

预期输出: "查找 文件 搜索 find files search filesystem"
```

- 复用 `octopus_llm::chat_text_with_prompt`（blocking HTTP，需 `spawn_blocking`）
- 复用 `config::llm_config_ignore_mode(&config)` 拿 LLM 配置（无需新配置字段）
- **LLM 未配置时**：keywords 为空，fuzzy 仍能匹配命令名 + 英文 description
- **后台批量**：启动后异步线程逐个生成（每命令 1 次 API 调用，间隔 500ms 防限流），生成一个存 DB 一个 + 刷新内存

### 2.4 CommandProvider

```rust
pub struct CommandProvider;

#[async_trait]
impl SearchProvider for CommandProvider {
    fn id(&self) -> &'static str { "command" }
    fn matches_tab(&self, tab: &str) -> bool { matches!(tab, "commands") }
    fn uses_frequency(&self) -> bool { false }

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let cmds = ctx.command_index.read();
        let mut scored: Vec<(Score, &CommandEntry)> = cmds.commands.iter()
            .filter_map(|cmd| {
                // name > keywords > description，取最高分
                let score = match_score(query, &cmd.name)
                    .or_else(|| match_score(query, &cmd.keywords))
                    .or_else(|| match_score(query, &cmd.description))?;
                Some((score, cmd))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(20).map(|(score, cmd)| SearchResult {
            source: "command".into(),
            title: cmd.name.clone(),
            subtitle: if cmd.keywords.is_empty() { cmd.description.clone() }
                      else { cmd.keywords.clone() },
            icon: None,
            action_type: "copy".into(),
            action_data: serde_json::json!({ "text": cmd.name }).to_string(),
            score,
        }).collect()
    }
}
```

### 2.5 notify-rs 文件监听

```rust
// crates/desktop/src/file_watcher.rs
use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config, EventKind};

pub fn start_app_watcher() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => { log::warn!("[file_watcher] init failed: {}, fallback to polling", e); return; }
    };
    for dir in &["/Applications", "/System/Applications", "/Applications/Utilities"] {
        let _ = watcher.watch(std::path::Path::new(dir), RecursiveMode::Recursive);
    }
    if let Some(home) = dirs::home_dir() {
        let _ = watcher.watch(&home.join("Applications"), RecursiveMode::Recursive);
    }
    std::thread::spawn(move || {
        let mut last_trigger = std::time::Instant::now();
        for ev in rx {
            if let Ok(e) = ev {
                if matches!(e.kind, EventKind::Create(_) | EventKind::Remove(_)) {
                    // debounce 3s——安装 app 触发大量事件
                    if last_trigger.elapsed() > std::time::Duration::from_secs(3) {
                        last_trigger = std::time::Instant::now();
                        if let Some(engine) = octopus_search::get_engine() {
                            let n = engine.refresh_app_index();
                            log::info!("[file_watcher] app 目录变化，重扫: {} 个应用", n);
                        }
                    }
                }
            }
        }
    });
}
```

- **保留 2 分钟数量轮询**（main.rs 现有逻辑）作为 fallback——macOS FSEvents 对非用户所有文件可能漏事件
- watcher 生命周期：fire-and-forget（随进程退出）

## 3. DB schema v36——统一 launcher_index 表

**合并 app_index + command_index 为 launcher_index**（用户决策：app 也该有 description/keywords，统一更合理）。

```sql
CREATE TABLE IF NOT EXISTS launcher_index (
    type        TEXT NOT NULL,               -- "app" | "command"
    name        TEXT NOT NULL,               -- Chrome / fd
    path        TEXT NOT NULL,               -- 绝对路径（UNIQUE——同一 path 不可能既是 app 又是 command）
    alias       TEXT NOT NULL DEFAULT '',     -- app 的本地化名（微信），command 无
    icon        TEXT NOT NULL DEFAULT '',     -- app 的 base64 icon，command 无
    source      TEXT NOT NULL DEFAULT '',     -- command 的 brew/cargo/system，app 用 "applications"
    description TEXT NOT NULL DEFAULT '',     -- 英文描述（command 的 whats/brew desc，app 可后补）
    keywords    TEXT NOT NULL DEFAULT '',     -- LLM 生成的中英文关键字（app 和 command 都可补）
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (type, path)
);
```

**迁移**（v36）：
1. 建 launcher_index 表
2. 从旧 app_index 迁移数据：`INSERT INTO launcher_index (type, name, path, alias, icon, source) SELECT 'app', name, path, alias, icon, 'applications' FROM app_index`
3. **删除旧 app_index 表**（数据已迁，保留无意义）
4. 现有 `load_app_index`/`save_app_index` 改为读写 launcher_index WHERE type='app'
5. command 数据走 `INSERT INTO launcher_index (type='command', ...)`

**Rust struct 层保持分开**（AppIndex + CommandIndex 各自字段需求不同），但底层 DB 统一 launcher_index 表，按 type 过滤读写。CRUD 函数：
- `load_launcher_by_type(type: &str) -> Vec<LauncherRow>` — 按 type 读
- `save_launcher_batch(type: &str, rows: &[LauncherRow])` — 按 type 全量替换（先 DELETE WHERE type 再 INSERT）
- `update_launcher_keywords(type: &str, path: &str, keywords: &str)` — LLM 生成后更新

现有 `load_app_index`/`save_app_index` 改为调 launcher 的 wrapper（`load_launcher_by_type("app")`）。

## 4. 前端 Tab

```
全部 ⌥A | 应用 ⌥D | 文件 ⌥F | 书签 ⌥B | 动作 ⌥Z | 命令 ⌥C
```

- `TabId` 加 `"commands"`
- `TABS` 加 `{ id: "commands", label: "命令", key: "c" }`
- `filterByTab` sourceMap 加 `commands: "command"`
- 命令 tab 不依赖选中文本，launch 模式也显示（不像 actions 要隐藏）
- 不加防抖（内存索引，亚毫秒）

## 5. SearchContext 扩展

```rust
pub struct SearchContext<'a> {
    pub app_index: &'a RwLock<AppIndex>,
    pub bookmarks: &'a RwLock<Vec<BookmarkEntry>>,
    pub frequency: &'a FrequencyScorer,
    pub command_index: &'a RwLock<CommandIndex>,  // 新增
    pub tab: &'a str,
}
```

SearchEngine 加 `command_index: RwLock<CommandIndex>` 字段。所有 Provider 的测试 ctx 构造要加这个字段。

## 6. 降级路径

| 场景 | 降级 |
|---|---|
| LLM 未配置 | keywords 为空，fuzzy 匹配命令名 + 英文 description |
| LLM API 失败 | 跳过该命令，keywords 留空，下次启动重试 |
| whats/brew desc 都没有 | description 为空，靠命令名匹配 |
| PATH 为空 | 命令索引为空，命令 tab 显示空 |
| notify-rs init 失败 | log warn，纯靠轮询 fallback |
| notify 漏事件 | 2 分钟轮询兜底校准 |

## 7. 性能

- PATH 扫描：~240 个可执行文件，read_dir + whats 查询，启动 < 500ms
- 英文描述：whatis 本地 DB 查询（微秒级），brew desc 仅对 brew 命令（~170 次 spawn，~3s）
- LLM 生成：后台异步，每命令 1 次 API（~500ms），240 个约 2 分钟，不阻塞搜索
- 搜索：内存索引 nucleo fuzzy，亚毫秒
- notify：秒级响应 vs 轮询 2 分钟

## 8. 不变量

1. CommandProvider 不进 all tab（独立 Tab，避免命令污染主搜索）
2. LLM 未配置时命令助手仍可用（英文描述 + 命令名匹配）
3. LLM 生成后台异步，不阻塞搜索（搜索读内存，缺中文的用英文 fallback）
4. notify 漏事件时数量轮询兜底（2 分钟内必定校准）
5. PATH 扫描覆盖所有来源（brew/cargo/npm/系统命令），不硬编码特定目录
6. 同名命令按 PATH 顺序去重（前面的优先）
7. 命令查阅不执行命令（action_type="copy" 复制命令名）
