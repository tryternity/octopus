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
        // 将所有整数字面量转为浮点（如 10 → 10.0），强制走 Float 路径。
        // evalexpr 11 对 Int/Int 做整数除法（10/4 → Int(2)），用户期望 JS/Python3 风格的
        // 浮点除法（10/4 → 2.5）。注意：reviewer 推荐的 1.0*(<expr>) 包裹在含括号或非
        // 最左除法的表达式上仍触发整数除法（如 5-10/4 → 5.0-2 = 3.0，期望 2.5），
        // 直接将字面量升为 Float 可在所有算式上一致正确。
        let q_float = promote_int_literals_to_float(q);
        match evalexpr::eval(&q_float) {
            Ok(val) => {
                // 跳过非有限结果（如 1/0 → inf、0/0 → NaN）：浮点化后除零不再报错，
                // 需在此显式过滤，避免展示 "inf" 这类无意义值。
                if let evalexpr::Value::Float(f) = &val {
                    if !f.is_finite() {
                        return vec![];
                    }
                }
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
    has_op && all_valid && !s.ends_with(['+', '-', '*', '/'])
}

/// 将表达式中的整数字面量改为浮点字面量（如 `10` → `10.0`，`10.5` 保持不变）。
/// 用于规避 evalexpr 11 的整数除法，让所有 `/` 都产生浮点结果。
fn promote_int_literals_to_float(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            // 读取整数部分
            let mut num = String::from(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() {
                    num.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            // 若紧随小数点，说明已是浮点字面量（如 10.5），原样保留小数部分
            if chars.peek() == Some(&'.') {
                num.push('.');
                chars.next();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() {
                        num.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&num);
            } else {
                out.push_str(&num);
                out.push_str(".0");
            }
        } else {
            out.push(c);
        }
    }
    out
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
    async fn basic_arithmetic() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("1+2", &ctx(&f, &a, &b, &c)).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "= 3");
    }

    #[tokio::test]
    async fn division_by_zero_returns_empty() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("1/0", &ctx(&f, &a, &b, &c)).await;
        assert!(r.is_empty(), "除零应返回空");
    }

    #[tokio::test]
    async fn non_expression_returns_empty() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        // "abc" 含字母，looks_like_expression 返回 false
        let r = p.search("abc", &ctx(&f, &a, &b, &c)).await;
        assert!(r.is_empty());
        // "hello" 无运算符
        let r = p.search("hello", &ctx(&f, &a, &b, &c)).await;
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn float_result() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        // 1.0* 包裹后 Int/Int 除法变 Float，10/4 → 2.5
        let r = p.search("10/4", &ctx(&f, &a, &b, &c)).await;
        assert_eq!(r[0].title, "= 2.5");
    }

    #[tokio::test]
    async fn integer_result_no_decimal() {
        let p = CalculatorProvider;
        let f = FrequencyScorer::with_test_data(Default::default());
        let a = RwLock::new(AppIndex { apps: vec![] });
        let b = RwLock::new(vec![]);
        let c = RwLock::new(CommandIndex::empty());
        let r = p.search("1+2", &ctx(&f, &a, &b, &c)).await;
        // 1.0*(1+2) = 3.0，format_value 应显示为 "3" 不是 "3.0"
        assert_eq!(r[0].title, "= 3");
    }
}
