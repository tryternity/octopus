# 命令查阅助手 + notify-rs 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** 新增"命令"Tab（CLI 命令查阅助手），扫 PATH 收集命令 + LLM 生成中英文描述 + fuzzy 匹配；引入 notify-rs 实时监听 app 目录。

**Architecture:** CommandIndex（仿 AppIndex）+ CommandProvider（独立 Tab）+ 后台 LLM 线程 + notify-rs file_watcher。

**Tech Stack:** Rust（octopus-search + octopus-desktop + octopus-infra）、Tauri 2、TypeScript/React、notify 8.x、octopus-llm（现有）。

## Global Constraints

- CommandProvider 不进 all tab（独立 "commands" Tab）。
- LLM 未配置时命令助手仍可用（英文描述 + 命令名匹配）。
- LLM 生成后台异步，不阻塞搜索。
- notify-rs 漏事件时数量轮询兜底（2 分钟）。
- PATH 扫描覆盖所有来源，不硬编码特定目录。
- 同名命令按 PATH 顺序去重。
- schema 当前最新 v35，新增 v36（统一 launcher_index 表，合并 app_index + command_index）。
- search crate 新增依赖：`notify = "8"`。
- 验证纪律：每任务后 cargo build + cargo test。

---

### Task 1: infra schema v36——统一 launcher_index 表 + 迁移 app_index + CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（v36 迁移：建 launcher_index + 迁移 app_index 数据 + 删旧表 + CRUD）
- Modify: `crates/infra/src/db.sql`（加 launcher_index 表，删 app_index 表）

**Interfaces:**
- Produces: `octopus_infra::db::{LauncherRow, load_launcher_by_type, save_launcher_batch, update_launcher_keywords}`
- Changes: `load_app_index`/`save_app_index` 改为 launcher_index 的 wrapper

- [ ] **Step 1: db.rs 加 LauncherRow + 统一 CRUD 函数**

在 db.rs 的 app_index 相关函数（`load_app_index`/`save_app_index`）附近加：

```rust
/// 统一启动器索引表的一行（app + command 共用）。
pub struct LauncherRow {
    pub r#type: String,       // "app" | "command"
    pub name: String,
    pub path: String,
    pub alias: String,        // app 的本地化名，command 无
    pub icon: String,         // app 的 base64 icon，command 无
    pub source: String,       // command 的 brew/cargo/system，app 用 "applications"
    pub description: String,  // 英文描述
    pub keywords: String,     // LLM 生成的中英文关键字
}

/// 按 type 加载启动器索引行。
pub fn load_launcher_by_type(item_type: &str) -> Result<Vec<LauncherRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT type, name, path, alias, icon, source, description, keywords
             FROM launcher_index WHERE type = ?1"
        )?;
        let rows = stmt.query_map(params![item_type], |r| LauncherRow {
            r#type: r.get(0)?, name: r.get(1)?, path: r.get(2)?, alias: r.get(3)?,
            icon: r.get(4)?, source: r.get(5)?, description: r.get(6)?, keywords: r.get(7)?,
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

/// 按 type 全量替换启动器索引（事务原子：先删该 type 再插）。
pub fn save_launcher_batch(item_type: &str, rows: &[LauncherRow]) -> Result<()> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM launcher_index WHERE type = ?1", params![item_type])?;
        for r in rows {
            tx.execute(
                "INSERT OR REPLACE INTO launcher_index
                 (type, name, path, alias, icon, source, description, keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![item_type, r.name, r.path, r.alias, r.icon, r.source, r.description, r.keywords],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

/// 更新单个启动器项的 keywords（LLM 生成后调）。
pub fn update_launcher_keywords(item_type: &str, path: &str, keywords: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE launcher_index SET keywords = ?3, updated_at = datetime('now')
             WHERE type = ?1 AND path = ?2",
            params![item_type, path, keywords],
        )?;
        Ok(())
    })
}
```

- [ ] **Step 2: 改 load_app_index / save_app_index 为 launcher_index wrapper**

现有 `load_app_index` 改为：
```rust
pub fn load_app_index() -> Result<Vec<(String, String, String, String)>> {
    let rows = load_launcher_by_type("app")?;
    Ok(rows.into_iter().map(|r| (r.name, r.alias, r.path, r.icon)).collect())
}
```
现有 `save_app_index` 改为：
```rust
pub fn save_app_index(rows: &[(String, String, String, String)]) -> Result<()> {
    let launcher_rows: Vec<LauncherRow> = rows.iter().map(|(name, alias, path, icon)| LauncherRow {
        r#type: "app".into(), name: name.clone(), path: path.clone(),
        alias: alias.clone(), icon: icon.clone(),
        source: "applications".into(), description: String::new(), keywords: String::new(),
    }).collect();
    save_launcher_batch("app", &launcher_rows)
}
```
**注意**：现有 save_app_index 签名是 `Vec<(name, alias, path, icon)>`——保持签名不变（搜索代码不破），内部转 LauncherRow。

- [ ] **Step 3: init_schema 加 v36——建 launcher_index + 迁移 app_index 数据 + 删旧表**

在 v35 分支后加：
```rust
    // v35→v36：统一 launcher_index 表（合并 app_index + command_index）。
    // 1. 建 launcher_index
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS launcher_index (
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            alias TEXT NOT NULL DEFAULT '',
            icon TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT '',
            description TEXT NOT NULL DEFAULT '',
            keywords TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (type, path)
        )",
    )?;
    // 2. 从旧 app_index 迁移数据（INSERT OR IGNORE 防重复迁移）
    conn.execute_batch(
        "INSERT OR IGNORE INTO launcher_index (type, name, path, alias, icon, source)
         SELECT 'app', name, path, alias, icon, 'applications' FROM app_index"
    )?;
    // 3. 删旧 app_index 表（数据已迁）
    conn.execute_batch("DROP TABLE IF EXISTS app_index")?;
    conn.execute("PRAGMA user_version = 36", [])?;
    log::info!("schema upgraded to v36 (launcher_index unified table, app_index migrated)");
```

更新早返回阈值：`if v >= 36 { return Ok(()); }`（原 v35）。

**全新库路径**（`v < 17` 分支末尾）：建 launcher_index（不建 app_index），user_version=36。删 db.sql 里的 app_index 建表语句，换 launcher_index。

- [ ] **Step 4: db.sql 更新——删 app_index 建 launcher_index**

db.sql 里找到 `CREATE TABLE IF NOT EXISTS app_index`，整段替换为 launcher_index 建表（不加 type 索引，PRIMARY KEY (type, path) 够了）。保留 app_index 的 name/alias 索引迁移到 launcher_index（加 `CREATE INDEX idx_launcher_name ON launcher_index(name)` 和 alias）。

- [ ] **Step 5: 更新现有测试的 user_version 断言**

grep `user_version.*35\|== 35\|v35` 在 db.rs tests，改为 36。

- [ ] **Step 6: 验证**

```bash
cargo test -p octopus-infra --lib 2>&1 | tail -5
cargo build -p octopus-infra -p octopus-search 2>&1 | tail -3
```
Expected: 0 error；现有测试全过（load_app_index/save_app_index wrapper 对 search crate 透明）。

- [ ] **Step 7: Commit**

```bash
git add crates/infra/src/db.rs crates/infra/src/db.sql
git commit -m "feat(infra): schema v36 统一 launcher_index 表（合并 app_index + command_index）"
```

---

### Task 2: CommandIndex（PATH 扫描 + 英文描述 + DB 缓存）

**Files:**
- Create: `crates/search/src/command_index.rs`
- Modify: `crates/search/src/lib.rs`（导出 command_index）

**Interfaces:**
- Produces: `crate::command_index::{CommandEntry, CommandIndex}`

- [ ] **Step 1: 实现 command_index.rs**

```rust
//! CLI 命令索引：扫描 PATH 收集可执行文件 + whats/brew desc 英文描述 + DB 缓存。

use super::engine::SearchResult;

pub struct CommandEntry {
    pub name: String,
    pub path: String,
    pub source: String,       // "brew" | "cargo" | "system" | "path"
    pub description: String,  // 英文（whatis/brew desc）
    pub keywords: String,     // LLM 生成的中英文关键字
}

pub struct CommandIndex {
    pub commands: Vec<CommandEntry>,
}

impl CommandIndex {
    /// 加载命令索引：扫 PATH + 填英文描述 + 读 DB 缓存 keywords。
    pub fn scan() -> Self {
        // 1. 扫 PATH 收集可执行文件
        let mut entries = scan_path();
        // 2. 去重（同名保留 PATH 靠前的）
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));
        // 3. 填英文描述（whatis + brew desc）
        for e in &mut entries {
            if e.description.is_empty() {
                e.description = whats_desc(&e.name);
            }
        }
        // brew 命令额外用 brew desc（更准）
        for e in &mut entries {
            if e.source == "brew" && !e.name.is_empty() {
                if let Some(d) = brew_desc(&e.name) {
                    e.description = d;
                }
            }
        }
        // 4. 读 DB 缓存的 keywords（launcher_index WHERE type='command'）
        let db_rows = octopus_infra::db::load_launcher_by_type("command").unwrap_or_default();
        let db_map: std::collections::HashMap<String, String> = db_rows.iter()
            .map(|r| (format!("{}|{}", r.name, r.path), r.keywords.clone()))
            .collect();
        for e in &mut entries {
            let key = format!("{}|{}", e.name, e.path);
            if let Some(kw) = db_map.get(&key) {
                e.keywords = kw.clone();
            }
        }
        // 5. 写 DB 缓存（全量替换 type='command'——PATH 变化时同步）
        let cache: Vec<octopus_infra::db::LauncherRow> = entries.iter()
            .map(|e| octopus_infra::db::LauncherRow {
                r#type: "command".into(), name: e.name.clone(), path: e.path.clone(),
                alias: String::new(), icon: String::new(),
                source: e.source.clone(), description: e.description.clone(),
                keywords: e.keywords.clone(),
            }).collect();
        let _ = octopus_infra::db::save_launcher_batch("command", &cache);
        log::info!("[search] 命令索引: {} 条", entries.len());
        Self { commands: entries }
    }

    /// 空索引（测试用）。
    pub fn empty() -> Self { Self { commands: vec![] } }
}

/// 扫描 PATH 收集可执行文件。
fn scan_path() -> Vec<CommandEntry> {
    let path_var = match std::env::var("PATH") { Ok(p) => p, Err(_) => return vec![] };
    let mut entries = vec![];
    for dir in path_var.split(':') {
        if dir.is_empty() { continue; }
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            // 跳过目录，只要可执行文件
            if !path.is_file() { continue; }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // 跳过隐藏文件（. 开头）
            if name.starts_with('.') { continue; }
            let source = classify_source(dir);
            entries.push(CommandEntry {
                name, path: path.to_string_lossy().to_string(),
                source, description: String::new(), keywords: String::new(),
            });
        }
    }
    entries
}

/// 按 PATH 目录路径判定命令来源。
fn classify_source(dir: &str) -> &'static str {
    if dir.contains("/homebrew/") || dir.contains("/linuxbrew/") { "brew" }
    else if dir.contains(".cargo/bin") { "cargo" }
    else if matches!(dir, "/usr/bin" | "/bin" | "/usr/sbin" | "/sbin") { "system" }
    else { "path" }
}

/// whats 命令查 man page 摘要（本地，微秒级）。
fn whats_desc(cmd: &str) -> String {
    let output = std::process::Command::new("whatis").arg(cmd).output();
    match output {
        Ok(o) if o.status.success() => {
            // whats 输出多行，取第一条含 cmd 的
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines()
                .find(|l| l.starts_with(cmd) && l.contains(" - "))
                .and_then(|l| l.splitn(2, " - ").nth(1))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// brew desc 查 brew 工具描述。
fn brew_desc(cmd: &str) -> Option<String> {
    let output = std::process::Command::new("brew").args(["desc", cmd]).output();
    match output {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // brew desc 输出 "name: description"，取冒号后
            s.splitn(2, ": ").nth(1).map(|d| d.to_string())
                .or_else(|| if s.is_empty() { None } else { Some(s) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_source_paths() {
        assert_eq!(classify_source("/opt/homebrew/bin"), "brew");
        assert_eq!(classify_source("/Users/me/.cargo/bin"), "cargo");
        assert_eq!(classify_source("/usr/bin"), "system");
        assert_eq!(classify_source("/usr/local/bin"), "path");
    }

    #[test]
    fn scan_path_returns_executables() {
        // PATH 至少含 /usr/bin，必有 ls/cat 等
        let idx = CommandIndex::scan();
        assert!(!idx.commands.is_empty(), "PATH 扫描应有结果");
        assert!(idx.commands.iter().any(|c| c.name == "ls" || c.name == "cat"),
            "应含常见命令，got: {:?}", idx.commands.iter().take(5).map(|c| &c.name).collect::<Vec<_>>());
    }

    #[test]
    fn dedup_keeps_path_first() {
        // 同名命令保留 PATH 靠前的
        let mut entries = vec![
            CommandEntry { name: "python3".into(), path: "/usr/bin/python3".into(), source: "system".into(), description: "".into(), keywords: "".into() },
            CommandEntry { name: "python3".into(), path: "/opt/homebrew/bin/python3".into(), source: "brew".into(), description: "".into(), keywords: "".into() },
        ];
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/usr/bin/python3");
    }
}
```

- [ ] **Step 2: lib.rs 导出**

`crates/search/src/lib.rs` 加 `pub mod command_index;`

- [ ] **Step 3: 验证**

```bash
cargo build -p octopus-search 2>&1 | tail -5
cargo test -p octopus-search --lib command_index 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/search/src/command_index.rs crates/search/src/lib.rs
git commit -m "feat(search): CommandIndex（PATH 扫描 + 英文描述 + DB 缓存）"
```

---

### Task 3: CommandProvider + SearchContext 扩展 + 注册

**Files:**
- Create: `crates/search/src/providers/command.rs`
- Modify: `crates/search/src/providers/mod.rs`
- Modify: `crates/search/src/provider.rs`（SearchContext 加 command_index）
- Modify: `crates/search/src/engine.rs`（SearchEngine 加 command_index 字段 + default_providers 注册 + ctx 构造）

- [ ] **Step 1: SearchContext 加 command_index 字段**

`provider.rs` SearchContext 加：
```rust
pub command_index: &'a parking_lot::RwLock<crate::command_index::CommandIndex>,
```

- [ ] **Step 2: SearchEngine 加 command_index 字段 + ctx 构造**

engine.rs：
- `SearchEngine` 加 `command_index: parking_lot::RwLock<crate::command_index::CommandIndex>`
- `init_search_engine` 初始化 `command_index: parking_lot::RwLock::new(crate::command_index::CommandIndex::scan())`
- `new_for_test` 加参数或用 `CommandIndex::empty()`
- search + search_streaming 的 ctx 构造加 `command_index: &self.command_index`
- `default_providers` 加 `Box::new(crate::providers::command::CommandProvider)`
- 加 `refresh_command_index` 方法（供后台 LLM 线程刷新）

- [ ] **Step 3: 实现 CommandProvider**

`crates/search/src/providers/command.rs`：
```rust
//! 命令查阅 Provider：CLI 命令 fuzzy 匹配（命令名 + 关键字 + 描述）。

use async_trait::async_trait;
use crate::command_index::CommandIndex;
use crate::engine::SearchResult;
use crate::matcher::match_score;
use crate::provider::{SearchContext, SearchProvider};

pub struct CommandProvider;

#[async_trait]
impl SearchProvider for CommandProvider {
    fn id(&self) -> &'static str { "command" }
    fn matches_tab(&self, tab: &str) -> bool { matches!(tab, "commands") }
    fn uses_frequency(&self) -> bool { false }

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let cmds = ctx.command_index.read();
        let mut scored: Vec<(i32, &crate::command_index::CommandEntry)> = cmds.commands.iter()
            .filter_map(|cmd| {
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
            subtitle: if cmd.keywords.is_empty() { cmd.description.clone() } else { cmd.keywords.clone() },
            icon: None,
            action_type: "copy".into(),
            action_data: serde_json::json!({ "text": cmd.name }).to_string(),
            score,
        }).collect()
    }
}
```

- [ ] **Step 4: mod.rs 加 `pub mod command;`**

- [ ] **Step 5: 更新所有 Provider 测试的 ctx 构造**

所有 test_ctx/test_providers helper 加 command_index 字段（用 `CommandIndex::empty()`）。

- [ ] **Step 6: 验证**

```bash
cargo build -p octopus-search 2>&1 | tail -10
cargo test -p octopus-search --lib 2>&1 | tail -10
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(search): CommandProvider + SearchContext command_index 字段 + 注册"
```

---

### Task 4: 命令索引后台 LLM 生成线程

**Files:**
- Modify: `crates/desktop/src/main.rs`（加后台 LLM 线程）
- Modify: `crates/search/src/engine.rs`（加 `commands_needing_keywords` + `update_command_keywords` 方法）

- [ ] **Step 1: engine.rs 加 LLM 支持方法**

```rust
impl SearchEngine {
    /// 返回 keywords 为空的命令（供后台 LLM 线程逐个生成）。
    pub fn commands_needing_keywords(&self) -> Vec<(String, String, String)> {
        self.command_index.read().commands.iter()
            .filter(|c| c.keywords.is_empty() && !c.description.is_empty())
            .map(|c| (c.name.clone(), c.path.clone(), c.description.clone()))
            .collect()
    }

    /// 为指定命令设置 keywords（LLM 生成后调）。
    pub fn update_command_keywords(&self, name: &str, path: &str, keywords: &str) {
        let mut cmds = self.command_index.write();
        if let Some(c) = cmds.commands.iter_mut().find(|c| c.name == name && c.path == path) {
            c.keywords = keywords.to_string();
        }
    }
}
```

- [ ] **Step 2: main.rs 加后台 LLM 线程**

在 setup 闭包里（app_index 轮询线程附近）加：

```rust
// 命令索引 LLM 关键字生成（后台异步，不阻塞搜索）
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_secs(60)); // 启动 60s 后开始（让 app 先跑起来）
    loop {
        let engine = match octopus_search::get_engine() {
            Some(e) => e,
            None => { std::thread::sleep(std::time::Duration::from_secs(300)); continue; }
        };
        let pending = engine.commands_needing_keywords();
        if pending.is_empty() {
            std::thread::sleep(std::time::Duration::from_secs(600)); // 无待生成，10 分钟后再查
            continue;
        }
        let config = crate::config::llm_config_ignore_mode(&cfg_read());
        let llm_config = match &config {
            Some(c) => c.clone(),
            None => { std::thread::sleep(std::time::Duration::from_secs(600)); continue; }
        };
        let system = "你是命令行工具专家。为给定命令生成简短的中英文搜索关键字，用空格分隔。只输出关键字，不要解释。包含：命令功能、同义词、中文翻译。限 30 字以内。";
        let mut generated = 0;
        for (name, path, desc) in pending.iter().take(20) { // 每轮最多 20 个
            let user = format!("命令: {}\n英文描述: {}", name, desc);
            let result = std::thread::spawn({
                let name = name.clone(); let path = path.clone();
                let user = user.clone(); let system = system.to_string();
                let cfg = llm_config.clone();
                move || octopus_llm::chat_text_with_prompt(&system, &user, &cfg)
                    .map(|s| (name, path, s.trim().to_string()))
            }).join();
            if let Ok(Ok((name, path, keywords))) = result {
                if !keywords.is_empty() {
                    let _ = octopus_infra::db::update_command_keywords(&name, &path, &keywords);
                    engine.update_command_keywords(&name, &path, &keywords);
                    generated += 1;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500)); // 防限流
        }
        log::info!("[search] 命令 LLM 关键字: 本轮生成 {} 个", generated);
        std::thread::sleep(std::time::Duration::from_secs(30)); // 轮间隔
    }
});
```

**注意**：`cfg_read()` 需从 AppState 读 AppConfig——看 main.rs 现有怎么读 config（grep `state.config` 或 `AppConfig`）。如果 config 是 State，需要 app_handle.state() 拿。

- [ ] **Step 3: 验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(desktop): 命令索引后台 LLM 关键字生成线程"
```

---

### Task 5: notify-rs file_watcher

**Files:**
- Modify: `crates/search/Cargo.toml`（加 notify 依赖——放 search 还是 desktop？放 desktop，因为 watcher 在 desktop 跑）
- Create: `crates/desktop/src/file_watcher.rs`
- Modify: `crates/desktop/src/main.rs`（启动 watcher + 保留轮询 fallback）
- Modify: `crates/desktop/Cargo.toml`（加 notify）

- [ ] **Step 1: Cargo.toml 加 notify**

desktop Cargo.toml `[dependencies]` 加 `notify = "8"`。

- [ ] **Step 2: 实现 file_watcher.rs**

```rust
//! notify-rs 文件监听：app 目录变化时实时刷新索引。
//! macOS FSEvents 对非用户所有文件可能漏事件——保留数量轮询作为 fallback。

use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config, EventKind};

/// 启动 app 目录监听。notify 收到 Create/Remove 事件 → debounce 3s → refresh_app_index。
/// 失败时静默返回（main.rs 的轮询线程会兜底）。
pub fn start_app_watcher() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[file_watcher] init failed: {}, fallback to polling only", e);
            return;
        }
    };
    for dir in &["/Applications", "/System/Applications", "/Applications/Utilities"] {
        if let Err(e) = watcher.watch(std::path::Path::new(dir), RecursiveMode::Recursive) {
            log::debug!("[file_watcher] watch {} failed: {}", dir, e);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let _ = watcher.watch(&home.join("Applications"), RecursiveMode::Recursive);
    }
    log::info!("[file_watcher] app 目录监听已启动");

    std::thread::spawn(move || {
        let mut last_trigger = std::time::Instant::now();
        for ev in rx {
            if let Ok(e) = ev {
                if matches!(e.kind, EventKind::Create(_) | EventKind::Remove(_)) {
                    // debounce 3s——安装/卸载 app 触发大量连续事件
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

- [ ] **Step 3: main.rs 启动 watcher**

在 setup 闭包里（app_index 轮询线程之前）加 `file_watcher::start_app_watcher();`。**保留现有 2 分钟轮询**作为 fallback。

- [ ] **Step 4: 验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(desktop): notify-rs file_watcher（app 目录秒级监听 + 轮询兜底）"
```

---

### Task 6: 前端 commands Tab

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts`
- Modify: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts`
- Modify: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.test.ts`

- [ ] **Step 1: searchTypes.ts 加 commands Tab**

TabId 加 `"commands"`：
```ts
export type TabId = "all" | "apps" | "files" | "bookmarks" | "actions" | "commands";
```

TABS 加（在 actions 后）：
```ts
{ id: "commands", label: "命令", key: "c" },
```

source 注释加 `"command"`。

- [ ] **Step 2: searchLogic.ts filterByTab 加映射**

sourceMap 加 `commands: "command"`。

- [ ] **Step 3: searchLogic.test.ts 更新测试**

Tab 数量从 5 改 6，getTabByKey/getNextTab/getTabIndex 加 commands 断言。

- [ ] **Step 4: 验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm test
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(frontend): 命令 Tab（⌥C）+ filterByTab 映射"
```

---

### Task 7: 测试 + 文档同步

- [ ] **Step 1: 全量测试**
```bash
cargo test --workspace --lib 2>&1 | tail -20
cd crates/desktop/frontend && npx tsc --noEmit && npm test
```

- [ ] **Step 2: architecture.md 更新**

加命令查阅助手 + notify-rs 段落 + schema v36。

- [ ] **Step 3: spec 状态改实现完成**

- [ ] **Step 4: Commit**
```bash
git add -A
git commit -m "docs: 命令查阅助手 + notify-rs 文档同步"
```
