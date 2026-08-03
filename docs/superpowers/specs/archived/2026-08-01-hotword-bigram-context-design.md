# bigram 上下文打分——热词多命中排序增强

> **日期**：2026-08-01
> **状态**：✅ 已实现（字级 bigram 上下文打分 + scheduler 定时构建）
> **依赖**：P2 hit_count 排序已完成（`find_candidates` 已按 hit_count 降序）

## 1. 动机

当前 `find_candidates` 多命中排序只有 hit_count + 字典序。用户反馈：**上下文更重要**——同一拼音命中的多个热词，应优先选「在这个上下文位置更像」的。hit_count 和拼音匹配度作为辅助。

## 2. 权威参考

搜索 ASR 后纠错领域工程实践：
- **微软经典 N-Gram**：候选用 character bigram overlap 生成，排序「最大化 local 5-gram 上下文频次」
- **工业共识**：拼音相似度做候选召回（我们已用 `normalize_fuzzy_pinyin` 归一化实现），n-gram 上下文频次做排序
- **PERL 2024 SOTA**：拼音特征 + 语义特征 attention 动态融合——需预训练模型，对轻量场景过重

**核心原则**：上下文频次主导排序，hit_count 辅助，拼音匹配度已在召回层实现不重复算。

## 3. 打分公式

```
score(候选词 w, prev_char, next_char) =
    bigram_score(w, prev_char, next_char) * W_CONTEXT
    + hit_count(w) as f64 * W_HIT
```

- `bigram_score = bigram_freq[(prev_char, w首字)] + bigram_freq[(w末字, next_char)]`——候选词在当前位置前后字级 bigram 频次（用户历史 voice 语料统计）。无边界时对应项 = 0
- `W_CONTEXT = 1.0`（主导），`W_HIT = 0.3`（辅助）
- 平局 → word 字典序（确定性）

**字级 bigram**（不分词）：构建和查询都极轻量。中文「打开八爪鱼」→ bigrams: (打,开)(开,八)(八,爪)(爪,鱼)。候选词「八爪鱼」在「打开X」位置时，查 `(开,八)` 前缀 bigram + `(鱼,?)` 后缀 bigram。

## 4. bigram 索引

### 语料
仅 voice 记录（`WHERE item_type='voice'`）——ASR 识别结果，跟纠错场景一致。

### 构建
`build_char_bigram_index(texts: &[String]) -> HashMap<(char, char), usize>`：对每条文本取相邻字符对计数。跳过非汉字字符对（可选——纯统计，含标点也无害，标点 bigram 自然频次低）。

### 时机
挂 scheduler 定时任务（CPU 空闲时跑，interval 600s）。首次 CPU 空闲 tick 立即建。构建在子线程跑（参考 vault_sync），结果写入 `LightCorrector` 的 `RwLock<HashMap<(char,char), usize>>`（锁外预建 + 整体替换）。

## 5. find_candidates 改造

签名加 prev_char / next_char 参数。correct_greedy 已有 chars 数组，窗口 `[i..i+sz]` 的 prev = `chars[i-1]`（i>0 时），next = `chars[i+sz]`（i+sz<n 时）。无边界传 `'\0'`（bigram 查 `('\0', x)` 必返 0）。

排序改为 `bigram_score * W_CONTEXT + hit_count * W_HIT` 降序 + 字典序。

## 6. 不在范围

- 拼音编辑距离（归一化已做模糊召回，编辑距离是扩大召回，后续）
- 词级 bigram（字级已足够轻量，词级需分词开销，后续）
- 时间衰减（旧 bigram 权重降低，后续）
