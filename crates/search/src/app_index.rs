//! 应用索引：扫描 macOS /Applications/ 等目录。

use super::matcher::{match_score, Score};
use super::engine::SearchResult;

/// 应用索引入口。
pub struct AppEntry {
    pub name: String,
    pub path: String,
    /// 本地化别名（如 WeChat 的中文名"微信"），用于拼音/模糊匹配
    pub aliases: Vec<String>,
    /// base64 PNG 图标（32×32），空=无图标
    pub icon: String,
}

pub struct AppIndex {
    pub apps: Vec<AppEntry>,
}

/// 提取 .app 的图标为 base64 PNG（32×32）。
/// 用 sips 命令将 icns/png 转为 32×32 PNG，再 base64 编码。
/// sips 是 macOS 内置工具，无需额外依赖。
fn extract_app_icon(app_path: &std::path::Path) -> String {
    // 1. 从 Info.plist 读 CFBundleIconFile 获取 icon 文件名
    let info_plist = app_path.join("Contents/Info.plist");
    if !info_plist.exists() { return String::new(); }

    // 读 Info.plist 找 CFBundleIconFile
    let icon_name = {
        let output = std::process::Command::new("defaults")
            .arg("read")
            .arg(&info_plist)
            .arg("CFBundleIconFile")
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if name.is_empty() { return String::new(); }
                name
            }
            _ => return String::new(),
        }
    };

    // 2. 查找 icon 文件（可能带或不带 .icns/.png 后缀）
    let resources = app_path.join("Contents/Resources");
    let icon_path = {
        // 尝试原名 + 常见后缀
        let candidates = [
            resources.join(&icon_name),
            resources.join(format!("{}.icns", icon_name)),
            resources.join(format!("{}.png", icon_name)),
        ];
        candidates.into_iter().find(|p| p.exists())
    };
    let icon_path = match icon_path { Some(p) => p, None => return String::new() };

    // 3. 用 sips 转为 32×32 PNG（必须 -s format png，否则 icns 输出仍为 icns）
    // 文件名含 pid + 纳秒时间戳——防跨进程双开/同进程多次提取互相覆盖（L3）
    let tmp = std::env::temp_dir().join(format!(
        "octopus_icon_{}_{}.png",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
    ));
    let output = std::process::Command::new("sips")
        .args(["-s", "format", "png", "-z", "32", "32", "--out"])
        .arg(&tmp)
        .arg(&icon_path)
        .output();
    let png_bytes = match output {
        Ok(o) if o.status.success() => std::fs::read(&tmp).unwrap_or_default(),
        _ => {
            // sips 失败也清理临时文件（L3——原失败分支直接 return 不清理，残留 tmp）
            let _ = std::fs::remove_file(&tmp);
            return String::new();
        }
    };
    let _ = std::fs::remove_file(&tmp);

    if png_bytes.is_empty() { return String::new(); }
    use base64::Engine;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png_bytes))
}
/// 收集 app 的本地化别名（用于搜索）。
///
/// 借鉴 wox（app_darwin.go ParseAppInfo）的分层策略：
/// 1. **首选 mdls kMDItemDisplayName**——Spotlight 索引了系统级 locale 数据库，
///    覆盖最全（/System/Applications 下的 Preview→"预览"、Calculator→"计算器"等
///    系统 app 的本地化名不在 bundle .strings 里，只有 Spotlight 有）。
/// 2. **补充读 .strings**——第三方 app 的 zh-Hans/zh_CN 本地化名（微信→WeChat）。
///
/// 两个来源合并去重，都作为 alias 供搜索匹配。
fn read_localized_names(app_path: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    let stem = app_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    // 1. mdls kMDItemDisplayName（首选——覆盖系统 app）
    if let Some(display_name) = mdls_display_name(app_path) {
        if display_name != stem && !names.contains(&display_name) {
            names.push(display_name);
        }
    }

    // 2. .strings 补充（第三方 app 的 zh-Hans/zh_CN 本地化名）
    for locale in &["zh-Hans.lproj", "zh_CN.lproj"] {
        let plist = app_path.join("Contents/Resources").join(locale).join("InfoPlist.strings");
        if !plist.exists() { continue; }
        let bytes = match std::fs::read(&plist) { Ok(b) => b, Err(_) => continue };

        let content = decode_plist_string(&bytes);

        for key in &["CFBundleDisplayName", "CFBundleName"] {
            // OpenStep 格式: "CFBundleDisplayName" = "微信";
            let pattern_openstep = format!("\"{}\" = \"", key);
            if let Some(pos) = content.find(&pattern_openstep) {
                let rest = &content[pos + pattern_openstep.len()..];
                if let Some(end) = rest.find('"') {
                    let name = rest[..end].trim().to_string();
                    if !name.is_empty() && name != stem && !names.contains(&name) {
                        names.push(name);
                    }
                    continue;
                }
            }
            // XML plist 格式
            let pattern_xml_key = format!("<key>{}</key>", key);
            if let Some(pos) = content.find(&pattern_xml_key) {
                let after = &content[pos + pattern_xml_key.len()..];
                if let Some(start) = after.find("<string>") {
                    let rest = &after[start + 8..];
                    if let Some(end) = rest.find("</string>") {
                        let name = rest[..end].trim().to_string();
                        if !name.is_empty() && name != stem && !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
        }
    }

    names
}

/// 用 mdls 查 Spotlight 的 kMDItemDisplayName（macOS 系统级本地化显示名）。
/// 借鉴 wox app_darwin.go:170 getAppNameFromMdls——Spotlight 索引覆盖最全，
/// 含系统 app 的本地化名（Preview → "预览"），这些不在 bundle .strings 里。
#[cfg(target_os = "macos")]
fn mdls_display_name(app_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("mdls")
        .args(["-name", "kMDItemDisplayName", "-raw"])
        .arg(app_path)
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() || name == "(null)" || name == "null" { return None; }
    Some(name)
}

#[cfg(not(target_os = "macos"))]
fn mdls_display_name(_app_path: &std::path::Path) -> Option<String> {
    None
}

/// 解码 plist 文件内容——处理 UTF-16 LE/BE 和 UTF-8。
fn decode_plist_string(bytes: &[u8]) -> String {
    // BOM 检测
    if bytes.len() >= 2 {
        if bytes[0] == 0xFF && bytes[1] == 0xFE {
            // UTF-16 LE
            let u16s: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            return String::from_utf16_lossy(&u16s);
        }
        if bytes[0] == 0xFE && bytes[1] == 0xFF {
            // UTF-16 BE
            let u16s: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect();
            return String::from_utf16_lossy(&u16s);
        }
    }
    // UTF-8 或 Latin-1 fallback
    String::from_utf8_lossy(bytes).to_string()
}

impl AppIndex {
    /// 加载应用索引。优先从 DB 缓存读取（<1ms），缓存为空时扫文件系统并写入缓存。
    pub fn scan() -> Self {
        // 先试 DB 缓存
        if let Ok(cached) = octopus_infra::db::load_app_index() {
            // 缓存有效：icon 列存在且有值（旧缓存 icon 全空需重扫）
            let has_icons = cached.iter().any(|(_, _, _, icon)| !icon.is_empty());
            if !cached.is_empty() && has_icons {
                let apps: Vec<AppEntry> = cached
                    .into_iter()
                    .map(|(name, alias, path, icon)| AppEntry {
                        name,
                        aliases: if alias.is_empty() { vec![] } else { vec![alias] },
                        path,
                        icon,
                    })
                    .collect();
                log::info!("[search] 应用索引（DB 缓存）: {} 个应用", apps.len());
                return Self { apps };
            }
        }

        // DB 为空 → 扫文件系统
        let apps = Self::scan_filesystem();

        // 写入 DB 缓存
        let cache_data: Vec<(String, String, String, String)> = apps
            .iter()
            .map(|a| (a.name.clone(), a.aliases.first().cloned().unwrap_or_default(), a.path.clone(), a.icon.clone()))
            .collect();
        if let Err(e) = octopus_infra::db::save_app_index(&cache_data) {
            log::warn!("[search] 应用索引缓存写入失败: {}", e);
        }

        Self { apps }
    }

    /// 强制重新扫描文件系统并更新缓存。
    pub fn rescan() -> Self {
        let apps = Self::scan_filesystem();
        let cache_data: Vec<(String, String, String, String)> = apps
            .iter()
            .map(|a| (a.name.clone(), a.aliases.first().cloned().unwrap_or_default(), a.path.clone(), a.icon.clone()))
            .collect();
        if let Err(e) = octopus_infra::db::save_app_index(&cache_data) {
            log::warn!("[search] 应用索引缓存写入失败: {}", e);
        }
        Self { apps }
    }

    /// 扫描文件系统构建应用索引。
    fn scan_filesystem() -> Vec<AppEntry> {
        let mut apps = Vec::new();
        for dir in &[
            "/Applications",
            "/System/Applications",
            "/Applications/Utilities",
        ] {
            Self::scan_apps_dir(std::path::Path::new(dir), &mut apps, 0);
        }
        // 也扫 ~/Applications
        if let Some(home) = dirs::home_dir() {
            Self::scan_apps_dir(&home.join("Applications"), &mut apps, 0);
        }
        log::info!("[search] 应用索引（文件系统扫描）: {} 个应用", apps.len());
        // 去重：跨目录同名 app 只保留第一个（/Applications 优先于 ~/Applications）
        let mut seen = std::collections::HashSet::new();
        apps.retain(|a| seen.insert(a.name.clone()));
        apps
    }

    /// 递归扫描目录下的 .app（深度受限，不进入 .app 包内部）。
    fn scan_apps_dir(dir: &std::path::Path, apps: &mut Vec<AppEntry>, depth: u32) {
        const MAX_DEPTH: u32 = 2;
        if depth > MAX_DEPTH {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("app") {
                // .app 是 bundle（叶子），加入但不递归进包内部
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if !name.is_empty() {
                        let aliases = read_localized_names(&path);
                        let icon = extract_app_icon(&path);
                        apps.push(AppEntry {
                            name: name.to_string(),
                            path: path.to_string_lossy().to_string(),
                            aliases,
                            icon,
                        });
                    }
                }
            } else if path.is_dir() {
                // 普通子目录：递归（覆盖 /Applications/Adobe/Adobe Photoshop.app 等嵌套）
                Self::scan_apps_dir(&path, apps, depth + 1);
            }
        }
    }

    /// 搜索应用，返回匹配结果（已排序）。对 name + aliases 都匹配，取最高分。
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let mut scored: Vec<(Score, &AppEntry)> = self
            .apps
            .iter()
            .filter_map(|app| {
                // 主名匹配
                let mut best = match_score(query, &app.name);
                // 别名匹配（中文名等），取最高分
                for alias in &app.aliases {
                    if let Some(s) = match_score(query, alias) {
                        best = Some(best.map_or(s, |b| s.max(b)));
                    }
                }
                best.map(|s| (s, app))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(10)
            .map(|(score, app)| SearchResult {
                source: "app".into(),
                title: app.name.clone(),
                subtitle: app.path.clone(),
                icon: if app.icon.is_empty() { None } else { Some(app.icon.clone()) },
                action_type: "launch_app".into(),
                action_data: serde_json::json!({ "path": app.path }).to_string(),
                score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_matching_apps() {
        let index = AppIndex { apps: vec![
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![], icon: String::new() },
            AppEntry { name: "Safari".into(), path: "/Applications/Safari.app".into(), aliases: vec![], icon: String::new() },
        ]};
        let results = index.search("chr");
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Chrome");
        assert_eq!(results[0].source, "app");
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let index = AppIndex { apps: vec![
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![], icon: String::new() },
        ]};
        let results = index.search("");
        assert!(results.is_empty());
    }

    #[test]
    fn search_matches_alias() {
        // WeChat 英文名不匹配 wx，但别名“微信”的拼音首字母 wx 能匹配
        let index = AppIndex { apps: vec![
            AppEntry { name: "WeChat".into(), path: "/Applications/WeChat.app".into(), aliases: vec!["微信".into()], icon: String::new() },
        ]};
        let results = index.search("wx");
        assert!(!results.is_empty(), "wx should match WeChat via alias 微信");
        assert_eq!(results[0].title, "WeChat");
    }

    #[test]
    fn search_matches_alias_by_name() {
        // 直接搜别名也能匹配
        let index = AppIndex { apps: vec![
            AppEntry { name: "WeChat".into(), path: "/Applications/WeChat.app".into(), aliases: vec!["微信".into()], icon: String::new() },
        ]};
        let results = index.search("微信");
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "WeChat");
    }
}
