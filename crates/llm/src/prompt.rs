// crates/llm/src/prompt.rs

use std::sync::RwLock;

/// 已确认部分的边界标记。
/// ★ 此标记须与 INCREMENTAL_RULE 中的【已确认部分】保持字面一致——
/// 通过 const 拼装避免双端失配。
const CONFIRMED_MARKER: &str = "已确认部分";

/// 增量保留规则（代码常量，强制拼接到用户 prompt 末尾）。
/// 来自原 DEFAULT_SYSTEM_PROMPT 第 7 条，用户不可见、不可改。
const INCREMENTAL_RULE: &str = "7. [增量保留]：若用户提供【已确认部分】，该部分必须逐字原样保留、严禁修改，仅润色【新增部分】，最终输出两者拼接。";

/// 当前激活的完整 system prompt（用户 prompt 部分 + INCREMENTAL_RULE）。
/// 启动时由 main.rs 从 DB 加载并 set_system_prompt。
static SYSTEM_PROMPT: RwLock<String> = RwLock::new(String::new());

/// 拼接用户 prompt content + 强制增量规则。
/// content 为 DB prompts 表的 content 字段（纯风格规则，不含增量逻辑）。
pub fn build_system_prompt(content: &str) -> String {
    format!("{}\n{}", content.trim_end(), INCREMENTAL_RULE)
}

/// 设置当前 system prompt（content 为用户 prompt 部分，内部自动拼接增量规则）。
/// 启动时调一次（从 DB 加载）；运行时切换 prompt 时再调。
pub fn set_system_prompt(content: &str) {
    let built = build_system_prompt(content);
    *SYSTEM_PROMPT.write().unwrap() = built;
}

/// 获取当前 system prompt（已含增量规则）。
/// 返回 clone 的 String（内部 RwLock<String>，非 &'static str）。
/// 未 set 时返回空串（正常流程 main.rs 启动时必 set，空串 = 降级，调用方应保证已 set）。
pub fn system_prompt() -> String {
    SYSTEM_PROMPT.read().unwrap().clone()
}

/// 构建 user prompt。
/// - preserved=None：全量润色（to_polish = 完整文本）。
/// - preserved=Some：编辑后增量润色，告知 LLM 已确认部分原样保留、仅润色 to_polish。
///
/// 分块文案中的「【{CONFIRMED_MARKER}...】」标记须与 INCREMENTAL_RULE
/// 中的【已确认部分】保持字面一致——通过 const 拼装避免双端失配。
pub fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
    let m = CONFIRMED_MARKER;
    match preserved {
        None => format!("请润色以下语音识别文本：\n{}", to_polish),
        Some(confirmed) => format!(
            "以下文本中，【{m}】已经用户人工校对，必须原样保留、严禁修改；仅对【新增部分】进行润色。\n\n\
             【{m}（原样保留）】\n{}\n\n【新增部分（请润色）】\n{}\n\n\
             请输出：{m} + 润色后的新增部分，拼接为完整文本，仅输出纯文本。",
            confirmed, to_polish
        ),
    }
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
        assert!(p.contains("已确认部分"));
        assert!(p.contains("原样保留"));
        assert!(p.contains("已确认文本"));
        assert!(p.contains("新增部分"));
        assert!(p.contains("新增文本"));
    }

    #[test]
    fn build_system_prompt_appends_incremental_rule() {
        let content = "# Role\n你是润色助手。";
        let built = build_system_prompt(content);
        assert!(built.starts_with("# Role\n你是润色助手。"));
        assert!(built.contains("增量保留"));
        assert!(built.contains(CONFIRMED_MARKER));
    }

    #[test]
    fn set_and_get_system_prompt_round_trip() {
        // 测试前先清空（避免受其他测试影响）
        *SYSTEM_PROMPT.write().unwrap() = String::new();
        assert!(system_prompt().is_empty());
        set_system_prompt("# 风格A");
        let got = system_prompt();
        assert!(got.contains("# 风格A"));
        assert!(got.contains("增量保留"));
        // 清理
        *SYSTEM_PROMPT.write().unwrap() = String::new();
    }
}
