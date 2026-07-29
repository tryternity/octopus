# ASR 数字 ITN（Inverse Text Normalization）设计

> **日期**：2026-07-27
> **状态**：✅ 已实现（单数字保护 + 黑名单词边界 + chinese2digits crate；corrector 后 hans 前；流式仅 Final 过 ITN；11 测试全过；e2e 待用户验证）
> **来源**：[竞品分析报告](../../research/2026-07-27-competitive-analysis.md) §1 语音输入 P0 缺口

---

## 1. 问题

ASR 引擎输出中文数字而非阿拉伯数字：「二零二六年七月二十六日」而非「2026年7月26日」。

**受影响引擎**（无内置 ITN）：
- Zipformer CTC / Zipformer Transducer
- Moonshine
- Whisper

**不受影响引擎**（自带 ITN，输出已是阿拉伯数字）：
- Qwen3-ASR
- SenseVoice
- Paraformer（离线 / 流式）

## 2. 方案

用 [chinese2digits](https://github.com/Wall-ee/chinese2digits) crate 的 `take_number_from_string` 做底层转换，但加两层保护避免误转：

1. **单数字保护**：只转连续 2+ 中文数字字符的片段。单个数字字符（前后非数字）一律保留——杜绝「统一→统1」「一些→1些」「七月→7月」（「七月」是地道中文）。
2. **黑名单词边界匹配**：含 2+ 数字字符但不是数字的常用词/成语，在词边界（前后非数字字符）时不转。

### 后处理链（修改后）

```
engine.transcribe() → raw_text
  → corrector.correct()      # 拼音纠错（现有，asr_correct 开关）
  → itn::normalize(&text)    # 🆕 中文数字→阿拉伯（始终开）
  → hans::to_simplified()    # 简繁归一（现有，output_simplified 开关）
  → 返回
```

**位置**：corrector 后、hans 前。
- corrector 先跑：修正数字附近的错字（热词纠错），ITN 拿到的是纠错后文本。
- hans 后跑：ITN 的 `force_simplified=true` 只管数字字符的繁简识别（「貳」→识别为 2），不影响 hans 的全文简繁归一职责（「臺灣」→「台湾」）。两者职责不重叠。

### ITN 应用点（两个「最终结果」产出处）

1. **离线转录**：`pipeline::transcribe_batch`（`crates/asr-local/src/pipeline.rs`）
   - corrector 之后、hans 之前插入 `itn::normalize`

2. **流式转录 Final**：`streaming_runner::finish()`（`crates/asr-local/src/streaming_runner.rs`）
   - `engine.finish()` 返回 text 后，过 `itn::normalize` 再返回 `TranscriptEvent::Final`

**Partial / Committed 不过 ITN**：流式中间态数字未说完时可能误转（「二」→「2」但用户要说「二十」）。Final 是完整结果，安全。Final 过 ITN 后写入 transcript，后续润色（含强制润色 PolishNow）/粘贴自然拿到阿拉伯数字。

## 3. 组件

### `crates/asr-local/src/itn.rs`（实际实现）

**核心逻辑**（非简单调 take_number_from_string，而是分段处理）：

1. **黑名单保护**：先用占位符替换黑名单词（词边界匹配——前后非数字字符时才保护）。两类黑名单：
   - **固定搭配**（任何情况都不转）：三十六计、三百六十行、二十四史、七十二变、八十一难、九九归一、三七二十一、八九不离十、三五成群、略知一二
   - **独立数字**（单独不转，连数字时转）：万一、千万、百万、二百五
2. **分段扫描**：遍历文本，收集连续数字字符片段：
   - **2+ 连续数字字符** → 调 `take_number_from_string` 转换
   - **单个数字字符** → 保留原文（「七月」「统一」「十分」不转）
3. **还原黑名单**：占位符替换回原词

**关键设计**：词边界匹配使「二百五」独立时不转（`二百五` → 保留），但跟数字连用时转（`二百五十六` → 256）。

### 修改 `pipeline.rs` `transcribe_batch`

```rust
// 现有：corrector → hans
// 改为：corrector → ITN → hans
let text = if cfg.correct && !engine.skip_corrector() && !is_english {
    let corrected = crate::corrector::get_corrector().correct(&raw_text);
    // ... 现有 corrector 逻辑 ...
    corrected
} else {
    raw_text
};
let text = crate::itn::normalize(&text);  // 🆕 ITN
if cfg.simplify {
    crate::hans::to_simplified(&text)
} else {
    crate::hans::to_traditional(&text)
}
```

### 修改 `streaming_runner.rs` `finish()`

```rust
pub fn finish(&mut self) -> TranscriptEvent {
    match self.engine.finish() {
        Ok(text) => TranscriptEvent::Final(crate::itn::normalize(&text)),  // 🆕
        Err(e) => TranscriptEvent::Error(e.to_string()),
    }
}
```

### 修改 `Cargo.toml`

`crates/asr-local/Cargo.toml` 加依赖：
```toml
chinese2digits = "1"
```

## 4. 不变量

| # | 不变量 | 保证方式 |
|---|---|---|
| INV-1 | 自带 ITN 引擎（Qwen3/SenseVoice/Paraformer）无副作用 | 文本无中文数字 → no-op |
| INV-2 | 英文文本无副作用 | 无中文数字字符 → no-op |
| INV-3 | 不影响 hans 全文简繁归一 | ITN 的 force_simplified 只管数字字符；hans 在 ITN 后做全文归一 |
| INV-4 | Partial/Committed 不过 ITN | 仅 `finish()`（Final）和 `transcribe_batch`（离线）调 ITN |
| INV-5 | 始终应用，无配置开关 | ASR 语音输入场景数字归一化几乎总是想要 |
| INV-6 | 单个数字字符不转 | 只转连续 2+ 数字字符片段；「统一」「七月」「十分」保留 |
| INV-7 | 黑名单词边界保护 | 固定搭配（三十六计等）任何情况不转；独立数字（万一/百万/二百五）单独不转、连数字时转 |
| INV-8 | 黑名单 + 数字连用时转 | 「二百五十六」→256（「二百五」后面跟「十六」，词边界失效） |

## 5. 测试

`itn.rs` 内联测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_and_date() {
        assert_eq!(normalize("二零二六年七月二十六日"), "2026年7月26日");
    }

    #[test]
    fn decimal() {
        assert_eq!(normalize("三点五"), "3.5");
    }

    #[test]
    fn count() {
        assert_eq!(normalize("十五个"), "15个");
    }

    #[test]
    fn large_number() {
        assert_eq!(normalize("一百美元"), "100美元");
    }

    #[test]
    fn no_chinese_number_noop() {
        assert_eq!(normalize("今天天气不错"), "今天天气不错");
    }

    #[test]
    fn already_arabic_noop() {
        // Qwen3/SenseVoice 输出已是阿拉伯数字
        assert_eq!(normalize("2026年7月26日"), "2026年7月26日");
    }

    #[test]
    fn english_noop() {
        assert_eq!(normalize("hello world"), "hello world");
    }

    #[test]
    fn traditional_digits() {
        // force_simplified=true：繁体数字字符也能识别
        assert_eq!(normalize("貳零貳陸年"), "2026年");
    }
}
```

## 6. chinese2digits 的 force_simplified 与 hans 的关系

**关键区分**（避免混淆）：

| | chinese2digits `force_simplified` | octopus `hans::to_simplified` |
|---|---|---|
| 作用域 | 仅数字字符（貳→贰→2） | 全文本（臺灣→台湾） |
| 目的 | 让繁体数字字符能被识别为数字 | 整体输出简/繁体 |
| 范围 | 数字相关字符（壹貳參肆...） | 所有汉字 |

**两者不能互相替代**。hans 不省略。

## 7. 不做

- **英文 ITN**（"twenty twenty six" → "2026"）：英文数字表达复杂，留作后续。
- **百分比/货币/日期时间格式化**（"百分之五十"→"50%"）：`pct=false` 关闭。
- **配置开关**：ASR 场景始终需要，不加 `itn_enabled`。
- **单数字+量词转换**（「七月→7月」「三个→3个」）：单个数字字符一律保留——「七月」「三个」是地道中文，转了反而不自然。
- **黑名单穷尽**：黑名单只覆盖高频误转词，不可能穷尽所有中文固有表达。新误转词发现后逐步加。

## 8. 相关代码位置

| 位置 | 作用 |
|---|---|
| `crates/asr-local/src/itn.rs`（新建） | ITN 模块 |
| `crates/asr-local/src/pipeline.rs:58-75` | 离线后处理链（corrector → ITN → hans） |
| `crates/asr-local/src/streaming_runner.rs:279-284` | 流式 finish()（Final 过 ITN） |
| `crates/asr-local/Cargo.toml` | 加 chinese2digits 依赖 |
| `crates/asr-local/src/lib.rs` | pub mod itn |
