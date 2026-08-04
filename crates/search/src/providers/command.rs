//! 命令查阅 Provider：CLI 命令 fuzzy 匹配（命令名 + 关键字 + 描述）。
//!
//! source="command"，matches_tab="commands"，uses_frequency=false（命令查阅
//! 频次信号弱——用户更关心"找到哪个命令"，而非"最近用过哪个"）。
//! 匹配优先级：name > keywords > description（取最高分），subtitle 优先 keywords
//! （LLM 中文，可读性好），空则回退 description（英文 whats/brew desc）。
//! action_type="copy"，action_data={"text": name}——用户选择即复制命令到剪贴板。

use async_trait::async_trait;

use crate::command_index::CommandEntry;
use crate::engine::SearchResult;
use crate::matcher::match_score;
use crate::provider::{SearchContext, SearchProvider};

pub struct CommandProvider;

#[async_trait]
impl SearchProvider for CommandProvider {
    fn id(&self) -> &'static str {
        "command"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "commands")
    }

    fn uses_frequency(&self) -> bool {
        false
    }

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let cmds = ctx.command_index.read();
        let mut scored: Vec<(i32, &CommandEntry)> = cmds
            .commands
            .iter()
            .filter_map(|cmd| {
                // 匹配优先级：name > keywords > description（取首个 Some）。
                // name/keywords 是用户主意图，description 是兜底信号——同时匹配多个时
                // name 匹配得分最高（exact 10000 / prefix 5000 等），保证命令名优先。
                let score = match_score(query, &cmd.name)
                    .or_else(|| match_score(query, &cmd.keywords))
                    .or_else(|| match_score(query, &cmd.description))?;
                Some((score, cmd))
            })
            .collect();
        // 按 score 降序——name 命中（高分）排在 keywords/description 命中前。
        scored.sort_by_key(|x| std::cmp::Reverse(x.0));
        scored
            .into_iter()
            .take(20)
            .map(|(score, cmd)| SearchResult {
                source: "command".into(),
                title: cmd.name.clone(),
                // subtitle 优先 keywords（LLM 生成的中文摘要，对中文用户更友好）；
                // keywords 空时回退 description（英文 whats/brew desc）。
                subtitle: if cmd.keywords.is_empty() {
                    cmd.description.clone()
                } else {
                    cmd.keywords.clone()
                },
                icon: None,
                action_type: "copy_and_reveal".into(),
                action_data: serde_json::json!({
                    "text": cmd.name,
                    "path": cmd.path,
                }).to_string(),
                score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_index::AppIndex;
    use crate::bookmark::BookmarkEntry;
    use crate::command_index::{CommandEntry, CommandIndex};
    use crate::frequency::FrequencyScorer;
    use parking_lot::RwLock;

    fn ctx<'a>(
        cmd_idx: &'a RwLock<CommandIndex>,
        f: &'a FrequencyScorer,
        a: &'a RwLock<AppIndex>,
        b: &'a RwLock<Vec<BookmarkEntry>>,
    ) -> SearchContext<'a> {
        SearchContext {
            app_index: a,
            bookmarks: b,
            frequency: f,
            command_index: cmd_idx,
            tab: "commands",
        }
    }

    fn mk_cmd(name: &str, keywords: &str, desc: &str) -> CommandEntry {
        CommandEntry {
            name: name.into(),
            path: format!("/usr/bin/{}", name),
            source: "system".into(),
            description: desc.into(),
            keywords: keywords.into(),
        }
    }

    #[tokio::test]
    async fn name_match_returns_result() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let idx = RwLock::new(CommandIndex {
            commands: vec![mk_cmd("git", "版本控制", "distributed version control")],
        });
        let r = p.search("git", &ctx(&idx, &f, &a, &b)).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source, "command");
        assert_eq!(r[0].title, "git");
        assert_eq!(r[0].action_type, "copy_and_reveal");
        assert!(r[0].action_data.contains("\"text\":\"git\""));
        assert!(r[0].action_data.contains("\"path\""), "action_data 应含 path（命令文件路径）");
        // keywords 非空 → subtitle 用 keywords
        assert_eq!(r[0].subtitle, "版本控制");
    }

    #[tokio::test]
    async fn keyword_match_used_when_name_misses() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let idx = RwLock::new(CommandIndex {
            commands: vec![mk_cmd("git", "版本控制", "distributed version control")],
        });
        // 搜 "版本" 不匹配 name="git" 和 description，但匹配 keywords
        let r = p.search("版本", &ctx(&idx, &f, &a, &b)).await;
        assert_eq!(r.len(), 1, "keywords 命中应返回结果");
        assert_eq!(r[0].title, "git");
    }

    #[tokio::test]
    async fn description_fallback_when_keywords_empty() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        // keywords 空 → subtitle 用 description
        let idx = RwLock::new(CommandIndex {
            commands: vec![mk_cmd("grep", "", "search file patterns")],
        });
        let r = p.search("grep", &ctx(&idx, &f, &a, &b)).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].subtitle, "search file patterns");
    }

    #[tokio::test]
    async fn no_match_returns_empty() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let idx = RwLock::new(CommandIndex {
            commands: vec![mk_cmd("git", "", "version control")],
        });
        let r = p.search("zzznomatch", &ctx(&idx, &f, &a, &b)).await;
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn results_sorted_by_score_desc() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        // "g" prefix-matches both "git" and "grep"; name 命中分数相同，
        // 但 "git" 短（remaining 少）→ prefix 分更高
        let idx = RwLock::new(CommandIndex {
            commands: vec![
                mk_cmd("grep", "", "search patterns"),
                mk_cmd("git", "", "version control"),
            ],
        });
        let r = p.search("g", &ctx(&idx, &f, &a, &b)).await;
        assert_eq!(r.len(), 2);
        assert!(r[0].score >= r[1].score, "应按 score 降序");
    }

    #[tokio::test]
    async fn takes_at_most_20() {
        let p = CommandProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        // 25 个都以 "cmd" 开头的命令——应只返回前 20
        let cmds: Vec<CommandEntry> = (0..25)
            .map(|i| mk_cmd(&format!("cmd{}", i), "", "test command"))
            .collect();
        let idx = RwLock::new(CommandIndex { commands: cmds });
        let r = p.search("cmd", &ctx(&idx, &f, &a, &b)).await;
        assert_eq!(r.len(), 20, "应截断到 20 条");
    }
}
