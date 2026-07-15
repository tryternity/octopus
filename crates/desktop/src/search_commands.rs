//! 搜索相关 Tauri 命令。

use crate::search::{self, SearchResult};

/// 综合搜索。
#[tauri::command]
pub async fn search_all(query: String, tab: String) -> Result<Vec<SearchResult>, String> {
    let engine = match search::get_engine() {
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

/// 执行 Shell 命令。
#[tauri::command]
pub async fn execute_shell(command: String) -> Result<String, String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(format!("exit: {}\n{}", output.status, stderr));
    }
    Ok(if stdout.is_empty() { stderr } else { stdout })
}
