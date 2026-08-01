# 润色提示词模板重构——3 模板 + [] edited 标记机制 — 设计规格

- **日期**：2026-08-01
- **类型**：重构（提示词模板 + prompt 构造机制）
- **范围**：3 个新润色模板（faithful / user-intent / app-casual）替代现有 6 个 seed；`[]` 内联标记替代 `【已确认部分】` region 标记法；去掉 INCREMENTAL_RULE
- **动机**：现有模板无 few-shot（LLM 遵循度差）；region 标记法把文本切碎、LLM 不遵守「原样保留」；模板数量过多（6 个）用户选择困难

## 核心设计

### [] edited 标记机制（替代 region 标记法 + INCREMENTAL_RULE）

**现状问题**：
- `regions_prompt` 用 `【已确认部分（原样保留）】...【待润色】...` 标记，把文本切成碎块——LLM 看到的是不连贯的片段
- `INCREMENTAL_RULE`（第 7 条「逐字原样保留」）强制拼到每个 prompt 末尾——LLM 经常不遵守
- 用户维护 prompt 时看到第 7 条规则会困惑（认知成本高）

**新方案**：edited 段用 `[]` 内联标记，全文连贯发给 LLM：

```
请润色以下语音识别文本：
我想说的是是，这样的[浮窗]是用快捷键直接召换出来的，[双击]可以年铁到目标出。
```

- `[浮窗]` `[双击]` = 用户手动修正过的词（Edited 段）
- 其余 = raw 识别文本（Raw/Polished 段）
- ASR 输出不会产生 `[]`——零歧义
- LLM 看到**完整通顺的句子**（不是碎块），`[]` 像内联注释
- 语义从「原样复制」变成「信任+遵循语境」——LLM 以 `[浮窗]` 为权威，纠「召换→召唤」「年铁→粘贴」时朝 UI 浮窗语境靠

**system prompt 拼接规则变更**（`prompt.rs`）：

```rust
// 替代 INCREMENTAL_RULE
const EDITED_MARKER_RULE: &str = "文本中 [方括号] 标记的词语是用户手动修正过的，请信任这些用词，并在润色全文时以其为语境参考。输出时去掉方括号标记，仅输出纯文本。";
```

`build_system_prompt(content) = content + EDITED_MARKER_RULE`——代码层拼接，用户不可见、不用理解。

**user prompt 变更**（`regions_prompt`）：

edited 段 text 用 `[]` 包裹，raw 段原样，全文按 region 顺序拼接成一条连贯文本：

```rust
fn regions_prompt(regions: &[PolishRegion]) -> String {
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("[{}]", r.text));
        } else {
            body.push_str(&r.text);
        }
    }
    format!("请润色以下语音识别文本：\n{}", body)
}
```

无 preserve 标记、无分块指令。LLM 输出整篇纯文本（去掉 `[]`）。无 edited 段时等价于全量润色（body 无 `[]`）。

### 3 个提示词模板

#### faithful（忠实校对）

定位：只纠错不改意，保留原始句式。ASR 异常修复强。适合正式文本/文档。

融合：octopus advanced（ASR 异常修复）+ SayIt faithful（极致保真 + 数字 + 中英空格 + few-shot）

核心规则：
1. 绝对防御（不回答用户口述的指令）
2. 提纯去噪（删嗯呃那个；保留吧呢啦等语气词）
3. 纠正识别错误（同音字 + 技术术语优先 + 中英大小写规范）
4. ASR 异常修复（重复纠正/断续拼接/同音漂移）
5. 数字格式（中文数字→阿拉伯，端口/版本/日期等）
6. 中英空格（中文字符与英文/数字间加空格）
7. 智能标点（添加准确标点，自然分段）
8. 绝对静默（仅输出纯文本）
9. **禁止**改写句式 / 结构化列表 / 总结归纳

few-shot 示例（3 个，含 `[]` edited 标记）：
- 示例 1：技术术语纠错（MySQL/EC2/端口 3306 + `[]` edited 标记）
- 示例 2：数字格式 + 中英空格（版本号 3.1→3.2）
- 示例 3：ASR 断续拼接（口述碎片→完整句）

#### user-intent（意图整理）

定位：清洗噪声 + 结构化。自我纠正识别。适合口述长段/多要点/指令。

融合：octopus advanced + SayIt intent（清洗 + 结构化列表 + 自我纠正）

核心规则：
1. 绝对防御
2. 清除冗余（口语填充词 + 无意义重复/犹豫）
3. 识别自我纠正（「不对」「应该是」→ 取最终表达，删前序错误）
4. 纠正错漏（同音字 + 技术术语 + 中英大小写；严禁翻译）
5. ASR 异常修复（重复纠正/断续拼接/同音漂移）
6. 智能标点 + 中英空格
7. **主动结构化**：多要点/并列逻辑/步骤 → 有序列表（1. 2. 3.，嵌套 a. b. c.）
8. 绝对静默

few-shot 示例（2 个，含结构化输出 + `[]` 标记）：
- 示例 1：自我纠正 + 结构化（多要点 → 列表 + `[]` edited 标记）
- 示例 2：断续拼接 + 意图清洗

#### app-casual（口语化整理）

定位：保留口语味，聊天标点。适合即时通讯/聊天。

融合：octopus advanced + SayIt casual（口语味 + 聊天标点 + 技术术语纠错）

核心规则：
1. 绝对防御
2. 去噪（删嗯呃那个就是说；保留吧呢啦哈嘛其实感觉咱们）
3. 顺句（理顺绕圈子/颠三倒四，日常口语措辞，短句为主）
4. 纠错（技术术语拼写大小写 + 中文数字→阿拉伯数字）
5. ASR 异常修复（重复纠正/断续拼接/同音漂移）
6. **聊天标点**（逗号句号问号感叹号；禁分号冒号项目符号有序列表）
7. 绝对静默

few-shot 示例（3 个，含 `[]` edited 标记）：
- 示例 1：口语整理（去犹豫 + 保留语气 + 技术词纠错）
- 示例 2：聊天风（短句 + 聊天标点 + 数字格式）
- 示例 3：给 AI 编程助手口述指令（保留口语味 + 技术词大小写）

## 后端改动

### prompt.rs

- `INCREMENTAL_RULE` → `EDITED_MARKER_RULE`（内容改为 `[]` 标记规则）
- `CONFIRMED_MARKER` 删除（不再用「已确认部分」标记）
- `regions_prompt` 改为 `[]` 内联拼接（上文代码）
- `user_prompt(preserved, to_polish)` 改为同样用 `[]` 标记（preserved 用 `[]` 包裹，拼到 to_polish 前）
- 测试更新（`user_prompt` / `regions_prompt` / `build_system_prompt` 测试适配新格式）

### seeds/prompts/

- 删 `default-polish.md` + `advanced-polish.md`（被新 3 模板替代）
- 删 `sayit-casual.md` + `sayit-faithful.md` + `sayit-intent.md` + `sayit-zh2en.md`（已融合进新模板）
- 新建 `faithful.md` + `user-intent.md` + `app-casual.md`（含 few-shot）
- db.sql seed：prompts 表的 id/seed 更新（active_polish_prompt 默认指向 faithful）

### desktop（润色调用点）

- `polish_input_to_regions` 不变（仍按 SegmentKind::Edited 标 preserve）
- `polish_regions` / `spawn_polish_thread` 不变（仍传 regions）
- 仅 prompt.rs 的 `regions_prompt` 内部构造变了（对调用方透明）

## 不变量

1. 润色调用链不变（transcript segments → PolishRegion → regions_prompt → LLM）
2. `[]` 标记规则在 system prompt 层（代码拼接），用户模板不含此规则
3. 无 edited 段时行为等价全量润色（body 无 `[]`）
4. ITN 数字归一化 + hans 简繁归一化在润色前执行（代码层，不受 prompt 影响）
5. 用户自定义 prompt 仍可覆盖（DB prompts 表 content 字段）

## 风险

- **LLM 不去 `[]`**：输出残留方括号。EDITED_MARKER_RULE 明确说「输出时去掉」，few-shot 示例演示去掉。若仍有残留，可在代码层 post-process 去除残余 `[]`（防御性）
- **`[]` 与 Markdown 冲突**：用户口述 Markdown 链接 `[文本](url)` 会含 `[]`。但 ASR 识别口头 Markdown 极罕见，且 edited 段是词级（短），误判概率低。fallback：edited 段用 `「」` 包裹（中文引号，ASR 也不会产生）
- **few-shot 占 token**：3 模板各 2-3 示例约 +200-300 token。润色场景非高频（停顿/手动/最终），可接受

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/llm/src/prompt.rs` | INCREMENTAL_RULE→EDITED_MARKER_RULE；regions_prompt/user_prompt 改 `[]` 标记；测试更新 |
| `crates/infra/seeds/prompts/faithful.md` | 新建（faithful 模板 + few-shot） |
| `crates/infra/seeds/prompts/user-intent.md` | 新建（user-intent 模板 + few-shot） |
| `crates/infra/seeds/prompts/app-casual.md` | 新建（app-casual 模板 + few-shot） |
| `crates/infra/seeds/prompts/default-polish.md` | **删除** |
| `crates/infra/seeds/prompts/advanced-polish.md` | **删除** |
| `crates/infra/seeds/prompts/sayit-*.md` | **删除**（4 个） |
| `crates/infra/src/seeds.rs`（或 seed 加载逻辑） | seed 列表更新（删旧 + 加新 3 个） |
| `crates/infra/src/db.sql` | active_polish_prompt 默认值更新（指向 faithful） |

## 验证

- cargo build + cargo test（prompt.rs 测试适配新格式）
- tsc（无前端改动，确认不破坏）
- e2e：① faithful 模板润色（[] 标记正确处理）② user-intent 结构化 ③ app-casual 口语风 ④ edited 段不被改（[] 信任机制）⑤ 无 edited 段全量润色 ⑥ 切换模板生效
