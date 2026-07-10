# ASR 热词系统 — 设计

- 日期：2026-07-09
- 分支：asr-wordbook（worktree）
- 状态：实施中（实施计划见 `plans/2026-07-10-asr-hotword.md`；v1 HotwordStore/Index/corrector 重构 + CandidateMiner + HotwordPanel UI 已落地，方言/排序等增补 Task 推进中）

## 背景

用户希望：日常高频说但容易被误识别的词（人名/地名/术语/口头禅等专名），通过「热词」机制提升识别准确率。核心诉求是「热词能否减少误识别」——答案是**能，但前提是热词表驱动、有界替换**，而非全词表无差别重打分。

### 调研结论：当前 11 个引擎的热词原生支持

| 引擎 | 路径 | 原生热词 | 机制 |
|---|---|---|---|
| Paraformer | 本地 ONNX | ⚠️ 需特定变体 | 仅 `paraformer-large-contextual`（CLAS/SeACoParaformer）支持，标准变体不支持 |
| SenseVoice | 本地 ONNX | ❌ | 多任务模型，热词非强项（官方 issue #2534） |
| FireRedASR | 本地 ONNX | ❌ | 高频词准、低频专名差，社区强烈要求未实现（issue #42/#87） |
| Qwen3-Audio | 本地 ONNX | ⚠️ 潜力 | audio-LLM 原则可注入文本上下文，本地 wrapper 是否暴露待验 |
| Moonshine | 本地 ONNX | ❌ | E2E encoder-decoder，无 prompt 通道 |
| Whisper | 本地 ONNX | ⚠️ trick | 仅 `initial_prompt` 注入偏置，效果有限 |
| Zipformer | 本地 ONNX | ⚠️ 运行时 | sherpa-onnx lattice 偏置，取决于运行时 |
| 阿里云 / 百度 / 火山 / 腾讯 | 云端 | ✅ | 四家全部原生支持（热词表 / 上下文增强 / WS 配置） |

**关键判断**：本地引擎原生热词能力极不均衡（基本只有 Paraformer-contextual 变体），云端四家全支持。要让**所有引擎统一受益**，必须有一条**与引擎无关的后处理热词层**作为打底。

### 与现有 corrector 的关系（过纠债）

`crates/asr-local/src/corrector.rs` 已实现 jieba 分词 + 模糊拼音 + unigram/bigram 重打分。但记忆里记录其**过纠**：把模型正确的「开始语音识别」改成「开始于饮食别」；sensevoice/qwen3/cloud 已 `skip_corrector(true)` 规避。根因是**全词典自由联想重打分**——模型正确输出被更低 bigram 分数的错误候选盖过。

热词系统的正确形态恰是 corrector 的**有界版本**：候选集仅来自热词表，不再全词典重打分 → 过纠根因消失。

## 目标

- 全部 11 个引擎（7 本地 + 4 云端）统一获得热词纠错能力
- 顺手清掉 corrector 过纠债：有界候选集让 sensevoice/qwen3 等高质量引擎重新安全启用纠错
- 热词来源：自动挖掘历史高频专名候选 + 人工确认（混合模式）
- 匹配：同音 + 复用 corrector 已验证的模糊拼音规则（zh/z·sh/s·ch/c·n/l·r/l·ing/in·eng/en·ang/an）

## 非目标（v1）

- 本地 paraformer-contextual 变体（需换模型，列为未来）
- 英文/多语言热词（v1 仅中文拼音路径；英文需编辑距离/音素匹配，未来）
- whisper `initial_prompt` 注入（未来，效果有限）
- 全自动入表（无人工确认）——会学进模型系统性误识别的错词，否决
- 通用同音消歧（它/她/他、在/再）——这是旧 corrector 过纠的来源之一，有界版放弃，换来零过纠

## 关键决策

1. **整体走向**：分阶段——L2 后处理热词纠错打底（v1）+ L1 原生热词叠加（v2 云端）。
2. **热词来源**：混合。CandidateMiner 从历史 transcript 挖低频高频专名 → 写 `Pending` → 设置页人工确认 → `Active`。
3. **匹配宽容度**：适中。归一化模糊拼音**精确等价**匹配（非编辑距离），字符必须不同才替换。两道闸控误命中。
4. **与 corrector 关系**：方案 A——重构 `corrector.rs` 为热词有界纠错。复用模糊拼音机制，候选集从全词典改为热词表。

## 架构

```
                 ┌─────────────────────────────────────────┐
  录音 → ASR 引擎 │  raw_text                               │
 (11 引擎)       └───────────────┬─────────────────────────┘
                            [L2] │ 热词有界纠错（corrector 重构）
                    ┌────────────▼───────────────┐
                    │ BoundedHotwordCorrector    │  ← 全局单例
                    │  · 文本→拼音序列           │
                    │  · 滑窗匹配热词 sound-pattern│
                    │  · 模糊拼音（同 corrector） │
                    │  · 命中且字符不同→替换     │
                    │  · 无命中→原样返回(零过纠) │
                    └────────────┬───────────────┘
                                 │ corrected_text → 显示/落库
  ┌──────────────────────────────┴──────────────────────────┐
  │ HotwordStore (DB hotwords 表)                            │
  │  word | status(Pending/Active) | source(Manual/Mined)   │
  └───────────────┬──────────────────────────┬──────────────┘
        [挖掘]    │                          │  [L1 叠加 v2]
  历史 transcript │                          │  active 词表透传
  jieba 分词+词频 │                          │  → 云端 4 provider
  低频高频词→Pending│                         │  session.update/START 帧
  (人工确认→Active)│
```

## 组件

| 组件 | 职责 | 位置 |
|---|---|---|
| **HotwordStore** | DB CRUD（active/pending 两态、增删改查） | `infra/src/db.rs` 新增 `hotwords` 表，db.sql + user_version v19→v20 |
| **HotwordIndex** | 内存索引：`(词长, 归一化模糊拼音) → Vec<热词>` + `active_words: HashSet`；`Arc<RwLock>`，热词变更时 reload | 新增 `asr-local/src/hotword.rs` |
| **BoundedHotwordCorrector** | 复用 `corrector.rs` 的 `normalize_fuzzy_pinyin`/`get_fuzzy_pinyin`；`correct()` 改滑窗拼音匹配，候选集仅来自 HotwordIndex | 重构 `corrector.rs` |
| **CandidateMiner** | 周期/按需扫历史 transcript，jieba 分词+词频，滤常用词（jieba 词频阈值），低频高频专名→`Pending` | 新增，会话结束或按需触发 |
| **CloudHotwordAdapter**（v2） | active 词表注入各 provider 会话起始 JSON | `asr-cloud/src/*_stream.rs` |

### 热词数据模型

DB 表 `hotwords`：

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK | |
| `word` | TEXT UNIQUE | 热词文本，如「八爪鱼」 |
| `status` | TEXT | `pending`（挖掘待确认）/ `active`（生效） |
| `source` | TEXT | `manual` / `mined` |
| `hit_count` | INTEGER | 纠错命中次数（消歧与排序用），默认 0 |
| `created_at` | INTEGER | unix 秒 |

## 数据流

**纠错流**（每次识别）：raw_text → `get_corrector().correct(&t)` → 文本逐字转拼音 → 按热词长度滑窗 → 窗口归一化模糊拼音匹配 HotwordIndex 且字符不同 → 替换该 span → 返回。无热词或无命中时直接返回原文（零开销）。

**挖掘流**（后台）：历史 transcript → jieba 分词 → 词频统计 → 滤常用词（jieba 默认词典词频 > 阈值）→ top-N 低频高频专名写 `Pending` → 设置页人工确认 → `Active` → 触发 HotwordIndex reload + `jieba.add_word(word)`（让热词成为原子 token，改善后续分词与挖掘）。

## 与现有代码的关系

- **调用点不变**：`asr-local/src/pipeline.rs:58`（批处理）与 `streaming_runner.rs:316-324`（流式 Partial/Committed）仍走 `get_corrector().correct(&t)`，零改调用方。
- **`skip_corrector()` 重估**：有界版无热词即 no-op、只向显式热词纠偏，过纠不可能发生 → sensevoice/qwen3/cloud 可重新打开热词纠错（受益面扩大）。实施时逐引擎核实，确认安全后清除其 `skip_corrector()` 返回 true。
- **`asr_correct` 旗标**：保留为主开关，语义从「拼音纠错」变为「热词纠错」。
- **`is_english` 守卫保留**（pipeline.rs:58）：v1 仅中文拼音路径。

## 错误处理与边界

- 热词表空 → `correct()` 立即返回原文（跳过 jieba 与滑窗），零成本。
- 多热词同音（同拼音异字）→ 取 `hit_count` 最高者，并列取首个，记日志。
- 误命中风险 → 两道闸：① 归一化模糊拼音精确等价（非编辑距离）② 替换前提是窗口字符与热词不同。
- 流式 Partial 即时纠偏：有界且廉价（哈希查表），行为形态同现状，无 flicker。
- reload 竞态：`Arc<RwLock>`，写时整体替换 index（COW 式），不阻塞纠错热路径读。
- 单例持有 `Arc<RwLock<HotwordIndex>>`，暴露 `reload_hotwords(new_index)`；DB 写路径构造新 index 后调 reload，纠错热路径读锁非阻塞。

## 分阶段交付

- **v1（本 spec 范围）**：HotwordStore + HotwordIndex + BoundedHotwordCorrector 重构 + CandidateMiner + 设置页热词管理 UI（增删 / 确认 Pending；生效热词卡片网格 + 拼音首字母搜索 + 时间/字母/命中度排序）。覆盖全部 11 引擎的 L2。
- **v2**：CloudHotwordAdapter——active 词表透传 4 家云端（JSON 字段级，每家一处小改：aliyun session.update / baidu START 帧 / bytedance / tencent 各自热词字段）。
- **未来**：本地 paraformer-contextual 变体；英文热词编辑距离；whisper initial_prompt；通用同音消歧（独立「激进模式」开关，默认关）。

## 测试策略

- **单测（核心）**：
  ① 给定文本+热词集，断言正确替换（含跨 jieba 边界的误识别 span）；
  ② 无命中断言原样返回；
  ③ **过纠回归**——`"开始语音识别"` 无对应热词时必须原样返回（直接对峙历史过纠案例）；
  ④ 多热词同音消歧取 hit_count 最高；
  ⑤ 模糊拼音容错（前后鼻音/平翘舌读错能救回）。
- **集成**：HotwordStore CRUD；Miner 候选生成（喂构造 transcript 断言 Pending 内容与常用词过滤）。
- **e2e（铁律）**：真实录音含一个会被模型误识别的专名 → active 热词后，断言最终文本含该热词。**直调 engine 绕过 corrector 会掩盖效果，必须走 pipeline 全链路**（对齐历史 CMVN/corrector e2e 教训）。

## 调研来源

- FunASR 选型（SenseVoice vs Paraformer 热词）：https://www.funasr.com/blog/which-funasr-model.html
- Paraformer-large-contextual ONNX：https://modelscope.cn/models/iic/speech_paraformer-large-contextual_asr_nat-zh-cn-16k-common-vocab8404-onnx
- SenseVoice 热词 issue #2534：https://github.com/modelscope/FunASR/issues/2534
- sherpa-onnx hotwords（通用 contextual biasing 运行时）：https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html
- FireRedASR 仓库与热词 issue #42/#87：https://github.com/FireRedTeam/FireRedASR
- Moonshine：https://github.com/moonshine-ai/moonshine
- 阿里云热词与上下文增强：https://www.alibabacloud.com/help/zh/model-studio/improve-asr-accuracy
- Qwen3-ASR-Flash Flexible Contextual Biasing：https://qwen.ai/blog?id=41e4c0f6175f9b004a03a07e42343eaaf48329e7
- whisper.cpp prompting：https://developers.openai.com/cookbook/examples/whisper_prompting_guide

---

## 方言模糊规则可配（2026-07-10 增补）

不同口音有不同声母/韵母混淆习惯（f/h、n/l、r/l、hu/wu 四组）。v1 模糊规则硬编码（n→l 默认开 + 平翘舌 + 前后鼻音）无法覆盖 f↔h、r↔l、hu↔wu，且 n→l 默认开对标准普通话用户增加误命中。

### 设计
- 四组方言做成**用户可勾选开关**（设置页「热词」面板，复用 Settings 的 Card/Row/Toggle 设计语言），存 `app_config.fuzzy_dialect`（逗号分隔 token：`f/h`、`hu/wu`、`n/l`、`r/l`），默认空 = 仅基础规则。
- 基础规则（平翘舌 zh/ch/sh→z/c/s + 前后鼻音 ing/eng/ang→in/en/an）**始终开**，不做开关。
- 归一化单向（查询与索引共用 `normalize_fuzzy_pinyin` → 双向对称命中）：
  - `f/h`：声母 f→h
  - `n/l`：声母 n→l（**行为变更**：从 v1 默认开改为可选）
  - `r/l`：声母 r→l（n、r、l 在 n/l + r/l 同开时都归一到 l，首字母不同互不冲突）
  - `hu/wu`：单字 hu→wu，其余 huX→wX（huang→wang、hua→wa）

### 已知局限
- `hu/wu` 覆盖 hu↔wu 单字及 huang↔wang / hua↔wa（h 声母+u 介音 vs 零声母 w），**不覆盖** hui↔wei（韵母 ui/ei 不同，拼音级无法统一）。
- `r/l` **仅救首字**：如「热词→乐视」，r/l 把「热 re→le」与「乐 le」归一一致（第一字命中），但第二字「词 ci」与「视 shi」（基础 sh→s 得 si）ci≠si 不匹配——sh/c 刻意不归一（避免级联误命中）。r/l 对纯 r/l 混淆（热↔乐、肉↔漏、人↔林）完整有效。

### 规则生效
`FuzzyRules`（`hotword.rs` 全局 `OnceLock<RwLock>`）→ `normalize_fuzzy_pinyin` 读全局。规则变更经 `corrector::reload_fuzzy_dialect` set 全局 + 用缓存 `active_words` 重建 `HotwordIndex`（索引 key 由 normalize 生成，规则变 key 必变）。
