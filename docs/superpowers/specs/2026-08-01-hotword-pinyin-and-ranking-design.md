# HotwordIndex 拼音优化 + correct 多命中排序

> **日期**：2026-08-01
> **状态**：🔜 待实现
> **依赖**：v57 hotword_words 表（已完成，DB 已存原始拼音）

## 1. 动机

两个耦合问题，合并一个 spec：

- **P1 拼音优化**：DB 的 `hotword_words.pinyin` 已存原始拼音（`word_plain_pinyins`），但 `HotwordIndex::from_words` 仍对每个字现算 `char_fuzzy_pinyin`（含 `to_pinyin()`）。correct 热路径（流式每个 Partial 帧）的查询侧 `get_fuzzy_pinyin` 也对滑窗字符重复算拼音。DB 拼音没用上。
- **P2 多命中排序**：`correct_greedy` 多命中时 `candidates.iter().find(|c| *c != &window_word)` 取第一个——顺序来自 `list_active_words` 的 HashSet 迭代（**不确定**）。同音多热词命中哪个取决于哈希运气。

## 2. P1：from_words 用 DB 拼音

### 现状
```rust
pub fn from_words(words: &[String]) -> Self {
    // 对每个字调 char_fuzzy_pinyin(c) = to_pinyin().plain() → normalize_fuzzy_pinyin
    let py: Vec<String> = chars.iter().filter_map(|&c| char_fuzzy_pinyin(c)).collect();
    let key = py.join("-");  // 归一化后的 key
}
```

### 改造
`from_words` 改接收带拼音的结构，跳过 `to_pinyin()` 部分，但仍需 `normalize_fuzzy_pinyin` 归一化生成 key（方言规则运行时生效）：

```rust
pub fn from_words(entries: &[(String, String)]) -> Self {
    // entries = [(word, pinyin)]，pinyin 是 DB 存的原始拼音（空格分隔 "ba zhao yu"）
    for (w, raw_pinyin) in entries {
        let chars: Vec<char> = w.chars().collect();
        let len = chars.len();
        if len < 2 { continue; }
        // 原始拼音 split → 逐字 normalize_fuzzy_pinyin（跳过 to_pinyin）
        let py: Vec<String> = raw_pinyin.split_whitespace()
            .map(|p| normalize_fuzzy_pinyin(p))
            .collect();
        if py.len() != len { continue; }
        let key = py.join("-");
        // ...
    }
}
```

**注意**：`normalize_fuzzy_pinyin` 当前签名是 `(&str) -> String`，输入单个拼音音节。DB 存的 `pinyin` 是空格分隔的多音节（如 `"ba zhao yu"`），split 后逐个 normalize。

### 调用链适配
- `reload_hotwords`（corrector.rs）：`list_active_words()` 已返回 `Vec<(word, pinyin)>`，直接传给 from_words（当前临时方案 `.map(|(w, _)| w.clone())` 丢弃拼音——改为直接传）
- 查询侧 `get_fuzzy_pinyin`（corrector.rs:30-40）：仍需现算（识别文本是动态的，DB 没存），**不改**

### 收益
建索引时（reload_hotwords / reload_fuzzy_dialect）跳过 `to_pinyin()` 查表（3000 词 × 平均 3 字 = 9000 次 to_pinyin）。查询侧不变（识别文本动态）。

## 3. P2：多命中按 hit_count 排序

### 现状
```rust
// correct_greedy L150-151
if let Some(hw) = candidates.iter().find(|c| *c != &window_word) {
    // 取第一个非原词候选——顺序不确定（HashSet 迭代序）
}
```

### 改造

**HotwordIndex 携带 hit_count**：
```rust
pub struct HotwordIndex {
    by_len_py: HashMap<usize, HashMap<String, Vec<(String, i64)>>>,  // (word, hit_count)
    active_words: HashSet<String>,
}
```

`from_words` 接收 `&[(word, pinyin, hit_count)]`（或从 `hotword_hits` JOIN）。`lookup` 返回 `&Vec<(String, i64)>`。

**find_candidates 排序**：
```rust
fn find_candidates(&self, query_word: &str) -> Vec<String> {
    let mut candidates: Vec<(String, i64)> = idx.lookup(...).cloned().unwrap_or_default();
    // 按 hit_count 降序（命中多的优先——用户验证过的更可信）
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    // 原词追加末尾（不参与排序）
    if !candidates.iter().any(|(w, _)| w == query_word) {
        candidates.push((query_word.to_string(), 0));
    }
    candidates.into_iter().map(|(w, _)| w).collect()
}
```

**correct_greedy 不变**——`candidates.iter().find(|c| *c != &window_word)` 仍取第一个，但现在第一个是 hit_count 最高的（确定性）。

### hit_count 来源
`hotword_hits` 表（全局 word→count，不绑 set）。`list_active_words` JOIN `hotword_hits` 带出 hit_count，或 corrector 加 `hits_cache`（类似 `FUZZY_RULES_CACHE` 模式，pipeline bump 后刷新）。

**选择 JOIN 方案**（简单）：`list_active_words` 返回 `Vec<(word, pinyin, hit_count)>`，LEFT JOIN hotword_hits，无命中记录 hit_count=0。

### 平局处理
hit_count 相同时，按 word 字典序（确定性，避免 HashSet 序）。`sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)))`。

## 4. 不在范围

- correct 热路径查询侧拼音缓存（per-char cache 消除重叠滑窗重复计算）——单独的性能优化，后续
- HotwordIndex 增量更新（当前全量重建）——后续

## 5. 测试

- P1：`from_words` 接收带拼音的 entries，验证 key 生成正确（原始拼音经 normalize 后与现算一致）
- P2：多命中时 hit_count 高的优先；hit_count 相同按字典序；零 hit_count 候选仍可用
