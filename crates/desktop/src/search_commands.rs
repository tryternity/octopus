//! 搜索相关 Tauri 命令。

use octopus_search::{self, SearchResult};

/// 综合搜索。
#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<Vec<SearchResult>, String> {
    let engine = match octopus_search::get_engine() {
        Some(e) => e,
        None => return Err("搜索引擎未初始化".into()),
    };
    Ok(engine.search(&query, &tab).await)
}

/// 启动应用。
#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 打开文件。
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 打开 URL（默认浏览器）。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 执行 Shell 命令（30s 超时，输出限制 100KB）。
#[tauri::command]
pub async fn execute_shell(command: String) -> Result<String, String> {
    let fut = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output();
    // 30 秒超时，防止挂起
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
        .await
        .map_err(|_| format!("Shell 命令超时（30s）: {}", command))?
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(format!("exit: {}\n{}", output.status, stderr));
    }
    let result = if stdout.is_empty() { stderr } else { stdout };
    // 输出限制 100KB，防止 OOM
    const MAX_OUTPUT: usize = 100 * 1024;
    if result.len() > MAX_OUTPUT {
        Ok(format!("{}...（输出已截断，共 {} 字节）", &result[..MAX_OUTPUT], result.len()))
    } else {
        Ok(result)
    }
}
