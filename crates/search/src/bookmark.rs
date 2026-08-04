//! 浏览器书签解析：Safari / Chrome / Edge。

use super::matcher::{match_score, Score};
use super::engine::SearchResult;

pub struct BookmarkEntry {
    pub title: String,
    pub url: String,
    pub browser: String,
}

/// 加载所有浏览器的书签：Chrome / Edge（JSON，扫描全部 profile）+ Safari（plist）+ Firefox（SQLite）。
///
/// 每个浏览器的 loader 自带降级（无权限/无文件返回空 Vec），这里只做存在性预检
/// 跳过明显不存在的路径以省 syscalls，不掩盖 loader 内部错误。
///
/// **多 profile 支持**：Chrome/Edge 登录多 Google 账号或从旧设备迁移时，profile 目录
/// 可能是 `Default` / `Profile 1` / `Profile 2` 等（非只有 `Default`）。扫 User Data 下
/// 所有含 `Bookmarks` 文件的 profile 目录，合并全部书签（按 url 去重）。
pub fn load_all_bookmarks() -> Vec<BookmarkEntry> {
    let mut bookmarks = Vec::new();
    if let Some(home) = dirs::home_dir() {
        // Chrome / Edge（JSON）——扫描全部 profile
        for (browser, user_data_rel) in &[
            ("Chrome", "Library/Application Support/Google/Chrome"),
            ("Edge", "Library/Application Support/Microsoft Edge"),
        ] {
            let user_data = home.join(user_data_rel);
            if user_data.is_dir() {
                bookmarks.extend(load_chromium_all_profiles(browser, &user_data));
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
    // 按 url 去重（跨 profile / 跨浏览器可能产出同 URL，保留首个）
    let mut seen = std::collections::HashSet::new();
    bookmarks.retain(|b| seen.insert(b.url.clone()));
    log::info!("[search] 书签索引: {} 条", bookmarks.len());
    bookmarks
}

/// 扫描 Chromium User Data 下所有 profile 目录的 Bookmarks 文件。
/// profile 目录：`Default` / `Profile 1` / `Profile 2` / ...（Chrome 多账号场景）。
/// 跳过 `Guest Profile` / `System Profile`（无用户书签）。
fn load_chromium_all_profiles(browser: &str, user_data: &std::path::Path) -> Vec<BookmarkEntry> {
    let mut all = Vec::new();
    let entries = match std::fs::read_dir(user_data) {
        Ok(e) => e,
        Err(_) => return all,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // 只处理目录（profile 都是目录）
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // 跳过系统/访客 profile（无用户书签）
        if name_str == "Guest Profile" || name_str == "System Profile" || name_str == "Snapshots" {
            continue;
        }
        let bookmarks_file = path.join("Bookmarks");
        if bookmarks_file.is_file() {
            all.extend(load_chromium_bookmarks(browser, &bookmarks_file));
        }
    }
    all
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

/// 搜索书签。匹配 title + URL（URL 剥掉协议/域名后缀噪声词后匹配）。
pub fn search_bookmarks(query: &str, bookmarks: &[BookmarkEntry]) -> Vec<SearchResult> {
    let mut scored: Vec<(Score, String, SearchResult)> = bookmarks
        .iter()
        .filter_map(|bm| {
            // 优先匹配 title；title 未命中则匹配清洗后的 url（去掉噪声词）
            let score = match_score(query, &bm.title)
                .or_else(|| {
                    let clean_url = strip_url_noise(&bm.url);
                    if clean_url.is_empty() { return None; }
                    match_score(query, &clean_url)
                })?;
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
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));
    // 按 url 去重（bookmark_bar + synced 同步可能产出同 URL，保留高分首个）
    let mut seen = std::collections::HashSet::new();
    scored.into_iter()
        .filter(|(_, url, _)| seen.insert(url.clone()))
        .take(10)
        .map(|(s, _, mut r)| { r.score = s; r })
        .collect()
}

/// 剥掉 URL 的噪声部分用于匹配：
/// 去协议（http:// https://）、去 www.、去域名后缀（.com .net .org .cn .io 等）。
/// 保留域名主体 + 路径——这些才是用户会搜的有意义部分。
/// 例："https://www.github.com/torvalds/linux" → "github/torvalds/linux"
fn strip_url_noise(url: &str) -> String {
    // 去协议
    let no_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // 去 www.
    let no_www = no_scheme.strip_prefix("www.").unwrap_or(no_scheme);
    // 按第一个 / 分割域名和路径
    if let Some(slash_pos) = no_www.find('/') {
        let domain = &no_www[..slash_pos];
        let path = &no_www[slash_pos + 1..];
        // 域名剥后缀（取主体，去 .com/.net/.org/.cn/.io/.dev 等）
        let domain_core = strip_domain_suffix(domain);
        if domain_core.is_empty() && path.is_empty() {
            return String::new();
        }
        format!("{}/{}", domain_core, path.trim_end_matches('/'))
    } else {
        // 纯域名无路径
        strip_domain_suffix(no_www)
    }
}

/// 域名剥掉后缀，保留主体部分。
/// "github.com" → "github"，"api.example.co.jp" → "api.example"
fn strip_domain_suffix(domain: &str) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    match parts.len() {
        0 | 1 => domain.to_string(),
        2 => parts[0].to_string(),  // github.com → github
        _ => {
            // 多段域名（api.example.co.jp）——去掉最后 1-2 段（TLD + ccTLD）
            // 保留前 N-2 段（.co.jp / .com.cn 等双段 TLD 取 N-2，单段 TLD 也取 N-2）
            parts[..parts.len() - 2].join(".")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_url_noise_basic() {
        assert_eq!(strip_url_noise("https://github.com"), "github");
        assert_eq!(strip_url_noise("http://google.com"), "google");
        assert_eq!(strip_url_noise("https://www.example.com"), "example");
        assert_eq!(strip_url_noise("https://github.com/torvalds/linux"), "github/torvalds/linux");
    }

    #[test]
    fn strip_url_noise_removes_tld() {
        assert_eq!(strip_url_noise("https://rust-lang.org"), "rust-lang");
        assert_eq!(strip_url_noise("https://nodejs.org/docs"), "nodejs/docs");
        assert_eq!(strip_url_noise("https://example.io"), "example");
        assert_eq!(strip_url_noise("https://example.dev/api/v1"), "example/api/v1");
    }

    #[test]
    fn strip_url_noise_multi_segment_domain() {
        // 多段域名保留主体，去 ccTLD
        assert_eq!(strip_url_noise("https://api.example.co.jp/users"), "api.example/users");
        assert_eq!(strip_url_noise("https://baidu.com"), "baidu");
    }

    #[test]
    fn strip_url_noise_protocol_relative() {
        // 无协议的 URL
        assert_eq!(strip_url_noise("github.com/ruanyf"), "github/ruanyf");
        assert_eq!(strip_url_noise("www.google.com/search"), "google/search");
    }

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

    /// 噪声词（http/https/www/com/net/org）不该触发误匹配。
    /// 这些在 URL 里到处都是，如果参与匹配会命中几乎所有书签。
    #[test]
    fn search_bookmarks_noise_words_dont_match_url() {
        let bookmarks = vec![
            BookmarkEntry { title: "GitHub".into(), url: "https://github.com".into(), browser: "Chrome".into() },
            BookmarkEntry { title: "Rust".into(), url: "https://rust-lang.org".into(), browser: "Chrome".into() },
        ];
        // "https" 不该匹配——它被 strip 掉了，title 里也没有
        let results = search_bookmarks("https", &bookmarks);
        assert!(results.is_empty(), "搜 'https' 不该命中（噪声词已 strip）");
        // "com" 不该匹配——域名后缀已 strip
        let results = search_bookmarks("com", &bookmarks);
        assert!(results.is_empty(), "搜 'com' 不该命中（TLD 已 strip）");
    }

    /// URL 路径里的有意义部分仍能匹配。
    #[test]
    fn search_bookmarks_url_path_matches() {
        let bookmarks = vec![
            BookmarkEntry {
                title: "某项目".into(),
                url: "https://github.com/torvalds/linux".into(),
                browser: "Chrome".into(),
            },
        ];
        // 标题"某项目"不含 "linux"，但 URL 路径含
        let results = search_bookmarks("linux", &bookmarks);
        assert!(!results.is_empty(), "搜 'linux' 应通过 URL 路径命中");
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

    /// 多 profile 扫描：Chrome 多账号场景下书签在 `Profile 1` 而非 `Default`。
    /// 验证 load_chromium_all_profiles 扫描所有 profile 目录 + 跳过 Guest/System。
    #[test]
    fn chromium_multiple_profiles_scanned() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("octopus_bm_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // 模拟 User Data 目录结构
        let default = tmp.join("Default");
        let profile1 = tmp.join("Profile 1");
        let guest = tmp.join("Guest Profile");
        fs::create_dir_all(&default).unwrap();
        fs::create_dir_all(&profile1).unwrap();
        fs::create_dir_all(&guest).unwrap();

        // Default profile：GitHub
        fs::write(default.join("Bookmarks"), r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"GitHub","url":"https://github.com"}]},"other":{"children":[]},"synced":{"children":[]}}}"#).unwrap();
        // Profile 1：Rust（用户的真实场景——Profile 1 是主 profile）
        fs::write(profile1.join("Bookmarks"), r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"Rust","url":"https://rust-lang.org"}]},"other":{"children":[]},"synced":{"children":[]}}}"#).unwrap();
        // Guest Profile：应被跳过
        fs::write(guest.join("Bookmarks"), r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"GuestOnly","url":"https://guest.example.com"}]},"other":{"children":[]},"synced":{"children":[]}}}"#).unwrap();

        let entries = load_chromium_all_profiles("Chrome", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        // 应扫到 Default + Profile 1 的 2 个书签，跳过 Guest
        assert_eq!(entries.len(), 2, "应扫描 Default + Profile 1，跳过 Guest，got: {:?}", entries.iter().map(|e| &e.title).collect::<Vec<_>>());
        assert!(entries.iter().any(|e| e.title == "GitHub"), "Default 的 GitHub 应在");
        assert!(entries.iter().any(|e| e.title == "Rust"), "Profile 1 的 Rust 应在（用户主 profile）");
        assert!(entries.iter().all(|e| e.title != "GuestOnly"), "Guest Profile 应被跳过");
    }

    /// 只有非 Default profile（Profile 1）时也应扫到——这是用户报告的真实场景。
    #[test]
    fn chromium_only_profile1_no_default() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("octopus_bm_test_p1_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // 只有 Profile 1，没有 Default（用户实际场景）
        let profile1 = tmp.join("Profile 1");
        fs::create_dir_all(&profile1).unwrap();
        fs::write(profile1.join("Bookmarks"), r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"MyBookmark","url":"https://example.com"}]},"other":{"children":[]},"synced":{"children":[]}}}"#).unwrap();

        let entries = load_chromium_all_profiles("Chrome", &tmp);
        let _ = fs::remove_dir_all(&tmp);

        assert_eq!(entries.len(), 1, "只有 Profile 1 也应扫到书签");
        assert_eq!(entries[0].title, "MyBookmark");
    }
}
