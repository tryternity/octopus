# 热词多版本管理（hotword-sets）Implementation Plan

> ## 📊 实施状态总览（2026-07-11）
> ✅ T1-T8 代码完成，每 Task 经 subagent-driven 两阶段 review（spec compliance + code quality）。
> 📝 T9 e2e 真实录音验证待用户执行（Step 2-5）。
>
> | Task | 内容 | Commit | Review |
> |---|---|---|---|
> | T1 | infra hotword_text（normalize + pinyin_initials） | e8c0e46 | ✅✅ |
> | T2 | hotword_sets/hits 表 + schema v23 迁移 | da9e159, bbe5517(upsert fix) | ✅✅ |
> | T3 | HotwordSet CRUD | e1cbb5c | ✅✅ |
> | T4 | list_active 并集 + bump hotword_hits + list_hits | e91c3a6 | ✅✅ |
> | T5 | miner collect_candidate_words | 439c53a | ✅✅ |
> | T6 | desktop 命令重写 + main.rs 注册 | aba057b | ✅✅ |
> | T7 | pinyin_initials re-export + 清理旧 hotword | 9e0d3d6 | ✅✅ |
> | T8 | 前端 HotwordPanel 重写 | dd7a436 | ✅✅ |
> | T9 | e2e + 文档同步 | 本提交 | 📝 e2e 待用户 |
>
> **🔧 T9 后增强（用户反馈驱动）**：
> - 新增/挖掘词一次性高亮定位（`recentlyAdded` Set，替换语义非累加，组件重挂自然清空，无定时器）
> - 新建/导入版本按钮修复：WKWebView 不支持 `window.prompt/confirm` → inline input 输入名 + `@tauri-apps/plugin-dialog` 原生确认框（ff72d66）
> - 挖掘改两步确认流（用户反馈「直接落库要一个个删」）：`mine_hotword_candidates_to_set`（直接落库）拆为 `list_hotword_candidates`（候选不写库）+ `add_words_to_set`（确认后批量）；前端「挖掘」拉候选 → 确认面板（默认全选/可取消/可补词）→ 确认才落库

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 v1 扁平热词（单表 `hotwords`、全局 active）升级为「多版本词表 + 多选叠加 + 全局命中」：版本 = `hotword_sets.words_text` 纯文本（导入/导出 txt），生效词 = enabled 版本并集，命中统计走全局 `hotword_hits`（不绑版本），UI 卡片化给「逐词管理」体感。

**Architecture:** infra 新增 `hotword_sets`(name/enabled/words_text) + `hotword_hits`(word/hit_count) 两表与 `hotword_text` 模块（`normalize_words_text` + 搬入 `pinyin_initials`）；`list_active_hotword_words` 改读 enabled 并集，`bump_hotword_hit_by_word` 改写 `hotword_hits` upsert——**corrector/pipeline 零改**（命中收集 `pending_hits` + `drain_hits()` 机制已就绪）。schema v22→v23 一次性迁移现有 active 词到「通用」版本。miner 改返候选词列表（不再写 pending）。desktop 命令重写（版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本），前端 HotwordPanel 重写（版本管理 Card + 卡片网格）。

**Tech Stack:** Rust（rusqlite、pinyin、parking_lot、tauri-plugin-dialog、tokio spawn_blocking）、Tauri 2 `#[tauri::command]`、React TS 前端。

**Spec:** `docs/superpowers/specs/2026-07-11-hotword-sets-design.md`

**关键约束（记忆教训）:**
- worktree cwd 陷阱：所有 cargo/grep/git 必须显式带 worktree 绝对路径（`--manifest-path`/`-C`/绝对路径）。
- 依赖方向：infra 是底层，asr-local/desktop 依赖 infra，**infra 不能依赖 asr-local**（否则循环）。故 `normalize_words_text`/`pinyin_initials` 放 infra，asr-local re-export。
- e2e 铁律：真实录音 + 走 pipeline 全链路断言文本；直调 engine 绕过 corrector 会掩盖效果。
- 命中分层：corrector 只收集命中（`pending_hits`），pipeline 经 `drain_hits()` 取走后 bump DB——corrector 不碰 DB（避免单测污染）。本计划不改此分层。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/infra/Cargo.toml` | 依赖 | 加 `pinyin` |
| `crates/infra/src/hotword_text.rs` | 新模块 | `normalize_words_text` + `pinyin_initials`（从 asr-local 搬入）+ 测试 |
| `crates/infra/src/lib.rs` | 导出 | `pub mod hotword_text;` |
| `crates/infra/src/db.sql` | schema | 追加 `hotword_sets` + `hotword_hits` DDL |
| `crates/infra/src/db.rs` | schema + CRUD | init_schema v22→v23 迁移；`HotwordSet` + CRUD；改造 `list_active_hotword_words`/`bump_hotword_hit_by_word`；新增 `list_hotword_hits`；末尾清理旧 hotword 函数 |
| `crates/asr-local/src/hotword.rs` | re-export | `pinyin_initials` 改 `pub use octopus_infra::hotword_text::pinyin_initials;`（删本地实现） |
| `crates/asr-local/src/miner.rs` | 改造 | `mine_pending_candidates` → `collect_candidate_words`（返 `Vec<String>`，不写 DB） |
| `crates/desktop/src/hotword_commands.rs` | 重写 | 版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本 + hits |
| `crates/desktop/src/main.rs` | 注册 | `generate_handler` 更新（移除旧 5 命令，加新命令） |
| `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` | 重写 | 版本管理 Card + 卡片网格 + 导入导出挖掘 |
| `crates/desktop/frontend/src/pages/Settings/index.tsx` | props | HotwordPanel props 调整（命中展示等） |

---

## Task 1: infra `hotword_text` 模块（normalize + pinyin_initials 搬迁）

**Files:**
- Modify: `crates/infra/Cargo.toml`（加 pinyin 依赖）
- Create: `crates/infra/src/hotword_text.rs`
- Modify: `crates/infra/src/lib.rs`（导出模块）

> `pinyin_initials` 从 `asr-local/hotword.rs` 搬到 infra（依赖方向：asr-local→infra），`normalize_words_text` 新增。两者纯函数、无 DB、无全局，单测干净。

- [ ] **Step 1: infra/Cargo.toml 加 pinyin 依赖**

读 `crates/infra/Cargo.toml`，在 `[dependencies]` 段加（版本对齐 asr-local 用的 pinyin 版本，先查）：

```bash
grep -n '^pinyin' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml
```

把查到的行（如 `pinyin = "0.x"`）原样加进 `crates/infra/Cargo.toml` 的 `[dependencies]`。

- [ ] **Step 2: 写失败测试——normalize_words_text + pinyin_initials**

创建 `crates/infra/src/hotword_text.rs`：

```rust
//! 热词文本工具：拼音首字母 + 写入规范化（切词→去重→排序→拼接）。
//! 纯函数、无 DB、无全局状态——供 db.rs（迁移/写 words_text）与 asr-local/desktop 复用。

use pinyin::ToPinyin;

/// 词 → 拼音首字母串（大写，非汉字跳过）。如「八爪鱼」→`BZY`、「浮窗」→`FC`。
pub fn pinyin_initials(word: &str) -> String {
    word.chars()
        .filter_map(|c| c.to_pinyin().and_then(|p| p.plain().chars().next()))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 写入规范化：任意空白切词 → 去重 → 按 `(pinyin_initials, 词文本)` 升序 → 空格拼接。
/// `hotword_sets.words_text` 始终经此函数，保持有序、去重的规范形态。
pub fn normalize_words_text(words: &str) -> String {
    let mut v: Vec<String> = words.split_whitespace().map(|s| s.to_string()).collect();
    v.sort_by(|a, b| {
        pinyin_initials(a)
            .cmp(&pinyin_initials(b))
            .then_with(|| a.cmp(b))
    });
    v.dedup(); // 排序后去相邻重复
    v.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_initials_basic() {
        assert_eq!(pinyin_initials("八爪鱼"), "BZY");
        assert_eq!(pinyin_initials("浮窗"), "FC");
        assert_eq!(pinyin_initials("热词"), "RC");
        assert_eq!(pinyin_initials("AI助手"), "ZS"); // 非汉字跳过
        assert_eq!(pinyin_initials(""), "");
    }

    #[test]
    fn normalize_splits_any_whitespace() {
        // 空格 / 换行 / 制表符 都切
        assert_eq!(normalize_words_text("八爪鱼 吴大锐\n浮窗"), "八爪鱼 浮窗 吴大锐");
    }

    #[test]
    fn normalize_dedupes() {
        assert_eq!(normalize_words_text("八爪鱼 八爪鱼 吴大锐"), "八爪鱼 吴大锐");
    }

    #[test]
    fn normalize_sorts_by_initials_then_text() {
        // 浮窗(FC) 热词(RC) 八爪鱼(BZY 按 B 排前)
        // B < F < R → 八爪鱼 浮窗 热词
        assert_eq!(normalize_words_text("热词 浮窗 八爪鱼"), "八爪鱼 浮窗 热词");
    }

    #[test]
    fn normalize_empty() {
        assert_eq!(normalize_words_text(""), "");
        assert_eq!(normalize_words_text("   \n\t  "), "");
    }

    #[test]
    fn normalize_keeps_non_hanzi() {
        // 含非汉字的词保留（HotwordIndex 会自行跳过；normalize 不删）
        assert_eq!(normalize_words_text("AI助手 八爪鱼"), "AI助手 八爪鱼");
    }
}
```

- [ ] **Step 3: 运行验证失败**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra hotword_text::
```
Expected: FAIL（模块未导出）。

- [ ] **Step 4: lib.rs 导出模块**

在 `crates/infra/src/lib.rs` 的 `pub mod` 区加：

```rust
pub mod hotword_text;
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra hotword_text::
```
Expected: PASS（6 个测试）。

- [ ] **Step 6: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/infra/Cargo.toml crates/infra/src/hotword_text.rs crates/infra/src/lib.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(infra): hotword_text 模块（normalize + pinyin_initials 搬迁）"
```

---

## Task 2: db.sql 新表 DDL + schema v22→v23 迁移

**Files:**
- Modify: `crates/infra/src/db.sql`（末尾追加两表）
- Modify: `crates/infra/src/db.rs`（init_schema v22→v23 + schema 测试更新）

- [ ] **Step 1: db.sql 追加 hotword_sets + hotword_hits DDL**

在 `crates/infra/src/db.sql` 末尾（现有 `hotwords` 表 DDL 之后）追加：

```sql

-- ── ASR 热词版本（多场景词表，多选叠加）──────────────────────
CREATE TABLE IF NOT EXISTS hotword_sets (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1,   -- 0/1 是否勾选生效
    words_text  TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- ── ASR 热词全局命中计数（词级，不绑版本）────────────────────
CREATE TABLE IF NOT EXISTS hotword_hits (
    word        TEXT    PRIMARY KEY,
    hit_count   INTEGER NOT NULL DEFAULT 0
);
```

- [ ] **Step 2: init_schema 升 v22→v23 + 数据迁移**

在 `crates/infra/src/db.rs` 的 `init_schema` 函数里，定位 v21→v22 env seed 块（约 209-214 行，`conn.execute("PRAGMA user_version = 22", [])?;` 与 `return Ok(());` 之间）。把顶部 `if v >= 22 { return Ok(()); }`（约 177 行）改为 `>= 23`，并在 v22 env seed 之后、`return Ok(());`（约 215 行）之前插入 v22→v23 块：

```rust
        // v22→v23：热词多版本——hotword_sets/hotword_hits 表由 db.sql IF NOT EXISTS 自动创建。
        // 一次性迁移：现有 active 热词 → 「通用」版本 words_text（normalize 排序去重）；
        // 命中计数 → hotword_hits。pending 词丢弃（废弃 pending 确认流）。
        // hotwords 表保留但停用（不 DROP，留待后续清理）。
        if v < 23 {
            conn.execute_batch(INIT_SQL).ok(); // 确保 hotword_sets/hotword_hits 已建
            let words_text: String = {
                let mut stmt = conn.prepare(
                    "SELECT word FROM hotwords WHERE status='active' ORDER BY created_at",
                )?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut words: Vec<String> = Vec::new();
                for r in rows { words.push(r?); }
                crate::hotword_text::normalize_words_text(&words.join(" "))
            };
            conn.execute(
                "INSERT OR IGNORE INTO hotword_sets(name, enabled, words_text) VALUES('通用', 1, ?1)",
                params![words_text],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO hotword_hits(word, hit_count) \
                 SELECT word, hit_count FROM hotwords WHERE status='active'",
                [],
            )?;
            conn.execute("PRAGMA user_version = 23", [])?;
            log::info!("schema upgraded to v23 (hotword_sets + hotword_hits)");
        }
        return Ok(());
```

> 同步把函数末尾「全新库」分支的 `conn.execute("PRAGMA user_version = 22", [])?;`（约 220 行）与 `log::info!("DB initialized (v22)...)`（约 221 行）改为 `= 23` / `v23`。schema 注释块（约 167-171 行）补一行：`/// v23：新增 hotword_sets + hotword_hits 表；现有 active 热词迁「通用」版本。`

- [ ] **Step 3: 更新 schema 版本断言测试**

在 `crates/infra/src/db.rs` 末尾 `#[cfg(test)]` 内，把现有断言 `user_version` 为 22 的测试改为 23（grep 定位）：

```bash
grep -n 'user_version.*22\|= 22\|v22\|builds_v22' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/src/db.rs
```

把找到的断言 `22` 全改为 `23`，函数名/日志里的 `v22` 改 `v23`。

- [ ] **Step 4: 加迁移测试——现有 active 词进「通用」版本**

在 db.rs `#[cfg(test)]` 内追加：

```rust
    #[test]
    fn migrate_v22_hotwords_to_general_set() {
        // 构造 v22 库：hotwords 表 2 个 active（带 hit_count）+ 1 个 pending
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("PRAGMA user_version = 22", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('八爪鱼','active','manual',3)", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('吴大锐','active','manual',1)", []).unwrap();
        conn.execute("INSERT INTO hotwords(word, status, source, hit_count) VALUES('候选词','pending','mined',0)", []).unwrap();

        init_schema(&mut conn).unwrap();

        // v23
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 23);

        // 「通用」版本存在，含两个 active 词（normalize 排序），不含 pending
        let (name, words_text): (String, String) = conn
            .query_row("SELECT name, words_text FROM hotword_sets WHERE name='通用'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "通用");
        assert_eq!(words_text, "八爪鱼 吴大锐"); // BZY, WDR 升序（B<W）

        // hit_count 迁入 hotword_hits
        let wu: i64 = conn
            .query_row("SELECT hit_count FROM hotword_hits WHERE word='吴大锐'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wu, 1);
        // pending 词不进 hits
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM hotword_hits", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }
```

> 注意：`init_schema(&mut conn)` 签名以现有为准（若实际是 `&Connection`，按真实签名调；先 grep `fn init_schema` 对齐）。

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra -- db::tests
```
Expected: PASS（含 v23 断言 + 迁移测试）。

- [ ] **Step 6: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/infra/src/db.sql crates/infra/src/db.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(infra): hotword_sets + hotword_hits 表 + schema v23 迁移"
```

---

## Task 3: HotwordSet struct + CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（HotwordSet + CRUD，仿现有 hotword 范式）

> 旧 hotword 函数（`Hotword`/`list_hotwords`/`insert_hotword` 等）本 Task **暂不动**（miner/commands 仍引用，Task 5/6/7 处理后再清理）。

- [ ] **Step 1: 写失败测试——HotwordSet CRUD round-trip**

在 db.rs `#[cfg(test)]` 内追加：

```rust
    #[test]
    fn hotword_set_crud_roundtrip() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();

        // create
        let id = insert_hotword_set_at(&conn, "项目A").unwrap();
        assert!(id > 0);

        // list
        let sets = list_hotword_sets_at(&conn).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "项目A");
        assert!(sets[0].enabled);
        assert_eq!(sets[0].words_text, "");

        // 重名 → 唯一冲突
        assert!(insert_hotword_set_at(&conn, "项目A").is_err());

        // rename
        rename_hotword_set_at(&conn, id, "项目A2").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].name, "项目A2");

        // toggle enabled
        toggle_hotword_set_at(&conn, id, false).unwrap();
        assert!(!list_hotword_sets_at(&conn).unwrap()[0].enabled);
        toggle_hotword_set_at(&conn, id, true).unwrap();

        // add_word（normalize：序 + 去重）
        add_word_to_set_at(&conn, id, "吴大锐").unwrap();
        add_word_to_set_at(&conn, id, "八爪鱼").unwrap();
        add_word_to_set_at(&conn, id, "八爪鱼").unwrap(); // 重复 → 去重
        let s = list_hotword_sets_at(&conn).unwrap()[0].clone();
        assert_eq!(s.words_text, "八爪鱼 吴大锐"); // BZY < WDR

        // remove_word
        remove_word_from_set_at(&conn, id, "八爪鱼").unwrap();
        assert_eq!(list_hotword_sets_at(&conn).unwrap()[0].words_text, "吴大锐");

        // delete set
        delete_hotword_set_at(&conn, id).unwrap();
        assert!(list_hotword_sets_at(&conn).unwrap().is_empty());
    }
```

- [ ] **Step 2: 运行验证失败**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra hotword_set_crud_roundtrip
```
Expected: FAIL（函数未定义）。

- [ ] **Step 3: 实现 HotwordSet struct + `_at` CRUD**

在 `crates/infra/src/db.rs`（现有 `// ── Hotword（ASR 热词）` 段之前）插入：

```rust
// ── HotwordSet（热词版本/场景）──────────────────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordSet {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub words_text: String,
    pub created_at: String,
    pub updated_at: String,
}

const HOTWORD_SET_COLS: &str = "id, name, enabled, words_text, created_at, updated_at";

fn row_to_hotword_set(row: &rusqlite::Row) -> rusqlite::Result<HotwordSet> {
    Ok(HotwordSet {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        words_text: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub fn list_hotword_sets() -> Result<Vec<HotwordSet>> {
    with_db(|conn| list_hotword_sets_at(conn))
}

fn list_hotword_sets_at(conn: &Connection) -> Result<Vec<HotwordSet>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {c} FROM hotword_sets ORDER BY id ASC",
        c = HOTWORD_SET_COLS
    ))?;
    let rows = stmt.query_map([], row_to_hotword_set)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

pub fn get_hotword_set(id: i64) -> Result<HotwordSet> {
    with_db(|conn| {
        conn.query_row(
            &format!("SELECT {c} FROM hotword_sets WHERE id=?1", c = HOTWORD_SET_COLS),
            params![id],
            row_to_hotword_set,
        )
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))
    })
}

pub fn insert_hotword_set(name: &str) -> Result<i64> {
    with_db(|conn| insert_hotword_set_at(conn, name))
}

fn insert_hotword_set_at(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO hotword_sets (name) VALUES (?1)",
        params![name],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_hotword_set(id: i64, name: &str) -> Result<()> {
    with_db(|conn| rename_hotword_set_at(conn, id, name))
}

fn rename_hotword_set_at(conn: &Connection, id: i64, name: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE hotword_sets SET name=?1, updated_at=datetime('now') WHERE id=?2",
        params![name, id],
    )?;
    if n == 0 { anyhow::bail!("热词版本不存在"); }
    Ok(())
}

pub fn toggle_hotword_set(id: i64, enabled: bool) -> Result<()> {
    with_db(|conn| toggle_hotword_set_at(conn, id, enabled))
}

fn toggle_hotword_set_at(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE hotword_sets SET enabled=?1, updated_at=datetime('now') WHERE id=?2",
        params![if enabled { 1 } else { 0 }, id],
    )?;
    if n == 0 { anyhow::bail!("热词版本不存在"); }
    Ok(())
}

pub fn delete_hotword_set(id: i64) -> Result<()> {
    with_db(|conn| delete_hotword_set_at(conn, id))
}

fn delete_hotword_set_at(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM hotword_sets WHERE id=?1", params![id])?;
    if n == 0 { anyhow::bail!("热词版本不存在"); }
    Ok(())
}

/// 覆盖写 words_text（已 normalize）。导入「覆盖」模式用。
pub fn set_hotword_set_words(id: i64, words_text: &str) -> Result<()> {
    with_db(|conn| {
        let normalized = crate::hotword_text::normalize_words_text(words_text);
        let n = conn.execute(
            "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
            params![normalized, id],
        )?;
        if n == 0 { anyhow::bail!("热词版本不存在"); }
        Ok(())
    })
}

/// 追加一词到指定版本（并集 + normalize）。重复词去重无副作用，返回是否实际新增。
pub fn add_word_to_set(id: i64, word: &str) -> Result<bool> {
    with_db(|conn| add_word_to_set_at(conn, id, word))
}

fn add_word_to_set_at(conn: &Connection, id: i64, word: &str) -> Result<bool> {
    let cur: String = conn
        .query_row("SELECT words_text FROM hotword_sets WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
    let merged = format!("{} {}", cur, word);
    let normalized = crate::hotword_text::normalize_words_text(&merged);
    let added = normalized != cur;
    conn.execute(
        "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
        params![normalized, id],
    )?;
    Ok(added)
}

/// 批量追加多词（挖掘/导入追加用），返回实际新增条数。
pub fn add_words_to_set(id: i64, words: &[String]) -> Result<usize> {
    with_db(|conn| {
        let cur: String = conn
            .query_row("SELECT words_text FROM hotword_sets WHERE id=?1", params![id], |r| r.get(0))
            .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
        let before: std::collections::HashSet<&str> =
            cur.split_whitespace().collect();
        let merged = format!("{} {}", cur, words.join(" "));
        let normalized = crate::hotword_text::normalize_words_text(&merged);
        let after: std::collections::HashSet<&str> =
            normalized.split_whitespace().collect();
        let added = after.len().saturating_sub(before.len());
        conn.execute(
            "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
            params![normalized, id],
        )?;
        Ok(added)
    })
}

/// 从指定版本移除一词（normalize 重排）。
pub fn remove_word_from_set(id: i64, word: &str) -> Result<()> {
    with_db(|conn| remove_word_from_set_at(conn, id, word))
}

fn remove_word_from_set_at(conn: &Connection, id: i64, word: &str) -> Result<()> {
    let cur: String = conn
        .query_row("SELECT words_text FROM hotword_sets WHERE id=?1", params![id], |r| r.get(0))
        .map_err(|e| anyhow::anyhow!("热词版本不存在: {}", e))?;
    let filtered: Vec<&str> = cur
        .split_whitespace()
        .filter(|w| *w != word)
        .collect();
    let normalized = crate::hotword_text::normalize_words_text(&filtered.join(" "));
    conn.execute(
        "UPDATE hotword_sets SET words_text=?1, updated_at=datetime('now') WHERE id=?2",
        params![normalized, id],
    )?;
    Ok(())
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra hotword_set_crud_roundtrip
```
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/infra/src/db.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(infra): HotwordSet CRUD（版本管理 + 单词增删 + normalize 写入）"
```

---

## Task 4: list_active_hotword_words 改造 + bump 改写 hotword_hits + list_hotword_hits

**Files:**
- Modify: `crates/infra/src/db.rs`

> corrector/pipeline 零改：`list_active_hotword_words` 改读 enabled `hotword_sets` 并集（main.rs setup + reload_after_write 调用点签名不变，仍返回 `Vec<String>`）；`bump_hotword_hit_by_word` 改写 `hotword_hits` upsert（pipeline.rs:63 调用点不变）；新增 `list_hotword_hits` 供前端卡片命中展示。

- [ ] **Step 1: 写失败测试——list_active 取 enabled 并集 + hits**

在 db.rs `#[cfg(test)]` 内追加：

```rust
    #[test]
    fn list_active_words_is_enabled_union() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn.execute("INSERT INTO hotword_sets(name, enabled, words_text) VALUES('通用', 1, '八爪鱼 吴大锐')", []).unwrap();
        conn.execute("INSERT INTO hotword_sets(name, enabled, words_text) VALUES('项目A', 1, '吴大锐 周会')", []).unwrap();
        conn.execute("INSERT INTO hotword_sets(name, enabled, words_text) VALUES('关闭的', 0, '浮窗')", []).unwrap();

        let words = list_active_hotword_words_at(&conn).unwrap();
        // 并集去重：八爪鱼 吴大锐 周会（enabled=0 的「浮窗」不在）
        let set: std::collections::HashSet<&str> = words.iter().map(|s| s.as_str()).collect();
        assert_eq!(set, ["八爪鱼", "吴大锐", "周会"].into_iter().collect());

        // 全关 → 空
        conn.execute("UPDATE hotword_sets SET enabled=0", []).unwrap();
        assert!(list_active_hotword_words_at(&conn).unwrap().is_empty());
    }

    #[test]
    fn bump_hit_upserts_global_hits() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();

        bump_hotword_hit_by_word_at(&conn, "吴大锐").unwrap();
        bump_hotword_hit_by_word_at(&conn, "吴大锐").unwrap();
        bump_hotword_hit_by_word_at(&conn, "八爪鱼").unwrap();

        let wu: i64 = conn.query_row("SELECT hit_count FROM hotword_hits WHERE word='吴大锐'", [], |r| r.get(0)).unwrap();
        assert_eq!(wu, 2);
        let ba: i64 = conn.query_row("SELECT hit_count FROM hotword_hits WHERE word='八爪鱼'", [], |r| r.get(0)).unwrap();
        assert_eq!(ba, 1);

        let hits = list_hotword_hits_at(&conn).unwrap();
        assert_eq!(hits.get("吴大锐"), Some(&2i64));
    }
```

- [ ] **Step 2: 运行验证失败**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra list_active_words_is_enabled_union
```
Expected: FAIL（`list_active_hotword_words_at`/`bump_hotword_hit_by_word_at`/`list_hotword_hits_at` 未定义或旧行为）。

- [ ] **Step 3: 改造 list_active_hotword_words 为 enabled 并集**

把 `crates/infra/src/db.rs` 现有 `list_active_hotword_words`（约 1220 行）整体替换为：

```rust
/// 纠错热路径用——取所有 enabled 版本的 words_text 切词去重并集（构造 HotwordIndex 用）。
pub fn list_active_hotword_words() -> Result<Vec<String>> {
    with_db(|conn| list_active_hotword_words_at(conn))
}

fn list_active_hotword_words_at(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT words_text FROM hotword_sets WHERE enabled=1")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in rows {
        for w in r?.split_whitespace() {
            set.insert(w.to_string());
        }
    }
    Ok(set.into_iter().collect())
}
```

- [ ] **Step 4: 改写 bump_hotword_hit_by_word 为 hotword_hits upsert + 加 list_hotword_hits**

把现有 `bump_hotword_hit_by_word`（约 1303 行）替换为：

```rust
/// 命中计数 +1（按词文本——corrector 命中时只有文本）。写全局 `hotword_hits`（upsert）。
/// pipeline 在 correct 后批量调用（best-effort，失败由调用方忽略，不阻断纠错）。
pub fn bump_hotword_hit_by_word(word: &str) -> Result<()> {
    with_db(|conn| bump_hotword_hit_by_word_at(conn, word))
}

fn bump_hotword_hit_by_word_at(conn: &Connection, word: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hotword_hits(word, hit_count) VALUES(?1, 1) \
         ON CONFLICT(word) DO UPDATE SET hit_count = hit_count + 1",
        params![word],
    )?;
    Ok(())
}

/// 全局命中计数（前端卡片命中展示用）。返回 word → hit_count。
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>> {
    with_db(|conn| list_hotword_hits_at(conn))
}

fn list_hotword_hits_at(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT word, hit_count FROM hotword_hits")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for r in rows {
        let (w, c) = r?;
        map.insert(w, c);
    }
    Ok(map)
}
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra -- db::tests
```
Expected: PASS（含并集 + upsert + hits）。

- [ ] **Step 6: 跑 asr-local 全量确认 reload 路径无回归**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml -p octopus-asr-local
```
Expected: PASS（corrector/pipeline 不受影响，list_active 返回类型不变）。

- [ ] **Step 7: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/infra/src/db.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(infra): list_active 取 enabled 并集 + bump 写 hotword_hits + list_hotword_hits"
```

---

## Task 5: miner 改返候选词列表（collect_candidate_words）

**Files:**
- Modify: `crates/asr-local/src/miner.rs`

> 旧 `mine_pending_candidates` 直接写 DB pending（调 `insert_hotword`）。新模型废弃 pending，改为返回候选词 `Vec<String>`，由命令层追加到用户选定版本。

- [ ] **Step 1: 改测试——collect_candidate_words 返回列表**

把 `crates/asr-local/src/miner.rs` 末尾 `#[cfg(test)]` 内追加（保留现有 `is_candidate` 测试）：

```rust
    #[test]
    fn collect_returns_ranked_candidates() {
        // collect_candidate_words 不写 DB，仅返回候选词列表（依赖 list_recent_text）。
        // 此处只验返回类型与非 panic；真实历史由 e2e 覆盖。
        let _ = collect_candidate_words();
    }
```

- [ ] **Step 2: 运行验证失败**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml -p octopus-asr-local miner::tests::collect_returns_ranked_candidates
```
Expected: FAIL（`collect_candidate_words` 未定义）。

- [ ] **Step 3: 实现 collect_candidate_words（替换 mine_pending_candidates）**

把 `crates/asr-local/src/miner.rs` 的 `mine_pending_candidates` 函数整体替换为：

```rust
/// 扫历史 → jieba 分词 → 词频过滤 → top-N 候选词。返回词列表（不写 DB）。
/// 命令层拿去追加到用户选定版本（废弃旧 pending 流）。
pub fn collect_candidate_words() -> anyhow::Result<Vec<String>> {
    let texts = octopus_infra::db::list_recent_text(HISTORY_LIMIT)?;
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let jieba = Jieba::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &texts {
        for w in jieba.cut(t, true) {
            if !is_candidate(w) {
                continue;
            }
            *counts.entry(w.to_string()).or_insert(0) += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_USER_COUNT)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(MAX_CANDIDATES);
    let words: Vec<String> = ranked.into_iter().map(|(w, _)| w).collect();
    log::info!("[hotword-miner] 挖掘 {} 条候选词", words.len());
    Ok(words)
}
```

> 顶部模块文档注释（第 1 行 `//! 候选挖掘...→ DB pending`）改为 `//! 候选挖掘：扫历史 ASR 文本，jieba 分词 + 词频过滤，低频高频专名 → 返回候选词列表（命令层追加到版本）。`

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml -p octopus-asr-local miner::
```
Expected: PASS。

- [ ] **Step 5: 跑 asr-local 全量确认无残留引用 mine_pending_candidates**

```bash
grep -rn 'mine_pending_candidates' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates --include='*.rs'
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml -p octopus-asr-local
```
Expected: grep 无结果（已全替换）；全量 PASS。

- [ ] **Step 6: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/asr-local/src/miner.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "refactor(asr-local): miner 改返候选词列表（collect_candidate_words）"
```

---

## Task 6: desktop hotword_commands 重写 + main.rs 注册

**Files:**
- Modify: `crates/desktop/src/hotword_commands.rs`（整体重写）
- Modify: `crates/desktop/src/main.rs`（generate_handler 更新）

- [ ] **Step 1: 整体重写 hotword_commands.rs**

把 `crates/desktop/src/hotword_commands.rs` 整体替换为：

```rust
//! 热词版本管理后端命令——版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本。
//! 底层：hotword_sets（版本纯文本）+ hotword_hits（全局命中）。

use octopus_infra::db::{self, HotwordSet};

/// 写库后统一 reload corrector 热词索引（enabled 版本并集）。
/// 失败仅告警，不阻断写操作（下次启动会重新装载）。
fn reload_after_write() {
    match db::list_active_hotword_words() {
        Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

// ── 版本 CRUD ──

#[tauri::command]
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>, String> {
    db::list_hotword_sets().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_hotword_set(name: String) -> Result<i64, String> {
    let id = db::insert_hotword_set(&name).map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub fn rename_hotword_set(id: i64, name: String) -> Result<(), String> {
    db::rename_hotword_set(id, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_hotword_set(id: i64) -> Result<(), String> {
    db::delete_hotword_set(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

#[tauri::command]
pub fn toggle_hotword_set(id: i64, enabled: bool) -> Result<(), String> {
    db::toggle_hotword_set(id, enabled).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

// ── 单词增删（系统透明维护 words_text）──

#[tauri::command]
pub fn add_word_to_set(id: i64, word: String) -> Result<bool, String> {
    let added = db::add_word_to_set(id, &word).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(added)
}

#[tauri::command]
pub fn remove_word_from_set(id: i64, word: String) -> Result<(), String> {
    db::remove_word_from_set(id, &word).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

// ── 全局命中 ──

#[tauri::command]
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>, String> {
    db::list_hotword_hits().map_err(|e| e.to_string())
}

// ── 挖掘到版本 ──
// ⚠️ post-T9（commit 46453a7a）已拆分为 list_hotword_candidates（候选不写库）
//    + add_words_to_set（确认后批量）。下方为 Task 6 历史实现，当前代码见状态总览「T9 后增强」。

/// 挖掘候选词并追加到指定版本。返回实际新增条数。
#[tauri::command]
pub fn mine_hotword_candidates_to_set(target_set_id: i64) -> Result<usize, String> {
    let words = octopus_asr_local::miner::collect_candidate_words().map_err(|e| e.to_string())?;
    if words.is_empty() {
        return Ok(0);
    }
    let added = db::add_words_to_set(target_set_id, &words).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(added)
}

// ── 导入 / 导出（照搬 save_image_dialog 的 spawn_blocking + dialog 范式）──

/// 导入 txt：mode = "new"（新建版本，需 new_name）/ "append"（追加到 target_set_id）
/// / "overwrite"（覆盖 target_set_id 的 words_text）。返回目标版本 id。
#[tauri::command]
pub async fn import_hotwords(
    app_handle: tauri::AppHandle,
    mode: String,
    target_set_id: Option<i64>,
    new_name: Option<String>,
) -> Result<i64, String> {
    tokio::task::spawn_blocking(move || -> Result<i64, String> {
        use tauri_plugin_dialog::DialogExt;
        let path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .blocking_open_file();
        let Some(path) = path else {
            return Err("未选择文件".into());
        };
        let path = path.as_path().ok_or("无效路径")?;
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

        match mode.as_str() {
            "new" => {
                let name = new_name.unwrap_or_else(|| "导入版本".to_string());
                let id = db::insert_hotword_set(&name).map_err(|e| e.to_string())?;
                db::set_hotword_set_words(id, &content).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            "append" => {
                let id = target_set_id.ok_or("append 模式需 target_set_id")?;
                let words: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();
                db::add_words_to_set(id, &words).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            "overwrite" => {
                let id = target_set_id.ok_or("overwrite 模式需 target_set_id")?;
                db::set_hotword_set_words(id, &content).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            other => Err(format!("未知导入模式: {}", other)),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 导出某版本 words_text 到 txt（用户选保存路径）。
#[tauri::command]
pub async fn export_hotwords(app_handle: tauri::AppHandle, set_id: i64) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let set = db::get_hotword_set(set_id).map_err(|e| e.to_string())?;
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .set_file_name(format!("{}.txt", set.name))
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &set.words_text).map_err(|e| e.to_string())?;
            log::info!("[hotword] 导出版本「{}」到 {}", set.name, path.display());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

- [ ] **Step 2: main.rs generate_handler 更新注册**

在 `crates/desktop/src/main.rs` 的 `tauri::generate_handler![...]`（约 194 行起）里，把现有 5 个旧命令注册（约 236-240 行）：

```rust
            hotword_commands::list_hotwords,
            hotword_commands::add_hotword,
            hotword_commands::confirm_pending_hotword,
            hotword_commands::delete_hotword,
            hotword_commands::mine_hotword_candidates,
```

替换为：

```rust
            hotword_commands::list_hotword_sets,
            hotword_commands::create_hotword_set,
            hotword_commands::rename_hotword_set,
            hotword_commands::delete_hotword_set,
            hotword_commands::toggle_hotword_set,
            hotword_commands::add_word_to_set,
            hotword_commands::remove_word_from_set,
            hotword_commands::list_hotword_hits,
            hotword_commands::mine_hotword_candidates_to_set,
            hotword_commands::import_hotwords,
            hotword_commands::export_hotwords,
```

- [ ] **Step 3: 编译验证**

```bash
cargo check --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/desktop/Cargo.toml -p octopus-desktop
```
Expected: 编译通过（main.rs setup 的 `list_active_hotword_words` 调用点签名未变，仍可用）。

> 若报 `Hotword`/`pinyin_initials` 相关错误：`hotword_commands.rs` 已不再用 `Hotword` struct（改用 `HotwordSet`）；asr-local 的 `pinyin_initials` re-export 在 Task 7 处理（此处 asr-local/hotword.rs 本地实现仍在，不报错）。

- [ ] **Step 4: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/desktop/src/hotword_commands.rs crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(desktop): 热词版本命令重写（CRUD + 增删词 + 导入导出 + 挖掘 + hits）"
```

---

## Task 7: asr-local pinyin_initials re-export + infra 清理旧 hotword 函数

**Files:**
- Modify: `crates/asr-local/src/hotword.rs`（pinyin_initials 改 re-export）
- Modify: `crates/infra/src/db.rs`（删旧 hotword 函数 + 测试）

> 此时 miner（Task 5）、commands（Task 6）均不再引用旧 hotword 函数，可安全清理。`pinyin_initials` 改 re-export infra 的（去重，单一真相）。

- [ ] **Step 1: asr-local/hotword.rs pinyin_initials 改 re-export**

把 `crates/asr-local/src/hotword.rs` 里的 `pinyin_initials` 实现（约 113-120 行）：

```rust
/// 词 → 拼音首字母串（大写，非汉字跳过）。如「八爪鱼」→`BZY`、「浮窗」→`FC`、「热词」→`RC`。
/// 供前端拼音首字母搜索/排序（与纠错共用同一 `pinyin` crate，保证一致）。
pub fn pinyin_initials(word: &str) -> String {
    word.chars()
        .filter_map(|c| c.to_pinyin().and_then(|p| p.plain().chars().next()))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}
```

替换为 re-export：

```rust
/// 词 → 拼音首字母串（大写，非汉字跳过）。实现搬至 `octopus_infra::hotword_text`
/// （infra 为底层，db.rs 迁移/写 words_text 需复用，避免循环依赖）。
pub use octopus_infra::hotword_text::pinyin_initials;
```

> 同步删除 asr-local/hotword.rs `#[cfg(test)]` 里的 `pinyin_initials_basic` 测试（已搬 infra，见 Task 1）。

- [ ] **Step 2: infra/db.rs 删除旧 hotword 函数**

把 `crates/infra/src/db.rs` 的 `// ── Hotword（ASR 热词）` 整段（`Hotword` struct、`HOTWORD_SELECT_COLS`、`row_to_hotword`、`list_hotwords`/`_at`、`insert_hotword`/`_at`、`confirm_pending_hotword`/`_at`、`delete_hotword`/`_at`、`bump_hotword_hit`（按 id））删除。**保留**：`list_recent_text`（挖掘用）、新 `list_active_hotword_words`/`bump_hotword_hit_by_word`/`list_hotword_hits`（Task 4 已改造）。

> grep 定位删除范围：
> ```bash
> grep -n 'pub struct Hotword\b\|pub fn list_hotwords\|pub fn insert_hotword\|pub fn confirm_pending_hotword\|pub fn delete_hotword\|pub fn bump_hotword_hit\b\|fn list_hotwords_at\|fn insert_hotword_at\|fn confirm_pending_hotword_at\|fn delete_hotword_at' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/src/db.rs
> ```
> 删完同步删 `#[cfg(test)]` 里引用旧函数的测试（如 `hotword_crud_roundtrip`、`hotwords_table_exists_after_init`）。

- [ ] **Step 3: 全仓 grep 确认无残留引用旧函数**

```bash
grep -rn 'list_hotwords\b\|insert_hotword\b\|confirm_pending_hotword\|delete_hotword\b\|bump_hotword_hit\b\|Hotword\b' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates --include='*.rs' | grep -v 'HotwordSet\|hotword_sets\|hotword_hits\|hotword_text\|hotword_commands\|HotwordHit'
```
Expected: 无结果（旧符号全清）。

- [ ] **Step 4: 编译 + 全量测试**

```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/Cargo.toml -p octopus-infra -p octopus-asr-local -p octopus-desktop
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/infra/Cargo.toml -p octopus-infra
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/asr-local/Cargo.toml -p octopus-asr-local
```
Expected: 编译通过；测试 PASS（infra hotword_text/迁移/HotwordSet/并集/hits + asr-local hotword/corrector/miner）。

- [ ] **Step 5: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/asr-local/src/hotword.rs crates/infra/src/db.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "refactor: pinyin_initials re-export infra + 清理旧 hotwords 表函数"
```

---

## Task 8: 前端 HotwordPanel 重写

**Files:**
- Rewrite: `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`

- [ ] **Step 1: 重写 HotwordPanel.tsx**

把 `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` 整体替换为：

```tsx
import { useEffect, useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '@/lib/utils';
import { Type, Plus, BookMarked, X, Search, Upload, Download, Trash2, Wand2, Check } from 'lucide-react';

interface HotwordSet {
  id: number;
  name: string;
  enabled: boolean;
  wordsText: string;
  createdAt: string;
  updatedAt: string;
}

interface Props {
  /** app_config.fuzzy_dialect（逗号分隔 token：f/h、hu/wu、n/l、r/l） */
  dialect: string;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
}

const DIALECT_OPTIONS: { tok: string; label: string }[] = [
  { tok: 'f/h', label: 'f/h 不分（浮 / 护）' },
  { tok: 'hu/wu', label: 'hu/wu 不分（黄 / 王）' },
  { tok: 'n/l', label: 'n/l 不分（刘 / 牛）' },
  { tok: 'r/l', label: 'r/l 不分（热 / 乐）' },
];

const selectClass = 'border border-border rounded-md bg-background px-2.5 py-1.5 text-sm cursor-pointer outline-none focus:border-voice/40 hover:border-foreground/30 transition-colors';

function Card({ icon: Icon, title, children }: { icon: React.ElementType; title: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-1">{children}</div>
    </div>
  );
}

function Row({ children }: { children: React.ReactNode }) {
  return <div className="flex items-center justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">{children}</div>;
}

function Toggle({ on, onClick, label }: { on: boolean; onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={onClick}
      className={cn('relative w-10 h-[22px] rounded-full transition-colors flex-shrink-0', on ? 'bg-voice' : 'bg-muted-foreground/25')}
    >
      <span className={cn('absolute top-0.5 left-0.5 w-[18px] h-[18px] bg-white rounded-full transition-transform shadow-sm', on && 'translate-x-[18px]')} />
    </button>
  );
}

export function HotwordPanel({ dialect, setVal, showToast }: Props) {
  const [sets, setSets] = useState<HotwordSet[]>([]);
  const [hits, setHits] = useState<Record<string, number>>({});
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [input, setInput] = useState('');
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<'time' | 'alpha' | 'hits'>('time');
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameVal, setRenameVal] = useState('');
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    const [s, h] = await Promise.all([
      invoke<HotwordSet[]>('list_hotword_sets'),
      invoke<Record<string, number>>('list_hotword_hits'),
    ]);
    setSets(s);
    setHits(h);
    if (s.length > 0 && (selectedId === null || !s.some((x) => x.id === selectedId))) {
      setSelectedId(s[0].id);
    }
    setLoaded(true);
  }, [selectedId]);

  useEffect(() => {
    refresh().catch((e) => showToast('加载失败：' + e));
  }, [refresh, showToast]);

  const selected = sets.find((s) => s.id === selectedId) || null;
  const words = useMemo(() => (selected?.wordsText.split(/\s+/).filter(Boolean) ?? []), [selected]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const arr = q ? words.filter((w) => w.toLowerCase().includes(q)) : words;
    return [...arr].sort((a, b) => {
      if (sort === 'hits') return (hits[b] ?? 0) - (hits[a] ?? 0);
      if (sort === 'alpha') return a.localeCompare(b);
      return 0; // time：保留 normalize 后的存储序（拼音首字母序）
    });
  }, [words, query, sort, hits]);

  const totalActiveWords = useMemo(
    () => sets.filter((s) => s.enabled).reduce((n, s) => n + new Set(s.wordsText.split(/\s+/).filter(Boolean)).size, 0),
    [sets],
  );

  // ── 版本操作 ──
  const createSet = useCallback(async () => {
    const name = prompt('版本名称', '新版本');
    if (!name) return;
    try {
      const id = await invoke<number>('create_hotword_set', { name });
      await refresh();
      setSelectedId(id);
      showToast('已新建版本');
    } catch (e) { showToast('新建失败：' + e); }
  }, [refresh, showToast]);

  const toggleSet = useCallback(async (id: number, enabled: boolean) => {
    try { await invoke('toggle_hotword_set', { id, enabled }); await refresh(); }
    catch (e) { showToast('切换失败：' + e); }
  }, [refresh, showToast]);

  const startRename = (id: number, cur: string) => { setRenaming(id); setRenameVal(cur); };
  const commitRename = useCallback(async (id: number) => {
    const name = renameVal.trim();
    if (!name) { setRenaming(null); return; }
    try { await invoke('rename_hotword_set', { id, name }); await refresh(); }
    catch (e) { showToast('重命名失败：' + e); }
    setRenaming(null);
  }, [renameVal, refresh, showToast]);

  const deleteSet = useCallback(async (id: number, name: string) => {
    if (!confirm(`删除版本「${name}」？（命中统计保留）`)) return;
    try { await invoke('delete_hotword_set', { id }); await refresh(); }
    catch (e) { showToast('删除失败：' + e); }
  }, [refresh, showToast]);

  // ── 单词操作 ──
  const addWord = useCallback(async () => {
    const w = input.trim();
    if (!w || selectedId === null) return;
    try {
      const added = await invoke<boolean>('add_word_to_set', { id: selectedId, word: w });
      setInput('');
      showToast(added ? '已添加' : '已存在');
      await refresh();
    } catch (e) { showToast('添加失败：' + e); }
  }, [input, selectedId, refresh, showToast]);

  const removeWord = useCallback(async (word: string) => {
    if (selectedId === null) return;
    try { await invoke('remove_word_from_set', { id: selectedId, word }); await refresh(); }
    catch (e) { showToast('删除失败：' + e); }
  }, [selectedId, refresh, showToast]);

  // ── 导入 / 导出 / 挖掘 ──
  const doImport = useCallback(async (mode: 'new' | 'append' | 'overwrite') => {
    if (selectedId === null) { showToast('请先选择版本'); return; }
    try {
      if (mode === 'new') {
        const name = prompt('新版本名称', '导入版本');
        if (!name) return;
        const id = await invoke<number>('import_hotwords', { mode, newName: name });
        await refresh(); setSelectedId(id); showToast('已导入为新版本');
      } else if (mode === 'overwrite' && !confirm('覆盖当前版本的全部词？')) {
        return;
      } else {
        await invoke('import_hotwords', { mode, targetSetId: selectedId });
        await refresh(); showToast(mode === 'append' ? '已追加' : '已覆盖');
      }
    } catch (e) { showToast('导入失败：' + e); }
  }, [selectedId, refresh, showToast]);

  const doExport = useCallback(async () => {
    if (selectedId === null) return;
    try { await invoke('export_hotwords', { setId: selectedId }); showToast('已导出'); }
    catch (e) { showToast('导出失败：' + e); }
  }, [selectedId, showToast]);

  const mine = useCallback(async () => {
    if (selectedId === null) { showToast('请先选择目标版本'); return; }
    try {
      const n = await invoke<number>('mine_hotword_candidates_to_set', { targetSetId: selectedId });
      showToast(n > 0 ? `挖掘完成，新增 ${n} 词` : '未发现新候选');
      await refresh();
    } catch (e) { showToast('挖掘失败：' + e); }
  }, [selectedId, refresh, showToast]);

  const toggleDialect = useCallback((tok: string) => {
    const sset = new Set(dialect.split(',').map((s) => s.trim()).filter(Boolean));
    if (sset.has(tok)) sset.delete(tok); else sset.add(tok);
    void setVal('fuzzy_dialect', [...sset].join(','));
  }, [dialect, setVal]);
  const enabledTokens = new Set(dialect.split(',').map((s) => s.trim()));

  return (
    <div className="max-w-[640px]">
      <div className="mb-5">
        <div className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">语音识别 · 热词纠错</div>
        <h2 className="mt-0.5 text-lg font-semibold tracking-tight">热词管理</h2>
        <p className="mt-1 text-xs text-muted-foreground">按场景管理多版本热词，勾选叠加生效。当前生效词 {totalActiveWords} 个。</p>
      </div>

      {/* 方言模糊 —— 保留 */}
      <Card icon={Type} title="方言模糊">
        <div className="grid grid-cols-2 gap-x-8 gap-y-1 py-1">
          {DIALECT_OPTIONS.map(({ tok, label }) => (
            <div key={tok} className="flex items-center justify-between py-2">
              <span className="text-sm">{label}</span>
              <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} label={label} />
            </div>
          ))}
        </div>
      </Card>

      {/* 版本管理 */}
      <Card icon={BookMarked} title={`热词版本（${sets.length}）`}>
        {!loaded ? (
          <p className="py-8 text-center text-sm text-muted-foreground">加载中…</p>
        ) : (
          <>
            <div className="flex items-center gap-2 py-2.5">
              <button onClick={createSet} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
                <Plus className="w-4 h-4" /> 新建版本
              </button>
              <button onClick={() => doImport('new')} className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
                <Upload className="w-3.5 h-3.5" /> 导入新版本
              </button>
            </div>
            {sets.map((s) => (
              <Row key={s.id}>
                <div className="flex items-center gap-2 flex-1 min-w-0">
                  <Toggle on={s.enabled} onClick={() => toggleSet(s.id, !s.enabled)} label={`启用 ${s.name}`} />
                  {renaming === s.id ? (
                    <input
                      autoFocus
                      value={renameVal}
                      onChange={(e) => setRenameVal(e.target.value)}
                      onBlur={() => commitRename(s.id)}
                      onKeyDown={(e) => { if (e.key === 'Enter') commitRename(s.id); if (e.key === 'Escape') setRenaming(null); }}
                      className="flex-1 min-w-0 bg-background border border-voice/50 rounded px-1.5 py-0.5 text-sm outline-none"
                    />
                  ) : (
                    <button
                      onClick={() => { setSelectedId(s.id); startRename(s.id, s.name); }}
                      className={cn('truncate text-sm hover:text-voice', selectedId === s.id && 'font-medium text-voice')}
                      title="点击重命名"
                    >
                      {s.name}
                    </button>
                  )}
                  <span className="font-mono text-[10px] text-muted-foreground/60 flex-shrink-0">
                    {s.wordsText.split(/\s+/).filter(Boolean).length} 词
                  </span>
                </div>
                <div className="flex items-center gap-0.5 flex-shrink-0">
                  <button onClick={() => setSelectedId(s.id)} className="rounded p-1 text-muted-foreground hover:text-foreground" aria-label="选中编辑">
                    <Check className={cn('w-3.5 h-3.5', selectedId === s.id ? 'text-voice' : 'opacity-40')} />
                  </button>
                  <button onClick={doExport} disabled={selectedId !== s.id} className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-30" aria-label="导出">
                    <Download className="w-3.5 h-3.5" />
                  </button>
                  <button onClick={() => deleteSet(s.id, s.name)} className="rounded p-1 text-muted-foreground hover:text-red-500" aria-label="删除版本">
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </Row>
            ))}
          </>
        )}
      </Card>

      {/* 选中版本的词（逐词管理体感） */}
      {selected && (
        <Card icon={Plus} title={`${selected.name}（${words.length} 词）`}>
          {/* 单个添加 + 导入追加/覆盖 + 挖掘 */}
          <div className="flex items-center gap-2 py-2.5">
            <input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && addWord()}
              placeholder="人名 / 地名 / 术语"
              className="flex-1 min-w-0 bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
            />
            <button onClick={addWord} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
              <Plus className="w-4 h-4" /> 添加
            </button>
            <button onClick={() => doImport('append')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title="导入追加到当前版本">
              <Upload className="w-3.5 h-3.5" /> 追加
            </button>
            <button onClick={() => doImport('overwrite')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title="导入覆盖当前版本">
              <Upload className="w-3.5 h-3.5" /> 覆盖
            </button>
            <button onClick={mine} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
              <Wand2 className="w-3.5 h-3.5" /> 挖掘
            </button>
          </div>

          {/* 搜索 + 排序 */}
          {words.length > 0 && (
            <div className="flex items-center gap-2 py-2 border-t border-border/40">
              <div className="relative flex-1 min-w-0">
                <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/50 pointer-events-none" />
                <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索（汉字）" className="w-full bg-background border border-border rounded pl-7 pr-2.5 py-1.5 text-sm outline-none focus:border-voice/50" />
              </div>
              <select value={sort} onChange={(e) => setSort(e.target.value as 'time' | 'alpha' | 'hits')} className={cn(selectClass, 'flex-shrink-0')} aria-label="排序方式">
                <option value="time">默认</option>
                <option value="alpha">字母</option>
                <option value="hits">命中度</option>
              </select>
            </div>
          )}

          {/* 卡片网格（命中数 inline） */}
          {words.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">空版本，添加或导入热词。</p>
          ) : visible.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">无匹配热词</p>
          ) : (
            <div className="flex flex-wrap gap-2 py-2.5">
              {visible.map((w) => {
                const h = hits[w] ?? 0;
                return (
                  <div key={w} className="relative rounded-md border border-border bg-background px-3 py-2 pr-7 min-w-[112px] max-w-[200px] hover:border-foreground/25">
                    <button onClick={() => removeWord(w)} className="absolute top-1 right-1 rounded p-0.5 text-muted-foreground/60 hover:text-red-500" aria-label={`删除 ${w}`}>
                      <X className="w-3 h-3" />
                    </button>
                    <div className="text-sm truncate">{w}</div>
                    <div className={cn('mt-1 font-mono text-[10px] tabular-nums', h > 0 ? 'text-voice' : 'text-muted-foreground/50')}>{h}</div>
                  </div>
                );
              })}
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
```

- [ ] **Step 2: 检查 index.tsx 的 HotwordPanel props**

```bash
grep -n 'HotwordPanel\|<HotwordPanel' /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/desktop/frontend/src/pages/Settings/index.tsx
```

确认传参仍为 `{ dialect, setVal, showToast }`（本次 props 未变），无需改 index.tsx。若 index.tsx 传了其他 v1 专属 prop（如 `initials`），移除。

- [ ] **Step 3: 前端类型检查**

```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/crates/desktop/frontend run build
```
Expected: 构建通过（TS 无类型错误）。

- [ ] **Step 4: Commit**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx crates/desktop/frontend/src/pages/Settings/index.tsx
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "feat(desktop): 热词面板重写（版本管理 + 卡片网格 + 导入导出挖掘）"
```

---

## Task 9: e2e 真实录音验证 + 文档同步

**Files:**
- Manual e2e（无代码）
- Modify: `docs/superpowers/specs/2026-07-11-hotword-sets-design.md`（状态 → 已实现）
- Modify: `docs/superpowers/plans/2026-07-11-hotword-sets.md`（实施状态总览 + 各 Task checkbox 回填）
- Modify: `docs/architecture.md`（热词章节：扁平 → 多版本）

> e2e 铁律（沿用 v1）：真实录音 + 走 desktop pipeline 全链路断言文本；直调 engine 绕过 corrector 会掩盖效果。

- [ ] **Step 1: 构建桌面 app**

```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management/Cargo.toml -p octopus-desktop
```
Expected: 编译通过。

- [ ] **Step 2: e2e——多版本生效**

启动 desktop，进设置页「热词」：
1. 确认迁移产物：有一个「通用」版本，含迁移前的 active 词（若之前有）。
2. 新建版本「项目A」，添加一个会被误识别的专名（如人名「吴大锐」），勾选 enabled。
3. 录一句含该专名、会被模型误识的语音 → 断言最终文本含「吴大锐」（命中纠错）。
4. 关闭「项目A」enabled → 同句录音 → 断言文本不被纠错（回到误识原样）。
5. 重新勾选 → 再次命中。
6. 查看该词卡片命中数 > 0（`hotword_hits` 累加）。

- [ ] **Step 3: e2e——导入导出 round-trip**

1. 「通用」版本点「导出」→ 选路径存 `通用.txt`。
2. 外部确认文件内容 = 版本词（空格分隔，已 normalize 排序）。
3. 新建空版本「导入测试」→ 「导入覆盖」选刚才的 `通用.txt` → 断言「导入测试」词集合 = 「通用」。
4. 「追加」模式：往某版本导入另一 txt → 断言为并集。

- [ ] **Step 4: e2e——挖掘到版本**

1. 选某版本 → 「挖掘」→ 断言 toast「新增 N 词」（或「未发现新候选」）。
2. 该版本词卡片网格出现挖掘词（可逐个 ✕ 删）。

- [ ] **Step 5: e2e——enabled 全关 no-op（过纠回归）**

所有版本 enabled 关闭 → 录一段正常语音（无误识专名）→ 断言文本原样（过纠为零，corrector no-op）。

- [ ] **Step 6: 文档同步**

1. spec 顶部「状态」改为：`✅ 已实现（含 e2e，YYYY-MM-DD 用户真实录音走 desktop pipeline 全链路验证通过）`。
2. 本 plan 顶部加「实施状态总览」段（同 asr-hotword plan 范式），各 Task Step checkbox 回填 `[x]`。
3. `docs/architecture.md` 热词章节：扁平 `hotwords` 表 → `hotword_sets`(多版本) + `hotword_hits`(全局命中)，描述生效词并集、命中全局、导入导出。

- [ ] **Step 7: Commit 文档**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management add docs/superpowers/specs/2026-07-11-hotword-sets-design.md docs/superpowers/plans/2026-07-11-hotword-sets.md docs/architecture.md
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/hotword-management commit -m "docs(hotword): 多版本热词 e2e 通过 + plan/spec/architecture 同步"
```

---

## Self-Review

**1. Spec 覆盖：**
- `hotword_sets`（版本，存 words_text）+ `hotword_hits`（全局命中）→ Task 2（表+迁移）✓
- `normalize_words_text`（切词→去重→拼音首字母排序→拼接）→ Task 1 ✓
- HotwordSet CRUD → Task 3 ✓
- 生效词 = enabled 并集（`list_active_hotword_words` 改造）→ Task 4 ✓
- 命中全局（`bump_hotword_hit_by_word` 改写 hotword_hits upsert）→ Task 4 ✓
- UI 卡片化（逐词体感）+ 版本管理 + 导入导出 + 挖掘 → Task 6/8 ✓
- 导入三选项（新建/追加/覆盖）+ 导出 → Task 6 import_hotwords/export_hotwords ✓
- 挖掘保留改造（废弃 pending，候选→确认面板→追加当前版本）→ Task 5（miner）+ Task 6 原始 mine_to_set（**post-T9 已拆分为 list_hotword_candidates + add_words_to_set**，见状态总览「T9 后增强」）✓
- corrector/pipeline 零改（命中分层保留）→ Task 4 仅改 infra bump ✓
- 方言模糊保留 → Task 8 UI 保留 ✓
- 删版本不删命中（全局历史）→ Task 8 deleteSet confirm 文案 + 命中表独立 ✓
- 数据迁移（active→通用，hit→hits，pending 丢弃）→ Task 2 ✓
- 旧 hotwords 表/函数清理 → Task 7 ✓
- e2e 铁律 → Task 9 ✓
- L1 云端原生热词 → spec 标为未来，本计划不含（一致）✓

**2. 占位符扫描：** 无 TBD/TODO；Task 1 Step 1 的 pinyin 版本要求先 grep 对齐（给出具体命令，非占位）；Task 7 Step 2 的删除范围给出 grep 定位命令（非占位）。

**3. 类型一致性：**
- `HotwordSet { id, name, enabled: bool, words_text, created_at, updated_at }`——infra struct（Task 3）↔ 前端 interface（Task 8，camelCase：`wordsText`）serde `rename_all="camelCase"` 一致 ✓
- `list_active_hotword_words() -> Result<Vec<String>>`——签名不变（main.rs setup 调用点 + reload_after_write 不破）✓
- `bump_hotword_hit_by_word(word: &str) -> Result<()>`——签名不变（pipeline.rs:63 不破）✓
- `list_hotword_hits() -> Result<HashMap<String,i64>>`——infra（Task 4）↔ 命令（Task 6）↔ 前端 `Record<string,number>`（Task 8）✓
- `collect_candidate_words() -> anyhow::Result<Vec<String>>`（Task 5）↔ `list_hotword_candidates` 调用（Task 6，post-T9 拆分后由前端确认再 `add_words_to_set`）✓
- `add_word_to_set(id, word) -> Result<bool>` / `add_words_to_set(id, &[String]) -> Result<usize>` / `set_hotword_set_words(id, &str)` / `remove_word_from_set(id, word)`——infra（Task 3）↔ 命令（Task 6）一致 ✓
- `normalize_words_text(&str) -> String`——infra hotword_text（Task 1），db.rs 迁移 + HotwordSet 写入引用一致 ✓
- `pinyin_initials` re-export 路径 `octopus_infra::hotword_text::pinyin_initials`（Task 1 定义 / Task 7 re-export）✓
- Tauri 命令返回类型均 `pub`（generate_handler 宏在 main.rs 引用）——HotwordSet pub ✓、HashMap 别名 `std::collections::HashMap` 命令签名直接用（编译可见）✓
