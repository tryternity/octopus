# ASR 热词系统 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给全部 11 个 ASR 引擎（7 本地 + 4 云端）统一的「热词纠错」能力：把 `corrector.rs` 重构为候选集有界版本（候选只来自热词表），顺手清掉过纠债；配 DB 热词表、自动挖掘+人工确认、设置页管理 UI。

**Architecture:** L2 后处理热词纠错打底——`LightCorrector.find_candidates` 的候选源从「全词典 fuzzy_pinyin_to_words」改为「HotwordIndex（仅热词）」，无热词或无命中时零改动返回原文（过纠根因消失）。热词存 DB `hotwords` 表（active/pending 两态），运行期 `parking_lot::RwLock<HotwordIndex>` 热路径读、reload 时整体替换。CandidateMiner 扫 `clipboard_history` 的 ASR 文本挖低频高频专名写 pending。调用点 `pipeline.rs:58` 与 `streaming_runner.rs:316` 不变。

**Tech Stack:** Rust（rusqlite、jieba_rs、pinyin、parking_lot、once_cell）、Tauri 2 `#[tauri::command]`、React TS 前端。

**Spec:** `docs/superpowers/specs/2026-07-09-asr-hotword-design.md`

**关键约束（记忆教训）:**
- worktree cwd 陷阱：所有 cargo/grep/git 必须显式带 worktree 绝对路径（`--manifest-path`/`-C`/绝对路径）。
- e2e 铁律：真实录音 + 走 pipeline 全链路断言文本；直调 engine 绕过 corrector 会掩盖效果。
- `config.rs:127` 注释已是「纠错与热词校正」、`asr_correct` 默认 `false`（主开关现成，无需改 config）。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/infra/src/db.sql` | schema | 新增 `hotwords` 表 DDL |
| `crates/infra/src/db.rs` | CRUD | struct + CRUD `_at` + user_version 19→20 |
| `crates/asr-local/src/hotword.rs` | 新模块 | `HotwordIndex` + 模糊拼音 helpers + `reload_hotwords` |
| `crates/asr-local/src/corrector.rs` | 重构 | `find_candidates` 改热词源；加 `hotwords` 字段；更新测试 |
| `crates/asr-local/src/lib.rs` | 导出 | `pub mod hotword;` |
| `crates/asr-local/src/miner.rs` | 新模块 | CandidateMiner 扫历史挖专名 |
| `crates/desktop/src/hotword_commands.rs` | 新模块 | Tauri 命令（CRUD + mine + reload） |
| `crates/desktop/src/main.rs` | 注册 | 注册命令 + 启动 reload |
| `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` | 新组件 | 热词管理 UI |
| `crates/desktop/frontend/src/pages/Settings/index.tsx` | 入口 | 加「热词」tab |

---

> **实施状态总览（2026-07-10 同步）**：Task 1–9 已实现并合入 main——DB `hotwords` 表 + `HotwordIndex` + corrector 有界重构（候选仅来自热词表）+ CandidateMiner + Tauri 命令 + 设置页 UI + 全引擎 `skip_corrector=false`。Task 10–12 为后续方言规则可配与 UI 迭代增补。各 Task 内 Step 级 checkbox 是 TDD 过程记录，实现已完成、不再逐个回填。

## Task 1: DB schema — `hotwords` 表

**Files:**
- Modify: `crates/infra/src/db.sql`（末尾追加）
- Modify: `crates/infra/src/db.rs`（user_version 19→20 + 测试）

- [ ] **Step 1: db.sql 追加 hotwords 表 DDL**

在 `crates/infra/src/db.sql` 文件末尾追加：

```sql

-- ── ASR 热词（active=生效/pending=挖掘待确认）──────────────────
CREATE TABLE IF NOT EXISTS hotwords (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    word        TEXT    NOT NULL UNIQUE,
    status      TEXT    NOT NULL DEFAULT 'active',   -- 'active' | 'pending'
    source      TEXT    NOT NULL DEFAULT 'manual',   -- 'manual' | 'mined'
    hit_count   INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_hotwords_status ON hotwords(status);
```

- [ ] **Step 2: db.rs 升 user_version 19→20**

在 `crates/infra/src/db.rs` 把 `PRAGMA user_version = 19`（约 191、198 行两处）改为 `= 20`；更新附近注释与 log：

```rust
        conn.execute("PRAGMA user_version = 20", [])?;
        log::info!("schema upgraded to v20 (hotwords)");
```

```rust
    conn.execute("PRAGMA user_version = 20", [])?;
    log::info!("DB initialized (v20): schema + seed + yaml 配置导入（无 yaml 则跳过）");
```

并把 schema 注释块（约 161-168 行）补一行：

```rust
/// v20：新增 hotwords 表（db.sql IF NOT EXISTS 自动创建）。
```

- [ ] **Step 3: 更新已存在的 schema 版本断言测试**

`crates/infra/src/db.rs` 末尾测试里，把 `init_schema_fresh_db_builds_v19` 改名 `init_schema_fresh_db_builds_v20`、断言 `19` 改 `20`；`init_schema_v19_is_noop` 改名 `init_schema_v20_is_noop`、断言同步。

```rust
fn init_schema_fresh_db_builds_v20() {
    // ... 原逻辑不变 ...
    assert_eq!(v, 20, "全新库 init_schema 后应到 v20");
}
```

- [ ] **Step 4: 加一个 hotwords 表存在性测试**

```rust
#[test]
fn hotwords_table_exists_after_init() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(INIT_SQL).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM hotwords", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "hotwords 表应存在且初始为空");
}
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra -- db::tests
```
Expected: PASS（含 v20 断言 + hotwords 表存在）。

- [ ] **Step 6: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): hotwords 表 + schema v20"
```

---

## Task 2: Hotword struct + CRUD

**Files:**
- Modify: `crates/infra/src/db.rs`（struct + CRUD，仿 action_bar_items 范式）

- [ ] **Step 1: 写失败测试——CRUD round-trip**

在 `crates/infra/src/db.rs` `#[cfg(test)] mod tests` 内追加：

```rust
#[test]
fn hotword_crud_roundtrip() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(INIT_SQL).unwrap();

    // insert（manual, active）
    let id = insert_hotword_at(&mut conn, "八爪鱼", "manual", "active").unwrap();
    assert!(id > 0);

    // list_active 只含 active
    let active = list_hotwords_at(&conn, "active").unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].word, "八爪鱼");
    assert_eq!(active[0].source, "manual");

    // pending 隔离
    insert_hotword_at(&mut conn, "吴大锐", "mined", "pending").unwrap();
    let pending = list_hotwords_at(&conn, "pending").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].word, "吴大锐");
    assert_eq!(active.len(), 1, "active 不受 pending 影响");

    // confirm：pending → active
    confirm_pending_hotword_at(&conn, pending[0].id).unwrap();
    assert_eq!(list_hotwords_at(&conn, "active").unwrap().len(), 2);
    assert_eq!(list_hotwords_at(&conn, "pending").unwrap().len(), 0);

    // delete
    delete_hotword_at(&conn, id).unwrap();
    assert_eq!(list_hotwords_at(&conn, "active").unwrap().len(), 1);

    // word 唯一约束
    assert!(insert_hotword_at(&mut conn, "吴大锐", "manual", "active").is_err());
}
```

- [ ] **Step 2: 运行验证失败**

```bash
cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra hotword_crud_roundtrip
```
Expected: FAIL（函数未定义）。

- [ ] **Step 3: 实现 Hotword struct + SELECT_COLS + row 映射**

在 `crates/infra/src/db.rs`（action_bar 相关代码附近）追加：

```rust
// ── Hotword（ASR 热词）──────────────────────────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hotword {
    pub id: i64,
    pub word: String,
    pub status: String,
    pub source: String,
    pub hit_count: i64,
    pub created_at: String,
}

const HOTWORD_SELECT_COLS: &str = "id, word, status, source, hit_count, created_at";

fn row_to_hotword(row: &rusqlite::Row) -> rusqlite::Result<Hotword> {
    Ok(Hotword {
        id: row.get(0)?,
        word: row.get(1)?,
        status: row.get(2)?,
        source: row.get(3)?,
        hit_count: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// status: "active" | "pending"。设置页按状态分组渲染。
pub fn list_hotwords(status: &str) -> Result<Vec<Hotword>> {
    with_db(|conn| list_hotwords_at(conn, status))
}

fn list_hotwords_at(conn: &Connection, status: &str) -> Result<Vec<Hotword>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM hotwords WHERE status=?1 ORDER BY created_at DESC",
        HOTWORD_SELECT_COLS
    ))?;
    let rows = stmt.query_map(params![status], row_to_hotword)?;
    let mut list = Vec::new();
    for r in rows { list.push(r?); }
    Ok(list)
}

/// 纠错热路径用——只取 active 词文本（构造 HotwordIndex）。
pub fn list_active_hotword_words() -> Result<Vec<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT word FROM hotwords WHERE status='active'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn insert_hotword(word: &str, source: &str, status: &str) -> Result<i64> {
    with_db(|conn| insert_hotword_at(conn, word, source, status))
}

fn insert_hotword_at(conn: &mut Connection, word: &str, source: &str, status: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO hotwords (word, source, status) VALUES (?1, ?2, ?3)",
        params![word, source, status],
    )?;
    Ok(conn.last_insert_rowid())
}

/// pending → active（人工确认）。
pub fn confirm_pending_hotword(id: i64) -> Result<()> {
    with_db(|conn| confirm_pending_hotword_at(conn, id))
}

fn confirm_pending_hotword_at(conn: &Connection, id: i64) -> Result<()> {
    let updated = conn.execute(
        "UPDATE hotwords SET status='active' WHERE id=?1 AND status='pending'",
        params![id],
    )?;
    if updated == 0 {
        anyhow::bail!("待确认热词不存在或非 pending 状态");
    }
    Ok(())
}

pub fn delete_hotword(id: i64) -> Result<()> {
    with_db(|conn| delete_hotword_at(conn, id))
}

fn delete_hotword_at(conn: &Connection, id: i64) -> Result<()> {
    let deleted = conn.execute("DELETE FROM hotwords WHERE id=?1", params![id])?;
    if deleted == 0 {
        anyhow::bail!("热词不存在");
    }
    Ok(())
}

/// 命中计数 +1（纠错命中时调，用于多热词同音消歧排序）。
pub fn bump_hotword_hit(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute("UPDATE hotwords SET hit_count=hit_count+1 WHERE id=?1", params![id])?;
        Ok(())
    })
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra hotword_crud_roundtrip
```
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): hotword CRUD（insert/list/confirm/delete）"
```

---

## Task 3: HotwordIndex + 模糊拼音 helpers

**Files:**
- Create: `crates/asr-local/src/hotword.rs`
- Modify: `crates/asr-local/src/lib.rs`（导出模块）

- [ ] **Step 1: lib.rs 导出模块**

在 `crates/asr-local/src/lib.rs` 加：

```rust
pub mod hotword;
```

- [ ] **Step 2: 写失败测试——HotwordIndex 构造与 lookup**

创建 `crates/asr-local/src/hotword.rs`，先只放测试：

```rust
use std::collections::{HashMap, HashSet};

use pinyin::ToPinyin;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_is_empty() {
        let idx = HotwordIndex::from_words(&[]);
        assert!(idx.is_empty());
        assert_eq!(idx.max_len(), 0);
        assert!(idx.lookup(3, "ba-zhua-yu").is_none());
    }

    #[test]
    fn groups_by_length_and_pinyin() {
        let idx = HotwordIndex::from_words(&[
            "八爪鱼".to_string(),   // ba-zhua-yu, len 3
            "巴掌鱼".to_string(),   // ba-zhang-yu → fuzzy ba-zhan-yu, len 3
            "吴大锐".to_string(),   // wu-da-rui, len 3
        ]);
        assert!(!idx.is_empty());
        assert_eq!(idx.max_len(), 3);
        // 精确拼音 lookup
        assert!(idx.lookup(3, "ba-zhua-yu").is_some());
        // 模糊：zhang→zhan 归一后能查到「巴掌鱼」
        assert!(idx.lookup(3, "ba-zhan-yu").is_some());
        // 不存在的拼音
        assert!(idx.lookup(3, "xxx-yyy-zzz").is_none());
    }

    #[test]
    fn fuzzy_pinyin_normalizes_accents() {
        // 卫生 wei-sheng → wei-shen；打扫 da-sao
        assert_eq!(char_fuzzy_pinyin('卫'), Some("wei".to_string()));
        assert_eq!(char_fuzzy_pinyin('生'), Some("shen".to_string()));
        assert_eq!(char_fuzzy_pinyin('A'), None); // 非汉字
    }
}
```

- [ ] **Step 3: 运行验证失败**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local hotword::
```
Expected: FAIL（类型未定义）。

- [ ] **Step 4: 实现 HotwordIndex + helpers**

在 `crates/asr-local/src/hotword.rs`（测试上方）补实现：

```rust
use std::collections::{HashMap, HashSet};

use pinyin::ToPinyin;

/// 模糊拼音归一化——与 corrector.rs 旧逻辑 1:1 一致：
/// zh→z, ch→c, sh→s（平翘舌）；n→l；ing→in, eng→en, ang→an（前后鼻音）。
pub fn normalize_fuzzy_pinyin(py: &str) -> String {
    let mut n = py.to_lowercase();
    if n.starts_with("zh") {
        n = n.replacen("zh", "z", 1);
    } else if n.starts_with("ch") {
        n = n.replacen("ch", "c", 1);
    } else if n.starts_with("sh") {
        n = n.replacen("sh", "s", 1);
    }
    if n.starts_with('n') {
        n = format!("l{}", &n[1..]);
    }
    if n.ends_with("ing") {
        n = n[..n.len() - 3].to_string() + "in";
    } else if n.ends_with("eng") {
        n = n[..n.len() - 3].to_string() + "en";
    } else if n.ends_with("ang") {
        n = n[..n.len() - 3].to_string() + "an";
    }
    n
}

/// 单字 → 归一化模糊拼音；非汉字（无拼音）返回 None。
pub fn char_fuzzy_pinyin(c: char) -> Option<String> {
    c.to_pinyin().map(|p| normalize_fuzzy_pinyin(p.plain()))
}

/// 热词的内存索引：按「字数 → 归一化拼音 → 候选词列表」分组。
/// 纠错热路径按窗口字数与拼音 O(1) 查表。
pub struct HotwordIndex {
    by_len_py: HashMap<usize, HashMap<String, Vec<String>>>,
    active_words: HashSet<String>,
}

impl HotwordIndex {
    pub fn empty() -> Self {
        Self { by_len_py: HashMap::new(), active_words: HashSet::new() }
    }

    /// words 为 active 热词文本列表（来自 DB list_active_hotword_words）。
    /// 单字热词忽略（歧义太大）；含非汉字的热词忽略（拼音数 ≠ 字数）。
    pub fn from_words(words: &[String]) -> Self {
        let mut by_len_py: HashMap<usize, HashMap<String, Vec<String>>> = HashMap::new();
        let mut active_words = HashSet::new();
        for w in words {
            let chars: Vec<char> = w.chars().collect();
            let len = chars.len();
            if len < 2 { continue; }
            let py: Vec<String> = chars.iter().filter_map(|&c| char_fuzzy_pinyin(c)).collect();
            if py.len() != len { continue; } // 含非汉字 → 跳过
            let key = py.join("-");
            by_len_py.entry(len).or_default().entry(key).or_default().push(w.clone());
            active_words.insert(w.clone());
        }
        Self { by_len_py, active_words }
    }

    pub fn is_empty(&self) -> bool { self.active_words.is_empty() }

    pub fn max_len(&self) -> usize { *self.by_len_py.keys().max().unwrap_or(&0) }

    pub fn lookup(&self, len: usize, py: &str) -> Option<&Vec<String>> {
        self.by_len_py.get(&len)?.get(py)
    }
}
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local hotword::
```
Expected: PASS（3 个测试全过）。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-local/src/hotword.rs crates/asr-local/src/lib.rs
git commit -m "feat(asr-local): HotwordIndex + 模糊拼音 helpers"
```

---

## Task 4: corrector 重构为热词有界纠错

**Files:**
- Modify: `crates/asr-local/src/corrector.rs`

> 核心：`find_candidates` 候选源从 `fuzzy_pinyin_to_words`（全词典）改为 `HotwordIndex`（仅热词）。空热词 → 无候选 → 零纠错。bigram 评分保留，但只在 ≤少量热词候选间排序，过纠根因（全词典自由联想）消失。

- [ ] **Step 1: 重写测试——旧通用纠错测试改为热词驱动 + 加过纠回归**

把 `crates/asr-local/src/corrector.rs` 末尾 `mod tests` 整体替换为：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：给单例 corrector 装载热词后返回它。
    fn with_hotwords(words: &[&str]) -> &'static LightCorrector {
        let v: Vec<String> = words.iter().map(|s| s.to_string()).collect();
        reload_hotwords(v);
        get_corrector()
    }

    #[test]
    fn test_hotword_homophone_replace() {
        let c = with_hotwords(&["已经"]);
        // 模型把「已经」误识为同音的「以经」→ 热词命中替换
        assert_eq!(c.correct("我们以经坐上飞机了"), "我们已经坐上飞机了");
    }

    #[test]
    fn test_hotword_fuzzy_accent() {
        let c = with_hotwords(&["卫生"]);
        // 平翘舌/前后鼻音误读：微生(wei-sheng)→卫生(wei-sheng)，模糊归一后命中
        assert_eq!(c.correct("打扫微生"), "打扫卫生");
    }

    #[test]
    fn test_no_hotword_is_noop() {
        // 空热词 → 原样返回，零纠错（过纠根因消失的铁证）
        let c = with_hotwords(&[]);
        assert_eq!(c.correct("我们以经坐上飞机了"), "我们以经坐上飞机了");
    }

    #[test]
    fn test_overcorrection_regression() {
        // 历史过纠案例：模型正确的「开始语音识别」在旧 corrector 被改成「开始于饮食别」。
        // 有界版即使挂了热词，未命中窗口也必须原样返回。
        let c = with_hotwords(&["八爪鱼"]);
        assert_eq!(c.correct("开始语音识别"), "开始语音识别");
    }

    #[test]
    fn test_unaffected_text() {
        let c = with_hotwords(&["八爪鱼"]);
        let input = "你好，世界！Hello World.";
        assert_eq!(c.correct(input), input);
    }

    #[test]
    fn test_longer_hotword_window() {
        // 3 字热词（旧 correct_greedy 窗口只到 3；重构后按 max_len 覆盖）
        let c = with_hotwords(&["八爪鱼"]);
        // 同音误识「扒爪鱼」(ba-zhua-yu) → 命中
        assert_eq!(c.correct("我在养扒爪鱼"), "我在养八爪鱼");
    }
}
```

- [ ] **Step 2: 运行验证失败**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local corrector::tests
```
Expected: FAIL（`reload_hotwords` 未定义；旧通用纠错行为已不期望）。

- [ ] **Step 3: 改 LightCorrector 结构——加 hotwords 字段、删 fuzzy_pinyin_to_words**

在 `crates/asr-local/src/corrector.rs` 顶部 use 区加：

```rust
use crate::hotword::{char_fuzzy_pinyin, normalize_fuzzy_pinyin, HotwordIndex};
```

> 注意：`normalize_fuzzy_pinyin` / `char_fuzzy_pinyin` 现归 `hotword.rs` 所有，本文件删除其本地定义，改用上面的 `use`。`get_fuzzy_pinyin`（词级）保留在本文件（find_candidates 仍用），但其内部 `normalize_fuzzy_pinyin` 调用现走 use 进来的。

把 struct 定义改为（删 `fuzzy_pinyin_to_words`，加 `hotwords`）：

```rust
pub struct LightCorrector {
    jieba: Jieba,
    // Unigram log probabilities: word -> log(prob)（评分用，保留）
    unigram_scores: HashMap<String, f64>,
    // Bigram log probabilities: w1 -> { w2 -> log(prob) }（评分用，保留）
    bigram_scores: HashMap<String, HashMap<String, f64>>,
    // 热词索引——纠错候选的唯一来源。热路径读锁，reload 整体替换。
    hotwords: parking_lot::RwLock<HotwordIndex>,
}
```

- [ ] **Step 4: 删 new() 里的 fuzzy_pinyin_to_words 构造，hotwords 初始空**

把 `LightCorrector::new()` 中构建 `fuzzy_pinyin_to_words` 的整段（约 68、98-104 行）删除，结尾 struct 字面量改为：

```rust
        Self {
            jieba: Jieba::new(),
            unigram_scores,
            bigram_scores,
            hotwords: parking_lot::RwLock::new(HotwordIndex::empty()),
        }
```

- [ ] **Step 5: 重写 find_candidates——候选源改 HotwordIndex**

把 `find_candidates` 整体替换为：

```rust
    fn find_candidates(&self, query_word: &str) -> Vec<String> {
        let char_len = query_word.chars().count();
        if char_len < 2 {
            return vec![query_word.to_string()];
        }
        let idx = self.hotwords.read();
        if idx.is_empty() {
            return vec![query_word.to_string()]; // 无热词 → 无候选 → 零纠错
        }
        let query_py = get_fuzzy_pinyin(query_word);
        if query_py.is_empty() {
            return vec![query_word.to_string()];
        }
        let mut candidates: Vec<String> = idx
            .lookup(char_len, &query_py)
            .cloned()
            .unwrap_or_default();
        if !candidates.contains(&query_word.to_string()) {
            candidates.push(query_word.to_string());
        }
        candidates
    }
```

- [ ] **Step 6: correct_greedy 窗口范围按热词 max_len 扩展**

在 `correct_greedy` 的 `while i < n {` 循环内、`for sz in (2..=3).rev()` 之前，读一次 max_len 并把范围改为动态。把：

```rust
        let mut i = 0;
        while i < n {
            let mut replaced_sz = 0;
            for sz in (2..=3).rev() {
```

改为：

```rust
        let max_sz = { self.hotwords.read().max_len().max(3) };
        let mut i = 0;
        while i < n {
            let mut replaced_sz = 0;
            for sz in (2..=max_sz).rev() {
```

> 空热词时 max_sz=3 但 find_candidates 短路返回单候选，循环不替换，行为等价旧版「无操作」。

- [ ] **Step 7: 加 reload_hotwords 全局函数**

在文件底部 `get_corrector` 附近加：

```rust
/// 用 active 热词文本列表重建 corrector 的热词索引。
/// 启动时（DB 初始化后）与每次热词增删/确认后调用。corrector 未初始化时为 no-op（首调 correct 时以空索引初始化，随后由调用方补 reload）。
pub fn reload_hotwords(words: Vec<String>) {
    if let Some(c) = CORRECTOR.get() {
        let idx = HotwordIndex::from_words(&words);
        *c.hotwords.write() = idx;
    } else {
        // corrector 尚未初始化——先 force init（空索引），再写入
        let _ = get_corrector();
        if let Some(c) = CORRECTOR.get() {
            let idx = HotwordIndex::from_words(&words);
            *c.hotwords.write() = idx;
        }
    }
}
```

- [ ] **Step 8: 运行测试验证通过**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local corrector::tests
```
Expected: PASS（6 个测试全过，含过纠回归 + 空热词 no-op）。

- [ ] **Step 9: 跑 asr-local 全量测试确认无回归**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local
```
Expected: PASS（pipeline/streaming 等其他测试不受影响，因调用点未变）。

- [ ] **Step 10: Commit**

```bash
git add crates/asr-local/src/corrector.rs
git commit -m "refactor(asr-local): corrector 改热词有界纠错（清过纠债）"
```

---

## Task 5: 启动 reload 接线

**Files:**
- Modify: `crates/desktop/src/main.rs`（或 setup hook 所在文件）

> 目的：app 启动 DB 就绪后，把 active 热词灌进 corrector；之后所有引擎的纠错自动用上热词。

- [ ] **Step 1: 定位 desktop setup/启动钩子**

```bash
grep -n "tauri::generate_handler\|\.setup(\|ensure_db\|\.invoke_handler" crates/desktop/src/main.rs
```
找到 `.setup(|app| { ... })` 与 DB 初始化完成的位置。

- [ ] **Step 2: setup 里 DB 就绪后调 reload_hotwords**

在 `.setup` 闭包中、`ensure_db`（或等价 DB 初始化）调用之后追加：

```rust
        // DB 就绪后装载 active 热词到 corrector（force init + reload）
        match octopus_infra::db::list_active_hotword_words() {
            Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
            Err(e) => log::warn!("[hotword] 启动装载失败，纠错将以空热词运行: {}", e),
        }
```

> 若 main 实际是 `lib.rs` 的 `pub fn run()`，改在等价 setup 处。用 `cargo check -p octopus-desktop` 定位编译错误来确认引用路径。

- [ ] **Step 3: 编译验证**

```bash
cargo check --manifest-path crates/desktop/Cargo.toml -p octopus-desktop
```
Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "feat(desktop): 启动装载 active 热词到 corrector"
```

---

## Task 6: Tauri 命令（CRUD + reload）

**Files:**
- Create: `crates/desktop/src/hotword_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册到 invoke_handler）

- [ ] **Step 1: 写命令模块**

创建 `crates/desktop/src/hotword_commands.rs`：

```rust
//! 热词管理后端命令——CRUD + 挖掘 + 纠错索引 reload。

use octopus_infra::db::{self, Hotword};

/// 写库后统一 reload corrector 热词索引（active 词表）。
fn reload_after_write() {
    match db::list_active_hotword_words() {
        Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

#[tauri::command]
pub fn list_hotwords(status: String) -> Result<Vec<Hotword>, String> {
    db::list_hotwords(&status).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_hotword(word: String) -> Result<i64, String> {
    let id = db::insert_hotword(&word, "manual", "active").map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(id)
}

#[tauri::command]
pub fn confirm_pending_hotword(id: i64) -> Result<(), String> {
    db::confirm_pending_hotword(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

#[tauri::command]
pub fn delete_hotword(id: i64) -> Result<(), String> {
    db::delete_hotword(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

/// 触发挖掘：扫历史 ASR 文本挖低频高频专名 → 写 pending（见 miner）。
#[tauri::command]
pub fn mine_hotword_candidates() -> Result<usize, String> {
    let n = octopus_asr_local::miner::mine_pending_candidates().map_err(|e| e.to_string())?;
    Ok(n)
}
```

在 `crates/desktop/src/main.rs`（或 lib.rs）顶部 `mod` 声明区加：

```rust
mod hotword_commands;
```

- [ ] **Step 2: 注册进 invoke_handler**

找到 `tauri::generate_handler![ ... ]`（grep `generate_handler`），把下列加入数组：

```rust
        hotword_commands::list_hotwords,
        hotword_commands::add_hotword,
        hotword_commands::confirm_pending_hotword,
        hotword_commands::delete_hotword,
        hotword_commands::mine_hotword_candidates,
```

- [ ] **Step 3: 编译验证（miner 尚未实现，先临时桩）**

为让 Task 6 独立编译，先在 `crates/asr-local/src/miner.rs` 放空桩（Task 7 实现）：

```rust
//! 候选挖掘——Task 7 实现。
pub fn mine_pending_candidates() -> anyhow::Result<usize> {
    Ok(0)
}
```

并在 `crates/asr-local/src/lib.rs` 加 `pub mod miner;`。

```bash
cargo check --manifest-path crates/desktop/Cargo.toml -p octopus-desktop
```
Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/hotword_commands.rs crates/desktop/src/main.rs crates/asr-local/src/miner.rs crates/asr-local/src/lib.rs
git commit -m "feat(desktop): 热词 Tauri 命令（CRUD + mine + reload）"
```

---

## Task 7: CandidateMiner（自动挖掘）

**Files:**
- Modify: `crates/asr-local/src/miner.rs`（替换桩）
- Modify: `crates/infra/src/db.rs`（加取历史文本的查询）

> 策略：取近 N 条 `clipboard_history` 中 ASR 文本（`item_type='voice'`，可加 `'text'`/`'ocr'`），jieba 分词，统计词频，滤掉 jieba 词典高频常用词（`jieba.freq(w)` 高于阈值），剩下的低频但用户高频的词作 pending 候选插入（INSERT OR IGNORE，word 唯一）。

- [ ] **Step 1: db.rs 加取历史文本查询**

在 `crates/infra/src/db.rs` 加：

```rust
/// 取最近 limit 条 ASR/文本记录的 content（挖掘候选用）。
pub fn list_recent_text(limit: i64) -> Result<Vec<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM clipboard_history
             WHERE item_type IN ('voice','text','ocr') AND content IS NOT NULL AND content != ''
             ORDER BY id DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}
```

- [ ] **Step 2: 写失败测试——miner 过滤常用词**

`crates/asr-local/src/miner.rs`：

```rust
//! 候选挖掘：扫历史 ASR 文本，jieba 分词 + 词频过滤，低频高频专名 → DB pending。

use jieba_rs::Jieba;

/// jieba 词典词频高于此阈值视为常用词，不作候选（数字为 jieba 内部 freq 口径）。
const COMMON_FREQ_THRESHOLD: f64 = 1000.0;
/// 用户历史中至少出现此次数才作候选。
const MIN_USER_COUNT: usize = 2;
/// 单次挖掘回看的历史条数。
const HISTORY_LIMIT: i64 = 500;
/// 单次最多写入的候选数。
const MAX_CANDIDATES: usize = 30;
/// 候选词长度范围（专名通常是 2-4 字）。
const MIN_LEN: usize = 2;
const MAX_LEN: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_rare_drops_common() {
        let jieba = Jieba::new();
        // 「我们」「中国」是常用词（jieba 高频）；「八爪鱼」低频
        let keep = is_candidate(&jieba, "八爪鱼");
        let drop_common = is_candidate(&jieba, "我们");
        assert!(keep, "低频专名应保留");
        assert!(!drop_common, "高频常用词应过滤");
    }

    #[test]
    fn length_bounds_enforced() {
        let jieba = Jieba::new();
        // 单字非候选
        assert!(!is_candidate(&jieba, "的"));
    }
}
```

- [ ] **Step 3: 运行验证失败**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local miner::
```
Expected: FAIL（`is_candidate` 未定义）。

- [ ] **Step 4: 实现 is_candidate + mine_pending_candidates**

在 `crates/asr-local/src/miner.rs` 测试上方补：

```rust
/// 是否值得作为候选：长度 2-4、非高频常用词、纯汉字。
pub fn is_candidate(jieba: &Jieba, word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < MIN_LEN || chars.len() > MAX_LEN {
        return false;
    }
    // 纯汉字（非汉字 char_fuzzy_pinyin 返回 None → 含则排除）
    if chars.iter().any(|c| crate::hotword::char_fuzzy_pinyin(*c).is_none()) {
        return false;
    }
    // jieba 词典高频 → 常用词，过滤
    let freq = jieba.freq(word).unwrap_or(0.0);
    freq < COMMON_FREQ_THRESHOLD
}

/// 扫历史 → jieba 分词 → 词频过滤 → top-N 写 pending。返回写入条数。
pub fn mine_pending_candidates() -> anyhow::Result<usize> {
    let texts = octopus_infra::db::list_recent_text(HISTORY_LIMIT)?;
    if texts.is_empty() {
        return Ok(0);
    }
    let jieba = Jieba::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &texts {
        for w in jieba.cut(t, true) {
            if !is_candidate(&jieba, w) {
                continue;
            }
            *counts.entry(w.to_string()).or_insert(0) += 1;
        }
    }
    // 用户高频（≥ MIN_USER_COUNT）的候选，按频次降序取 top-N
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_USER_COUNT)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(MAX_CANDIDATES);

    let mut written = 0;
    for (word, _) in &ranked {
        // INSERT OR IGNORE：已存在（任意状态）则跳过，不覆盖 active
        match octopus_infra::db::insert_hotword(word, "mined", "pending") {
            Ok(_) => written += 1,
            Err(_) => { /* 已存在，跳过 */ }
        }
    }
    log::info!("[hotword-miner] 挖掘写入 {} 条 pending 候选", written);
    Ok(written)
}
```

> `insert_hotword_at` 用普通 INSERT（word 唯一约束），重复词插入会 Err 被吞掉 → 等价 INSERT OR IGNORE 语义。若想显式 OR IGNORE，可在 db.rs 加 `insert_hotword_or_ignore` 变体；此处 Err 吞足够。

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local miner::
```
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/asr-local/src/miner.rs crates/infra/src/db.rs
git commit -m "feat(asr-local): CandidateMiner 自动挖掘热词候选"
```

---

## Task 8: 前端设置页热词管理

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`

- [ ] **Step 1: 写 HotwordPanel 组件**

创建 `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx`：

```tsx
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface Hotword {
  id: number;
  word: string;
  status: string;
  source: string;
  hitCount: number;
  createdAt: string;
}

export function HotwordPanel() {
  const [active, setActive] = useState<Hotword[]>([]);
  const [pending, setPending] = useState<Hotword[]>([]);
  const [input, setInput] = useState('');
  const [mining, setMining] = useState(false);

  async function refresh() {
    setActive(await invoke<Hotword[]>('list_hotwords', { status: 'active' }));
    setPending(await invoke<Hotword[]>('list_hotwords', { status: 'pending' }));
  }

  useEffect(() => { refresh(); }, []);

  async function add() {
    const w = input.trim();
    if (!w) return;
    await invoke('add_hotword', { word: w });
    setInput('');
    await refresh();
  }

  async function confirm(id: number) {
    await invoke('confirm_pending_hotword', { id });
    await refresh();
  }

  async function remove(id: number) {
    await invoke('delete_hotword', { id });
    await refresh();
  }

  async function mine() {
    setMining(true);
    try {
      const n = await invoke<number>('mine_hotword_candidates');
      alert(`挖掘完成，新增 ${n} 条候选`);
    } finally {
      setMining(false);
      await refresh();
    }
  }

  return (
    <div style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 16 }}>
      <section>
        <h3>添加热词</h3>
        <div style={{ display: 'flex', gap: 8 }}>
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && add()}
            placeholder="人名 / 地名 / 术语 / 口头禅"
            style={{ flex: 1 }}
          />
          <button onClick={add}>添加</button>
          <button onClick={mine} disabled={mining}>
            {mining ? '挖掘中…' : '从历史挖掘'}
          </button>
        </div>
      </section>

      {pending.length > 0 && (
        <section>
          <h3>待确认（挖掘候选）</h3>
          <ul style={{ listStyle: 'none', padding: 0 }}>
            {pending.map((h) => (
              <li key={h.id} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <span>{h.word}</span>
                <button onClick={() => confirm(h.id)}>确认</button>
                <button onClick={() => remove(h.id)}>丢弃</button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section>
        <h3>生效热词（{active.length}）</h3>
        <ul style={{ listStyle: 'none', padding: 0 }}>
          {active.map((h) => (
            <li key={h.id} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <span>{h.word}</span>
              <small style={{ color: '#888' }}>
                {h.source === 'mined' ? '挖掘' : '手动'} · 命中 {h.hitCount}
              </small>
              <button onClick={() => remove(h.id)}>删除</button>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
```

- [ ] **Step 2: 注册到 Settings 入口 tab**

读 `crates/desktop/frontend/src/pages/Settings/index.tsx`，仿现有 tab（如 ActionBarPanel）导入并加一项：

```tsx
import { HotwordPanel } from './HotwordPanel';
// ... 在 tab 列表 / 路由配置里加：
//   { key: 'hotword', label: '热词', component: <HotwordPanel /> }
```

> 具体 tab 注册结构依 index.tsx 现有写法对齐（可能是数组或条件渲染）。打开文件按其模式追加一项，label「热词」。

- [ ] **Step 3: 前端构建验证**

```bash
npm --prefix crates/desktop/frontend run build
```
Expected: 构建通过（TS 无类型错误）。

- [ ] **Step 4: 手动冒烟（GUI 核心验证）**

启动 desktop，进设置页「热词」tab：添加一个词（如「八爪鱼」）→ 列表出现 → 用 ASR 录一句含同音误识的语音 → 确认纠错生效。GUI 行为核对。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx crates/desktop/frontend/src/pages/Settings/index.tsx
git commit -m "feat(desktop): 设置页热词管理面板"
```

---

## Task 9: skip_corrector 重评估（保守、测试把关）✅ 已实施

> **结果（2026-07-10 核实）**：sensevoice_orig.rs:114 / qwen3_asr.rs:138 / asr-cloud/src/batch.rs:86 全部 `skip_corrector() = false`（注释「有界热词纠错安全，空热词 no-op，重新启用」）。即所有引擎现在都经有界热词纠错。下列 Step 为原始计划，已执行。

**Files:**
- Modify: `crates/asr-local/src/sensevoice_orig.rs:114`、`qwen3_asr.rs:138`、`asr-cloud/src/batch.rs:86`

> 有界版无热词即 no-op、只向显式热词纠偏，过纠不可能发生（Task 4 `test_overcorrection_regression` 已证）。故 sensevoice/qwen3/cloud 可重新打开热词纠错，扩大受益面。**保守策略：逐引擎改 `skip_corrector() -> false`，每改一个跑一次该引擎相关测试 + 一次真实录音 e2e 确认无回归。**

- [ ] **Step 1: sensevoice_orig 改回 false**

```rust
// sensevoice_orig.rs:114
fn skip_corrector(&self) -> bool {
    false // 有界热词纠错安全（无热词即 no-op），重新启用
}
```

- [ ] **Step 2: 验证**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local sensevoice
```
Expected: PASS。真实录音 e2e 确认无误识别回归（若 sensevoice 本地无热词，行为应与改前一致——空热词 no-op）。

- [ ] **Step 3: qwen3_asr 同样改 false + 验证**

```rust
// qwen3_asr.rs:138
fn skip_corrector(&self) -> bool { false }
```
```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local qwen3
```
Expected: PASS。

- [ ] **Step 4: asr-cloud batch 同样改 false + 验证**

```rust
// asr-cloud/src/batch.rs:86
fn skip_corrector(&self) -> bool { false }
```
```bash
cargo test --manifest-path crates/asr-cloud/Cargo.toml -p octopus-asr-cloud
```
Expected: PASS。

- [ ] **Step 5: 全量测试 + Commit**

```bash
cargo test --manifest-path crates/asr-local/Cargo.toml -p octopus-asr-local
cargo test --manifest-path crates/asr-cloud/Cargo.toml -p octopus-asr-cloud
```
Expected: PASS。

```bash
git add crates/asr-local/src/sensevoice_orig.rs crates/asr-local/src/qwen3_asr.rs crates/asr-cloud/src/batch.rs
git commit -m "refactor(asr): 重新启用 sensevoice/qwen3/cloud 热词纠错（有界版安全）"
```

> 若任一引擎 e2e 出现回归，回退该引擎的 `skip_corrector() -> true`，记 issue 单独排查（不阻塞本次合入）。

---

## 收尾验证（全链路 e2e，铁律）

- [ ] **真实录音 e2e**：录一句含一个会被模型误识别的专名（如人名「吴大锐」被识成同音错字）→ 该专名加入 active 热词 → 走 desktop pipeline 全链路 → 断言最终文本含「吴大锐」。**必须走 pipeline，直调 engine 绕过 corrector 会掩盖效果。**
- [ ] **空热词 e2e**：清空热词 → 同一段录音 → 断言文本不被改动（过纠回归的端到端印证）。

---

## Self-Review

**1. Spec 覆盖：**
- HotwordStore（DB hotwords 表）→ Task 1+2 ✓
- HotwordIndex（asr-local/src/hotword.rs）→ Task 3 ✓
- BoundedHotwordCorrector（重构 corrector.rs）→ Task 4 ✓
- CandidateMiner → Task 7 ✓
- 设置页 UI → Task 8 ✓
- skip_corrector 重估 → Task 9 ✓
- reload 接线（启动 + 写后）→ Task 5 + Task 6 reload_after_write ✓
- L1 云端原生叠加 → spec 标为 v2，本计划不含（一致）✓

**2. 占位符扫描：** 无 TBD/TODO；Task 8 Step 2 的 tab 注册按 index.tsx 现有模式对齐（已说明打开文件按模式追加，非占位——是真实可执行步骤）。

**3. 类型一致性：** `HotwordIndex::from_words/lookup/is_empty/max_len`、`reload_hotwords(Vec<String>)`、`insert_hotword(word, source, status)`、`list_active_hotword_words()` 在各 Task 间签名一致 ✓。`is_candidate(jieba, word)` 定义与测试调用一致 ✓。

---

## Task 10（增补，2026-07-10）：方言模糊规则可配

热词 spec 增补：f/h、hu/wu、n/l 三组方言混淆做成 checkbox，存 `app_config.fuzzy_dialect`。

- [x] **10.1** hotword.rs：`FuzzyRules{fh,nl,hw}` + 全局 + `parse_dialect`（token f/h、hu/wu、n/l）+ `normalize_with_rules`（基础始终 + 方言可选 else if 互斥）。commit c9f411c
- [x] **10.2** corrector.rs：`active_words` 缓存 + `reload_fuzzy_dialect(s)`。commit a5e0ce0
- [x] **10.3** config.rs：`AppConfig.fuzzy_dialect`（serde default + Default impl 两处构造点）。commit e8dd04d / fccc0e9
- [x] **10.4** settings_commands.rs：`apply_config_value` fuzzy_dialect case（校验子集）+ set_config 热重载。commit fccc0e9
- [x] **10.5** main.rs：setup `reload_fuzzy_dialect`（先于 reload_hotwords）。commit 8c682a4
- [x] **10.6** 前端 HotwordPanel：3 checkbox + props；index.tsx 传参。commit 03cfbf7

**验证**：cargo test -p octopus-asr-local（hotword 11 + corrector 11）+ -p octopus-desktop settings_commands（11）全绿；cargo check desktop + npm build 通过。e2e 待用户（勾 f/h + 热词「浮窗」+ sensevoice 录音）。

---

## Task 11（增补，2026-07-10）：r/l 方言组 + 热词面板 UI 重设计

两件事一并交付：① 用户要求的 r/l 不分方言组（**只 r→l，刻意不动 sh/c**——sh/c 是死结，加 r/l 已减轻很多）；② 顺手把 HotwordPanel 从粗糙 inline-style 升级到 Settings 统一设计语言。

**r/l 设计决策**：声母 r→l，与 n/l 都归一到 l（首字母 n/r 不同，同开互不冲突）。**已知局限**：r/l 仅救首字——「热词→乐视」第一字「热 re→le」与「乐 le」归一命中，但第二字「词 ci」≠「视 shi→si」（sh/c 不归一，避免级联误命中）；对纯 r/l 混淆（热↔乐、肉↔漏、人↔林）完整有效。

- [x] **11.1** hotword.rs：`FuzzyRules` 加 `rl` 字段 + `parse_dialect` 加 token `r/l` + `normalize_with_rules` else if 链加 `r→l` 分支（nl→fh→rl→hw 互斥）+ 测试（`normalize_rl_dialect`、`normalize_nl_rl_both`、`parse_dialect` rl、四组组合）。
- [x] **11.2** settings_commands.rs：`apply_config_value` fuzzy_dialect case 合法 token 加 `r/l`（matches 加分支）+ 单测 valid 加 r/l 单独与四组组合。
- [x] **11.3** HotwordPanel.tsx **完全重写**（复用 GeneralPanel 的 Card/Row/Toggle + ActionBarPanel 的 TypeTag/按钮/输入/删除/空状态类）：4 方言 Toggle（含 r/l）+ 添加热词（voice 主按钮 + 挖掘次按钮）+ 待确认（确认/丢弃）+ 生效热词（pad2 序号 + SourceTag 来源色点[手动=voice/挖掘=emerald] + 命中数色阶[>0=voice/=0=muted] + 删除图标）+ `showToast` 替 `alert` + loaded 空状态。
- [x] **11.4** index.tsx：HotwordPanel 调用处加传 `showToast`。
- [x] **11.5** 文档：spec 方言节「三组→四组」+ r/l 归一 + sh/c 局限；architecture.md 三处（corrector 模块说明、方言段落、倒排索引列举）加 r/l。

**验证**：cargo test -p octopus-asr-local hotword（17 passed，含 rl 新测）+ -p octopus-desktop settings_commands（11 passed）+ 前端 tsc --noEmit（EXIT=0）。e2e 待用户（勾 r/l + 热词「乐」+ 说「热」/ 录音含 r/l 混淆专名）。

**未提交**：本任务代码与文档改动尚未 commit（待用户指示）。

---

## Task 12（增补，2026-07-10）：生效热词卡片化 + 拼音首字母搜索/排序

在 Task 11 重设计基础上的 UI 迭代 + 新功能：生效热词改卡片网格、加拼音首字母搜索与排序。

**拼音首字母**：复用 asr-local 的 `pinyin` crate（不引前端依赖、与纠错一致），新增 `pinyin_initials(word)`（汉字→大写首字母，非汉字跳过：八爪鱼→BZY、浮窗→FC、热词→RC）。

- [x] **12.1** asr-local/hotword.rs：`pub fn pinyin_initials(word) -> String` + `pinyin_initials_basic` 测试。
- [x] **12.2** desktop/hotword_commands.rs：`pub struct HotwordView`（Serialize camelCase，Hotword + initials）+ `impl From<Hotword>`（填充 pinyin_initials）；`list_hotwords` 返回 `Vec<HotwordView>`（infra Hotword 不动）。踩坑：tauri 命令返回类型须 `pub`（generate_handler 宏在 main.rs 引用，私有类型报 `private type`）。
- [x] **12.3** HotwordPanel 布局迭代：① 方言模糊改 `grid grid-cols-2`（一行两列）；② 生效热词改卡片网格（每词一卡：词名 + 右上角 X 删除 + 下方 meta），`flex flex-wrap gap-2`；③ 命中数去「命中」前缀纯数字，meta 顺序「方式色点 + 命中数字」（11.3 的列表 + pad2 序号样式废弃）。
- [x] **12.4** 搜索 + 排序（纯前端 state）：搜索框（拼音首字母前缀 `initials.startsWith(q)` OR 汉字包含 `word.includes(q)`）+ 排序下拉（最近=createdAt desc 默认 / 字母=initials localeCompare / 命中度=hitCount desc）；`useMemo` 派生 `visible`；无匹配→「无匹配热词」空态。
- [x] **12.5** 文档同步（spec 热词管理 UI + plan）。

**验证**：cargo test -p octopus-asr-local hotword（18 passed，含 pinyin_initials）+ cargo check -p octopus-desktop（HotwordView pub 修复后 Finished）+ 前端 tsc --noEmit（EXIT=0）。e2e 待用户。

**未提交**：Task 11 + 12 代码与文档改动尚未 commit（待用户指示）。
