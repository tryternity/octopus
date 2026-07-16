//! 计算器 Provider：表达式求值（evalexpr）。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct CalculatorProvider;

#[async_trait]
impl SearchProvider for CalculatorProvider {
    fn id(&self) -> &'static str {
        "calculator"
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
        if !looks_like_expression(q) {
            return vec![];
        }
        match evalexpr::eval(q) {
            Ok(val) => {
                let num_str = format_value(&val);
                // 不显示无意义结果（空字符串）
                if num_str.is_empty() {
                    return vec![];
                }
                vec![SearchResult {
                    source: "calculator".into(),
                    title: format!("= {}", num_str),
                    subtitle: "计算结果".into(),
                    icon: None,
                    action_type: "copy".into(),
                    action_data: serde_json::json!({ "text": num_str }).to_string(),
                    score: 10000,
                }]
            }
            Err(_) => vec![],
        }
    }
}

fn looks_like_expression(s: &str) -> bool {
    let has_op = s.chars().any(|c| matches!(c, '+' | '-' | '*' | '/' | '%'));
    let all_valid = s.chars().all(|c| {
        c.is_ascii_digit()
            || matches!(c, '+' | '-' | '*' | '/' | '%' | '(' | ')' | '.' | ' ')
    });
    has_op && all_valid && !s.ends_with(|c: char| matches!(c, '+' | '-' | '*' | '/'))
}

fn format_value(val: &evalexpr::Value) -> String {
    use evalexpr::Value::*;
    match val {
        Int(i) => i.to_string(),
        Float(f) => {
            // 整数浮点显示为整数（2.0 → 2）
            if f.fract() == 0.0 && f.is_finite() {
                format!("{:.0}", f)
            } else {
                format!("{}", f)
            }
        }
        Boolean(b) => b.to_string(),
        String(s) => s.clone(),
        _ => val.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_index::AppIndex;
    use crate::bookmark::BookmarkEntry;
    use crate::frequency::FrequencyScorer;
    use parking_lot::RwLock;

    fn ctx<'a>(
        f: &'a FrequencyScorer,
        a: &'a RwLock<AppIndex>,
        b: &'a RwLock<Vec<BookmarkEntry>>,
    ) -> SearchContext<'a> {
        SearchContext {
            app_index: a,
            bookmarks: b,
            frequency: f,
        }
    }

    #[tokio::test]
    async fn basic_arithmetic() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let r = p.search("1+2", &ctx(&f, &a, &b)).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "= 3");
    }

    #[tokio::test]
    async fn division_by_zero_returns_empty() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let r = p.search("1/0", &ctx(&f, &a, &b)).await;
        assert!(r.is_empty(), "除零应返回空");
    }

    #[tokio::test]
    async fn non_expression_returns_empty() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        // "abc" 含字母，looks_like_expression 返回 false
        let r = p.search("abc", &ctx(&f, &a, &b)).await;
        assert!(r.is_empty());
        // "hello" 无运算符
        let r = p.search("hello", &ctx(&f, &a, &b)).await;
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn float_result() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        // 注意：evalexpr 11 对 Int/Int 做整数除法（10/4 → Int(2)），
        // 故此处用浮点操作数触发 Float 路径以验证非整数显示。
        let r = p.search("10.0/4", &ctx(&f, &a, &b)).await;
        assert_eq!(r[0].title, "= 2.5");
    }
}
