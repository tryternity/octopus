//! 频次加权：基于历史命中给搜索结果加分（简化版 wox 斐波那契衰减）。

use std::collections::HashMap;

use crate::engine::SearchResult;

/// 从 action_data JSON 提取稳定字段，拼成 score_key。
/// 格式：`<source>|<稳定字段>`。稳定字段：app=path、file=、bookmark=url、menu/quicklink=id。
/// title 不参与（title 随本地化变）。
pub fn make_score_key(source: &str, action_type: &str, action_data: &str) -> String {
    let stable = extract_stable_field(action_type, action_data);
    format!("{}|{}", source, stable)
}

fn extract_stable_field(action_type: &str, action_data: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(action_data) {
        Ok(v) => v,
        Err(_) => return action_data.to_string(),  // fallback：原文
    };
    // 优先字段：path > url > id > command（按 action_type 语义）
    for key in &["path", "url", "id", "command"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return s.to_string();
        }
        if let Some(n) = v.get(key).and_then(|x| x.as_i64()) {
            return n.to_string();
        }
    }
    let _ = action_type;
    action_data.to_string()
}

pub struct FrequencyScorer {
    /// 内存缓存：启动时从 DB load，record 时同步更新。
    freqs: parking_lot::RwLock<HashMap<String, octopus_infra::db::FreqRow>>,
}

impl FrequencyScorer {
    /// 生产构造：从 DB 加载全部频次记录。
    pub fn load() -> Self {
        let freqs = octopus_infra::db::load_search_frequency().unwrap_or_default();
        FrequencyScorer {
            freqs: parking_lot::RwLock::new(freqs),
        }
    }

    /// 测试构造：直接注入数据。
    pub fn with_test_data(freqs: HashMap<String, octopus_infra::db::FreqRow>) -> Self {
        FrequencyScorer {
            freqs: parking_lot::RwLock::new(freqs),
        }
    }

    /// 给一批结果加分。query 是当前查询（完全匹配额外加分）。
    pub fn boost(&self, results: &mut [SearchResult], query: &str) {
        let freqs = self.freqs.read();
        let now = now_ts();
        for r in results.iter_mut() {
            // shell/calculator/url 不加权（Provider 声明 uses_frequency=false，
            // 但 boost 不知道 Provider——用 source 名单判断）
            if matches!(r.source.as_str(), "shell" | "calculator" | "url") {
                continue;
            }
            let key = make_score_key(&r.source, &r.action_type, &r.action_data);
            if let Some(f) = freqs.get(&key) {
                let days_ago = (now - f.last_hit_ts) / 86400;
                let base: i32 = match days_ago {
                    0 => 3000,
                    1 => 2000,
                    2..=3 => 1000,
                    4..=7 => 500,
                    _ => 0,
                };
                let count_factor = (f.hit_count as i32).min(5);
                r.score += base * count_factor;
                if !query.is_empty() && f.query.eq_ignore_ascii_case(query) {
                    r.score += 500;
                }
            }
        }
    }

    /// 记录一次命中（执行动作时调）。同步刷 DB + 内存。
    pub fn record(&self, result: &SearchResult, query: &str) {
        let key = make_score_key(&result.source, &result.action_type, &result.action_data);
        if let Err(e) = octopus_infra::db::record_search_frequency(&key, query) {
            log::warn!("[search] record_search_frequency failed: {}", e);
        }
        // 刷内存：重 load（简单，避免重复实现 upsert 内存逻辑）
        if let Ok(new_freqs) = octopus_infra::db::load_search_frequency() {
            *self.freqs.write() = new_freqs;
        }
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_key_uses_action_data_stable_fields() {
        // app: source + path
        let k = make_score_key("app", "launch_app", r#"{"path":"/Applications/Chrome.app"}"#);
        assert_eq!(k, "app|/Applications/Chrome.app");
        // bookmark: source + url
        let k = make_score_key("bookmark", "url", r#"{"url":"https://github.com"}"#);
        assert_eq!(k, "bookmark|https://github.com");
        // menu: source + id
        let k = make_score_key("menu", "menu", r#"{"id":42}"#);
        assert_eq!(k, "menu|42");
    }

    #[test]
    fn boost_today_higher_than_week_ago() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut freqs = std::collections::HashMap::new();
        freqs.insert("app|/A.app".to_string(), octopus_infra::db::FreqRow {
            hit_count: 3, last_hit_ts: now, query: "a".into(),
        });
        freqs.insert("app|/B.app".to_string(), octopus_infra::db::FreqRow {
            hit_count: 3, last_hit_ts: now - 8 * 86400, query: "b".into(),
        });
        let scorer = FrequencyScorer::with_test_data(freqs);
        let mut results = vec![
            SearchResult { source: "app".into(), title: "A".into(), subtitle: "".into(),
                icon: None, action_type: "launch_app".into(),
                action_data: r#"{"path":"/A.app"}"#.into(), score: 4000 },
            SearchResult { source: "app".into(), title: "B".into(), subtitle: "".into(),
                icon: None, action_type: "launch_app".into(),
                action_data: r#"{"path":"/B.app"}"#.into(), score: 4000 },
        ];
        scorer.boost(&mut results, "a");
        // A 今天用过，加分；B 一周前，不加分 → A 分高
        assert!(results[0].score > results[1].score, "today ({}) should outrank week-ago ({})", results[0].score, results[1].score);
    }

    #[test]
    fn boost_query_exact_match_bonus() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut freqs = std::collections::HashMap::new();
        freqs.insert("app|/A.app".to_string(), octopus_infra::db::FreqRow {
            hit_count: 1, last_hit_ts: now, query: "abc".into(),
        });
        let scorer = FrequencyScorer::with_test_data(freqs);
        let mut r = vec![SearchResult {
            source: "app".into(), title: "A".into(), subtitle: "".into(),
            icon: None, action_type: "launch_app".into(),
            action_data: r#"{"path":"/A.app"}"#.into(), score: 4000,
        }];
        scorer.boost(&mut r, "abc");  // query 完全匹配
        let with_match = r[0].score;
        let mut r2 = vec![SearchResult {
            source: "app".into(), title: "A".into(), subtitle: "".into(),
            icon: None, action_type: "launch_app".into(),
            action_data: r#"{"path":"/A.app"}"#.into(), score: 4000,
        }];
        scorer.boost(&mut r2, "xyz");  // query 不匹配
        assert!(with_match > r2[0].score, "query exact match should get bonus");
    }
}
