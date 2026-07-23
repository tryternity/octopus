//! CLI 命令索引：扫描 PATH 收集可执行文件 + whats/brew desc 英文描述 + DB 缓存。

pub struct CommandEntry {
    pub name: String,
    pub path: String,
    pub source: String,       // "brew" | "cargo" | "system" | "path"
    pub description: String,  // 英文（whatis/brew desc）
    pub keywords: String,     // LLM 生成的中英文关键字
}

pub struct CommandIndex {
    pub commands: Vec<CommandEntry>,
}

impl CommandIndex {
    /// 加载命令索引：扫 PATH + 填英文描述 + 读 DB 缓存 keywords。
    pub fn scan() -> Self {
        // 1. 扫 PATH 收集可执行文件
        let mut entries = scan_path();
        // 2. 去重（同名保留 PATH 靠前的）
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));
        // 3. 填英文描述：一次性 `apropos .` dump 全部 man 摘要建 map（O(1) 查询）。
        //    macOS `whatis <cmd>` 每次都要重建 man 索引查询，单次 5-8s，
        //    逐命令 spawn 会卡死；`apropos .` 一次 ~9s dump 全部条目（~15k 行），
        //    格式 "name(section) - description"，取 name==cmd 的第一条。
        let whats_map = build_whats_map();
        for e in &mut entries {
            if e.description.is_empty() {
                if let Some(d) = whats_map.get(e.name.as_str()) {
                    e.description = d.clone();
                }
            }
        }
        // brew 命令额外用 brew desc（更准）：一次批量查所有 formula（brew 启动
        // 开销 ~4-5s 固定，逐命令 spawn 会卡死）。先 brew list --formula 拿有效
        // formula 名，与 brew 命令匹配后一次 brew desc 批量取 desc。
        let brew_map = build_brew_map(entries.iter().filter(|e| e.source == "brew").map(|e| e.name.as_str()));
        for e in &mut entries {
            if e.source == "brew" {
                if let Some(d) = brew_map.get(e.name.as_str()) {
                    e.description = d.clone();
                }
            }
        }
        // 4. 读 DB 缓存的 keywords（launcher_index WHERE type='command'）
        let db_rows = octopus_infra::db::load_launcher_by_type("command").unwrap_or_default();
        let db_map: std::collections::HashMap<String, String> = db_rows.iter()
            .map(|r| (format!("{}|{}", r.name, r.path), r.keywords.clone()))
            .collect();
        for e in &mut entries {
            let key = format!("{}|{}", e.name, e.path);
            if let Some(kw) = db_map.get(&key) {
                e.keywords = kw.clone();
            }
        }
        // 5. 写 DB 缓存（全量替换 type='command'——PATH 变化时同步）
        let cache: Vec<octopus_infra::db::LauncherRow> = entries.iter()
            .map(|e| octopus_infra::db::LauncherRow {
                r#type: "command".into(), name: e.name.clone(), path: e.path.clone(),
                alias: String::new(), icon: String::new(),
                source: e.source.clone(), description: e.description.clone(),
                keywords: e.keywords.clone(),
                bundle_id: String::new(),  // command 无 bundle_id
            }).collect();
        let _ = octopus_infra::db::save_launcher_batch("command", &cache);
        log::info!("[search] 命令索引: {} 条", entries.len());
        Self { commands: entries }
    }

    /// 空索引（测试用）。
    pub fn empty() -> Self { Self { commands: vec![] } }
}

/// 扫描 PATH 收集可执行文件。
fn scan_path() -> Vec<CommandEntry> {
    let path_var = match std::env::var("PATH") { Ok(p) => p, Err(_) => return vec![] };
    let mut entries = vec![];
    for dir in path_var.split(':') {
        if dir.is_empty() { continue; }
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            // 跳过目录，只要可执行文件
            if !path.is_file() { continue; }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // 跳过隐藏文件（. 开头）
            if name.starts_with('.') { continue; }
            let source = classify_source(dir);
            entries.push(CommandEntry {
                name, path: path.to_string_lossy().to_string(),
                source: source.to_string(), description: String::new(), keywords: String::new(),
            });
        }
    }
    entries
}

/// 按 PATH 目录路径判定命令来源。
fn classify_source(dir: &str) -> &'static str {
    if dir.contains("/homebrew/") || dir.contains("/linuxbrew/") { "brew" }
    else if dir.contains(".cargo/bin") { "cargo" }
    else if matches!(dir, "/usr/bin" | "/bin" | "/usr/sbin" | "/sbin") { "system" }
    else { "path" }
}

/// 一次性 spawn `apropos .` dump 全部 man 摘要，建 name→description map。
///
/// macOS `whatis <cmd>` 每次查询都要扫 man 索引，单次 5-8s；逐命令 spawn 会
/// 卡死 scan（240 命令 × 5s = 20 分钟）。`apropos .` 一次 ~9s dump 全部条目
/// （~15k 行），格式 "name(section) - description"，按第一个空格前 name 建索引。
/// 失败（无 apropos / 无 man db）返回空 map，描述留空不影响主流程。
fn build_whats_map() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let output = std::process::Command::new("apropos").arg(".").output();
    let stdout = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return map,
    };
    for line in stdout.lines() {
        // 行格式 "name(section)    - description"；name 是 ( 前的部分。
        let name = match line.split('(').next() {
            Some(n) => n.trim(),
            None => continue,
        };
        if name.is_empty() || map.contains_key(name) { continue; }
        if let Some(idx) = line.find(" - ") {
            let desc = line[idx + 3..].trim();
            if !desc.is_empty() {
                map.insert(name.to_string(), desc.to_string());
            }
        }
    }
    map
}

/// 批量查 brew formula 描述，返回 命令名→desc map。
///
/// `brew desc` 单次 spawn ~4-5s 固定开销（Ruby 启动），逐命令 spawn 会卡死
/// （170 公式 × 4s = 11 分钟）。策略：
/// 1. `brew list --formula -1`（~0.1s）拿全部已装 formula 名；
/// 2. 与传入的 brew 命令名做前缀匹配（formula `git` 匹配命令 `git`/`git-cvsserver`），
///    只保留有对应 formula 的命令；
/// 3. 一次 `brew desc <matched formulas...>` 批量取 desc（~5s 固定）；
/// 4. 把 formula 的 desc 回填到该 formula 产生的所有命令名。
///
/// 任一步失败返回空 map，brew 命令描述回退到 whats 提供的。
fn build_brew_map<'a, I: IntoIterator<Item = &'a str>>(brew_cmds: I) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let brew_cmds: Vec<&str> = brew_cmds.into_iter().collect();
    if brew_cmds.is_empty() { return map; }

    // 1. 拿全部 formula 名。
    let formulas: Vec<String> = match std::process::Command::new("brew")
        .args(["list", "--formula", "-1"]).output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        _ => return map,
    };
    if formulas.is_empty() { return map; }

    // 2. 命令→formula 映射：命令名 == formula，或命令名以 "<formula>-" 开头。
    //    formula 按长度降序排，避免短 formula 误匹配（如 "go" 匹配 "gobject"）。
    let mut sorted = formulas.clone();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.len()));
    let mut cmd_to_formula: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for cmd in &brew_cmds {
        for f in &sorted {
            if *cmd == f.as_str() || cmd.starts_with(&format!("{}-", f)) {
                cmd_to_formula.insert(cmd, f.clone());
                break;
            }
        }
    }
    if cmd_to_formula.is_empty() { return map; }

    // 3. 一次批量 brew desc。
    let unique_formulas: Vec<&str> = {
        let mut s: Vec<&str> = cmd_to_formula.values().map(|s| s.as_str()).collect();
        s.sort(); s.dedup(); s
    };
    let output = std::process::Command::new("brew")
        .arg("desc").args(&unique_formulas).output();
    let stdout = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return map,
    };
    let mut formula_desc: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in stdout.lines() {
        // "formula_name: description"
        if let Some(idx) = line.find(": ") {
            let (name, desc) = line.split_at(idx);
            formula_desc.insert(name.trim().to_string(), desc[2..].trim().to_string());
        }
    }

    // 4. 回填：每个 brew 命令映射到 formula 的 desc。
    for (cmd, formula) in cmd_to_formula {
        if let Some(d) = formula_desc.get(&formula) {
            map.insert(cmd.to_string(), d.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_source_paths() {
        assert_eq!(classify_source("/opt/homebrew/bin"), "brew");
        assert_eq!(classify_source("/Users/me/.cargo/bin"), "cargo");
        assert_eq!(classify_source("/usr/bin"), "system");
        assert_eq!(classify_source("/usr/local/bin"), "path");
    }

    #[test]
    fn scan_path_returns_executables() {
        // PATH 至少含 /usr/bin，必有 ls/cat 等
        let idx = CommandIndex::scan();
        assert!(!idx.commands.is_empty(), "PATH 扫描应有结果");
        assert!(idx.commands.iter().any(|c| c.name == "ls" || c.name == "cat"),
            "应含常见命令，got: {:?}", idx.commands.iter().take(5).map(|c| &c.name).collect::<Vec<_>>());
    }

    #[test]
    fn dedup_keeps_path_first() {
        // 同名命令保留 PATH 靠前的
        let mut entries = vec![
            CommandEntry { name: "python3".into(), path: "/usr/bin/python3".into(), source: "system".into(), description: "".into(), keywords: "".into() },
            CommandEntry { name: "python3".into(), path: "/opt/homebrew/bin/python3".into(), source: "brew".into(), description: "".into(), keywords: "".into() },
        ];
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.name.clone()));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/usr/bin/python3");
    }
}
