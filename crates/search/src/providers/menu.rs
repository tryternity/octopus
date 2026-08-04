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
        // 第七轮 P2-b：DB 调用包 spawn_blocking，避免 with_db 全局 ReentrantMutex 阻塞 tokio
        // worker（转录持久化/热词写/配置 save 持锁时搜索卡顿）。
        let rows = match tokio::task::spawn_blocking(octopus_infra::db::list_action_bar_items).await {
            Ok(Ok(r)) => r,
            _ => return vec![],
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

/// Slash 命令匹配（v2）：query 以 `/cmd [params]` 或 `、cmd [params]` 开头时，
/// 在**所有 is_enabled 非 submenu 菜单项**上做双维 fuzzy（命令名 trigger_keyword +
/// 标题 title），返回 source="slash" 候选。params 记入 action_data 供执行时用。
///
/// IME 兼容：开头 `/` 或 `、`（U+3001 顿号，中文输入法下 / 的变形）都触发。
///
/// 仅 "/" / "、" → 返回全部候选（固定基础分，保持 DB 行序）。
/// query 不以 / 或 、 开头 → 空结果（不影响普通搜索）。
///
/// 计分：
/// - 基础分 SLASH_BASE_SCORE（15_000，参考旧 quicklink）保证 all tab 不沉底；
/// - 命令名匹配（trigger_keyword 非空且命中）加 SLASH_KW_BOOST（10_000），
///   保证命令名任意命中（含最低 fuzzy）+ boost > 标题最高 exact(10_000)，即命令名项始终优先于纯标题项；
/// - 标题匹配作为第二召回维度（无 trigger_keyword 的项也能命中）；
/// - 取两者中较高者（命令名命中时再叠加 boost）。
fn search_slash_commands(
    query: &str,
    rows: &[octopus_infra::db::ActionBarItem],
) -> Vec<SearchResult> {
    // IME 兼容：开头 / 或 、（U+3001 顿号）
    let rest = match query.strip_prefix('/').or_else(|| query.strip_prefix('、')) {
        Some(r) => r,
        None => return vec![],
    };

    // 候选池：is_enabled && 非 submenu（不限 trigger_keyword）
    let candidates: Vec<&octopus_infra::db::ActionBarItem> = rows
        .iter()
        .filter(|r| r.is_enabled && r.action_type != "submenu")
        .collect();

    // 仅 / 或 、 → 返回全部候选（用户主动列命令，统一高基础分置顶）
    if rest.is_empty() {
        return candidates
            .iter()
            .map(|r| slash_result(r, "", SLASH_BASE_SCORE))
            .collect();
    }

    // 切 cmd（/ 后到空格前）+ params（空格后）
    let (cmd, params) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let cmd_lower = cmd.to_lowercase();

    // 双维匹配：trigger_keyword（若有）+ title，命令名命中加 boost
    let mut scored: Vec<(i32, &octopus_infra::db::ActionBarItem)> = candidates
        .iter()
        .filter_map(|r| {
            // 命令名匹配（trigger_keyword 非空时）；大小写归一化（I3）
            let kw_score = if !r.trigger_keyword.is_empty() {
                match_score(&cmd_lower, &r.trigger_keyword.to_lowercase())
            } else {
                None
            };
            // 标题匹配（第二召回维度）
            let title_score = match_score(&cmd_lower, &r.title.to_lowercase());
            // 命令名命中时用命令名分+boost（命令名是更强信号，不与标题分取 max）；
            // 否则用标题分。boost 保证命令名任意命中 > 标题最高 exact。
            let score = match (kw_score, title_score) {
                (Some(k), _) => Some(k + SLASH_KW_BOOST),
                (None, Some(t)) => Some(t),
                (None, None) => None,
            };
            score.map(|s| (s, *r))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(10)
        .map(|(s, r)| slash_result(r, params, SLASH_BASE_SCORE + s))
        .collect()
}

/// slash 结果的基础分（参考旧 quicklink 用的 15_000）。
/// 确保 all tab 下 slash 命令不沉底（其他源 menu/app/quicklink 都有正分）。
const SLASH_BASE_SCORE: i32 = 15_000;

/// 命令名匹配 boost：trigger_keyword 命中比纯标题命中略优先（v2 双维匹配）。
const SLASH_KW_BOOST: i32 = 10_000;

/// 构造 slash 命令候选结果（v2 合并版）。
/// r: 候选菜单项；params: query 中空格后的参数；score: 最终分数（含基础分）。
/// 显示标题用菜单 title（而非 /trigger_keyword），让用户看到可读名称；
/// action_data 携带 title 供前端 Tab 补全。
fn slash_result(
    r: &octopus_infra::db::ActionBarItem,
    params: &str,
    score: i32,
) -> SearchResult {
    SearchResult {
        source: "slash".into(),
        title: r.title.clone(),
        subtitle: if r.trigger_keyword.is_empty() {
            r.action_type.clone()
        } else {
            format!("/{}", r.trigger_keyword)
        },
        icon: None,
        action_type: r.action_type.clone(),
        action_data: serde_json::json!({
            "id": r.id,
            "title": r.title,
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

    /// 构造测试用 ActionBarItem（trigger_keyword 非空才进命令名匹配，空则走标题匹配）。
    /// 字段以 crates/infra/src/db/action_bar.rs 的真实 struct 为准。
    /// 参数 title 为菜单标题（v2 候选显示标题）；trigger 为命令名（可空）。
    fn item(id: i64, title: &str, trigger: &str, action_type: &str) -> ActionBarItem {
        ActionBarItem {
            id,
            parent_id: None,
            title: if title.is_empty() { format!("Test {}", id) } else { title.into() },
            icon: String::new(),
            action_type: action_type.into(),
            action_data: "https://example.com/?q={query}".into(),
            sort_order: 0,
            is_system: false,
            is_enabled: true,
            is_async: false,
            write_output_to_clipboard: false,
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
        // v2: 候选显示菜单标题（"Test 1"），不再显示 "/google"
        assert_eq!(results[0].title, "Test 1");
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["cmd"], "google");
        assert_eq!(data["params"], "hello");
        assert_eq!(data["id"], 1);
        // v2: action_data 携带 title 供前端 Tab 补全
        assert_eq!(data["title"], "Test 1");
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
    fn slash_only_returns_all_candidates() {
        // v2: 仅 "/" → 返回所有 is_enabled 非 submenu 项（不限 trigger_keyword）
        let rows = vec![
            item(1, "", "google", "url"),       // 有 trigger
            item(2, "翻译", "", "ai"),           // 无 trigger，但非 submenu
            item(3, "Agent菜单", "", "submenu"), // submenu 排除
        ];
        let results = search_slash_commands("/", &rows);
        assert_eq!(results.len(), 2); // google + 翻译
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
    fn slash_empty_trigger_keyword_no_title_match_returns_empty() {
        // v2: trigger_keyword 空的项也进候选池（按标题匹配）；
        // 标题（Test 1）与查询（anything）不匹配 → 不命中。
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

    // ── v2：IME 兼容 + 候选池扩大 + 双维匹配 ──

    #[test]
    fn slash_ideographic_comma_prefix_also_works() {
        // 、（顿号，U+3001）开头等同 / 开头（中文输入法下 / 的变形）
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("、google hello", &rows);
        assert_eq!(results.len(), 1);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["params"], "hello");
    }

    #[test]
    fn slash_title_match_for_item_without_trigger_keyword() {
        // v2: 无 trigger_keyword 的项，按标题匹配进候选
        let rows = vec![item(2, "百度搜索", "", "url")]; // trigger_keyword 空
        let results = search_slash_commands("/百度", &rows);
        assert_eq!(results.len(), 1);
        // 候选显示标题
        assert_eq!(results[0].title, "百度搜索");
    }

    #[test]
    fn slash_command_name_outranks_title_match() {
        // v2: trigger_keyword 命中（+KW_BOOST）优先于纯标题 fuzzy
        let rows = vec![
            item(1, "Google", "google", "url"),   // trigger=google
            item(2, "Google Scholar", "", "url"),  // 标题含 google，无 trigger
        ];
        let results = search_slash_commands("/google", &rows);
        assert_eq!(results.len(), 2);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["id"], 1); // trigger 命中的排前
    }

    #[test]
    fn slash_all_items_are_candidates() {
        // v2: 所有 is_enabled 非 submenu 项都进候选池（即使无 trigger_keyword）
        let rows = vec![
            item(1, "Google", "google", "url"),
            item(2, "翻译", "", "ai"),        // 无 trigger
            item(3, "Agent菜单", "", "submenu"), // submenu 排除
        ];
        let results = search_slash_commands("/", &rows);
        assert_eq!(results.len(), 2); // google + 翻译，submenu 排除
    }

    #[test]
    fn slash_command_name_fuzzy_outranks_title_exact() {
        // boost=10000 保证：命令名 prefix/fuzzy 命中（低分）+ boost 仍 > 标题 exact(10000)
        // 项 A：trigger="gg"，query="/g" → kw prefix 命中（~4999）+ boost 10000 = ~14999
        // 项 B：trigger 空，title="g"，query="/g" → title exact = 10000
        // A 应排前（命令名优先）
        let rows = vec![
            item(1, "某命令", "gg", "url"),    // trigger=gg，命令名 prefix 命中
            item(2, "g", "", "url"),            // 无 trigger，标题 exact 命中
        ];
        let results = search_slash_commands("/g", &rows);
        assert_eq!(results.len(), 2);
        let first_id: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(first_id["id"], 1, "命令名命中（含 boost）应优先于标题 exact");
    }
}
