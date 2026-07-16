//! 浏览器书签解析：Safari / Chrome / Edge。

use super::matcher::{match_score, Score};
use super::engine::SearchResult;

pub struct BookmarkEntry {
    pub title: String,
    pub url: String,
    pub browser: String,
}

/// 加载所有浏览器的书签：Chrome / Edge（JSON）+ Safari（plist）+ Firefox（SQLite）。
///
/// 每个浏览器的 loader 自带降级（无权限/无文件返回空 Vec），这里只做存在性预检
/// 跳过明显不存在的路径以省 syscalls，不掩盖 loader 内部错误。
pub fn load_all_bookmarks() -> Vec<BookmarkEntry> {
    let mut bookmarks = Vec::new();
    if let Some(home) = dirs::home_dir() {
        // Chrome / Edge（JSON，无需 Full Disk Access）
        for (browser, path) in &[
            ("Chrome", "Library/Application Support/Google/Chrome/Default/Bookmarks"),
            ("Edge", "Library/Application Support/Microsoft Edge/Default/Bookmarks"),
        ] {
            let full_path = home.join(path);
            if full_path.exists() {
                bookmarks.extend(load_chromium_bookmarks(browser, &full_path));
            }
        }
        // Safari（plist，需 Full Disk Access——失败则 load_safari_bookmarks 自降级）
        let safari_path = home.join("Library/Safari/Bookmarks.plist");
        if safari_path.exists() {
            bookmarks.extend(load_safari_bookmarks(&safari_path));
        }
    }
    // Firefox（SQLite，独立函数自己找 profile）
    bookmarks.extend(load_firefox_bookmarks());
    log::info!("[search] 书签索引: {} 条", bookmarks.len());
    bookmarks
}

/// 解析 Chrome/Edge 书签 JSON。
fn load_chromium_bookmarks(browser: &str, path: &std::path::Path) -> Vec<BookmarkEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut result = Vec::new();
    // 递归遍历 children 数组
    fn walk(node: &serde_json::Value, browser: &str, out: &mut Vec<BookmarkEntry>) {
        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            for child in children {
                if child.get("type").and_then(|t| t.as_str()) == Some("url") {
                    let title = child.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let url = child.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                    if !title.is_empty() && !url.is_empty() {
                        out.push(BookmarkEntry { title, url, browser: browser.into() });
                    }
                }
                walk(child, browser, out);
            }
        }
    }
    let roots = root.get("roots");
    if let Some(roots) = roots {
        // Chromium 的 roots 是一个对象，含 bookmark_bar / other / synced 三个 folder 键。
        // 每个 folder 才有 children 数组。遍历 roots 的每个 value 递归 walk。
        if let Some(roots_obj) = roots.as_object() {
            for (_, folder) in roots_obj {
                walk(folder, browser, &mut result);
            }
        } else {
            // 某些版本 roots 本身就是数组，直接 walk
            walk(roots, browser, &mut result);
        }
    }
    result
}

/// 解析 Safari 书签 plist（XML 或二进制）。
///
/// **降级**：需 Full Disk Access。失败（权限拒绝 / 文件缺失 / 格式异常）时
/// 返回空 Vec + log debug，不 panic 不弹窗。这样无权限用户与无书签用户行为一致。
pub fn load_safari_bookmarks(path: &std::path::Path) -> Vec<BookmarkEntry> {
    let plist_val = match plist::Value::from_file(path) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("[search] Safari plist 解析失败 {}: {}", path.display(), e);
            return vec![];
        }
    };
    let mut result = vec![];
    walk_safari(&plist_val, &mut result);
    result
}

/// 递归遍历 Safari plist 节点。
/// - `WebBookmarkTypeLeaf`：取 `URIDictionary.title` + `URLString`
/// - `WebBookmarkTypeList`：递归 `Children`
///
/// 叶子节点之外也会无差别递归 `Children`（根 dict 不是 WebBookmarkType 但含 Children）。
fn walk_safari(node: &plist::Value, out: &mut Vec<BookmarkEntry>) {
    let dict = match node.as_dictionary() {
        Some(d) => d,
        None => return,
    };
    let bm_type = dict.get("WebBookmarkType").and_then(|v| v.as_string()).unwrap_or("");
    if bm_type == "WebBookmarkTypeLeaf" {
        let title = dict.get("URIDictionary")
            .and_then(|d| d.as_dictionary())
            .and_then(|d| d.get("title"))
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();
        let url = dict.get("URLString")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();
        if !title.is_empty() && !url.is_empty() {
            out.push(BookmarkEntry { title, url, browser: "Safari".into() });
        }
    }
    if let Some(children) = dict.get("Children").and_then(|v| v.as_array()) {
        for child in children {
            walk_safari(child, out);
        }
    }
}

/// 解析 Firefox 书签：读 `places.sqlite`（拷临时文件避免锁运行中的 Firefox）。
///
/// **降级**：找不到 profile / 文件缺失 / 拷贝失败 / 查询失败 → 返回空 Vec。
pub fn load_firefox_bookmarks() -> Vec<BookmarkEntry> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let profiles_dir = home.join("Library/Application Support/Firefox/Profiles");
    // 找 *.default-release profile（Firefox 主用户 profile 命名约定）
    let profile_path = match std::fs::read_dir(&profiles_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().ends_with(".default-release"))
            .map(|e| e.path()),
        Err(_) => return vec![],
    };
    let profile_path = match profile_path {
        Some(p) => p,
        None => return vec![],
    };
    let places = profile_path.join("places.sqlite");
    if !places.exists() {
        return vec![];
    }
    // 拷到临时文件：运行中的 Firefox 会持锁，直接 OpenFlags::READ_ONLY 在某些
    // 平台仍会失败。拷一份隔离，避免污染原 DB / 阻塞用户使用 Firefox。
    let tmp = std::env::temp_dir()
        .join(format!("octopus_ff_places_{}.db", std::process::id()));
    if std::fs::copy(&places, &tmp).is_err() {
        return vec![];
    }
    let result = query_firefox_places(&tmp);
    let _ = std::fs::remove_file(&tmp); // 清理（失败忽略——tmp 目录 OS 会定期清）
    result
}

fn query_firefox_places(db_path: &std::path::Path) -> Vec<BookmarkEntry> {
    use rusqlite::OpenFlags;
    let conn = match rusqlite::Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(e) => {
            log::debug!("[search] Firefox places 打开失败: {}", e);
            return vec![];
        }
    };
    // type=1 是 bookmark（其余如 folder=2 / separator=3 跳过）；
    // 过滤 place:% 这些 Firefox 内部伪 URL（不是真实网页书签）。
    let mut stmt = match conn.prepare(
        "SELECT b.title, p.url FROM moz_bookmarks b
         JOIN moz_places p ON b.fk = p.id
         WHERE b.type = 1 AND p.url NOT LIKE 'place:%'"
    ) {
        Ok(s) => s,
        Err(e) => {
            log::debug!("[search] Firefox places prepare 失败: {}", e);
            return vec![];
        }
    };
    let rows = stmt.query_map([], |row| {
        Ok(BookmarkEntry {
            title: row.get::<_, String>(0)?,
            url: row.get::<_, String>(1)?,
            browser: "Firefox".into(),
        })
    });
    match rows {
        Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
        Err(_) => vec![],
    }
}

/// 搜索书签。
pub fn search_bookmarks(query: &str, bookmarks: &[BookmarkEntry]) -> Vec<SearchResult> {
    let mut scored: Vec<(Score, String, SearchResult)> = bookmarks
        .iter()
        .filter_map(|bm| {
            let score = match_score(query, &bm.title)
                .or_else(|| match_score(query, &bm.url))?;
            Some((score, bm.url.clone(), SearchResult {
                source: "bookmark".into(),
                title: bm.title.clone(),
                subtitle: format!("[{}] {}", bm.browser, bm.url),
                icon: None,
    action_type: "url".into(),
                action_data: serde_json::json!({ "url": bm.url }).to_string(),
                score: 0,
            }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    // 按 url 去重（bookmark_bar + synced 同步可能产出同 URL，保留高分首个）
    let mut seen = std::collections::HashSet::new();
    scored.into_iter()
        .filter(|(_, url, _)| seen.insert(url.clone()))
        .take(10)
        .map(|(s, _, mut r)| { r.score = s; r })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_bookmarks_matches_title() {
        let bookmarks = vec![
            BookmarkEntry { title: "GitHub".into(), url: "https://github.com".into(), browser: "Chrome".into() },
            BookmarkEntry { title: "Google".into(), url: "https://google.com".into(), browser: "Chrome".into() },
        ];
        let results = search_bookmarks("git", &bookmarks);
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "GitHub");
    }

    #[test]
    fn search_bookmarks_empty_query_returns_empty() {
        let bookmarks = vec![
            BookmarkEntry { title: "GitHub".into(), url: "https://github.com".into(), browser: "Chrome".into() },
        ];
        let results = search_bookmarks("", &bookmarks);
        assert!(results.is_empty());
    }

    /// Safari plist 解析降级：文件不存在/无权限时返回空 Vec，不 panic。
    /// 锁住"失败不爆炸"语义——无 Full Disk Access 的用户与无书签用户行为一致。
    #[test]
    fn safari_nonexistent_returns_empty() {
        let entries = load_safari_bookmarks(std::path::Path::new("/nonexistent/Bookmarks.plist"));
        assert!(entries.is_empty());
    }

    /// Safari plist 解析：从测试 fixture（XML plist）解析出书签。
    /// fixture 不存在则 skip（不 fail——开发环境可能未生成）。
    #[test]
    fn safari_plist_parsed_from_fixture() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/safari_bookmarks.plist");
        if !fixture.exists() {
            eprintln!("skip: fixture not found at {}", fixture.display());
            return;
        }
        let entries = load_safari_bookmarks(&fixture);
        // fixture 含 3 个 leaf 书签：GitHub + Rust + MDN Web Docs
        assert_eq!(entries.len(), 3, "应解析出 3 个书签，got: {:?}", entries.iter().map(|e| &e.title).collect::<Vec<_>>());
        assert!(entries.iter().all(|e| e.browser == "Safari"));
        assert!(entries.iter().all(|e| e.url.starts_with("http")), "URL 应是 http");
        // 加强：验证具体 title 集合
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"GitHub"), "应含 GitHub，got: {:?}", titles);
        assert!(titles.contains(&"Rust"), "应含 Rust，got: {:?}", titles);
        assert!(titles.contains(&"MDN Web Docs"), "应含 MDN Web Docs，got: {:?}", titles);
    }

    /// Firefox places.sqlite 查询：直接单测私有 `query_firefox_places`（绕开 home_dir 探测）。
    /// fixture 不存在则 skip。
    /// fixture 含 GitHub + Rust 两个真实书签 + 一个 place:% 内部 URL（应被过滤）。
    #[test]
    fn firefox_places_query_from_fixture() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/firefox_places.sqlite");
        if !fixture.exists() {
            eprintln!("skip: fixture not found at {}", fixture.display());
            return;
        }
        let entries = query_firefox_places(&fixture);
        // fixture 含 GitHub + Rust 两个真实书签 + 一个 place:% 内部 URL（type=1 但伪 URL）
        // 查询应过滤掉 place:%，只返回 2 个真实书签
        assert_eq!(entries.len(), 2, "应过滤 place:% 只返回 2 个书签，got: {:?}", entries.iter().map(|e| &e.title).collect::<Vec<_>>());
        assert!(entries.iter().all(|e| e.browser == "Firefox"));
        assert!(entries.iter().any(|e| e.title == "GitHub"), "应含 GitHub");
        assert!(entries.iter().any(|e| e.title == "Rust"), "应含 Rust");
        assert!(entries.iter().all(|e| e.url.starts_with("http")), "URL 应是 http");
    }

    #[test]
    fn parse_chromium_bookmarks_json() {
        // 模拟真实 Chromium Bookmarks 文件结构
        let json = r#"{
            "roots": {
                "bookmark_bar": {
                    "children": [
                        {"type": "url", "name": "GitHub", "url": "https://github.com"},
                        {"type": "folder", "name": "Dev", "children": [
                            {"type": "url", "name": "Rust", "url": "https://rust-lang.org"}
                        ]}
                    ]
                },
                "other": {
                    "children": [
                        {"type": "url", "name": "Google", "url": "https://google.com"}
                    ]
                },
                "synced": {
                    "children": []
                }
            }
        }"#;
        let path = std::env::temp_dir().join("test_bookmarks.json");
        std::fs::write(&path, json).unwrap();
        let entries = load_chromium_bookmarks("Chrome", &path);
        let _ = std::fs::remove_file(&path);

        // 应解析出 3 个书签（GitHub + Rust(嵌套) + Google）
        assert_eq!(entries.len(), 3, "expected 3 bookmarks, got {}: {:?}", entries.len(), entries.iter().map(|e| &e.title).collect::<Vec<_>>());
        assert!(entries.iter().any(|e| e.title == "GitHub"));
        assert!(entries.iter().any(|e| e.title == "Rust"));
        assert!(entries.iter().any(|e| e.title == "Google"));
    }
}
