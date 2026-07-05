# FTS5 搜索切换设计

## 背景

`clipboard_history_fts`（FTS5，trigram tokenizer）已建表（db.sql:228），三个触发器（`clip_fts_ai/ad/au`）在每次 INSERT/DELETE/UPDATE 时自动维护索引。但 voice 历史搜索函数 `list_transcriptions_search_at`（db.rs:976）仍用 `content LIKE '%query%'` 全表扫描——**索引建好了、触发器在跑、但搜索没用它**。审查报告标记为"已建索引却白维护"。

本 spec 定义：让搜索代码走 FTS5 MATCH，同时处理三个切换约束（backfill / 短查询 / 转义）。

## 目标

- voice 历史搜索走 FTS5 MATCH（>=3 字符），利用已有倒排索引
- <3 字符回退 LIKE（trigram 无法生成 3-gram）
- 历史 voice 行（触发器建表前已存在）回填进 FTS5 索引
- 不破坏现有搜索语义（子串匹配）

## 不变量

1. **搜索结果集不变**：FTS5 trigram MATCH 的语义是子串匹配，与 `LIKE '%query%'` 等价（对 >=3 字符查询），不会多/少返回行
2. **content="" 的行不索引**：voice/ocr/text 有文本被索引，image/file content="" 自动跳过（FTS5 空文本不生成 trigram）
3. **触发器行为不变**：已有的 `clip_fts_ai/ad/au` 触发器持续维护新行，本次只补 backfill + 改搜索

## 设计

### 1. Backfill 迁移（user_version 17 → 18）

历史 voice 行（触发器建表前或从旧 schema 迁移来的）不在 FTS5 索引中，需一次性回填。

**迁移逻辑**（`init_schema` 内，v17→v18）：

```sql
INSERT INTO clipboard_history_fts(rowid, content)
SELECT id, content FROM clipboard_history
WHERE content != ''
  AND id NOT IN (SELECT rowid FROM clipboard_history_fts);
```

- `content != ''`：空文本不索引（与触发器行为一致——空文本不生成 trigram）
- `NOT IN (SELECT rowid ...)`：幂等——已索引的行不重复插入（FTS5 外部内容表不设主键约束，重复 INSERT 会导致重复 rowid）
- 全新库（v0→v18）：`INIT_SQL` 建表时触发器自动维护首批 INSERT，无需 backfill

**init_schema 改造**：

```rust
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn.query_row("PRAGMA user_version", [])?;
    if v >= 18 { return Ok(()); }       // 已最新
    if v >= 17 {
        // v17→v18：backfill FTS5 索引（触发器建表前的历史行）
        conn.execute_batch(
            "INSERT INTO clipboard_history_fts(rowid, content)
             SELECT id, content FROM clipboard_history
             WHERE content != ''
               AND id NOT IN (SELECT rowid FROM clipboard_history_fts)"
        )?;
        conn.execute("PRAGMA user_version = 18", [])?;
        log::info!("FTS5 backfill 完成 (v17→v18)");
        return Ok(());
    }
    // v0 全新库：建表 + seed + yaml 导入，直跳 v18
    conn.execute_batch(INIT_SQL)?;
    migrate_yaml_to_db(conn)?;
    conn.execute("PRAGMA user_version = 18", [])?;
    Ok(())
}
```

### 2. 搜索函数改造

`list_transcriptions_search_at`（db.rs:976）按查询字符数分流：

```rust
fn list_transcriptions_search_at(
    conn: &Connection, limit: u32, offset: u32, search: Option<&str>,
) -> Result<Vec<TranscriptionRecord>> {
    if let Some(q) = search.filter(|s| !s.is_empty()) {
        let q_len = q.chars().count();
        if q_len >= 3 {
            // FTS5 MATCH 路径：利用倒排索引
            let escaped = escape_fts5_match(q);
            let rows = conn.prepare(
                "SELECT id, created_at,
                        COALESCE(json_extract(meta_info, '$.engine'), '') as engine,
                        CASE WHEN json_extract(meta_info, '$.polished') = 1 THEN 'done' ELSE 'off' END as polish_status,
                        CAST(json_extract(meta_info, '$.duration_ms') AS INTEGER) as duration_ms,
                        segments, content
                 FROM clipboard_history
                 WHERE item_type = 'voice'
                   AND id IN (SELECT rowid FROM clipboard_history_fts
                              WHERE clipboard_history_fts MATCH ?1)
                 ORDER BY id DESC LIMIT ?2 OFFSET ?3"
            )?.query_map(params![escaped, limit, offset], row_mapper)?;
            return Ok(rows.filter_map(|r| r.ok()).collect());
        }
        // <3 字符：回退 LIKE（trigram 无法生成 3-gram）
        let pattern = format!("%{}%", q);
        // ... 现有 LIKE SQL
    }
    list_transcriptions_at(conn, limit, offset)
}
```

- **`id IN (SELECT rowid FROM ... MATCH)`**：用子查询而非 JOIN——MATCH 的结果集是 rowid 列表，子查询语义清晰
- **SELECT 列与现有完全一致**——row_mapper 不变，`TranscriptionRecord` 结构不变

### 3. FTS5 query 转义

FTS5 MATCH 有自己的查询语法（`AND`/`OR`/`NOT`/`NEAR` 关键字、`*` 前缀通配符、`"` phrase、`(` `)` 分组）。用户搜索词可能含这些字符导致语法错误或非预期行为。

**策略**：用双引号包裹为 phrase query（trigram tokenizer 对 phrase 做连续 3-gram 匹配，语义等价子串匹配），内部双引号双写转义：

```rust
/// 转义 FTS5 MATCH 查询：用双引号包裹为 phrase，内部双引号双写。
/// trigram tokenizer 对 phrase 做连续 3-gram 匹配，语义等价子串匹配。
fn escape_fts5_match(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}
```

| 输入 | 输出 | MATCH 行为 |
|------|------|-----------|
| `会议纪要` | `"会议纪要"` | trigram `会议纪`/`议纪要` 子串匹配 |
| `a"b` | `"a""b"` | 含双引号的子串匹配 |
| `AND` | `"AND"` | 字面子串 "AND"，非逻辑运算符 |
| `test*` | `"test*"` | 字面子串 "test*"，非前缀通配 |

## 测试

在 db.rs `#[cfg(test)] mod tests` 新增（用 `open_init()` 内存 DB 走真实代码）：

1. **backfill 后搜索命中历史行**：INSERT voice 行 → 模拟 v17（不经过 v18 backfill）→ 搜索 LIKE 命中但 MATCH 不命中 → 跑 backfill → MATCH 命中
2. **>=3 字符走 MATCH**：INSERT "会议纪要很好" → 搜"会议纪要"（4 char）→ MATCH 命中
3. **<3 字符回退 LIKE**：INSERT "会议纪要" → 搜"会议"（2 char）→ LIKE 命中（MATCH 会漏，所以回退）
4. **特殊字符查询不报错**：搜 `a"b` / `AND` / `test*` → 不 panic、不 SQL 错误
5. **空 content 不索引**：INSERT content="" 的 voice 行 → backfill 不报错 → 搜任意词不命中

## 验证

```bash
cargo test -p octopus-infra -- fts5
cargo test -p octopus-infra -- list_transcriptions
cargo clippy -p octopus-infra
```

## 文档同步

- `docs/architecture.md`：infra DB 段补"voice 历史搜索走 FTS5 MATCH（trigram），<3 字符回退 LIKE"
- `docs/superpowers/specs/2026-07-05-code-review-remediation-design.md`：I-H3 标记 ✅
