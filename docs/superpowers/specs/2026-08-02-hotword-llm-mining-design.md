# LLM 热词挖掘——语义提取替代 jieba 分词

> **日期**：2026-08-02
> **状态**：🔜 待实现
> **依赖**：octopus-llm crate（已集成）、`list_recent_edited_segments`（已实现）

## 1. 动机

当前挖掘用 jieba 分词 + 词频统计从用户编辑段提取候选词。问题：
- **jieba 分词不准**：产品名/术语（"八爪鱼""浮窗"）被拆成单字
- **is_common_word 过滤覆盖不全**：依赖 jieba unigram 词表，常用词漏过过滤
- **纯统计无语义**：无法区分"八爪鱼"(专名)和"实际上"(常用词)

LLM 能理解语义，直接从文本提取完整专有名词，不依赖分词质量。

## 2. 方案：LLM 提取 + jieba 兜底

### 流程

```
collect_candidate_words()
  → list_recent_edited_segments(500)    // 用户编辑段文本
  → 拼接成 prompt
  → octopus_llm::mine_hotwords(prompt)  // LLM 提取专有名词
  → 成功 → LLM 返回的词列表（过滤已有热词 + is_candidate 基本检查）
  → 失败 → 回退 jieba 分词（现有逻辑）
```

### LLM prompt

```
以下文本来自语音识别后用户手动编辑纠正的片段。
其中包含语音引擎容易识别错的专有名词（人名/地名/产品名/技术术语/项目名）。
请从中提取这类专有名词，要求：
1. 只提取专有名词，不含常用词（如"我们""这个""可以"）
2. 每行一个词，不要标点、不要编号
3. 词长 2-6 字
4. 如果文本中没有专有名词，返回空

文本：
{edited_texts_joined}
```

### octopus_llm 接口

```rust
/// 用 LLM 从文本中提取热词候选（专有名词）。
/// 复用润色 LLM 客户端（同 API key / endpoint）。
/// 失败返回 Err（调用方回退 jieba 分词）。
pub async fn mine_hotwords(text: &str) -> Result<Vec<String>>;
```

### 降级策略

- LLM 未配置（无 API key）→ 回退 jieba 分词
- LLM 调用失败（网络/超时）→ 回退 jieba 分词
- LLM 返回空 → 回退 jieba 分词
- jieba 兜底用现有逻辑（从编辑段提取 + 频次降序）

## 3. 前端交互

挖掘按钮已有（Wand2 图标）。改为异步等待（LLM 调用 1-3 秒）：
- 点击后显示 loading 状态
- LLM 返回后弹出候选确认浮窗（已有 minePending 浮窗）
- 失败静默回退 jieba（用户无感，只是结果质量差一点）

## 4. 不在范围

- LLM 挖掘的定时自动跑（当前只手动触发）
- LLM 挖掘结果直接落库（仍需用户确认）
- 挖掘结果缓存（每次点按钮重新调 LLM，挖掘是低频操作）
