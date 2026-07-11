# 热词多版本管理（hotword-sets）— 设计

- 日期：2026-07-11
- 分支：worktree-hotword-management
- 状态：✅ 已实现（代码完成 2026-07-11，T1-T8 全通过 subagent-driven 两阶段 review；e2e 真实录音待用户验证，见 plans/2026-07-11-hotword-sets.md Task 9 Step 2-5）
- 前置：`docs/superpowers/specs/2026-07-09-asr-hotword-design.md`（v1 扁平热词，已实现并合入 main）

## 背景

v1 热词系统是一张**扁平单表** `hotwords`（`word UNIQUE / status(active|pending) / source / hit_count`）：所有 active 词全局进 `HotwordIndex`，识别后统一纠错。没有「版本 / 场景」概念——同一时刻所有 active 词都生效。

新需求：**不同工作 / 场景用不同的热词集合，像「主题」一样可切换**。比如做项目 A 时用一组人名术语、日常生活用另一组。并要求**支持纯文本导入（按空格分隔）与导出**。

### 用户决策（brainstorming 过程敲定）

1. **多选叠加**：同时可勾选多个版本生效，最终生效词 = 所有勾选版本词的并集（非单选主题）。
2. **版本 = 一坨纯文本词表**（`words_text`，空格分隔），存 DB；**不是**逐词 DB 行，也**不是**真实磁盘文件。
3. **命中统计全局化**：`hit_count` 按「词」全局记一份，**不绑版本 / 场景**——同词跨版本命中累加到同一计数。
4. **UI 仍给「逐词管理」体感**：词以卡片展示、可单词添加、可点卡片 `✕` 删除；系统在背后透明地维护那坨 `words_text`。
5. **导入 / 导出** 对接真实 txt；导入弹窗支持「新建版本 / 追加到已有 / 覆盖已有」三选项。
6. **挖掘保留但改造**：废弃 pending→逐词确认流；挖掘拉候选词 → 前端确认面板（默认全选、可取消/补词）→ 确认才追加到当前选中版本（不直接落库、无弹窗选版本）。
7. 不要「编辑全文」入口——批量编辑走「导出 → 外部编辑 → 重新导入覆盖」。

## 目标

- 多版本热词，多选叠加生效；版本可新建 / 重命名 / 删除 / 启停
- 版本 = 纯文本词表，支持 txt 导入（新建/追加/覆盖）与导出
- UI 卡片化，单词增删，体感等同逐词管理
- 命中统计全局，与版本解耦
- 保留 v1 的方言模糊规则（f/h、hu/wu、n/l、r/l）与有界纠错（零过纠）特性

## 非目标

- 真实磁盘文件存储 / 文件监听（用 DB 存文本，导入导出对接 txt 足够）
- 「编辑全文」入口（导出→改→导入覆盖替代）
- 英文 / 多语言热词（沿用 v1，仅中文拼音路径）
- 跨设备同步（本地 DB）
- L1 云端原生热词透传（v2，沿用 asr-hotword spec 的未来项）

## 关键决策

1. **数据模型三件套**：`hotword_sets`（版本，存 `words_text`）+ `hotword_hits`（全局命中）+ 废弃 `hotwords` 表（迁移后停用，不 DROP）。
2. **词不是 DB 实体**：词是 `words_text` 切出来的临时产物；只有命中统计需要按词落库（`hotword_hits`）。这消解了 v1 的 `word UNIQUE` 与「同词进多版本」的冲突。
3. **`words_text` 写入统一规范化**：任何写操作（单词增 / 卡片删 / 导入 / 挖掘）都走 `切词 → 去重 → 按拼音首字母排序 → 空格拼接`（sort key = `(pinyin_initials(word), word)`，首字母相同时按词文本 `localeCompare`）。`words_text` 始终是有序、去重的规范形态。
4. **生效词 = enabled 版本并集**：`SELECT words_text WHERE enabled=1` → 切词去重并集 → `HotwordIndex`。全关 = 空集 = corrector no-op（过纠为零）。
5. **命中全局**：corrector 命中替换某词 → `hotword_hits` 该词 +1（`INSERT ON CONFLICT(word) DO UPDATE`）。与版本无关。
6. **UI 透明维护文本**：用户感知「卡片 + 单词增删」，底层每次写都是系统在规范化 `words_text`。

## 架构

```
                 ┌────────────────────────────────────────────┐
  录音 → ASR 引擎 │  raw_text                                  │
 (11 引擎)       └────────────────────────────────────────────┘
                            [L2] │ 热词有界纠错（corrector，v1 重构沿用）
                    ┌────────────▼───────────────┐
                    │ BoundedHotwordCorrector    │  ← 全局单例
                    │  · 生效词 = enabled 版本    │
                    │    words_text 切词并集     │
                    │  · 滑窗拼音匹配 + 模糊规则 │
                    │  · 命中且字符不同→替换     │
                    │  · 命中 → hotword_hits +1  │  ← 新增：命中写库
                    │  · 无命中→原样返回(零过纠) │
                    └────────────┬───────────────┘
                                 │ corrected_text → 显示/落库
  ┌──────────────────────────────┴───────────────────────────────┐
  │ hotword_sets（版本）                hotword_hits（全局命中）  │
  │  id | name(UNIQUE) | enabled |       word(PK) | hit_count    │
  │  words_text | created/updated_at                                 │
  └───────────────────────────────────────────────────────────────┘
        ▲                          ▲                       ▲
        │ enabled toggle           │ 单词增/卡片删/        │ 导入(新建/追加/覆盖)
        │                          │ 导入/挖掘 → 规范化     │ 导出(写 txt)
        │                          │ 写回 words_text        │
     [设置页 热词面板]
        · 版本列表（checkbox enabled + 重命名/导出/删除）
        · 选中版本 → 卡片网格（单词添加 / 卡片✕删 / 命中数 inline）
        · 导入 / 导出 / 从历史挖掘（弹窗选目标版本）
```

## 组件

| 组件 | 职责 | 位置 / 动作 |
|---|---|---|
| **HotwordSetStore** | `hotword_sets` CRUD（新建/重命名/删除/enabled toggle/改 words_text）+ `hotword_hits` 增改查 | `infra/src/db.rs` 新增两表 + CRUD，schema v22→v23 |
| **normalize_words_text** | 写入规范化：切词（任意空白）→ 去重 → 按拼音首字母排序 → 空格拼接 | `asr-local/src/hotword.rs` 新增（复用 `pinyin_initials`） |
| **HotwordIndex** | 不变（v1 已实现），`from_words` 接收 enabled 并集切词结果 | `asr-local/src/hotword.rs`，零改 |
| **BoundedHotwordCorrector** | 命中替换时新增「写 `hotword_hits +1`」；其余沿用 v1 | `asr-local/src/corrector.rs`，`correct()` 命中分支加一行 |
| **CandidateMiner** | 改造为「返回候选词列表」，不再直接写 pending | `asr-local/src/miner.rs`，`mine_pending_candidates` → `collect_candidate_words`（返 `Vec<String>`） |
| **hotword_commands** | 重写 Tauri 命令：版本 CRUD / enabled / 单词增删 / 导入 / 导出 / 挖掘 / list（带命中数） | `desktop/src/hotword_commands.rs` 重写 |
| **HotwordPanel** | 重写 UI：版本管理 Card + 卡片网格 + 导入导出挖掘 | `frontend/.../Settings/HotwordPanel.tsx` 重写 |

### 数据模型

**`hotword_sets`（版本）**

| 列 | 类型 | 说明 |
|---|---|---|
| `id` | INTEGER PK AUTOINCREMENT | |
| `name` | TEXT NOT NULL UNIQUE | 版本名，如「通用」「项目A」 |
| `enabled` | INTEGER NOT NULL DEFAULT 1 | 0/1，是否勾选生效（多选叠加） |
| `words_text` | TEXT NOT NULL DEFAULT '' | 空格分隔的规范词文本 |
| `created_at` | TEXT NOT NULL DEFAULT (datetime('now')) | |
| `updated_at` | TEXT NOT NULL DEFAULT (datetime('now')) | |

**`hotword_hits`（全局命中）**

| 列 | 类型 | 说明 |
|---|---|---|
| `word` | TEXT PRIMARY KEY | 热词文本 |
| `hit_count` | INTEGER NOT NULL DEFAULT 0 | 全局纠错命中次数 |

**迁移（schema v22 → v23）**：
1. `CREATE TABLE hotword_sets` / `CREATE TABLE hotword_hits`（`db.sql` 追加，IF NOT EXISTS 幂等）。
2. 建「通用」版本：`INSERT INTO hotword_sets(name, enabled, words_text) VALUES('通用', 1, <现有 active 词规范化拼接>)`。
3. 迁命中：`INSERT INTO hotword_hits(word, hit_count) SELECT word, hit_count FROM hotwords WHERE status='active'`。
4. **pending 词丢弃**（未确认的挖掘候选，废弃 pending 流后不带入）。
5. `hotwords` 表保留但停用（不再读写；避免破坏性 DROP，留待后续清理）。

> 现有 schema 已是 **v22**（asr-hotword plan 文档写的 v20 已过时，v21/v22 被后续功能占用），本次走 **v23**。

## 数据流

**生效词装载（启动 + 每次版本/词变更后 reload）**：
`SELECT words_text FROM hotword_sets WHERE enabled=1` → 逐行按任意空白切词 → 全局去重并集 → `HotwordIndex::from_words` → `corrector::reload_hotwords`。enabled 全关 → 空集 → 纠错 no-op。

**纠错命中（每次识别）**：沿用 v1 滑窗拼音匹配；命中且字符不同 → 替换 span → 对命中词 `hotword_hits +1`（`INSERT INTO hotword_hits(word,hit_count) VALUES(?,1) ON CONFLICT(word) DO UPDATE SET hit_count=hit_count+1`）。无命中 → 原样返回。

**写 words_text（单词增 / 卡片删 / 导入 / 挖掘）**：读当前 `words_text` → 切词集合 → {新增词 union / 删除词 difference / 覆盖} → `normalize_words_text`（去重 + 拼音首字母排序 + 空格拼接）→ UPDATE `words_text` + `updated_at` → 触发 corrector reload。

**导入**：选 txt → 读全文 → 弹窗选模式：① 新建版本（输入名）② 追加到已有版本 ③ 覆盖已有版本 → 按模式写 `words_text`（追加 = 并集，覆盖 = 替换，均走 normalize）→ reload。

**导出**：某版本 `words_text` → 写 txt 文件（词空格拼接，与导入对称）。

**挖掘（两步：候选 → 确认）**：`list_hotword_candidates()` 调 `collect_candidate_words()`（扫历史 + jieba + 词频过滤，逻辑同 v1）→ 返回候选词列表（**不写库**）→ 前端展示确认面板：默认全选、可逐个取消勾选、可手动补词 → 用户点「确认」才调 `add_words_to_set(id, words)` 批量追加（读当前 `words_text` → union 新词 → normalize → UPDATE → reload）→ toast「新增 N 词」并高亮本次新增。目标版本 = 当前选中版本（无弹窗选择）。挖掘不创建名为「挖掘」的版本（v1 pending 流已废弃）。

## UI（HotwordPanel 重写）

方言模糊 Card **保留不变**（f/h、hu/wu、n/l、r/l 四组 toggle + 说明）。其余 v1 的「添加热词 / 待确认 / 生效热词卡片网格」三块整体替换为：

**① 版本管理 Card**
- 版本列表，每行：`[☑ enabled toggle] 版本名（可重命名）· N词 [📤导出][🗑删除]`
- 顶部按钮：`+ 新建版本` / `📥 导入新版本`（均为 inline 输入名，WKWebView 不支持 window.prompt）

**② 选中某版本 → 卡片网格**（逐词管理体感）
- 每张卡：词名 + 右上角 `✕`（删该词）+ 命中数 inline（查 `hotword_hits[word]`，>0 高亮色、=0 淡色）
- 上方：单个添加输入框 + 「添加」（系统追加一词 → normalize）+ `追加`/`覆盖`（导入到当前版本）+ `⛏ 挖掘`（调 `list_hotword_candidates` 拉候选）
- **挖掘确认面板**（点「挖掘」后展开，已选版本为目标）：候选词 chip 默认全选（`border-voice` 实色），点 chip 切换勾选（取消 → 划线淡色）；「全选/全不选」切换 + 「取消」丢弃；底部「手动补词」输入框（Enter 加入候选并默认勾选）+ 「添加选中的 N 个」确认按钮（调 `add_words_to_set`，落库后高亮本次新增）
- 搜索 + 排序（复用 v1 的拼音首字母搜索 + 时间/字母/命中度排序）

> 用户操作 = 卡片 / 单词添加；底层每次都是系统规范化 `words_text`。批量需求走导出→改→导入覆盖。

## 与现有代码的关系

- **`infra/src/db.rs`**：新增 `hotword_sets` / `hotword_hits` 表 + CRUD；v1 的 `list_hotwords` / `insert_hotword` / `confirm_pending_hotword` / `delete_hotword` / `bump_hotword_hit` / `list_active_hotword_words` / `list_recent_text` 中——`list_active_hotword_words` 改为取 enabled 并集、`list_recent_text` 保留（挖掘用）、其余随 pending 流一并移除。
- **`asr-local/src/hotword.rs`**：新增 `normalize_words_text(words: &str) -> String`（切词→去重→`pinyin_initials` 排序→拼接）+ `pinyin_initials`（v1 已有）；`HotwordIndex` 零改。
- **`asr-local/src/corrector.rs`**：`correct()` 命中分支加「`hotword_hits +1`」；`reload_hotwords(Vec<String>)` 签名不变。`reload_fuzzy_dialect` 不变。
- **`asr-local/src/miner.rs`**：`mine_pending_candidates` → `collect_candidate_words`，返回 `Vec<String>`，不再写 DB。
- **`desktop/src/hotword_commands.rs`**：重写命令（`list_hotword_sets` / `create_hotword_set` / `rename_hotword_set` / `delete_hotword_set` / `toggle_hotword_set` / `add_word_to_set` / `remove_word_from_set` / `add_words_to_set`（批量） / `import_hotwords` / `export_hotwords` / `list_hotword_candidates`（挖掘候选不写库） / `list_hotword_hits`）。挖掘 = `list_hotword_candidates`（候选）+ `add_words_to_set`（确认后批量落库）两命令组合，无直接落库的 mine 命令。
- **`frontend/.../HotwordPanel.tsx`**：重写（见 UI 节）。
- **`config.rs`**：`fuzzy_dialect` 保留；无需 `active_hotword_sets`（enabled 在表里）。`asr_correct` 主开关语义不变。

## 错误处理与边界

- enabled 全关 → 生效词空集 → corrector no-op（零过纠铁证保留）。
- 空 `words_text` → 该版本无词；切词得空并集不影响其他版本。
- 重复词：normalize 去重；单词添加已存在的词 → toast「已存在」。
- 多版本含同词：并集去重，HotwordIndex 只一份；命中累加到 `hotword_hits` 同一行。
- 删除版本 → 仅删 `hotword_sets` 行；**不删 `hotword_hits`**（命中是全局历史统计，保留）。
- 删除/重命名「通用」「挖掘」无特殊保护——它们是普通版本，迁移产物无特殊地位。
- 导入覆盖 → UI 二次确认（防误操作丢失整版词）。
- 「通用」版本名唯一约束：迁移建一次；用户若已手建同名则迁移跳过建、改把词并入既有同名版本。
- 并发：`with_db` 已是 `parking_lot::ReentrantMutex`（v1 已修），写 `words_text` 走 `with_db`，安全。
- 命中写库失败：仅 `log::warn!`，不阻断纠错（纠错正确性不依赖 hit_count 落库）。

## 测试策略

- **normalize_words_text 单测**：① 任意空白（空格/换行/制表符）切词 ② 去重 ③ 按拼音首字母排序（`八爪鱼 浮窗 周会` 序）④ 空串→空串 ⑤ 含非汉字词保留。
- **生效词并集**：多 enabled 版本、含同词去重、enabled=0 版本不参与。
- **命中全局累加**：同词在多版本命中，`hotword_hits` 单行递增（不分裂）。
- **迁移**：构造 v22 库（active+pending 词 + hit_count）→ 升 v23 → 断言「通用」版本 `words_text` = active 词规范化、`hotword_hits` 迁入 active 词计数、pending 不在内、`hotwords` 表仍在但停用。
- **导入导出 round-trip**：导出 → （外部不变）→ 导入覆盖 → `words_text` 等价。
- **corrector 集成**：enabled 全关 no-op；勾选含目标热词的版本后命中纠错；命中后 `hotword_hits` 增长。
- **挖掘候选**：`collect_candidate_words` 返回滤常用词后的候选（v1 逻辑保留）；`list_hotword_candidates` 命令不写库，前端确认后 `add_words_to_set` 批量落库（新增条数 = 已存在外的词数）。
- **e2e（铁律，沿用 v1）**：真实录音含一误识专名 → 建版本加入该词 + enabled → 走 desktop pipeline 全链路 → 断言纠错命中 + `hotword_hits` 该词 +1。直调 engine 绕过 corrector 会掩盖效果，禁用。

## 分阶段交付

- **本 spec 范围（多版本管理）**：`hotword_sets` + `hotword_hits` + 迁移 + normalize + corrector 命中写库 + 命令重写 + UI 重写 + 导入导出 + 挖掘改造。
- **未来**：`hotwords` 表清理；跨设备同步；L1 云端原生热词透传（沿用 asr-hotword v2）。
