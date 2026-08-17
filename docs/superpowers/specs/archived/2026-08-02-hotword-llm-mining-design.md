# LLM 热词挖掘——语义提取替代 jieba 分词

> **日期**：2026-08-02
> **状态**：✅ 已实现
> **依赖**：octopus-llm crate（已集成）、`list_recent_edited_segments`（已实现）

## 1. 动机

当前挖掘用 jieba 分词 + 词频统计从用户编辑段提取候选词。问题：
- **jieba 分词不准**：产品名/术语（"八爪鱼""浮窗"）被拆成单字
- **is_common_word 过滤覆盖不全**：依赖 jieba unigram 词表，常用词漏过过滤
- **纯统计无语义**：无法区分"八爪鱼"(专名)和"实际上"(常用词)

LLM 能理解语义，直接从文本提取完整专有名词，不依赖分词质量。

## 2. 方案：LLM 优先 + jieba 兜底

### 流程

```
list_hotword_candidates()（desktop 命令，async）
  → list_recent_edited_segments(500)     // 用户编辑段文本
  → 预处理过滤（标点/单字/纯数字/无汉字）
  → 拼接成 prompt
  → octopus_llm::mine_hotwords(prompt, joined, &llm_config)  // LLM 提取
  → 成功非空 → LLM 返回的词列表
  → 失败/空 → 回退 miner::collect_candidate_words()（jieba 从编辑段提取）
```

### 预处理过滤

Edited 段可能含噪声（标点 `""，。`、单字、纯数字、纯英文），发给 LLM 前过滤：
- `<2 字`（单字/标点）
- 无汉字（纯英文/标点）
- 纯数字
- 过滤后空 → 直接返回空（不调 LLM）

### 挖掘提示词

独立 .md 文件：`crates/desktop/resources/hotword_mine.md`（include_str! 编译期嵌入）。
用户可在 `~/.octopus/HOTWORD_MINE.md` 放自定义版本覆盖。
提示词含结构化的正例（人名/地名/术语）、反例（常用词/动词）、判断要点、输出格式。

### octopus_llm 接口

```rust
/// 用 LLM 从用户编辑段文本中提取热词候选（专有名词）。
/// 复用润色 LLM 客户端（同 API key / endpoint）。
/// system_prompt = 挖掘提示词（调用方从 resource 文件读，允许用户自定义覆盖）。
/// 失败返回 Err（调用方回退 jieba 分词挖掘）。
pub fn mine_hotwords(system_prompt: &str, edited_texts: &str, config: &CompatibleLlmConfig) -> Result<Vec<String>>;
```

返回结果解析——每行一个词，只保留纯汉字行（2-6 字）。

### 降级策略

- 预处理后 cleaned 为空 → 直接返回空
- LLM 未配置（无 API key）→ 回退 jieba
- LLM 调用失败（网络/超时 30s）→ 回退 jieba
- LLM 返回空 → 回退 jieba
- jieba 兜底：只从 Edited 段提取，频次降序取 top 30

## 3. 前端交互

挖掘按钮已有（Wand2 图标）。`list_hotword_candidates` 改为 async：
- LLM 调用 1-3 秒，前端 `invoke` 自动等待
- 返回后弹出候选确认浮窗（已有 minePending 浮窗）
- 失败静默回退 jieba（用户无感，只是结果质量差一点）

## 4. 不在范围

- LLM 挖掘的定时自动跑（当前只手动触发）
- LLM 挖掘结果直接落库（仍需用户确认）
- 挖掘结果缓存（每次点按钮重新调 LLM，挖掘是低频操作）
