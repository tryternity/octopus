//! URL 检测 Provider：输入像域名/http 时提供"打开网址"项。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct UrlProvider;

#[async_trait]
impl SearchProvider for UrlProvider {
    fn id(&self) -> &'static str {
        "url"
    }

    /// 仅由 search() 的 tab=="all" 包含。
    fn matches_tab(&self, _tab: &str) -> bool {
        false
    }

    fn uses_frequency(&self) -> bool {
        false
    }

    async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let q = query.trim();
        if !looks_like_url(q) {
            return vec![];
        }
        let url = if q.starts_with("http://") || q.starts_with("https://") {
            q.to_string()
        } else {
            format!("https://{}", q)
        };
        vec![SearchResult {
            source: "url".into(),
            title: format!("打开 {}", q),
            subtitle: "网址".into(),
            icon: None,
            action_type: "url".into(),
            action_data: serde_json::json!({ "url": url }).to_string(),
            score: 9000,
        }]
    }
}

fn looks_like_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        || (s.contains('.') && {
            let last = s.rsplit('.').next().unwrap_or("");
            last.len() >= 2 && last.chars().all(|c| c.is_ascii_alphabetic())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_index::AppIndex;
    use crate::bookmark::BookmarkEntry;
    use crate::command_index::CommandIndex;
    use crate::frequency::FrequencyScorer;
    use parking_lot::RwLock;

    fn ctx<'a>(
        f: &'a FrequencyScorer,
        a: &'a RwLock<AppIndex>,
        b: &'a RwLock<Vec<BookmarkEntry>>,
        c: &'a RwLock<CommandIndex>,
    ) -> SearchContext<'a> {
        SearchContext {
            app_index: a,
            bookmarks: b,
            frequency: f,
            command_index: c,
            tab: "all",
        }
    }

    #[tokio::test]
    async fn domain_detected() {
        let p = UrlProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("github.com", &ctx(&f, &a, &b, &c)).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].action_type, "url");
        assert!(r[0].action_data.contains("https://github.com"));
    }

    #[tokio::test]
    async fn http_prefix_kept() {
        let p = UrlProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("http://example.com", &ctx(&f, &a, &b, &c)).await;
        assert!(r[0].action_data.contains("http://example.com"));
    }

    #[tokio::test]
    async fn non_url_rejected() {
        let p = UrlProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("hello", &ctx(&f, &a, &b, &c)).await;
        assert!(r.is_empty());
    }
}
