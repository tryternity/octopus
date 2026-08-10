// crates/llm/src/prompt.rs

use std::sync::RwLock;

/// {} edited 标记规则（代码层拼接到 system prompt 末尾，用户不可见）。
/// 替代旧 INCREMENTAL_RULE：从「原样保留」改为「信任+遵循语境」。
/// 用 {花括号} 而非 [方括号]——避免与 few-shot 示例里的 [技术术语] 标记冲突。
const EDITED_MARKER_RULE: &str = "文本中 {花括号} 标记的词语是用户手动修正过的，请信任这些用词，并在润色全文时以其为语境参考。输出时去掉花括号标记，仅输出纯文本。";

/// <> hotwords 候选标记规则（热词多命中时，LLM 从候选选一个）。
const HOTWORDS_MARKER_RULE: &str = "文本中 <尖括号> 内用竖线分隔的多个词语是语音识别的候选词，请根据上下文选择最合适的一个，去掉尖括号和竖线，仅输出选中的词语。";

/// 当前激活的完整 system prompt（用户 prompt 部分 + EDITED_MARKER_RULE）。
/// 启动时由 main.rs 从 DB 加载并 set_system_prompt。
static SYSTEM_PROMPT: RwLock<String> = RwLock::new(String::new());

/// app 上下文（注入 user prompt 头部，仅 inject_context=1 的模板用）。
#[derive(Debug, Clone)]
pub struct AppContext {
    pub name: String,
    /// 空串=无类别（仅注入 app 名称）。
    pub category: String,
}

/// bundle_id → 类别映射（精简，覆盖典型场景，其余靠 LLM 从 app 名称推断）。
/// 大小写不敏感：真实 bundle id 常用 CamelCase（如 com.microsoft.Word）。
/// 类别名与 app-casual 模板 Role 段场景对齐：聊天通讯 / 编程开发 / 文档写作。
pub fn classify_app_context(bundle_id: &str) -> &'static str {
    let b = bundle_id.to_ascii_lowercase();
    let b = b.as_str();
    match b {
        // 即时通讯 / 聊天通讯
        b if b.starts_with("com.tencent.xinwechat")
            || b.starts_with("com.tencent.qq")
            || b.starts_with("com.tencent.wework")     // 企业微信
            || b.starts_with("com.tencent.dingtalk")   // 钉钉
            || b.starts_with("com.tinyspeck.slack")    // Slack (Mac)
            || b.starts_with("com.slack")              // Slack 备用前缀
            || b.starts_with("com.lark")               // 飞书 Lark（含 com.lark.electron / com.electron.lark）
            || b.starts_with("com.electron.lark")
            => "即时通讯",
        // 编程开发（IDE / 编辑器）
        b if b.starts_with("com.microsoft.vscode")
            || b.starts_with("com.apple.dt.xcode")     // Xcode
            || b.starts_with("com.jetbrains")          // IntelliJ/PyCharm/GoLand/CLion 等
            || b.starts_with("com.todesktop")          // Cursor 等 todesktop 打包的 Electron IDE
            || b.starts_with("com.sublimetext")
            || b.starts_with("com.neovide")            // neovim GUI
            => "编程开发",
        // 文档写作
        b if b.starts_with("com.microsoft.word")
            || b.starts_with("com.apple.textedit")
            || b.starts_with("com.apple.pages")
            || b.starts_with("com.microsoft.onenote.mac") // OneNote
            => "文档写作",
        _ => "",
    }
}

/// 构造 app 上下文前缀行。name 为空 → 无前缀；category 为空 → 仅 app 名。
fn app_context_prefix(app_context: Option<&AppContext>) -> String {
    match app_context {
        Some(ctx) if !ctx.name.is_empty() => {
            if ctx.category.is_empty() {
                format!("当前应用：{}\n", ctx.name)
            } else {
                format!("当前应用：{}（{}）\n", ctx.name, ctx.category)
            }
        }
        _ => String::new(),
    }
}

/// 拼接用户 prompt content + 强制 edited 标记规则。
/// content 为 DB prompts 表的 content 字段（纯风格规则，不含 edited 标记逻辑）。
pub fn build_system_prompt(content: &str) -> String {
    format!("{}\n{}\n{}", content.trim_end(), EDITED_MARKER_RULE, HOTWORDS_MARKER_RULE)
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
/// preserve=true 的段（edited）用 `{...}` 内联标记；
/// candidates=Some 的段（hotwords）用 `<a|b|c>` 标记（LLM 从列表选一个）；
/// 其余段原样拼接。LLM 输出整篇（含润色后的全文），仅纯文本，无标记符号。
pub(crate) fn regions_prompt(
    regions: &[crate::PolishRegion],
    app_context: Option<&AppContext>,
) -> String {
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("{{{}}}", r.text));
        } else if let Some(ref cands) = r.candidates {
            if cands.len() >= 2 {
                // 热词多候选：<候选1|候选2|候选3>（LLM 选一个）
                // 第二十九轮 P3-LLM1 契约闭合：单候选不包裹 <>（无需 LLM 选，原样 push
                // 文本）——避免 strip_edited_markers 正则（要求含 |）无法清单候选残留 <词>。
                body.push_str(&format!("<{}>", cands.join("|")));
            } else {
                // 单候选或空——原样 push（单候选不需 LLM 选择）
                body.push_str(cands.first().map(|s| s.as_str()).unwrap_or(""));
            }
        } else {
            body.push_str(&r.text);
        }
    }
    format!("{}请润色以下语音识别文本：\n{}", app_context_prefix(app_context), body)
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
        let rs = vec![crate::PolishRegion { preserve: false, text: "你好".into() , candidates: None }];
        let p = regions_prompt(&rs, None);
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(!p.contains("原样保留"));
        assert!(!p.contains('{'));  // 无 preserve → 无花括号
    }

    #[test]
    fn regions_prompt_marks_preserved_regions() {
        let rs = vec![
            crate::PolishRegion { preserve: true, text: "已确认".into() , candidates: None },
            crate::PolishRegion { preserve: false, text: "待润色".into() , candidates: None },
        ];
        let p = regions_prompt(&rs, None);
        assert!(p.contains("{已确认}"));
        assert!(p.contains("待润色"));
    }

    #[test]
    fn app_context_prefix_none_is_empty() {
        assert_eq!(super::app_context_prefix(None), "");
        let ctx = super::AppContext { name: String::new(), category: String::new() };
        assert_eq!(super::app_context_prefix(Some(&ctx)), "");
    }

    #[test]
    fn app_context_prefix_with_category() {
        let ctx = super::AppContext { name: "微信".into(), category: "即时通讯".into() };
        assert_eq!(super::app_context_prefix(Some(&ctx)), "当前应用：微信（即时通讯）\n");
    }

    #[test]
    fn app_context_prefix_without_category() {
        let ctx = super::AppContext { name: "Code".into(), category: String::new() };
        assert_eq!(super::app_context_prefix(Some(&ctx)), "当前应用：Code\n");
    }

    #[test]
    fn regions_prompt_injects_app_context() {
        let regions = vec![crate::PolishRegion { preserve: false, text: "你好".into() , candidates: None }];
        let ctx = super::AppContext { name: "微信".into(), category: "即时通讯".into() };
        let p = super::regions_prompt(&regions, Some(&ctx));
        assert!(p.starts_with("当前应用：微信（即时通讯）\n请润色以下语音识别文本：\n"));
        assert!(p.ends_with("你好"));
    }

    #[test]
    fn classify_app_context_known_and_unknown() {
        // 即时通讯
        assert_eq!(super::classify_app_context("com.tencent.xinWeChat"), "即时通讯");
        assert_eq!(super::classify_app_context("com.tencent.qq"), "即时通讯");
        assert_eq!(super::classify_app_context("com.tencent.DingTalk"), "即时通讯");
        assert_eq!(super::classify_app_context("com.tinyspeck.slack"), "即时通讯");
        assert_eq!(super::classify_app_context("com.lark.electron"), "即时通讯");
        // 编程开发
        assert_eq!(super::classify_app_context("com.microsoft.VSCode"), "编程开发");
        assert_eq!(super::classify_app_context("com.apple.dt.Xcode"), "编程开发");
        assert_eq!(super::classify_app_context("com.jetbrains.intellij"), "编程开发");
        assert_eq!(super::classify_app_context("com.todesktop.230313mzl4w4u92"), "编程开发"); // Cursor
        // 文档写作
        assert_eq!(super::classify_app_context("com.microsoft.Word"), "文档写作");
        assert_eq!(super::classify_app_context("com.apple.TextEdit"), "文档写作");
        assert_eq!(super::classify_app_context("com.microsoft.onenote.mac"), "文档写作");
        // 未知（靠 LLM 推断）
        assert_eq!(super::classify_app_context("com.apple.Safari"), "");
        assert_eq!(super::classify_app_context("com.bitwarden.desktop"), "");
    }
}
