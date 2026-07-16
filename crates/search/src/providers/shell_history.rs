//! Shell 历史记录缓存（进程内，惰性加载）。

pub struct ShellHistoryCache {
    entries: std::sync::OnceLock<Vec<String>>,
}

impl ShellHistoryCache {
    pub fn new() -> Self {
        ShellHistoryCache {
            entries: std::sync::OnceLock::new(),
        }
    }

    /// fuzzy 匹配历史命令，返回最多 20 条。
    pub fn search(&self, query: &str) -> Vec<String> {
        if query.is_empty() {
            return vec![];
        }
        let entries = self.entries.get_or_init(load_history_files);
        let mut scored: Vec<(i32, String)> = entries.iter()
            .filter_map(|h| {
                crate::matcher::fuzzy_match(query, h).map(|s| (s, h.clone()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(20).map(|(_, h)| h).collect()
    }
}

impl Default for ShellHistoryCache {
    fn default() -> Self { Self::new() }
}

fn load_history_files() -> Vec<String> {
    let mut all = vec![];
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return all,
    };
    // zsh_history（含时间戳格式 : ts:0;cmd）
    let zsh_path = home.join(".zsh_history");
    if let Ok(content) = std::fs::read_to_string(&zsh_path) {
        all.extend(parse_zsh_history(&content));
    }
    // bash_history（纯命令行）
    let bash_path = home.join(".bash_history");
    if let Ok(content) = std::fs::read_to_string(&bash_path) {
        all.extend(content.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
    }
    all
}

/// 解析 zsh_history：每行格式 `: <ts>:0;<cmd>` 或扩展历史 `<timestamp>;<cmd>`。
fn parse_zsh_history(content: &str) -> Vec<String> {
    content.lines()
        .map(|line| {
            let line = line.trim();
            // 格式 ": 1234567890:0;git status"
            if let Some(idx) = line.find(';') {
                // 跳过 ": ts:0" 前缀（第一个 ; 之后）
                let after = &line[idx + 1..];
                // 有时 after 还含一层 "<ts>;"（extended_history），再找一次
                if let Some(idx2) = after.find(';') {
                    if after[..idx2].chars().all(|c| c.is_ascii_digit()) {
                        return after[idx2 + 1..].trim().to_string();
                    }
                }
                return after.trim().to_string();
            }
            line.to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_zsh_basic() {
        let content = ": 1234567890:0;git status\n: 1234567891:0;ls -la\n";
        let parsed = parse_zsh_history(content);
        assert_eq!(parsed, vec!["git status".to_string(), "ls -la".to_string()]);
    }

    #[test]
    fn parse_zsh_extended_history() {
        let content = ": 1234567890:0;echo hi";
        let parsed = parse_zsh_history(content);
        assert_eq!(parsed[0], "echo hi");
    }

    #[test]
    fn parse_zsh_no_timestamp_fallback() {
        let content = "git status\nls\n";
        let parsed = parse_zsh_history(content);
        assert_eq!(parsed, vec!["git status".to_string(), "ls".to_string()]);
    }
}
