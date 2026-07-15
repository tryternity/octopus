//! 搜索相关 Tauri 命令。

use octopus_search::{self, SearchResult};

/// 综合搜索。
#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<Vec<SearchResult>, String> {
    let query = query.trim().to_string();
    let engine = match octopus_search::get_engine() {
        Some(e) => e,
        None => return Err("搜索引擎未初始化".into()),
    };
    Ok(engine.search(&query, &tab).await)
}

/// 启动应用。用 status() 替代 spawn() 防僵尸进程（open 命令本身很快退出）。
#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 打开文件。
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 打开 URL（默认浏览器）。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&url)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 执行 Shell 命令（30s 超时，输出限制 100KB）。
#[tauri::command]
pub async fn execute_shell(command: String) -> Result<String, String> {
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .kill_on_drop(true) // 超时 drop future 时杀子进程，防孤儿
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    // 30 秒超时，防止挂起
    let output = match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => return Err(format!("Shell 命令超时（30s）: {}", command)),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(format!("exit: {}\n{}", output.status, stderr));
    }
    let result = if stdout.is_empty() { stderr } else { stdout };
    // 输出限制 100KB（按字符截断，不 panic on non-ASCII boundary）
    const MAX_OUTPUT_CHARS: usize = 100 * 1024;
    if result.chars().count() > MAX_OUTPUT_CHARS {
        let truncated: String = result.chars().take(MAX_OUTPUT_CHARS).collect();
        Ok(format!("{}...（输出已截断）", truncated))
    } else {
        Ok(result)
    }
}

/// 重新扫描应用索引并更新 DB 缓存（安装/卸载应用后调用）。
#[tauri::command]
pub fn reindex_apps() -> Result<usize, String> {
    let index = octopus_search::app_index::AppIndex::rescan();
    Ok(index.apps.len())
}
