# ASR 数字 ITN（Inverse Text Normalization）设计

> **日期**：2026-07-27
> **状态**：设计阶段（待实现）
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

用 [chinese2digits](https://github.com/Wall-ee/chinese2digits) crate 的 `take_number_from_string`，在 ASR 后处理链中插入 ITN 步骤。

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

### 新增 `crates/asr-local/src/itn.rs`

```rust
use chinese2digits::take_number_from_string;

/// ASR 数字 ITN：中文数字→阿拉伯数字。
///
/// 用 chinese2digits crate 的 take_number_from_string，从文本中找中文数字
/// 替换为阿拉伯数字，保留非数字文字。
///
/// 参数 force_simplified=true：先把繁体数字字符（貳貳參...）转简体再识别。
/// 注意：这只管数字字符的繁简，不替代 hans 模块的全文简繁归一。
pub fn normalize(text: &str) -> String {
    let result = take_number_from_string(text, false, true);
    result.replaced_text
}
```

- 纯函数，无状态
- 第二参数 `false`：不做百分比转换（「百分之五十」→「50%」非核心需求，且可能误转）
- 第三参数 `true`：force_simplified（繁体数字字符先转简体再识别）

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
| INV-1 | 自带 ITN 引擎（Qwen3/SenseVoice/Paraformer）无副作用 | 文本无中文数字 → `take_number_from_string` 找不到 → 返回原文（no-op） |
| INV-2 | 英文文本无副作用 | 无中文数字字符 → no-op |
| INV-3 | 不影响 hans 全文简繁归一 | ITN 的 force_simplified 只管数字字符；hans 在 ITN 后做全文归一 |
| INV-4 | Partial/Committed 不过 ITN | 仅 `finish()`（Final）和 `transcribe_batch`（离线）调 ITN；流式 partial 路径不调 |
| INV-5 | 始终应用，无配置开关 | ASR 语音输入场景数字归一化几乎总是想要 |

## 5. 测试（TDD）

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

- **英文 ITN**（"twenty twenty six" → "2026"）：英文数字表达复杂（序数词/小数/分数/货币），且 Moonshine/Whisper 英文场景非核心。留作后续。
- **百分比/货币/日期时间格式化**（"百分之五十"→"50%"）：chinese2digits 的第二参数 `pct=false` 关闭。非核心需求，可能误转。
- **配置开关**：ASR 场景始终需要数字归一化，不加 `itn_enabled` 配置项。

## 8. 相关代码位置

| 位置 | 作用 |
|---|---|
| `crates/asr-local/src/itn.rs`（新建） | ITN 模块 |
| `crates/asr-local/src/pipeline.rs:58-75` | 离线后处理链（corrector → ITN → hans） |
| `crates/asr-local/src/streaming_runner.rs:279-284` | 流式 finish()（Final 过 ITN） |
| `crates/asr-local/Cargo.toml` | 加 chinese2digits 依赖 |
| `crates/asr-local/src/lib.rs` | pub mod itn |
