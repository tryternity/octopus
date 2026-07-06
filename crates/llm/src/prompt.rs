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
pub(crate) fn build_system_prompt(content: &str) -> String {
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
pub(crate) fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
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

/// 段模型多段润色 user prompt。
/// preserve=true 的段（edited）用【已确认部分】标记原样保留；其余段待润色。
/// LLM 输出整篇（edited 区 verbatim + 润色后的非 edited 区拼接），仅纯文本。
///
/// 无 preserve 段时走全量润色分支（与旧 user_prompt(None) 等价语义）。
/// CONFIRMED_MARKER 字面须与 INCREMENTAL_RULE 中的【已确认部分】一致（const 拼装）。
pub(crate) fn regions_prompt(regions: &[crate::PolishRegion]) -> String {
    if regions.iter().all(|r| !r.preserve) {
        // 无 edited 段 → 全量润色（与旧 user_prompt(None) 等价语义）
        let full: String = regions.iter().map(|r| r.text.as_str()).collect();
        return format!("请润色以下语音识别文本：\n{}", full);
    }
    let m = CONFIRMED_MARKER;
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("【{m}（原样保留）】{}\n", r.text));
        } else {
            body.push_str(&format!("【待润色】{}\n", r.text));
        }
    }
    format!(
        "以下文本中，【{m}】已经用户人工校对，必须逐字原样保留、严禁修改；仅对【待润色】区域润色。\n\n\
         {body}\n请输出：所有区域按原顺序拼接为完整文本（{m} 原样），仅输出纯文本。",
    )
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

    #[test]
    fn regions_prompt_no_preserve_is_plain() {
        let rs = vec![crate::PolishRegion { preserve: false, text: "你好".into() }];
        let p = regions_prompt(&rs);
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(!p.contains("原样保留"));
    }

    #[test]
    fn regions_prompt_marks_preserved_regions() {
        let rs = vec![
            crate::PolishRegion { preserve: true, text: "已确认".into() },
            crate::PolishRegion { preserve: false, text: "待润色".into() },
        ];
        let p = regions_prompt(&rs);
        assert!(p.contains("已确认部分"));
        assert!(p.contains("原样保留"));
        assert!(p.contains("待润色"));
    }
}
