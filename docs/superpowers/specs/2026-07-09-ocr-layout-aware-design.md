# OCR 布局感知：段落识别 + 标点分段

> **设计日期**：2026-07-09
> **来源**：[`2026-07-09-action-bar-related-tools-survey.md`](./2026-07-09-action-bar-related-tools-survey.md) §2.5（eSearch 借鉴点 #5）
> **范围**：截图 / 图片 OCR 输出从扁平文本升级为结构化 Markdown（段落 + 列表 + 标题），提升「截图→AI」动作的输入质量。
> **依赖**：无新增依赖；纯后处理，基于已有 `OcrBlock` 几何信息。

---

## 1. 目标与动机

### 问题

当前 OCR 输出 `lines.join("\n")` 的扁平字符串：每个 det 文本框（merge_same_line_blocks 后）= 一行，行间硬换行。后果：

1. **段落被打碎**——一个自然段被自动换行切成多行碎片，AI 看到的是 `"今天天气\n很好我们\n去公园"` 而非连续文本。
2. **无结构信息**——标题、列表、段落混为平铺文本，AI 无法区分文档层次。
3. **CompactEditor 可读性差**——用户看到一堆短行，需要手动整理才能用。

### 目标

OCR 输出 Markdown 纯文本，保留原文档的段落 / 列表 / 标题结构，段内 reflow 成连续文本。输出直接存入 `clipboard_history.content`，CompactEditor 和 AI 动作零改动受益。

### 非目标

- ❌ 竖排文本识别（涉及模型能力边界，留远期）
- ❌ 表格结构化提取（复杂度高，本次不做）
- ❌ 引用块 / 代码块检测（误判风险高，性价比低）
- ❌ 多栏布局识别（merge_same_line_blocks 已将同 y 不同列合并为一行，多栏需改动更深）
- ❌ 标点补全 / 修复（rec 模型标点能力可靠，硬补引入错误）

---

## 2. 设计决策（brainstorming 确认）

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 范围 | 段落识别 + 标点分段 | 竖排留远期 |
| 输出形式 | Markdown 纯文本 | 对 AI 最友好，零 schema 改动 |
| Markdown 元素 | 段落 `\n\n` + 列表 `- `/`1. ` + 标题 `#`/`##` | det 几何信息天然支持，边际成本低 |
| 段内折行 | ~~reflow 成一整行~~ → 改用 code fence 保留分行 | 用户反馈：reflow 破坏对齐排版/地址等场景；多行正文用 ` ``` ` 包裹保留原始分行 |
| 算法位置 | ocr crate 内，`run_ocr` 之后 | 保留几何信息，一处改动惠及全部入口 |
| 标点分段 | 零额外工作 | rec 原生标点 + 段落 reflow 自然实现 |

---

## 3. 数据流

### 现状

```
paddle-ocr (det→cls→rec)
  → OcrOutput { boxes, txts, scores }
  → ocr_output_to_blocks() → Vec<OcrBlock>（每框一行，带 x/y/w/h/score）
  → merge_same_line_blocks()（同行框合并 + 间隙补空格）
  → segment_english_words()（PP-OCRv5 英文分词，v6 跳过）
  → blocks.iter().map(|b| b.text).join("\n")  ← 扁平字符串
```

### 改后

```
paddle-ocr (det→cls→rec)
  → ocr_output_to_blocks() → Vec<OcrBlock>
  → merge_same_line_blocks()
  → segment_english_words()
  → to_markdown(blocks) → String  ← Markdown 结构化文本（新增）
```

`to_markdown` 替换原来的 `join("\n")`，消费 `OcrBlock` 的几何信息（h=字号代理、y 间距=段落边界、x=缩进、text=列表标记）输出结构化 Markdown。

### 接口变更

`recognize` 和 `recognize_with_blocks` 的签名不变（都返回 `String` / `(String, Vec<OcrBlock>)`），但返回的 String 语义从「扁平文本」变为「Markdown」：

```rust
// 改前：内部走 recognize_image / recognize_long_image → Vec<String> → join("\n")
// 改后：统一走 blocks → to_markdown(blocks)
pub fn recognize(&self, image_bytes: &[u8]) -> Result<String>  // 现在返回 Markdown
pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)>  // text 是 Markdown，blocks 仍是原始 det 框（前端叠加用）
```

`recognize_image` 和 `recognize_long_image`（返回 `Vec<String>`）被移除——逻辑统一到 blocks 路径。

**前端预览叠加不受影响**：`OcrResult.blocks` 仍是原始 det 文本框（未经 markdown 转换），前端 ImagePreview 组件按 block 几何位置叠加显示。只有 `OcrResult.text`（存 DB + CompactEditor 编辑 + AI 输入）变为 Markdown。

---

## 4. 算法设计

新增模块 `crates/ocr/src/layout.rs`，导出 `pub fn to_markdown(blocks: &[OcrBlock]) -> String`。

### 4.0 前置条件

blocks 已经过 `merge_same_line_blocks`（同行合并）+ `segment_english_words`（英文分词），按 y 中心升序排列（从上到下），同一 y 内按 x 升序（从左到右）。每个 block = 一个视觉文本行。

**块数不足跳过**：`blocks.len() < MIN_BLOCKS_FOR_LAYOUT`（3）时，不分析布局，直接 `\n\n` 连接所有 block 文本。太少行无法可靠统计基线。

### 4.1 全局基线计算

```
heights = blocks.map(|b| b.h).collect()
median_h = median(heights)   // 正文中位数高度 = body 基线
```

median_h 是后续所有比例判断的基准。用 median 而非 mean——少数大标题不拉高 median，但会拉高 mean。

### 4.2 逐行分类

每个 block 分为三类之一：

#### Title（标题）

```
ratio = block.h / median_h
if ratio >= TITLE_H1_RATIO (1.6) → H1（# 前缀）
elif ratio >= TITLE_H2_RATIO (1.3) → H2（## 前缀）
```

依据：det 框紧贴文字笔画，框高 ≈ 字号。标题字号显著大于正文，ratio 可靠。两档（1.3/1.6）覆盖 H2/H1，更深层次（H3+）在截图 OCR 中罕见。

#### ListItem（列表项）

文本前缀匹配列表标记模式：

| 类别 | 匹配模式 | 输出 |
|------|----------|------|
| 无序 | `•` `·` `○` `●` `■` `□` `◆` `◇` `►` `▶` `－` `-` `—` `＊` `*` + 空格 | `- text`（去原标记） |
| 有序 | `①`-`⑳` / `⑴`-`⑵⓪` / `⒈`-`⒛` | `1. text`（顺序编号） |
| 有序 | `\d+[.、)]` + 空格? / `[（(]\d+[)）]` / 全宽 `．` `）` | `1. text`（顺序编号） |

有序列表统一重新编号为 `1. 2. 3.`（不保留 OCR 原始数字——OCR 可能误读数字）。

#### Body（正文）

不满足 Title 和 ListItem 条件的 block。

### 4.3 段落聚类（仅 Body 行）

对**连续的 Body 行**分组为段落。Title 和 ListItem 天然打断段落（它们前后都是段落边界）。

分组规则——对相邻 Body 行 `prev` → `curr`：

```
gap_y = curr.y - (prev.y + prev.h)    // 两行之间的垂直间隙
if gap_y > median_h * PARAGRAPH_GAP_RATIO (0.8) → 新段落
```

依据：同段落行间距通常 < 0.5 × 行高；段落间距通常 > 0.8 × 行高。0.8 是分界点，实测后可调。

**不使用 x 对齐做段落切分**——缩进段落、引用缩进等场景 x 变化但不代表新段落。v1 仅靠垂直间隙。

### 4.4 多行正文 Code Fence 包裹（用户反馈后修订）

> 原设计为 reflow 成一行连续文本。用户反馈后改为保留原始分行。

同一段落内的 Body 行**不合并**，用 markdown code fence 包裹保留原始分行：

```
```
第一行原文
第二行原文
第三行原文
```
```

**单行段落**（段内仅 1 行）直接输出，不包裹。

**Code fence 嵌套**：内容含 ` ``` ` 时围栏加长为 ` ```` ` 避免 markdown 嵌套冲突。

**理由**：reflow 会破坏对齐排版、地址、表格残留等场景。保留原始分行对 LLM 已足够友好（code fence 内的文本语义清晰），且避免合并风险。

### 4.5 Markdown 拼装

按 block 顺序遍历，维护一个输出缓冲。各类型 block 的输出规则：

| 类型 | 输出 | 与前一块的间隔 |
|------|------|---------------|
| H1 | `# {text}\n` | `\n`（空行） |
| H2 | `## {text}\n` | `\n`（空行） |
| ListItem | `- {text}` 或 `{n}. {text}` | 前一个同列表项 → `\n`（单换行）；否则 `\n\n`（空行） |
| Body 段落 | 多行 → ` ```\n{text}\n...``` `；单行 → `{text}` | `\n\n`（空行） |

**连续列表项**：同一列表块内（连续的 ListItem，中间无 Title/Body 打断）用单 `\n` 分隔（markdown 列表语法要求）。列表块结束后空行分隔后续内容。**列表间距切分**：连续列表项间距 > median_h × 0.8 时视为不同列表，`\n\n` 分隔 + 有序列表重编号。

输出示例：

````markdown
# 会议纪要

```
今天讨论了三个议题，分别是产品规划和技术方案。
```

## 议题一

- 完成季度目标定义
- 确定下季度优先级
- 分配团队任务

## 议题二

```
后端服务需要迁移到新架构
预计两周完成
```
````

### 4.6 常量

```rust
const MIN_BLOCKS_FOR_LAYOUT: usize = 3;  // 少于此数不分析布局
const TITLE_H2_RATIO: f64 = 1.3;         // h ≥ median_h × 1.3 → H2
const TITLE_H1_RATIO: f64 = 1.6;         // h ≥ median_h × 1.6 → H1
const PARAGRAPH_GAP_RATIO: f64 = 0.8;    // gap > median_h × 0.8 → 新段落
```

均为起始值，可基于实测调整。

---

## 5. Code Fence 边界情况

| 场景 | 处理 |
|------|------|
| 内容含 ``` | 围栏加长为 `````` 避免嵌套冲突 |
| 英文单词跨行断裂（`Wor`+`ld`） | 保留原始分行，不修复（code fence 保留原样） |
| 纯英文段落 | 保留原始分行，code fence 包裹 |
| 数字 / 符号行 | 保留原始分行 |
| 空文本 block | 跳过（text.trim().is_empty() 的 block 不参与输出） |

---

## 6. 对现有功能的影响

### 6.1 CompactEditor

CompactEditor 是 `contentEditable` div，直接吃 Markdown 纯文本。`\n\n` 自然显示为段落分隔，`#`/`-` 前缀作为文本显示（不渲染为 HTML 标题/列表——这是预期行为，CompactEditor 是纯文本编辑器）。

用户编辑 Markdown 后 Ctrl+S 经 `set_clipboard_item_text` 回写 DB，与现有流程一致。

### 6.2 前端 ImagePreview 叠加

不受影响。`OcrResult.blocks` 仍是原始 det 框（未经 to_markdown 处理），前端按 block 几何位置叠加文字。

### 6.3 DB 存储

零改动。`clipboard_history.content` 字段存 Markdown 字符串，`meta_info` 仍只含 engine/model/char_count（char_count 是 Markdown 字符数，含 `#`/`-` 前缀）。

### 6.4 CLI / Server

如果 CLI 或 Server 调用 `recognize`（非 with_blocks 版本），输出也从扁平文本变为 Markdown。需在实现阶段确认所有 `recognize` 调用点。

### 6.5 `recognize_image` / `recognize_long_image` 移除

这两个内部方法返回 `Vec<String>`，统一后不再需要。确认无外部调用后移除。

---

## 7. 测试策略

所有测试为 `crates/ocr/src/layout.rs` 内联 `#[cfg(test)] mod tests`，使用合成 `OcrBlock` 输入（无需真实模型）。

### 7.1 单元测试用例

| 用例 | 输入 | 预期输出 |
|------|------|----------|
| 块数不足（< 3） | 2 个等高 block | `\n\n` 连接，无标题检测 |
| 单段落 reflow（CJK） | 3 行同高、同 x、小间距 | 一行连续 CJK 文本 |
| 单段落 reflow（中英混排） | 行末 ASCII + 行首 CJK | 正确补空格 |
| 两段落 | 3 行 + 大间距 + 2 行 | 两个 `\n\n` 分隔的段落 |
| H1 标题检测 | 1 行高 1.8x + 3 行高 1.0x | `# title` + 正文 |
| H2 标题检测 | 1 行高 1.4x + 3 行高 1.0x | `## title` + 正文 |
| 无标题（等高） | 全部等高 | 纯段落，无 `#` |
| 无序列表（`•` 标记） | 3 行 `• xxx` | 3 行 `- xxx` |
| 有序列表（`①②③`） | 3 行 `①xxx` | 3 行 `1. 2. 3.` |
| 列表 + 段落混合 | 段落 + 列表 + 段落 | 三块 `\n\n` 分隔，列表内单 `\n` |
| 空文本 block | 中间混入空 text block | 跳过空 block |

### 7.2 集成验证

实现完成后，用真实截图跑 `recognize_with_blocks`，人工检查 Markdown 输出质量：
- 截文档段落 → 段落分隔正确
- 截 PPT → 标题 + 列表识别
- 截纯代码 → 纯文本（代码不在本次结构化范围，但 reflow 不应严重破坏）

---

## 8. 文件变更

| 文件 | 变更 |
|------|------|
| `crates/ocr/src/layout.rs` | **新增**：`to_markdown` + 分类 + 聚类 + reflow + tests |
| `crates/ocr/src/engine.rs` | `recognize` / `recognize_with_blocks` 统一走 blocks → `to_markdown`；移除 `recognize_image` / `recognize_long_image` |
| `crates/ocr/src/lib.rs` | `pub mod layout`（如需） |
| `docs/features/ocr.md` | 更新 §4 后处理：新增 Markdown 布局感知 |

---

## 9. 远期演进

| 方向 | 触发条件 |
|------|----------|
| 竖排文本识别 | 有真实竖排场景需求（CJK 传统排版 / 日文书） |
| 表格结构化 | det 框已含表格几何，可加表格检测 → markdown table |
| 多栏布局 | 需改 merge_same_line_blocks 的同行判定逻辑 |
| 代码块检测 | 等宽字体检测 + 缩进连续行 → markdown code fence |
| Markdown 可配置 | 如用户需要关闭 Markdown 输出回归扁平文本 |
