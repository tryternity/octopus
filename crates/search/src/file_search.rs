//! mdfind 文件搜索（macOS Spotlight metadata）。

use super::engine::SearchResult;
use super::matcher::{match_score, Score};

/// 用 mdfind 搜索文件名。异步执行，限制 10 条结果。
pub async fn search_files(query: &str) -> Vec<SearchResult> {
    if query.len() < 2 {
        return Vec::new();
    }

    let output = tokio::process::Command::new("mdfind")
        .args(["-name", query])
        .output()
        .await;

    let stdout = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<(Score, SearchResult)> = stdout
        .lines()
        .take(20)
        .filter_map(|line| {
            let filename = std::path::Path::new(line)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(line);
            let score = match_score(query, filename)?;
            Some((score, SearchResult {
                source: "file".into(),
                title: filename.to_string(),
                subtitle: line.to_string(),
                action_type: "open_file".into(),
                action_data: serde_json::json!({ "path": line }).to_string(),
                score: 0,
            }))
        })
        .collect();

    results.sort_by(|a, b| b.0.cmp(&a.0));
    results.into_iter().take(10).map(|(s, mut r)| { r.score = s; r }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_empty() {
        // search_files 是 async，但 query < 2 直接返回空，无需 await
        // 用 block_on 测
        let rt = tokio::runtime::Runtime::new().unwrap();
        let results = rt.block_on(search_files(""));
        assert!(results.is_empty());
    }

    #[test]
    fn single_char_returns_empty() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let results = rt.block_on(search_files("x"));
        assert!(results.is_empty());
    }
}
