// crates/llm/src/prompt.rs

use std::sync::RwLock;

/// {} edited 标记规则（代码层拼接到 system prompt 末尾，用户不可见）。
/// 替代旧 INCREMENTAL_RULE：从「原样保留」改为「信任+遵循语境」。
/// 用 {花括号} 而非 [方括号]——避免与 few-shot 示例里的 [技术术语] 标记冲突。
const EDITED_MARKER_RULE: &str = "文本中 {花括号} 标记的词语是用户手动修正过的，请信任这些用词，并在润色全文时以其为语境参考。输出时去掉花括号标记，仅输出纯文本。";

/// 当前激活的完整 system prompt（用户 prompt 部分 + EDITED_MARKER_RULE）。
/// 启动时由 main.rs 从 DB 加载并 set_system_prompt。
static SYSTEM_PROMPT: RwLock<String> = RwLock::new(String::new());

/// 拼接用户 prompt content + 强制 edited 标记规则。
/// content 为 DB prompts 表的 content 字段（纯风格规则，不含 edited 标记逻辑）。
pub(crate) fn build_system_prompt(content: &str) -> String {
    format!("{}\n{}", content.trim_end(), EDITED_MARKER_RULE)
}

/// 设置当前 system prompt（content 为用户 prompt 部分，内部自动拼接 edited 标记规则）。
/// 启动时调一次（从 DB 加载）；运行时切换 prompt 时再调。
pub fn set_system_prompt(content: &str) {
    let built = build_system_prompt(content);
    *SYSTEM_PROMPT.write().unwrap() = built;
}

/// 获取当前 system prompt（已含 edited 标记规则）。
/// 返回 clone 的 String（内部 RwLock<String>，非 &'static str）。
/// 未 set 时返回空串（正常流程 main.rs 启动时必 set，空串 = 降级，调用方应保证已 set）。
pub fn system_prompt() -> String {
    SYSTEM_PROMPT.read().unwrap().clone()
}

/// 构建 user prompt。
/// - preserved=None：全量润色（to_polish = 完整文本）。
/// - preserved=Some：编辑后增量润色，edited 部分用 `[]` 包裹拼到 raw 前。
///
/// edited 部分用 `{}` 包裹（`{}` 在 ASR 输出中不会出现，零歧义），
/// LLM 信任这些用词作为语境参考（见 EDITED_MARKER_RULE）。
pub(crate) fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
    match preserved {
        None => format!("请润色以下语音识别文本：\n{}", to_polish),
        Some(confirmed) => format!(
            "请润色以下语音识别文本：\n{{{}}}{}",
            confirmed, to_polish
        ),
    }
}

/// 段模型多段润色 user prompt。
/// preserve=true 的段（edited）用 `{...}` 内联标记；其余段原样拼接。
/// LLM 输出整篇（含润色后的全文），仅纯文本，无 `{}`。
///
/// 无 preserve 段时 body 无 `{}`，等价全量润色（与 user_prompt(None) 等价语义）。
pub(crate) fn regions_prompt(regions: &[crate::PolishRegion]) -> String {
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("{{{}}}", r.text));
        } else {
            body.push_str(&r.text);
        }
    }
    format!("请润色以下语音识别文本：\n{}", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_without_preserved_is_plain() {
        let p = user_prompt(None, "你好");
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(p.contains("你好"));
        assert!(!p.contains("已确认部分"));
    }

    #[test]
    fn user_prompt_with_preserved_marks_boundary() {
        let p = user_prompt(Some("已确认文本"), "新增文本");
        assert!(p.contains("{已确认文本}"));
        assert!(p.contains("新增文本"));
    }

    #[test]
    fn build_system_prompt_appends_edited_marker_rule() {
        let content = "# Role\n你是润色助手。";
        let built = build_system_prompt(content);
        assert!(built.starts_with("# Role\n你是润色助手。"));
        assert!(built.contains("花括号"));
        assert!(built.contains("信任"));
    }

    #[test]
    fn set_and_get_system_prompt_round_trip() {
        // 测试前先清空（避免受其他测试影响）
        *SYSTEM_PROMPT.write().unwrap() = String::new();
        assert!(system_prompt().is_empty());
        set_system_prompt("# 风格A");
        let got = system_prompt();
        assert!(got.contains("# 风格A"));
        assert!(got.contains("花括号"));
        // 清理
        *SYSTEM_PROMPT.write().unwrap() = String::new();
    }

    #[test]
    fn regions_prompt_no_preserve_is_plain() {
        let rs = vec![crate::PolishRegion { preserve: false, text: "你好".into() }];
        let p = regions_prompt(&rs);
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(!p.contains("原样保留"));
        assert!(!p.contains('{'));  // 无 preserve → 无花括号
    }

    #[test]
    fn regions_prompt_marks_preserved_regions() {
        let rs = vec![
            crate::PolishRegion { preserve: true, text: "已确认".into() },
            crate::PolishRegion { preserve: false, text: "待润色".into() },
        ];
        let p = regions_prompt(&rs);
        assert!(p.contains("{已确认}"));
        assert!(p.contains("待润色"));
    }
}
