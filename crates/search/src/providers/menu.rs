//! 菜单 + Slash 命令搜索 Provider。一次 DB 读，产出 menu/slash 两类 source。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::matcher::match_score;
use crate::provider::{SearchContext, SearchProvider};

pub struct MenuProvider;

#[async_trait]
impl SearchProvider for MenuProvider {
    fn id(&self) -> &'static str {
        "menu"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "quick" | "actions" | "slash")
    }

    async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let rows = match octopus_infra::db::list_action_bar_items() {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut results = search_menus(query, &rows);
        results.extend(search_slash_commands(query, &rows));
        results
    }
}

fn search_menus(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
    let mut scored: Vec<(i32, SearchResult)> = rows
        .iter()
        .filter(|r| r.is_enabled && r.action_type != "submenu")
        .filter_map(|row| {
            let score = match_score(query, &row.title)?;
            let action_data = serde_json::json!({
                "action_type": row.action_type,
                "action_data": row.action_data,
                "id": row.id,
            });
            Some((score, SearchResult {
                source: if row.action_type == "url" { "quicklink" } else { "menu" }.into(),
                title: row.title.clone(),
                subtitle: row.action_type.clone(),
                icon: None,
                action_type: if row.action_type == "url" { "url" } else { "menu" }.into(),
                action_data: action_data.to_string(),
                score: 0,
            }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(5)
        .map(|(s, mut r)| {
            r.score = s;
            r
        })
        .collect()
}

/// Slash 命令匹配：query 以 `/cmd [params]` 模式开头时，
/// fuzzy 匹配 trigger_keyword 非空的菜单项（所有 action_type），
/// 返回 source="slash" 候选。params 记入 action_data 供执行时用。
///
/// 仅 "/" → 返回所有配了 trigger_keyword 的命令（固定基础分，保持 DB 行序）。
/// query 不以 / 开头 → 空结果（不影响普通搜索）。
///
/// 计分（I1）：slash 结果用高基础分（15_000，参考旧 quicklink）确保用户手动切到
/// all tab 时命令不因 score=0 沉底；cmd 匹配时叠加 fuzzy match_score（精确 > fuzzy）。
fn search_slash_commands(
    query: &str,
    rows: &[octopus_infra::db::ActionBarItem],
) -> Vec<SearchResult> {
    let rest = match query.strip_prefix('/') {
        Some(r) => r,
        None => return vec![],
    };
    // 仅 "/" → 返回全部命令（用户主动列命令，统一高基础分置顶）
    if rest.is_empty() {
        return rows
            .iter()
            .filter(|r| r.is_enabled && !r.trigger_keyword.is_empty())
            .map(slash_result)
            .collect();
    }
    // 切 cmd（/ 后到空格前）+ params（空格后）
    let (cmd, params) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let mut scored: Vec<(i32, SearchResult)> = rows
        .iter()
        .filter(|r| r.is_enabled && !r.trigger_keyword.is_empty())
        .filter_map(|r| {
            // 大小写归一化（I3）：DB trigger_keyword 可能含大写（seed/导入/老数据），
            // 用户也可能输大写（/Google）。两边都 to_lowercase 后再匹配，避免漏匹配。
            let ms = match_score(&cmd.to_lowercase(), &r.trigger_keyword.to_lowercase())?;
            let final_score = SLASH_BASE_SCORE + ms;
            Some((final_score, slash_result_with_params(r, params, final_score)))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(10)
        .map(|(s, mut r)| {
            r.score = s;
            r
        })
        .collect()
}

/// slash 结果的基础分（参考旧 quicklink 用的 15_000）。
/// 确保 all tab 下 slash 命令不沉底（其他源 menu/app/quicklink 都有正分）。
const SLASH_BASE_SCORE: i32 = 15_000;

/// 构造 slash 命令候选结果（无 params 版，用于仅 "/" 时列全部）。
/// 统一用基础分——仅 "/" 时所有命令等权，保持 DB 行序。
fn slash_result(r: &octopus_infra::db::ActionBarItem) -> SearchResult {
    slash_result_with_params(r, "", SLASH_BASE_SCORE)
}

fn slash_result_with_params(
    r: &octopus_infra::db::ActionBarItem,
    params: &str,
    score: i32,
) -> SearchResult {
    SearchResult {
        source: "slash".into(),
        title: format!("/{}", r.trigger_keyword),
        subtitle: r.title.clone(),
        icon: None,
        action_type: r.action_type.clone(),
        action_data: serde_json::json!({
            "id": r.id,
            "cmd": r.trigger_keyword,
            "params": params,
            "action_type": r.action_type,
            "action_data": r.action_data,
        })
        .to_string(),
        score,
    }
}

#[cfg(test)]
mod slash_command_tests {
    use super::*;
    use octopus_infra::db::ActionBarItem;

    /// 构造测试用 ActionBarItem（trigger_keyword 非空才进 slash 匹配）。
    /// 字段以 crates/infra/src/db/action_bar.rs 的真实 struct 为准。
    fn item(id: i64, _over: &str, trigger: &str, action_type: &str) -> ActionBarItem {
        ActionBarItem {
            id,
            parent_id: None,
            title: format!("Test {}", id),
            icon: String::new(),
            action_type: action_type.into(),
            action_data: "https://example.com/?q={query}".into(),
            sort_order: 0,
            is_system: false,
            is_enabled: true,
            is_async: false,
            write_output_to_clipboard: false,
            shortcut: String::new(),
            agent: String::new(),
            accepts: "text".into(),
            trigger_keyword: trigger.into(),
            global_shortcut: String::new(),
            need_voice: false,
            app_bundle_ids: String::new(),
        }
    }

    #[test]
    fn slash_with_cmd_and_params_matches() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/google hello", &rows);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "slash");
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["cmd"], "google");
        assert_eq!(data["params"], "hello");
        assert_eq!(data["id"], 1);
    }

    #[test]
    fn slash_cmd_no_params() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/google", &rows);
        assert_eq!(results.len(), 1);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["params"], "");
    }

    #[test]
    fn slash_only_returns_all_commands() {
        // 仅 "/" → 返回所有配了 trigger_keyword 的项
        let rows = vec![item(1, "", "google", "url"), item(2, "", "tolaria", "agent")];
        let results = search_slash_commands("/", &rows);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn slash_fuzzy_matches_partial() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/goo", &rows);
        assert_eq!(results.len(), 1); // fuzzy 命中
    }

    #[test]
    fn slash_no_match_returns_empty() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/xyz", &rows);
        assert!(results.is_empty());
    }

    #[test]
    fn non_slash_query_returns_empty() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("google hello", &rows);
        assert!(results.is_empty()); // 不以 / 开头，不触发 slash 匹配
    }

    #[test]
    fn slash_matches_all_action_types() {
        // agent/ai/script 类型配了 trigger_keyword 也能匹配
        let rows = vec![item(1, "", "tolaria", "agent")];
        let results = search_slash_commands("/tolaria", &rows);
        assert_eq!(results.len(), 1);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["action_type"], "agent");
    }

    #[test]
    fn slash_empty_trigger_keyword_excluded() {
        let rows = vec![item(1, "", "", "url")]; // trigger_keyword 空
        let results = search_slash_commands("/anything", &rows);
        assert!(results.is_empty());
    }

    #[test]
    fn slash_uppercase_trigger_keyword_matches_lowercase_query() {
        // I3：DB trigger_keyword 含大写（seed/导入/老数据），用户输小写应命中
        let rows = vec![item(1, "", "Google", "url")];
        let results = search_slash_commands("/google", &rows);
        assert_eq!(results.len(), 1); // 大小写归一化后命中
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["cmd"], "Google"); // 保留原始大小写
    }

    #[test]
    fn slash_uppercase_query_matches_lowercase_trigger() {
        // I3：用户输大写 query，DB 小写 trigger_keyword 也应命中
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/Google", &rows);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn slash_only_all_results_have_high_base_score() {
        // I1：仅 "/" 列全部命令时，score 应为基础分（非 0），all tab 不沉底
        let rows = vec![item(1, "", "google", "url"), item(2, "", "tolaria", "agent")];
        let results = search_slash_commands("/", &rows);
        assert_eq!(results.len(), 2);
        assert!(results[0].score > 0);
        assert!(results[1].score > 0);
    }

    #[test]
    fn slash_exact_match_scores_higher_than_fuzzy() {
        // I1：精确匹配 /google 应比 fuzzy /goo 分高（都叠加在 15_000 基础分上）
        let rows = vec![item(1, "", "google", "url")];
        let exact = search_slash_commands("/google", &rows);
        let fuzzy = search_slash_commands("/goo", &rows);
        assert_eq!(exact.len(), 1);
        assert_eq!(fuzzy.len(), 1);
        assert!(exact[0].score > fuzzy[0].score);
    }
}
