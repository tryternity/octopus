# 热词管理增强（容量限制 + fuzzy 搜索 + 批量添加浮窗）

> **日期**：2026-08-01
> **状态**：✅ 已实现
> **背景**：PR 0801 热词页 UI 重构过程中的功能增强。与 [hotword-streaming-effective](2026-08-01-hotword-streaming-effective.md)（流式纠错）互补——本 spec 聚焦热词**管理**侧（容量/搜索/批量添加）。

---

## 1. 动机

热词页多轮 UI 调整中暴露的三个管理侧问题：
1. **无容量限制**：单个词典词数无上限，`HotwordIndex::from_words` 构建 O(N) + fuzzy `match_score` 逐词 O(N)，词数过大影响启动 + 搜索性能。
2. **搜索无拼音**：原搜索是纯字符 `includes`（连续子串），不支持拼音首字母（如「by」匹配「八爪鱼」bzy），与 ActionBar 的 fuzzy 体验不一致。
3. **添加入口分散**：单词 input + 批量导入两套，单词 input 占操作行空间且与批量场景重复。

## 2. 容量限制：单词典 3000 词

### 2.1 设计

`crates/infra/src/db/hotword.rs` 新增 `pub const HOTWORD_SET_MAX_WORDS: usize = 3000` + `ensure_within_capacity(prospective_words_text)` 校验函数。三个写入入口统一校验（normalize 后词数 > 3000 即 bail）：

| 入口 | 用途 |
|---|---|
| `set_hotword_set_words` | 覆盖导入 / 新建导入 |
| `add_word_to_set_at` | 单词追加 |
| `add_words_to_set` | 批量追加（挖掘确认、批量添加浮层） |

超限错误消息：「词典容量已满（3000 词上限），建议另建新词典分摊（当前 N 词）」。前端各入口 catch 后 toast 显示，用户即获知容量限制 + 应对建议。

### 2.2 为什么 3000

- `HotwordIndex::from_words` 启动时构建 O(N) 索引——每词典数千词 × 多词典，启动延迟可感知。
- `filter_hotwords_fuzzy` 搜索时逐词 `match_score` O(N)——3000 词单次 ~ms 级，可接受；万级会卡顿。
- 3000 覆盖典型场景（专业术语/专有名词），超出建议分摊到多词典（用户可创建多个版本，勾选叠加生效）。

### 2.3 测试

TDD 3 测试（`crates/infra/src/db/hotword.rs`）：
- `set_words_respects_capacity_limit`：恰好 3000 通过 / 3001 被拒
- `add_words_rejects_when_exceeding_capacity`：批量追加超限被拒，原内容不动（不部分写入）
- `add_single_word_rejects_when_at_capacity`：满后再加一词被拒

## 3. fuzzy 搜索：复用 matcher::match_score

### 3.1 设计

新增 Tauri command `filter_hotwords_fuzzy(query: String, words: Vec<String>) -> Vec<String>`（`crates/desktop/src/commands/hotword_commands.rs`），复用 `octopus_search::matcher::match_score`（与 ActionBar 同款算法）：

```
对每个 word 调 match_score(query, word)，过滤 None，按 score 降序返回
```

`match_score` 五级 scoring（取最高）：exact (10000) > prefix (5000-) > word-prefix (4500-) > pinyin (4000-) > fuzzy (nucleo)。拼音首字母用 `pinyin` crate（`ToPinyin::first_letter()`，多音字取常用读音），如「八爪鱼」→ `bzy`，「by」匹配 `bzy` 的 `contains` → 1000 分。

### 3.2 为什么复用后端 matcher

ActionBar 的 fuzzy 在 **Rust 后端**（`crates/search/src/matcher.rs`，依赖 `pinyin` + `nucleo-matcher` crate），前端无法 import。三个路径对比：
- **A（采纳）后端命令复用**：算法与 ActionBar 完全一致，零重复维护，热词列表通常 <几百条 IPC 开销可接受
- B 前端 port + pinyin-pro：引入几百 KB 拼音表依赖 + 两套算法易漂移
- C 抽前端 util 共享子序列：无拼音，不满足需求

### 3.3 前端

HotwordPanel 搜索改 async：`query` state 变化 → debounce 120ms → `invoke('filter_hotwords_fuzzy', { query, words })` → `fuzzyMatches` state（null=显示全部，string[]=命中集，已按 match_score 降序）。`visible` = fuzzy 命中集 + 用户选的 sort（字母/命中度）排序。placeholder「搜索（模糊）」。

### 3.4 测试

TDD 5 测试（`hotword_commands.rs`）：
- `fuzzy_filters_and_sorts_by_match_score`：汉字匹配 + 过滤
- `fuzzy_pinyin_initials_match`：「bzy」命中「八爪鱼」
- `fuzzy_exact_ranks_above_prefix`：exact 10000 > prefix
- `fuzzy_empty_query_returns_empty`
- `fuzzy_no_match_returns_empty`

## 4. 批量添加浮窗

### 4.1 设计

原操作行「单词 input + 添加按钮」删除，改为：CardHeader 排序图标后跟 4 个图标按钮（追加 Upload / 覆盖 RefreshCw / 挖掘 Wand2 / 添加 Plus）。「添加」点击弹出 textarea 浮层：

- textarea 接收批量文本，任意空白（空格/Tab/换行）分割
- 底部「添加」「取消」按钮
- outside-click / 取消关闭
- 确认调 `add_words_to_set`（批量），受 3000 词容量限制

「挖掘」也改浮窗（原内联面板）：候选词 chip 勾选 + 手动补词 + 添加/取消。

### 4.2 单词添加入口删除

原 `addWord`（单词 `add_word_to_set`）+ `input` state 删除——批量浮层涵盖单词场景（输入一个词即可）。`add_word_to_set` DB 函数保留（容量校验需覆盖它，且可能被其它路径调用）。

## 5. 不在本次范围

- **textarea 无长度上限提示**：用户可粘贴超长文本，容量限制会在后端拦住，但前端 textarea 无实时计数。可考虑前端实时显示词数 + 超限预警（P2）。
- **fuzzy 搜索 IPC 延迟压测**：debounce 120ms 已加，但接近 3000 词时未压测极端情况。

## 6. 代码位置速查

| 位置 | 作用 |
|---|---|
| `crates/infra/src/db/hotword.rs::HOTWORD_SET_MAX_WORDS` | 容量常量 3000 |
| `crates/infra/src/db/hotword.rs::ensure_within_capacity` | 容量校验（三写入口共用） |
| `crates/desktop/src/commands/hotword_commands.rs::filter_hotwords_fuzzy` | fuzzy 搜索 Tauri command |
| `crates/search/src/matcher.rs::match_score` | 复用的 fuzzy 算法（exact>prefix>word-prefix>pinyin>fuzzy） |
| `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` | fuzzyMatches state + debounce + 添加浮窗 + 挖掘浮窗 |
